//! The rules behind the `[php]` section (PHP parity plan, P0-5): what an
//! unset or malformed value means.
//!
//! `app_config::PhpSettings` stores whatever the file says; [`resolve`]
//! turns that into the values consumers act on. A bad entry (an empty
//! interpreter, a language level that is not `major.minor`, port 0) reads
//! as unset rather than failing the whole section.

use std::collections::BTreeMap;

use app_config::php::{PhpServerSetting, PhpSettings};
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

/// The language servers the PHP page toggles: `(id, shown name, shipped
/// default for diagnostics)`. Both run by default (ADR-0066).
pub const PHP_SERVERS: [(&str, &str, bool); 2] = [
    ("intelephense", "Intelephense", true),
    ("phpactor", "Phpactor", false),
];

/// What the Settings > PHP page edits, as the page shows it: text fields
/// stay text so the rules for reading them live here, not in the view.
/// Fields the page does not show (`xdebug_port`, servers it
/// does not know) are untouched by [`PhpForm::apply_to`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpForm {
    pub interpreter: String,
    pub language_level: String,
    /// One path per line.
    pub include_paths: String,
    /// Comma- or whitespace-separated extension names; blank leaves the
    /// server's own list.
    pub stubs: String,
    pub container_target: String,
    /// `exec`, `run` or blank.
    pub container_mode: String,
    /// A `formatters` contribution id, or blank for the language server.
    pub formatter: String,
    pub intelephense_enabled: bool,
    pub intelephense_diagnostics: bool,
    pub phpactor_enabled: bool,
    pub phpactor_diagnostics: bool,
}

/// Why a [`PhpForm`] cannot be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhpFormError {
    /// The language level is not `major.minor`.
    InvalidLanguageLevel(String),
}

impl std::fmt::Display for PhpFormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLanguageLevel(v) => write!(
                f,
                "\"{v}\" is not a language level; write it as major.minor, for example 8.3"
            ),
        }
    }
}

impl std::error::Error for PhpFormError {}

impl PhpForm {
    pub fn from_settings(php: &PhpSettings) -> Self {
        let toggles = |id: &str, default_diagnostics: bool| {
            let server = php.servers.get(id);
            (
                server.and_then(|s| s.enabled).unwrap_or(true),
                server
                    .and_then(|s| s.diagnostics)
                    .unwrap_or(default_diagnostics),
            )
        };
        let (intelephense_enabled, intelephense_diagnostics) =
            toggles(PHP_SERVERS[0].0, PHP_SERVERS[0].2);
        let (phpactor_enabled, phpactor_diagnostics) = toggles(PHP_SERVERS[1].0, PHP_SERVERS[1].2);
        Self {
            interpreter: php.interpreter.clone().unwrap_or_default(),
            language_level: php.language_level.clone().unwrap_or_default(),
            include_paths: php.include_paths.join("\n"),
            stubs: php.stubs.as_ref().map(|s| s.join(", ")).unwrap_or_default(),
            container_target: php.container_target.clone().unwrap_or_default(),
            container_mode: php.container_mode.clone().unwrap_or_default(),
            formatter: php.formatter.clone().unwrap_or_default(),
            intelephense_enabled,
            intelephense_diagnostics,
            phpactor_enabled,
            phpactor_diagnostics,
        }
    }

    pub fn validate(&self) -> Result<(), PhpFormError> {
        let level = self.language_level.trim();
        if level.is_empty() || is_language_level(level) {
            Ok(())
        } else {
            Err(PhpFormError::InvalidLanguageLevel(level.to_string()))
        }
    }

    /// Write the form into `php`. Blank text is "unset", and a server
    /// toggle equal to the shipped default is stored as unset, so a later
    /// change of the default reaches a user who never chose.
    pub fn apply_to(&self, php: &mut PhpSettings) {
        let text = |value: &str| Some(value.trim().to_string()).filter(|v| !v.is_empty());
        php.interpreter = text(&self.interpreter);
        php.language_level = text(&self.language_level);
        php.include_paths = self
            .include_paths
            .lines()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        let stubs: Vec<String> = self
            .stubs
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        php.stubs = (!stubs.is_empty()).then_some(stubs);
        php.container_target = text(&self.container_target);
        php.container_mode = text(&self.container_mode).filter(|_| {
            php.container_target.is_some() && matches!(self.container_mode.trim(), "exec" | "run")
        });
        php.formatter = text(&self.formatter);
        let wanted = [
            (
                PHP_SERVERS[0],
                self.intelephense_enabled,
                self.intelephense_diagnostics,
            ),
            (
                PHP_SERVERS[1],
                self.phpactor_enabled,
                self.phpactor_diagnostics,
            ),
        ];
        for ((id, _, default_diagnostics), enabled, diagnostics) in wanted {
            let entry = php.servers.entry(id.to_string()).or_default();
            entry.enabled = (!enabled).then_some(false);
            entry.diagnostics = (diagnostics != default_diagnostics).then_some(diagnostics);
            if *entry == PhpServerSetting::default() {
                php.servers.remove(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_form_round_trips_and_untouched_defaults_write_nothing() {
        let mut php = PhpSettings {
            xdebug_port: Some(9100),
            ..PhpSettings::default()
        };
        let form = PhpForm::from_settings(&php);
        assert!(form.intelephense_enabled && form.intelephense_diagnostics);
        assert!(form.phpactor_enabled && !form.phpactor_diagnostics);
        form.apply_to(&mut php);
        assert_eq!(
            php,
            PhpSettings {
                xdebug_port: Some(9100),
                ..PhpSettings::default()
            }
        );
    }

    #[test]
    fn form_text_is_read_by_the_rules() {
        let mut php = PhpSettings::default();
        PhpForm {
            interpreter: " /opt/php ".into(),
            language_level: "8.3".into(),
            include_paths: "vendor\n\n  lib  \n".into(),
            stubs: "redis, mongodb  gd".into(),
            container_target: "t1".into(),
            container_mode: "exec".into(),
            formatter: " pint ".into(),
            intelephense_enabled: false,
            intelephense_diagnostics: true,
            phpactor_enabled: true,
            phpactor_diagnostics: true,
        }
        .apply_to(&mut php);
        assert_eq!(php.interpreter.as_deref(), Some("/opt/php"));
        assert_eq!(php.include_paths, ["vendor", "lib"]);
        assert_eq!(
            php.stubs.as_deref(),
            Some(&["redis".to_string(), "mongodb".into(), "gd".into()][..])
        );
        assert_eq!(php.container_mode.as_deref(), Some("exec"));
        assert_eq!(php.formatter.as_deref(), Some("pint"));
        assert_eq!(PhpForm::from_settings(&php).formatter, "pint");
        assert_eq!(php.servers["intelephense"].enabled, Some(false));
        assert_eq!(php.servers["intelephense"].diagnostics, None);
        assert_eq!(php.servers["phpactor"].diagnostics, Some(true));
    }

    #[test]
    fn a_container_mode_without_a_target_or_with_a_bogus_word_is_dropped() {
        let mut php = PhpSettings::default();
        let mut form = PhpForm::from_settings(&php);
        form.container_mode = "exec".into();
        form.apply_to(&mut php);
        assert_eq!(php.container_mode, None);
        form.container_target = "t".into();
        form.container_mode = "bogus".into();
        form.apply_to(&mut php);
        assert_eq!(php.container_mode, None);
    }

    #[test]
    fn a_bad_language_level_is_refused_and_blank_is_fine() {
        let mut form = PhpForm::from_settings(&PhpSettings::default());
        assert_eq!(form.validate(), Ok(()));
        form.language_level = "^8.1".into();
        assert!(matches!(
            form.validate(),
            Err(PhpFormError::InvalidLanguageLevel(_))
        ));
    }
}
