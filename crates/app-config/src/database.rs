//! The `[database]` section (Database Tools plan F1.4, database-tools.md
//! §8, ADR-0061 §1).
//!
//! Persistence only, like [`crate::containers`]: what a driver id or SSL
//! mode *means* is `db-core`'s/`settings-model`'s job, not this file's.
//! [`DataSourceSetting`] structurally has **no secret field at all** — not
//! a blank one by convention, the type has no such member — so a
//! `settings.toml`, however produced or hand-edited, cannot carry a
//! password, SSH passphrase or client-TLS key password even by accident.
//! Those three live in the OS keychain under service `ide.database`, keys
//! `<id>`, `<id>/ssh`, `<id>/ssl-key` (ADR-0061 §1).
//!
//! Global by default, project-overridable: a project's own `[[database.
//! sources]]` rows merge with the global list *by id* rather than
//! replacing it wholesale (`settings_model::scope::resolve_database_sources`,
//! the same union-by-key rule named layouts already use, ADR-0045 §2) —
//! unlike `[containers]`, which replaces the whole section, a data source
//! is an item in a collection a teammate's checkout should be able to add
//! to without hiding the ones the user configured for themselves.
//! `file_sources` (project-only: which console file defaults to which
//! source id) has no global counterpart at all.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The `[database]` section.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct DatabaseSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_cap_mib: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_close_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_cap: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_third_party_drivers: Option<bool>,
    /// Named data sources, in the order the user added them.
    #[serde(default, rename = "sources", skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<DataSourceSetting>,
}

/// `#[serde(skip_serializing_if)]` predicate for `Settings::database`.
pub fn is_default(value: &DatabaseSettings) -> bool {
    value == &DatabaseSettings::default()
}

pub const DEFAULT_PAGE_SIZE: u32 = 500;
pub const DEFAULT_MEMORY_CAP_MIB: u32 = 256;
pub const DEFAULT_IDLE_CLOSE_MINUTES: u32 = 30;
pub const DEFAULT_HISTORY_CAP: u32 = 1000;
pub const DEFAULT_ALLOW_THIRD_PARTY_DRIVERS: bool = false;

impl DatabaseSettings {
    pub fn page_size_or_default(&self) -> u32 {
        self.page_size.unwrap_or(DEFAULT_PAGE_SIZE)
    }
    pub fn memory_cap_mib_or_default(&self) -> u32 {
        self.memory_cap_mib.unwrap_or(DEFAULT_MEMORY_CAP_MIB)
    }
    pub fn idle_close_minutes_or_default(&self) -> u32 {
        self.idle_close_minutes
            .unwrap_or(DEFAULT_IDLE_CLOSE_MINUTES)
    }
    pub fn history_cap_or_default(&self) -> u32 {
        self.history_cap.unwrap_or(DEFAULT_HISTORY_CAP)
    }
    pub fn allow_third_party_drivers_or_default(&self) -> bool {
        self.allow_third_party_drivers
            .unwrap_or(DEFAULT_ALLOW_THIRD_PARTY_DRIVERS)
    }
}

pub const DEFAULT_SCRIPT_POLICY: &str = "stop_on_error";
/// The data editor's default when a table has no primary key (F4.2):
/// refuse to offer it as editable at all, rather than silently matching a
/// `WHERE` on every column (database-tools.md §4's `NoPrimaryKey` policy).
pub const DEFAULT_NO_PRIMARY_KEY_POLICY: &str = "refuse";

impl DataSourceSetting {
    pub fn script_policy_or_default(&self) -> &str {
        if self.script_policy.is_empty() {
            DEFAULT_SCRIPT_POLICY
        } else {
            self.script_policy.as_str()
        }
    }

    pub fn no_primary_key_policy_or_default(&self) -> &str {
        if self.no_primary_key_policy.is_empty() {
            DEFAULT_NO_PRIMARY_KEY_POLICY
        } else {
            self.no_primary_key_policy.as_str()
        }
    }
}

/// One named data source. No password, passphrase or key-password field —
/// see this module's doc comment.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct DataSourceSetting {
    /// Stable id, also the OS-keychain key. Never reused after a source is
    /// removed.
    pub id: String,
    pub name: String,
    /// Which driver row (`plugin_api`'s `database-drivers` contribution,
    /// e.g. `"postgresql"`, `"sqlite"`) — a plain string, like every other
    /// kind tag this codebase persists (ADR-0017/ADR-0039's "persistence
    /// stays dumb" rule).
    pub driver: String,
    /// `"work/analytics"`-shaped path the tree groups sources under.
    #[serde(default)]
    pub group: String,
    /// `#rrggbb`, empty for "no colour".
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub user: String,
    /// `"password"`, `"agent"`, `"none"`, … — free-form like `driver`.
    #[serde(default)]
    pub auth: String,
    #[serde(default)]
    pub read_only: bool,
    /// Whether executed statements are appended to this source's history
    /// file. `true` by default: turning history off is the opt-out, the
    /// same "least surprising first run" default `ContainerSettings`'s
    /// dock filters use.
    #[serde(default = "default_true")]
    pub history: bool,
    /// A script's on-error policy: `"stop_on_error"` (default), `"continue"`
    /// or `"ask"` — free-form like `driver`/`auth`, the console bar's own
    /// vocabulary (`ui_shell::bridge::ffi::FfiDbScriptPolicy`) rather than a
    /// typed enum here, the same "persistence stays dumb" rule every other
    /// kind tag in this struct already follows.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script_policy: String,
    /// The data editor's own policy (F4.2) for a table with no primary
    /// key: `"refuse"` (default, the grid opens read-only with that as
    /// its reason) or `"all_columns_where"` (opt in to `db_core::dml::
    /// EditBuffer`'s all-original-columns `WHERE` fallback) — free-form
    /// like `script_policy`, same reasoning.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub no_primary_key_policy: String,
    /// A full connection URL, when the driver takes one instead of
    /// host/port/database (e.g. a SQLite file path, or an ADBC DSN).
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "is_default_ssl")]
    pub ssl: SslSetting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshSetting>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
}

fn default_true() -> bool {
    true
}

fn is_default_ssl(ssl: &SslSetting) -> bool {
    ssl == &SslSetting::default()
}

/// `[database.sources.ssl]`. Mode is a plain string (`"disable"`,
/// `"prefer"`, `"require"`, `"verify-ca"`, `"verify-full"`) — the typed
/// vocabulary is `settings_model::database`'s, this file only stores what
/// was chosen (ADR-0061 §2: no "skip verification" mode exists at all).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct SslSetting {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub ca_file: String,
}

/// `[database.sources.ssh]`. No password/passphrase field — the SSH
/// credential lives in the keychain under `<id>/ssh` (ADR-0061 §1).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct SshSetting {
    #[serde(default)]
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: String,
    /// `"agent"`, `"password"`, `"key"` — free-form like `DataSourceSetting::auth`.
    #[serde(default)]
    pub auth: String,
    #[serde(default)]
    pub key_file: String,
}

/// The project's `[database]` override (`ProjectSettings::database`):
/// sources this project adds or overrides (merged by id with the global
/// list, never replacing it — see this module's doc comment) plus
/// `file_sources`, which has no global counterpart at all.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct DatabaseProjectSettings {
    #[serde(default, rename = "sources", skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<DataSourceSetting>,
    /// Console-file-relative path (e.g. `"sql/reports.sql"`) to the data
    /// source id it defaults to.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_sources: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_section_writes_nothing() {
        let text = toml::to_string(&DatabaseSettings::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn defaults_apply_when_unset() {
        let settings = DatabaseSettings::default();
        assert_eq!(settings.page_size_or_default(), DEFAULT_PAGE_SIZE);
        assert_eq!(settings.memory_cap_mib_or_default(), DEFAULT_MEMORY_CAP_MIB);
        assert_eq!(
            settings.idle_close_minutes_or_default(),
            DEFAULT_IDLE_CLOSE_MINUTES
        );
        assert_eq!(settings.history_cap_or_default(), DEFAULT_HISTORY_CAP);
        assert!(!settings.allow_third_party_drivers_or_default());
    }

    #[test]
    fn a_data_source_round_trips_with_ssl_and_ssh() {
        let settings = DatabaseSettings {
            sources: vec![DataSourceSetting {
                id: "3f0c".to_string(),
                name: "prod-replica".to_string(),
                driver: "postgresql".to_string(),
                group: "work/analytics".to_string(),
                color: "#3a7bd5".to_string(),
                host: "db.internal".to_string(),
                port: Some(5432),
                database: "shop".to_string(),
                user: "florian".to_string(),
                auth: "password".to_string(),
                read_only: true,
                history: false,
                script_policy: "ask".to_string(),
                no_primary_key_policy: "all_columns_where".to_string(),
                url: String::new(),
                ssl: SslSetting {
                    mode: "verify-full".to_string(),
                    ca_file: "/etc/ca.pem".to_string(),
                },
                ssh: Some(SshSetting {
                    host: "bastion".to_string(),
                    port: Some(22),
                    user: "florian".to_string(),
                    auth: "agent".to_string(),
                    key_file: String::new(),
                }),
                options: BTreeMap::from([("application_name".to_string(), "ide".to_string())]),
            }],
            ..Default::default()
        };
        let text = toml::to_string(&settings).expect("serialize");
        let parsed: DatabaseSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, settings);
    }

    #[test]
    fn a_data_source_carries_no_secret_field() {
        // Compile-time proof by construction: every field here is one this
        // test named, so a secret field could only exist if this test's
        // literal failed to build.
        let _ = DataSourceSetting {
            id: String::new(),
            name: String::new(),
            driver: String::new(),
            group: String::new(),
            color: String::new(),
            host: String::new(),
            port: None,
            database: String::new(),
            user: String::new(),
            auth: String::new(),
            read_only: false,
            history: true,
            script_policy: String::new(),
            no_primary_key_policy: String::new(),
            url: String::new(),
            ssl: SslSetting::default(),
            ssh: None,
            options: BTreeMap::new(),
        };
    }

    #[test]
    fn script_policy_defaults_to_stop_on_error_when_unset() {
        let setting = DataSourceSetting::default();
        assert_eq!(setting.script_policy_or_default(), DEFAULT_SCRIPT_POLICY);
    }

    #[test]
    fn an_unset_script_policy_writes_nothing() {
        let setting = DataSourceSetting {
            id: "s1".to_string(),
            ..Default::default()
        };
        let text = toml::to_string(&setting).expect("serialize");
        assert!(!text.contains("script_policy"));
    }

    #[test]
    fn script_policy_round_trips() {
        let setting = DataSourceSetting {
            id: "s1".to_string(),
            script_policy: "ask".to_string(),
            ..Default::default()
        };
        let text = toml::to_string(&setting).expect("serialize");
        let parsed: DataSourceSetting = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.script_policy, "ask");
        assert_eq!(parsed.script_policy_or_default(), "ask");
    }

    #[test]
    fn no_primary_key_policy_defaults_to_refuse_when_unset() {
        let setting = DataSourceSetting::default();
        assert_eq!(
            setting.no_primary_key_policy_or_default(),
            DEFAULT_NO_PRIMARY_KEY_POLICY
        );
    }

    #[test]
    fn an_unset_no_primary_key_policy_writes_nothing() {
        let setting = DataSourceSetting {
            id: "s1".to_string(),
            ..Default::default()
        };
        let text = toml::to_string(&setting).expect("serialize");
        assert!(!text.contains("no_primary_key_policy"));
    }

    #[test]
    fn no_primary_key_policy_round_trips() {
        let setting = DataSourceSetting {
            id: "s1".to_string(),
            no_primary_key_policy: "all_columns_where".to_string(),
            ..Default::default()
        };
        let text = toml::to_string(&setting).expect("serialize");
        let parsed: DataSourceSetting = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.no_primary_key_policy, "all_columns_where");
        assert_eq!(
            parsed.no_primary_key_policy_or_default(),
            "all_columns_where"
        );
    }

    #[test]
    fn project_file_sources_round_trip() {
        let project = DatabaseProjectSettings {
            sources: vec![],
            file_sources: BTreeMap::from([("sql/reports.sql".to_string(), "3f0c".to_string())]),
        };
        let text = toml::to_string(&project).expect("serialize");
        let parsed: DatabaseProjectSettings = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, project);
    }
}
