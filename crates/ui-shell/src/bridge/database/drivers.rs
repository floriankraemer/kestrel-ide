//! Driver install UI + consent (Database Tools plan F8.5,
//! `database-tools.md` §9, ADR-0061 §4): `DriverInstallService` drives
//! `db_driver_adbc::{install, quarantine}` for one `adbc`-backend
//! `database-drivers` row at a time — a status line, an install (download →
//! verify sha256 → unpack → manifest, on a worker thread), and re-enabling a
//! quarantined driver.
//!
//! [`status_for`] is the pure half: no network, no filesystem write, so it
//! is unit-tested directly. The `DriverInstallService` QObject below is
//! translation only, per this crate's own rule.

use std::path::Path;
use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use plugin_api::DatabaseDriverContribution;

use crate::bridge::errors::{self, CODE_REFUSED};
use crate::bridge::ffi::{self, FfiResult};

use super::backend::{adbc_artifact_for_platform, ADBC_INSTALLED_SLOT};

/// The plugin id every `database-drivers`/`sql-dialects` row this build
/// ships lives on — anything else is a third-party plugin's own row,
/// gated by `[database] allow_third_party_drivers` (ADR-0061 §4).
pub const BUILTIN_PLUGIN_ID: &str = "database-tools";

/// This process's platform key, as the manifest's `adbc.artifacts` map
/// keys its rows (`database-tools.md` §7/§11).
pub fn current_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows_amd64",
        _ => "linux_amd64",
    }
}

/// One ADBC row's install status, computed from the filesystem only — no
/// network, and no shared-library load.
///
/// ponytail: there is no live "does the ADBC standard search already find
/// a system-installed copy" probe here — answering that for real would
/// mean loading an arbitrary shared library just to paint a status line.
/// `SystemSearch` is the honest label for that case: the real answer only
/// comes from `AdbcDriver`'s own quarantine-guarded load on an actual
/// connect attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverStatus {
    /// An IDE-managed install exists.
    Installed,
    /// Left marked from a previous crash; needs an explicit re-enable.
    Quarantined,
    /// Nothing installed, but this platform has a pinned artifact to fetch.
    Installable,
    /// Nothing installed, no artifact for this platform, but the manifest
    /// names a hint for what to do instead.
    InstallHint(String),
    /// Nothing installed, no artifact, no hint: the ADBC driver manager's
    /// own standard search is this row's only path (see this type's own
    /// doc comment).
    SystemSearch,
    /// Contributed by a plugin other than [`BUILTIN_PLUGIN_ID`] while
    /// `allow_third_party_drivers` is off — listed, but not installable.
    ThirdPartyBlocked,
    /// Not an `adbc`-backend row at all; no install concept applies.
    NotApplicable,
}

impl DriverStatus {
    /// The exact text F8.5's status line shows.
    pub fn status_text(&self) -> String {
        match self {
            DriverStatus::Installed => "installed".to_string(),
            DriverStatus::Quarantined => "quarantined — Re-enable".to_string(),
            DriverStatus::Installable => "not installed — Install…".to_string(),
            DriverStatus::InstallHint(hint) => format!("install hint: {hint}"),
            DriverStatus::SystemSearch => "found via ADBC search".to_string(),
            DriverStatus::ThirdPartyBlocked => {
                "installing third-party drivers is off (allow_third_party_drivers)".to_string()
            }
            DriverStatus::NotApplicable => String::new(),
        }
    }
}

/// Computes [`DriverStatus`] for one `database-drivers` row.
///
/// `plugin_id` is the id of the plugin that contributed `contribution` —
/// [`plugin_host::LoadedPlugin::id`] at the real call site, a plain string
/// here so this function needs no live registry to test.
pub fn status_for(
    contribution: &DatabaseDriverContribution,
    plugin_id: &str,
    allow_third_party: bool,
    config_dir: &Path,
) -> DriverStatus {
    if contribution.backend != "adbc" {
        return DriverStatus::NotApplicable;
    }
    if plugin_id != BUILTIN_PLUGIN_ID && !allow_third_party {
        return DriverStatus::ThirdPartyBlocked;
    }

    let manifest_name = contribution
        .adbc
        .as_ref()
        .and_then(|adbc| adbc.manifest_name.as_deref())
        .unwrap_or(contribution.id.as_str());

    if db_driver_adbc::quarantine::Quarantine::new(config_dir)
        .check(manifest_name)
        .is_err()
    {
        return DriverStatus::Quarantined;
    }

    let managed =
        db_driver_adbc::locate::managed_dir(config_dir, manifest_name, ADBC_INSTALLED_SLOT);
    if managed.join("manifest.toml").is_file() {
        return DriverStatus::Installed;
    }

    if adbc_artifact_for_platform(contribution, current_platform()).is_some() {
        return DriverStatus::Installable;
    }
    if let Some(hint) = contribution
        .adbc
        .as_ref()
        .and_then(|a| a.install_hint.clone())
    {
        return DriverStatus::InstallHint(hint);
    }
    DriverStatus::SystemSearch
}

fn to_ffi_status(status: DriverStatus) -> ffi::FfiDriverStatus {
    let installable = matches!(status, DriverStatus::Installable);
    let can_reenable = matches!(status, DriverStatus::Quarantined);
    ffi::FfiDriverStatus {
        text: QString::from(status.status_text().as_str()),
        installable,
        can_reenable,
    }
}

fn find_contribution(driver_id: &str) -> Option<(String, DatabaseDriverContribution)> {
    plugin_host::registry()
        .database_drivers()
        .find(|(_, contribution)| contribution.id == driver_id)
        .map(|(plugin, contribution)| (plugin.id().to_string(), contribution.clone()))
}

fn allow_third_party_drivers() -> bool {
    let config_dir = app_core::resolve_config_dir();
    app_config::load(&config_dir)
        .map(|settings| settings.database.allow_third_party_drivers_or_default())
        .unwrap_or(app_config::database::DEFAULT_ALLOW_THIRD_PARTY_DRIVERS)
}

/// Rust side of the `DriverInstallService` QObject: no owned state beyond
/// what every call recomputes fresh (the same freshness rule
/// `driver_catalog()` already follows) — installs run on their own
/// detached worker thread, reported back through `installFinished`.
#[derive(Default)]
pub struct DriverInstallServiceRust;

impl ffi::DriverInstallService {
    /// This row's current status line (F8.5).
    pub fn status(&self, driver_id: &QString) -> ffi::FfiDriverStatus {
        let driver_id = driver_id.to_string();
        let Some((plugin_id, contribution)) = find_contribution(&driver_id) else {
            return to_ffi_status(DriverStatus::NotApplicable);
        };
        let config_dir = app_core::resolve_config_dir();
        to_ffi_status(status_for(
            &contribution,
            &plugin_id,
            allow_third_party_drivers(),
            &config_dir,
        ))
    }

    /// What the consent dialog names before an install: the pinned
    /// artifact's own URL and sha256 for this platform, empty when this
    /// row has none (nothing to consent to — `install` would refuse it
    /// anyway).
    pub fn consent(&self, driver_id: &QString) -> ffi::FfiDriverConsent {
        let driver_id = driver_id.to_string();
        let Some((_, contribution)) = find_contribution(&driver_id) else {
            return ffi::FfiDriverConsent::default();
        };
        let Some(artifact) = adbc_artifact_for_platform(&contribution, current_platform()) else {
            return ffi::FfiDriverConsent::default();
        };
        ffi::FfiDriverConsent {
            url: QString::from(artifact.url.as_str()),
            sha256: QString::from(artifact.sha256.as_str()),
            publisher: QString::from(url_host(&artifact.url).as_str()),
        }
    }

    /// Downloads, verifies and installs this row's pinned artifact for the
    /// current platform, off the UI thread. Refuses immediately (no
    /// thread spawned) when third-party installs are off for a
    /// non-builtin plugin's row, or when this row has no artifact for
    /// this platform at all.
    pub fn install(self: Pin<&mut Self>, driver_id: &QString) -> FfiResult {
        let driver_id = driver_id.to_string();
        let Some((plugin_id, contribution)) = find_contribution(&driver_id) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such database driver");
        };
        let config_dir = app_core::resolve_config_dir();
        let allow_third_party = allow_third_party_drivers();
        match status_for(&contribution, &plugin_id, allow_third_party, &config_dir) {
            DriverStatus::ThirdPartyBlocked => {
                return errors::failure(
                    CODE_REFUSED,
                    "installing third-party drivers is off; enable \
                     `allow_third_party_drivers` on the Database settings page first",
                );
            }
            DriverStatus::Installable => {}
            _ => {
                return errors::failure(
                    CODE_REFUSED,
                    "this driver has no pinned artifact to install for this platform",
                );
            }
        }
        let manifest_name = contribution
            .adbc
            .as_ref()
            .and_then(|adbc| adbc.manifest_name.as_deref())
            .unwrap_or(contribution.id.as_str())
            .to_string();
        let Some(spec) = contribution
            .adbc
            .as_ref()
            .and_then(|adbc| adbc.artifacts.get(current_platform()))
            .map(|artifact| db_driver_adbc::install::DriverInstall {
                url: artifact.url.clone(),
                sha256: artifact.sha256.clone(),
                platform: current_platform().to_string(),
                library: artifact.library.clone(),
            })
        else {
            return errors::failure(
                CODE_REFUSED,
                "this driver has no artifact for this platform",
            );
        };

        let qt_thread = self.qt_thread();
        let thread_driver_id = driver_id.clone();
        std::thread::spawn(move || {
            let outcome = db_driver_adbc::install::install(
                &config_dir,
                &manifest_name,
                ADBC_INSTALLED_SLOT,
                &spec,
            );
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DriverInstallService>| {
                let (ok, message) = match outcome {
                    Ok(_) => (true, "Installed.".to_string()),
                    Err(error) => (false, error.to_string()),
                };
                service.as_mut().install_finished(
                    QString::from(thread_driver_id.as_str()),
                    ok,
                    QString::from(message.as_str()),
                );
            });
        });
        FfiResult::default()
    }

    /// Clears a driver's quarantine marker so the next connect attempt
    /// tries loading it again (F8.5, `db_driver_adbc::quarantine::Quarantine::reset`).
    pub fn reenable(&self, driver_id: &QString) -> FfiResult {
        let driver_id = driver_id.to_string();
        let Some((_, contribution)) = find_contribution(&driver_id) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such database driver");
        };
        let manifest_name = contribution
            .adbc
            .as_ref()
            .and_then(|adbc| adbc.manifest_name.as_deref())
            .unwrap_or(contribution.id.as_str());
        let config_dir = app_core::resolve_config_dir();
        match db_driver_adbc::quarantine::Quarantine::new(config_dir).reset(manifest_name) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }
}

/// `https://host/path...` -> `host`, for the consent dialog's "publisher"
/// line — not a real publisher registry, just the download's own domain,
/// which is what the consent dialog actually needs to show (ADR-0061 §4).
fn url_host(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn adbc_row(id: &str) -> DatabaseDriverContribution {
        DatabaseDriverContribution {
            id: id.to_string(),
            name: id.to_string(),
            family: id.to_string(),
            backend: "adbc".to_string(),
            native_id: None,
            default_port: None,
            url_template: None,
            dump_tool: None,
            icon: None,
            adbc: Some(plugin_api::AdbcDriverSection {
                manifest_name: Some(id.to_string()),
                url: None,
                sha256: None,
                entrypoint: None,
                artifacts: BTreeMap::new(),
                install_hint: None,
            }),
        }
    }

    #[test]
    fn a_native_row_has_no_install_status() {
        let mut row = adbc_row("sqlite");
        row.backend = "native".to_string();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_for(&row, BUILTIN_PLUGIN_ID, false, dir.path()),
            DriverStatus::NotApplicable
        );
    }

    #[test]
    fn a_third_party_row_is_blocked_by_default() {
        let row = adbc_row("mystery");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_for(&row, "some-plugin", false, dir.path()),
            DriverStatus::ThirdPartyBlocked
        );
    }

    #[test]
    fn a_third_party_row_is_allowed_once_the_setting_is_on() {
        let row = adbc_row("mystery");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_for(&row, "some-plugin", true, dir.path()),
            DriverStatus::SystemSearch
        );
    }

    #[test]
    fn a_builtin_row_with_no_artifact_or_hint_falls_back_to_system_search() {
        let row = adbc_row("duckdb");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_for(&row, BUILTIN_PLUGIN_ID, false, dir.path()),
            DriverStatus::SystemSearch
        );
    }

    #[test]
    fn a_row_with_a_platform_artifact_is_installable() {
        let mut row = adbc_row("duckdb");
        row.adbc.as_mut().unwrap().artifacts.insert(
            current_platform().to_string(),
            plugin_api::AdbcArtifact {
                url: "https://example.com/libduckdb.zip".to_string(),
                sha256: "a".repeat(64),
                library: "libduckdb.so".to_string(),
            },
        );
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_for(&row, BUILTIN_PLUGIN_ID, false, dir.path()),
            DriverStatus::Installable
        );
    }

    #[test]
    fn a_row_with_only_an_install_hint_reports_it() {
        let mut row = adbc_row("trino");
        row.adbc.as_mut().unwrap().install_hint = Some("use ODBC instead".to_string());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_for(&row, BUILTIN_PLUGIN_ID, false, dir.path()),
            DriverStatus::InstallHint("use ODBC instead".to_string())
        );
    }

    #[test]
    fn an_installed_manifest_is_reported_installed() {
        let row = adbc_row("duckdb");
        let dir = tempfile::tempdir().unwrap();
        let managed =
            db_driver_adbc::locate::managed_dir(dir.path(), "duckdb", ADBC_INSTALLED_SLOT);
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(managed.join("manifest.toml"), "manifest_version = 1").unwrap();
        assert_eq!(
            status_for(&row, BUILTIN_PLUGIN_ID, false, dir.path()),
            DriverStatus::Installed
        );
    }

    #[test]
    fn a_quarantined_driver_is_reported_before_the_installed_check() {
        let row = adbc_row("duckdb");
        let dir = tempfile::tempdir().unwrap();
        db_driver_adbc::quarantine::Quarantine::new(dir.path())
            .arm("duckdb")
            .unwrap();
        assert_eq!(
            status_for(&row, BUILTIN_PLUGIN_ID, false, dir.path()),
            DriverStatus::Quarantined
        );
    }

    #[test]
    fn status_text_matches_the_f85_vocabulary() {
        assert_eq!(DriverStatus::Installed.status_text(), "installed");
        assert_eq!(
            DriverStatus::Quarantined.status_text(),
            "quarantined — Re-enable"
        );
        assert_eq!(
            DriverStatus::Installable.status_text(),
            "not installed — Install…"
        );
        assert_eq!(
            DriverStatus::InstallHint("do X".to_string()).status_text(),
            "install hint: do X"
        );
        assert_eq!(
            DriverStatus::SystemSearch.status_text(),
            "found via ADBC search"
        );
    }

    #[test]
    fn url_host_extracts_the_domain() {
        assert_eq!(
            url_host("https://github.com/duckdb/duckdb/releases/x.zip"),
            "github.com"
        );
        assert_eq!(
            url_host("https://files.pythonhosted.org/packages/x.whl"),
            "files.pythonhosted.org"
        );
    }
}
