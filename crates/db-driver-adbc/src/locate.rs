//! Resolves which shared library backs one ADBC driver id (F8.1): the
//! IDE-managed install directory first
//! (`<config_dir>/database/drivers/<id>/<version>/manifest.toml`, written
//! by [`crate::install`]), then falling back to letting the ADBC driver
//! manager search its own standard locations by name (an already-installed
//! system driver, or one on `PATH`/`ADBC_DRIVER_PATH`).

use std::path::{Path, PathBuf};

/// Where a driver's shared library was found — the two places
/// `database-tools.md` §8 names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverLocation {
    /// An IDE-managed install: a manifest TOML the driver manager can
    /// load directly (`ManagedDriver::load_dynamic_from_filename`).
    ManagedManifest(PathBuf),
    /// No IDE-managed install found; ask the driver manager to resolve
    /// `name` through its own standard search (`ManagedDriver::load_from_name`).
    SystemName(String),
}

fn manifest_path(config_dir: &Path, driver_id: &str, version: &str) -> PathBuf {
    config_dir
        .join("database")
        .join("drivers")
        .join(driver_id)
        .join(version)
        .join("manifest.toml")
}

/// The IDE-managed directory for one driver id/version — where
/// [`crate::install::install`] unpacks the library and writes the
/// manifest, and where [`locate`] looks first.
pub fn managed_dir(config_dir: &Path, driver_id: &str, version: &str) -> PathBuf {
    config_dir
        .join("database")
        .join("drivers")
        .join(driver_id)
        .join(version)
}

/// Resolves `driver_id`/`version`: an IDE-managed manifest if one was
/// installed, otherwise the ADBC standard by-name search.
pub fn locate(config_dir: &Path, driver_id: &str, version: &str) -> DriverLocation {
    let path = manifest_path(config_dir, driver_id, version);
    if path.is_file() {
        DriverLocation::ManagedManifest(path)
    } else {
        DriverLocation::SystemName(driver_id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_system_search_when_nothing_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let location = locate(dir.path(), "duckdb", "1.0.0");
        assert_eq!(location, DriverLocation::SystemName("duckdb".to_string()));
    }

    #[test]
    fn prefers_the_ide_managed_manifest_once_one_exists() {
        let dir = tempfile::tempdir().unwrap();
        let managed = managed_dir(dir.path(), "duckdb", "1.0.0");
        std::fs::create_dir_all(&managed).unwrap();
        let manifest = managed.join("manifest.toml");
        std::fs::write(&manifest, "manifest_version = 1").unwrap();

        let location = locate(dir.path(), "duckdb", "1.0.0");
        assert_eq!(location, DriverLocation::ManagedManifest(manifest));
    }

    #[test]
    fn a_different_version_of_the_same_driver_is_not_confused_with_an_installed_one() {
        let dir = tempfile::tempdir().unwrap();
        let managed = managed_dir(dir.path(), "duckdb", "1.0.0");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(managed.join("manifest.toml"), "manifest_version = 1").unwrap();

        let location = locate(dir.path(), "duckdb", "2.0.0");
        assert_eq!(location, DriverLocation::SystemName("duckdb".to_string()));
    }
}
