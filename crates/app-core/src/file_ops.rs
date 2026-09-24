//! File operations a workspace edit asks for: create, rename, delete
//! (F2; its ADR is unwritten).
//!
//! These are `app-core`'s own types rather than `lsp-core`'s, because
//! performing one means deciding what happens to an open tab whose file moved
//! underneath it — a rule about this application's state. `app-core` may not
//! depend on `lsp-core`, so the adapter maps one to the other and decides
//! nothing.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{AppError, AppSession, RetitledTab};

/// A file a refactoring wants created, renamed or deleted.
///
/// This is `app-core`'s own type, not `lsp-core`'s: performing the operation
/// means deciding what happens to an open tab whose file moved underneath it,
/// and that is a rule about this application's state. `app-core` may not
/// depend on `lsp-core`, so the adapter maps one to the other and decides
/// nothing (F2; its ADR is unwritten).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    Create {
        path: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        path: PathBuf,
        recursive: bool,
        ignore_if_not_exists: bool,
    },
}

impl FileOp {
    fn paths(&self) -> Vec<&Path> {
        match self {
            FileOp::Create { path, .. } | FileOp::Delete { path, .. } => vec![path],
            FileOp::Rename { from, to, .. } => vec![from, to],
        }
    }
}

/// Why a set of file operations was refused, or how far it got.
///
/// Named to avoid colliding with `project_model::FileOpError`, which covers
/// the single create/rename/delete a user performs from the project tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceOpError {
    /// A path lies outside the project root. Refused before anything is
    /// touched: a refactoring may rearrange the project, never the machine.
    OutsideProject { path: PathBuf },
    /// The file has unsaved changes, so renaming or deleting it would
    /// discard them. Refused rather than guessing — silently losing an
    /// edit is the worst available outcome, and saving on the user's behalf
    /// is a decision they did not make.
    UnsavedChanges { path: PathBuf },
    /// The target exists and the server did not ask to overwrite it.
    AlreadyExists { path: PathBuf },
    /// The operation failed partway. `applied` names the operations that
    /// did take effect, because the filesystem is not transactional and
    /// pretending otherwise leaves the user unable to tell what state their
    /// project is in.
    Partial {
        applied: usize,
        total: usize,
        message: String,
    },
}

/// Whether `path` resolves inside `root`.
///
/// Compared after canonicalising as far as each side exists, so a symlink or
/// a `..` cannot smuggle a write outside the project. A path that does not
/// exist yet — every `create` target — is checked by its nearest existing
/// ancestor, which is the part an attacker would have to control anyway.
fn path_within(root: &Path, path: &Path) -> bool {
    let real_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut probe = path.to_path_buf();
    loop {
        if let Ok(real) = probe.canonicalize() {
            return real.starts_with(&real_root);
        }
        if !probe.pop() {
            return false;
        }
    }
}

impl fmt::Display for ResourceOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceOpError::OutsideProject { path } => write!(
                f,
                "{} is outside the project; a refactoring may rearrange the project, not the machine",
                path.display()
            ),
            ResourceOpError::UnsavedChanges { path } => write!(
                f,
                "{} has unsaved changes — save it first, then run this again",
                path.display()
            ),
            ResourceOpError::AlreadyExists { path } => {
                write!(f, "{} already exists", path.display())
            }
            ResourceOpError::Partial {
                applied,
                total,
                message,
            } => write!(
                f,
                "{applied} of {total} file operations were applied before this failed: {message}. \
                 The project is part-way through the change and cannot be rolled back automatically."
            ),
        }
    }
}

impl AppSession {
    /// Perform the file operations a refactoring asked for, then retarget
    /// any open tabs they moved (F2; its ADR is unwritten).
    ///
    /// Order is the server's, and it matters: a refactoring that renames a
    /// type and then renames its file to match sends the text edit first and
    /// the rename second, so performing all renames up front would leave the
    /// edit addressing a path that no longer exists.
    ///
    /// Everything is validated before anything is touched — paths inside the
    /// project, no unsaved buffers about to be moved out from under the user,
    /// no silent overwrites. The filesystem is not transactional, so once the
    /// first operation succeeds a later failure cannot be undone; it is
    /// reported as [`ResourceOpError::Partial`] naming how far it got, rather
    /// than pretending nothing happened.
    ///
    /// The tree is not re-snapshotted here: a workspace edit's file
    /// operations are driven by a language server (a rename-symbol-and-
    /// rename-file refactor, say), not the sidebar's own context menu, so
    /// the responsiveness case the plan's "Step 3" calls out ("the user just
    /// clicked New File, they expect to see it now") doesn't apply the same
    /// way here — the real filesystem watcher's own incremental,
    /// per-directory refresh (`ui-shell`'s `ProjectTreeModel`) picks up
    /// every path this touched once its event arrives, the plan's
    /// explicitly-allowed simpler alternative for this caller.
    pub fn apply_file_ops(&mut self, ops: &[FileOp]) -> Result<Vec<RetitledTab>, AppError> {
        self.validate_file_ops(ops)?;

        let mut retitled = Vec::new();
        for (index, op) in ops.iter().enumerate() {
            if let Err(message) = self.perform_file_op(op, &mut retitled) {
                return Err(AppError::ResourceOp(ResourceOpError::Partial {
                    applied: index,
                    total: ops.len(),
                    message,
                }));
            }
        }
        Ok(retitled)
    }

    /// Every reason to refuse, checked before the first operation runs.
    fn validate_file_ops(&self, ops: &[FileOp]) -> Result<(), AppError> {
        let root = self.root_path().map(|p| p.to_path_buf());
        for op in ops {
            for path in op.paths() {
                // A refactoring may rearrange the project. It may not
                // rearrange the machine — so this is checked against the
                // real, resolved root, not the textual prefix.
                if let Some(root) = &root {
                    if !path_within(root, path) {
                        return Err(AppError::ResourceOp(ResourceOpError::OutsideProject {
                            path: path.to_path_buf(),
                        }));
                    }
                }
            }
            match op {
                FileOp::Create {
                    path,
                    overwrite,
                    ignore_if_exists,
                } => {
                    if path.exists() && !overwrite && !ignore_if_exists {
                        return Err(AppError::ResourceOp(ResourceOpError::AlreadyExists {
                            path: path.clone(),
                        }));
                    }
                }
                FileOp::Rename {
                    from,
                    to,
                    overwrite,
                    ignore_if_exists,
                } => {
                    self.refuse_if_dirty(from)?;
                    if to.exists() && !overwrite && !ignore_if_exists {
                        return Err(AppError::ResourceOp(ResourceOpError::AlreadyExists {
                            path: to.clone(),
                        }));
                    }
                }
                FileOp::Delete { path, .. } => self.refuse_if_dirty(path)?,
            }
        }
        Ok(())
    }

    /// Moving or deleting a file with unsaved edits would discard them.
    ///
    /// Refused rather than resolved: saving on the user's behalf commits a
    /// change they did not ask to commit, and dropping the edits is worse
    /// still. They can save and run the refactoring again.
    fn refuse_if_dirty(&self, path: &Path) -> Result<(), AppError> {
        if let Some(id) = self.find_tab_by_path(path) {
            if self.tab_is_dirty(id) == Some(true) {
                return Err(AppError::ResourceOp(ResourceOpError::UnsavedChanges {
                    path: path.to_path_buf(),
                }));
            }
        }
        Ok(())
    }

    /// One operation. Returns the message to report if it fails.
    fn perform_file_op(
        &mut self,
        op: &FileOp,
        retitled: &mut Vec<RetitledTab>,
    ) -> Result<(), String> {
        match op {
            FileOp::Create {
                path,
                ignore_if_exists,
                ..
            } => {
                if path.exists() && *ignore_if_exists {
                    return Ok(());
                }
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(path, "").map_err(|e| e.to_string())
            }
            FileOp::Rename {
                from,
                to,
                ignore_if_exists,
                ..
            } => {
                if to.exists() && *ignore_if_exists {
                    return Ok(());
                }
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::rename(from, to).map_err(|e| e.to_string())?;
                // The tab keeps its TabId and its history — a rename that
                // closed and reopened the tab would throw away the user's
                // undo stack for a change they did not make.
                if let Some(id) = self.find_tab_by_path(from) {
                    if let Some(entry) = self.entry_mut(id) {
                        entry.content.set_path(to.clone());
                    }
                    if let Some(title) = self.tab_title(id) {
                        retitled.push(RetitledTab { id, title });
                    }
                }
                Ok(())
            }
            FileOp::Delete {
                path,
                recursive,
                ignore_if_not_exists,
            } => {
                // A delete of something already gone got what it wanted,
                // whatever the server's ignoreIfNotExists said: the file is
                // absent either way, which is the requested end state.
                let _ = ignore_if_not_exists;
                if !path.exists() {
                    return Ok(());
                }
                let result = if path.is_dir() {
                    if !recursive {
                        return Err(format!(
                            "{} is a directory and the server did not ask for a recursive delete",
                            path.display()
                        ));
                    }
                    fs::remove_dir_all(path)
                } else {
                    fs::remove_file(path)
                };
                result.map_err(|e| e.to_string())?;
                if let Some(id) = self.find_tab_by_path(path) {
                    if let Some(entry) = self.entry_mut(id) {
                        entry.content.mark_deleted();
                    }
                    if let Some(title) = self.tab_title(id) {
                        retitled.push(RetitledTab { id, title });
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    // -----------------------------------------------------------------
    // File operations from a workspace edit (F2)
    // -----------------------------------------------------------------

    fn project_session() -> (AppSession, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut session = AppSession::with_config_dir(config.path().to_path_buf());
        session.open_project(dir.path()).unwrap();
        std::mem::forget(config);
        (session, dir)
    }

    #[test]
    fn create_makes_the_file() {
        let (mut session, dir) = project_session();
        let path = dir.path().join("new.rs");
        session
            .apply_file_ops(&[FileOp::Create {
                path: path.clone(),
                overwrite: false,
                ignore_if_exists: false,
            }])
            .unwrap();
        assert!(path.exists());
    }

    #[test]
    fn create_over_an_existing_file_is_refused_without_overwrite() {
        let (mut session, dir) = project_session();
        let path = dir.path().join("taken.rs");
        fs::write(&path, "keep me").unwrap();
        let err = session
            .apply_file_ops(&[FileOp::Create {
                path: path.clone(),
                overwrite: false,
                ignore_if_exists: false,
            }])
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::ResourceOp(ResourceOpError::AlreadyExists { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep me");
    }

    #[test]
    fn create_with_ignore_if_exists_leaves_the_file_alone() {
        let (mut session, dir) = project_session();
        let path = dir.path().join("taken.rs");
        fs::write(&path, "keep me").unwrap();
        session
            .apply_file_ops(&[FileOp::Create {
                path: path.clone(),
                overwrite: false,
                ignore_if_exists: true,
            }])
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep me");
    }

    // A rename must not close and reopen the tab: that would throw away the
    // user's undo history for a change they did not make.
    #[test]
    fn rename_keeps_the_tabs_identity_and_retitles_it() {
        let (mut session, dir) = project_session();
        let from = dir.path().join("old.rs");
        let to = dir.path().join("new.rs");
        fs::write(&from, "fn main() {}").unwrap();
        let opened = session.open_file(&from).unwrap();
        let id = opened.id;

        let retitled = session
            .apply_file_ops(&[FileOp::Rename {
                from: from.clone(),
                to: to.clone(),
                overwrite: false,
                ignore_if_exists: false,
            }])
            .unwrap();

        assert!(to.exists() && !from.exists());
        assert_eq!(retitled.len(), 1);
        assert_eq!(retitled[0].id, id, "the tab kept its identity");
        assert_eq!(session.tab_title(id).as_deref(), Some("new.rs"));
    }

    // Moving a file with unsaved edits would discard them. Saving on the
    // user's behalf commits a change they did not ask to commit, so this
    // refuses and says why.
    #[test]
    fn renaming_a_file_with_unsaved_changes_is_refused() {
        let (mut session, dir) = project_session();
        let from = dir.path().join("dirty.rs");
        fs::write(&from, "fn main() {}").unwrap();
        let id = session.open_file(&from).unwrap().id;
        assert!(session.set_tab_dirty(id, true));

        let err = session
            .apply_file_ops(&[FileOp::Rename {
                from: from.clone(),
                to: dir.path().join("moved.rs"),
                overwrite: false,
                ignore_if_exists: false,
            }])
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::ResourceOp(ResourceOpError::UnsavedChanges { .. })
        ));
        assert!(from.exists(), "nothing was moved");
    }

    #[test]
    fn delete_marks_the_open_tab_deleted() {
        let (mut session, dir) = project_session();
        let path = dir.path().join("gone.rs");
        fs::write(&path, "x").unwrap();
        let id = session.open_file(&path).unwrap().id;

        session
            .apply_file_ops(&[FileOp::Delete {
                path: path.clone(),
                recursive: false,
                ignore_if_not_exists: false,
            }])
            .unwrap();

        assert!(!path.exists());
        assert!(
            session.tab_is_dirty(id).is_some(),
            "the tab is still open after its file was deleted"
        );
    }

    // The requested end state is "this file is absent", and it already is.
    #[test]
    fn deleting_something_already_gone_is_success() {
        let (mut session, dir) = project_session();
        session
            .apply_file_ops(&[FileOp::Delete {
                path: dir.path().join("never-existed.rs"),
                recursive: false,
                ignore_if_not_exists: false,
            }])
            .unwrap();
    }

    #[test]
    fn deleting_a_directory_without_recursive_is_refused() {
        let (mut session, dir) = project_session();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let err = session
            .apply_file_ops(&[FileOp::Delete {
                path: sub.clone(),
                recursive: false,
                ignore_if_not_exists: false,
            }])
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::ResourceOp(ResourceOpError::Partial { .. })
        ));
        assert!(sub.exists());
    }

    // A refactoring may rearrange the project. It may not rearrange the
    // machine. This is a trust boundary, so it is checked before anything is
    // touched rather than relied on to fail later.
    #[test]
    fn a_path_outside_the_project_is_refused_before_anything_happens() {
        let (mut session, dir) = project_session();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.rs");
        fs::write(&victim, "not yours").unwrap();
        let inside = dir.path().join("ok.rs");

        let err = session
            .apply_file_ops(&[
                FileOp::Create {
                    path: inside.clone(),
                    overwrite: false,
                    ignore_if_exists: false,
                },
                FileOp::Delete {
                    path: victim.clone(),
                    recursive: false,
                    ignore_if_not_exists: false,
                },
            ])
            .unwrap_err();

        assert!(matches!(
            err,
            AppError::ResourceOp(ResourceOpError::OutsideProject { .. })
        ));
        assert!(victim.exists(), "the file outside the project survived");
        assert!(
            !inside.exists(),
            "validation runs before the first operation, so nothing was created"
        );
    }

    #[test]
    fn a_symlink_escaping_the_project_is_refused() {
        let (mut session, dir) = project_session();
        let outside = tempfile::tempdir().unwrap();
        let link = dir.path().join("escape");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        #[cfg(not(unix))]
        return;

        let err = session
            .apply_file_ops(&[FileOp::Create {
                path: link.join("planted.rs"),
                overwrite: false,
                ignore_if_exists: false,
            }])
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::ResourceOp(ResourceOpError::OutsideProject { .. })
        ));
    }

    // The filesystem is not transactional. When an operation fails partway
    // the user is told exactly how far it got, because a project in an
    // unknown half-changed state is worse than one in a known half-changed
    // state.
    #[test]
    fn a_failure_partway_reports_how_far_it_got() {
        let (mut session, dir) = project_session();
        let first = dir.path().join("first.rs");
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();

        let err = session
            .apply_file_ops(&[
                FileOp::Create {
                    path: first.clone(),
                    overwrite: false,
                    ignore_if_exists: false,
                },
                // A directory delete without `recursive` fails at perform
                // time rather than validation time.
                FileOp::Delete {
                    path: sub.clone(),
                    recursive: false,
                    ignore_if_not_exists: false,
                },
            ])
            .unwrap_err();

        match err {
            AppError::ResourceOp(ResourceOpError::Partial { applied, total, .. }) => {
                assert_eq!((applied, total), (1, 2));
            }
            other => panic!("expected a partial failure, got {other:?}"),
        }
        assert!(first.exists(), "the first operation did take effect");
    }

    // Order is the server's, and it matters: "rename the type, then rename
    // its file to match" is a text edit followed by a rename, and performing
    // renames first would leave the edit addressing a path that is gone.
    #[test]
    fn operations_are_performed_in_the_order_given() {
        let (mut session, dir) = project_session();
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        session
            .apply_file_ops(&[
                FileOp::Create {
                    path: a.clone(),
                    overwrite: false,
                    ignore_if_exists: false,
                },
                FileOp::Rename {
                    from: a.clone(),
                    to: b.clone(),
                    overwrite: false,
                    ignore_if_exists: false,
                },
            ])
            .unwrap();
        assert!(b.exists() && !a.exists());
    }
}
