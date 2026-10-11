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

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{load_toml, project_settings, save_toml, ConfigError};

const LOCAL_DIR: &str = "local";
const HISTORY_FILE: &str = "test-history.toml";
const TEMP_HISTORY_FILE: &str = "test-history.toml.tmp";

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

/// Replace the file with `history`, creating `.ide/local` if needed, and
/// `.ide/.gitignore`, which keeps `local/` out of version control.
pub fn save(project_root: &Path, history: &TestHistorySettings) -> Result<(), ConfigError> {
    let dir = local_dir(project_root)?;
    std::fs::create_dir_all(&dir)?;
    project_settings::ensure_gitignore(&project_settings::project_dir(project_root)?)?;
    save_toml(
        &dir.join(HISTORY_FILE),
        &dir.join(TEMP_HISTORY_FILE),
        history,
    )
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
            }],
        };
        save(dir.path(), &history).unwrap();
        assert!(dir.path().join(".ide/local/test-history.toml").is_file());
        let gitignore = std::fs::read_to_string(dir.path().join(".ide/.gitignore")).unwrap();
        assert!(gitignore.contains("local/"), "{gitignore}");
        assert_eq!(load(dir.path()).unwrap(), history);
    }
}
