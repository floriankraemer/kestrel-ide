//! Version control, Qt-free (ADR-0031).
//!
//! Reads of object/index state (discovery, HEAD, status, refs, log, blame)
//! go through `gix`, in-process. Anything that honours the user's config,
//! credentials, hooks or signing — fetch, pull, push, commit, staging,
//! checkout, branch, merge — shells out to the user's own `git` binary via
//! [`cli`]. See ADR-0031 for why, and `docs/architecture/layering.md`'s
//! `vcs-core` row for the dependency rule this split implies.
//!
//! No Qt/cxx-qt dependency, direct or transitive; `crates/ui-shell`'s
//! `VcsService` (F3-12, not yet built) is the only thing that may call this
//! crate from behind the FFI seam.

pub mod blame;
pub mod branch;
pub mod cli;
pub mod commit;
pub mod commit_diff;
mod error;
pub mod history;
pub mod hunks;
pub mod repo;
pub mod revert;
pub mod staging;

pub use blame::{BlameCache, BlameLine};
pub use commit_diff::{ChangedCommitFile, FileDiff};
pub use error::VcsError;
pub use history::{CommitDetail, HistoryCache, LogEntry};
pub use hunks::{HunkCache, WorkingHunks};
pub use repo::{ChangeKind, DiscoverResult, FileStatus, HeadInfo, RepoStatus, Repository};
pub use revert::{revert_hunk, TextEdit};
pub use staging::hunk_patch;

pub mod remote;
