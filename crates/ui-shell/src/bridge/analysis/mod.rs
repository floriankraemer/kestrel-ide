//! `AnalysisService` (the PHP tooling plan's B8): runs analyzer jobs on
//! worker threads through `analysis_core::Scheduler`, and publishes what
//! they find into the shared `diagnostics_core::DiagnosticStore` —
//! `DiagnosticsServiceRust`'s store, reused rather than duplicated (A4).
//!
//! # Threading
//!
//! One registered `#[qobject]` (ADR-0032): `Scheduler::run_manual` spawns
//! its own worker thread per analyzer, and this adapter chains one
//! analyzer's completion into the next's start on the Qt thread via
//! `CxxQtThread::queue()`, the same pattern `BuildService` (`bridge::build`)
//! uses for its single long-running process.
//!
//! Translation only, per `docs/architecture/layering.md`: which analyzers
//! exist, how to detect them, and how to parse their output are
//! `analysis-core`'s and `plugin-api`'s; this adapter only gathers the
//! contributions, resolves settings, and moves bytes between them. Errors
//! cross the seam as `FfiResult`'s typed code + message (ADR-0003) — never
//! a bare `QString` sentinel.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::errors;
use crate::bridge::ffi;
use crate::bridge::registry::SharedDiagnostics;

/// A project-wide run can take much longer than a per-file one; generous
/// rather than tuned. `run_manual`'s "serialized, never concurrent" rule
/// means a slow tool only blocks its own turn in this batch's queue, not
/// the editor.
const MANUAL_RUN_TIMEOUT: Duration = Duration::from_secs(180);

/// Rust side of the `AnalysisService` QObject.
#[derive(Default)]
pub struct AnalysisServiceRust {
    scheduler: analysis_core::Scheduler,
    /// Analyzers still to run in the current "Inspect Project" batch —
    /// popped one at a time so `Scheduler::run_manual`'s "serialized, never
    /// concurrent" rule holds across analyzers too, not only within one.
    queue: RefCell<VecDeque<analysis_core::AnalyzerDef>>,
    store: SharedDiagnostics,
}

fn current_project_root() -> Option<PathBuf> {
    crate::bridge::convert::current_project_root()
}

/// This source's key in the shared store (ADR-0046): distinct from a
/// language server's or a build's, so an analyzer's rows for a file never
/// clobber (or get clobbered by) either.
fn source_key(analyzer_id: &str) -> String {
    format!("analysis:{analyzer_id}")
}

/// Every analyzer any loaded plugin contributes, gathered fresh on every
/// call so a plugin enabled or disabled mid-session is picked up without a
/// restart — the same freshness `bridge::language::plugin_servers` gives
/// language servers.
fn contributed_analyzers() -> Vec<plugin_api::AnalyzerContribution> {
    plugin_host::registry()
        .analyzers()
        .map(|(_, analyzer)| analyzer.clone())
        .collect()
}

fn analysis_draft() -> settings_model::analysis::AnalysisDraft {
    settings_model::analysis::AnalysisDraft::new(
        &crate::bridge::convert::load_resolved_settings(),
        &contributed_analyzers(),
    )
}

impl ffi::AnalysisService {
    /// One row per contributed analyzer: its configuration (enabled,
    /// trigger) and its live detection status against the open project.
    /// The status bar and the Analysis settings page (B9) both read this
    /// rather than keeping their own copies.
    pub fn analyzer_rows(&self) -> Vec<ffi::FfiAnalyzerRow> {
        let Some(root) = current_project_root() else {
            return Vec::new();
        };
        let draft = analysis_draft();
        let contributions = contributed_analyzers();
        draft
            .rows()
            .iter()
            .map(|row| {
                let candidates = contributions
                    .iter()
                    .find(|c| c.id == row.id)
                    .map(|c| c.program_candidates.clone())
                    .unwrap_or_default();
                // Composer-package matching (which package name explains a
                // tool that resolves to nothing) is C2's job, the PHP-
                // specific manifest rows; until then such a tool reports
                // `NotDetected` rather than `DeclaredNotInstalled`.
                let status = analysis_core::status(&candidates, &root, &[]);
                ffi::FfiAnalyzerRow {
                    id: QString::from(row.id.as_str()),
                    name: QString::from(row.name.as_str()),
                    enabled: row.enabled,
                    trigger_id: QString::from(row.trigger.id()),
                    trigger_label: QString::from(row.trigger.label()),
                    status_text: QString::from(status.describe(&row.name).as_str()),
                }
            })
            .collect()
    }

    /// Whether a project-wide run is currently in flight — the "Inspect
    /// Project" action's enablement (B9), rather than the view tracking it
    /// from signals alone.
    pub fn is_inspecting(&self) -> bool {
        !self.queue.borrow().is_empty() || self.scheduler.is_manual_run_in_progress()
    }

    /// Run every enabled, detected analyzer against the whole open project
    /// (`Trigger::Manual`), one at a time. Findings replace each
    /// analyzer's previous rows in the shared store as each one finishes,
    /// so the Problems dock and the editor's squiggles fill in
    /// analyzer-by-analyzer rather than waiting for the slowest one.
    pub fn inspect_project(self: Pin<&mut Self>) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return errors::failure(errors::CODE_NO_PROJECT, "no project is open");
        };
        if self.scheduler.is_manual_run_in_progress() || !self.queue.borrow().is_empty() {
            return errors::failure(
                errors::CODE_REFUSED,
                "a project-wide analysis is already running",
            );
        }

        let draft = analysis_draft();
        let defs: VecDeque<analysis_core::AnalyzerDef> = contributed_analyzers()
            .iter()
            .filter(|c| draft.row(&c.id).is_some_and(|row| row.enabled))
            .filter(|c| analysis_core::find_program(&c.program_candidates, &root).is_some())
            .map(analysis_core::AnalyzerDef::from_contribution)
            .collect();

        if defs.is_empty() {
            return errors::failure(
                errors::CODE_REFUSED,
                "no enabled, installed analyzer is available for this project",
            );
        }

        *self.queue.borrow_mut() = defs;
        let mut this = self;
        this.as_mut().analysis_started();
        this.run_next(root);
        ffi::FfiResult::default()
    }

    /// Pop and run the next queued analyzer, or announce the batch is
    /// finished when the queue is empty.
    fn run_next(mut self: Pin<&mut Self>, root: PathBuf) {
        let Some(analyzer) = self.queue.borrow_mut().pop_front() else {
            self.as_mut().analysis_finished();
            return;
        };
        let Some(program) = analysis_core::find_program(&analyzer.program_candidates, &root) else {
            // Detected when the batch was built, gone by the time its turn
            // came (uninstalled mid-run) — skip it rather than fail the
            // whole batch over one analyzer.
            return self.run_next(root);
        };
        let mut args = analyzer.args.clone();
        args.push(root.to_string_lossy().into_owned());

        let qt_thread = self.as_mut().qt_thread();
        let id = analyzer.id.clone();
        self.as_mut().analyzer_started(QString::from(id.as_str()));
        let root_for_next = root.clone();
        let started =
            self.scheduler
                .run_manual(program, args, &root, MANUAL_RUN_TIMEOUT, move |result| {
                    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::AnalysisService>| {
                        publish_result(&service, &analyzer, &result);
                        let (ok, message) = match &result {
                            Ok(_) => (true, String::new()),
                            Err(analysis_core::RunFailure::NotFound) => {
                                (false, format!("{id}: program not found"))
                            }
                            Err(analysis_core::RunFailure::TimedOut) => {
                                (false, format!("{id}: timed out"))
                            }
                            Err(analysis_core::RunFailure::Io(msg)) => {
                                (false, format!("{id}: {msg}"))
                            }
                        };
                        service.as_mut().analyzer_finished(
                            QString::from(id.as_str()),
                            ok,
                            QString::from(message.as_str()),
                        );
                        service.run_next(root_for_next);
                    });
                });
        if !started {
            // The scheduler itself refused (should not happen: this
            // adapter is the only caller of `run_manual`), so the batch
            // cannot proceed — report and stop rather than spin.
            self.queue.borrow_mut().clear();
            self.as_mut().analysis_finished();
        }
    }
}

/// Parse `analyzer`'s output (per its `output_format`) and publish the
/// findings into the shared store, grouped by file, replacing whatever
/// this analyzer previously reported.
///
/// Neither an unrecognised format nor a run that exited non-zero is
/// treated as nothing-to-publish here: PHPStan and PHPCS both exit
/// non-zero when they *found* something, which is the normal case, not a
/// failure — "did the run itself fail" is `RunFailure`, already reported
/// by the caller through `analyzerFinished`.
fn publish_result(
    service: &ffi::AnalysisService,
    analyzer: &analysis_core::AnalyzerDef,
    result: &analysis_core::RunResult,
) {
    let Ok(output) = result else {
        return;
    };
    if analyzer.output_format != "checkstyle-xml" {
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Ok(findings) = analysis_core::parse_checkstyle_xml(&text) else {
        return;
    };
    let files: BTreeSet<&str> = findings.iter().map(|f| f.file.as_str()).collect();
    let mut store = service.store.borrow_mut();
    let key = source_key(&analyzer.id);
    store.clear_source(&key);
    for file in files {
        let diagnostics = analysis_core::to_diagnostics(&findings, file, analyzer);
        let uri = diagnostics_core::uri_from_path(file);
        store.replace(&key, &uri, diagnostics);
    }
}
