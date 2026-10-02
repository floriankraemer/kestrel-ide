//! The live templates in force: plugin-contributed ones with the user's
//! `[[live_template]]` rows layered on top (ADR-0072).
//!
//! A user row replaces the plugin template with the same
//! `(language, abbreviation, postfix)` and otherwise adds a new one. A row
//! with no abbreviation, body or language is ignored rather than failing the
//! load.

use app_config::LiveTemplateSetting;
use edit_ops::templates::Site;
use plugin_api::{LiveTemplateContribution, TemplateContext};

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
}
