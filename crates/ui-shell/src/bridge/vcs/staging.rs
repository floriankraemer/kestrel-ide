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

#[derive(Clone, Copy, PartialEq, Eq)]
enum HunkOp {
    Stage,
    Unstage,
}

/// Which cache a hunk index refers to: `requestHunks`'s buffer-based one
/// (the gutter) or `requestFileHunks`'s disk-based one (the Changes dock).
#[derive(Clone, Copy)]
enum HunkSource {
    Buffer,
    Disk,
}

fn to_ffi_hunk_states(states: &[vcs_core::HunkStageState]) -> Vec<ffi::FfiHunkState> {
    states
        .iter()
        .map(|state| ffi::FfiHunkState {
            state: match state {
                vcs_core::HunkStageState::Unstaged => ffi::FfiHunkStageState::Unstaged,
                vcs_core::HunkStageState::Staged => ffi::FfiHunkStageState::Staged,
                vcs_core::HunkStageState::Both => ffi::FfiHunkStageState::Both,
            },
        })
        .collect()
}

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
        self.apply_hunk_op(path, hunk_index, HunkOp::Stage, HunkSource::Buffer);
    }

    pub fn unstage_hunk(self: Pin<&mut Self>, path: &QString, hunk_index: u32) {
        self.apply_hunk_op(path, hunk_index, HunkOp::Unstage, HunkSource::Buffer);
    }

    pub fn stage_file_hunk(self: Pin<&mut Self>, path: &QString, hunk_index: u32) {
        self.apply_hunk_op(path, hunk_index, HunkOp::Stage, HunkSource::Disk);
    }

    pub fn unstage_file_hunk(self: Pin<&mut Self>, path: &QString, hunk_index: u32) {
        self.apply_hunk_op(path, hunk_index, HunkOp::Unstage, HunkSource::Disk);
    }

    /// The Changes dock's per-hunk rows (R6): the file read from disk,
    /// diffed against `HEAD`, classified against the index — the same
    /// three values `requestHunks` caches for an open buffer, into a
    /// separate cache (see `file_hunks`).
    pub fn request_file_hunks(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        let job_path = path.clone();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let relative = to_repo_relative(&worker.repo, Path::new(&job_path));
            let outcome = std::fs::read_to_string(&job_path)
                .map_err(|e| vcs_core::VcsError::Read(format!("{job_path}: {e}")))
                .and_then(|working_text| {
                    let working = worker.repo.hunks_against_head(&relative, &working_text)?;
                    let states = worker.repo.classify_hunks(&relative, &working.hunks)?;
                    Ok(CachedHunks {
                        before_text: working.before_text,
                        working_text,
                        hunks: working.hunks,
                        states,
                    })
                });
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match outcome {
                Ok(cached) => {
                    service
                        .file_hunks
                        .borrow_mut()
                        .insert(job_path.clone(), cached);
                    service
                        .as_mut()
                        .file_hunks_ready(QString::from(job_path.as_str()));
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn file_hunks(&self, path: &QString) -> Vec<ffi::FfiHunk> {
        match self.file_hunks.borrow().get(&path.to_string()) {
            Some(cached) => crate::bridge::convert::to_ffi_hunks(&cached.hunks),
            None => Vec::new(),
        }
    }

    pub fn file_hunk_states(&self, path: &QString) -> Vec<ffi::FfiHunkState> {
        match self.file_hunks.borrow().get(&path.to_string()) {
            Some(cached) => to_ffi_hunk_states(&cached.states),
            None => Vec::new(),
        }
    }

    pub fn hunk_states(&self, path: &QString) -> Vec<ffi::FfiHunkState> {
        match self.hunks.borrow().get(&path.to_string()) {
            Some(cached) => to_ffi_hunk_states(&cached.states),
            None => Vec::new(),
        }
    }

    pub fn hunk_removed_text(&self, path: &QString, hunk_index: u32) -> QString {
        let cache = self.hunks.borrow();
        let Some(cached) = cache.get(&path.to_string()) else {
            return QString::default();
        };
        let Some(hunk) = cached.hunks.get(hunk_index as usize) else {
            return QString::default();
        };
        let removed: Vec<&str> = cached
            .before_text
            .lines()
            .skip(hunk.old.start)
            .take(hunk.old.end - hunk.old.start)
            .collect();
        QString::from(removed.join("\n").as_str())
    }

    /// `git diff --name-only <revision>` on the worker; the project-root
    /// "Compare with Branch, Tag or Revision…" file list (R6).
    pub fn request_changed_paths_against(mut self: Pin<&mut Self>, revision: &QString) {
        let revision = revision.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let outcome = worker.repo.changed_paths_against(&revision);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match outcome {
                Ok(paths) => {
                    let paths = paths
                        .iter()
                        .map(|p| ffi::FfiRepoPath {
                            path: QString::from(p.to_string_lossy().as_ref()),
                        })
                        .collect();
                    service
                        .as_mut()
                        .changed_paths_ready(QString::from(revision.as_str()), paths);
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    /// "Add to .gitignore" (R6): `vcs_core::Repository::add_to_gitignore`
    /// on the worker; the watcher then sees the new line and `git status`
    /// stops listing the path, which is what `statusChanged` reports.
    pub fn add_to_gitignore(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let relative = to_repo_relative(&worker.repo, Path::new(&path));
            let result = worker.repo.add_to_gitignore(&relative);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(_) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
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
    fn apply_hunk_op(
        mut self: Pin<&mut Self>,
        path: &QString,
        hunk_index: u32,
        op: HunkOp,
        source: HunkSource,
    ) {
        let path = path.to_string();
        let cache = match source {
            HunkSource::Buffer => &self.hunks,
            HunkSource::Disk => &self.file_hunks,
        };
        let Some(cached): Option<CachedHunks> = cache.borrow().get(&path).cloned() else {
            return;
        };
        let reverse = op == HunkOp::Unstage;
        let Some(hunk) = cached.hunks.get(hunk_index as usize).cloned() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            // `path` (the cache key, and every path this bridge takes from
            // the view) is absolute; every `vcs_core` call wants a
            // repository-relative one, or `git apply --cached` rejects the
            // patch outright ("invalid path") — the same conversion
            // `stage_file`/`unstage_file`/`revert_file` above already make.
            let relative = to_repo_relative(&worker.repo, Path::new(&path));
            let result = if reverse {
                worker.repo.unstage_hunk_matching(&relative, &hunk)
            } else {
                worker
                    .repo
                    .stage_hunk_matching(&relative, &cached.working_text, &hunk)
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

    pub fn commit(
        mut self: Pin<&mut Self>,
        message: &QString,
        amend: bool,
        author: &QString,
        signoff: bool,
    ) {
        let message = message.to_string();
        let author = author.to_string().trim().to_string();
        let options = vcs_core::CommitOptions {
            amend,
            author: (!author.is_empty()).then_some(author),
            signoff,
        };
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.commit_with(&message, &options);
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
