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

/// The `php-builtin-server` run configuration's own sub-table (PHP parity
/// plan, I5): blank/zero fields mean "the default", and what the default
/// is (`localhost`, 8000, the project root) is `run-core`'s rule. The
/// `php-console` kind needs no sub-table.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpBuiltinServerRunSetting {
    /// Listen address; blank is `localhost`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// Listen port; 0 is 8000.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub port: u16,
    /// Document root (`-t`), macros allowed; blank is the project root.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub document_root: String,
    /// Router script, macros allowed; blank serves files directly.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub router: String,
}

/// The `kind` of a PHPUnit run configuration (ADR-0073).
pub const PHPUNIT_KIND: &str = "phpunit";

/// PHPUnit's own automatic config-file discovery order (no `-c` given,
/// PHPUnit's `Configuration::locateConfigurationFile`): the bare filename
/// before its `.dist` counterparts, since a tracked default should not
/// shadow a developer's own untracked override. `phpunit.dist.xml` is the
/// PHPUnit 10+ spelling of `phpunit.xml.dist`.
pub const PHPUNIT_CONFIG_CANDIDATES: &[&str] =
    &["phpunit.xml", "phpunit.dist.xml", "phpunit.xml.dist"];

/// The `phpunit` run configuration's own sub-table (PHPUnit editor runs plan,
/// E4): what to run and how. The scope is a string for the reason
/// `RunConfigSetting::kind` is one (ADR-0039): persistence stays dumb, and
/// `php-core` maps it back, so an unknown scope loads as the default
/// instead of failing the settings file. Environment variables are the
/// configuration's own `env`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpUnitRunSetting {
    /// `directory`, `file`, `class`, `method`, `data_set` or `config`;
    /// blank and unknown mean `config`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// Directory or file scope: project-relative with `/`, macros allowed.
    /// Class, method and data-set scopes: the test file, to narrow the run.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// The class's fully qualified name (`App\Tests\GreeterTest`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// The text after `with data set `: `"name"` (quotes included) or `#0`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data_set: String,
    /// A PCRE for `--filter` selecting tests inside a file-scope run (a Pest
    /// `it()` has no class to name).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
    /// Config scope: the test suite to run (`--testsuite`); blank runs all.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub testsuite: String,
    /// An alternative `phpunit.xml` (`--configuration`), macros allowed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub configuration: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_groups: Vec<String>,
    /// Extra arguments, split like a shell line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub runner_options: String,
    /// The `php` program for this configuration; blank uses `[php]`'s.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub interpreter: String,
    /// PhpStorm's "Interpreter options": arguments for the interpreter
    /// itself (`-d memory_limit=-1`), placed before the test runner and
    /// split like a shell line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub interpreter_options: String,
    /// PhpStorm's "Custom working directory": project-relative with `/`,
    /// macros allowed; blank is the project root. A monorepo package's runs
    /// start in the package.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_directory: String,
}

fn is_zero(port: &u16) -> bool {
    *port == 0
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

    #[test]
    fn an_untouched_table_writes_nothing() {
        let text = toml::to_string(&PhpBuiltinServerRunSetting::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn a_phpunit_table_round_trips_and_an_untouched_one_writes_nothing() {
        assert_eq!(
            toml::to_string(&PhpUnitRunSetting::default())
                .unwrap()
                .trim(),
            ""
        );
        let setting = PhpUnitRunSetting {
            scope: "method".into(),
            path: "tests/GreeterTest.php".into(),
            class: "App\\GreeterTest".into(),
            method: "testGreets".into(),
            data_set: "\"a\"".into(),
            filter: "x".into(),
            testsuite: "Unit".into(),
            configuration: "ci.xml".into(),
            groups: vec!["fast".into()],
            exclude_groups: vec!["slow".into()],
            runner_options: "--stop-on-failure".into(),
            interpreter: "php8.3".into(),
            interpreter_options: "-d memory_limit=-1".into(),
            working_directory: "packages/mailer".into(),
        };
        let text = toml::to_string(&setting).unwrap();
        assert_eq!(toml::from_str::<PhpUnitRunSetting>(&text).unwrap(), setting);
    }

    #[test]
    fn round_trips_every_field() {
        let setting = PhpBuiltinServerRunSetting {
            host: "0.0.0.0".into(),
            port: 8080,
            document_root: "$PROJECT_DIR$/public".into(),
            router: "router.php".into(),
        };
        let text = toml::to_string(&setting).expect("serialize");
        assert_eq!(
            toml::from_str::<PhpBuiltinServerRunSetting>(&text).unwrap(),
            setting
        );
    }
}
