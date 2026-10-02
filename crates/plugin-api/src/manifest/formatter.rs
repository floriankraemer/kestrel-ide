//! The `formatters` contribution point (ADR-0070): a code formatter the IDE
//! runs as a native process over a buffer, whole-file.
//!
//! Split out of `mod.rs` to keep that file under the file-size ceiling.

use serde::Deserialize;

use super::{check_id, check_tool_package_and_interpreter, non_empty};
use crate::error::LoadErrorKind;

fn default_success_exit_codes() -> Vec<i32> {
    vec![0]
}

/// One formatter a plugin offers (php-cs-fixer, Pint, phpcbf).
///
/// Shaped after `AnalyzerContribution` — a wasm guest can neither spawn a
/// process nor be trusted with a buffer, so this is native launch data —
/// with the three things a formatter needs that a linter does not: the
/// result is the formatted *text* (not a report), a tool may exit non-zero
/// on success (phpcbf exits 1 when it fixed something), and the buffer
/// strategy is limited to the two a formatter can honour.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatterContribution {
    /// Stable id, e.g. `"php-cs-fixer"`; what `[php].formatter` names.
    pub id: String,
    /// What the settings page and the status line show.
    pub name: String,
    /// Language ids whose files this formatter formats. At least one: a
    /// formatter that names no language could never be chosen.
    pub languages: Vec<String>,
    /// Programs to probe, in order; same rule as
    /// `AnalyzerContribution::program_candidates`.
    #[serde(rename = "program-candidates")]
    pub program_candidates: Vec<String>,
    /// Arguments, with `{file}` replaced by the path the tool reads: the
    /// temp copy for `temp-copy` (which must name it, because the tool
    /// rewrites that file in place), the real path for `stdin` (where it is
    /// only a hint such as `--stdin-path={file}`).
    #[serde(default)]
    pub args: Vec<String>,
    /// `"temp-copy"` (default): the tool rewrites a copy of the buffer
    /// beside the original and the copy is read back. `"stdin"`: the buffer
    /// goes to stdin and the formatted text comes back on stdout.
    #[serde(default)]
    pub buffer: Option<String>,
    /// Exit codes that mean "formatted". Defaults to `[0]`.
    #[serde(default = "default_success_exit_codes", rename = "success-exit-codes")]
    pub success_exit_codes: Vec<i32>,
    /// Config filenames to look for in the project root, so the settings
    /// page can show which one a run uses. Empty is valid.
    #[serde(default, rename = "config-file-candidates")]
    pub config_file_candidates: Vec<String>,
    /// Same meaning as `AnalyzerContribution::composer_package`.
    #[serde(default, rename = "composer-package")]
    pub composer_package: Option<String>,
    /// Same meaning as `AnalyzerContribution::requires_interpreter`.
    #[serde(default, rename = "requires-interpreter")]
    pub requires_interpreter: Option<String>,
    /// Arguments placed before [`Self::args`] to narrow a run to the one
    /// rule an analyzer finding names (`["--sniffs={code}"]`). Empty means
    /// this formatter cannot fix a single finding. Must contain `{code}`
    /// (the whole rule id) or `{sniff}` (its first three dot-separated
    /// parts, PHPCS's `Standard.Category.Sniff`).
    #[serde(default, rename = "fix-args")]
    pub fix_args: Vec<String>,
}

impl FormatterContribution {
    pub(super) fn validate(&self) -> Result<(), LoadErrorKind> {
        check_id("contributes.formatters.id", &self.id)?;
        non_empty("contributes.formatters.name", &self.name)?;
        if self.languages.is_empty() {
            return Err(LoadErrorKind::EmptyField(
                "contributes.formatters.languages",
            ));
        }
        for language in &self.languages {
            non_empty("contributes.formatters.languages", language)?;
        }
        if self.program_candidates.is_empty() {
            return Err(LoadErrorKind::EmptyField(
                "contributes.formatters.program-candidates",
            ));
        }
        for candidate in &self.program_candidates {
            non_empty("contributes.formatters.program-candidates", candidate)?;
        }
        if self.success_exit_codes.is_empty() {
            return Err(LoadErrorKind::EmptyField(
                "contributes.formatters.success-exit-codes",
            ));
        }
        match self.buffer.as_deref() {
            None | Some("stdin") => {}
            Some("temp-copy") => {}
            Some(other) => {
                return Err(LoadErrorKind::MalformedManifest(format!(
                    "contributes.formatters.buffer `{other}` must be one of `stdin`, `temp-copy`"
                )))
            }
        }
        if self.buffer.as_deref() != Some("stdin")
            && !self.args.iter().any(|a| a.contains("{file}"))
        {
            return Err(LoadErrorKind::MalformedManifest(
                "contributes.formatters.args must contain `{file}` for a temp-copy formatter"
                    .to_string(),
            ));
        }
        let names_rule = |a: &String| a.contains("{code}") || a.contains("{sniff}");
        if !self.fix_args.is_empty() && !self.fix_args.iter().any(names_rule) {
            return Err(LoadErrorKind::MalformedManifest(
                "contributes.formatters.fix-args must contain `{code}` or `{sniff}`".to_string(),
            ));
        }
        check_tool_package_and_interpreter(
            "contributes.formatters",
            self.composer_package.as_deref(),
            self.requires_interpreter.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::with;
    use super::super::*;

    const CBF: &str = r#"
        [[contributes.formatters]]
        id = "phpcbf"
        name = "phpcbf"
        languages = ["php"]
        program-candidates = ["vendor/bin/phpcbf", "phpcbf"]
        args = ["-q", "--stdin-path={file}", "-"]
        buffer = "stdin"
        success-exit-codes = [0, 1]
        config-file-candidates = ["phpcs.xml"]
        composer-package = "squizlabs/php_codesniffer"
        requires-interpreter = "php"
    "#;

    #[test]
    fn a_formatter_contribution_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(CBF)).expect("valid");
        let f = &manifest.contributes.formatters[0];
        assert_eq!(f.id, "phpcbf");
        assert_eq!(f.languages, vec!["php"]);
        assert_eq!(f.buffer.as_deref(), Some("stdin"));
        assert_eq!(f.success_exit_codes, vec![0, 1]);
        assert_eq!(f.config_file_candidates, vec!["phpcs.xml"]);
        assert_eq!(f.requires_interpreter.as_deref(), Some("php"));
        assert!(!manifest.contributes.is_empty());
        assert_eq!(ContributionPoint::Formatters.key(), "formatters");
    }

    #[test]
    fn fix_args_must_name_the_code() {
        let base = "languages = [\"php\"]\nprogram-candidates = [\"x\"]\nargs = [\"{file}\"]\n";
        assert!(matches!(
            rejected(&format!("{base}fix-args = [\"--sniffs\"]")),
            LoadErrorKind::MalformedManifest(_)
        ));
        let ok = PluginManifest::from_toml_str(&with(&format!(
            "[[contributes.formatters]]\nid = \"x\"\nname = \"X\"\n{base}fix-args = [\"--sniffs={{code}}\"]"
        )))
        .expect("valid");
        assert_eq!(
            ok.contributes.formatters[0].fix_args,
            vec!["--sniffs={code}"]
        );
        assert!(PluginManifest::from_toml_str(&with(&format!(
            "[[contributes.formatters]]\nid = \"x\"\nname = \"X\"\n{base}fix-args = [\"--sniffs={{sniff}}\"]"
        )))
        .is_ok());
    }

    #[test]
    fn defaults_are_temp_copy_and_exit_code_zero() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.formatters]]
            id = "pint"
            name = "Pint"
            languages = ["php"]
            program-candidates = ["vendor/bin/pint"]
            args = ["{file}"]
            "#,
        ))
        .expect("valid");
        let f = &manifest.contributes.formatters[0];
        assert_eq!(f.buffer, None);
        assert_eq!(f.success_exit_codes, vec![0]);
    }

    fn rejected(body: &str) -> LoadErrorKind {
        PluginManifest::from_toml_str(&with(&format!(
            "[[contributes.formatters]]\nid = \"x\"\nname = \"X\"\n{body}"
        )))
        .unwrap_err()
    }

    #[test]
    fn a_formatter_must_name_a_language_and_a_program() {
        assert_eq!(
            rejected("languages = []\nprogram-candidates = [\"x\"]\nargs = [\"{file}\"]"),
            LoadErrorKind::EmptyField("contributes.formatters.languages")
        );
        assert_eq!(
            rejected("languages = [\"php\"]\nprogram-candidates = []\nargs = [\"{file}\"]"),
            LoadErrorKind::EmptyField("contributes.formatters.program-candidates")
        );
    }

    #[test]
    fn a_temp_copy_formatter_must_name_the_file() {
        assert!(matches!(
            rejected("languages = [\"php\"]\nprogram-candidates = [\"x\"]\nargs = [\"fix\"]"),
            LoadErrorKind::MalformedManifest(_)
        ));
    }

    #[test]
    fn an_unknown_buffer_and_empty_exit_codes_are_rejected() {
        let base = "languages = [\"php\"]\nprogram-candidates = [\"x\"]\nargs = [\"{file}\"]\n";
        assert!(matches!(
            rejected(&format!("{base}buffer = \"saved-only\"")),
            LoadErrorKind::MalformedManifest(_)
        ));
        assert_eq!(
            rejected(&format!("{base}success-exit-codes = []")),
            LoadErrorKind::EmptyField("contributes.formatters.success-exit-codes")
        );
    }

    #[test]
    fn duplicate_formatter_ids_are_rejected() {
        let one = "[[contributes.formatters]]\nid = \"x\"\nname = \"X\"\nlanguages = [\"php\"]\n\
                   program-candidates = [\"x\"]\nargs = [\"{file}\"]\n";
        assert_eq!(
            PluginManifest::from_toml_str(&with(&format!("{one}{one}"))).unwrap_err(),
            LoadErrorKind::DuplicateContributionId {
                point: "formatters",
                id: "x".to_string()
            }
        );
    }
}
