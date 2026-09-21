//! The F8.3/F8.4 spike's findings as data: one row per ADBC driver this
//! phase looked at, with a pinned `url`+`sha256` artifact per platform
//! where the spike found one, or an `install_hint` where it didn't.
//!
//! This is **data**, not yet a plugin manifest — F8b moves the pinnable
//! rows into `builtin/database-tools/plugin.toml`'s driver entries. No
//! binaries are committed; every `sha256` here was computed by
//! downloading the artifact once during the spike (`database-tools.md`
//! §11, `database-tools-plan.md` F8.3).

use std::collections::BTreeMap;

use serde::Deserialize;

/// One platform's pinned artifact.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    /// The shared library's exact path inside the downloaded archive —
    /// what [`crate::install::DriverInstall::library`] needs.
    pub library: String,
}

/// One ADBC driver the catalogue knows about.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DriverEntry {
    pub id: String,
    pub name: String,
    pub family: String,
    #[serde(rename = "manifest-name")]
    pub manifest_name: String,
    pub entrypoint: String,
    /// Keyed by platform (`"linux_amd64"`, `"windows_amd64"`), only for
    /// drivers the spike found a stable `https://` URL + published
    /// checksum for.
    #[serde(default)]
    pub artifacts: BTreeMap<String, Artifact>,
    /// Set instead of `artifacts` when no pinnable artifact exists —
    /// what to tell the user (F8.5 shows this text; F8.6 was the image,
    /// not this).
    #[serde(rename = "install-hint", default)]
    pub install_hint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawCatalogue {
    #[serde(rename = "driver", default)]
    drivers: Vec<DriverEntry>,
}

/// The catalogue this repository ships, embedded at compile time so a
/// corrupted or missing on-disk copy can never silently produce an empty
/// driver list.
const BUNDLED_CATALOGUE: &str = include_str!("../catalogue.toml");

/// Parses a catalogue TOML document (used for both the bundled catalogue
/// and, in tests, ad-hoc fixtures).
pub fn parse(toml_text: &str) -> Result<Vec<DriverEntry>, toml::de::Error> {
    let raw: RawCatalogue = toml::from_str(toml_text)?;
    Ok(raw.drivers)
}

/// The catalogue this build ships.
pub fn bundled() -> Vec<DriverEntry> {
    parse(BUNDLED_CATALOGUE).expect("db-driver-adbc's own catalogue.toml must parse")
}

impl DriverEntry {
    /// The artifact for `platform`, if the spike found a pinnable one.
    pub fn artifact_for(&self, platform: &str) -> Option<&Artifact> {
        self.artifacts.get(platform)
    }

    /// This row's `platform` artifact, as [`crate::install::install`] needs it.
    pub fn install_spec(&self, platform: &str) -> Option<crate::install::DriverInstall> {
        let artifact = self.artifact_for(platform)?;
        Some(crate::install::DriverInstall {
            url: artifact.url.clone(),
            sha256: artifact.sha256.clone(),
            platform: platform.to_string(),
            library: artifact.library.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_catalogue_parses_and_is_not_empty() {
        let drivers = bundled();
        assert!(!drivers.is_empty());
    }

    #[test]
    fn every_bundled_row_has_either_an_artifact_or_an_install_hint() {
        for driver in bundled() {
            assert!(
                !driver.artifacts.is_empty() || driver.install_hint.is_some(),
                "{} has neither a pinned artifact nor an install hint",
                driver.id
            );
        }
    }

    #[test]
    fn a_row_with_a_pinned_artifact_is_reachable_by_platform() {
        let toml = r#"
            [[driver]]
            id = "duckdb"
            name = "DuckDB"
            family = "duckdb"
            manifest-name = "duckdb"
            entrypoint = "duckdb_adbc_init"

            [driver.artifacts.linux_amd64]
            url = "https://example.invalid/libduckdb.so"
            sha256 = "abc123"
            library = "libduckdb.so"
        "#;
        let drivers = parse(toml).unwrap();
        assert_eq!(drivers.len(), 1);
        let artifact = drivers[0].artifact_for("linux_amd64").unwrap();
        assert_eq!(artifact.url, "https://example.invalid/libduckdb.so");
        assert_eq!(drivers[0].artifact_for("windows_amd64"), None);
    }

    #[test]
    fn a_row_with_only_an_install_hint_has_no_artifacts() {
        let toml = r#"
            [[driver]]
            id = "bigquery"
            name = "Google BigQuery"
            family = "bigquery"
            manifest-name = "bigquery"
            entrypoint = "BigQueryDriverInit"
            install-hint = "No pinnable shared-library release found; build from source or use a vendor package."
        "#;
        let drivers = parse(toml).unwrap();
        assert!(drivers[0].artifacts.is_empty());
        assert!(drivers[0].install_hint.is_some());
    }
}
