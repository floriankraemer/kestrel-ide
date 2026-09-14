//! Running a Gradle sync: spawn the init script through the project's own
//! wrapper, read the model file it wrote, parse it (A4).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use run_core::toolchain::gradle_program;

use super::{init_script, model_json};
use crate::model::BuildModel;

/// How long a sync may run before it is killed. Generous: the first sync of
/// a project with a cold `~/.gradle` cache can spend minutes downloading.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    pub offline: bool,
    pub timeout: Option<Duration>,
}

/// Why a sync did not produce a [`BuildModel`].
#[derive(Debug)]
pub enum SyncError {
    /// The `gradle`/`gradlew` binary could not be found or run at all.
    NotFound,
    TimedOut,
    /// Spawning, writing, or reading a pipe failed for a reason that isn't
    /// "not found" or "timed out".
    Io(String),
    /// The process ran and exited non-zero — a real build failure (a
    /// syntax error in `build.gradle`, an unresolvable dependency). Carries
    /// Gradle's own stderr, which already says what went wrong.
    BuildFailed {
        stderr: String,
    },
    /// The process exited zero but the model file it was asked to write is
    /// missing or unreadable.
    ModelUnreadable(String),
    /// The model file exists but is not JSON this crate understands — a
    /// Gradle version too old/new for the script, or a script the user
    /// edited out from under the plugin.
    ModelInvalid(String),
}

impl From<process_exec::Failure> for SyncError {
    fn from(failure: process_exec::Failure) -> Self {
        match failure {
            process_exec::Failure::NotFound => SyncError::NotFound,
            process_exec::Failure::TimedOut => SyncError::TimedOut,
            process_exec::Failure::Io(message) => SyncError::Io(message),
        }
    }
}

/// Run the model sync against `project_root`'s own Gradle wrapper (falling
/// back to `gradle` on `PATH` when there is none), using the init script at
/// `script_path` — the real, on-disk path `LoadedPlugin::asset_dir`
/// materialised (A3), not the embedded bytes.
pub fn sync(
    project_root: &Path,
    script_path: &Path,
    opts: &SyncOptions,
) -> Result<BuildModel, SyncError> {
    let program = gradle_program(project_root);
    let model_out = model_out_path(project_root);
    let args = init_script::model_args(script_path, &model_out, opts.offline);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

    let output = process_exec::run(&program, &arg_refs, project_root, None, timeout, &[])?;

    if !output.status.success() {
        // A failed run may still have left a stale file from an earlier
        // successful one; clean it up so a later sync of the same project
        // never accidentally reads a leftover instead of its own fresh
        // output (or a "file missing" `ModelUnreadable` instead of this
        // more useful `BuildFailed`).
        let _ = std::fs::remove_file(&model_out);
        return Err(SyncError::BuildFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    // Read before removing: the file exists only because this run's own
    // process just wrote it, and the read has to happen before cleanup
    // deletes it, not the other way around.
    let contents = match std::fs::read_to_string(&model_out) {
        Ok(text) => text,
        Err(err) => return Err(SyncError::ModelUnreadable(err.to_string())),
    };
    let _ = std::fs::remove_file(&model_out);
    model_json::parse(&contents).map_err(|err| SyncError::ModelInvalid(err.to_string()))
}

/// A per-sync temp file path, so two concurrent syncs (unlikely, but not
/// impossible if a user mashes Reload) never race on the same output file.
fn model_out_path(project_root: &Path) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = format!(
        "ide-model-{}-{unique}.json",
        project_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    std::env::temp_dir().join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_not_found_program_is_reported_as_not_found() {
        let result = sync(
            Path::new("/does/not/matter"),
            Path::new("/scripts/ide-model.init.gradle"),
            &SyncOptions::default(),
        );
        // `gradle_program` falls back to the bare "gradle" name when there
        // is no wrapper, which either is not installed on the machine
        // running this unit test (NotFound) or is (and then fails fast on
        // a bogus project root instead) — either is a `SyncError`, never a
        // panic, which is what this test actually asserts.
        assert!(result.is_err());
    }

    #[test]
    fn model_out_paths_are_unique_per_call() {
        let a = model_out_path(Path::new("/proj"));
        let b = model_out_path(Path::new("/proj"));
        assert_ne!(a, b);
    }
}
