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
            severities,
        }
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
