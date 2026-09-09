//! Staging: per-file `git add`, and per-hunk via a generated patch fed to
//! `git apply --cached` (F3-6).

use std::path::Path;

use editor_core::diff::Hunk;

use crate::cli::{self, argv};
use crate::error::VcsError;
use crate::repo::Repository;

impl Repository {
    /// `git add <path>` — stage the whole file as it stands in the working
    /// tree, whatever it currently contains.
    pub fn stage_file(&self, relative_path: &Path) -> Result<(), VcsError> {
        let work_dir = self.work_dir_or_err()?;
        let path = path_str(relative_path)?;
        cli::run(&work_dir, &argv::add(&[path]))?;
        Ok(())
    }

    /// `git reset -- <path>` — unstage the whole file, leaving the working
    /// tree and `HEAD` untouched. The whole-file inverse of [`Self::stage_file`];
    /// [`Self::unstage_hunk`] only reverses one hunk already known to the
    /// caller.
    pub fn unstage_file(&self, relative_path: &Path) -> Result<(), VcsError> {
        let work_dir = self.work_dir_or_err()?;
        let path = path_str(relative_path)?;
        cli::run(&work_dir, &argv::reset(&[path]))?;
        Ok(())
    }

    /// `git add -A` — stage every change in the repository, the whole-tree
    /// counterpart to [`Self::stage_file`] behind the Changes dock's
    /// "Stage all" button.
    pub fn stage_all(&self) -> Result<(), VcsError> {
        let work_dir = self.work_dir_or_err()?;
        cli::run(&work_dir, &argv::add_all())?;
        Ok(())
    }

    /// `git reset` — unstage everything, leaving the working tree and
    /// `HEAD` untouched. The whole-tree counterpart to [`Self::unstage_file`]
    /// behind the Changes dock's "Unstage all" button.
    pub fn unstage_all(&self) -> Result<(), VcsError> {
        let work_dir = self.work_dir_or_err()?;
        cli::run(&work_dir, &argv::reset_all())?;
        Ok(())
    }

    /// Stage exactly one hunk, via a generated patch applied with
    /// `git apply --cached`. `before`/`after` must be the same two texts
    /// the hunk was computed from (typically the index's copy and the
    /// working tree's copy — [`crate::hunks::HunkCache`] diffs against
    /// `HEAD`, not the index, so a caller staging a hunk needs the
    /// index-vs-worktree hunk, not the gutter's HEAD-vs-worktree one).
    pub fn stage_hunk(
        &self,
        relative_path: &Path,
        before: &str,
        after: &str,
        hunk: &Hunk,
    ) -> Result<(), VcsError> {
        self.apply_hunk(relative_path, before, after, hunk, false)
    }

    /// The inverse of [`Self::stage_hunk`]: `git apply --reverse --cached`,
    /// removing just this hunk's change from the index without touching the
    /// working tree.
    pub fn unstage_hunk(
        &self,
        relative_path: &Path,
        before: &str,
        after: &str,
        hunk: &Hunk,
    ) -> Result<(), VcsError> {
        self.apply_hunk(relative_path, before, after, hunk, true)
    }

    fn apply_hunk(
        &self,
        relative_path: &Path,
        before: &str,
        after: &str,
        hunk: &Hunk,
        reverse: bool,
    ) -> Result<(), VcsError> {
        let work_dir = self.work_dir_or_err()?;
        let path = path_str(relative_path)?;
        let patch = hunk_patch(path, before, after, hunk);
        cli::run_with_stdin(&work_dir, &argv::apply_cached(reverse), &patch)?;
        Ok(())
    }

    pub(crate) fn work_dir_or_err(&self) -> Result<std::path::PathBuf, VcsError> {
        self.work_dir().ok_or(VcsError::OutsideWorkingTree)
    }
}

pub(crate) fn path_str(path: &Path) -> Result<&str, VcsError> {
    path.to_str()
        .ok_or_else(|| VcsError::Read(format!("{} is not valid UTF-8", path.display())))
}

/// How many unchanged lines to carry on each side of a hunk. `git apply`
/// verifies a patch's context against the file it is applied to as well as
/// the line numbers in the range header; a hunk built with zero context
/// reliably fails with "patch does not apply" even when the numbers are
/// exact, so this matches `diff -u`'s own default rather than trying to
/// find the minimum `git apply` will accept.
const CONTEXT_LINES: usize = 3;

/// Build a unified-diff patch for exactly one hunk, in the shape
/// `git apply` expects: `--- a/<path>` / `+++ b/<path>` headers, one
/// `@@ -old_start,old_len +new_start,new_len @@` range header, up to
/// [`CONTEXT_LINES`] unchanged lines, the removed and added lines, then up
/// to [`CONTEXT_LINES`] more unchanged lines.
///
/// A trailing-newline mismatch between `before`/`after` and their own
/// `.lines()` split is not handled (`git apply`'s `\ No newline at end of
/// file` marker) — ponytail: every caller today hands in text read from a
/// real file, which ends in a newline. Add the marker if a hunk touching
/// the last, newline-less line of a file needs staging.
pub fn hunk_patch(path: &str, before: &str, after: &str, hunk: &Hunk) -> String {
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();

    // Context lines are common to both sides by definition (a hunk only
    // begins where the two texts diverge), so the same count and the same
    // source (`old_lines`) work as leading and trailing context for both
    // the old and the new range.
    let context_before = hunk.old.start.min(CONTEXT_LINES);
    let context_after = (old_lines.len() - hunk.old.end).min(CONTEXT_LINES);

    let old_start = hunk.old.start - context_before;
    let old_len = context_before + hunk.old.len() + context_after;
    let new_start = hunk.new.start - context_before;
    let new_len = context_before + hunk.new.len() + context_after;

    let mut body = String::new();
    for line in &old_lines[old_start..hunk.old.start] {
        body.push(' ');
        body.push_str(line);
        body.push('\n');
    }
    for line in &old_lines[hunk.old.clone()] {
        body.push('-');
        body.push_str(line);
        body.push('\n');
    }
    for line in &new_lines[hunk.new.clone()] {
        body.push('+');
        body.push_str(line);
        body.push('\n');
    }
    for line in &old_lines[hunk.old.end..hunk.old.end + context_after] {
        body.push(' ');
        body.push_str(line);
        body.push('\n');
    }

    format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{old_len} +{},{new_len} @@\n{body}",
        old_start + 1,
        new_start + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DiscoverResult;
    use editor_core::diff::diff_lines;
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

    fn status_porcelain(dir: &Path) -> String {
        Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(dir)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap()
    }

    // -----------------------------------------------------------------
    // hunk_patch: well-formed unified diff, no real git needed.
    // -----------------------------------------------------------------

    #[test]
    fn hunk_patch_headers_name_the_path_on_both_sides() {
        let before = "one\ntwo\nthree\n";
        let after = "one\nTWO\nthree\n";
        let hunk = &diff_lines(before, after).unwrap()[0];
        let patch = hunk_patch("a.txt", before, after, hunk);
        assert!(patch.starts_with("--- a/a.txt\n+++ b/a.txt\n"));
    }

    #[test]
    fn hunk_patch_body_carries_context_around_the_change() {
        let before = "one\ntwo\nthree\n";
        let after = "one\nTWO\nthree\n";
        let hunk = &diff_lines(before, after).unwrap()[0];
        let patch = hunk_patch("a.txt", before, after, hunk);
        let body = patch.split_once("@@\n").unwrap().1;
        assert_eq!(body, " one\n-two\n+TWO\n three\n");
    }

    #[test]
    fn hunk_patch_range_header_matches_a_pure_addition() {
        let before = "a\nc\n";
        let after = "a\nb\nc\n";
        let hunk = &diff_lines(before, after).unwrap()[0];
        let patch = hunk_patch("a.txt", before, after, hunk);
        // Both files are shorter than CONTEXT_LINES, so every line becomes
        // context: old carries all 2 lines, new carries all 3.
        assert!(patch.contains("@@ -1,2 +1,3 @@\n"), "patch was:\n{patch}");
    }

    // -----------------------------------------------------------------
    // Round-trip against a real git binary: apply, then apply --reverse,
    // must reproduce the original index state.
    // -----------------------------------------------------------------

    #[test]
    fn staging_a_hunk_updates_the_index_and_status() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let before = "one\ntwo\nthree\n";
        let after = "one\nTWO\nthree\n";
        std::fs::write(dir.path().join("a.txt"), after).unwrap();
        let hunk = diff_lines(before, after).unwrap().remove(0);

        let repo = open(dir.path());
        repo.stage_hunk(Path::new("a.txt"), before, after, &hunk)
            .unwrap();

        // Staged in the index (shows as "M" in the first status column) and
        // no longer dirty in the worktree relative to the index.
        assert_eq!(status_porcelain(dir.path()), "M  a.txt\n");
    }

    #[test]
    fn unstaging_a_hunk_reverses_a_staged_one() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let before = "one\ntwo\nthree\n";
        let after = "one\nTWO\nthree\n";
        std::fs::write(dir.path().join("a.txt"), after).unwrap();
        let hunk = diff_lines(before, after).unwrap().remove(0);

        let repo = open(dir.path());
        repo.stage_hunk(Path::new("a.txt"), before, after, &hunk)
            .unwrap();
        repo.unstage_hunk(Path::new("a.txt"), before, after, &hunk)
            .unwrap();

        // Back to fully unstaged: worktree differs from HEAD/index, index
        // matches HEAD.
        assert_eq!(status_porcelain(dir.path()), " M a.txt\n");
    }

    #[test]
    fn stage_file_stages_the_whole_working_tree_copy() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        let repo = open(dir.path());
        repo.stage_file(Path::new("a.txt")).unwrap();

        assert_eq!(status_porcelain(dir.path()), "M  a.txt\n");
    }

    #[test]
    fn unstage_file_reverses_a_staged_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        let repo = open(dir.path());
        repo.stage_file(Path::new("a.txt")).unwrap();
        assert_eq!(status_porcelain(dir.path()), "M  a.txt\n");

        repo.unstage_file(Path::new("a.txt")).unwrap();
        assert_eq!(status_porcelain(dir.path()), " M a.txt\n");
    }

    #[test]
    fn stage_all_stages_every_change_tracked_and_untracked() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        let repo = open(dir.path());
        repo.stage_all().unwrap();

        let output = status_porcelain(dir.path());
        let mut lines: Vec<&str> = output.lines().collect();
        lines.sort_unstable();
        assert_eq!(lines, vec!["A  new.txt", "M  a.txt"]);
    }

    #[test]
    fn unstage_all_reverses_every_staged_change() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        let repo = open(dir.path());
        repo.unstage_all().unwrap();

        let output = status_porcelain(dir.path());
        let mut lines: Vec<&str> = output.lines().collect();
        lines.sort_unstable();
        assert_eq!(lines, vec![" M a.txt", "?? new.txt"]);
    }
}
