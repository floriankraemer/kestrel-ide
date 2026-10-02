//! The `[php]` section: the interpreter and the PHP-specific knobs that
//! drive analyzers, language servers, run and debug (PHP parity plan, P0-5).
//!
//! Persistence only, like [`crate::analysis`]: what an unset value means
//! (`php` on `PATH`, port 9003, the language level read from
//! `composer.json`) is `settings_model::php::resolve`'s rule, and every
//! enum-shaped field is a plain string so a file written by a newer build
//! round-trips through an older one. Global by default and
//! project-overridable (ADR-0022): which interpreter a checkout runs under
//! belongs to the checkout at least as often as to the person.
//!
//! The Intelephense licence key is deliberately absent: it is a secret and
//! lives in `secret-store`, never in a `settings.toml` that may be committed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One language server's PHP-specific toggles, keyed by its server id in
/// [`PhpSettings::servers`].
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpServerSetting {
    /// `Some(false)` turns the server off; `None` is its shipped default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Whether the server's own diagnostics reach the Problems dock;
    /// `None` is its shipped default (Phpactor defaults to off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<bool>,
}

/// The `[php]` section. Every field is optional: absent means "this
/// build's default", so a shipped default can change under a user who never
/// touched it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpSettings {
    /// The `php` program: a bare name looked up on `PATH` or a path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<String>,
    /// `"8.3"`-style language level; absent reads `composer.json`'s
    /// `require.php`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_level: Option<String>,
    /// Extra include paths (project-relative or absolute) for the servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_paths: Vec<String>,
    /// Enabled extension stubs; absent leaves the server's own list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stubs: Option<Vec<String>>,
    /// The `[containers.target]` id PHP runs in instead of the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_target: Option<String>,
    /// `"exec"` (a running service) or `"run"` (`run --rm -i`); absent
    /// lets the target's source decide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_mode: Option<String>,
    /// The port Xdebug connects back to; absent is 9003.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xdebug_port: Option<u16>,
    /// The `formatters` contribution id that formats PHP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formatter: Option<String>,
    /// Per-server toggles, as `[php.servers.<id>]` tables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, PhpServerSetting>,
}

/// `skip_serializing_if` for the owning `Settings`/`ProjectSettings` field.
pub fn is_default(value: &PhpSettings) -> bool {
    value == &PhpSettings::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_section_writes_nothing() {
        let text = toml::to_string(&PhpSettings::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn a_configured_section_round_trips() {
        let settings = PhpSettings {
            interpreter: Some("/usr/bin/php8.3".to_string()),
            language_level: Some("8.3".to_string()),
            include_paths: vec!["vendor".to_string()],
            stubs: Some(vec!["redis".to_string()]),
            container_target: Some("t1".to_string()),
            container_mode: Some("exec".to_string()),
            xdebug_port: Some(9004),
            formatter: Some("pint".to_string()),
            servers: BTreeMap::from([(
                "phpactor".to_string(),
                PhpServerSetting {
                    enabled: Some(true),
                    diagnostics: Some(false),
                },
            )]),
        };
        let text = toml::to_string(&settings).expect("serialize");
        let parsed: PhpSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, settings);
    }
}
