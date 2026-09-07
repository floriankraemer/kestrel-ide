//! Commit history: the full log via `gix`, and per-file history via a
//! walk-and-compare `gix` has no pathspec-filtered revwalk to do for us
//! (F3-10). Both cached, both off the hot path — a caller reaches for
//! either on demand (opening a history panel), never on a keystroke.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::VcsError;
use crate::repo::Repository;

/// One commit, as much as a history or blame view needs — not the full
/// `gix_object::Commit`, so a `gix` upgrade cannot change this crate's
/// public surface out from under a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Full hex object id.
    pub id: String,
    /// The commit message's first line.
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the Unix epoch, author time (not committer time — the
    /// one a "when was this written" view wants).
    pub author_time: i64,
}

/// One commit in full, for the commit-detail dock — everything [`LogEntry`]
/// carries plus the full message body, the committer identity (which can
/// differ from the author, e.g. after a rebase), and parent ids (empty for
/// a root commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub id: String,
    pub summary: String,
    /// The message with the summary line and its blank-line separator
    /// stripped — empty when the commit has no body beyond the summary.
    pub body: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_time: i64,
    pub parent_ids: Vec<String>,
}

impl Repository {
    /// The commit history reachable from `HEAD`, newest first, via `gix`'s
    /// commit walk — no subprocess. `max` caps how many commits are
    /// returned (a history panel pages; nothing here needs the whole repo
    /// at once).
    pub fn log(&self, max: Option<usize>) -> Result<Vec<LogEntry>, VcsError> {
        let head_id = match self.inner.head_id() {
            Ok(id) => id,
            // Unborn HEAD: no commits exist yet. Not an error — the same
            // judgement F3-2's `DiscoverResult` made for "not a repository".
            Err(_) => return Ok(Vec::new()),
        };
        let ancestors = head_id
            .ancestors()
            // Reads parents and dates from `.git/objects/info/commit-graph`
            // where the repository has one, instead of decoding a commit
            // object per ancestor. Repositories that have never been `gc`'d
            // have no such file and simply get the old path.
            .use_commit_graph(true)
            .all()
            .map_err(|e| VcsError::Read(e.to_string()))?;

        let mut entries = Vec::new();
        for info in ancestors {
            let info = info.map_err(|e| VcsError::Read(e.to_string()))?;
            let commit = info.object().map_err(|e| VcsError::Read(e.to_string()))?;
            entries.push(log_entry(&commit)?);
            if max.is_some_and(|max| entries.len() >= max) {
                break;
            }
        }
        Ok(entries)
    }

    /// The full detail of one commit by id (or any revspec `gix` accepts),
    /// or `None` if it does not resolve — a caller reaches for this from a
    /// real log entry, so "not found" is a display edge case, not an error.
    pub fn commit_detail(&self, id: &str) -> Result<Option<CommitDetail>, VcsError> {
        let Ok(id) = self.inner.rev_parse_single(id) else {
            return Ok(None);
        };
        let Ok(object) = id.object() else {
            return Ok(None);
        };
        let Ok(commit) = object.try_into_commit() else {
            return Ok(None);
        };
        let message = commit
            .message()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let author = commit.author().map_err(|e| VcsError::Read(e.to_string()))?;
        let author_time = author.time().map_err(|e| VcsError::Read(e.to_string()))?;
        let committer = commit
            .committer()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let committer_time = committer
            .time()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        Ok(Some(CommitDetail {
            id: commit.id.to_hex().to_string(),
            summary: message.summary().to_string(),
            body: message
                .body()
                .map(|b| b.to_string())
                .unwrap_or_default(),
            author_name: author.name.to_string(),
            author_email: author.email.to_string(),
            author_time: author_time.seconds,
            committer_name: committer.name.to_string(),
            committer_email: committer.email.to_string(),
            committer_time: committer_time.seconds,
            parent_ids: commit.parent_ids().map(|id| id.to_hex().to_string()).collect(),
        }))
    }

    /// Commits that touched `relative_path`, newest first, via
    /// `git log --follow`.
    ///
    /// The one read in this crate other than blame that shells out, and for
    /// the same kind of reason ADR-0031 §4 already accepted there: `gix`
    /// 0.87 still has no path-filtered revwalk and no changed-path bloom
    /// filters (`gix-0.87.1/src/revision/walk.rs` offers `sorting`,
    /// `first_parent_only`, `use_commit_graph`, `with_boundary`,
    /// `with_hidden` and a commit-id `selected` predicate — no pathspec),
    /// so the in-process version was a walk-and-compare that decoded a
    /// commit, its tree, its first parent and *that* tree for every
    /// ancestor. Measured on a 50 000-commit repository whose target file
    /// was touched in three of them, that walk cost 2.3 s against 0.69 s
    /// for `git log`, and the gap is structural rather than a tuning
    /// problem: `max` could only ever cap the *matches*, never the walk.
    ///
    /// `--follow` also gives rename tracking the walk-and-compare
    /// explicitly did not do.
    ///
    /// An unborn `HEAD` is "no history", not an error — the same judgement
    /// [`Repository::log`] makes.
    pub fn file_history(
        &self,
        relative_path: &Path,
        max: Option<usize>,
    ) -> Result<Vec<LogEntry>, VcsError> {
        if self.inner.head_id().is_err() {
            return Ok(Vec::new());
        }
        let Some(work_dir) = self.work_dir() else {
            return Ok(Vec::new());
        };

        let path = relative_path.to_string_lossy().into_owned();
        let max = max.map(|max| max.to_string());
        let mut args = vec!["log", "--follow", LOG_FORMAT];
        if let Some(max) = max.as_deref() {
            args.push("-n");
            args.push(max);
        }
        args.push("--");
        args.push(&path);

        let output = crate::cli::run(&work_dir, &args)?;
        Ok(output.lines().filter_map(parse_log_line).collect())
    }
}

/// NUL between fields so no field can contain the separator, one commit per
/// line so the subject (which cannot contain a newline) ends the record.
const LOG_FORMAT: &str = "--format=%H%x00%an%x00%ae%x00%at%x00%s";

/// One `LOG_FORMAT` line. A line that does not have the five fields is
/// skipped rather than failing the whole history: the panel showing the
/// commits that did parse beats showing nothing.
fn parse_log_line(line: &str) -> Option<LogEntry> {
    let mut fields = line.split('\0');
    let id = fields.next()?.to_string();
    let author_name = fields.next()?.to_string();
    let author_email = fields.next()?.to_string();
    let author_time = fields.next()?.parse().ok()?;
    let summary = fields.next()?.to_string();
    Some(LogEntry {
        id,
        summary,
        author_name,
        author_email,
        author_time,
    })
}

fn log_entry(commit: &gix::Commit<'_>) -> Result<LogEntry, VcsError> {
    let message = commit
        .message()
        .map_err(|e| VcsError::Read(e.to_string()))?;
    let author = commit.author().map_err(|e| VcsError::Read(e.to_string()))?;
    let time = author.time().map_err(|e| VcsError::Read(e.to_string()))?;
    Ok(LogEntry {
        id: commit.id.to_hex().to_string(),
        summary: message.summary().to_string(),
        author_name: author.name.to_string(),
        author_email: author.email.to_string(),
        author_time: time.seconds,
    })
}

/// Caches [`Repository::log`] and [`Repository::file_history`], keyed by
/// the repository's current `HEAD` commit (and, for file history, the
/// path). A history view is opened by the user, not driven by a keystroke,
/// so this exists to make re-opening it or switching between files cheap,
/// not to survive concurrent-edit races the way [`crate::hunks::HunkCache`]
/// must.
type LogKey = (String, Option<usize>);
type FileHistoryKey = (PathBuf, String, Option<usize>);

#[derive(Default)]
pub struct HistoryCache {
    log: Mutex<HashMap<LogKey, Vec<LogEntry>>>,
    file_history: Mutex<HashMap<FileHistoryKey, Vec<LogEntry>>>,
    /// Keyed by commit id alone, never by `HEAD` — a commit's own detail
    /// never changes once it exists, unlike `log`/`file_history` which
    /// answer for "as of `HEAD`".
    commit_detail: Mutex<HashMap<String, Option<CommitDetail>>>,
}

impl HistoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn log(&self, repo: &Repository, max: Option<usize>) -> Result<Vec<LogEntry>, VcsError> {
        let key = (head_key(repo)?, max);
        if let Some(cached) = self.log.lock().unwrap().get(&key) {
            return Ok(cached.clone());
        }
        let entries = repo.log(max)?;
        self.log.lock().unwrap().insert(key, entries.clone());
        Ok(entries)
    }

    pub fn file_history(
        &self,
        repo: &Repository,
        relative_path: &Path,
        max: Option<usize>,
    ) -> Result<Vec<LogEntry>, VcsError> {
        let key = (relative_path.to_path_buf(), head_key(repo)?, max);
        if let Some(cached) = self.file_history.lock().unwrap().get(&key) {
            return Ok(cached.clone());
        }
        let entries = repo.file_history(relative_path, max)?;
        self.file_history
            .lock()
            .unwrap()
            .insert(key, entries.clone());
        Ok(entries)
    }

    pub fn commit_detail(
        &self,
        repo: &Repository,
        id: &str,
    ) -> Result<Option<CommitDetail>, VcsError> {
        if let Some(cached) = self.commit_detail.lock().unwrap().get(id) {
            return Ok(cached.clone());
        }
        let detail = repo.commit_detail(id)?;
        self.commit_detail
            .lock()
            .unwrap()
            .insert(id.to_string(), detail.clone());
        Ok(detail)
    }
}

/// The cache key for "current `HEAD`": the commit hex id, or a fixed string
/// for the unborn-HEAD case (no commit id exists yet to key on). Shared
/// with [`crate::blame::BlameCache`], which needs the identical key.
pub(crate) fn head_key(repo: &Repository) -> Result<String, VcsError> {
    match repo.inner.head_id() {
        Ok(id) => Ok(id.to_hex().to_string()),
        Err(_) => Ok("unborn".to_string()),
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
    fn log_on_an_unborn_repository_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        let repo = open(dir.path());
        assert!(repo.log(None).unwrap().is_empty());
    }

    #[test]
    fn log_lists_commits_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "second"]);

        let repo = open(dir.path());
        let log = repo.log(None).unwrap();
        assert_eq!(
            log.iter().map(|e| e.summary.as_str()).collect::<Vec<_>>(),
            vec!["second", "first"]
        );
        assert_eq!(log[0].author_email, "test@example.com");
    }

    #[test]
    fn log_respects_max() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        for n in 0..3 {
            std::fs::write(dir.path().join("a.txt"), format!("{n}\n")).unwrap();
            git(dir.path(), &["add", "a.txt"]);
            git(dir.path(), &["commit", "-m", &format!("commit {n}")]);
        }
        let repo = open(dir.path());
        assert_eq!(repo.log(Some(2)).unwrap().len(), 2);
    }

    #[test]
    fn file_history_only_lists_commits_that_touched_the_path() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "add a"]);

        std::fs::write(dir.path().join("b.txt"), "unrelated\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-m", "add b, unrelated to a"]);

        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "edit a"]);

        let repo = open(dir.path());
        let history = repo.file_history(Path::new("a.txt"), None).unwrap();
        assert_eq!(
            history
                .iter()
                .map(|e| e.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["edit a", "add a"]
        );
    }

    #[test]
    fn file_history_respects_max() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        for n in 0..3 {
            std::fs::write(dir.path().join("a.txt"), format!("{n}\n")).unwrap();
            git(dir.path(), &["add", "a.txt"]);
            git(dir.path(), &["commit", "-m", &format!("commit {n}")]);
        }
        let repo = open(dir.path());
        assert_eq!(
            repo.file_history(Path::new("a.txt"), Some(2))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn history_cache_serves_a_second_call_without_recomputing_a_changed_key() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let cache = HistoryCache::new();
        let first = cache.log(&repo, None).unwrap();
        let second = cache.log(&repo, None).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
    }

    #[test]
    fn commit_detail_reports_summary_body_and_identities() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(
            dir.path(),
            &["commit", "-m", "summary line\n\nfirst body line\nsecond body line"],
        );

        let repo = open(dir.path());
        let head = repo.log(None).unwrap()[0].id.clone();
        let detail = repo.commit_detail(&head).unwrap().unwrap();
        assert_eq!(detail.summary, "summary line");
        assert_eq!(detail.body, "first body line\nsecond body line\n");
        assert_eq!(detail.author_email, "test@example.com");
        assert_eq!(detail.committer_email, "test@example.com");
    }

    #[test]
    fn commit_detail_on_a_root_commit_has_no_parents() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "root"]);

        let repo = open(dir.path());
        let head = repo.log(None).unwrap()[0].id.clone();
        let detail = repo.commit_detail(&head).unwrap().unwrap();
        assert!(detail.parent_ids.is_empty());
    }

    #[test]
    fn commit_detail_reports_a_parent_after_a_second_commit() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        let first_id = repo_head(dir.path());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);

        let repo = open(dir.path());
        let head = repo.log(None).unwrap()[0].id.clone();
        let detail = repo.commit_detail(&head).unwrap().unwrap();
        assert_eq!(detail.parent_ids, vec![first_id]);
    }

    #[test]
    fn commit_detail_for_an_unknown_id_is_none() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        assert!(repo
            .commit_detail("0000000000000000000000000000000000000000")
            .unwrap()
            .is_none());
    }

    #[test]
    fn history_cache_serves_commit_detail_without_a_head_move_invalidating_it() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let head = repo.log(None).unwrap()[0].id.clone();
        let cache = HistoryCache::new();
        let first = cache.commit_detail(&repo, &head).unwrap();

        // Move HEAD forward: a commit-keyed cache entry must still answer
        // for the old id, unlike `log`/`file_history` which are keyed by
        // HEAD and would recompute here.
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);
        let second = cache.commit_detail(&repo, &head).unwrap();
        assert_eq!(first, second);
    }

    fn repo_head(dir: &Path) -> String {
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
}
