//! What one commit changed: the changed-file list and a per-file diff
//! against its first parent, for the commit-detail dock (F3-12e). A merge
//! commit is diffed against its first parent only, the same choice `git
//! show` makes by default — a full multi-parent combined diff is not
//! something this view needs.

use std::path::Path;

use editor_core::diff::{self, DiffError, Hunk};

use crate::error::VcsError;
use crate::repo::{ChangeKind, Repository};

/// One path a commit touched, reusing [`ChangeKind`] rather than a new enum
/// — a commit's changes and the working tree's are the same four kinds of
/// change to a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedCommitFile {
    pub path: std::path::PathBuf,
    pub change: ChangeKind,
}

/// Whole-file before/after text plus line hunks for one file in one commit,
/// for the commit-detail dock's `DiffView`. Not cached — `commit_diff` is
/// only ever fetched for the commit(s) currently open in the dock, so there
/// is nothing a cache would save a second look-up of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old_text: String,
    pub new_text: String,
    pub hunks: Vec<Hunk>,
}

impl Repository {
    /// The paths `id` changed relative to its first parent (or relative to
    /// the empty tree, for a root commit), newest-parent-agnostic — a merge
    /// commit's other parents are not diffed against.
    pub fn changed_files(&self, id: &str) -> Result<Vec<ChangedCommitFile>, VcsError> {
        let commit = self.resolve_commit(id)?;
        let new_tree = commit.tree().map_err(|e| VcsError::Read(e.to_string()))?;
        let old_tree = match commit.parent_ids().next() {
            Some(parent_id) => parent_id
                .object()
                .map_err(|e| VcsError::Read(e.to_string()))?
                .peel_to_tree()
                .map_err(|e| VcsError::Read(e.to_string()))?,
            None => self.inner.empty_tree(),
        };

        let mut files = Vec::new();
        old_tree
            .changes()
            .map_err(|e| VcsError::Read(e.to_string()))?
            .options(|opts| {
                opts.track_rewrites(None);
            })
            .for_each_to_obtain_tree(&new_tree, |change| {
                use gix::object::tree::diff::Change;
                let (path, kind, is_blob) = match &change {
                    Change::Addition {
                        location,
                        entry_mode,
                        ..
                    } => (location, ChangeKind::Added, entry_mode.is_blob()),
                    Change::Deletion {
                        location,
                        entry_mode,
                        ..
                    } => (location, ChangeKind::Deleted, entry_mode.is_blob()),
                    Change::Modification {
                        location,
                        entry_mode,
                        ..
                    } => (location, ChangeKind::Modified, entry_mode.is_blob()),
                    Change::Rewrite {
                        location,
                        entry_mode,
                        ..
                    } => (location, ChangeKind::Modified, entry_mode.is_blob()),
                };
                if is_blob {
                    files.push(ChangedCommitFile {
                        path: gix::path::from_bstr(*path).into_owned(),
                        change: kind,
                    });
                }
                Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
            })
            .map_err(|e| VcsError::Read(e.to_string()))?;

        Ok(files)
    }

    /// The before/after text and line hunks for one file as changed by
    /// commit `id`, against its first parent — an added or deleted file
    /// gets an empty side, the same convention [`Repository::head_blob`]
    /// uses for a file `HEAD` does not have.
    pub fn commit_file_diff(&self, id: &str, relative_path: &Path) -> Result<FileDiff, VcsError> {
        let commit = self.resolve_commit(id)?;
        let new_text = self.blob_at(id, relative_path)?.unwrap_or_default();
        let old_text = match commit.parent_ids().next() {
            Some(parent_id) => self
                .blob_at(&parent_id.to_hex().to_string(), relative_path)?
                .unwrap_or_default(),
            None => String::new(),
        };

        let hunks = diff::diff_lines(&old_text, &new_text).map_err(|e| match e {
            DiffError::TooLarge => VcsError::TooLargeToDiff,
        })?;

        Ok(FileDiff {
            old_text,
            new_text,
            hunks,
        })
    }

    fn resolve_commit(&self, id: &str) -> Result<gix::Commit<'_>, VcsError> {
        self.inner
            .rev_parse_single(id)
            .map_err(|e| VcsError::Read(e.to_string()))?
            .object()
            .map_err(|e| VcsError::Read(e.to_string()))?
            .try_into_commit()
            .map_err(|e| VcsError::Read(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DiscoverResult;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn open(dir: &Path) -> Repository {
        match Repository::discover(dir).unwrap() {
            DiscoverResult::Found(repo) => *repo,
            DiscoverResult::NotARepository => panic!("expected a repository"),
        }
    }

    fn head(dir: &Path) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    #[test]
    fn changed_files_on_a_root_commit_reports_additions() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "root"]);

        let repo = open(dir.path());
        let files = repo.changed_files(&head(dir.path())).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, Path::new("a.txt"));
        assert_eq!(files[0].change, ChangeKind::Added);
    }

    #[test]
    fn changed_files_reports_a_modification() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);

        let repo = open(dir.path());
        let files = repo.changed_files(&head(dir.path())).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].change, ChangeKind::Modified);
    }

    #[test]
    fn changed_files_reports_a_deletion() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::remove_file(dir.path().join("a.txt")).unwrap();
        git(dir.path(), &["commit", "-am", "delete a"]);

        let repo = open(dir.path());
        let files = repo.changed_files(&head(dir.path())).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].change, ChangeKind::Deleted);
    }

    #[test]
    fn changed_files_for_a_merge_commit_is_against_its_first_parent_only() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        git(dir.path(), &["checkout", "-b", "main"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "base"]);

        git(dir.path(), &["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("b.txt"), "feature\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-m", "feature file"]);

        git(dir.path(), &["checkout", "main"]);
        std::fs::write(dir.path().join("c.txt"), "main\n").unwrap();
        git(dir.path(), &["add", "c.txt"]);
        git(dir.path(), &["commit", "-m", "main file"]);
        git(
            dir.path(),
            &["merge", "--no-ff", "-m", "merge feature", "feature"],
        );

        let repo = open(dir.path());
        let files = repo.changed_files(&head(dir.path())).unwrap();
        // Against first parent (main file's commit): only feature's own
        // addition (b.txt) shows up, not c.txt (already on first parent).
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, Path::new("b.txt"));
        assert_eq!(files[0].change, ChangeKind::Added);
    }

    #[test]
    fn commit_file_diff_matches_diff_lines_directly() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("a.txt"), "one\nTWO\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);

        let repo = open(dir.path());
        let diff = repo
            .commit_file_diff(&head(dir.path()), Path::new("a.txt"))
            .unwrap();
        assert_eq!(diff.old_text, "one\ntwo\n");
        assert_eq!(diff.new_text, "one\nTWO\n");
        let expected = diff::diff_lines("one\ntwo\n", "one\nTWO\n").unwrap();
        assert_eq!(diff.hunks, expected);
    }

    #[test]
    fn commit_file_diff_for_an_added_file_has_an_empty_old_side() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "root"]);

        let repo = open(dir.path());
        let diff = repo
            .commit_file_diff(&head(dir.path()), Path::new("a.txt"))
            .unwrap();
        assert_eq!(diff.old_text, "");
        assert_eq!(diff.new_text, "one\n");
    }

    #[test]
    fn commit_file_diff_for_a_deleted_file_has_an_empty_new_side() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::remove_file(dir.path().join("a.txt")).unwrap();
        git(dir.path(), &["commit", "-am", "delete a"]);

        let repo = open(dir.path());
        let diff = repo
            .commit_file_diff(&head(dir.path()), Path::new("a.txt"))
            .unwrap();
        assert_eq!(diff.old_text, "one\n");
        assert_eq!(diff.new_text, "");
    }
}
