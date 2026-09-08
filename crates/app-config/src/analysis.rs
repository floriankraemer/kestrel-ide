//! The `[analysis]` section: per-analyzer trigger and enabled overrides
//! (the PHP tooling plan's B7).
//!
//! Persistence only, like the rest of this crate: which analyzers *exist*
//! is `plugin-api`'s `AnalyzerContribution` (B2), and how an override here
//! resolves against a project's own is `settings_model::scope`'s job. This
//! file only says what is written down.
//!
//! One row per analyzer that has ever been touched — an analyzer nobody
//! has configured has no row, exactly the sparse idiom `[[language_server]]`
//! already uses, so a settings file never grows an entry claiming a choice
//! the user never made.

use serde::{Deserialize, Serialize};

/// One analyzer's overrides. `enabled`/`trigger` are `Option` rather than a
/// bare value for the same reason `ai_persist_conversations` is: a `None`
/// (or an absent row entirely) means "this build's default", so a shipped
/// default can still change under a user who never touched this analyzer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalyzerSetting {
    /// The `AnalyzerContribution::id` this row overrides.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `"on-type"`, `"on-save"`, or `"manual"` — kept as a string here
    /// rather than an enum this crate would have to own, mirroring how
    /// `ai_mode` stays a bare string; `settings_model::analysis::Trigger`
    /// is the typed vocabulary that parses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
}

/// The `[analysis]` section: every analyzer with a non-default setting.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalysisSettings {
    #[serde(default, rename = "analyzer", skip_serializing_if = "Vec::is_empty")]
    pub analyzers: Vec<AnalyzerSetting>,
}

impl AnalysisSettings {
    pub fn get(&self, id: &str) -> Option<&AnalyzerSetting> {
        self.analyzers.iter().find(|a| a.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_section_writes_nothing() {
        let text = toml::to_string(&AnalysisSettings::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn a_configured_analyzer_round_trips() {
        let settings = AnalysisSettings {
            analyzers: vec![AnalyzerSetting {
                id: "phpstan".to_string(),
                enabled: Some(false),
                trigger: Some("on-save".to_string()),
            }],
        };
        let text = toml::to_string(&settings).expect("serialize");
        let parsed: AnalysisSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, settings);
        assert_eq!(parsed.get("phpstan"), Some(&settings.analyzers[0]));
        assert_eq!(parsed.get("phpcs"), None);
    }
}
