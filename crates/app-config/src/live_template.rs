//! One `[[live_template]]` row (ADR-0072): the user's own live template.
//!
//! A row replaces the plugin-contributed template with the same
//! `(language, abbreviation, postfix)`, or adds a new one. The merge lives in
//! `settings-model`; this crate only stores the rows.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct LiveTemplateSetting {
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub abbreviation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// LSP snippet syntax plus `$SELECTION$` / `$EXPR$`.
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub postfix: bool,
    /// `any` (default), `statement`, `expression` or `class`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::{load, save, Settings, SETTINGS_FILE};

    use super::*;

    #[test]
    fn rows_round_trip_as_array_of_tables() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            live_templates: vec![LiveTemplateSetting {
                language: "php".into(),
                abbreviation: "pr".into(),
                body: "print_r($1);".into(),
                ..LiveTemplateSetting::default()
            }],
            ..Settings::default()
        };
        save(dir.path(), &settings).unwrap();
        let toml = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(toml.contains("[[live_template]]"), "{toml}");
        assert_eq!(
            load(dir.path()).unwrap().live_templates,
            settings.live_templates
        );
    }

    #[test]
    fn a_file_without_rows_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &Settings::default()).unwrap();
        assert!(load(dir.path()).unwrap().live_templates.is_empty());
    }
}
