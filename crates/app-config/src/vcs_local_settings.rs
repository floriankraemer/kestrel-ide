//! Machine-local VCS state: `<project_root>/.ide/local/vcs.toml`.
//!
//! Distinct from [`crate::project_settings`]: that file is meant to be
//! committed and shared with everyone working on the project; this one holds
//! a preference specific to this checkout on this machine (whether this
//! person already said "not now" to initializing a Git repository here), so
//! it lives under `.ide/local/`, which `project_settings::ensure_gitignore`
//! already seeds as ignored.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{load_toml, project_settings, update_toml, ConfigError};

const LOCAL_DIR: &str = "local";
const VCS_LOCAL_SETTINGS_FILE: &str = "vcs.toml";
const TEMP_VCS_LOCAL_SETTINGS_FILE: &str = "vcs.toml.tmp";

/// How many past commit messages [`VcsLocalSettings::commit_message_history`]
/// keeps — R6's "last 25", per project.
const COMMIT_MESSAGE_HISTORY_LIMIT: usize = 25;

/// This machine's own VCS preferences for one project. `None` means never
/// set, distinct from `Some(false)` (asked, and not declined).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VcsLocalSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declined_git_init: Option<bool>,
    /// The commit messages this person has actually committed with in this
    /// project, newest first, capped at
    /// [`COMMIT_MESSAGE_HISTORY_LIMIT`] — the Changes dock's message combo.
    /// Machine-local like the rest of this file: a colleague's commit
    /// message wording is not something cloning this project should hand
    /// you.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commit_message_history: Vec<String>,
    /// Repository-relative paths of every file this person last had the
    /// gutter's "Annotate with Blame" toggle on for (R7) — a `HashSet`
    /// would serialize as an unordered TOML array that reshuffles on every
    /// save for no reason; a sorted `Vec` diffs cleanly and a lookup over
    /// the handful of files anyone leaves blame on is not worth a second
    /// data structure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blame_enabled_paths: Vec<String>,
}

impl VcsLocalSettings {
    /// Whether `path` (repository-relative) last had blame annotation on.
    pub fn blame_enabled(&self, path: &str) -> bool {
        self.blame_enabled_paths.iter().any(|p| p == path)
    }

    /// Record this machine's blame-toggle choice for `path`
    /// (repository-relative), keeping the list sorted and deduplicated.
    pub fn set_blame_enabled(&mut self, path: &str, enabled: bool) {
        let already = self.blame_enabled(path);
        if enabled && !already {
            self.blame_enabled_paths.push(path.to_string());
            self.blame_enabled_paths.sort();
        } else if !enabled && already {
            self.blame_enabled_paths.retain(|p| p != path);
        }
    }
    /// Record a just-made commit's message at the front of the history,
    /// removing any earlier occurrence of the exact same message first so
    /// re-using a recent message moves it to the top rather than
    /// duplicating it, then truncating to
    /// [`COMMIT_MESSAGE_HISTORY_LIMIT`].
    pub fn push_commit_message(&mut self, message: String) {
        self.commit_message_history.retain(|m| m != &message);
        self.commit_message_history.insert(0, message);
        self.commit_message_history
            .truncate(COMMIT_MESSAGE_HISTORY_LIMIT);
    }
}

fn local_dir(project_root: &Path) -> Result<std::path::PathBuf, ConfigError> {
    Ok(project_settings::project_dir(project_root)?.join(LOCAL_DIR))
}

/// Load `<project_root>/.ide/local/vcs.toml`. A missing file (or project
/// with no `.ide` at all) means no local preference has been recorded yet —
/// every field defaults to `None`, not an error.
pub fn load(project_root: &Path) -> Result<VcsLocalSettings, ConfigError> {
    let dir = local_dir(project_root)?;
    load_toml(&dir.join(VCS_LOCAL_SETTINGS_FILE))
}

/// Load, edit, save `<project_root>/.ide/local/vcs.toml`, creating
/// `.ide/local` if needed.
pub fn update(
    project_root: &Path,
    edit: impl FnOnce(&mut VcsLocalSettings),
) -> Result<(), ConfigError> {
    let dir = local_dir(project_root)?;
    fs::create_dir_all(&dir)?;
    update_toml(
        &dir.join(VCS_LOCAL_SETTINGS_FILE),
        &dir.join(TEMP_VCS_LOCAL_SETTINGS_FILE),
        edit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_file_defaults_to_not_declined() {
        let root = tempfile::tempdir().unwrap();
        let settings = load(root.path()).unwrap();
        assert_eq!(settings.declined_git_init, None);
    }

    #[test]
    fn declined_git_init_round_trips_through_update_and_load() {
        let root = tempfile::tempdir().unwrap();
        update(root.path(), |s| {
            s.declined_git_init = Some(true);
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        assert_eq!(loaded.declined_git_init, Some(true));
    }

    #[test]
    fn commit_message_history_round_trips_and_stays_newest_first() {
        let root = tempfile::tempdir().unwrap();
        update(root.path(), |s| {
            s.push_commit_message("first".to_string());
            s.push_commit_message("second".to_string());
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        assert_eq!(loaded.commit_message_history, vec!["second", "first"]);
    }

    #[test]
    fn re_pushing_a_message_moves_it_to_the_front_without_duplicating() {
        let mut settings = VcsLocalSettings::default();
        settings.push_commit_message("a".to_string());
        settings.push_commit_message("b".to_string());
        settings.push_commit_message("a".to_string());
        assert_eq!(settings.commit_message_history, vec!["a", "b"]);
    }

    #[test]
    fn commit_message_history_is_capped_at_25() {
        let mut settings = VcsLocalSettings::default();
        for i in 0..30 {
            settings.push_commit_message(format!("message {i}"));
        }
        assert_eq!(settings.commit_message_history.len(), 25);
        assert_eq!(settings.commit_message_history[0], "message 29");
    }

    #[test]
    fn blame_enabled_is_false_for_a_never_toggled_path() {
        let settings = VcsLocalSettings::default();
        assert!(!settings.blame_enabled("src/main.rs"));
    }

    #[test]
    fn blame_toggle_round_trips_through_update_and_load() {
        let root = tempfile::tempdir().unwrap();
        update(root.path(), |s| {
            s.set_blame_enabled("src/main.rs", true);
        })
        .unwrap();

        let loaded = load(root.path()).unwrap();
        assert!(loaded.blame_enabled("src/main.rs"));
        assert!(!loaded.blame_enabled("src/other.rs"));
    }

    #[test]
    fn turning_blame_off_removes_the_path_rather_than_leaving_it_false() {
        let mut settings = VcsLocalSettings::default();
        settings.set_blame_enabled("a.rs", true);
        settings.set_blame_enabled("b.rs", true);
        settings.set_blame_enabled("a.rs", false);
        assert_eq!(settings.blame_enabled_paths, vec!["b.rs".to_string()]);
    }

    #[test]
    fn re_enabling_an_already_enabled_path_does_not_duplicate_it() {
        let mut settings = VcsLocalSettings::default();
        settings.set_blame_enabled("a.rs", true);
        settings.set_blame_enabled("a.rs", true);
        assert_eq!(settings.blame_enabled_paths, vec!["a.rs".to_string()]);
    }

    #[test]
    fn saving_does_not_touch_the_committed_project_settings_file() {
        let root = tempfile::tempdir().unwrap();
        update(root.path(), |s| {
            s.declined_git_init = Some(true);
        })
        .unwrap();
        assert!(!root
            .path()
            .join(project_settings::PROJECT_DIR)
            .join("settings.toml")
            .exists());
    }
}
