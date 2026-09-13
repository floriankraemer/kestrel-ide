//! Working-tree hunks against `HEAD`, and the cache that keeps a gutter
//! diff off the disk and off `gix` on every keystroke (F3-4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use editor_core::diff::{self, DiffError, Hunk};

use crate::error::VcsError;
use crate::repo::Repository;

impl Repository {
    /// The `HEAD` blob for a repository-relative path, decoded as UTF-8, and
    /// its object id — or `None` if the path does not exist in `HEAD` (a new
    /// file, or a repository with no commits yet, which
    /// [`gix::Repository::head_tree_id_or_empty`] treats as an empty tree).
    ///
    /// A blob that is not valid UTF-8 is reported as [`VcsError::Read`]
    /// rather than lossily decoded — a diff against a mangled binary-as-text
    /// blob is worse than an explicit "can't diff this" the caller can
    /// distinguish from "no changes".
    pub fn head_blob(&self, relative_path: &Path) -> Result<Option<(String, String)>, VcsError> {
        let tree_id = self
            .inner
            .head_tree_id_or_empty()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let tree = self
            .inner
            .find_tree(tree_id)
            .map_err(|e| VcsError::Read(e.to_string()))?;
        Self::blob_in_tree(&tree, relative_path)
    }

    /// A blob at an arbitrary revision — a commit id, a tag, a branch name,
    /// or anything else `git rev-parse` understands — used by "compare with
    /// revision" (File History), where [`head_blob`](Self::head_blob) only
    /// ever answers for `HEAD`.
    ///
    /// `None` covers both "the revision doesn't exist" being surfaced by the
    /// caller some other way (they picked it from a real log entry) and the
    /// ordinary "this path didn't exist yet at that revision" case
    /// [`head_blob`](Self::head_blob) already treats the same way.
    pub fn blob_at(
        &self,
        revision: &str,
        relative_path: &Path,
    ) -> Result<Option<String>, VcsError> {
        let tree = self
            .inner
            .rev_parse_single(revision)
            .map_err(|e| VcsError::Read(e.to_string()))?
            .object()
            .map_err(|e| VcsError::Read(e.to_string()))?
            .peel_to_tree()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        Ok(Self::blob_in_tree(&tree, relative_path)?.map(|(_, text)| text))
    }

    fn blob_in_tree(
        tree: &gix::Tree<'_>,
        relative_path: &Path,
    ) -> Result<Option<(String, String)>, VcsError> {
        let Some(entry) = tree
            .lookup_entry_by_path(relative_path)
            .map_err(|e| VcsError::Read(e.to_string()))?
        else {
            return Ok(None);
        };
        let mut object = entry.object().map_err(|e| VcsError::Read(e.to_string()))?;
        let oid = object.id.to_hex().to_string();
        let text = String::from_utf8(std::mem::take(&mut object.data)).map_err(|_| {
            VcsError::Read(format!("{} is not valid UTF-8", relative_path.display()))
        })?;
        Ok(Some((oid, text)))
    }

    /// The git index's blob for a repository-relative path, decoded as
    /// UTF-8, and its object id — or `None` when the index has no entry for
    /// it (untracked, or removed from the index without a commit).
    ///
    /// This is the staging counterpart to [`head_blob`](Self::head_blob):
    /// per-hunk staging (§ [`hunks_against_index`](Self::hunks_against_index))
    /// must diff against what is *already staged*, not `HEAD`, or a second
    /// hunk staged in the same file re-stages the first one's lines too
    /// (the bug `docs/architecture/intellij-parity-refinement-plan.md`'s R6
    /// names: "stages the whole file" once part of it is already staged).
    pub fn index_blob(&self, relative_path: &Path) -> Result<Option<(String, String)>, VcsError> {
        use gix::bstr::ByteSlice;
        let index = self
            .inner
            .index_or_empty()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let path = crate::staging::path_str(relative_path)?;
        let Some(entry) = index.entry_by_path(path.as_bytes().as_bstr()) else {
            return Ok(None);
        };
        let oid = entry.id;
        let mut object = self
            .inner
            .find_object(oid)
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let text = String::from_utf8(std::mem::take(&mut object.data)).map_err(|_| {
            VcsError::Read(format!("{} is not valid UTF-8", relative_path.display()))
        })?;
        Ok(Some((oid.to_hex().to_string(), text)))
    }

    /// Hunks between the git index's copy of `path` and `working_text` —
    /// the diff per-hunk staging must act against, as opposed to
    /// [`HunkCache::hunks`]'s `HEAD`-vs-worktree diff, which is what the
    /// gutter paints (IDEA's three-state colouring: a hunk can be
    /// unstaged-only, staged-only, or both, and only the `HEAD`-vs-worktree
    /// diff shows the whole picture).
    ///
    /// Not cached like [`HunkCache`]: staging is a user-initiated action,
    /// not a per-keystroke read, so there is no settle-tick cost to avoid
    /// here the way there is for the gutter.
    pub fn hunks_against_index(
        &self,
        relative_path: &Path,
        working_text: &str,
    ) -> Result<WorkingHunks, VcsError> {
        let index = self.index_blob(relative_path)?;
        let (index_oid, before) = match index {
            Some((oid, text)) => (oid, text),
            None => (NO_INDEX_BLOB.to_string(), String::new()),
        };
        let hunks = diff::diff_lines(&before, working_text).map_err(|e| match e {
            DiffError::TooLarge => VcsError::TooLargeToDiff,
        })?;
        Ok(WorkingHunks {
            head_oid: index_oid,
            before_text: before,
            hunks,
        })
    }

    /// Hunks between `HEAD`'s copy of `path` and the index's copy — what is
    /// already staged, independent of any further, still-unstaged edit in
    /// the working tree. The unstage counterpart to
    /// [`hunks_against_index`](Self::hunks_against_index): unstaging a hunk
    /// removes it from the index back toward `HEAD`, so the patch it needs
    /// is built from this pair, not from the working tree at all.
    pub fn staged_hunks(&self, relative_path: &Path) -> Result<StagedHunks, VcsError> {
        let head_text = self
            .head_blob(relative_path)?
            .map_or_else(String::new, |(_, t)| t);
        let index_text = self
            .index_blob(relative_path)?
            .map_or_else(String::new, |(_, t)| t);
        let hunks = diff::diff_lines(&head_text, &index_text).map_err(|e| match e {
            DiffError::TooLarge => VcsError::TooLargeToDiff,
        })?;
        Ok(StagedHunks {
            head_text,
            index_text,
            hunks,
        })
    }
}

/// The blob id used as the cache key (and returned to the caller) when the
/// index has no entry for the file at all — the staging counterpart to
/// [`NO_HEAD_BLOB`], same sentinel value since both mean "diff against
/// nothing".
pub const NO_INDEX_BLOB: &str = NO_HEAD_BLOB;

/// `HEAD`-vs-index hunks for one file, with both texts they were computed
/// from — [`Repository::unstage_hunk`] needs both ends of the patch, unlike
/// [`WorkingHunks`], which only ever needs the `HEAD`/index side since the
/// working text is already the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedHunks {
    pub head_text: String,
    pub index_text: String,
    pub hunks: Vec<Hunk>,
}

/// Whether a `HEAD`-vs-worktree hunk (the gutter's own diff) is not staged
/// at all, fully staged, or partially staged — IDEA's three-state hunk
/// colouring, and what "Stage Hunk"/"Unstage Hunk" must reconcile against
/// the index rather than assume from the gutter's own `HEAD`-vs-worktree
/// view alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkStageState {
    /// No overlap with any `HEAD`-vs-index hunk: none of this change is in
    /// the index yet.
    Unstaged,
    /// An exact match with a `HEAD`-vs-index hunk: this whole change is
    /// already staged.
    Staged,
    /// Overlaps a `HEAD`-vs-index hunk without matching it exactly: the
    /// file was edited again after a partial `git add`, so part of what the
    /// gutter shows is staged and part is not.
    Both,
}

/// Classify one gutter hunk against the file's `HEAD`-vs-index hunks (see
/// [`Repository::staged_hunks`]), comparing `HEAD`-side line ranges — the
/// one coordinate system a `HEAD`-vs-worktree hunk and a `HEAD`-vs-index
/// hunk share.
///
/// Ceiling: two hunks that touch the same `HEAD`-side lines but were staged
/// and re-edited into different line *counts* can still read as an exact
/// range match here even though their content no longer agrees line for
/// line — a full three-way reconciliation would need to diff `HEAD`, the
/// index and the working tree against each other, not just compare ranges.
/// ponytail: acceptable for the common case (one hunk edited then either
/// staged in full or not at all); revisit if partial-restage on an
/// already-partially-staged hunk is reported as showing the wrong state.
pub fn classify_hunk(gutter_hunk: &Hunk, staged_hunks: &[Hunk]) -> HunkStageState {
    let exact = staged_hunks
        .iter()
        .any(|h| h.old == gutter_hunk.old && h.new.len() == gutter_hunk.new.len());
    if exact {
        return HunkStageState::Staged;
    }
    if staged_hunks
        .iter()
        .any(|h| ranges_overlap(&h.old, &gutter_hunk.old))
    {
        HunkStageState::Both
    } else {
        HunkStageState::Unstaged
    }
}

pub(crate) fn ranges_overlap(a: &std::ops::Range<usize>, b: &std::ops::Range<usize>) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// Working-tree hunks for one file: `HEAD`'s blob id (`"none"` for a file
/// `HEAD` does not have), that blob's text, and the line hunks between it
/// and the working text handed in.
///
/// The text is carried out rather than left for the caller to read again:
/// the gutter's caller needs it for the diff popup, and asking for it
/// separately meant walking the `HEAD` tree and inflating the blob twice per
/// settle tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingHunks {
    pub head_oid: String,
    pub before_text: String,
    pub hunks: Vec<Hunk>,
}

/// The blob id used as the cache key (and returned to the caller) when
/// `HEAD` has no version of the file at all.
pub const NO_HEAD_BLOB: &str = "none";

struct CacheEntry {
    head_oid: String,
    revision: u64,
    hunks: Vec<Hunk>,
}

/// Caches [`WorkingHunks`] per path, invalidated by `(head_oid, revision)`.
///
/// `revision` is the caller's: this crate has no idea about the live buffer
/// (`QTextDocument` on the other side of the seam), so the caller supplies
/// whatever it can cheaply bump on every edit — a monotonic counter is
/// enough, since the cache only needs to tell "same edit state" from "not",
/// never to reconstruct history.
#[derive(Default)]
pub struct HunkCache {
    entries: Mutex<HashMap<PathBuf, CacheEntry>>,
}

impl HunkCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hunks between `HEAD`'s copy of `path` and `working_text`, from cache
    /// when `head_oid`/`revision` still match what produced the cached
    /// value, computed and stored otherwise.
    ///
    /// Errs [`VcsError::TooLargeToDiff`] rather than serving or caching a
    /// diff for a file past `editor_core::diff::MAX_DIFF_BYTES` — a gutter
    /// with no markers on a huge file reads as "nothing changed", which is
    /// exactly the failure mode `editor_core::diff` was built to refuse.
    pub fn hunks(
        &self,
        repo: &Repository,
        path: &Path,
        working_text: &str,
        revision: u64,
    ) -> Result<WorkingHunks, VcsError> {
        let head = repo.head_blob(path)?;
        let (head_oid, before) = match head {
            Some((oid, text)) => (oid, text),
            None => (NO_HEAD_BLOB.to_string(), String::new()),
        };

        {
            let cache = self.entries.lock().unwrap();
            if let Some(entry) = cache.get(path) {
                if entry.head_oid == head_oid && entry.revision == revision {
                    return Ok(WorkingHunks {
                        head_oid,
                        before_text: before,
                        hunks: entry.hunks.clone(),
                    });
                }
            }
        }

        let hunks = diff::diff_lines(&before, working_text).map_err(|e| match e {
            DiffError::TooLarge => VcsError::TooLargeToDiff,
        })?;

        self.entries.lock().unwrap().insert(
            path.to_path_buf(),
            CacheEntry {
                head_oid: head_oid.clone(),
                revision,
                hunks: hunks.clone(),
            },
        );

        Ok(WorkingHunks {
            head_oid,
            before_text: before,
            hunks,
        })
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

    #[test]
    fn a_file_with_no_head_version_diffs_against_empty() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        let repo = open(dir.path());
        let cache = HunkCache::new();
        let result = cache
            .hunks(&repo, Path::new("new.txt"), "one\ntwo\n", 0)
            .unwrap();
        assert_eq!(result.head_oid, NO_HEAD_BLOB);
        assert_eq!(result.hunks.len(), 1);
    }

    #[test]
    fn hunks_reflect_a_modification_against_head() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let cache = HunkCache::new();
        let result = cache
            .hunks(&repo, Path::new("a.txt"), "one\nTWO\n", 0)
            .unwrap();
        assert_ne!(result.head_oid, NO_HEAD_BLOB);
        assert_eq!(result.hunks.len(), 1);
    }

    #[test]
    fn the_same_revision_is_served_from_the_cache_rather_than_rediffed() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let cache = HunkCache::new();
        let first = cache
            .hunks(&repo, Path::new("a.txt"), "one\nTWO\n", 7)
            .unwrap();
        // Same revision, different text: a cache that is actually consulted
        // answers from the key it was given. Asking with text the caller
        // says belongs to a revision already answered for is the only way
        // to observe a hit from outside, and it is exactly the case the
        // view hits on a tab switch or a settle tick after typing stops —
        // which a per-call counter made unreachable.
        let second = cache
            .hunks(&repo, Path::new("a.txt"), "something else entirely\n", 7)
            .unwrap();
        assert_eq!(first.hunks, second.hunks);
    }

    #[test]
    fn blob_at_reads_an_older_revision_by_commit_id() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        let first_commit = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);

        let repo = open(dir.path());
        assert_eq!(
            repo.blob_at(&first_commit, Path::new("a.txt")).unwrap(),
            Some("one\n".to_string())
        );
        assert_eq!(
            repo.blob_at("HEAD", Path::new("a.txt")).unwrap(),
            Some("one\ntwo\n".to_string())
        );
    }

    #[test]
    fn blob_at_a_path_absent_from_that_revision_is_none() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        assert_eq!(
            repo.blob_at("HEAD", Path::new("missing.txt")).unwrap(),
            None
        );
    }

    #[test]
    fn a_stale_revision_is_recomputed_not_served_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let cache = HunkCache::new();
        let first = cache
            .hunks(&repo, Path::new("a.txt"), "one\ntwo\n", 0)
            .unwrap();
        assert_eq!(first.hunks.len(), 1);

        // Same path, bumped revision, different working text: must not
        // reuse the stale cached hunks from revision 0.
        let second = cache
            .hunks(&repo, Path::new("a.txt"), "one\ntwo\nthree\n", 1)
            .unwrap();
        assert_eq!(second.hunks.len(), 1);
        assert_eq!(second.hunks[0].new, 1..3);
    }

    #[test]
    fn a_repeated_call_with_the_same_revision_is_served_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let cache = HunkCache::new();
        let first = cache
            .hunks(&repo, Path::new("a.txt"), "one\ntwo\n", 7)
            .unwrap();
        // Pass working text that would diff differently, but keep the same
        // revision: the cached value from revision 7 must still come back,
        // proving the cache — not just a correct recompute — is exercised.
        let second = cache
            .hunks(&repo, Path::new("a.txt"), "completely different", 7)
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn a_file_past_the_diff_ceiling_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        let repo = open(dir.path());
        let cache = HunkCache::new();
        let huge = "x\n".repeat(diff::MAX_DIFF_BYTES);
        let err = cache
            .hunks(&repo, Path::new("huge.txt"), &huge, 0)
            .unwrap_err();
        assert!(matches!(err, VcsError::TooLargeToDiff));
    }

    #[test]
    fn hunks_against_index_diffs_the_staged_copy_not_head() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        // Stage a first edit, then make a second, still-unstaged edit.
        std::fs::write(dir.path().join("a.txt"), "ONE\ntwo\nthree\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        let working = "ONE\ntwo\nTHREE\n";
        std::fs::write(dir.path().join("a.txt"), working).unwrap();

        let repo = open(dir.path());
        let result = repo
            .hunks_against_index(Path::new("a.txt"), working)
            .unwrap();

        // Against HEAD there would be two hunks (line 1 and line 3);
        // against the index there is exactly the one still-unstaged edit.
        assert_eq!(result.hunks.len(), 1);
        assert_eq!(result.hunks[0].new, 2..3);
    }

    #[test]
    fn index_blob_is_none_for_an_untracked_file() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        let repo = open(dir.path());
        assert_eq!(repo.index_blob(Path::new("new.txt")).unwrap(), None);
    }

    // -----------------------------------------------------------------
    // Three-state hunk colouring: classify_hunk.
    // -----------------------------------------------------------------

    #[test]
    fn a_gutter_hunk_with_no_staged_overlap_is_unstaged() {
        let gutter = diff::diff_lines("one\ntwo\nthree\n", "one\nTWO\nthree\n").unwrap();
        let state = classify_hunk(&gutter[0], &[]);
        assert_eq!(state, HunkStageState::Unstaged);
    }

    #[test]
    fn a_gutter_hunk_matching_a_staged_hunk_exactly_is_staged() {
        let gutter = diff::diff_lines("one\ntwo\nthree\n", "one\nTWO\nthree\n").unwrap();
        let staged = diff::diff_lines("one\ntwo\nthree\n", "one\nTWO\nthree\n").unwrap();
        let state = classify_hunk(&gutter[0], &staged);
        assert_eq!(state, HunkStageState::Staged);
    }

    #[test]
    fn a_gutter_hunk_overlapping_but_not_matching_a_staged_hunk_is_both() {
        // Staged: line 2 became "TWO". Working tree since then: line 2 is
        // now "TWO-EDITED" and line 3 changed too — one gutter hunk spans
        // both, only part of which is reflected in the index.
        let gutter = diff::diff_lines("one\ntwo\nthree\n", "one\nTWO-EDITED\nTHREE\n").unwrap();
        let staged = diff::diff_lines("one\ntwo\nthree\n", "one\nTWO\nthree\n").unwrap();
        let state = classify_hunk(&gutter[0], &staged);
        assert_eq!(state, HunkStageState::Both);
    }

    #[test]
    fn staged_hunks_reads_head_and_index_not_the_working_tree() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        std::fs::write(dir.path().join("a.txt"), "ONE\ntwo\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        // A further, unstaged edit — staged_hunks must ignore it.
        std::fs::write(dir.path().join("a.txt"), "ONE\nTWO\n").unwrap();

        let repo = open(dir.path());
        let result = repo.staged_hunks(Path::new("a.txt")).unwrap();
        assert_eq!(result.hunks.len(), 1);
        assert_eq!(result.index_text, "ONE\ntwo\n");
    }
}
