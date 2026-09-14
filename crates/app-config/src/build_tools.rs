//! The `[build_tools]` section (the jvm-build-tools plan's A7, ADR-0057).
//!
//! Persistence only, like [`crate::analysis`]: which build tools *exist* is
//! `plugin-api`'s `BuildToolContribution`, and what a value here means (a
//! distribution choice, an auto-reload mode) is `settings_model::build_tools`'s
//! job. This file only says what is written down.
//!
//! [`BuildToolsSettings::trusted_roots`] is deliberately **global only**
//! (ADR-0057 §3): a project directory is exactly what the trust gate exists
//! to guard against, so nothing here reads it from a project-scoped file —
//! `settings_model::scope` never lists it as a `ScopedField`. The
//! `[build_tools.gradle]`/`[build_tools.maven]` sub-tables are ordinary
//! project-overridable settings, the same shape [`crate::containers`]'s
//! connections are.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Gradle-specific overrides.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct GradleToolSettings {
    /// `"wrapper"` (default) or `"gradle-home"`. Kept as a plain string,
    /// like [`crate::AnalyzerSetting::trigger`] — the typed vocabulary is
    /// `settings_model::build_tools::GradleDistribution`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradle_home: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_home: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,
    /// `"external"` (default), `"any"`, or `"none"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_reload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_sources: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jvm_args: Vec<String>,
}

/// Maven-specific overrides.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct MavenToolSettings {
    /// `"wrapper"` (default), `"path"`, or a directory — free-form for the
    /// same reason [`GradleToolSettings::distribution`] is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maven_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_settings_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_repository: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_tests: Option<bool>,
    /// `mvn -T`'s own spelling (`"4"`, `"1C"`), so this never has to
    /// interpret it — a free-form string, kept exactly as `mvn` would take
    /// it on the command line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_update_snapshots: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_reload: Option<String>,
}

/// The `[build_tools]` section.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildToolsSettings {
    /// Project roots the user has explicitly agreed to sync (ADR-0057 §3).
    /// Global only — see this module's doc comment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_roots: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "is_default_gradle")]
    pub gradle: GradleToolSettings,
    #[serde(default, skip_serializing_if = "is_default_maven")]
    pub maven: MavenToolSettings,
}

fn is_default_gradle(settings: &GradleToolSettings) -> bool {
    settings == &GradleToolSettings::default()
}

fn is_default_maven(settings: &MavenToolSettings) -> bool {
    settings == &MavenToolSettings::default()
}

impl BuildToolsSettings {
    /// Is `root` (already canonicalised by the caller) one the user has
    /// agreed to sync? A plain `Vec::contains` — canonicalisation happens
    /// once, at the call site (`jvm_build_core::sync::is_trusted`), so this
    /// stays a leaf comparison with no filesystem access of its own.
    pub fn is_trusted(&self, root: &std::path::Path) -> bool {
        self.trusted_roots.iter().any(|trusted| trusted == root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_section_writes_nothing() {
        let text = toml::to_string(&BuildToolsSettings::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn a_trusted_root_round_trips() {
        let settings = BuildToolsSettings {
            trusted_roots: vec![PathBuf::from("/home/u/project")],
            ..Default::default()
        };
        let text = toml::to_string(&settings).expect("serialize");
        let parsed: BuildToolsSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, settings);
        assert!(parsed.is_trusted(std::path::Path::new("/home/u/project")));
        assert!(!parsed.is_trusted(std::path::Path::new("/home/u/other")));
    }

    #[test]
    fn a_configured_gradle_section_round_trips() {
        let settings = BuildToolsSettings {
            gradle: GradleToolSettings {
                distribution: Some("gradle-home".to_string()),
                gradle_home: Some(PathBuf::from("/opt/gradle-8.10")),
                offline: Some(true),
                auto_reload: Some("any".to_string()),
                jvm_args: vec!["-Xmx2g".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let text = toml::to_string(&settings).expect("serialize");
        let parsed: BuildToolsSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, settings);
    }

    #[test]
    fn a_configured_maven_section_round_trips() {
        let settings = BuildToolsSettings {
            maven: MavenToolSettings {
                offline: Some(true),
                skip_tests: Some(true),
                threads: Some("1C".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let text = toml::to_string(&settings).expect("serialize");
        let parsed: BuildToolsSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, settings);
    }
}
