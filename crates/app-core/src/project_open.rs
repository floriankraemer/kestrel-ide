//! The swap-in half of an off-thread project open/rebuild (ADR-0037).
//!
//! `ui-shell`'s worker thread does the actual filesystem walk with
//! `project_model::open_folder_sorted`/`rebuild_tree_sorted` — both pure
//! functions, no `AppSession`, safe to run off the Qt thread — and hands the
//! result back here to install.

use std::path::Path;

use project_model::{DirectoryTree, Project, ProjectWatcher};

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

    /// Swap in an already re-walked tree for the still-current project
    /// root, e.g. after a filesystem-watcher rebuild ran off the Qt thread.
    /// Returns whether it was applied — `false` means `root` no longer
    /// names the open project (it changed while the rebuild was in
    /// flight), so the caller should not reset its view.
    pub fn install_rebuilt_tree(&mut self, root: &Path, tree: DirectoryTree) -> bool {
        self.project.install_tree(root, tree)
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
    fn install_rebuilt_tree_is_dropped_when_the_root_no_longer_matches() {
        let (project_dir, _config, mut session) = session_with_project();
        fs::write(project_dir.path().join("c.txt"), "gamma").unwrap();
        let tree =
            project_model::rebuild_tree_sorted(project_dir.path(), session.tree_sort_order())
                .unwrap();

        let other_dir = tempfile::tempdir().unwrap();
        assert!(!session.install_rebuilt_tree(other_dir.path(), tree));

        let tree =
            project_model::rebuild_tree_sorted(project_dir.path(), session.tree_sort_order())
                .unwrap();
        assert!(session.install_rebuilt_tree(project_dir.path(), tree));
    }
}
