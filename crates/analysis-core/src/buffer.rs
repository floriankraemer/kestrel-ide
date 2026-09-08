//! B6 — how an analyzer sees a file that has unsaved changes.
//!
//! Three strategies, named by an analyzer's manifest (`C2`'s job to
//! populate for PHPStan/PHPCS; this module is the mechanism, not the
//! per-tool wiring):
//!
//! - [`BufferStrategy::Stdin`] — pipe the buffer's bytes to the tool's
//!   stdin (PHPCS supports this natively).
//! - [`BufferStrategy::TempCopy`] — write the buffer to a dotfile beside
//!   the original, analyze that, delete it afterwards.
//! - [`BufferStrategy::SavedOnly`] — the tool can only ever see what is on
//!   disk. [`effective_trigger`] is what turns that into a visible
//!   `OnType` → `OnSave` downgrade rather than a silent no-op.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::def::Trigger;

/// How an analyzer is willing to see a buffer that has not been saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferStrategy {
    Stdin,
    TempCopy,
    SavedOnly,
}

/// `OnType` needs the tool to read something other than the last save;
/// `SavedOnly` cannot offer that, so the effective trigger silently
/// becomes `OnSave` — silently to the process, not to the user: the
/// settings page's job (B9) is to show [`degradation_reason`] wherever
/// this substitution applies, so a `SavedOnly` analyzer that only ever
/// fires on save is a stated property, not a bug report waiting to
/// happen.
pub fn effective_trigger(requested: Trigger, strategy: BufferStrategy) -> Trigger {
    if requested == Trigger::OnType && strategy == BufferStrategy::SavedOnly {
        Trigger::OnSave
    } else {
        requested
    }
}

/// The sentence a settings page shows next to an analyzer whose trigger
/// was downgraded, or `None` when [`effective_trigger`] made no change.
pub fn degradation_reason(requested: Trigger, strategy: BufferStrategy) -> Option<&'static str> {
    (effective_trigger(requested, strategy) != requested)
        .then_some("cannot read an unsaved buffer, so On Type is downgraded to On Save")
}

/// The gitignore pattern [`write_temp_copy`]'s files match, so exactly one
/// pattern needs adding to the project's `.gitignore` regardless of how
/// many analyzers or runs use this strategy.
pub const TEMP_COPY_GITIGNORE_PATTERN: &str = ".*.ide-analysis-tmp-*";

/// A [`write_temp_copy`] file, deleted when this guard is dropped — on
/// ordinary completion and on a scheduler-level cancellation alike, since
/// both simply stop holding the guard.
pub struct TempCopyGuard {
    path: PathBuf,
}

impl TempCopyGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempCopyGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to a dotfile beside `original`, named uniquely per
/// call (a monotonic in-process counter plus the wall clock, so two runs
/// started in the same process never collide, and two started from
/// different processes essentially never do). Seeds the project's root
/// `.gitignore` with [`TEMP_COPY_GITIGNORE_PATTERN`] so the copy is never
/// picked up by the tool's own file walker or accidentally committed
/// (Risk 4 of the PHP tooling plan) — via
/// `app_config::project_settings::ensure_root_gitignore_pattern`, the same
/// seed-a-gitignore-line mechanism `.ide/.gitignore` itself is seeded by.
pub fn write_temp_copy(
    project_root: &Path,
    original: &Path,
    contents: &[u8],
) -> io::Result<TempCopyGuard> {
    let file_name = original
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "original has no file name"))?;
    let suffix = format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
        RUN_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let dir = original.parent().unwrap_or_else(|| Path::new("."));
    let path = dir.join(format!(
        ".{}.ide-analysis-tmp-{suffix}",
        file_name.to_string_lossy()
    ));
    fs::write(&path, contents)?;

    if let Err(e) = app_config::project_settings::ensure_root_gitignore_pattern(
        project_root,
        TEMP_COPY_GITIGNORE_PATTERN,
    ) {
        // A failed gitignore seed must not fail the analysis run itself —
        // it is a housekeeping nicety, and the temp file is still deleted
        // by `TempCopyGuard` regardless of whether it was ever ignored.
        // Logged rather than silently dropped so the failure is at least
        // observable somewhere other than the working tree going dirty.
        eprintln!("analysis-core: could not seed .gitignore for temp copies: {e}");
    }

    Ok(TempCopyGuard { path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_type_is_unaffected_for_stdin_and_temp_copy() {
        assert_eq!(
            effective_trigger(Trigger::OnType, BufferStrategy::Stdin),
            Trigger::OnType
        );
        assert_eq!(
            effective_trigger(Trigger::OnType, BufferStrategy::TempCopy),
            Trigger::OnType
        );
    }

    #[test]
    fn on_type_degrades_to_on_save_for_saved_only() {
        assert_eq!(
            effective_trigger(Trigger::OnType, BufferStrategy::SavedOnly),
            Trigger::OnSave
        );
        assert!(degradation_reason(Trigger::OnType, BufferStrategy::SavedOnly).is_some());
    }

    #[test]
    fn on_save_and_manual_are_never_degraded() {
        for strategy in [
            BufferStrategy::Stdin,
            BufferStrategy::TempCopy,
            BufferStrategy::SavedOnly,
        ] {
            assert_eq!(
                effective_trigger(Trigger::OnSave, strategy),
                Trigger::OnSave
            );
            assert_eq!(
                effective_trigger(Trigger::Manual, strategy),
                Trigger::Manual
            );
            assert!(degradation_reason(Trigger::OnSave, strategy).is_none());
        }
    }

    #[test]
    fn a_temp_copy_is_written_beside_the_original_and_removed_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("Greeter.php");
        fs::write(&original, "<?php\n").unwrap();

        let path = {
            let guard = write_temp_copy(root.path(), &original, b"<?php\n// edited\n").unwrap();
            let path = guard.path().to_path_buf();
            assert!(path.exists());
            assert_eq!(path.parent().unwrap(), root.path());
            assert!(path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".Greeter.php.ide-analysis-tmp-"));
            path
        };
        assert!(
            !path.exists(),
            "the guard must delete the temp file on drop"
        );
    }

    #[test]
    fn two_runs_get_different_temp_files() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("Greeter.php");
        fs::write(&original, "<?php\n").unwrap();

        let a = write_temp_copy(root.path(), &original, b"a").unwrap();
        let b = write_temp_copy(root.path(), &original, b"b").unwrap();
        assert_ne!(a.path(), b.path());
    }

    #[test]
    fn writing_a_temp_copy_seeds_the_root_gitignore() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("Greeter.php");
        fs::write(&original, "<?php\n").unwrap();

        let _guard = write_temp_copy(root.path(), &original, b"x").unwrap();
        let gitignore = fs::read_to_string(root.path().join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|l| l == TEMP_COPY_GITIGNORE_PATTERN));
    }
}
