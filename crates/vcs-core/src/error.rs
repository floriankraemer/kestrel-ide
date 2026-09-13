//! Why a `vcs-core` operation failed.
//!
//! Mirrors `app_core::AppError`'s shape (ADR-0003): a typed variant per
//! failure kind, a stable numeric code, and a `Display` message meant to be
//! shown to the user verbatim — never a bare `QString` sentinel, and never a
//! raw process exit code reaching a future UI layer.
//!
//! This crate has no FFI seam yet (F3-12 bridges it), but the codes are laid
//! out now in the 700-799 range §5 of `next-five-features-plan.md` reserves
//! for `vcs-core`, so the future bridge does not have to renumber or
//! translate a wrong shape.

use std::fmt;
use std::io;

/// Why a `vcs-core` operation failed. Each variant carries a stable code
/// (see [`VcsError::code`]); the `Display` message is user-facing English,
/// built from the underlying cause rather than exposing it verbatim, except
/// where the cause (`git`'s own stderr, a hook's own output) *is* the
/// message the user needs to see.
#[derive(Debug)]
pub enum VcsError {
    /// `gix::discover` found something that looked like a repository but
    /// could not be opened (corrupt `.git`, permission denied, etc.).
    /// **Not** produced for an ordinary folder with no `.git` at all — see
    /// [`crate::repo::Repository::discover`], which returns `Ok(None)` for
    /// that, because "not a repository" is an expected outcome, not a
    /// failure.
    Discover(String),
    /// A `gix` read (status, HEAD resolution, ref listing, log, blob read)
    /// failed against a repository that did open.
    Read(String),
    /// The `git` binary is not on `PATH`. Kept distinct from every other
    /// `cli` failure so the caller can say exactly that, once, rather than
    /// have it read as some other kind of failure (F3-5).
    GitNotInstalled,
    /// `git` was spawned but exited non-zero; `stderr` is exactly what
    /// `git` printed. This is also what a rejected commit looks like: a
    /// pre-commit or commit-msg hook that fails prints to the same stderr
    /// `git commit` inherits and fails with, so this crate does not
    /// special-case "hook rejected the commit" as a distinct variant — the
    /// hook's own message is already carried here verbatim (F3-7), and
    /// there is no reliable, non-heuristic way to tell "a hook said no"
    /// from any other reason `git commit` can fail.
    GitFailed { command: String, stderr: String },
    /// `git` did not finish within `cli::TIMEOUT`; it has been killed.
    GitTimedOut { command: String },
    /// `git branch -d` refused to delete a branch with commits not merged
    /// anywhere else. The caller may retry as a force delete; this variant
    /// exists so that retry is a deliberate second call, never automatic
    /// (F3-8).
    UnmergedBranch { branch: String },
    /// A path given to a `vcs-core` call is not inside this repository's
    /// working tree.
    OutsideWorkingTree,
    /// A file's content (either side of a diff) is past
    /// `editor_core::diff::MAX_DIFF_BYTES`.
    TooLargeToDiff,
    /// A generated patch could not be parsed back by `git apply` — a bug in
    /// `staging::hunk_patch`, surfaced rather than silently no-opped.
    MalformedPatch(String),
    /// `git push` was rejected because the remote has commits the local
    /// branch does not (a non-fast-forward push). `git`'s own message is
    /// kept, since "pull first" advice differs by workflow and this crate
    /// does not decide that for the caller.
    PushRejected { stderr: String },
    /// `git` refused to operate because the repository's ownership looks
    /// dubious to it (a mismatch between the current user and the
    /// directory's owner — common on WSL paths under `//wsl.localhost/...`
    /// and on networked drives). `path` is the directory `git` itself named
    /// in its error, extracted so a caller can offer to mark it safe
    /// (`git config --global --add safe.directory <path>`) rather than just
    /// showing the raw message.
    DubiousOwnership { path: std::path::PathBuf },
    /// `merge`, `rebase`, `cherry_pick` or `revert_commit` (R7) left the
    /// working tree with unmerged paths — `git`'s own exit code for this is
    /// the same `1` a dozen other refusals share, so this crate tells the
    /// two apart by checking `git diff --name-only --diff-filter=U`
    /// afterwards rather than by exit code. `paths` are repository-relative,
    /// ready for the Changes dock's existing "Merge Conflicts" group (R7's
    /// ADR-0031 amendment).
    MergeConflict { paths: Vec<String> },
    /// `push` was attempted with no `<remote> <branch>` pair the branch
    /// already tracks and none was given explicitly — `git`'s "no upstream
    /// branch" refusal (F3-9's `push` always names both explicitly, so this
    /// is reached only through R7's "Push" per-branch action offering to
    /// push a newly created local branch).
    NoUpstream { branch: String },
}

impl VcsError {
    pub const CODE_DISCOVER: i32 = 700;
    pub const CODE_READ: i32 = 701;
    pub const CODE_GIT_NOT_INSTALLED: i32 = 702;
    pub const CODE_GIT_FAILED: i32 = 703;
    pub const CODE_GIT_TIMED_OUT: i32 = 704;
    pub const CODE_UNMERGED_BRANCH: i32 = 705;
    pub const CODE_OUTSIDE_WORKING_TREE: i32 = 706;
    pub const CODE_TOO_LARGE_TO_DIFF: i32 = 707;
    pub const CODE_MALFORMED_PATCH: i32 = 708;
    pub const CODE_PUSH_REJECTED: i32 = 709;
    pub const CODE_DUBIOUS_OWNERSHIP: i32 = 710;
    pub const CODE_MERGE_CONFLICT: i32 = 711;
    pub const CODE_NO_UPSTREAM: i32 = 712;

    /// The variant's stable numeric code. Append-only once this crosses an
    /// FFI seam (ADR-0003): existing numbers must never be renumbered.
    pub fn code(&self) -> i32 {
        match self {
            VcsError::Discover(_) => Self::CODE_DISCOVER,
            VcsError::Read(_) => Self::CODE_READ,
            VcsError::GitNotInstalled => Self::CODE_GIT_NOT_INSTALLED,
            VcsError::GitFailed { .. } => Self::CODE_GIT_FAILED,
            VcsError::GitTimedOut { .. } => Self::CODE_GIT_TIMED_OUT,
            VcsError::UnmergedBranch { .. } => Self::CODE_UNMERGED_BRANCH,
            VcsError::OutsideWorkingTree => Self::CODE_OUTSIDE_WORKING_TREE,
            VcsError::TooLargeToDiff => Self::CODE_TOO_LARGE_TO_DIFF,
            VcsError::MalformedPatch(_) => Self::CODE_MALFORMED_PATCH,
            VcsError::PushRejected { .. } => Self::CODE_PUSH_REJECTED,
            VcsError::DubiousOwnership { .. } => Self::CODE_DUBIOUS_OWNERSHIP,
            VcsError::MergeConflict { .. } => Self::CODE_MERGE_CONFLICT,
            VcsError::NoUpstream { .. } => Self::CODE_NO_UPSTREAM,
        }
    }
}

impl fmt::Display for VcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VcsError::Discover(msg) => write!(f, "could not open the Git repository: {msg}"),
            VcsError::Read(msg) => write!(f, "{msg}"),
            VcsError::GitNotInstalled => {
                write!(
                    f,
                    "Git was not found on PATH; install Git to use this feature"
                )
            }
            VcsError::GitFailed { command, stderr } => {
                if stderr.trim().is_empty() {
                    write!(f, "`{command}` failed")
                } else {
                    write!(f, "`{command}` failed: {}", stderr.trim())
                }
            }
            VcsError::GitTimedOut { command } => write!(f, "`{command}` timed out"),
            VcsError::UnmergedBranch { branch } => write!(
                f,
                "branch \"{branch}\" has commits not merged elsewhere; force delete to discard them"
            ),
            VcsError::OutsideWorkingTree => {
                write!(f, "that path is outside the repository's working tree")
            }
            VcsError::TooLargeToDiff => write!(f, "file is too large to diff"),
            VcsError::MalformedPatch(msg) => write!(f, "generated patch was rejected: {msg}"),
            VcsError::PushRejected { stderr } => {
                write!(f, "push was rejected: {}", stderr.trim())
            }
            VcsError::DubiousOwnership { path } => {
                write!(f, "Git doesn't trust the ownership of {}", path.display())
            }
            VcsError::MergeConflict { paths } => write!(
                f,
                "conflicts in {} file(s); resolve them and commit",
                paths.len()
            ),
            VcsError::NoUpstream { branch } => write!(
                f,
                "\"{branch}\" has no upstream branch; push with an explicit remote"
            ),
        }
    }
}

impl std::error::Error for VcsError {}

impl From<io::Error> for VcsError {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::NotFound {
            VcsError::GitNotInstalled
        } else {
            VcsError::Read(err.to_string())
        }
    }
}
