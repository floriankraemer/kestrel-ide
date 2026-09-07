//! Revert one hunk as an edit against the open buffer, not a write to disk
//! (F3-11).
//!
//! The point, worth getting right now even though nothing crosses the FFI
//! seam from this crate yet: reverting a hunk must be **spliced into the
//! open buffer**, exactly like every other edit source this IDE has (a
//! reformat, an intention, an applied AI block) — so the same
//! `beginEditBlock` makes it one `Ctrl+Z`, and a file with unsaved changes
//! elsewhere never gets clobbered by a whole-file write.

use std::path::Path;

use editor_core::diff::Hunk;

use crate::cli::{self, argv};
use crate::error::VcsError;
use crate::repo::Repository;
use crate::staging::path_str;

/// The edit shape a hunk revert produces — `editor_core::diff::LineEdit`,
/// re-exported under the name this crate's callers already use.
pub use editor_core::diff::LineEdit as TextEdit;

impl Repository {
    /// The edit that reverts `hunk` in `relative_path`, computed against
    /// `HEAD`'s copy of the file (via [`Repository::head_blob`]) — the
    /// caller splices this into the live buffer rather than writing
    /// anything to disk.
    pub fn revert_hunk_edit(
        &self,
        relative_path: &Path,
        hunk: &Hunk,
    ) -> Result<TextEdit, VcsError> {
        let before = match self.head_blob(relative_path)? {
            Some((_, text)) => text,
            // A file HEAD has no copy of (new, or HEAD is unborn) can only
            // have a pure-addition hunk against it; reverting deletes
            // everything the hunk added, against an empty "before".
            None => String::new(),
        };
        Ok(revert_hunk(&before, hunk))
    }
}

impl Repository {
    /// `git checkout HEAD -- <path>` — put one file back the way `HEAD` has
    /// it, discarding both its staged and its unstaged changes.
    ///
    /// The whole-file counterpart to [`Self::revert_hunk_edit`], and
    /// deliberately a different shape: a hunk revert is an edit spliced into
    /// the open buffer so one `Ctrl+Z` undoes it, but discarding a whole file
    /// is what `git` itself already does correctly (submodules, symlinks,
    /// filters, CRLF), and re-deriving that from `HEAD`'s blob would be a
    /// second, worse implementation of it. This is a write, so per ADR-0031
    /// §2 it shells out.
    ///
    /// Not undoable from inside the IDE — the working-tree content is gone
    /// once `git` has run. The caller confirms before calling.
    ///
    /// An untracked file is not `HEAD`'s to restore: `git` fails with its own
    /// "did not match any file(s) known to git", which crosses the seam as
    /// [`VcsError::GitFailed`] rather than being silently treated as success.
    pub fn discard_file(&self, relative_path: &Path) -> Result<(), VcsError> {
        let work_dir = self.work_dir_or_err()?;
        let path = path_str(relative_path)?;
        cli::run(&work_dir, &argv::checkout_paths(&[path]))?;
        Ok(())
    }
}

/// The edit that reverts one hunk, given `HEAD`'s text for the file —
/// `editor_core::diff::revert_hunk_edit`, which the diff viewer's apply
/// chevron shares so a revert needs no repository.
pub use editor_core::diff::revert_hunk_edit as revert_hunk;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DiscoverResult;
    use editor_core::diff::diff_lines;
    use std::process::Command;

    /// Apply a [`TextEdit`] to `text`, so a test can assert the *result* of
    /// splicing rather than just the edit's shape.
    fn apply(text: &str, edit: &TextEdit) -> String {
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        let replacement: Vec<String> = edit.new_text.lines().map(str::to_string).collect();
        lines.splice(edit.start_line..edit.end_line, replacement);
        let mut out = lines.join("\n");
        if text.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out
    }

    // -----------------------------------------------------------------
    // Repository::revert_hunk_edit, against a real HEAD blob.
    // -----------------------------------------------------------------

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

    #[test]
    fn revert_hunk_edit_reads_head_and_builds_the_edit() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let working = "one\nTWO\nthree\n";
        let hunk = diff_lines("one\ntwo\nthree\n", working).unwrap().remove(0);

        let repo = open(dir.path());
        let edit = repo.revert_hunk_edit(Path::new("a.txt"), &hunk).unwrap();
        assert_eq!(apply(working, &edit), "one\ntwo\nthree\n");
    }

    #[test]
    fn revert_hunk_edit_on_a_brand_new_file_deletes_everything_the_hunk_added() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);

        let working = "new content\n";
        let hunk = diff_lines("", working).unwrap().remove(0);

        let repo = open(dir.path());
        let edit = repo.revert_hunk_edit(Path::new("new.txt"), &hunk).unwrap();
        assert_eq!(apply(working, &edit), "");
    }

    fn status_porcelain(dir: &Path) -> String {
        Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(dir)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap()
    }

    /// A repository with `a.txt` committed as `"one\ntwo\nthree\n"`.
    fn committed_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        dir
    }

    #[test]
    fn discarding_a_file_restores_heads_content() {
        let dir = committed_repo();
        std::fs::write(dir.path().join("a.txt"), "clobbered\n").unwrap();

        open(dir.path()).discard_file(Path::new("a.txt")).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );
        assert_eq!(status_porcelain(dir.path()), "");
    }

    #[test]
    fn discarding_a_file_clears_a_staged_change_too() {
        // The reason this shells out `checkout HEAD --` and not a bare
        // `checkout --`: the latter restores from the *index*, so a staged
        // edit would survive and "revert this file" would mean two different
        // things depending on whether the user had staged it.
        let dir = committed_repo();
        std::fs::write(dir.path().join("a.txt"), "staged\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        std::fs::write(dir.path().join("a.txt"), "and then edited\n").unwrap();

        open(dir.path()).discard_file(Path::new("a.txt")).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );
        assert_eq!(status_porcelain(dir.path()), "");
    }

    #[test]
    fn discarding_an_untracked_file_fails_and_leaves_it_alone() {
        // `HEAD` has no copy to restore, so this is git's error to report,
        // not something to swallow as success — and the file the user never
        // committed must still be there afterwards.
        let dir = committed_repo();
        std::fs::write(dir.path().join("new.txt"), "mine\n").unwrap();

        let err = open(dir.path())
            .discard_file(Path::new("new.txt"))
            .unwrap_err();

        assert!(matches!(err, VcsError::GitFailed { .. }), "got {err:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "mine\n"
        );
    }
}
