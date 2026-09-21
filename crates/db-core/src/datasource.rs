//! `DataSource`: a typed view over `app_config::DataSourceSetting`, and
//! `ConnectSpec`/`SslConfig` — what a `Driver::connect` call actually
//! needs, with the secrets `app-config` structurally never stores folded
//! in by the caller (ADR-0061 §1).
//!
//! `db-core` does not depend on `secret-store`: the crate that owns a rule
//! never owns the OS integration reading its input, the same split
//! `analysis-core` already has with `app-config`'s gitignore-pattern
//! helper (`layering.md`'s `db-core` row) — `ui-shell` reads the keychain
//! and hands the result in as [`Secrets`].

use app_config::database::DataSourceSetting;

/// The three credentials a data source can need, already resolved by the
/// caller (`ui-shell`, from `secret-store`) — `None` for "no secret
/// configured or the keychain had nothing for this key", never a
/// distinguishable "keychain unavailable" here; that distinction is
/// `secret-store::SecretError`'s to make, one layer up.
#[derive(Clone, Default)]
pub struct Secrets {
    pub password: Option<String>,
    pub ssh_password: Option<String>,
    pub ssl_key_password: Option<String>,
}

// `Secrets` never derives `Debug`: a default derive would print every
// field's value, and a panic message or `{:?}` log built from a struct
// that carries a password is exactly the leak ADR-0061 §1 rules out. If a
// caller genuinely needs to log that a secret was present, it reads the
// `Option`'s `is_some()` itself rather than formatting this type.

/// TLS posture (ADR-0061 §2): no "skip verification" mode exists — the
/// closest is [`SslMode::Disable`] (no TLS at all, a different risk than
/// "TLS negotiated, certificate ignored").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    pub fn from_id(id: &str) -> Self {
        match id {
            "disable" => SslMode::Disable,
            "require" => SslMode::Require,
            "verify-ca" => SslMode::VerifyCa,
            "verify-full" => SslMode::VerifyFull,
            _ => SslMode::Prefer,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyCa => "verify-ca",
            SslMode::VerifyFull => "verify-full",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SslConfig {
    pub mode: SslMode,
    pub ca_file: Option<String>,
}

impl Default for SslConfig {
    fn default() -> Self {
        Self {
            mode: SslMode::Prefer,
            ca_file: None,
        }
    }
}

/// A typed, read-only view over one `DataSourceSetting` — the id/driver
/// tag and connection fields, unpacked into the shapes `db-core`/`ui-shell`
/// actually consume instead of every call site re-reading `app_config`'s
/// persistence struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSource {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    pub user: String,
    pub url: String,
    pub read_only: bool,
    pub ssl: SslConfig,
}

impl From<&DataSourceSetting> for DataSource {
    fn from(setting: &DataSourceSetting) -> Self {
        Self {
            id: setting.id.clone(),
            name: setting.name.clone(),
            driver: setting.driver.clone(),
            host: setting.host.clone(),
            port: setting.port,
            database: setting.database.clone(),
            user: setting.user.clone(),
            url: setting.url.clone(),
            read_only: setting.read_only,
            ssl: SslConfig {
                mode: SslMode::from_id(&setting.ssl.mode),
                ca_file: (!setting.ssl.ca_file.is_empty()).then(|| setting.ssl.ca_file.clone()),
            },
        }
    }
}

/// What `Driver::connect` actually needs: [`DataSource`]'s connection
/// fields plus the secrets `ui-shell` resolved. `Debug` redacts every
/// credential-shaped field — see this module's `Secrets` note.
#[derive(Clone)]
pub struct ConnectSpec {
    pub driver: String,
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    pub user: String,
    pub url: String,
    pub password: Option<String>,
    pub ssl: SslConfig,
}

impl ConnectSpec {
    pub fn from(source: &DataSource, secrets: &Secrets) -> Self {
        Self {
            driver: source.driver.clone(),
            host: source.host.clone(),
            port: source.port,
            database: source.database.clone(),
            user: source.user.clone(),
            url: source.url.clone(),
            password: secrets.password.clone(),
            ssl: source.ssl.clone(),
        }
    }
}

impl std::fmt::Debug for ConnectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectSpec")
            .field("driver", &self.driver)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("url", &self.url)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("ssl", &self.ssl)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting() -> DataSourceSetting {
        DataSourceSetting {
            id: "abc".to_string(),
            name: "prod".to_string(),
            driver: "postgresql".to_string(),
            host: "db.internal".to_string(),
            port: Some(5432),
            database: "shop".to_string(),
            user: "florian".to_string(),
            ssl: app_config::database::SslSetting {
                mode: "verify-full".to_string(),
                ca_file: "/etc/ca.pem".to_string(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn data_source_reads_the_ssl_mode_and_ca_file() {
        let source = DataSource::from(&setting());
        assert_eq!(source.ssl.mode, SslMode::VerifyFull);
        assert_eq!(source.ssl.ca_file.as_deref(), Some("/etc/ca.pem"));
    }

    #[test]
    fn an_empty_ca_file_is_none_not_an_empty_string() {
        let mut plain = setting();
        plain.ssl.ca_file.clear();
        plain.ssl.mode.clear();
        let source = DataSource::from(&plain);
        assert_eq!(source.ssl.mode, SslMode::Prefer);
        assert_eq!(source.ssl.ca_file, None);
    }

    #[test]
    fn connect_spec_carries_the_resolved_password() {
        let source = DataSource::from(&setting());
        let secrets = Secrets {
            password: Some("hunter2".to_string()),
            ..Default::default()
        };
        let spec = ConnectSpec::from(&source, &secrets);
        assert_eq!(spec.password.as_deref(), Some("hunter2"));
        assert_eq!(spec.host, "db.internal");
    }

    #[test]
    fn connect_spec_debug_never_prints_the_password() {
        let source = DataSource::from(&setting());
        let secrets = Secrets {
            password: Some("hunter2".to_string()),
            ..Default::default()
        };
        let spec = ConnectSpec::from(&source, &secrets);
        let rendered = format!("{spec:?}");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn connect_spec_debug_says_nothing_present_when_there_is_no_password() {
        let source = DataSource::from(&setting());
        let spec = ConnectSpec::from(&source, &Secrets::default());
        let rendered = format!("{spec:?}");
        assert!(!rendered.contains("<redacted>"));
        assert!(rendered.contains("password: None"));
    }
}
