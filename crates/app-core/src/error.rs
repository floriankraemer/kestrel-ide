//! `AppError` (ADR-0003): why an [`crate::AppSession`] command failed.
//!
//! Split out of `lib.rs` once that file hit its size-gate ceiling — a
//! mechanical move, no behavior change (same pattern `vcs/mod.rs` used for
//! `staging`/`remote`).

use std::fmt;
use std::io;
use std::path::PathBuf;

use project_model::{FileOpError, OpenFolderError};

use crate::ResourceOpError;

/// Why an [`crate::AppSession`] command failed. Each variant carries a stable
/// numeric code (ADR-0003) so the UI can branch on error kind — e.g. show
/// the binary-file rejection as information rather than an error — while the
/// `Display` message is shown to the user verbatim.
#[derive(Debug)]
pub enum AppError {
    /// The `TabId` doesn't name an open tab (closed, or never issued).
    NoSuchTab,
    /// "Open Folder" failed; the current project is left unchanged (US-1).
    OpenFolder(OpenFolderError),
    /// The old binary-open rejection (US-2b). No longer produced: a binary
    /// file now opens a read-only hex tab (ADR-0020). Retained because the
    /// numeric codes below are an append-only FFI contract.
    BinaryFile(PathBuf),
    /// Reading the file into a document failed (e.g. not valid UTF-8).
    OpenFile(io::Error),
    /// Writing the tab's content to disk failed; the dirty flag stays set
    /// (US-4: no silent data loss).
    Save(io::Error),
    /// A create/rename/delete filesystem mutation failed (US-2b).
    FileOp(FileOpError),
    /// Re-reading the tab's backing file from disk failed (US-3's "Reload").
    Reload(io::Error),
    /// The mutation itself succeeded but re-snapshotting the tree from disk
    /// failed afterwards (e.g. the root vanished mid-operation).
    TreeRebuild(io::Error),
    /// A workspace edit's file operations were refused, or failed partway
    /// (F2; its ADR is unwritten).
    ResourceOp(ResourceOpError),
    /// The tab exists but holds a binary file, and the command asked for
    /// something only a text document can do (edit, save, reload). Distinct
    /// from [`AppError::NoSuchTab`] so the view can tell "that tab is gone"
    /// from "that tab is not editable".
    NotATextTab(PathBuf),
    /// The filesystem watcher failed to start (e.g. a watch-descriptor
    /// limit). Non-fatal: the project still opens, only external changes
    /// go unnoticed until it's reopened.
    WatcherFailed(String),
}

impl AppError {
    /// Success code at the FFI seam; never produced by an `AppError`.
    pub const CODE_OK: i32 = 0;
    pub const CODE_NO_SUCH_TAB: i32 = 1;
    pub const CODE_OPEN_FOLDER: i32 = 2;
    pub const CODE_BINARY_FILE: i32 = 3;
    pub const CODE_OPEN_FILE: i32 = 4;
    pub const CODE_SAVE: i32 = 5;
    pub const CODE_FILE_OP: i32 = 6;
    pub const CODE_RELOAD: i32 = 7;
    pub const CODE_TREE_REBUILD: i32 = 8;
    pub const CODE_NOT_A_TEXT_TAB: i32 = 9;
    /// Append-only: these codes cross the FFI seam (ADR-0003), so a new
    /// error takes the next number and never reuses a retired one.
    pub const CODE_RESOURCE_OP: i32 = 10;
    pub const CODE_WATCHER_FAILED: i32 = 11;

    /// The variant's stable numeric code (ADR-0003). These are part of the
    /// FFI contract — `main_window.cpp` branches on them — so existing
    /// numbers must never be renumbered, only appended to.
    pub fn code(&self) -> i32 {
        match self {
            AppError::NoSuchTab => Self::CODE_NO_SUCH_TAB,
            AppError::OpenFolder(_) => Self::CODE_OPEN_FOLDER,
            AppError::BinaryFile(_) => Self::CODE_BINARY_FILE,
            AppError::OpenFile(_) => Self::CODE_OPEN_FILE,
            AppError::Save(_) => Self::CODE_SAVE,
            AppError::FileOp(_) => Self::CODE_FILE_OP,
            AppError::Reload(_) => Self::CODE_RELOAD,
            AppError::TreeRebuild(_) => Self::CODE_TREE_REBUILD,
            AppError::NotATextTab(_) => Self::CODE_NOT_A_TEXT_TAB,
            AppError::ResourceOp(_) => Self::CODE_RESOURCE_OP,
            AppError::WatcherFailed(_) => Self::CODE_WATCHER_FAILED,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::ResourceOp(err) => write!(f, "{err}"),
            AppError::NoSuchTab => write!(f, "no such tab"),
            AppError::OpenFolder(e) => write!(f, "{e}"),
            AppError::BinaryFile(p) => write!(
                f,
                "\"{}\" is a binary file and cannot be opened as text.",
                p.display()
            ),
            AppError::OpenFile(e) => write!(f, "{e}"),
            AppError::Save(e) => write!(f, "{e}"),
            AppError::FileOp(e) => write!(f, "{e}"),
            AppError::Reload(e) => write!(f, "{e}"),
            AppError::TreeRebuild(e) => write!(f, "{e}"),
            AppError::NotATextTab(p) => write!(
                f,
                "\"{}\" is open as a binary file and cannot be edited.",
                p.display()
            ),
            AppError::WatcherFailed(message) => {
                write!(f, "live file watching could not be started: {message}")
            }
        }
    }
}

impl std::error::Error for AppError {}
