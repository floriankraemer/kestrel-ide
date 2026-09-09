//! Repository discovery and reads (F3-2, F3-3).

use std::path::{Path, PathBuf};

use crate::error::VcsError;

/// A discovered, opened repository. Wraps `gix::Repository` rather than
/// re-exporting it, so a future `gix` major-version bump (or a swap to a
/// different backend) does not ripple past this crate's boundary — the same
/// reason `settings-model` wraps rather than leaks its dependents' types.
pub struct Repository {
    pub(crate) inner: gix::Repository,
}

/// The outcome of looking for a repository at or above a path.
///
/// Opening a plain folder with no `.git` is an entirely ordinary outcome —
/// most folders are not repositories — so it is a variant of a successful
/// result, not an `Err`. A caller that only cares whether Git features
/// should be offered at all can match this without touching `VcsError`.
pub enum DiscoverResult {
    Found(Box<Repository>),
    NotARepository,
}

/// How much decoded-object cache each [`Repository`] handle gets.
///
/// One repository is open at a time (the worker owns it), so this is a
/// per-process cost, not a per-file one. 16 MiB is enough to hold the trees
/// an ancestry walk revisits without being a number anyone has to think
/// about on a developer machine.
const OBJECT_CACHE_BYTES: usize = 16 * 1024 * 1024;

impl Repository {
    /// `git init` in `path`, then open what it just created.
    ///
    /// Shells out rather than using `gix::init`: this is a one-off, rare
    /// write a user explicitly asked for (the Changes dock's "Initialize
    /// Git Repository" button), not a hot path, so it goes through the same
    /// `git` binary the rest of this crate's writes do (ADR-0031) rather
    /// than adding a second code path for repository creation.
    pub fn init(path: &Path) -> Result<Repository, VcsError> {
        crate::cli::run(path, &["init"])?;
        match Self::discover(path)? {
            DiscoverResult::Found(repo) => Ok(*repo),
            DiscoverResult::NotARepository => Err(VcsError::Discover(format!(
                "`git init` succeeded but no repository was found at {}",
                path.display()
            ))),
        }
    }

    /// Walk upward from `path` looking for a `.git`. Returns
    /// [`DiscoverResult::NotARepository`], not an error, when none is found
    /// before the filesystem root or a discovery ceiling.
    pub fn discover(path: impl AsRef<Path>) -> Result<DiscoverResult, VcsError> {
        use gix::discover::upwards::Error as UpwardsError;
        use gix::discover::Error as DiscoverError;

        match gix::discover(path.as_ref()) {
            Ok(mut inner) => {
                // A `gix::Repository` starts with its decoded-object cache
                // switched off, and clones itself empty, so the budget has
                // to be set per handle — gix's own docs put the difference
                // at "2x or more" for workloads that re-read objects, which
                // is every workload in this crate: `file_history` walks an
                // ancestry re-reading the same trees, and the gutter reads
                // the same `HEAD` tree on every settle tick.
                inner.object_cache_size(OBJECT_CACHE_BYTES);
                Ok(DiscoverResult::Found(Box::new(Repository { inner })))
            }
            Err(DiscoverError::Discover(
                UpwardsError::NoGitRepository { .. }
                | UpwardsError::NoGitRepositoryWithinCeiling { .. }
                | UpwardsError::NoGitRepositoryWithinFs { .. },
            )) => Ok(DiscoverResult::NotARepository),
            Err(err) => Err(VcsError::Discover(err.to_string())),
        }
    }

    /// The repository's working tree root, if it is not bare.
    pub fn work_dir(&self) -> Option<PathBuf> {
        self.inner.workdir().map(Path::to_path_buf)
    }

    /// The `.git` directory itself.
    pub fn git_dir(&self) -> PathBuf {
        self.inner.git_dir().to_path_buf()
    }

    /// What `HEAD` currently points at.
    ///
    /// `vcs-core`'s own type, not `gix::head::Kind` — wrapping keeps a
    /// future `gix` upgrade from rippling past this crate's boundary, and it
    /// is not `gix::Repository::status`'s own error, so a bare `String` here
    /// is not disguising a `Result` a caller should have branched on.
    pub fn head(&self) -> Result<HeadInfo, VcsError> {
        let head = self
            .inner
            .head()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        Ok(match head.kind {
            gix::head::Kind::Symbolic(reference) => {
                HeadInfo::Branch(reference.name.shorten().to_string())
            }
            gix::head::Kind::Unborn(full_name) => HeadInfo::Unborn(full_name.shorten().to_string()),
            gix::head::Kind::Detached { target, .. } => {
                HeadInfo::Detached(target.to_hex().to_string())
            }
        })
    }

    /// Changed paths: branch/upstream/ahead/behind, staged (`HEAD` vs
    /// index), unstaged (index vs working tree), renamed, conflicted and
    /// untracked — via `git status --porcelain=v2`, not `gix`.
    ///
    /// Shells out rather than walking `gix::Repository::status` in-process:
    /// status is configuration-dependent (`.gitattributes`, `core.autocrlf`,
    /// `core.excludesFile`, sparse-checkout, submodules) in exactly the way
    /// ADR-0031 reserves for the `git` binary, and on a
    /// `\\wsl.localhost\...` project it makes the *distro's* git compute
    /// status against the *distro's* index — the same git that performs the
    /// writes, which the previous `gix` walk (running Windows-side over the
    /// 9P share) did not.
    pub fn status(&self) -> Result<RepoStatus, VcsError> {
        let work_dir = self.work_dir_or_err()?;
        let output = crate::cli::run(&work_dir, &crate::cli::argv::status())?;
        Ok(crate::status::parse_porcelain_v2(&output))
    }
}

/// What `HEAD` points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadInfo {
    /// On a branch, by its short name (`main`, not `refs/heads/main`).
    Branch(String),
    /// Detached, at this commit (hex object id).
    Detached(String),
    /// A fresh repository before the first commit: `HEAD` names a branch
    /// that does not exist yet.
    Unborn(String),
}

/// What kind of change a path has, staged or unstaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    TypeChanged,
    Renamed,
    Copied,
    /// A merge/rebase conflict — `git status --porcelain=v2`'s `u` entries.
    /// Never paired with a staged state: a conflicted path is not a thing
    /// the Changes panel offers to stage (plan doc, "a conflicted row has
    /// no checkbox").
    Conflicted,
    /// Not in the index at all. Folded into [`FileStatus::unstaged`] rather
    /// than kept as `RepoStatus`'s own separate list — see that struct's
    /// doc comment.
    Untracked,
}

/// One changed path's staged and/or unstaged state, plus — for a rename or
/// copy — the path it was renamed/copied from. A path can be both staged
/// and unstaged (staged one edit, then edited again), which is exactly why
/// `staged`/`unstaged` are two `Option`s rather than one flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: PathBuf,
    pub staged: Option<ChangeKind>,
    pub unstaged: Option<ChangeKind>,
    /// The path this entry was renamed or copied from. `Some` only when
    /// `staged` or `unstaged` is [`ChangeKind::Renamed`]/[`ChangeKind::Copied`].
    pub orig_path: Option<PathBuf>,
}

/// The repository's current picture: branch/upstream/ahead-behind plus
/// every changed path, staged, unstaged, renamed, conflicted or untracked.
///
/// Untracked paths are entries in `files` with `unstaged: Some(Untracked)`
/// rather than a separate list — `git status --porcelain=v2` already
/// reports them as one stream of paths, and every caller of the previous
/// two-list shape (`crates/ui-shell/src/bridge/vcs/mod.rs`) immediately
/// merged them back into one anyway.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoStatus {
    pub files: Vec<FileStatus>,
    /// The current branch's short name, or `"(detached)"` when `HEAD` does
    /// not point at a branch. `None` only for a repository status could not
    /// determine a branch header for at all (unborn `HEAD` on some git
    /// versions) — callers that need to tell that apart from `(detached)`
    /// compare the string, not this being `None`.
    pub branch: Option<String>,
    /// The upstream's name (`origin/main`), `None` when the branch has none.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(dir: &Path) {
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir)
            .status()
            .expect("git must be on PATH for these tests");
        assert!(status.success());
    }

    #[test]
    fn a_plain_folder_is_not_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let result = Repository::discover(dir.path()).unwrap();
        assert!(matches!(result, DiscoverResult::NotARepository));
    }

    #[test]
    fn init_creates_a_repository_discover_then_finds() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        assert_eq!(
            repo.work_dir().unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
        let result = Repository::discover(dir.path()).unwrap();
        assert!(matches!(result, DiscoverResult::Found(_)));
    }

    #[test]
    fn a_git_init_ed_folder_is_found() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let result = Repository::discover(dir.path()).unwrap();
        assert!(matches!(result, DiscoverResult::Found(_)));
    }

    #[test]
    fn discovery_walks_upward_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let sub = dir.path().join("a/b/c");
        std::fs::create_dir_all(&sub).unwrap();
        let result = Repository::discover(&sub).unwrap();
        assert!(matches!(result, DiscoverResult::Found(_)));
    }

    #[test]
    fn work_dir_is_the_repository_root() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let Ok(DiscoverResult::Found(repo)) = Repository::discover(dir.path()) else {
            panic!("expected a repository");
        };
        // Canonicalize both sides: on macOS `TMPDIR` is under a symlink
        // (`/tmp` -> `/private/tmp`), and `gix` resolves it.
        assert_eq!(
            repo.work_dir().unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .current_dir(dir)
            .status()
            .expect("git must be on PATH for these tests");
        assert!(status.success(), "git {args:?} failed");
    }

    fn open(dir: &Path) -> Repository {
        match Repository::discover(dir).unwrap() {
            DiscoverResult::Found(repo) => *repo,
            DiscoverResult::NotARepository => panic!("expected a repository"),
        }
    }

    #[test]
    fn head_on_a_fresh_repository_is_unborn() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let repo = open(dir.path());
        assert!(matches!(repo.head().unwrap(), HeadInfo::Unborn(_)));
    }

    #[test]
    fn head_after_a_commit_names_the_branch() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        git(dir.path(), &["branch", "-M", "main"]);
        let repo = open(dir.path());
        assert_eq!(repo.head().unwrap(), HeadInfo::Branch("main".into()));
    }

    #[test]
    fn head_detached_at_a_commit() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        git(dir.path(), &["checkout", "--detach", "--quiet", "HEAD"]);
        let repo = open(dir.path());
        assert!(matches!(repo.head().unwrap(), HeadInfo::Detached(_)));
    }

    #[test]
    fn status_on_a_clean_repository_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert!(status.files.is_empty());
    }

    #[test]
    fn status_reports_an_untracked_file() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, PathBuf::from("new.txt"));
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Untracked));
    }

    #[test]
    fn status_reports_an_unstaged_modification() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, PathBuf::from("a.txt"));
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Modified));
        assert_eq!(status.files[0].staged, None);
    }

    #[test]
    fn status_reports_a_staged_addition() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, PathBuf::from("b.txt"));
        assert_eq!(status.files[0].staged, Some(ChangeKind::Added));
    }

    #[test]
    fn status_reports_a_staged_and_unstaged_combination_on_one_path() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        // Stage one edit, then edit again without staging the second one.
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        std::fs::write(dir.path().join("a.txt"), "three\n").unwrap();
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, PathBuf::from("a.txt"));
        assert_eq!(status.files[0].staged, Some(ChangeKind::Modified));
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Modified));
    }

    #[test]
    fn status_reports_a_deletion() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::remove_file(dir.path().join("a.txt")).unwrap();
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, PathBuf::from("a.txt"));
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Deleted));
    }

    #[test]
    fn status_reports_a_staged_rename_with_its_original_path() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        // Rename detection needs enough shared content between old and new
        // to score above git's similarity threshold.
        let body = "one\ntwo\nthree\nfour\nfive\n".repeat(4);
        std::fs::write(dir.path().join("old.txt"), &body).unwrap();
        git(dir.path(), &["add", "old.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        git(dir.path(), &["add", "-A"]);
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.files.len(), 1);
        let file = &status.files[0];
        assert_eq!(file.path, PathBuf::from("new.txt"));
        assert_eq!(file.staged, Some(ChangeKind::Renamed));
        assert_eq!(file.orig_path, Some(PathBuf::from("old.txt")));
    }

    #[test]
    fn status_reports_a_merge_conflict() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "base\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "base"]);
        git(dir.path(), &["checkout", "--quiet", "-b", "side"]);
        std::fs::write(dir.path().join("a.txt"), "side\n").unwrap();
        git(dir.path(), &["commit", "-am", "side change"]);
        git(dir.path(), &["checkout", "--quiet", "-"]);
        std::fs::write(dir.path().join("a.txt"), "main\n").unwrap();
        git(dir.path(), &["commit", "-am", "main change"]);
        // Merge is expected to conflict; ignore its non-zero exit. `git
        // merge` checks committer identity up front — before it even knows
        // whether it will conflict, since a clean merge needs a commit —
        // so this needs the same author/committer env the `git()` helper
        // sets, or it fails on identity before ever attempting the merge.
        let _ = std::process::Command::new("git")
            .args(["merge", "side"])
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .current_dir(dir.path())
            .status();

        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, PathBuf::from("a.txt"));
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Conflicted));
        assert_eq!(status.files[0].staged, None);
    }

    #[test]
    fn status_does_not_list_an_ignored_file() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join(".gitignore"), "ignored.log\n").unwrap();
        git(dir.path(), &["add", ".gitignore"]);
        git(dir.path(), &["commit", "-m", "first"]);
        std::fs::write(dir.path().join("ignored.log"), "noise\n").unwrap();
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert!(status.files.is_empty());
    }

    #[test]
    fn status_reports_the_branch_name() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);
        git(dir.path(), &["branch", "-M", "main"]);
        let repo = open(dir.path());
        let status = repo.status().unwrap();
        assert_eq!(status.branch, Some("main".to_string()));
    }
}
