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

/// `entries` in the order `vcs-core` returns them (newest first), each
/// carrying its `vcs_core::graph::lanes` position — computed once here
/// rather than the view re-deriving it, the same "the rule lives in
/// `vcs-core`, the bridge only translates" split every other conversion in
/// this module follows.
fn to_ffi_log_entries(entries: &[vcs_core::LogEntry]) -> Vec<ffi::FfiLogEntry> {
    let lanes = vcs_core::lanes(entries);
    entries
        .iter()
        .zip(lanes.iter())
        .map(|(entry, lane)| ffi::FfiLogEntry {
            id: QString::from(entry.id.as_str()),
            summary: QString::from(entry.summary.as_str()),
            author_name: QString::from(entry.author_name.as_str()),
            author_email: QString::from(entry.author_email.as_str()),
            author_time: entry.author_time,
            lane: lane.lane as u32,
            parent_lanes: lane.parent_lanes.iter().map(|l| *l as u32).collect(),
        })
        .collect()
}

fn to_ffi_ref_kind(kind: vcs_core::RefKind) -> ffi::FfiRefKind {
    match kind {
        vcs_core::RefKind::Head => ffi::FfiRefKind::Head,
        vcs_core::RefKind::Local => ffi::FfiRefKind::Local,
        vcs_core::RefKind::Remote => ffi::FfiRefKind::Remote,
        vcs_core::RefKind::Tag => ffi::FfiRefKind::Tag,
    }
}

fn to_ffi_reset_mode(mode: ffi::FfiResetMode) -> vcs_core::ResetMode {
    match mode {
        ffi::FfiResetMode::Mixed => vcs_core::ResetMode::Mixed,
        ffi::FfiResetMode::Hard => vcs_core::ResetMode::Hard,
        // `Soft` and any value outside the declared enum (cxx passes the
        // wire value through unchecked) both fall to the least destructive
        // mode — a reset a caller cannot lose work to by accident.
        _ => vcs_core::ResetMode::Soft,
    }
}

/// An empty `QString` means "no filter on this field" everywhere
/// `commitLogFiltered` takes one.
fn non_empty(value: &QString) -> Option<String> {
    let value = value.to_string();
    (!value.is_empty()).then_some(value)
}

/// `path` (a tab's absolute path) relative to `root` — the blame-toggle
/// memory's own key, computed the same tolerant way
/// `super::to_repo_relative` is, but from the project root string alone:
/// unlike every other user of that helper, `blameEnabledFor`/
/// `setBlameEnabledFor` are synchronous config reads/writes with no worker
/// round trip to ask the `Repository` handle itself.
fn repo_relative_string(root: &str, path: &str) -> String {
    super::strip_prefix_tolerant(root, path, cfg!(windows))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
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
        author_time: line.author_time,
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
            // Tags ride along for the revision picker (R6): a third cheap
            // ref read on the same trip, never a round trip of its own.
            let refs = worker.repo.ref_names();
            // R7: remote-tracking branches (the popup's Remote section), ref
            // decorations (the log's chips) and the remote list (the push
            // picker) — three more cheap `gix` reads on the same trip,
            // recomputed on every branch/HEAD move the same as
            // `names`/`current`/`refs` already are.
            let remote_branches = worker.repo.remote_branches();
            let refs_by_commit = worker.repo.refs_by_commit();
            let remotes = worker.repo.remotes();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                match (
                    names,
                    current,
                    refs,
                    remote_branches,
                    refs_by_commit,
                    remotes,
                ) {
                    (
                        Ok(names),
                        Ok(current),
                        Ok(refs),
                        Ok(remote_branches),
                        Ok(refs_by_commit),
                        Ok(remotes),
                    ) => {
                        *service.branches.borrow_mut() = names;
                        *service.ref_names.borrow_mut() = refs;
                        *service.current_branch.borrow_mut() = current.unwrap_or_default();
                        *service.remote_branches.borrow_mut() = remote_branches;
                        *service.refs_by_commit.borrow_mut() = refs_by_commit;
                        *service.remotes.borrow_mut() = remotes;
                        service.as_mut().branch_changed();
                    }
                    (Err(err), ..)
                    | (_, Err(err), ..)
                    | (_, _, Err(err), ..)
                    | (_, _, _, Err(err), ..)
                    | (_, _, _, _, Err(err), _)
                    | (_, _, _, _, _, Err(err)) => {
                        let result = to_ffi_result(&err);
                        service.as_mut().vcs_failed(result);
                    }
                }
            });
        });
    }

    pub fn ref_names(&self) -> Vec<ffi::FfiBranch> {
        self.ref_names
            .borrow()
            .iter()
            .map(|name| ffi::FfiBranch {
                name: QString::from(name.as_str()),
            })
            .collect()
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

    pub fn remote_branches(&self) -> Vec<ffi::FfiBranch> {
        self.remote_branches
            .borrow()
            .iter()
            .map(|name| ffi::FfiBranch {
                name: QString::from(name.as_str()),
            })
            .collect()
    }

    pub fn commit_refs(&self, id: &QString) -> Vec<ffi::FfiRefDecoration> {
        match self.refs_by_commit.borrow().get(&id.to_string()) {
            Some(refs) => refs
                .iter()
                .map(|r| ffi::FfiRefDecoration {
                    name: QString::from(r.name.as_str()),
                    kind: to_ffi_ref_kind(r.kind),
                })
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn remotes(&self) -> Vec<ffi::FfiRemoteInfo> {
        self.remotes
            .borrow()
            .iter()
            .map(|r| ffi::FfiRemoteInfo {
                name: QString::from(r.name.as_str()),
                url: QString::from(r.url.as_str()),
            })
            .collect()
    }

    pub fn checkout(mut self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.checkout(&name);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => {
                    // A full `refreshBranches` round trip, not a bare
                    // `branchChanged()`: `current_branch`/`branches` are
                    // cached RefCells this job never touched directly, so
                    // a listener re-reading them right after this signal
                    // (the branch popup's own `populate`, the status-bar
                    // widget) would see the answer from before the
                    // checkout.
                    service.as_mut().refresh_branches();
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
                // See `checkout`'s own comment on why this is a full
                // refresh, not a bare signal.
                Ok(()) => service.as_mut().refresh_branches(),
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
                Ok(()) => service.as_mut().refresh_branches(),
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
                    service.as_mut().refresh_branches();
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
                Ok(()) => service.as_mut().refresh_branches(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn push_tracking(mut self: Pin<&mut Self>) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.push_tracking();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_branches(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    /// Shared body of `merge`/`rebase`/`cherryPick`/`revertCommit`: run
    /// `op` on the worker, then either refresh (`statusChanged` picks up
    /// any conflict the Changes dock's "Merge Conflicts" group shows) or
    /// report `vcsFailed`.
    fn run_integration(
        mut self: Pin<&mut Self>,
        op: impl FnOnce(&vcs_core::Repository) -> Result<(), vcs_core::VcsError> + Send + 'static,
    ) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = op(&worker.repo);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => {
                    service.as_mut().refresh_branches();
                    service.as_mut().refresh_status();
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn merge(self: Pin<&mut Self>, branch: &QString) {
        let branch = branch.to_string();
        self.run_integration(move |repo| repo.merge(&branch));
    }

    pub fn rebase(self: Pin<&mut Self>, onto: &QString) {
        let onto = onto.to_string();
        self.run_integration(move |repo| repo.rebase(&onto));
    }

    pub fn cherry_pick(self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        self.run_integration(move |repo| repo.cherry_pick(&id));
    }

    pub fn revert_commit(self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        self.run_integration(move |repo| repo.revert_commit(&id));
    }

    pub fn reset_to(self: Pin<&mut Self>, id: &QString, mode: ffi::FfiResetMode) {
        let id = id.to_string();
        let mode = to_ffi_reset_mode(mode);
        self.run_integration(move |repo| repo.reset_to(&id, mode));
    }

    pub fn rename_branch(mut self: Pin<&mut Self>, old: &QString, new_name: &QString) {
        let old = old.to_string();
        let new_name = new_name.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.rename_branch(&old, &new_name);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_branches(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn stash_push(mut self: Pin<&mut Self>, message: &QString) {
        let message = message.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let msg = if message.is_empty() {
                None
            } else {
                Some(message.as_str())
            };
            let result = worker.repo.stash_push(msg);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn stash_pop(mut self: Pin<&mut Self>, index: u32) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.stash_pop(index as usize);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn stash_drop(mut self: Pin<&mut Self>, index: u32) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.stash_drop(index as usize);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(()) => service.as_mut().refresh_status(),
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn request_stash_list(mut self: Pin<&mut Self>) {
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = worker.repo.stash_list();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(entries) => {
                    let entries: Vec<ffi::FfiStashEntry> = entries
                        .iter()
                        .map(|e| ffi::FfiStashEntry {
                            index: e.index as u32,
                            message: QString::from(e.message.as_str()),
                        })
                        .collect();
                    service.as_mut().stash_list_ready(entries);
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    /// `commitLog`, filtered — same shape as `commit_log` but through
    /// `Repository::log_filtered` (a `git log` round trip) whenever the
    /// filter is non-empty, and `HistoryCache::log` (in-process, cached)
    /// otherwise, so a cleared filter bar is exactly as cheap as
    /// `commitLog` always was.
    pub fn commit_log_filtered(
        mut self: Pin<&mut Self>,
        author: &QString,
        path: &QString,
        text: &QString,
        since: i64,
        until: i64,
        max: u32,
    ) {
        let filter = vcs_core::LogFilter {
            author: non_empty(author),
            path: non_empty(path).map(std::path::PathBuf::from),
            text: non_empty(text),
            since: (since != i64::MIN).then_some(since),
            until: (until != i64::MIN).then_some(until),
        };
        let max = if max == 0 { HISTORY_MAX } else { max as usize };
        let qt_thread = self.as_mut().qt_thread();
        self.as_ref().push_job(move |worker: &VcsWorker| {
            let result = if filter.is_empty() {
                worker.history_cache.log(&worker.repo, Some(max))
            } else {
                worker.repo.log_filtered(&filter, Some(max))
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(entries) => {
                    let entries: Vec<ffi::FfiLogEntry> = to_ffi_log_entries(&entries);
                    service.as_mut().commit_log_ready(entries);
                }
                Err(err) => {
                    let result = to_ffi_result(&err);
                    service.as_mut().vcs_failed(result);
                }
            });
        });
    }

    pub fn blame_enabled_for(&self, path: &QString) -> bool {
        let root = self.project_root.borrow();
        let path = path.to_string();
        if root.is_empty() || path.is_empty() {
            return false;
        }
        let relative = repo_relative_string(root.as_str(), &path);
        app_config::vcs_local_settings::load(Path::new(root.as_str()))
            .map(|settings| settings.blame_enabled(&relative))
            .unwrap_or(false)
    }

    pub fn set_blame_enabled_for(&self, path: &QString, enabled: bool) {
        let root = self.project_root.borrow();
        let path = path.to_string();
        if root.is_empty() || path.is_empty() {
            return;
        }
        let relative = repo_relative_string(root.as_str(), &path);
        let _ = app_config::vcs_local_settings::update(Path::new(root.as_str()), |settings| {
            settings.set_blame_enabled(&relative, enabled);
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
                    .file_history(&worker.repo, &relative, Some(HISTORY_MAX));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(entries) => {
                    let entries: Vec<ffi::FfiLogEntry> = to_ffi_log_entries(&entries);
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
                    let entries: Vec<ffi::FfiLogEntry> = to_ffi_log_entries(&entries);
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
            let result = worker.blame_cache.blame(&worker.repo, &relative);
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
