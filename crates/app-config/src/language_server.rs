//! One `[[language_server]]` entry (ADR-0066).

use serde::{Deserialize, Serialize};

/// What the user says about one language server.
///
/// Every field but `language_id` is optional, so `enabled = false` alone
/// switches a shipped server off without wiping its command. This mirrors
/// `lsp_core::ServerOverride` field for field but is declared here so the
/// config crate keeps no dependency on the LSP client (ADR-0016) —
/// `settings-model` maps one to the other at the seam.
///
/// `settings` and `initialization_options` are free-form because each server
/// defines its own shape; they are written as nested tables.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct LanguageServerSetting {
    /// The server this entry is about. Absent addresses the first server of
    /// `language_id`, which is how an entry written when a language had one
    /// server keeps working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// LSP language id, e.g. `"rust"`.
    #[serde(default)]
    pub language_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Whether this server's diagnostics are shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<bool>,
    /// Answered for the server's `workspace/configuration` section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<toml::Table>,
    /// Sent as `initialize.initializationOptions`; a change restarts the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<toml::Table>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_written_before_servers_had_ids_still_loads() {
        let parsed: LanguageServerSetting =
            toml::from_str("language_id = \"rust\"\ncommand = \"/opt/ra\"\n").unwrap();
        assert_eq!(parsed.id, None);
        assert_eq!(parsed.command.as_deref(), Some("/opt/ra"));
        assert_eq!(parsed.settings, None);
    }

    #[test]
    fn id_settings_and_initialization_options_round_trip() {
        let text = "id = \"intelephense\"\nlanguage_id = \"php\"\n\
                    [initialization_options]\nlicenceKey = \"k\"\n\
                    [settings.intelephense.files]\nmaxSize = 1000000\n";
        let parsed: LanguageServerSetting = toml::from_str(text).unwrap();
        assert_eq!(parsed.id.as_deref(), Some("intelephense"));
        assert_eq!(
            parsed.initialization_options.as_ref().unwrap()["licenceKey"].as_str(),
            Some("k")
        );
        let again: LanguageServerSetting =
            toml::from_str(&toml::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(again, parsed);
    }
}
