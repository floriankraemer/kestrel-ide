//! Staging and commit (F3-12c). Every operation here shells out
//! (ADR-0031) and reports success by asking `refreshStatus` again rather
//! than guessing the new status itself — one source of truth for what
//! `changedFiles()` says, exactly as `vcs_core::Repository::status` is.

use core::pin::Pin;
use std::path::Path;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::ffi;

use super::{to_ffi_result, to_repo_relative, CachedHunks, VcsWorker};

impl ffi::VcsService {
    pub fn stage_file(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker
                .repo
                .stage_file(&to_repo_relative(&worker.repo, Path::new(&path)));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn unstage_file(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker
                .repo
                .unstage_file(&to_repo_relative(&worker.repo, Path::new(&path)));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn stage_all(mut self: Pin<&mut Self>) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.stage_all();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn unstage_all(mut self: Pin<&mut Self>) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.unstage_all();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn revert_file(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker
                .repo
                .discard_file(&to_repo_relative(&worker.repo, Path::new(&path)));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn stage_hunk(self: Pin<&mut Self>, path: &QString, hunk_index: u32) {
        self.apply_hunk_op(path, hunk_index, false);
    }

    pub fn unstage_hunk(self: Pin<&mut Self>, path: &QString, hunk_index: u32) {
        self.apply_hunk_op(path, hunk_index, true);
    }

    /// Stage or unstage `hunks(path)[hunk_index]` — a `HEAD`-vs-worktree
    /// hunk, the gutter's own view — against the index, not `HEAD`.
    ///
    /// `vcs_core::Repository::stage_hunk_matching`/`unstage_hunk_matching`
    /// carry the real work (finding the index-vs-worktree, respectively
    /// `HEAD`-vs-index, hunk that actually corresponds to the one clicked):
    /// this is translation only, matching R6's fix for "stages the whole
    /// file" — staging used to always diff against `HEAD`, so it staged
    /// everything between `HEAD` and the worktree once part of the file was
    /// already staged.
    fn apply_hunk_op(mut self: Pin<&mut Self>, path: &QString, hunk_index: u32, reverse: bool) {
        let path = path.to_string();
        let Some(cached): Option<CachedHunks> = self.hunks.borrow().get(&path).cloned() else {
            return;
        };
        let Some(hunk) = cached.hunks.get(hunk_index as usize).cloned() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let relative = Path::new(&path);
            let result = if reverse {
                worker.repo.unstage_hunk_matching(relative, &hunk)
            } else {
                worker
                    .repo
                    .stage_hunk_matching(relative, &cached.working_text, &hunk)
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(_) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn commit(mut self: Pin<&mut Self>, message: &QString, amend: bool) {
        let message = message.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.commit(&message, amend);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => {
                    service.as_mut().record_commit_message(&message);
                    service.as_mut().refresh_status();
                    service.as_mut().branch_changed();
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    /// Append `message` to this project's commit-message history
    /// (`.ide/local/vcs.toml`, ADR-0022) after a successful commit.
    fn record_commit_message(&self, message: &str) {
        let root = self.project_root.borrow();
        if root.is_empty() {
            return;
        }
        let _ = app_config::vcs_local_settings::update(Path::new(root.as_str()), |settings| {
            settings.push_commit_message(message.to_string());
        });
    }

    /// This project's commit-message history, newest first — the Changes
    /// dock's message combo.
    pub fn commit_history(&self) -> Vec<ffi::FfiCommitMessage> {
        let root = self.project_root.borrow();
        if root.is_empty() {
            return Vec::new();
        }
        app_config::vcs_local_settings::load(Path::new(root.as_str()))
            .map(|settings| {
                settings
                    .commit_message_history
                    .into_iter()
                    .map(|message| ffi::FfiCommitMessage {
                        message: QString::from(message.as_str()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
