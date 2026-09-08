//! Branches, remotes, history and blame (F3-12d).
//!
//! No UI consumer in this chunk — the Changes dock, branch widget and
//! history/blame panels are F3-17/18, blocked on F0-7's dock registry — the
//! same situation `vcs-core` itself landed a whole chunk earlier with zero
//! bridge. Translation only, like every other method in this module: what a
//! branch or a commit *is* stays `vcs-core`'s.

use core::pin::Pin;
use std::path::Path;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::ffi;

use super::{to_ffi_result, to_repo_relative, VcsWorker};

/// `fileHistory`/`blame` page rather than load the whole ancestry — a panel
/// only ever shows the first page of commits for one file.
const HISTORY_MAX: usize = 200;

fn to_ffi_log_entry(entry: &vcs_core::LogEntry) -> ffi::FfiLogEntry {
    ffi::FfiLogEntry {
        id: QString::from(entry.id.as_str()),
        summary: QString::from(entry.summary.as_str()),
        author_name: QString::from(entry.author_name.as_str()),
        author_email: QString::from(entry.author_email.as_str()),
        author_time: entry.author_time,
    }
}

fn to_ffi_commit_detail(detail: &vcs_core::CommitDetail) -> ffi::FfiCommitDetail {
    ffi::FfiCommitDetail {
        id: QString::from(detail.id.as_str()),
        summary: QString::from(detail.summary.as_str()),
        body: QString::from(detail.body.as_str()),
        author_name: QString::from(detail.author_name.as_str()),
        author_email: QString::from(detail.author_email.as_str()),
        author_time: detail.author_time,
        committer_name: QString::from(detail.committer_name.as_str()),
        committer_email: QString::from(detail.committer_email.as_str()),
        committer_time: detail.committer_time,
        parent_ids: QString::from(detail.parent_ids.join(" ").as_str()),
    }
}

fn to_ffi_changed_commit_file(file: &vcs_core::ChangedCommitFile) -> ffi::FfiChangedCommitFile {
    ffi::FfiChangedCommitFile {
        path: QString::from(file.path.to_string_lossy().as_ref()),
        change: super::to_ffi_change_kind(Some(file.change)),
    }
}

fn to_ffi_blame_line(line: &vcs_core::BlameLine) -> ffi::FfiBlameLine {
    ffi::FfiBlameLine {
        line: line.line as u32,
        commit: QString::from(line.commit.as_str()),
        author_name: QString::from(line.author_name.as_str()),
        author_email: QString::from(line.author_email.as_str()),
        summary: QString::from(line.summary.as_str()),
        content: QString::from(line.content.as_str()),
    }
}

impl ffi::VcsService {
    pub fn refresh_branches(mut self: Pin<&mut Self>) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            // One round trip for both: a caller asking for the branch list
            // almost always wants to know which one is current too (the
            // branch widget and menu both do), and `current_branch` is a
            // cheap `HEAD` read next to a ref listing.
            let names = worker.repo.branches();
            let current = worker.repo.current_branch();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match (names, current) {
                (Ok(names), Ok(current)) => {
                    *service.branches.borrow_mut() = names;
                    *service.current_branch.borrow_mut() = current.unwrap_or_default();
                    service.as_mut().branch_changed();
                }
                (Err(err), _) | (_, Err(err)) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn branches(&self) -> Vec<ffi::FfiBranch> {
        self.branches
            .borrow()
            .iter()
            .map(|name| ffi::FfiBranch {
                name: QString::from(name.as_str()),
            })
            .collect()
    }

    pub fn current_branch(&self) -> QString {
        QString::from(self.current_branch.borrow().as_str())
    }

    pub fn checkout(mut self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.checkout(&name);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => {
                    service.as_mut().branch_changed();
                    service.as_mut().refresh_status();
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn create_branch(mut self: Pin<&mut Self>, name: &QString, start_point: &QString) {
        let name = name.to_string();
        let start_point = start_point.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let start = if start_point.is_empty() {
                None
            } else {
                Some(start_point.as_str())
            };
            let result = worker.repo.create_branch(&name, start);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().branch_changed(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn delete_branch(mut self: Pin<&mut Self>, name: &QString, force: bool) {
        let name = name.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.delete_branch(&name, force);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().branch_changed(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn fetch(mut self: Pin<&mut Self>, remote: &QString) {
        let remote = remote.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.fetch(&remote);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if let Err(err) = result {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn pull(mut self: Pin<&mut Self>, remote: &QString, branch: &QString) {
        let remote = remote.to_string();
        let branch = branch.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.pull(&remote, &branch);
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

    pub fn push(mut self: Pin<&mut Self>, remote: &QString, branch: &QString, set_upstream: bool) {
        let remote = remote.to_string();
        let branch = branch.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.push(&remote, &branch, set_upstream);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().branch_changed(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn file_history(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        let signal_path = path.clone();
        let unavailable_path = path.clone();
        let queued = self.as_ref().push_job(move |worker: &VcsWorker| {
            // `path` is the tab's own (absolute) path — same normalization
            // `requestHunks`/`requestBlobAt` need, now shared.
            let relative = to_repo_relative(&worker.repo, Path::new(&path));
            let result =
                worker
                    .history_cache
                    .file_history(&worker.repo, relative, Some(HISTORY_MAX));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(entries) => {
                    let entries: Vec<ffi::FfiLogEntry> =
                        entries.iter().map(to_ffi_log_entry).collect();
                    service
                        .as_mut()
                        .history_ready(QString::from(signal_path.as_str()), entries);
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
        if !queued {
            // No worker yet: not a repository, or discovery still running.
            // An empty `historyReady` would read as "no commits" — tell the
            // panel explicitly instead of leaving it to wait forever.
            self.as_mut()
                .history_unavailable(QString::from(unavailable_path.as_str()));
        }
    }

    pub fn commit_log(mut self: Pin<&mut Self>, max: u32) {
        let max = if max == 0 { HISTORY_MAX } else { max as usize };
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.history_cache.log(&worker.repo, Some(max));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(entries) => {
                    let entries: Vec<ffi::FfiLogEntry> =
                        entries.iter().map(to_ffi_log_entry).collect();
                    service.as_mut().commit_log_ready(entries);
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn request_commit_detail(mut self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        let qt_thread = self.as_mut().qt_thread();
        let job_id = id.clone();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            // Both come off the same commit — one worker round trip rather
            // than the view firing a second request once it sees the
            // first answer.
            let detail = worker.history_cache.commit_detail(&worker.repo, &job_id);
            let files = worker.repo.changed_files(&job_id);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                match detail {
                    Ok(Some(detail)) => {
                        service
                            .commit_details
                            .borrow_mut()
                            .insert(job_id.clone(), detail);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        service.as_mut().vcs_failed(to_ffi_result(&err));
                        return;
                    }
                }
                match files {
                    Ok(files) => {
                        service
                            .changed_commit_files
                            .borrow_mut()
                            .insert(job_id.clone(), files);
                    }
                    Err(err) => {
                        service.as_mut().vcs_failed(to_ffi_result(&err));
                        return;
                    }
                }
                service
                    .as_mut()
                    .commit_detail_ready(QString::from(job_id.as_str()));
            });
        });
    }

    pub fn commit_detail(&self, id: &QString) -> ffi::FfiCommitDetail {
        match self.commit_details.borrow().get(&id.to_string()) {
            Some(detail) => to_ffi_commit_detail(detail),
            None => ffi::FfiCommitDetail::default(),
        }
    }

    pub fn changed_commit_files(&self, id: &QString) -> Vec<ffi::FfiChangedCommitFile> {
        match self.changed_commit_files.borrow().get(&id.to_string()) {
            Some(files) => files.iter().map(to_ffi_changed_commit_file).collect(),
            None => Vec::new(),
        }
    }

    pub fn request_commit_file_diff(mut self: Pin<&mut Self>, id: &QString, path: &QString) {
        let id = id.to_string();
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        let job_id = id.clone();
        let job_path = path.clone();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            // `path` here is a repository-relative path off `changedCommitFiles`,
            // not a tab's absolute path — no `to_repo_relative` translation
            // needed, unlike `requestHunks`/`requestBlobAt`.
            let result = worker.repo.commit_file_diff(&job_id, Path::new(&job_path));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(diff) => {
                    service
                        .commit_file_diffs
                        .borrow_mut()
                        .insert((job_id.clone(), job_path.clone()), diff);
                    service.as_mut().commit_file_diff_ready(
                        QString::from(job_id.as_str()),
                        QString::from(job_path.as_str()),
                    );
                }
                Err(err) => {
                    service.as_mut().vcs_failed(to_ffi_result(&err));
                }
            });
        });
    }

    pub fn commit_file_diff(&self, id: &QString, path: &QString) -> ffi::FfiFileDiff {
        match self
            .commit_file_diffs
            .borrow()
            .get(&(id.to_string(), path.to_string()))
        {
            Some(diff) => ffi::FfiFileDiff {
                path: path.clone(),
                old_text: QString::from(diff.old_text.as_str()),
                new_text: QString::from(diff.new_text.as_str()),
            },
            None => ffi::FfiFileDiff::default(),
        }
    }

    pub fn commit_file_diff_hunks(&self, id: &QString, path: &QString) -> Vec<ffi::FfiHunk> {
        match self
            .commit_file_diffs
            .borrow()
            .get(&(id.to_string(), path.to_string()))
        {
            Some(diff) => crate::bridge::convert::to_ffi_hunks(&diff.hunks),
            None => Vec::new(),
        }
    }

    pub fn blame(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let qt_thread = self.as_mut().qt_thread();
        let signal_path = path.clone();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let relative = to_repo_relative(&worker.repo, Path::new(&path));
            let result = worker.blame_cache.blame(&worker.repo, relative);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(lines) => {
                    let lines: Vec<ffi::FfiBlameLine> =
                        lines.iter().map(to_ffi_blame_line).collect();
                    service
                        .as_mut()
                        .blame_ready(QString::from(signal_path.as_str()), lines);
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }
}
