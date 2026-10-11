//! Machine-local test results: `<project_root>/.ide/local/test-history.toml`
//! (PHPUnit editor runs plan, E8's G4).
//!
//! PhpStorm restores the editor gutter's test icons after a restart from
//! its test history; this file is that history: the last result of each
//! test, so a gutter shows it before anything has run in this session.
//! Under `.ide/local/` for the reason [`crate::breakpoint_settings`] gives:
//! it is this machine's last run, not something the project agrees on.
//!
//! The shape is dumb and stringly, like every other table in this crate:
//! `php-core::test_history` decides what goes in and what a row means.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{load_toml, project_settings, save_toml, ConfigError};

const LOCAL_DIR: &str = "local";
const HISTORY_FILE: &str = "test-history.toml";
/// Where an unreadable history is moved rather than overwritten.
const UNREADABLE_HISTORY_FILE: &str = "test-history.toml.bak";

/// One read-merge-write at a time in this process: two runs' saves never
/// interleave their reads and renames.
static WRITER: Mutex<()> = Mutex::new(());
/// Makes each save's temporary file name its own.
static SAVES: AtomicU64 = AtomicU64::new(0);

/// One test's last result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResultSetting {
    /// The Tests dock's node id (`Ns\ClassTest::testMethod`).
    #[serde(default)]
    pub id: String,
    /// The test file on this machine, absolute.
    #[serde(default)]
    pub file: String,
    /// What the test is in its file: `class`, `method`, `description`,
    /// `pest_class` or `pest_method`.
    #[serde(default)]
    pub symbol: String,
    /// The symbol's class, or a Pest description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub class: String,
    /// A method's name, or a Pest method's title.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// `passed`, `failed` or `skipped`.
    #[serde(default)]
    pub status: String,
    /// The failure message or skip reason, shortened.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub duration_ms: u64,
    /// A comparison failure's expected side, shortened.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub expected: String,
    /// A comparison failure's actual side, shortened.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actual: String,
    /// When the result was recorded, ms since the epoch: the newer row wins
    /// when two writers merge (0 for a row an earlier build wrote).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub recorded_at: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// This machine's last test results for one project, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestHistorySettings {
    #[serde(default, rename = "result", skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<TestResultSetting>,
}

fn local_dir(project_root: &Path) -> Result<std::path::PathBuf, ConfigError> {
    Ok(project_settings::project_dir(project_root)?.join(LOCAL_DIR))
}

/// Load `<project_root>/.ide/local/test-history.toml`. A missing file is an
/// empty history, not an error.
pub fn load(project_root: &Path) -> Result<TestHistorySettings, ConfigError> {
    load_toml(&local_dir(project_root)?.join(HISTORY_FILE))
}

/// Write the history `merge` makes of what the file holds now, creating
/// `.ide/local` if needed, and `.ide/.gitignore`, which keeps `local/` out
/// of version control; returns what was written.
///
/// The file is re-read under this process's writer lock, so a save merges
/// with what another save or another IDE instance wrote meanwhile instead of
/// replacing it. An unreadable file is moved aside to
/// `test-history.toml.bak`, never overwritten unseen, and `merge` starts
/// from an empty history.
// ponytail: two processes can still race between the read and the rename;
// a lock file would close that, the merge already keeps the loss to one save.
pub fn update(
    project_root: &Path,
    merge: impl FnOnce(TestHistorySettings) -> TestHistorySettings,
) -> Result<TestHistorySettings, ConfigError> {
    let _writer = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = local_dir(project_root)?;
    std::fs::create_dir_all(&dir)?;
    project_settings::ensure_gitignore(&project_settings::project_dir(project_root)?)?;
    let path = dir.join(HISTORY_FILE);
    let current = match load_toml(&path) {
        Ok(current) => current,
        Err(ConfigError::Parse(_)) => {
            std::fs::rename(&path, dir.join(UNREADABLE_HISTORY_FILE))?;
            TestHistorySettings::default()
        }
        Err(error) => return Err(error),
    };
    let history = merge(current);
    save_toml(&path, &temp_path(&dir), &history)?;
    Ok(history)
}

/// A temporary file name no other save (in this process or another) uses.
fn temp_path(dir: &Path) -> PathBuf {
    let save = SAVES.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{HISTORY_FILE}.{}.{save}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_with_no_file_has_no_history() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), TestHistorySettings::default());
    }

    #[test]
    fn results_round_trip_under_ide_local() {
        let dir = tempfile::tempdir().unwrap();
        let history = TestHistorySettings {
            results: vec![TestResultSetting {
                id: "App\\GreeterTest::testGreets".into(),
                file: "/p/tests/GreeterTest.php".into(),
                symbol: "method".into(),
                class: "App\\GreeterTest".into(),
                name: "testGreets".into(),
                status: "failed".into(),
                message: "Failed asserting that false is true.".into(),
                duration_ms: 12,
                expected: "true".into(),
                actual: "false".into(),
                recorded_at: 1_700_000_000_000,
            }],
        };
        update(dir.path(), |_| history.clone()).unwrap();
        assert!(dir.path().join(".ide/local/test-history.toml").is_file());
        let gitignore = std::fs::read_to_string(dir.path().join(".ide/.gitignore")).unwrap();
        assert!(gitignore.contains("local/"), "{gitignore}");
        assert_eq!(load(dir.path()).unwrap(), history);
    }

    #[test]
    fn a_save_merges_with_what_the_file_holds_now() {
        let dir = tempfile::tempdir().unwrap();
        let row = |id: &str| TestResultSetting {
            id: id.into(),
            ..TestResultSetting::default()
        };
        update(dir.path(), |_| TestHistorySettings {
            results: vec![row("a")],
        })
        .unwrap();
        let written = update(dir.path(), |current| {
            assert_eq!(current.results, [row("a")]);
            let mut results = current.results;
            results.push(row("b"));
            TestHistorySettings { results }
        })
        .unwrap();
        assert_eq!(load(dir.path()).unwrap(), written);
        let local = dir.path().join(".ide/local");
        let leftovers: Vec<_> = std::fs::read_dir(&local)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn an_unreadable_file_is_moved_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(".ide/local");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join(HISTORY_FILE), "not = [toml").unwrap();
        assert!(matches!(load(dir.path()), Err(ConfigError::Parse(_))));
        let written = update(dir.path(), |current| {
            assert!(current.results.is_empty());
            current
        })
        .unwrap();
        assert_eq!(written, TestHistorySettings::default());
        assert_eq!(
            std::fs::read_to_string(local.join(UNREADABLE_HISTORY_FILE)).unwrap(),
            "not = [toml"
        );
        assert_eq!(load(dir.path()).unwrap(), TestHistorySettings::default());
    }

    #[test]
    fn concurrent_saves_keep_every_writer_s_rows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let writers: Vec<_> = (0..8)
            .map(|n| {
                let root = root.clone();
                std::thread::spawn(move || {
                    update(&root, |mut current| {
                        current.results.push(TestResultSetting {
                            id: format!("t{n}"),
                            ..TestResultSetting::default()
                        });
                        current
                    })
                    .unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(load(&root).unwrap().results.len(), 8);
    }
}
