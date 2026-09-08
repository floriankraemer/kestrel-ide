//! The `[[ai_provider]]` and `[[ai_tool_policy]]` sections of
//! `settings.toml`.
//!
//! Split out of `lib.rs` for its size gate, not because the AI settings
//! are special: they are two plain serde structs the rest of the crate
//! only ever stores. What their string fields *mean* belongs to
//! `settings_model::ai` (ADR-0017).

use serde::{Deserialize, Serialize};

/// One `[[ai_provider]]` entry: what the user says about one AI chat
/// provider.
///
/// Every field is defaulted so a `settings.toml` written before AI chat
/// existed still loads, and so `enabled = false` alone switches a shipped
/// provider off without wiping the rest of its configuration.
///
/// `kind` is a plain `String` here on purpose. This crate stores a provider
/// kind exactly the way it stores a language id: as an opaque string it
/// never interprets, so a kind a newer build understands survives a
/// load/save cycle in an older one. What the four kinds *mean* is
/// `settings_model::ai`'s business (ADR-0017), and the dialect behind each
/// is `ai-chat-core`'s.
///
/// **There is deliberately no API-key field, and there must never be one.**
/// The IDE never writes a key to disk. `api_key_env` holds the *name* of an
/// environment variable, and the key itself is read with `std::env::var` at
/// request time. A future reader who "fixes" the missing field by adding
/// `api_key: String` would move every user's secret into a plain-text file
/// under the config directory.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AiProviderSetting {
    /// Stable id of the provider entry, e.g. `"anthropic"`. The key both the
    /// default catalog and this table are keyed by.
    #[serde(default)]
    pub id: String,
    /// Provider dialect, e.g. `"anthropic"`, `"openai"`,
    /// `"openai-compatible"`, `"gemini"`. Opaque to this crate.
    #[serde(default)]
    pub kind: String,
    /// API base URL. Empty means "use the kind's default", which is what an
    /// OpenAI-compatible endpoint (Ollama, Groq, vLLM) overrides.
    #[serde(default)]
    pub base_url: String,
    /// Model id sent with each request, e.g. `"claude-sonnet-4-5"`.
    #[serde(default)]
    pub model: String,
    /// Name of the environment variable the API key is read from — never the
    /// key. Empty is legitimate: a local endpoint needs no key at all.
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default)]
    pub enabled: bool,
}

/// One `[[ai_tool_policy]]` entry: how far the agent may go with one tool.
///
/// `policy` is one of `auto`, `ask`, `never`, as a plain string for the same
/// reason `AiProviderSetting::kind` is: this crate stores the vocabulary, it
/// does not own it. The read/write classification that decides the *default*
/// for a tool with no entry here lives in `settings_model::ai` (ADR-0017),
/// so a tool added to the catalog needs no change in this crate.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AiToolPolicySetting {
    /// Tool name as the tool catalog spells it, e.g. `"edit_buffer"`.
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub policy: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load, save, Settings, SETTINGS_FILE};
    use std::fs;

    #[test]
    fn ai_providers_and_tool_policies_round_trip_as_arrays_of_tables() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            ai_providers: vec![
                AiProviderSetting {
                    id: "anthropic".into(),
                    kind: "anthropic".into(),
                    model: "claude-sonnet-4-5".into(),
                    api_key_env: "ANTHROPIC_API_KEY".into(),
                    enabled: true,
                    ..AiProviderSetting::default()
                },
                AiProviderSetting {
                    id: "ollama".into(),
                    kind: "openai_compatible".into(),
                    base_url: "http://localhost:11434/v1".into(),
                    model: "qwen2.5-coder".into(),
                    enabled: false,
                    ..AiProviderSetting::default()
                },
            ],
            ai_active_provider: "anthropic".into(),
            ai_tool_policies: vec![AiToolPolicySetting {
                tool: "save_buffer".into(),
                policy: "never".into(),
            }],
            ..Settings::default()
        };

        save(dir.path(), &settings).unwrap();
        let toml = fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(toml.contains("[[ai_provider]]"), "{toml}");
        assert!(toml.contains("[[ai_tool_policy]]"), "{toml}");
        // The one field that must never reach the disk.
        assert!(!toml.contains("api_key ="), "{toml}");

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn a_settings_file_written_before_ai_chat_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            concat!(
                "theme = \"light\"\n",
                "editor_font_size = 13\n",
                "\n",
                "[[language_server]]\n",
                "language_id = \"rust\"\n",
            ),
        )
        .unwrap();

        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded.theme_name(), "light");
        assert!(loaded.ai_providers.is_empty());
        assert!(loaded.ai_tool_policies.is_empty());
        assert_eq!(loaded.ai_active_provider, "");
        assert_eq!(loaded.ai_mode, "");
        // Unset means on, so an upgrade does not silently stop keeping
        // transcripts.
        assert_eq!(loaded.ai_persist_conversations, None);
        assert!(loaded.ai_persist_conversations_or_default());
    }
}
