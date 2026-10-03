//! [`AnalyzerDef`]: an [`AnalyzerContribution`] joined with the severity
//! vocabulary it declares, and the trigger it prefers.
//!
//! `plugin-api` stays a leaf (it does not depend on `diagnostics-core`), so
//! its `severity_map` is `BTreeMap<String, String>` — free-form tool
//! spelling to free-form neutral spelling. This module is where that map is
//! parsed into `diagnostics_core::Severity`, once, at the point a
//! contribution becomes an `AnalyzerDef`, so nothing downstream re-parses a
//! severity string.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use diagnostics_core::Severity;
use plugin_api::AnalyzerContribution;

use crate::buffer::BufferStrategy;

/// When an analyzer runs.
///
/// Mirrors the plan's three triggers. `OnType` and `OnSave` analyze one
/// file; `Manual` analyzes the project. An analyzer whose manifest can only
/// read the saved file (see [`crate::buffer::BufferStrategy`]) is never
/// resolved to `OnType` — [`AnalyzerDef::effective_trigger`] degrades it to
/// `OnSave` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    OnType,
    OnSave,
    Manual,
}

/// One analyzer, ready to run: the manifest data plus its parsed severity
/// table.
///
/// Unknown tool-severity strings are simply absent from `severities` rather
/// than an error at load time — a manifest typo here should not disable the
/// analyzer, only its worst-case classification of that one severity word;
/// [`AnalyzerDef::severity_for`] falls back to
/// [`Severity::Warning`], a deliberately visible rather than silently
/// dropped default.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyzerDef {
    pub id: String,
    pub name: String,
    pub program_candidates: Vec<String>,
    pub args: Vec<String>,
    pub output_format: String,
    /// Language ids this analyzer checks per file; empty = project runs only.
    pub languages: Vec<String>,
    /// Arguments (and `{file}` placeholder) for a single-file run.
    pub file_args: Vec<String>,
    pub buffer: BufferStrategy,
    /// `Some("php")` when the program must run under the PHP interpreter.
    pub requires_interpreter: Option<String>,
    /// The comment template that silences one finding (`{code}` = rule id).
    pub suppress_comment: Option<String>,
    /// Rule ids are `Id: text` message prefixes, not `source` attributes.
    pub code_in_message: bool,
    /// Id of the `formatters` contribution that can fix a single finding.
    pub fixer: Option<String>,
    config_file_candidates: Vec<String>,
    ruleset_default: Option<String>,
    project_paths_config: Vec<String>,
    severities: HashMap<String, Severity>,
}

impl AnalyzerDef {
    pub fn from_contribution(contribution: &AnalyzerContribution) -> Self {
        let severities = contribution
            .severity_map
            .iter()
            .filter_map(|(tool_word, neutral)| {
                parse_severity(neutral).map(|s| (tool_word.clone(), s))
            })
            .collect();
        Self {
            id: contribution.id.clone(),
            name: contribution.name.clone(),
            program_candidates: contribution.program_candidates.clone(),
            args: contribution.args.clone(),
            output_format: contribution.output_format.clone(),
            languages: contribution.languages.clone(),
            file_args: contribution.file_args.clone(),
            buffer: BufferStrategy::from_manifest(contribution.buffer.as_deref()),
            requires_interpreter: contribution.requires_interpreter.clone(),
            suppress_comment: contribution.suppress_comment.clone(),
            code_in_message: contribution.code_in_message,
            fixer: contribution.fixer.clone(),
            config_file_candidates: contribution.config_file_candidates.clone(),
            ruleset_default: contribution.ruleset_default.clone(),
            project_paths_config: contribution.project_paths_config.clone(),
            severities,
        }
    }

    /// This analyzer with `{ruleset}` resolved for the project at `root`:
    /// the first `config-file-candidates` entry that exists there (kept
    /// project-relative, so it means the same inside a container or WSL,
    /// where the tool runs from the project root), else `ruleset-default`.
    #[must_use]
    pub fn with_ruleset_for(mut self, root: &Path) -> Self {
        let ruleset = self
            .config_file_candidates
            .iter()
            .find(|candidate| root.join(candidate).is_file())
            .cloned()
            .or_else(|| self.ruleset_default.clone());
        if let Some(ruleset) = ruleset {
            for arg in self.args.iter_mut().chain(self.file_args.iter_mut()) {
                *arg = arg.replace("{ruleset}", &ruleset);
            }
        }
        self
    }

    /// How to spawn `program` for this analyzer: under `php_binary` when the
    /// manifest says `requires-interpreter = "php"` and the program needs
    /// it (see [`crate::php_invocation`]), otherwise as is. The returned
    /// prefix goes before [`Self::args`].
    pub fn invocation(&self, program: &Path, php_binary: &str) -> (PathBuf, Vec<String>) {
        match self.requires_interpreter.as_deref() {
            Some("php") => crate::php::invocation(program, Some(php_binary)),
            _ => (program.to_path_buf(), Vec::new()),
        }
    }

    /// The full argv tail for a single-file run against `file` (the path the
    /// tool should read: the real file, or a temp copy).
    ///
    /// `args`, then `file_args` with `{file}` replaced; when no `file_args`
    /// entry names the file and the buffer goes over stdin, the path is
    /// appended, because the tool has to be told which file to read.
    pub fn file_run_args(&self, file: &Path) -> Vec<String> {
        let file = file.to_string_lossy();
        let names_file = self
            .args
            .iter()
            .chain(&self.file_args)
            .any(|a| a.contains("{file}"));
        let mut argv: Vec<String> = self
            .args
            .iter()
            .chain(&self.file_args)
            .map(|a| a.replace("{file}", &file))
            .collect();
        if !names_file && self.buffer != BufferStrategy::Stdin {
            argv.push(file.into_owned());
        }
        argv
    }

    /// The argv tail for a project-wide run over `root`: `args`, with a
    /// `{file}` placeholder (a tool whose path is not the last argument,
    /// like PHPMD's `<path> <format> <ruleset>`) replaced by the root, else
    /// the root appended.
    pub fn project_run_args(&self, root: &Path) -> Vec<String> {
        if self.config_names_project_paths(root) {
            return self.args.clone();
        }
        let root = root.to_string_lossy();
        if self.args.iter().any(|a| a.contains("{file}")) {
            self.args
                .iter()
                .map(|a| a.replace("{file}", &root))
                .collect()
        } else {
            let mut argv = self.args.clone();
            argv.push(root.into_owned());
            argv
        }
    }

    /// Whether the project's own tool config (the first
    /// `project-paths-config` file that exists) names paths: a `paths:` key
    /// (PHPStan's NEON) or a `<file>` element (PHPCS's ruleset).
    ///
    /// ponytail: a line scan, not a NEON or XML parser, so paths that only an
    /// `includes:` file provides are not seen; the run then gets the root, as
    /// before. Parse the config if that ever matters.
    fn config_names_project_paths(&self, root: &Path) -> bool {
        self.project_paths_config
            .iter()
            .find_map(|candidate| std::fs::read_to_string(root.join(candidate)).ok())
            .is_some_and(|text| {
                text.lines()
                    .any(|line| line.trim_start().starts_with("paths:") || line.contains("<file>"))
            })
    }

    /// The severity a tool's own word maps to, or [`Severity::Warning`]
    /// when the manifest's `severity-map` says nothing about it.
    pub fn severity_for(&self, tool_word: &str) -> Severity {
        self.severities
            .get(tool_word)
            .copied()
            .unwrap_or(Severity::Warning)
    }
}

fn parse_severity(word: &str) -> Option<Severity> {
    match word.to_ascii_lowercase().as_str() {
        "error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "information" | "info" | "notice" => Some(Severity::Information),
        "hint" => Some(Severity::Hint),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contribution() -> AnalyzerContribution {
        AnalyzerContribution {
            id: "phpstan".into(),
            name: "PHPStan".into(),
            program_candidates: vec!["vendor/bin/phpstan".into(), "phpstan".into()],
            args: vec!["analyse".into()],
            output_format: "checkstyle-xml".into(),
            severity_map: [("error".to_string(), "error".to_string())]
                .into_iter()
                .collect(),
            languages: vec![],
            file_args: vec![],
            buffer: None,
            composer_package: None,
            requires_interpreter: None,
            suppress_comment: None,
            code_in_message: false,
            fixer: None,
            config_file_candidates: vec![],
            ruleset_default: None,
            project_paths_config: vec![],
        }
    }

    #[test]
    fn a_declared_severity_word_maps_through() {
        let def = AnalyzerDef::from_contribution(&contribution());
        assert_eq!(def.severity_for("error"), Severity::Error);
    }

    #[test]
    fn an_undeclared_severity_word_falls_back_to_warning() {
        let def = AnalyzerDef::from_contribution(&contribution());
        assert_eq!(
            def.severity_for("whatever-this-tool-calls-it"),
            Severity::Warning
        );
    }

    #[test]
    fn file_run_fields_carry_over_with_the_buffer_parsed() {
        let mut c = contribution();
        c.languages = vec!["php".into()];
        c.file_args = vec!["--stdin-path={file}".into(), "-".into()];
        c.buffer = Some("stdin".into());
        c.requires_interpreter = Some("php".into());
        let def = AnalyzerDef::from_contribution(&c);
        assert_eq!(def.languages, vec!["php"]);
        assert_eq!(def.file_args, vec!["--stdin-path={file}", "-"]);
        assert_eq!(def.buffer, BufferStrategy::Stdin);
        assert_eq!(def.requires_interpreter.as_deref(), Some("php"));
    }

    #[test]
    fn only_an_analyzer_requiring_php_is_launched_under_the_interpreter() {
        let dir = tempfile::tempdir().unwrap();
        let phar = dir.path().join("tool.phar");
        std::fs::write(&phar, "x").unwrap();
        let plain = AnalyzerDef::from_contribution(&contribution());
        assert_eq!(plain.invocation(&phar, "php8"), (phar.clone(), vec![]));
        let mut c = contribution();
        c.requires_interpreter = Some("php".into());
        let php = AnalyzerDef::from_contribution(&c);
        assert_eq!(
            php.invocation(&phar, "php8"),
            (
                PathBuf::from("php8"),
                vec![phar.to_string_lossy().into_owned()]
            )
        );
    }

    #[test]
    fn a_saved_only_file_run_appends_the_path() {
        let def = AnalyzerDef::from_contribution(&contribution());
        assert_eq!(
            def.file_run_args(Path::new("/p/a.php")),
            vec!["analyse", "/p/a.php"]
        );
    }

    #[test]
    fn a_stdin_file_run_substitutes_the_placeholder_and_appends_nothing() {
        let mut c = contribution();
        c.args = vec!["--report=checkstyle".into()];
        c.file_args = vec!["--stdin-path={file}".into(), "-".into()];
        c.buffer = Some("stdin".into());
        let def = AnalyzerDef::from_contribution(&c);
        assert_eq!(
            def.file_run_args(Path::new("/p/a.php")),
            vec!["--report=checkstyle", "--stdin-path=/p/a.php", "-"]
        );
    }

    #[test]
    fn a_placeholder_in_args_positions_the_path_for_file_and_project_runs() {
        let mut c = contribution();
        c.args = vec!["{file}".into(), "checkstyle".into(), "cleancode".into()];
        let def = AnalyzerDef::from_contribution(&c);
        assert_eq!(
            def.file_run_args(Path::new("/p/a.php")),
            vec!["/p/a.php", "checkstyle", "cleancode"]
        );
        assert_eq!(
            def.project_run_args(Path::new("/p")),
            vec!["/p", "checkstyle", "cleancode"]
        );
    }

    #[test]
    fn a_project_run_leaves_the_path_to_a_config_that_names_one() {
        let mut c = contribution();
        c.args = vec!["analyse".into(), "--no-progress".into()];
        c.project_paths_config = vec!["phpstan.neon".into(), "phpstan.neon.dist".into()];
        let def = AnalyzerDef::from_contribution(&c);

        let bare = tempfile::tempdir().unwrap();
        let root = bare.path();
        // No config: the root is the only thing to analyse.
        assert_eq!(
            def.project_run_args(root),
            vec![
                "analyse".to_string(),
                "--no-progress".into(),
                root.to_string_lossy().into_owned()
            ]
        );

        // A config without `paths:` still needs the root.
        std::fs::write(root.join("phpstan.neon"), "parameters:\n\tlevel: 5\n").unwrap();
        assert_eq!(def.project_run_args(root).len(), 3);

        // The first candidate that exists decides, and `paths:` removes the root.
        std::fs::write(
            root.join("phpstan.neon"),
            "parameters:\n\tlevel: 5\n\tpaths:\n\t\t- app\n",
        )
        .unwrap();
        assert_eq!(def.project_run_args(root), vec!["analyse", "--no-progress"]);

        // A PHPCS ruleset names its paths with `<file>` elements.
        std::fs::remove_file(root.join("phpstan.neon")).unwrap();
        std::fs::write(
            root.join("phpstan.neon.dist"),
            "<ruleset><file>src</file></ruleset>",
        )
        .unwrap();
        assert_eq!(def.project_run_args(root), vec!["analyse", "--no-progress"]);
    }

    #[test]
    fn ruleset_is_the_projects_config_file_when_one_exists_else_the_default() {
        let mut c = contribution();
        c.args = vec!["{file}".into(), "checkstyle".into(), "{ruleset}".into()];
        c.config_file_candidates = vec!["phpmd.xml".into(), "phpmd.xml.dist".into()];
        c.ruleset_default = Some("cleancode,codesize".into());
        let def = AnalyzerDef::from_contribution(&c);

        let bare = tempfile::tempdir().unwrap();
        assert_eq!(
            def.clone().with_ruleset_for(bare.path()).args,
            vec!["{file}", "checkstyle", "cleancode,codesize"]
        );

        let configured = tempfile::tempdir().unwrap();
        std::fs::write(configured.path().join("phpmd.xml.dist"), "<ruleset/>").unwrap();
        assert_eq!(
            def.with_ruleset_for(configured.path()).args,
            vec!["{file}", "checkstyle", "phpmd.xml.dist"]
        );
    }

    #[test]
    fn a_project_run_without_a_placeholder_appends_the_root() {
        let def = AnalyzerDef::from_contribution(&contribution());
        assert_eq!(def.project_run_args(Path::new("/p")), vec!["analyse", "/p"]);
    }

    #[test]
    fn an_absent_buffer_means_saved_only() {
        let def = AnalyzerDef::from_contribution(&contribution());
        assert_eq!(def.buffer, BufferStrategy::SavedOnly);
    }

    #[test]
    fn fields_carry_over_verbatim() {
        let def = AnalyzerDef::from_contribution(&contribution());
        assert_eq!(def.id, "phpstan");
        assert_eq!(def.name, "PHPStan");
        assert_eq!(def.output_format, "checkstyle-xml");
        assert_eq!(
            def.program_candidates,
            vec!["vendor/bin/phpstan", "phpstan"]
        );
    }
}
