//! The `[file_associations]` section: which "handler" a file pattern opens
//! with (issue #258, generalising the original "always show images" ask).
//!
//! Persistence only, like the rest of this crate. What a pattern matches,
//! what a handler name means, and which built-in defaults apply when the
//! user has set nothing are `settings_model::file_associations`'s rules
//! (ADR-0017) — this crate stores the vocabulary and never interprets it,
//! the same split [`crate::EditingSettings::line_endings`] uses for its
//! policy name.

use serde::{Deserialize, Serialize};

/// One `[[file_associations.rule]]` entry: `pattern` is a glob
/// (`settings_model::file_associations` is the one place that reads it),
/// and `handler` is a handler-kind name opaque to this crate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAssociationRule {
    pub pattern: String,
    pub handler: String,
}

/// A layer's file-association rules — the global defaults, or a project's
/// overrides. Written as `[[file_associations.rule]]` blocks, checked in
/// order (first match wins) before the built-in defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAssociationSettings {
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<FileAssociationRule>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_section_writes_nothing() {
        let written = toml::to_string(&FileAssociationSettings::default()).unwrap();
        assert!(written.is_empty(), "{written}");
    }

    #[test]
    fn rules_round_trip_through_toml() {
        let text = "\
[[rule]]
pattern = \"*.svg\"
handler = \"image\"

[[rule]]
pattern = \"*.bin\"
handler = \"binary\"
";
        let settings: FileAssociationSettings = toml::from_str(text).unwrap();
        assert_eq!(settings.rules.len(), 2);
        assert_eq!(settings.rules[0].pattern, "*.svg");
        assert_eq!(settings.rules[0].handler, "image");

        let round_tripped: FileAssociationSettings =
            toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(round_tripped, settings);
    }
}
