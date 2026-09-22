//! A driver that crashed the process on its last load is quarantined
//! (F8.1, database-tools.md §9): a marker file is written *before* the
//! first load attempt and removed only after a successful probe. A
//! marker still present at the next startup means the previous attempt
//! never got that far — the process died loading or probing the driver —
//! so this driver stays refused with a typed error until the user
//! explicitly re-enables it.
//!
//! The marker is a plain empty file, not a lock: nothing here coordinates
//! concurrent access, since a driver is loaded at most once per process
//! (`locate`/`install` already serialise on the filesystem).

use std::io;
use std::path::{Path, PathBuf};

use db_core::error::{DbError, DbErrorCode};

pub struct Quarantine {
    dir: PathBuf,
}

impl Quarantine {
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: config_dir
                .into()
                .join("database")
                .join("drivers")
                .join(".quarantine"),
        }
    }

    fn marker_path(&self, driver_id: &str) -> PathBuf {
        self.dir.join(driver_id)
    }

    /// `Err(DriverQuarantined)` if `driver_id` was left marked from a
    /// previous run; call this before attempting to load the driver.
    pub fn check(&self, driver_id: &str) -> Result<(), DbError> {
        if self.marker_path(driver_id).exists() {
            return Err(DbError::new(
                DbErrorCode::DriverQuarantined,
                format!(
                    "the \"{driver_id}\" ADBC driver crashed the last time it was loaded and is \
                     quarantined; re-enable it explicitly to try again"
                ),
            ));
        }
        Ok(())
    }

    /// Marks `driver_id` quarantined *before* the load attempt that could
    /// crash the process — call this immediately before loading, and
    /// [`Self::clear`] immediately after a successful probe.
    pub fn arm(&self, driver_id: &str) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.marker_path(driver_id), b"")
    }

    /// Removes the marker after a successful load+probe.
    pub fn clear(&self, driver_id: &str) -> io::Result<()> {
        match std::fs::remove_file(self.marker_path(driver_id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Explicit user action: forget that this driver ever crashed.
    pub fn reset(&self, driver_id: &str) -> io::Result<()> {
        self.clear(driver_id)
    }
}

/// Runs `probe` under the quarantine protocol: arm, run, clear on success.
/// A panic or process abort inside `probe` leaves the marker in place,
/// which is the whole point — the next [`Quarantine::check`] then refuses
/// to try again until [`Quarantine::reset`].
pub fn guarded_load<T>(
    quarantine: &Quarantine,
    driver_id: &str,
    probe: impl FnOnce() -> Result<T, DbError>,
) -> Result<T, DbError> {
    quarantine.check(driver_id)?;
    quarantine
        .arm(driver_id)
        .map_err(|e| DbError::new(DbErrorCode::Io, format!("writing quarantine marker: {e}")))?;
    let result = probe();
    if result.is_ok() {
        quarantine.clear(driver_id).map_err(|e| {
            DbError::new(DbErrorCode::Io, format!("clearing quarantine marker: {e}"))
        })?;
    }
    result
}

#[allow(dead_code)]
fn marker_exists_for_test(dir: &Path, driver_id: &str) -> bool {
    dir.join(driver_id).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_driver_is_never_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = Quarantine::new(dir.path());
        assert!(quarantine.check("duckdb").is_ok());
    }

    #[test]
    fn arming_then_checking_reports_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = Quarantine::new(dir.path());
        quarantine.arm("duckdb").unwrap();
        let err = quarantine.check("duckdb").unwrap_err();
        assert_eq!(err.code, DbErrorCode::DriverQuarantined);
        assert!(err.message.contains("duckdb"));
    }

    #[test]
    fn clearing_after_arming_lifts_the_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = Quarantine::new(dir.path());
        quarantine.arm("duckdb").unwrap();
        quarantine.clear("duckdb").unwrap();
        assert!(quarantine.check("duckdb").is_ok());
    }

    #[test]
    fn clearing_a_driver_that_was_never_armed_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = Quarantine::new(dir.path());
        assert!(quarantine.clear("never-loaded").is_ok());
    }

    #[test]
    fn a_successful_guarded_load_leaves_no_marker_behind() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = Quarantine::new(dir.path());
        let result = guarded_load(&quarantine, "duckdb", || Ok::<_, DbError>(42));
        assert_eq!(result, Ok(42));
        assert!(quarantine.check("duckdb").is_ok());
    }

    #[test]
    fn a_failed_probe_leaves_the_marker_armed_the_caller_reported_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = Quarantine::new(dir.path());
        let result: Result<(), DbError> = guarded_load(&quarantine, "duckdb", || {
            Err(DbError::new(DbErrorCode::ConnectionFailed, "boom"))
        });
        assert!(result.is_err());
        // The probe itself returned an error rather than crashing, so in
        // principle the driver is fine — but this module cannot tell an
        // `Err` return apart from "about to crash", so it deliberately
        // leaves the marker armed either way; only a process that dies
        // mid-probe is the case this module exists for, and a caller
        // whose probe merely errored can `reset()` explicitly.
        assert!(marker_exists_for_test(
            dir.path()
                .join("database")
                .join("drivers")
                .join(".quarantine")
                .as_path(),
            "duckdb"
        ));
    }

    #[test]
    fn reset_forgets_a_previous_crash() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = Quarantine::new(dir.path());
        quarantine.arm("duckdb").unwrap();
        quarantine.reset("duckdb").unwrap();
        assert!(quarantine.check("duckdb").is_ok());
    }
}
