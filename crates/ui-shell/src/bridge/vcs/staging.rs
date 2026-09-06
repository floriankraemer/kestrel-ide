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
                .stage_file(to_repo_relative(&worker.repo, Path::new(&path)));
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
                .unstage_file(to_repo_relative(&worker.repo, Path::new(&path)));
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
                .discard_file(to_repo_relative(&worker.repo, Path::new(&path)));
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

    /// Stage or reverse-stage `hunks(path)[hunk_index]`, against the same
    /// `HEAD`/working text `requestHunks` last cached for `path`.
    ///
    /// `vcs_core::Repository::stage_hunk`'s own doc comment flags the real
    /// limitation this inherits: `HunkCache` diffs against `HEAD`, not the
    /// index, so this is exactly right on a clean index and increasingly
    /// wrong the more of the file is already staged. Correct per-hunk
    /// staging against the index belongs to F3-17's Changes dock, which has
    /// a reason to read the index's own blob; this bridge exposes what
    /// `vcs-core` already has rather than growing that read early.
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
                worker
                    .repo
                    .unstage_hunk(relative, &cached.before_text, &cached.working_text, &hunk)
            } else {
                worker
                    .repo
                    .stage_hunk(relative, &cached.before_text, &cached.working_text, &hunk)
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
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
}
