//! Installs one ADBC driver's shared library into the IDE-managed
//! directory (F8.1): download → verify sha256 *before* unpacking →
//! extract the one named library entry → write a manifest
//! [`crate::locate`] can find → atomic rename into place.
//!
//! The network call ([`install`]) is a thin wrapper around
//! [`install_from_bytes`], which is where every actual rule lives and
//! which every test below exercises directly — no network in tests, per
//! the plan.

use std::io::Read;
use std::path::{Path, PathBuf};

use db_core::error::{DbError, DbErrorCode};
use sha2::{Digest, Sha256};

use crate::locate::managed_dir;

/// What a `database-drivers` plugin contribution's `adbc.artifacts` row
/// (`plugin_api::manifest::AdbcArtifact`) records for one platform's
/// artifact: where to download it, its expected hash, and the one file
/// inside the archive that is the actual shared library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverInstall {
    pub url: String,
    pub sha256: String,
    pub platform: String,
    /// The path of the shared library *inside* the downloaded archive
    /// (e.g. `"libduckdb.so"`).
    pub library: String,
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Extracts `library`'s bytes out of `archive`, trying zip first (`.zip`
/// is unambiguous by its magic bytes) then falling back to gzip'd tar
/// (DuckDB's `.tar.gz` releases) — whichever the bytes actually are.
fn unpack_one(archive: &[u8], library: &str) -> Result<Vec<u8>, DbError> {
    if let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(archive)) {
        let mut entry = zip.by_name(library).map_err(|e| {
            DbError::new(
                DbErrorCode::Io,
                format!("archive has no entry named \"{library}\": {e}"),
            )
        })?;
        let mut out = Vec::new();
        entry.read_to_end(&mut out).map_err(|e| {
            DbError::new(
                DbErrorCode::Io,
                format!("reading \"{library}\" from zip: {e}"),
            )
        })?;
        return Ok(out);
    }

    let gunzipped = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gunzipped);
    let entries = tar.entries().map_err(|e| {
        DbError::new(
            DbErrorCode::Io,
            format!("not a valid zip or tar.gz archive: {e}"),
        )
    })?;
    for entry in entries {
        let mut entry =
            entry.map_err(|e| DbError::new(DbErrorCode::Io, format!("reading tar entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| DbError::new(DbErrorCode::Io, format!("reading tar entry path: {e}")))?
            .to_string_lossy()
            .into_owned();
        if path == library || path.ends_with(&format!("/{library}")) {
            let mut out = Vec::new();
            entry.read_to_end(&mut out).map_err(|e| {
                DbError::new(
                    DbErrorCode::Io,
                    format!("reading \"{library}\" from tar: {e}"),
                )
            })?;
            return Ok(out);
        }
    }
    Err(DbError::new(
        DbErrorCode::Io,
        format!("archive has no entry named \"{library}\""),
    ))
}

fn manifest_toml(driver_id: &str, version: &str, library_path: &Path) -> String {
    // `[Driver].shared` as a plain string (this OS/arch's own library,
    // already selected by `DriverInstall::platform` before we ever
    // downloaded it) — the table form (per-platform keys) is for a
    // manifest meant to travel across machines, which an IDE-managed,
    // already-platform-specific install never is.
    format!(
        "manifest_version = 1\nname = \"{driver_id}\"\nversion = \"{version}\"\n\n[ADBC]\nversion = \"1.1.0\"\n\n[Driver]\nshared = {library_path:?}\n",
    )
}

/// Verifies, unpacks and installs `archive` (already downloaded) into
/// `<config_dir>/database/drivers/<driver_id>/<version>/`. Every rule
/// this module has lives here; [`install`] only adds the download.
pub fn install_from_bytes(
    config_dir: &Path,
    driver_id: &str,
    version: &str,
    spec: &DriverInstall,
    archive: &[u8],
) -> Result<PathBuf, DbError> {
    let actual = hex_sha256(archive);
    if !actual.eq_ignore_ascii_case(&spec.sha256) {
        return Err(DbError::new(
            DbErrorCode::DriverChecksum,
            format!(
                "downloaded archive for \"{driver_id}\" does not match its published sha256 \
                 (expected {}, got {actual}) — refusing to install it",
                spec.sha256
            ),
        ));
    }

    let library_bytes = unpack_one(archive, &spec.library)?;

    let final_dir = managed_dir(config_dir, driver_id, version);
    let staging_dir = final_dir.with_extension("tmp-install");
    let _ = std::fs::remove_dir_all(&staging_dir);
    std::fs::create_dir_all(&staging_dir)
        .map_err(|e| DbError::new(DbErrorCode::Io, format!("creating install dir: {e}")))?;

    let library_name = Path::new(&spec.library)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| spec.library.clone());
    let library_path = staging_dir.join(&library_name);
    std::fs::write(&library_path, &library_bytes)
        .map_err(|e| DbError::new(DbErrorCode::Io, format!("writing library file: {e}")))?;

    let manifest = manifest_toml(driver_id, version, &library_path);
    let manifest_path = staging_dir.join("manifest.toml");
    std::fs::write(&manifest_path, manifest)
        .map_err(|e| DbError::new(DbErrorCode::Io, format!("writing manifest: {e}")))?;

    let _ = std::fs::remove_dir_all(&final_dir);
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            DbError::new(DbErrorCode::Io, format!("creating driver parent dir: {e}"))
        })?;
    }
    std::fs::rename(&staging_dir, &final_dir)
        .map_err(|e| DbError::new(DbErrorCode::Io, format!("installing driver (rename): {e}")))?;

    Ok(final_dir.join("manifest.toml"))
}

/// Downloads `spec.url` and installs it — the one function in this
/// module that touches the network.
pub fn install(
    config_dir: &Path,
    driver_id: &str,
    version: &str,
    spec: &DriverInstall,
) -> Result<PathBuf, DbError> {
    let response = reqwest::blocking::get(&spec.url)
        .map_err(|e| DbError::new(DbErrorCode::Io, format!("downloading {}: {e}", spec.url)))?;
    let bytes = response
        .bytes()
        .map_err(|e| DbError::new(DbErrorCode::Io, format!("reading download body: {e}")))?;
    install_from_bytes(config_dir, driver_id, version, spec, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_spec(library: &str, sha256: &str) -> DriverInstall {
        DriverInstall {
            url: "https://example.invalid/driver.zip".to_string(),
            sha256: sha256.to_string(),
            platform: "linux_amd64".to_string(),
            library: library.to_string(),
        }
    }

    fn zip_with_one_file(name: &str, contents: &[u8]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(contents).unwrap();
            writer.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn installs_a_valid_zip_and_writes_a_loadable_manifest() {
        let library_bytes = b"not really a shared library, just test bytes";
        let archive = zip_with_one_file("libduckdb.so", library_bytes);
        let sha256 = hex_sha256(&archive);
        let spec = fixture_spec("libduckdb.so", &sha256);

        let config_dir = tempfile::tempdir().unwrap();
        let manifest_path =
            install_from_bytes(config_dir.path(), "duckdb", "1.0.0", &spec, &archive)
                .expect("install should succeed");

        assert!(manifest_path.ends_with("manifest.toml"));
        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(manifest.contains("manifest_version = 1"));
        assert!(manifest.contains("[Driver]"));
        assert!(manifest.contains("libduckdb.so"));

        let library_path = manifest_path.parent().unwrap().join("libduckdb.so");
        assert_eq!(std::fs::read(library_path).unwrap(), library_bytes);
    }

    #[test]
    fn a_wrong_hash_is_refused_before_any_unpacking() {
        let archive = zip_with_one_file("libduckdb.so", b"payload");
        let spec = fixture_spec("libduckdb.so", "0".repeat(64).as_str());

        let config_dir = tempfile::tempdir().unwrap();
        let err =
            install_from_bytes(config_dir.path(), "duckdb", "1.0.0", &spec, &archive).unwrap_err();
        assert_eq!(err.code, DbErrorCode::DriverChecksum);
        assert!(!managed_dir(config_dir.path(), "duckdb", "1.0.0").exists());
    }

    #[test]
    fn a_corrupted_archive_is_refused_with_an_io_error() {
        let archive = b"this is not a zip or a tar.gz file at all".to_vec();
        let sha256 = hex_sha256(&archive);
        let spec = fixture_spec("libduckdb.so", &sha256);

        let config_dir = tempfile::tempdir().unwrap();
        let err =
            install_from_bytes(config_dir.path(), "duckdb", "1.0.0", &spec, &archive).unwrap_err();
        assert_eq!(err.code, DbErrorCode::Io);
    }

    #[test]
    fn an_archive_missing_the_named_library_is_refused() {
        let archive = zip_with_one_file("some_other_file.txt", b"payload");
        let sha256 = hex_sha256(&archive);
        let spec = fixture_spec("libduckdb.so", &sha256);

        let config_dir = tempfile::tempdir().unwrap();
        let err =
            install_from_bytes(config_dir.path(), "duckdb", "1.0.0", &spec, &archive).unwrap_err();
        assert_eq!(err.code, DbErrorCode::Io);
    }

    #[test]
    fn reinstalling_replaces_the_previous_version_atomically() {
        let archive_v1 = zip_with_one_file("libduckdb.so", b"v1");
        let spec_v1 = fixture_spec("libduckdb.so", &hex_sha256(&archive_v1));
        let config_dir = tempfile::tempdir().unwrap();
        install_from_bytes(config_dir.path(), "duckdb", "1.0.0", &spec_v1, &archive_v1).unwrap();

        let archive_v2 = zip_with_one_file("libduckdb.so", b"v2 is longer than v1");
        let spec_v2 = fixture_spec("libduckdb.so", &hex_sha256(&archive_v2));
        let manifest_path =
            install_from_bytes(config_dir.path(), "duckdb", "1.0.0", &spec_v2, &archive_v2)
                .unwrap();

        let library_path = manifest_path.parent().unwrap().join("libduckdb.so");
        assert_eq!(
            std::fs::read(library_path).unwrap(),
            b"v2 is longer than v1"
        );
    }
}
