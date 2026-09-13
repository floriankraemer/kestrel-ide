//! Staging: per-file `git add`, and per-hunk via a generated patch fed to
//! `git apply --cached` (F3-6).

use std::path::Path;

use editor_core::diff::Hunk;

use crate::cli::{self, argv};
use crate::error::VcsError;
use crate::hunks::ranges_overlap;
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

    /// Stage the part of `gutter_hunk` — a `HEAD`-vs-worktree hunk, the
    /// gutter's own view — that is not staged yet, by finding the
    /// corresponding index-vs-worktree hunk (matched by overlapping
    /// working-tree line range, the coordinate the two share) and staging
    /// that one instead of `gutter_hunk` itself. This is the fix for R6's
    /// "stages the whole file" bug: staging always used to diff against
    /// `HEAD`, so it staged everything between `HEAD` and the worktree —
    /// correct only when nothing was staged yet.
    ///
    /// Returns `Ok(false)` with nothing staged when no index-vs-worktree
    /// hunk overlaps `gutter_hunk` at all — it is already fully staged.
    pub fn stage_hunk_matching(
        &self,
        relative_path: &Path,
        working_text: &str,
        gutter_hunk: &Hunk,
    ) -> Result<bool, VcsError> {
        let index_diff = self.hunks_against_index(relative_path, working_text)?;
        let Some(target) = index_diff
            .hunks
            .iter()
            .find(|h| ranges_overlap(&h.new, &gutter_hunk.new))
        else {
            return Ok(false);
        };
        self.stage_hunk(relative_path, &index_diff.before_text, working_text, target)?;
        Ok(true)
    }

    /// Unstage the part of `gutter_hunk` that is currently staged, by
    /// finding the corresponding `HEAD`-vs-index hunk (matched by
    /// overlapping `HEAD`-side line range — see
    /// [`crate::hunks::classify_hunk`] for the same match used to colour
    /// the gutter popup) and reversing that one out of the index.
    ///
    /// Returns `Ok(false)` with nothing unstaged when no staged hunk
    /// overlaps `gutter_hunk` — none of it is staged.
    pub fn unstage_hunk_matching(
        &self,
        relative_path: &Path,
        gutter_hunk: &Hunk,
    ) -> Result<bool, VcsError> {
        let staged = self.staged_hunks(relative_path)?;
        let Some(target) = staged
            .hunks
            .iter()
            .find(|h| ranges_overlap(&h.old, &gutter_hunk.old))
        else {
            return Ok(false);
        };
        self.unstage_hunk(relative_path, &staged.head_text, &staged.index_text, target)?;
        Ok(true)
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
/// A line handed a `str::lines()` split loses information about whether the
/// *source* text ended in a newline — `"a\nb"` and `"a\nb\n"` both yield
/// `["a", "b"]`. When the last line this patch touches (context, or a
/// changed line) is also the source text's last line and that text does not
/// end in `\n`, `git apply` requires the patch to say so with a trailing
/// `\ No newline at end of file` marker, or it refuses the hunk outright.
pub fn hunk_patch(path: &str, before: &str, after: &str, hunk: &Hunk) -> String {
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();
    let old_ends_with_newline = before.is_empty() || before.ends_with('\n');
    let new_ends_with_newline = after.is_empty() || after.ends_with('\n');

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

    // Whether this patch's last body line comes from `old_lines`/`new_lines`
    // at all (trailing context) — if there is trailing context, that line is
    // never the source text's last line, so no marker is ever needed for it.
    let has_trailing_context = context_after > 0;

    let mut body = String::new();
    for line in &old_lines[old_start..hunk.old.start] {
        body.push(' ');
        body.push_str(line);
        body.push('\n');
    }
    push_changed_lines(
        &mut body,
        '-',
        &old_lines[hunk.old.clone()],
        !has_trailing_context && !old_ends_with_newline,
    );
    push_changed_lines(
        &mut body,
        '+',
        &new_lines[hunk.new.clone()],
        !has_trailing_context && !new_ends_with_newline,
    );
    for line in &old_lines[hunk.old.end..hunk.old.end + context_after] {
        body.push(' ');
        body.push_str(line);
        body.push('\n');
    }
    if has_trailing_context
        && !old_ends_with_newline
        && hunk.old.end + context_after == old_lines.len()
    {
        body.push_str("\\ No newline at end of file\n");
    }

    format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{old_len} +{},{new_len} @@\n{body}",
        old_start + 1,
        new_start + 1,
    )
}

/// Emit one side (`-` removed or `+` added) of a hunk's changed lines,
/// appending `git apply`'s `\ No newline at end of file` marker after the
/// last one when `last_line_has_no_newline` says this side's source text
/// ends without a trailing `\n` right at this hunk's last line.
fn push_changed_lines(
    body: &mut String,
    prefix: char,
    lines: &[&str],
    last_line_has_no_newline: bool,
) {
    for (i, line) in lines.iter().enumerate() {
        body.push(prefix);
        body.push_str(line);
        body.push('\n');
        if last_line_has_no_newline && i == lines.len() - 1 {
            body.push_str("\\ No newline at end of file\n");
        }
    }
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

    #[test]
    fn hunk_patch_marks_a_changed_line_with_no_trailing_newline() {
        // "three" is both the changed line and the file's last line, with
        // no trailing "\n" — after.lines() alone can't tell that apart from
        // a file that does end in "\n", so the marker must come from the
        // source text itself.
        let before = "one\ntwo\nthree\n";
        let after = "one\ntwo\nTHREE";
        let hunk = &diff_lines(before, after).unwrap()[0];
        let patch = hunk_patch("a.txt", before, after, hunk);
        assert!(
            patch.ends_with("-three\n+THREE\n\\ No newline at end of file\n"),
            "patch was:\n{patch}"
        );
    }

    #[test]
    fn hunk_patch_marks_the_removed_side_when_only_before_lacks_a_newline() {
        let before = "one\ntwo\nthree";
        let after = "one\ntwo\nTHREE\n";
        let hunk = &diff_lines(before, after).unwrap()[0];
        let patch = hunk_patch("a.txt", before, after, hunk);
        let body = patch.split_once("@@\n").unwrap().1;
        assert_eq!(
            body,
            " one\n two\n-three\n\\ No newline at end of file\n+THREE\n"
        );
    }

    #[test]
    fn hunk_patch_adds_no_marker_when_the_hunk_has_trailing_context() {
        let before = "one\ntwo\nthree\n";
        let after = "ONE\ntwo\nthree\n";
        let hunk = &diff_lines(before, after).unwrap()[0];
        let patch = hunk_patch("a.txt", before, after, hunk);
        assert!(
            !patch.contains("No newline"),
            "both texts end in a newline: patch was:\n{patch}"
        );
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
    fn staging_a_hunk_that_touches_a_newline_less_last_line_applies() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let before = "one\ntwo\nthree";
        let after = "one\ntwo\nTHREE";
        std::fs::write(dir.path().join("a.txt"), after).unwrap();
        let hunk = diff_lines(before, after).unwrap().remove(0);

        let repo = open(dir.path());
        // Before the `\ No newline at end of file` marker, `git apply`
        // rejected this hunk outright ("patch does not apply" /
        // "corrupt patch") — this must now succeed.
        repo.stage_hunk(Path::new("a.txt"), before, after, &hunk)
            .unwrap();

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
    fn stage_hunk_matching_stages_only_the_still_unstaged_hunk() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(
            dir.path().join("a.txt"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        // Stage an edit to line 2, then make a second, unstaged edit to
        // line 6 — the reproduction of R6's "stages the whole file" bug.
        std::fs::write(
            dir.path().join("a.txt"),
            "one\nTWO\nthree\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        git(dir.path(), &["add", "a.txt"]);
        let working = "one\nTWO\nthree\nfour\nfive\nSIX\nseven\n";
        std::fs::write(dir.path().join("a.txt"), working).unwrap();

        let gutter_hunk = diff_lines("one\ntwo\nthree\nfour\nfive\nsix\nseven\n", working)
            .unwrap()
            .into_iter()
            .find(|h| h.new.contains(&5))
            .expect("a gutter hunk covering the unstaged line");

        let repo = open(dir.path());
        let staged = repo
            .stage_hunk_matching(Path::new("a.txt"), working, &gutter_hunk)
            .unwrap();
        assert!(staged);

        // Both edits are now staged; the worktree matches the index.
        assert_eq!(status_porcelain(dir.path()), "M  a.txt\n");
        let indexed = String::from_utf8(
            Command::new("git")
                .args(["show", ":a.txt"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert_eq!(indexed, working);
    }

    #[test]
    fn stage_hunk_matching_is_a_noop_when_already_fully_staged() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let working = "one\nTWO\nthree\n";
        std::fs::write(dir.path().join("a.txt"), working).unwrap();
        git(dir.path(), &["add", "a.txt"]);

        let gutter_hunk = diff_lines("one\ntwo\nthree\n", working).unwrap().remove(0);

        let repo = open(dir.path());
        let staged = repo
            .stage_hunk_matching(Path::new("a.txt"), working, &gutter_hunk)
            .unwrap();
        assert!(!staged);
    }

    #[test]
    fn unstage_hunk_matching_reverses_only_the_matching_staged_hunk() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let working = "one\nTWO\nthree\n";
        std::fs::write(dir.path().join("a.txt"), working).unwrap();
        git(dir.path(), &["add", "a.txt"]);

        let gutter_hunk = diff_lines("one\ntwo\nthree\n", working).unwrap().remove(0);

        let repo = open(dir.path());
        let unstaged = repo
            .unstage_hunk_matching(Path::new("a.txt"), &gutter_hunk)
            .unwrap();
        assert!(unstaged);
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
