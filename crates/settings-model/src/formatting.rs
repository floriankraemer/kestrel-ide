//! Which formatter Reformat Code and format-on-save use (ADR-0070).
//!
//! A configured tool formatter (`[php].formatter`) wins over the language
//! server when it is usable; otherwise the editor falls back to LSP
//! formatting, exactly as before the formatters existed.
//!
//! A tool formats the whole file, so it is never chosen for a selection:
//! Reformat on a selection keeps going to the server's
//! `rangeFormatting`, and a tool never reformats lines the user did not
//! select.

use plugin_api::FormatterContribution;

/// Who formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatPlan {
    /// Run the `formatters` contribution with this id over the whole file.
    Tool(String),
    /// Ask the language server (the whole file or the selection).
    Lsp,
}

/// The formatter id the settings name for `language_id`, if any. Only PHP
/// has a tool-formatter setting today.
pub fn configured_formatter<'a>(
    settings: &'a crate::php::ResolvedPhp,
    language_id: &str,
) -> Option<&'a str> {
    (language_id == "php")
        .then_some(settings.formatter.as_deref())
        .flatten()
}

/// Choose the formatter for one reformat request.
///
/// `configured` is [`configured_formatter`]'s answer; `installed` says
/// whether a contribution's program can be found on its host (the caller
/// owns that probe). Anything that rules the tool out — a selection, an
/// unknown id, a contribution that does not list the language, a tool that
/// is not installed — falls back to [`FormatPlan::Lsp`].
pub fn plan(
    configured: Option<&str>,
    language_id: &str,
    has_selection: bool,
    contributions: &[FormatterContribution],
    installed: impl Fn(&FormatterContribution) -> bool,
) -> FormatPlan {
    if has_selection {
        return FormatPlan::Lsp;
    }
    configured
        .and_then(|id| contributions.iter().find(|c| c.id == id))
        .filter(|c| c.languages.iter().any(|l| l == language_id))
        .filter(|c| installed(c))
        .map_or(FormatPlan::Lsp, |c| FormatPlan::Tool(c.id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pint() -> FormatterContribution {
        FormatterContribution {
            id: "pint".into(),
            name: "Pint".into(),
            languages: vec!["php".into()],
            program_candidates: vec!["vendor/bin/pint".into()],
            args: vec!["{file}".into()],
            buffer: None,
            success_exit_codes: vec![0],
            config_file_candidates: vec![],
            composer_package: None,
            requires_interpreter: None,
            fix_args: vec![],
        }
    }

    #[test]
    fn a_configured_installed_formatter_for_the_language_is_chosen() {
        assert_eq!(
            plan(Some("pint"), "php", false, &[pint()], |_| true),
            FormatPlan::Tool("pint".into())
        );
    }

    #[test]
    fn no_setting_means_the_language_server() {
        assert_eq!(
            plan(None, "php", false, &[pint()], |_| true),
            FormatPlan::Lsp
        );
    }

    #[test]
    fn a_selection_always_goes_to_the_language_server() {
        assert_eq!(
            plan(Some("pint"), "php", true, &[pint()], |_| true),
            FormatPlan::Lsp
        );
    }

    #[test]
    fn an_unknown_wrong_language_or_missing_tool_falls_back() {
        assert_eq!(
            plan(Some("nope"), "php", false, &[pint()], |_| true),
            FormatPlan::Lsp
        );
        assert_eq!(
            plan(Some("pint"), "rust", false, &[pint()], |_| true),
            FormatPlan::Lsp
        );
        assert_eq!(
            plan(Some("pint"), "php", false, &[pint()], |_| false),
            FormatPlan::Lsp
        );
    }

    #[test]
    fn only_php_has_a_configured_formatter() {
        let mut settings = app_config::Settings::default();
        settings.php.formatter = Some("pint".into());
        let php = crate::php::resolve(&settings);
        assert_eq!(configured_formatter(&php, "php"), Some("pint"));
        assert_eq!(configured_formatter(&php, "rust"), None);
    }
}
