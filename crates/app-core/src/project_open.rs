//! The swap-in half of an off-thread project open/watcher-registration/
//! directory-refresh (ADR-0037, ADR-0062).
//!
//! `ui-shell`'s worker threads do the actual filesystem I/O with
//! `project_model::open_folder_sorted`/`list_dir` — both pure functions, no
//! `AppSession`, safe to run off the Qt thread — and hand the result back
//! here to install.

use std::path::Path;

use project_model::{DirDiff, ListedEntry, Project, ProjectWatcher};

use crate::AppSession;

impl AppSession {
    /// Install an already walked-and-sorted project as current, replacing
    /// any previous one — the swap-in half of "Open Folder" when the walk
    /// itself ran off the Qt thread (`ui-shell`'s async worker). Returns the
    /// previous project and watcher, if any, so the caller can drop them off
    /// the Qt thread instead of blocking paint on freeing a huge tree.
    pub fn install_opened_project(
        &mut self,
        project: Project,
    ) -> (Option<Project>, Option<ProjectWatcher>) {
        self.project.install_project(project)
    }

    /// Install a watcher that finished registering off the Qt thread; see
    /// `project_model::ProjectSession::install_watcher` for the stale-root
    /// guard this forwards to.
    pub fn install_watcher(
        &mut self,
        root: &Path,
        watcher: ProjectWatcher,
    ) -> Result<Option<ProjectWatcher>, ProjectWatcher> {
        self.project.install_watcher(root, watcher)
    }

    /// Mark a directory as having a `list_dir` worker already in flight for
    /// it (the `fetchMore` re-entrancy guard) — see
    /// `project_model::ProjectSession::mark_dir_loading`.
    pub fn mark_tree_dir_loading(&mut self, root: &Path, dir_id: usize) {
        self.project.mark_dir_loading(root, dir_id);
    }

    /// Insert an off-thread `list_dir` result as `dir_path`'s children (a
    /// `fetchMore` landing); see
    /// `project_model::ProjectSession::attach_dir_children`.
    pub fn attach_tree_children(
        &mut self,
        root: &Path,
        dir_path: &Path,
        entries: Vec<ListedEntry>,
    ) -> Option<Vec<usize>> {
        self.project.attach_dir_children(root, dir_path, entries)
    }

    /// Diff an off-thread `list_dir` result against `dir_path`'s current
    /// children (a watcher-driven or mutation-driven incremental refresh);
    /// see `project_model::ProjectSession::refresh_dir`.
    pub fn refresh_tree_dir(
        &mut self,
        root: &Path,
        dir_path: &Path,
        entries: Vec<ListedEntry>,
    ) -> Option<DirDiff> {
        self.project.refresh_dir(root, dir_path, entries)
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
        let mut session = AppSession::with_config_dir(config_dir.path().to_path_buf());
        session.open_project(project_dir.path()).unwrap();
        (project_dir, config_dir, session)
    }

    #[test]
    fn install_opened_project_replaces_the_current_project() {
        let project_dir = tempfile::tempdir().unwrap();
        fs::write(project_dir.path().join("a.txt"), "alpha").unwrap();
        let mut session = AppSession::new();
        assert!(session.project().is_none());

        let project = project_model::open_folder_sorted(
            project_dir.path(),
            project_model::SortOrder::Ascending,
        )
        .unwrap();
        let (replaced, replaced_watcher) = session.install_opened_project(project);

        assert!(replaced.is_none());
        assert!(replaced_watcher.is_none());
        assert_eq!(session.root_path().unwrap(), project_dir.path());
    }

    #[test]
    fn install_watcher_is_rejected_for_a_stale_root() {
        let (project_dir, _config, mut session) = session_with_project();
        let watcher =
            project_model::ProjectWatcher::start(project_dir.path(), false, |_, _| {}).unwrap();

        let other_dir = tempfile::tempdir().unwrap();
        assert!(session.install_watcher(other_dir.path(), watcher).is_err());
    }

    #[test]
    fn install_watcher_applies_for_the_still_open_root() {
        let (project_dir, _config, mut session) = session_with_project();
        let watcher =
            project_model::ProjectWatcher::start(project_dir.path(), false, |_, _| {}).unwrap();

        let Ok(none_replaced) = session.install_watcher(project_dir.path(), watcher) else {
            panic!("root still matches");
        };
        assert!(none_replaced.is_none());
    }

    #[test]
    fn attach_tree_children_is_dropped_when_the_root_no_longer_matches() {
        let (project_dir, _config, mut session) = session_with_project();
        fs::create_dir(project_dir.path().join("sub")).unwrap();
        let entries =
            project_model::list_dir(&project_dir.path().join("sub"), session.tree_sort_order())
                .unwrap();

        let other_dir = tempfile::tempdir().unwrap();
        assert!(session
            .attach_tree_children(other_dir.path(), &project_dir.path().join("sub"), entries)
            .is_none());
    }

    #[test]
    fn attach_tree_children_applies_for_the_still_open_root() {
        // `sub` must exist before the project opens, so `open_root`'s own
        // root-level listing already has a node for it — `attach_dir_children`
        // resolves its target by path against nodes the tree already knows
        // about, the same as the real `fetchMore` flow (expand a directory
        // that's already a visible row).
        let project_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        fs::create_dir(project_dir.path().join("sub")).unwrap();
        fs::write(project_dir.path().join("sub/f.txt"), "").unwrap();
        let mut session = AppSession::with_config_dir(config_dir.path().to_path_buf());
        session.open_project(project_dir.path()).unwrap();

        let entries =
            project_model::list_dir(&project_dir.path().join("sub"), session.tree_sort_order())
                .unwrap();
        let inserted = session
            .attach_tree_children(project_dir.path(), &project_dir.path().join("sub"), entries)
            .unwrap();
        assert_eq!(inserted.len(), 1);
    }

    #[test]
    fn refresh_tree_dir_is_dropped_when_the_root_no_longer_matches() {
        let (project_dir, _config, mut session) = session_with_project();
        let entries =
            project_model::list_dir(project_dir.path(), session.tree_sort_order()).unwrap();

        let other_dir = tempfile::tempdir().unwrap();
        assert!(session
            .refresh_tree_dir(other_dir.path(), project_dir.path(), entries)
            .is_none());
    }

    #[test]
    fn refresh_tree_dir_applies_for_the_still_open_root() {
        let (project_dir, _config, mut session) = session_with_project();
        fs::write(project_dir.path().join("b.txt"), "beta").unwrap();
        let entries =
            project_model::list_dir(project_dir.path(), session.tree_sort_order()).unwrap();

        let diff = session
            .refresh_tree_dir(project_dir.path(), project_dir.path(), entries)
            .unwrap();
        assert!(!diff.ops.is_empty());
    }
}
