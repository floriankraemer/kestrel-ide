//! The `[terminal]` section: which shell the embedded terminal spawns, where
//! it starts, and what it adds to the environment.
//!
//! Persistence only, like the rest of this crate. *Which* shells a machine
//! offers is `pty_core::shells`' answer, and *which* of the fields below
//! wins for a given tab is the adapter's — see `ui-shell`'s
//! `bridge/terminal.rs`. This file only says what is written down.
//!
//! Project-scoped (ADR-0022): the shell a checkout wants is a property of
//! the checkout — a repository whose tooling only runs under WSL, or under
//! `bash` on a machine whose owner uses `fish` — far more often than it is a
//! property of the person. The global layer is the person's own default and
//! the project layer overrides it, exactly like `[editing]`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What the embedded terminal launches, and where.
///
/// Every field carries an "unset" state as an empty value, the same
/// zero-is-unset idiom [`crate::EditingSettings`] documents: an empty
/// `shell_id` means "the platform default", an empty `start_directory`
/// means "the open project's root". None of them has a meaningful empty
/// value of its own, so no `Option` is needed to tell the two apart.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalSettings {
    /// A `pty_core::ShellCandidate::id` — `system`, `zsh`, `pwsh`,
    /// `wsl:Ubuntu`. Empty means the platform default.
    ///
    /// Stored by id rather than by path so the same committed project file
    /// works on two machines that install `zsh` in different places, and so
    /// a machine that no longer has the named shell falls back to its
    /// default instead of failing to spawn.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell_id: String,

    /// A shell this build has never heard of, named by path. Wins over
    /// [`TerminalSettings::shell_id`] when set, which is what makes the
    /// settings page's "Custom…" entry a real escape hatch rather than a
    /// request for the catalogue to grow.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell_path: String,

    /// Arguments for the shell, space-separated — the same convention
    /// `FfiRunConfig::args` already crosses the FFI seam with. Shell-style
    /// quoting is the upgrade if a literal space in an argument ever
    /// matters; nothing needs it yet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell_args: String,

    /// Where a new terminal starts. Empty — the default — means the open
    /// project's root, which is what someone opening a terminal in an IDE
    /// is asking for; before this setting existed the terminal inherited
    /// the IDE process's own directory, which is never useful.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start_directory: String,

    /// Added to the inherited environment, never replacing it
    /// (`pty_core::ShellSpec::env`). A `BTreeMap` so the file is written in
    /// a stable order and two saves of the same settings produce the same
    /// bytes — a settings file that reorders itself is noise in a diff.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl TerminalSettings {
    /// The environment as `pty-core` takes it.
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        self.env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_mean_platform_shell_in_the_project_root() {
        let settings = TerminalSettings::default();
        assert!(settings.shell_id.is_empty());
        assert!(settings.shell_path.is_empty());
        assert!(settings.start_directory.is_empty());
        assert!(settings.env.is_empty());
    }

    #[test]
    fn a_configured_section_round_trips_through_toml() {
        let mut settings = TerminalSettings {
            shell_id: "wsl:Ubuntu".to_string(),
            shell_args: "-l".to_string(),
            start_directory: "/srv/checkout".to_string(),
            ..TerminalSettings::default()
        };
        settings
            .env
            .insert("RUST_LOG".to_string(), "debug".to_string());

        let text = toml::to_string(&settings).expect("serialize");
        let parsed: TerminalSettings = toml::from_str(&text).expect("deserialize");

        assert_eq!(parsed, settings);
    }

    /// The whole point of the empty-is-unset idiom: a default section
    /// writes nothing at all, so a settings file never grows a block
    /// claiming the user chose what they never touched.
    #[test]
    fn an_untouched_section_writes_nothing() {
        let text = toml::to_string(&TerminalSettings::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn env_crosses_to_pty_core_as_pairs() {
        let mut settings = TerminalSettings::default();
        settings.env.insert("B".to_string(), "2".to_string());
        settings.env.insert("A".to_string(), "1".to_string());

        // Sorted, because the map is: a stable order is what keeps two
        // saves of the same settings byte-identical.
        assert_eq!(
            settings.env_pairs(),
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string()),
            ]
        );
    }
}
