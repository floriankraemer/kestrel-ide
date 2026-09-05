//! Blame: `git blame --porcelain`, parsed into per-line attribution
//! (F3-10). Off the hot path and cached — nothing in this crate requests it
//! on a keystroke.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::cli;
use crate::error::VcsError;
use crate::history::head_key;
use crate::repo::Repository;

/// One line's attribution, as `git blame --porcelain` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// 1-based line number in the file's current (`HEAD`) revision.
    pub line: usize,
    pub commit: String,
    pub author_name: String,
    pub author_email: String,
    /// The commit's message summary (first line).
    pub summary: String,
    pub content: String,
}

impl Repository {
    /// `git blame --porcelain -- <path>`, parsed. Shells out rather than
    /// using `gix`'s own blame: `gix`'s is young, blame is not on the hot
    /// path here, and `git`'s rename-following is better than a
    /// reimplementation would be — the same reasoning ADR-0031 gives for
    /// this crate's write operations, applied to a read for a different
    /// reason (maturity, not credentials).
    pub fn blame(&self, relative_path: &Path) -> Result<Vec<BlameLine>, VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        let path = relative_path.to_str().ok_or_else(|| {
            VcsError::Read(format!("{} is not valid UTF-8", relative_path.display()))
        })?;
        let output = cli::run(&work_dir, &["blame", "--porcelain", "--", path])?;
        Ok(parse_porcelain(&output))
    }
}

/// Parse `git blame --porcelain`'s output.
///
/// Format, per line of the blamed file: a header
/// `<40-or-64-hex-oid> <orig-line> <final-line> [<group-len>]`, then either
/// a full metadata block (`author`, `author-mail`, `author-time`,
/// `author-tz`, `committer*`, `summary`, optionally `previous`/`boundary`/
/// `filename`) the **first** time a commit is seen in this run, or nothing
/// at all for a later line from the same commit — followed in both cases
/// by exactly one content line, always prefixed with a tab (even when the
/// source line is empty, in which case the tab is followed by nothing).
/// This is why the metadata for a commit is cached by oid as it is seen:
/// the second and later lines from that commit carry only the header and
/// the tab-content line.
pub fn parse_porcelain(output: &str) -> Vec<BlameLine> {
    let mut seen: HashMap<String, (String, String, String)> = HashMap::new();
    let mut result = Vec::new();
    let mut lines = output.lines().peekable();

    while let Some(header) = lines.next() {
        let mut parts = header.split_whitespace();
        let (Some(oid), Some(_orig_line), Some(final_line)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue; // not a header line this parser recognizes; skip.
        };
        let Ok(final_line) = final_line.parse::<usize>() else {
            continue;
        };

        let mut author_name = String::new();
        let mut author_email = String::new();
        let mut summary = String::new();
        if let Some((name, email, sum)) = seen.get(oid) {
            author_name = name.clone();
            author_email = email.clone();
            summary = sum.clone();
        }

        let content = loop {
            let Some(&next) = lines.peek() else {
                break String::new(); // truncated input; best effort.
            };
            if let Some(text) = next.strip_prefix('\t') {
                lines.next();
                break text.to_string();
            }
            let next = lines.next().expect("peeked Some above");
            if let Some(rest) = next.strip_prefix("author ") {
                author_name = rest.to_string();
            } else if let Some(rest) = next.strip_prefix("author-mail ") {
                author_email = rest.trim_matches(['<', '>']).to_string();
            } else if let Some(rest) = next.strip_prefix("summary ") {
                summary = rest.to_string();
            }
            // committer*, *-time, *-tz, boundary, previous, filename: not
            // needed by BlameLine today, deliberately ignored.
        };

        seen.insert(
            oid.to_string(),
            (author_name.clone(), author_email.clone(), summary.clone()),
        );
        result.push(BlameLine {
            line: final_line,
            commit: oid.to_string(),
            author_name,
            author_email,
            summary,
            content,
        });
    }

    result
}

/// Caches [`Repository::blame`] per `(path, head_oid, file stamp)`.
///
/// Nearly [`crate::history::HistoryCache`]'s shape, and for the same
/// reason — a blame view is opened on demand, so this exists to make
/// re-opening it cheap — with one difference that is not cosmetic.
/// A file's history cannot change without `HEAD` moving, but
/// [`Repository::blame`] blames the file *in the working tree*, whose
/// uncommitted lines `git` attributes to the all-zero "not committed yet"
/// commit. Keying on `HEAD` alone therefore served a saved-over-again file
/// its pre-edit blame until the next commit. The stamp is the working
/// file's length and modification time: the same evidence `git` itself
/// uses to decide a tracked file may have changed.
#[derive(Default)]
pub struct BlameCache {
    entries: Mutex<HashMap<BlameKey, Vec<BlameLine>>>,
}

/// `(repository-relative path, HEAD, working-file stamp)`. The stamp is
/// `None` when the file cannot be stat'd, which keys every such call
/// separately rather than sharing one bucket for "unknown".
type BlameKey = (PathBuf, String, Option<(u64, Duration)>);

/// The working file's length and modification time, as far as the
/// filesystem will say. `None` rather than an error: a file that cannot be
/// stat'd is a cache miss, not a failed blame — `git` is about to give its
/// own verdict on it either way.
fn file_stamp(work_dir: &Path, relative_path: &Path) -> Option<(u64, Duration)> {
    let metadata = std::fs::metadata(work_dir.join(relative_path)).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some((metadata.len(), modified))
}

impl BlameCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blame(
        &self,
        repo: &Repository,
        relative_path: &Path,
    ) -> Result<Vec<BlameLine>, VcsError> {
        let stamp = repo
            .work_dir()
            .and_then(|work_dir| file_stamp(&work_dir, relative_path));
        let key = (relative_path.to_path_buf(), head_key(repo)?, stamp);
        if let Some(cached) = self.entries.lock().unwrap().get(&key) {
            return Ok(cached.clone());
        }
        let lines = repo.blame(relative_path)?;
        self.entries.lock().unwrap().insert(key, lines.clone());
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DiscoverResult;
    use std::process::Command;

    // -----------------------------------------------------------------
    // parse_porcelain: fixture output, no real git needed.
    // -----------------------------------------------------------------

    const TWO_COMMIT_FIXTURE: &str = "\
6500bb516812b2aba148bd482374a1d2123d7bc 1 1 1
author A
author-mail <a@b.com>
author-time 1787660081
author-tz +0000
committer A
committer-mail <a@b.com>
committer-time 1787660081
committer-tz +0000
summary first
boundary
filename f.txt
\tone
bdb73700ff95d6e5977e3b0be817afecd9323ec 2 2 1
author A
author-mail <a@b.com>
author-time 1787660081
author-tz +0000
committer A
committer-mail <a@b.com>
committer-time 1787660081
committer-tz +0000
summary second
previous 6500bb516812b2aba148bd482374a1d2123d7bc f.txt
filename f.txt
\tTWO
";

    #[test]
    fn parses_two_lines_from_two_different_commits() {
        let lines = parse_porcelain(TWO_COMMIT_FIXTURE);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line, 1);
        assert_eq!(lines[0].content, "one");
        assert_eq!(lines[0].summary, "first");
        assert_eq!(lines[0].author_email, "a@b.com");
        assert_eq!(lines[1].line, 2);
        assert_eq!(lines[1].content, "TWO");
        assert_eq!(lines[1].summary, "second");
    }

    #[test]
    fn a_repeated_commit_reuses_cached_metadata_with_no_repeated_block() {
        // Second and third lines both belong to the first commit; git
        // porcelain output only carries full metadata once per commit per
        // invocation, so lines 2 and 3 here go straight from header to the
        // tab-content line.
        let fixture = "\
6500bb516812b2aba148bd482374a1d2123d7bc 1 1 3
author A
author-mail <a@b.com>
author-time 1787660081
author-tz +0000
committer A
committer-mail <a@b.com>
committer-time 1787660081
committer-tz +0000
summary first
boundary
filename f.txt
\tone
6500bb516812b2aba148bd482374a1d2123d7bc 2 2 3
\ttwo
6500bb516812b2aba148bd482374a1d2123d7bc 3 3 3
\tthree
";
        let lines = parse_porcelain(fixture);
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert_eq!(line.author_email, "a@b.com");
            assert_eq!(line.summary, "first");
        }
        assert_eq!(
            lines.iter().map(|l| l.content.as_str()).collect::<Vec<_>>(),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn an_empty_source_line_has_empty_content_not_a_missing_line() {
        let fixture = "\
6500bb516812b2aba148bd482374a1d2123d7bc 1 1 1
author A
author-mail <a@b.com>
author-time 1787660081
author-tz +0000
committer A
committer-mail <a@b.com>
committer-time 1787660081
committer-tz +0000
summary first
boundary
filename f.txt
\t
";
        let lines = parse_porcelain(fixture);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].content, "");
    }

    // -----------------------------------------------------------------
    // Integration: real `git blame` against a scratch repo.
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
    fn an_edited_working_file_is_blamed_again_rather_than_served_stale() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("f.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "f.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let cache = BlameCache::new();
        assert_eq!(cache.blame(&repo, Path::new("f.txt")).unwrap().len(), 1);

        // `HEAD` has not moved, but the file being blamed has. Keyed on
        // `HEAD` alone this served the one-line answer for the file's
        // previous contents.
        std::fs::write(dir.path().join("f.txt"), "one\ntwo\n").unwrap();
        let blame = cache.blame(&repo, Path::new("f.txt")).unwrap();
        assert_eq!(blame.len(), 2);
        assert_eq!(blame[1].content, "two");
    }

    #[test]
    fn blame_attributes_each_line_to_the_commit_that_last_touched_it() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("f.txt"), "one\ntwo\n").unwrap();
        git(dir.path(), &["add", "f.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("f.txt"), "one\nTWO\n").unwrap();
        git(dir.path(), &["add", "f.txt"]);
        git(dir.path(), &["commit", "-m", "second"]);

        let repo = open(dir.path());
        let blame = repo.blame(Path::new("f.txt")).unwrap();
        assert_eq!(blame.len(), 2);
        assert_eq!(blame[0].summary, "first");
        assert_eq!(blame[0].content, "one");
        assert_eq!(blame[1].summary, "second");
        assert_eq!(blame[1].content, "TWO");
    }

    #[test]
    fn blame_cache_serves_a_second_call() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        std::fs::write(dir.path().join("f.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "f.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        let repo = open(dir.path());
        let cache = BlameCache::new();
        let first = cache.blame(&repo, Path::new("f.txt")).unwrap();
        let second = cache.blame(&repo, Path::new("f.txt")).unwrap();
        assert_eq!(first, second);
    }
}
