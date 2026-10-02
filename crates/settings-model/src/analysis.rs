//! Settings > Analysis (the PHP tooling plan's B7): the draft the page
//! edits, and which of its rows are worth persisting.
//!
//! Every analyzer a plugin contributes gets a row, the same "no Add button
//! needed" shape `servers::ServerDraft` already has — an analyzer with no
//! override is simply enabled with its default trigger. Detected/installed
//! status is deliberately absent from this module: it is live, per-project
//! filesystem state (`analysis_core::status`), not a setting, and belongs
//! in `ui-shell`'s bridge the same way a language server's live status
//! comes from `LspManager`'s events rather than from `ServerRow`.

use app_config::{AnalyzerSetting, Settings};
use plugin_api::AnalyzerContribution;

/// When an analyzer runs, as the settings page shows and edits it.
///
/// A separate vocabulary from `analysis_core::Trigger` on purpose:
/// `settings-model` must not depend on `analysis-core` for this (nothing
/// here needs detection, scheduling, or output parsing), and the two
/// enums' `id()` strings are the actual contract between them — both sides
/// read/write the same `AnalyzerSetting::trigger` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    OnType,
    OnSave,
    Manual,
}

impl Trigger {
    /// What an analyzer runs as when nothing overrides it — the same
    /// default the plan states ("the default is on-the-fly").
    pub const DEFAULT: Trigger = Trigger::OnType;

    pub fn id(self) -> &'static str {
        match self {
            Trigger::OnType => "on-type",
            Trigger::OnSave => "on-save",
            Trigger::Manual => "manual",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "on-type" => Some(Trigger::OnType),
            "on-save" => Some(Trigger::OnSave),
            "manual" => Some(Trigger::Manual),
            _ => None,
        }
    }

    /// What the settings page's dropdown shows.
    pub fn label(self) -> &'static str {
        match self {
            Trigger::OnType => "On Type",
            Trigger::OnSave => "On Save",
            Trigger::Manual => "Manual",
        }
    }
}

/// One row: an analyzer, and how it is configured to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerRow {
    /// `AnalyzerContribution::id`.
    pub id: String,
    /// What the page's Name column shows.
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
}

/// The page's draft: one row per contributed analyzer, committed on OK.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisDraft {
    rows: Vec<AnalyzerRow>,
}

impl AnalysisDraft {
    /// Build the rows from the saved settings and the live plugin
    /// registry's analyzer contributions — `contributions` is
    /// `ui-shell`'s job to gather, the same way `ServerDraft::new` is
    /// handed `plugin_servers` rather than reading the registry itself.
    pub fn new(settings: &Settings, contributions: &[AnalyzerContribution]) -> Self {
        let rows = contributions
            .iter()
            .map(|contribution| {
                let over = settings.analysis.get(&contribution.id);
                AnalyzerRow {
                    id: contribution.id.clone(),
                    name: contribution.name.clone(),
                    enabled: over.and_then(|o| o.enabled).unwrap_or(true),
                    trigger: over
                        .and_then(|o| o.trigger.as_deref())
                        .and_then(Trigger::from_id)
                        .unwrap_or(Trigger::DEFAULT),
                }
            })
            .collect();
        Self { rows }
    }

    pub fn rows(&self) -> &[AnalyzerRow] {
        &self.rows
    }

    pub fn row(&self, id: &str) -> Option<&AnalyzerRow> {
        self.rows.iter().find(|row| row.id == id)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(row) = self.row_mut(id) {
            row.enabled = enabled;
        }
    }

    pub fn set_trigger(&mut self, id: &str, trigger: Trigger) {
        if let Some(row) = self.row_mut(id) {
            row.trigger = trigger;
        }
    }

    /// The `[[analysis.analyzer]]` entries worth writing: only a row that
    /// differs from the shipped default (enabled, `Trigger::DEFAULT`), the
    /// same "only what changed" rule `ServerDraft::overrides` follows, so a
    /// changed default trigger still reaches an analyzer nobody has
    /// touched.
    pub fn overrides(&self) -> Vec<AnalyzerSetting> {
        self.rows
            .iter()
            .filter_map(|row| {
                let enabled = (!row.enabled).then_some(false);
                let trigger =
                    (row.trigger != Trigger::DEFAULT).then(|| row.trigger.id().to_string());
                if enabled.is_none() && trigger.is_none() {
                    return None;
                }
                Some(AnalyzerSetting {
                    id: row.id.clone(),
                    enabled,
                    trigger,
                })
            })
            .collect()
    }

    pub fn apply_to(&self, settings: &mut Settings) {
        settings.analysis.analyzers = self.overrides();
    }

    fn row_mut(&mut self, id: &str) -> Option<&mut AnalyzerRow> {
        self.rows.iter_mut().find(|row| row.id == id)
    }
}

/// What happened to a file that may warrant a per-file analysis run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEvent {
    /// The buffer changed (the caller debounces).
    Edit,
    /// The buffer was written to disk.
    Save,
}

/// One analyzer that should run for a file event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileJob {
    /// `AnalyzerContribution::id`.
    pub analyzer_id: String,
}

/// Which analyzers fire for `event` on a file of language `language_id`.
///
/// An analyzer fires when it is enabled, names the language, and its
/// effective trigger matches: `OnType` runs on every edit and on save,
/// `OnSave` only on save, `Manual` never. An analyzer whose manifest
/// `buffer` is absent or `saved-only` cannot read an unsaved buffer, so a
/// configured `OnType` is downgraded to `OnSave` (the same rule as
/// `analysis_core::effective_trigger`, restated here because this crate
/// does not depend on `analysis-core`).
pub fn file_jobs(
    event: FileEvent,
    language_id: &str,
    draft: &AnalysisDraft,
    contributions: &[AnalyzerContribution],
) -> Vec<FileJob> {
    contributions
        .iter()
        .filter(|c| c.languages.iter().any(|l| l == language_id))
        .filter_map(|c| {
            let row = draft.row(&c.id).filter(|row| row.enabled)?;
            let reads_unsaved = matches!(c.buffer.as_deref(), Some("stdin" | "temp-copy"));
            let trigger = match row.trigger {
                Trigger::OnType if !reads_unsaved => Trigger::OnSave,
                other => other,
            };
            let fires = match (event, trigger) {
                (_, Trigger::Manual) => false,
                (FileEvent::Edit, trigger) => trigger == Trigger::OnType,
                (FileEvent::Save, _) => true,
            };
            fires.then(|| FileJob {
                analyzer_id: c.id.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phpstan() -> AnalyzerContribution {
        AnalyzerContribution {
            id: "phpstan".into(),
            name: "PHPStan".into(),
            program_candidates: vec!["phpstan".into()],
            args: vec![],
            output_format: "checkstyle-xml".into(),
            severity_map: Default::default(),
            languages: vec![],
            file_args: vec![],
            buffer: None,
            composer_package: None,
            requires_interpreter: None,
        }
    }

    #[test]
    fn an_untouched_analyzer_is_enabled_with_the_default_trigger() {
        let draft = AnalysisDraft::new(&Settings::default(), &[phpstan()]);
        let row = draft.row("phpstan").unwrap();
        assert!(row.enabled);
        assert_eq!(row.trigger, Trigger::DEFAULT);
        assert!(draft.overrides().is_empty());
    }

    #[test]
    fn disabling_a_row_produces_a_minimal_override() {
        let mut draft = AnalysisDraft::new(&Settings::default(), &[phpstan()]);
        draft.set_enabled("phpstan", false);
        let overrides = draft.overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].id, "phpstan");
        assert_eq!(overrides[0].enabled, Some(false));
        assert_eq!(overrides[0].trigger, None);
    }

    #[test]
    fn changing_the_trigger_produces_a_minimal_override() {
        let mut draft = AnalysisDraft::new(&Settings::default(), &[phpstan()]);
        draft.set_trigger("phpstan", Trigger::OnSave);
        let overrides = draft.overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].enabled, None);
        assert_eq!(overrides[0].trigger.as_deref(), Some("on-save"));
    }

    #[test]
    fn a_saved_override_round_trips_into_the_draft() {
        let mut settings = Settings::default();
        settings.analysis.analyzers.push(AnalyzerSetting {
            id: "phpstan".into(),
            enabled: Some(false),
            trigger: Some("manual".into()),
        });
        let draft = AnalysisDraft::new(&settings, &[phpstan()]);
        let row = draft.row("phpstan").unwrap();
        assert!(!row.enabled);
        assert_eq!(row.trigger, Trigger::Manual);
    }

    #[test]
    fn apply_to_writes_only_the_overrides() {
        let mut draft = AnalysisDraft::new(&Settings::default(), &[phpstan()]);
        draft.set_enabled("phpstan", false);
        let mut settings = Settings::default();
        draft.apply_to(&mut settings);
        assert_eq!(settings.analysis.analyzers.len(), 1);
        assert_eq!(settings.analysis.analyzers[0].id, "phpstan");
    }

    fn php_analyzer(id: &str, buffer: Option<&str>) -> AnalyzerContribution {
        AnalyzerContribution {
            id: id.into(),
            languages: vec!["php".into()],
            buffer: buffer.map(String::from),
            ..phpstan()
        }
    }

    fn ids(jobs: Vec<FileJob>) -> Vec<String> {
        jobs.into_iter().map(|j| j.analyzer_id).collect()
    }

    fn draft_with(contribs: &[AnalyzerContribution], trigger: Trigger) -> AnalysisDraft {
        let mut draft = AnalysisDraft::new(&Settings::default(), contribs);
        for c in contribs {
            draft.set_trigger(&c.id, trigger);
        }
        draft
    }

    #[test]
    fn an_edit_fires_only_analyzers_that_can_read_the_unsaved_buffer() {
        let contribs = [
            php_analyzer("phpstan", None),
            php_analyzer("phpcs", Some("stdin")),
        ];
        let draft = draft_with(&contribs, Trigger::OnType);
        let jobs = file_jobs(FileEvent::Edit, "php", &draft, &contribs);
        assert_eq!(ids(jobs), vec!["phpcs"]);
    }

    #[test]
    fn a_save_fires_the_downgraded_saved_only_analyzer_too() {
        let contribs = [
            php_analyzer("phpstan", None),
            php_analyzer("phpcs", Some("stdin")),
        ];
        let draft = draft_with(&contribs, Trigger::OnType);
        let jobs = file_jobs(FileEvent::Save, "php", &draft, &contribs);
        assert_eq!(ids(jobs), vec!["phpstan", "phpcs"]);
    }

    #[test]
    fn an_on_save_analyzer_ignores_edits() {
        let contribs = [php_analyzer("phpcs", Some("stdin"))];
        let draft = draft_with(&contribs, Trigger::OnSave);
        assert!(file_jobs(FileEvent::Edit, "php", &draft, &contribs).is_empty());
        assert_eq!(
            ids(file_jobs(FileEvent::Save, "php", &draft, &contribs)),
            vec!["phpcs"]
        );
    }

    #[test]
    fn a_manual_analyzer_never_fires_per_file() {
        let contribs = [php_analyzer("phpcs", Some("stdin"))];
        let draft = draft_with(&contribs, Trigger::Manual);
        assert!(file_jobs(FileEvent::Edit, "php", &draft, &contribs).is_empty());
        assert!(file_jobs(FileEvent::Save, "php", &draft, &contribs).is_empty());
    }

    #[test]
    fn a_disabled_analyzer_never_fires() {
        let contribs = [php_analyzer("phpcs", Some("temp-copy"))];
        let mut draft = draft_with(&contribs, Trigger::OnType);
        draft.set_enabled("phpcs", false);
        assert!(file_jobs(FileEvent::Save, "php", &draft, &contribs).is_empty());
    }

    #[test]
    fn another_language_or_an_analyzer_without_languages_never_fires() {
        let contribs = [php_analyzer("phpcs", Some("stdin")), phpstan()];
        let draft = draft_with(&contribs, Trigger::OnType);
        assert!(file_jobs(FileEvent::Save, "rust", &draft, &contribs).is_empty());
        let jobs = file_jobs(FileEvent::Save, "php", &draft, &contribs);
        assert_eq!(ids(jobs), vec!["phpcs"]);
    }
}
