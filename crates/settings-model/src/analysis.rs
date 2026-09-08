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
}
