//! The `live-templates` contribution point (ADR-0072): abbreviation,
//! surround and postfix snippets a plugin offers for a language.
//!
//! Split out of `mod.rs` to keep that file under the file-size ceiling.

use serde::Deserialize;

use super::non_empty;
use crate::error::LoadErrorKind;

/// Placeholder a surround template puts where the selected text goes.
pub const SELECTION_VAR: &str = "$SELECTION$";
/// Placeholder a postfix template puts where the expression before the dot goes.
pub const EXPR_VAR: &str = "$EXPR$";

/// Where in the code a template may be offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateContext {
    /// Anywhere (the default).
    Any,
    /// Only where a statement may start (`fore`, `if`).
    Statement,
    /// Only where an expression is expected.
    Expression,
    /// Only directly inside a class body (`pubf`, `ctor`).
    Class,
}

/// One live template a plugin offers.
///
/// `body` uses the LSP snippet syntax (`$1`, `${1:default}`, `$0`) plus
/// `$SELECTION$` (surround) and, for a `postfix` template, `$EXPR$`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveTemplateContribution {
    /// Language id the template applies to, e.g. `"php"`.
    pub language: String,
    /// What the user types: `fore`, or the part after the dot for a postfix.
    pub abbreviation: String,
    /// One line shown in the Insert Live Template list.
    pub description: String,
    pub body: String,
    /// Triggered after `expr.` instead of at a word start.
    #[serde(default)]
    pub postfix: bool,
    #[serde(default = "default_context")]
    pub context: TemplateContext,
}

fn default_context() -> TemplateContext {
    TemplateContext::Any
}

impl LiveTemplateContribution {
    /// `language:abbreviation[.postfix]`: the key user overrides match on and
    /// the uniqueness key within a manifest (a postfix and a plain template
    /// may share an abbreviation).
    pub fn key(&self) -> String {
        let suffix = if self.postfix { ".postfix" } else { "" };
        format!("{}:{}{suffix}", self.language, self.abbreviation)
    }

    pub(super) fn validate(&self) -> Result<(), LoadErrorKind> {
        non_empty("contributes.live-templates.language", &self.language)?;
        non_empty(
            "contributes.live-templates.abbreviation",
            &self.abbreviation,
        )?;
        non_empty("contributes.live-templates.description", &self.description)?;
        non_empty("contributes.live-templates.body", &self.body)?;
        if self
            .abbreviation
            .chars()
            .any(|c| !(c.is_alphanumeric() || c == '_'))
        {
            return Err(LoadErrorKind::MalformedManifest(format!(
                "contributes.live-templates.abbreviation `{}` must be letters, digits and `_`",
                self.abbreviation
            )));
        }
        if self.postfix && !self.body.contains(EXPR_VAR) {
            return Err(LoadErrorKind::MalformedManifest(format!(
                "contributes.live-templates `{}`: a postfix body must contain `{EXPR_VAR}`",
                self.abbreviation
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::with;
    use super::super::*;

    const FORE: &str = r#"
        [[contributes.live-templates]]
        language = "php"
        abbreviation = "fore"
        description = "foreach loop"
        body = "foreach ($1 as $2) {\n\t$0\n}"
        context = "statement"
    "#;

    #[test]
    fn a_live_template_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(FORE)).expect("valid");
        let t = &manifest.contributes.live_templates[0];
        assert_eq!(t.key(), "php:fore");
        assert_eq!(t.context, TemplateContext::Statement);
        assert!(!t.postfix);
        assert!(!manifest.contributes.is_empty());
        assert_eq!(ContributionPoint::LiveTemplates.key(), "live-templates");
    }

    #[test]
    fn context_defaults_to_any() {
        let manifest =
            PluginManifest::from_toml_str(&with(&FORE.replace("context = \"statement\"", "")))
                .expect("valid");
        assert_eq!(
            manifest.contributes.live_templates[0].context,
            TemplateContext::Any
        );
    }

    #[test]
    fn a_postfix_body_must_name_the_expression() {
        let bad = FORE.replace("context = \"statement\"", "postfix = true");
        assert!(matches!(
            PluginManifest::from_toml_str(&with(&bad)).unwrap_err(),
            LoadErrorKind::MalformedManifest(_)
        ));
        let ok = bad.replace("foreach ($1", "foreach ($EXPR$");
        assert!(PluginManifest::from_toml_str(&with(&ok)).is_ok());
    }

    #[test]
    fn an_abbreviation_is_a_single_word() {
        let bad = FORE.replace("\"fore\"", "\"fo re\"");
        assert!(matches!(
            PluginManifest::from_toml_str(&with(&bad)).unwrap_err(),
            LoadErrorKind::MalformedManifest(_)
        ));
    }

    #[test]
    fn a_duplicate_key_is_rejected_but_postfix_may_share_it() {
        let dup = format!("{FORE}{FORE}");
        assert!(matches!(
            PluginManifest::from_toml_str(&with(&dup)).unwrap_err(),
            LoadErrorKind::DuplicateContributionId { .. }
        ));
        let postfix = FORE
            .replace("context = \"statement\"", "postfix = true")
            .replace("foreach ($1", "foreach ($EXPR$");
        assert!(PluginManifest::from_toml_str(&with(&format!("{FORE}{postfix}"))).is_ok());
    }
}
