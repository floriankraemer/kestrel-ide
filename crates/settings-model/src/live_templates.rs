//! The live templates in force: plugin-contributed ones with the user's
//! `[[live_template]]` rows layered on top (ADR-0072).
//!
//! A user row replaces the plugin template with the same
//! `(language, abbreviation, postfix)` and otherwise adds a new one. A row
//! with no abbreviation, body or language is ignored rather than failing the
//! load.

use app_config::LiveTemplateSetting;
use edit_ops::indent::IndentStyle;
use edit_ops::templates::{self, Expansion, Prepared, Site};
use plugin_api::{LiveTemplateContribution, TemplateContext};
use syntax_core::Language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTemplate {
    pub language: String,
    pub abbreviation: String,
    pub description: String,
    pub body: String,
    pub postfix: bool,
    pub context: TemplateContext,
}

impl LiveTemplate {
    /// Whether the template may be offered at `site`.
    pub fn fits(&self, site: Site) -> bool {
        match self.context {
            TemplateContext::Any => true,
            TemplateContext::Statement => site == Site::Statement,
            TemplateContext::Expression => site == Site::Expression,
            TemplateContext::Class => site == Site::ClassBody,
        }
    }

    fn same_slot(&self, other: &LiveTemplate) -> bool {
        self.language == other.language
            && self.abbreviation == other.abbreviation
            && self.postfix == other.postfix
    }
}

impl From<&LiveTemplateContribution> for LiveTemplate {
    fn from(c: &LiveTemplateContribution) -> Self {
        Self {
            language: c.language.clone(),
            abbreviation: c.abbreviation.clone(),
            description: c.description.clone(),
            body: c.body.clone(),
            postfix: c.postfix,
            context: c.context,
        }
    }
}

fn parse_context(raw: Option<&str>) -> TemplateContext {
    match raw {
        Some("statement") => TemplateContext::Statement,
        Some("expression") => TemplateContext::Expression,
        Some("class") => TemplateContext::Class,
        _ => TemplateContext::Any,
    }
}

fn from_setting(row: &LiveTemplateSetting) -> Option<LiveTemplate> {
    let usable = !row.language.is_empty() && !row.abbreviation.is_empty() && !row.body.is_empty();
    usable.then(|| LiveTemplate {
        language: row.language.clone(),
        abbreviation: row.abbreviation.clone(),
        description: if row.description.is_empty() {
            row.abbreviation.clone()
        } else {
            row.description.clone()
        },
        body: row.body.clone(),
        postfix: row.postfix,
        context: parse_context(row.context.as_deref()),
    })
}

/// Plugin templates first, user rows overriding or extending them.
pub fn resolve<'a>(
    plugin: impl IntoIterator<Item = &'a LiveTemplateContribution>,
    user: &[LiveTemplateSetting],
) -> Vec<LiveTemplate> {
    let mut all: Vec<LiveTemplate> = plugin.into_iter().map(LiveTemplate::from).collect();
    for row in user.iter().filter_map(from_setting) {
        match all.iter_mut().find(|t| t.same_slot(&row)) {
            Some(slot) => *slot = row,
            None => all.push(row),
        }
    }
    all
}

/// The templates of one language, abbreviation order (what the Insert Live
/// Template list shows).
pub fn for_language<'a>(all: &'a [LiveTemplate], language: &str) -> Vec<&'a LiveTemplate> {
    let mut found: Vec<_> = all.iter().filter(|t| t.language == language).collect();
    found.sort_by(|a, b| a.abbreviation.cmp(&b.abbreviation));
    found
}

/// What Tab does at `caret`: the postfix template named after a dot, else
/// the plain template named by the word before the caret that fits where it
/// stands. `None` means Tab keeps its ordinary meaning.
pub fn expand_before_caret(
    all: &[LiveTemplate],
    language: Language,
    text: &str,
    caret: usize,
    style: IndentStyle,
) -> Option<Expansion> {
    let id = language.id();
    let named = |abbreviation: &str, postfix: bool| {
        all.iter()
            .find(|t| t.postfix == postfix && t.language == id && t.abbreviation == abbreviation)
    };
    if let Some(site) = templates::postfix_site(language, text, caret) {
        if let Some(t) = named(&text[site.abbreviation.clone()], true) {
            return Some(templates::postfix(text, &site, &t.body, style));
        }
    }
    let word = templates::abbreviation_before(text, caret)?;
    let template = named(&text[word.clone()], false)?;
    let site = templates::site_at(language, text, word.clone());
    template
        .fits(site)
        .then(|| templates::expand(text, word, &template.body, style))
}

/// The plain templates the Insert Live Template list offers at `caret`.
pub fn insertable<'a>(
    all: &'a [LiveTemplate],
    language: Language,
    text: &str,
    caret: usize,
) -> Vec<&'a LiveTemplate> {
    let word = templates::word_before(text, caret).unwrap_or(caret..caret);
    let site = templates::site_at(language, text, word);
    for_language(all, &language.id())
        .into_iter()
        .filter(|t| !t.postfix && t.fits(site))
        .collect()
}

/// Insert the plain template `abbreviation` at `caret`, replacing the
/// half-typed word before it when that is a start of the abbreviation.
pub fn insert(
    all: &[LiveTemplate],
    language: Language,
    text: &str,
    caret: usize,
    abbreviation: &str,
    style: IndentStyle,
) -> Option<Expansion> {
    let id = language.id();
    let template = all
        .iter()
        .find(|t| !t.postfix && t.language == id && t.abbreviation == abbreviation)?;
    let word = templates::word_before(text, caret)
        .filter(|w| abbreviation.starts_with(&text[w.clone()]))
        .unwrap_or(caret..caret);
    Some(templates::expand(text, word, &template.body, style))
}

/// The templates Surround With offers: the ones that wrap a selection.
pub fn surround_candidates(all: &[LiveTemplate], language: Language) -> Vec<&LiveTemplate> {
    for_language(all, &language.id())
        .into_iter()
        .filter(|t| !t.postfix && t.body.contains(templates::SELECTION_VAR))
        .collect()
}

/// Wrap `selection` in the template `abbreviation`.
pub fn surround_with(
    all: &[LiveTemplate],
    language: Language,
    text: &str,
    selection: std::ops::Range<usize>,
    abbreviation: &str,
    style: IndentStyle,
) -> Option<Expansion> {
    let template = surround_candidates(all, language)
        .into_iter()
        .find(|t| t.abbreviation == abbreviation)?;
    Some(templates::surround(text, selection, &template.body, style))
}

/// One template as a completion item: shown as `abbreviation`, and accepted
/// by replacing `prepared.range` with `prepared.source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateOffer {
    pub abbreviation: String,
    pub description: String,
    pub prepared: Prepared,
}

impl TemplateOffer {
    fn new(template: &LiveTemplate, prepared: Prepared) -> Self {
        Self {
            abbreviation: template.abbreviation.clone(),
            description: template.description.clone(),
            prepared,
        }
    }
}

/// The templates the completion popup lists at `caret`.
///
/// After `expr.`: the postfix templates whose abbreviation starts with what
/// follows the dot. Elsewhere: only the plain template whose abbreviation is
/// exactly the word typed, first, so that Tab or Enter on `fore` expands it
/// instead of accepting the keyword `foreach`. Listing every plain template
/// by prefix would bury the language's own keywords in every popup; Ctrl+J
/// lists them all.
pub fn completions(
    all: &[LiveTemplate],
    language: Language,
    text: &str,
    caret: usize,
    style: IndentStyle,
) -> Vec<TemplateOffer> {
    let id = language.id();
    if let Some(site) = templates::postfix_site(language, text, caret) {
        let typed = text[site.abbreviation.clone()].to_lowercase();
        return for_language(all, &id)
            .into_iter()
            .filter(|t| t.postfix && t.abbreviation.to_lowercase().starts_with(&typed))
            .map(|t| TemplateOffer::new(t, templates::prepare_postfix(text, &site, &t.body, style)))
            .collect();
    }
    let Some(word) = templates::abbreviation_before(text, caret) else {
        return Vec::new();
    };
    let typed = &text[word.clone()];
    let site = templates::site_at(language, text, word.clone());
    all.iter()
        .find(|t| !t.postfix && t.language == id && t.abbreviation == typed && t.fits(site))
        .map(|t| TemplateOffer::new(t, templates::prepare_expand(text, word, &t.body, style)))
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(abbr: &str, body: &str) -> LiveTemplateContribution {
        LiveTemplateContribution {
            language: "php".into(),
            abbreviation: abbr.into(),
            description: "d".into(),
            body: body.into(),
            postfix: false,
            context: TemplateContext::Any,
        }
    }

    fn row(abbr: &str, body: &str) -> LiveTemplateSetting {
        LiveTemplateSetting {
            language: "php".into(),
            abbreviation: abbr.into(),
            body: body.into(),
            ..LiveTemplateSetting::default()
        }
    }

    #[test]
    fn a_user_row_overrides_the_plugin_template_in_place() {
        let plugin = [plugin("if", "old"), plugin("fn", "f")];
        let all = resolve(&plugin, &[row("if", "new")]);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].body, "new");
        assert_eq!(all[0].description, "if");
    }

    #[test]
    fn a_user_row_with_a_new_abbreviation_extends() {
        let all = resolve(&[plugin("if", "x")], &[row("pr", "print_r($1);")]);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].abbreviation, "pr");
    }

    #[test]
    fn a_postfix_row_does_not_override_the_plain_template() {
        let mut postfix = row("if", "if ($EXPR$) {}");
        postfix.postfix = true;
        assert_eq!(resolve(&[plugin("if", "x")], &[postfix]).len(), 2);
    }

    #[test]
    fn unusable_rows_and_unknown_contexts_are_tolerated() {
        let mut odd = row("a", "b");
        odd.context = Some("nonsense".into());
        let all = resolve(&[], &[row("", "b"), row("a", ""), odd]);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].context, TemplateContext::Any);
    }

    #[test]
    fn context_decides_where_a_template_fits() {
        let mut t = LiveTemplate::from(&plugin("a", "b"));
        assert!(t.fits(Site::Other));
        t.context = TemplateContext::Statement;
        assert!(t.fits(Site::Statement) && !t.fits(Site::Expression));
        t.context = TemplateContext::Class;
        assert!(t.fits(Site::ClassBody) && !t.fits(Site::Statement));
    }

    #[test]
    fn the_two_crates_spell_the_variables_alike() {
        assert_eq!(
            edit_ops::templates::SELECTION_VAR,
            plugin_api::SELECTION_VAR
        );
        assert_eq!(edit_ops::templates::EXPR_VAR, plugin_api::EXPR_VAR);
    }

    #[test]
    fn for_language_filters_and_sorts() {
        let mut other = plugin("zz", "z");
        other.language = "rust".into();
        let all = resolve(&[plugin("b", "1"), plugin("a", "2"), other], &[]);
        let php: Vec<_> = for_language(&all, "php")
            .iter()
            .map(|t| t.abbreviation.as_str())
            .collect();
        assert_eq!(php, ["a", "b"]);
    }

    fn shipped_php() -> Vec<LiveTemplate> {
        let php_tools: Vec<_> = plugin_host::BUILTIN_PLUGINS
            .iter()
            .copied()
            .filter(|b| b.manifest.contains("id = \"php-tools\""))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let registry = plugin_host::load(dir.path(), &php_tools, &[]);
        assert!(registry.errors().is_empty(), "{:?}", registry.errors());
        let all = resolve(registry.live_templates().map(|(_, t)| t), &[]);
        for_language(&all, "php").into_iter().cloned().collect()
    }

    #[test]
    fn every_shipped_php_template_expands_to_clean_code() {
        use edit_ops::indent::IndentStyle;
        use edit_ops::templates::{expand, postfix, surround, PostfixSite};
        let style = IndentStyle::default();
        let templates = shipped_php();
        assert_eq!(templates.iter().filter(|t| !t.postfix).count(), 14);
        assert_eq!(templates.iter().filter(|t| t.postfix).count(), 9);
        for t in &templates {
            let text = "$xs";
            let e = if t.postfix {
                let site = PostfixSite {
                    abbreviation: 4..4,
                    expr: 0..3,
                };
                postfix("$xs.", &site, &t.body, style)
            } else {
                expand("", 0..0, &t.body, style)
            };
            let out = &e.edit.text;
            assert!(
                !out.contains("$SELECTION$") && !out.contains("$EXPR$"),
                "{out}"
            );
            assert!(
                !out.contains("\\$"),
                "an unescaped dollar leaked in {}: {out}",
                t.abbreviation
            );
            if t.postfix {
                assert!(out.contains(text), "{out}");
            }
        }
        let fore = templates.iter().find(|t| t.abbreviation == "fore").unwrap();
        let e = surround("a();", 0..4, &fore.body, style);
        assert_eq!(e.edit.text, "foreach ($array as $item) {\n    a();\n}");
    }

    fn lang() -> Language {
        syntax_core::language_by_id("php").unwrap()
    }

    #[test]
    fn tab_prefers_a_postfix_template_then_a_plain_one_that_fits() {
        let all = shipped_php();
        let style = IndentStyle::default();
        let text = "<?php\nfunction f() {\n    fore\n}\n";
        let caret = text.find("fore").unwrap() + 4;
        let e = expand_before_caret(&all, lang(), text, caret, style).unwrap();
        assert!(e.edit.text.starts_with("foreach ("));

        // `fore` as a variable name or inside an expression is left alone.
        let expr = "<?php\n$x = fore;\n";
        let caret = expr.find("fore").unwrap() + 4;
        assert!(expand_before_caret(&all, lang(), expr, caret, style).is_none());

        let post = "<?php\nfunction f() {\n    $xs.foreach\n}\n";
        let caret = post.find(".foreach").unwrap() + 8;
        let e = expand_before_caret(&all, lang(), post, caret, style).unwrap();
        assert!(
            e.edit.text.starts_with("foreach ($xs as "),
            "{}",
            e.edit.text
        );
    }

    #[test]
    fn insert_lists_what_fits_and_replaces_a_typed_prefix() {
        let all = shipped_php();
        let style = IndentStyle::default();
        let text = "<?php\nclass A {\n    pu\n}\n";
        let caret = text.find("pu").unwrap() + 2;
        let names: Vec<_> = insertable(&all, lang(), text, caret)
            .iter()
            .map(|t| t.abbreviation.as_str())
            .collect();
        assert!(names.contains(&"pubf") && names.contains(&"ctor"));
        assert!(!names.contains(&"fore"));
        let e = insert(&all, lang(), text, caret, "pubf", style).unwrap();
        assert_eq!(e.edit.range, caret - 2..caret);
        assert!(e.edit.text.starts_with("public function "));
    }

    #[test]
    fn surround_offers_the_wrapping_templates_only() {
        let all = shipped_php();
        let names: Vec<_> = surround_candidates(&all, lang())
            .iter()
            .map(|t| t.abbreviation.as_str())
            .collect();
        assert_eq!(names, ["fore", "forek", "if", "ife", "try"]);
        let e = surround_with(&all, lang(), "a();", 0..4, "if", IndentStyle::default()).unwrap();
        assert_eq!(e.edit.text, "if (condition) {\n    a();\n}");
        assert!(surround_with(&all, lang(), "a();", 0..4, "fn", IndentStyle::default()).is_none());
    }

    #[test]
    fn completions_list_postfix_templates_after_a_dot_and_the_exact_plain_one_elsewhere() {
        let all = shipped_php();
        let text = "<?php\n$xs.is";
        let offers = completions(&all, lang(), text, text.len(), IndentStyle::default());
        let names: Vec<_> = offers.iter().map(|o| o.abbreviation.as_str()).collect();
        assert_eq!(names, ["isset"]);
        assert_eq!(&text[offers[0].prepared.range.clone()], "$xs.is");
        let after_dot = "<?php\n$xs.";
        assert_eq!(
            completions(
                &all,
                lang(),
                after_dot,
                after_dot.len(),
                IndentStyle::default()
            )
            .len(),
            9
        );
        assert!(completions(&all, lang(), "<?php\n$xs", 9, IndentStyle::default()).is_empty());
    }
}
