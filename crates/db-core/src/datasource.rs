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

use app_config::database::{DataSourceSetting, SshSetting};

use crate::tunnel::{SshAuthMode, SshConfig};

/// The three credentials a data source can need, already resolved by the
/// caller (`ui-shell`, from `secret-store`) — `None` for "no secret
/// configured or the keychain had nothing for this key", never a
/// distinguishable "keychain unavailable" here; that distinction is
/// `secret-store::SecretError`'s to make, one layer up.
///
/// `ssh_password` doubles as both an SSH password ([`SshAuthMode::
/// Password`]) and a private key's passphrase ([`SshAuthMode::KeyFile`]) —
/// [`SshConfig::password`] carries the same single field for the same
/// reason (only one of the two modes is ever active for a given source).
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
    /// The SSH tunnel to open before connecting, if this source is
    /// configured to go through one — `None` from the plain [`From`] impl
    /// below (no secret available there); [`Self::from_setting`] is the
    /// constructor that actually resolves it.
    pub ssh: Option<SshConfig>,
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
            ssh: None,
        }
    }
}

fn ssh_config(setting: &SshSetting, password: Option<String>) -> SshConfig {
    SshConfig {
        host: setting.host.clone(),
        port: setting.port.unwrap_or(22),
        user: setting.user.clone(),
        auth: SshAuthMode::from_id(&setting.auth),
        key_file: (!setting.key_file.is_empty()).then(|| setting.key_file.clone()),
        password,
    }
}

impl DataSource {
    /// [`From<&DataSourceSetting>`] plus the SSH tunnel field, resolved from
    /// `setting.ssh` and `secrets.ssh_password` (a password or a key
    /// passphrase, depending on [`SshAuthMode`] — see [`Secrets`]'s doc
    /// comment). The plain `From` impl cannot do this itself: it has no
    /// secret to fold in (`db-core` never reads the keychain — this
    /// module's own doc comment), so every caller that actually opens a
    /// tunnel (F7c: `ui-shell::bridge::database::open_session` and its
    /// callers) must build a `DataSource` through this constructor instead.
    pub fn from_setting(setting: &DataSourceSetting, secrets: &Secrets) -> Self {
        let mut source = Self::from(setting);
        source.ssh = setting
            .ssh
            .as_ref()
            .map(|ssh| ssh_config(ssh, secrets.ssh_password.clone()));
        source
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

    fn setting_with_ssh() -> DataSourceSetting {
        DataSourceSetting {
            ssh: Some(app_config::database::SshSetting {
                host: "bastion.example".to_string(),
                port: Some(2222),
                user: "florian".to_string(),
                auth: "key".to_string(),
                key_file: "/home/florian/.ssh/id_ed25519".to_string(),
            }),
            ..setting()
        }
    }

    #[test]
    fn from_setting_is_none_when_the_source_has_no_ssh_configured() {
        let source = DataSource::from_setting(&setting(), &Secrets::default());
        assert!(source.ssh.is_none());
    }

    #[test]
    fn from_setting_maps_ssh_config_and_folds_in_the_secret() {
        let secrets = Secrets {
            ssh_password: Some("passphrase".to_string()),
            ..Default::default()
        };
        let source = DataSource::from_setting(&setting_with_ssh(), &secrets);
        let ssh = source.ssh.expect("ssh config");
        assert_eq!(ssh.host, "bastion.example");
        assert_eq!(ssh.port, 2222);
        assert_eq!(ssh.user, "florian");
        assert_eq!(ssh.auth, SshAuthMode::KeyFile);
        assert_eq!(
            ssh.key_file.as_deref(),
            Some("/home/florian/.ssh/id_ed25519")
        );
        assert_eq!(ssh.password.as_deref(), Some("passphrase"));
    }

    #[test]
    fn from_setting_defaults_the_ssh_port_to_22_when_unset() {
        let mut setting = setting_with_ssh();
        setting.ssh.as_mut().unwrap().port = None;
        let source = DataSource::from_setting(&setting, &Secrets::default());
        assert_eq!(source.ssh.unwrap().port, 22);
    }

    #[test]
    fn from_setting_leaves_an_empty_key_file_as_none() {
        let mut setting = setting_with_ssh();
        setting.ssh.as_mut().unwrap().key_file.clear();
        let source = DataSource::from_setting(&setting, &Secrets::default());
        assert_eq!(source.ssh.unwrap().key_file, None);
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
