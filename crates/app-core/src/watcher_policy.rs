//! The filesystem-watcher policy (US-3): which watcher events are a
//! genuine external change the user is asked about, and which are this
//! session's own writes echoing back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{AppSession, RetitledTab, TabId};

/// What an open tab becomes when its file was deleted outside the IDE.
#[derive(Debug, PartialEq, Eq)]
pub enum ExternalDeletion {
    /// No unsaved edits: the tab closes, as PhpStorm closes the editor of
    /// a file deleted on disk.
    Close(TabId),
    /// Unsaved edits: the tab stays with them, flagged deleted as a project
    /// tree delete flags it (US-2b), so nothing is lost and nothing saves
    /// silently.
    Keep(RetitledTab),
}

/// How long after this session writes a path to disk itself (`save_tab`) or
/// repoints a tab onto a new path (a tree-driven rename) a matching
/// filesystem-watcher event for that path is treated as an echo of our own
/// change rather than a genuine external edit — see
/// [`AppSession::check_external_change`]. Generous enough to absorb typical
/// inotify/Qt-event-loop latency; not meant to be race-proof.
const SELF_CHANGE_SUPPRESSION_WINDOW: Duration = Duration::from_millis(1500);

/// Whether nothing is at `path` any more (a dangling symlink counts as
/// there: its target may come back).
fn is_gone(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// A digest of file bytes, to tell "still what we wrote" from "changed".
fn digest(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl AppSession {
    /// Decide whether a filesystem-watcher event for `path` is a genuine
    /// external change the user must be prompted about (US-3), returning the
    /// affected tab if so. `None` when `path` has no open tab, when the tab
    /// was already flagged deleted by a tree-driven delete (nothing to
    /// reload/keep), or when `path` was changed by this session itself
    /// within the suppression window (`save_tab` or a tree-driven rename
    /// onto `path`) rather than externally, or when the file still holds
    /// exactly what this session last wrote to it — however late the event
    /// is handled, our own write never raises the prompt.
    pub fn check_external_change(&mut self, path: &Path) -> Option<TabId> {
        let id = self.find_tab_by_path(path)?;
        if self
            .entry(id)
            .map(|e| e.content.is_deleted())
            .unwrap_or(true)
            || is_gone(path)
        {
            return None;
        }
        let is_own_change = self
            .suppressed_changes
            .get(path)
            .map(|at| at.elapsed() < SELF_CHANGE_SUPPRESSION_WINDOW)
            .unwrap_or(false);
        if is_own_change || self.disk_holds_own_write(path) {
            return None;
        }
        Some(id)
    }

    /// Decide what becomes of the open tab of `path` when a watcher event
    /// says the file is gone: `None` when it still exists, has no open tab,
    /// or the tab is already flagged deleted. A deleted file is no "modified
    /// outside the editor" question: Reload has nothing to read.
    pub fn check_external_deletion(&mut self, path: &Path) -> Option<ExternalDeletion> {
        let id = self.find_tab_by_path(path)?;
        let entry = self.entry_mut(id)?;
        if entry.content.is_deleted() || !is_gone(path) {
            return None;
        }
        if self.tab_is_dirty(id) != Some(true) {
            return Some(ExternalDeletion::Close(id));
        }
        self.entry_mut(id)?.content.mark_deleted();
        let title = self.tab_title(id)?;
        Some(ExternalDeletion::Keep(RetitledTab { id, title }))
    }

    /// Remember that this session just wrote `content` to `path`.
    pub(crate) fn note_own_write(&mut self, path: PathBuf, content: &str) {
        self.suppressed_changes.insert(path.clone(), Instant::now());
        self.own_writes.insert(path, digest(content.as_bytes()));
    }

    /// Whether `path` still holds exactly what this session last wrote there.
    fn disk_holds_own_write(&self, path: &Path) -> bool {
        self.own_writes
            .get(path)
            .is_some_and(|&written| std::fs::read(path).is_ok_and(|disk| digest(&disk) == written))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn session_with_project() -> (tempfile::TempDir, tempfile::TempDir, AppSession) {
        let project_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        fs::write(project_dir.path().join("a.txt"), "alpha").unwrap();
        fs::write(project_dir.path().join("b.txt"), "beta").unwrap();
        let mut session = AppSession::with_config_dir(config_dir.path().to_path_buf());
        session.open_project(project_dir.path()).unwrap();
        (project_dir, config_dir, session)
    }

    /// Force a suppression entry to look older than the window without
    /// actually sleeping through it.
    fn expire_suppression(session: &mut AppSession, path: &Path) {
        let expired = Instant::now()
            .checked_sub(SELF_CHANGE_SUPPRESSION_WINDOW + Duration::from_secs(1))
            .expect("process uptime exceeds the suppression window in tests");
        session
            .suppressed_changes
            .insert(path.to_path_buf(), expired);
    }

    #[test]
    fn external_change_is_reported_only_for_open_undeleted_unsuppressed_tabs() {
        let (project_dir, _config, mut session) = session_with_project();
        let open_path = project_dir.path().join("a.txt");
        let closed_path = project_dir.path().join("b.txt");
        let tab = session.open_file(&open_path).unwrap();

        // A path with no open tab: nothing to prompt about.
        assert_eq!(session.check_external_change(&closed_path), None);
        // A genuinely external change to an open tab: prompt.
        assert_eq!(session.check_external_change(&open_path), Some(tab.id));
    }

    #[test]
    fn a_file_deleted_outside_closes_its_clean_tab_and_keeps_a_dirty_one() {
        let (project_dir, _config, mut session) = session_with_project();
        let clean = project_dir.path().join("a.txt");
        let dirty = project_dir.path().join("b.txt");
        let clean_tab = session.open_file(&clean).unwrap();
        let dirty_tab = session.open_file(&dirty).unwrap();
        session.edit_tab(dirty_tab.id, "beta, edited").unwrap();
        // Still there: no deletion, whatever else changed.
        assert_eq!(session.check_external_deletion(&clean), None);

        fs::remove_file(&clean).unwrap();
        fs::remove_file(&dirty).unwrap();
        // Not a "modified outside the editor" question: nothing to reload.
        assert_eq!(session.check_external_change(&clean), None);
        assert_eq!(
            session.check_external_deletion(&clean),
            Some(ExternalDeletion::Close(clean_tab.id))
        );
        assert_eq!(
            session.check_external_deletion(&dirty),
            Some(ExternalDeletion::Keep(RetitledTab {
                id: dirty_tab.id,
                title: "b.txt (deleted)".into()
            }))
        );
        // The kept tab is flagged once: a later event asks nothing.
        assert_eq!(session.check_external_deletion(&dirty), None);
        assert_eq!(session.check_external_change(&dirty), None);
    }

    #[test]
    fn suppression_expires_after_the_window() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();
        session.save_tab(tab.id, "our own write").unwrap();
        assert_eq!(session.check_external_change(&path), None);

        expire_suppression(&mut session, &path);
        fs::write(&path, "someone else's write").unwrap();
        assert_eq!(
            session.check_external_change(&path),
            Some(tab.id),
            "an old suppression entry must not mask a real external change"
        );
    }

    #[test]
    fn a_late_echo_of_our_own_write_is_not_an_external_change() {
        // The Qt thread was busy (a slow format-on-save, program lookups)
        // for longer than the window before it handled the watcher event.
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();
        session.save_tab(tab.id, "our own write").unwrap();
        expire_suppression(&mut session, &path);
        assert_eq!(session.check_external_change(&path), None);

        // The same holds for Save As and an MCP save_buffer.
        let other = project_dir.path().join("c.txt");
        session
            .save_tab_as(tab.id, other.clone(), "saved as")
            .unwrap();
        expire_suppression(&mut session, &other);
        assert_eq!(session.check_external_change(&other), None);
        session.edit_tab(tab.id, "edited by an agent").unwrap();
        session.save_buffer(tab.id).unwrap();
        expire_suppression(&mut session, &other);
        assert_eq!(session.check_external_change(&other), None);
        fs::write(&other, "changed outside").unwrap();
        assert_eq!(session.check_external_change(&other), Some(tab.id));
    }
}
