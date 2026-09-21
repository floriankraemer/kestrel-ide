//! Backend selection at the seam (Database Tools plan F8b,
//! `database-tools.md` §7): maps one `database-drivers` plugin
//! contribution's `backend` to how the rest of this module builds the
//! `db_core::driver::Driver` that actually runs a connection — `native`
//! through `db_drivers::DriverRegistry`, `adbc` through
//! `db_driver_adbc::AdbcDriver` (locate by manifest-name; the quarantine
//! arm/check/clear cycle already lives inside `AdbcDriver` itself), `odbc`
//! through `db_driver_odbc::OdbcDriver`.
//!
//! [`backend_for`] is plain-data mapping only — no filesystem, no network
//! — so it is unit-testable with hand-built `DatabaseDriverContribution`
//! values, never a live plugin registry.

use plugin_api::DatabaseDriverContribution;

/// The IDE-managed ADBC install directory always uses this literal in
/// place of a real semver.
///
/// ponytail: single install slot per driver id, no side-by-side versions;
/// add real version tracking (reading the installed manifest's own
/// `version` key) if a driver ever needs two versions to coexist.
pub const ADBC_INSTALLED_SLOT: &str = "current";

/// Which `db_core::driver::Driver` implementation one `database-drivers`
/// row resolves to, and the data that implementation needs to construct
/// itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// `db_drivers::DriverRegistry`'s own id.
    Native { native_id: String },
    /// An ADBC driver: the manifest-name the driver manager searches by
    /// (an IDE-managed install first, then its own standard search), and
    /// an optional non-standard C entrypoint symbol.
    Adbc {
        manifest_name: String,
        entrypoint: Option<String>,
    },
    /// Any ODBC-configured driver/DSN — the data source's own fields (DSN
    /// name, driver name, or a raw connection string) carry every
    /// per-connection specific, so this row needs nothing else.
    Odbc,
}

/// A `database-drivers` row that cannot be resolved to a backend at all —
/// a malformed contribution slipping past `plugin-api` validation, which
/// should not happen but is reported rather than panicked on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError(pub String);

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Resolves one `database-drivers` contribution's `backend` field.
pub fn backend_for(contribution: &DatabaseDriverContribution) -> Result<Backend, BackendError> {
    match contribution.backend.as_str() {
        "native" => contribution
            .native_id
            .clone()
            .map(|native_id| Backend::Native { native_id })
            .ok_or_else(|| {
                BackendError(format!(
                    "database driver '{}' has backend=native but no native-id",
                    contribution.id
                ))
            }),
        "adbc" => {
            let adbc = contribution.adbc.as_ref().ok_or_else(|| {
                BackendError(format!(
                    "database driver '{}' has backend=adbc but no [adbc] section",
                    contribution.id
                ))
            })?;
            let manifest_name = adbc
                .manifest_name
                .clone()
                .unwrap_or_else(|| contribution.id.clone());
            Ok(Backend::Adbc {
                manifest_name,
                entrypoint: adbc.entrypoint.clone(),
            })
        }
        "odbc" => Ok(Backend::Odbc),
        other => Err(BackendError(format!(
            "database driver '{}' names an unknown backend '{other}'",
            contribution.id
        ))),
    }
}

/// Whether this row's status line should offer an install action at all —
/// only an `adbc` row with a pinned artifact for `platform` is
/// installable; a `native`/`odbc` row or an `adbc` row with only an
/// `install-hint` is not (F8.5).
pub fn adbc_artifact_for_platform<'a>(
    contribution: &'a DatabaseDriverContribution,
    platform: &str,
) -> Option<&'a plugin_api::AdbcArtifact> {
    contribution.adbc.as_ref()?.artifacts.get(platform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn contribution(backend: &str) -> DatabaseDriverContribution {
        DatabaseDriverContribution {
            id: "test-driver".to_string(),
            name: "Test Driver".to_string(),
            family: "test".to_string(),
            backend: backend.to_string(),
            native_id: None,
            default_port: None,
            url_template: None,
            dump_tool: None,
            icon: None,
            adbc: None,
        }
    }

    #[test]
    fn a_native_row_resolves_to_its_native_id() {
        let mut row = contribution("native");
        row.native_id = Some("postgresql".to_string());
        assert_eq!(
            backend_for(&row).unwrap(),
            Backend::Native {
                native_id: "postgresql".to_string()
            }
        );
    }

    #[test]
    fn a_native_row_without_a_native_id_is_rejected() {
        let row = contribution("native");
        assert!(backend_for(&row).is_err());
    }

    #[test]
    fn an_odbc_row_needs_no_extra_data() {
        let row = contribution("odbc");
        assert_eq!(backend_for(&row).unwrap(), Backend::Odbc);
    }

    #[test]
    fn an_adbc_row_without_an_adbc_section_is_rejected() {
        let row = contribution("adbc");
        assert!(backend_for(&row).is_err());
    }

    #[test]
    fn an_adbc_row_falls_back_to_its_id_when_manifest_name_is_absent() {
        let mut row = contribution("adbc");
        row.adbc = Some(plugin_api::AdbcDriverSection {
            manifest_name: None,
            url: Some("https://example.com/driver.zip".to_string()),
            sha256: Some("a".repeat(64)),
            entrypoint: None,
            artifacts: BTreeMap::new(),
            install_hint: None,
        });
        let backend = backend_for(&row).unwrap();
        assert_eq!(
            backend,
            Backend::Adbc {
                manifest_name: "test-driver".to_string(),
                entrypoint: None,
            }
        );
    }

    #[test]
    fn an_adbc_row_carries_its_entrypoint_override_through() {
        let mut row = contribution("adbc");
        row.adbc = Some(plugin_api::AdbcDriverSection {
            manifest_name: Some("duckdb".to_string()),
            url: None,
            sha256: None,
            entrypoint: Some("duckdb_adbc_init".to_string()),
            artifacts: BTreeMap::new(),
            install_hint: None,
        });
        let backend = backend_for(&row).unwrap();
        assert_eq!(
            backend,
            Backend::Adbc {
                manifest_name: "duckdb".to_string(),
                entrypoint: Some("duckdb_adbc_init".to_string()),
            }
        );
    }

    #[test]
    fn an_unknown_backend_is_rejected() {
        let row = contribution("telepathic");
        assert!(backend_for(&row).is_err());
    }

    #[test]
    fn adbc_artifact_for_platform_finds_the_matching_row() {
        let mut row = contribution("adbc");
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            "linux_amd64".to_string(),
            plugin_api::AdbcArtifact {
                url: "https://example.com/libduckdb.zip".to_string(),
                sha256: "a".repeat(64),
                library: "libduckdb.so".to_string(),
            },
        );
        row.adbc = Some(plugin_api::AdbcDriverSection {
            manifest_name: Some("duckdb".to_string()),
            url: None,
            sha256: None,
            entrypoint: None,
            artifacts,
            install_hint: None,
        });
        assert!(adbc_artifact_for_platform(&row, "linux_amd64").is_some());
        assert!(adbc_artifact_for_platform(&row, "windows_amd64").is_none());
    }

    #[test]
    fn adbc_artifact_for_platform_is_none_for_a_native_row() {
        let mut row = contribution("native");
        row.native_id = Some("postgresql".to_string());
        assert!(adbc_artifact_for_platform(&row, "linux_amd64").is_none());
    }
}
