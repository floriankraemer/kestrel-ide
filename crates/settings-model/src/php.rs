//! The rules behind the `[php]` section (PHP parity plan, P0-5): what an
//! unset or malformed value means.
//!
//! `app_config::PhpSettings` stores whatever the file says; [`resolve`]
//! turns that into the values consumers act on. A bad entry (an empty
//! interpreter, a language level that is not `major.minor`, port 0) reads
//! as unset rather than failing the whole section.

use std::collections::BTreeMap;

use app_config::php::PhpSettings;
use app_config::Settings;

/// Xdebug's default client port.
pub const DEFAULT_XDEBUG_PORT: u16 = 9003;

/// The program run when `interpreter` is unset: `php` from `PATH`.
pub const DEFAULT_INTERPRETER: &str = "php";

/// How a container target runs PHP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerMode {
    /// `exec` into a running container or compose service.
    Exec,
    /// `run --rm -i` a fresh container.
    Run,
}

/// Where PHP runs when it is not the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRef {
    /// A `[containers.target]` id.
    pub target_id: String,
    /// `None` lets the target's source decide.
    pub mode: Option<ContainerMode>,
}

/// One server's toggles; `None` is the server's shipped default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServerToggles {
    pub enabled: Option<bool>,
    pub diagnostics: Option<bool>,
}

/// The PHP settings in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPhp {
    /// Program to spawn as `php`; never empty.
    pub interpreter: String,
    /// `"8.3"`, or `None` to read `composer.json`.
    pub language_level: Option<String>,
    pub include_paths: Vec<String>,
    pub stubs: Option<Vec<String>>,
    pub container: Option<ContainerRef>,
    pub xdebug_port: u16,
    pub formatter: Option<String>,
    pub servers: BTreeMap<String, ServerToggles>,
}

impl ResolvedPhp {
    /// Whether the server `id` runs; `default` is its shipped default.
    pub fn server_enabled(&self, id: &str, default: bool) -> bool {
        self.servers
            .get(id)
            .and_then(|s| s.enabled)
            .unwrap_or(default)
    }

    /// Whether the server `id`'s own diagnostics are shown; `default` is
    /// its shipped default (Phpactor: `false`).
    pub fn server_diagnostics(&self, id: &str, default: bool) -> bool {
        self.servers
            .get(id)
            .and_then(|s| s.diagnostics)
            .unwrap_or(default)
    }
}

/// Resolve the `[php]` section of already scope-resolved `settings`.
pub fn resolve(settings: &Settings) -> ResolvedPhp {
    let php: &PhpSettings = &settings.php;
    ResolvedPhp {
        interpreter: non_blank(&php.interpreter)
            .unwrap_or(DEFAULT_INTERPRETER)
            .to_string(),
        language_level: non_blank(&php.language_level)
            .filter(|level| is_language_level(level))
            .map(str::to_string),
        include_paths: php.include_paths.clone(),
        stubs: php.stubs.clone(),
        container: non_blank(&php.container_target).map(|id| ContainerRef {
            target_id: id.to_string(),
            mode: match non_blank(&php.container_mode) {
                Some("exec") => Some(ContainerMode::Exec),
                Some("run") => Some(ContainerMode::Run),
                _ => None,
            },
        }),
        xdebug_port: php
            .xdebug_port
            .filter(|port| *port != 0)
            .unwrap_or(DEFAULT_XDEBUG_PORT),
        formatter: non_blank(&php.formatter).map(str::to_string),
        servers: php
            .servers
            .iter()
            .map(|(id, s)| {
                (
                    id.clone(),
                    ServerToggles {
                        enabled: s.enabled,
                        diagnostics: s.diagnostics,
                    },
                )
            })
            .collect(),
    }
}

fn non_blank(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// `major.minor`, digits only (`8.3`); a patch version or a constraint
/// like `^8.1` is not a language level.
fn is_language_level(value: &str) -> bool {
    let mut parts = value.split('.');
    let digits =
        |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_config::php::PhpServerSetting;

    fn with(php: PhpSettings) -> Settings {
        Settings {
            php,
            ..Settings::default()
        }
    }

    #[test]
    fn defaults_when_nothing_is_set() {
        let r = resolve(&Settings::default());
        assert_eq!(r.interpreter, "php");
        assert_eq!(r.xdebug_port, 9003);
        assert_eq!(r.language_level, None);
        assert_eq!(r.container, None);
        assert!(r.server_enabled("intelephense", true));
        assert!(!r.server_diagnostics("phpactor", false));
    }

    #[test]
    fn a_blank_interpreter_reads_as_unset() {
        let r = resolve(&with(PhpSettings {
            interpreter: Some("  ".into()),
            ..PhpSettings::default()
        }));
        assert_eq!(r.interpreter, "php");
    }

    #[test]
    fn a_configured_interpreter_and_port_win() {
        let r = resolve(&with(PhpSettings {
            interpreter: Some("/opt/php83/bin/php".into()),
            xdebug_port: Some(9100),
            ..PhpSettings::default()
        }));
        assert_eq!(r.interpreter, "/opt/php83/bin/php");
        assert_eq!(r.xdebug_port, 9100);
    }

    #[test]
    fn port_zero_reads_as_the_default() {
        let r = resolve(&with(PhpSettings {
            xdebug_port: Some(0),
            ..PhpSettings::default()
        }));
        assert_eq!(r.xdebug_port, 9003);
    }

    #[test]
    fn only_major_dot_minor_is_a_language_level() {
        for (input, expected) in [
            ("8.3", Some("8.3")),
            ("8.3.1", None),
            ("^8.1", None),
            ("8", None),
            ("", None),
        ] {
            let r = resolve(&with(PhpSettings {
                language_level: Some(input.into()),
                ..PhpSettings::default()
            }));
            assert_eq!(r.language_level.as_deref(), expected, "{input:?}");
        }
    }

    #[test]
    fn a_container_target_carries_its_mode_and_ignores_an_unknown_one() {
        let target = |mode: Option<&str>| {
            resolve(&with(PhpSettings {
                container_target: Some("t1".into()),
                container_mode: mode.map(String::from),
                ..PhpSettings::default()
            }))
            .container
            .unwrap()
        };
        assert_eq!(target(Some("exec")).mode, Some(ContainerMode::Exec));
        assert_eq!(target(Some("run")).mode, Some(ContainerMode::Run));
        assert_eq!(target(Some("bogus")).mode, None);
        assert_eq!(target(None).target_id, "t1");
    }

    #[test]
    fn server_toggles_override_the_shipped_default() {
        let r = resolve(&with(PhpSettings {
            servers: BTreeMap::from([(
                "phpactor".to_string(),
                PhpServerSetting {
                    enabled: Some(false),
                    diagnostics: Some(true),
                },
            )]),
            ..PhpSettings::default()
        }));
        assert!(!r.server_enabled("phpactor", true));
        assert!(r.server_diagnostics("phpactor", false));
        assert!(r.server_enabled("intelephense", true));
    }
}
