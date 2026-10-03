//! `AnalysisService` (the PHP tooling plan's B8): runs analyzer jobs on
//! worker threads through `analysis_core::Scheduler`, and publishes what
//! they find into the shared `diagnostics_core::DiagnosticStore` —
//! `DiagnosticsServiceRust`'s store, reused rather than duplicated (A4).
//!
//! `AnalysisEditor` (B9's Analysis settings page) lives in this module too:
//! it edits the same `[analysis]` overrides `AnalysisService::analyzer_rows`
//! reads, following `LanguageServerEditor`'s begin_edit(scope)/rows/set_*/
//! is_dirty/commit shape exactly (`bridge::settings`).
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

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::errors;
use crate::bridge::ffi;
use crate::bridge::registry::SharedDiagnostics;
use settings_model::analysis::FileEvent;

/// A project-wide run can take much longer than a per-file one; generous
/// rather than tuned. `run_manual`'s "serialized, never concurrent" rule
/// means a slow tool only blocks its own turn in this batch's queue, not
/// the editor.
const MANUAL_RUN_TIMEOUT: Duration = Duration::from_secs(180);

/// A per-file run analyses one file on a keystroke or save; a tool that has
/// not answered in this long is stuck, not thorough.
const FILE_RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// A resolved analyzer program and the argv prefix that runs it under the
/// configured interpreter.
type Launch = (PathBuf, Vec<String>);

/// Each analyzer's program as found on its host, a miss included.
///
/// Locked only on worker threads, never on the Qt thread: a lookup holds
/// the lock while it probes (a `compose run` container per probe can take
/// a second), which is also what keeps two triggers from probing the same
/// analyzer twice.
type SharedLaunchCache = Arc<Mutex<analysis_core::LaunchCache<PathBuf>>>;

/// One analyzer of an "Inspect Project" batch, its program already found.
struct QueuedRun {
    analyzer: analysis_core::AnalyzerDef,
    host: process_exec::host::ExecHost,
    launch: Launch,
}

/// One per-file run, ready for the scheduler: the program found, the argv
/// built, and the buffer handed over the way the manifest reads it.
struct FileRun {
    analyzer: analysis_core::AnalyzerDef,
    host: process_exec::host::ExecHost,
    program: PathBuf,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
    /// The temp copy a `temp-copy` tool reads, deleted when the run ends.
    guard: Option<analysis_core::TempCopyGuard>,
}

impl FileRun {
    fn prepare(
        analyzer: analysis_core::AnalyzerDef,
        host: process_exec::host::ExecHost,
        program: &Path,
        interpreter: &str,
        root: &Path,
        path: &Path,
        text: &str,
    ) -> Option<Self> {
        let (program, prefix) = analyzer.invocation(program, interpreter);
        let (target, guard, stdin) = match analyzer.buffer {
            analysis_core::BufferStrategy::Stdin => {
                (path.to_path_buf(), None, Some(text.as_bytes().to_vec()))
            }
            analysis_core::BufferStrategy::SavedOnly => (path.to_path_buf(), None, None),
            analysis_core::BufferStrategy::TempCopy => {
                let guard = analysis_core::write_temp_copy(root, path, text.as_bytes()).ok()?;
                (guard.path().to_path_buf(), Some(guard), None)
            }
        };
        let args = [prefix, analyzer.file_run_args(&target)].concat();
        Some(Self {
            analyzer,
            host,
            program,
            args,
            stdin,
            guard,
        })
    }
}

/// `analyzer`'s program on `host`, from `cache` or looked up once.
fn cached_program(
    cache: &mut analysis_core::LaunchCache<PathBuf>,
    root: &Path,
    analyzer_id: &str,
    candidates: &[String],
    host: &process_exec::host::ExecHost,
) -> Option<PathBuf> {
    cache.get_or_resolve(root, analyzer_id, &format!("{host:?}"), || {
        analysis_core::find_program_on(candidates, root, host)
    })
}

/// Rust side of the `AnalysisService` QObject.
#[derive(Default)]
pub struct AnalysisServiceRust {
    scheduler: analysis_core::Scheduler,
    /// Analyzers still to run in the current "Inspect Project" batch —
    /// popped one at a time so `Scheduler::run_manual`'s "serialized, never
    /// concurrent" rule holds across analyzers too, not only within one.
    queue: RefCell<VecDeque<QueuedRun>>,
    /// An "Inspect Project" batch is still looking its programs up.
    resolving: Cell<bool>,
    store: SharedDiagnostics,
    /// Each analyzer's program per host (a miss is cached too): per-file
    /// runs fire on every debounced edit, and finding a program spawns
    /// `wsl.exe` on a WSL root or a container in a `run`-mode interpreter.
    /// Shared by the status pass, the per-file runs and the project runs,
    /// so a lookup happens once per (analyzer, host) until `LaunchCache`
    /// invalidates it (Composer files changed, a run reports the program
    /// gone or the host unreachable, another project).
    program_cache: SharedLaunchCache,
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

/// One `FfiAnalyzerRow` per `draft` row: joins its configuration to its
/// live detection status against `root` (`analysis_core::status`'s PATH
/// lookup, composer.json parse, and — on a WSL root — `ExecHost`'s
/// `wsl.exe` probe, `process_exec::host::resolve_program`).
///
/// A free function, not a method, so it can run identically on the Qt
/// thread (`analyzer_rows`, the settings page's always-fresh getter) or on
/// a worker thread (`refresh_analyzer_status_async`, the status label's
/// off-thread path) — the same split `AppSession`'s `install_*` methods use
/// between "do it yourself" and "hand me the already-computed answer".
fn build_analyzer_rows(
    root: &Path,
    settings: &app_config::Settings,
    draft: &settings_model::analysis::AnalysisDraft,
    contributions: &[plugin_api::AnalyzerContribution],
    mut find: impl FnMut(&str, &[String], &process_exec::host::ExecHost) -> Option<PathBuf>,
) -> Vec<ffi::FfiAnalyzerRow> {
    draft
        .rows()
        .iter()
        .map(|row| {
            let contribution = contributions.iter().find(|c| c.id == row.id);
            let candidates = contribution
                .map(|c| c.program_candidates.clone())
                .unwrap_or_default();
            // Which Composer package explains a tool that resolves to
            // nothing: the manifest's own `composer-package`.
            let packages: Vec<&str> = contribution
                .and_then(|c| c.composer_package.as_deref())
                .into_iter()
                .collect();
            let host = crate::bridge::php::tool_host(
                contribution.and_then(|c| c.requires_interpreter.as_deref()),
                settings,
                root,
            );
            let found = find(&row.id, &candidates, &host);
            let status = analysis_core::status_from(found, root, &packages);
            let status_kind = match status {
                analysis_core::AnalyzerStatus::Detected { .. } => {
                    ffi::FfiAnalyzerStatusKind::Detected
                }
                analysis_core::AnalyzerStatus::DeclaredNotInstalled { .. } => {
                    ffi::FfiAnalyzerStatusKind::DeclaredNotInstalled
                }
                analysis_core::AnalyzerStatus::NotDetected => {
                    ffi::FfiAnalyzerStatusKind::NotDetected
                }
            };
            ffi::FfiAnalyzerRow {
                id: QString::from(row.id.as_str()),
                name: QString::from(row.name.as_str()),
                enabled: row.enabled,
                trigger_id: QString::from(row.trigger.id()),
                trigger_label: QString::from(row.trigger.label()),
                status_kind,
                status_text: QString::from(status.describe(&row.name).as_str()),
            }
        })
        .collect()
}

impl ffi::AnalysisService {
    /// One row per contributed analyzer: its configuration (enabled,
    /// trigger) and its live detection status against the open project.
    /// The Analysis settings page (B9) reads this — always a fresh
    /// detection pass, so a plugin enabled/disabled or a tool installed
    /// mid-session shows up without a restart. The status bar's label
    /// reads `refreshAnalyzerStatusAsync`'s cached answer instead
    /// (`analyzerStatusReady`), not this: opening the settings page is a
    /// deliberate, occasional action, unlike every `projectOpened`.
    pub fn analyzer_rows(&self) -> Vec<ffi::FfiAnalyzerRow> {
        let Some(root) = current_project_root() else {
            return Vec::new();
        };
        let draft = analysis_draft();
        let contributions = contributed_analyzers();
        let settings = crate::bridge::convert::load_resolved_settings();
        build_analyzer_rows(
            &root,
            &settings,
            &draft,
            &contributions,
            |_, candidates, host| analysis_core::find_program_on(candidates, &root, host),
        )
    }

    /// `analyzer_rows`'s answer, computed off the Qt thread and delivered
    /// via `analyzerStatusReady` (fast-project-open-plan step 5).
    ///
    /// `analysis_core::status`'s program-candidate probe used to run
    /// inline as part of the `projectOpened` slot chain; on a WSL project
    /// root it spawns `wsl.exe` per candidate
    /// (`process_exec::host::resolve_program`), which can freeze the Qt
    /// thread for seconds. The result crosses back guarded by a stale-root
    /// check — the same shape `ProjectTreeModel::start_watcher_async` uses
    /// for its own off-thread result — so a project switched away from
    /// while this was running never overwrites the new project's own
    /// label.
    pub fn refresh_analyzer_status_async(mut self: Pin<&mut Self>) {
        let Some(root) = current_project_root() else {
            return;
        };
        let contributions = contributed_analyzers();
        let cache = self.program_cache.clone();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            // The explicit-root, cache-backed reader (ADR-0037's
            // `SearchModel::open_index` shape): `load_resolved_settings`
            // reads the thread-local `shared_session`, sound only on the
            // Qt thread.
            let settings = crate::bridge::convert::load_resolved_settings_for(&root);
            let draft = settings_model::analysis::AnalysisDraft::new(&settings, &contributions);
            let rows = {
                let mut cache = cache.lock().expect("launch cache");
                build_analyzer_rows(&root, &settings, &draft, &contributions, |id, c, host| {
                    cached_program(&mut cache, &root, id, c, host)
                })
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if current_project_root().as_deref() != Some(root.as_path()) {
                    return;
                }
                service.as_mut().analyzer_status_ready(rows);
            });
        });
    }

    /// Whether a project-wide run is currently in flight — the "Inspect
    /// Project" action's enablement (B9), rather than the view tracking it
    /// from signals alone.
    pub fn is_inspecting(&self) -> bool {
        self.resolving.get()
            || !self.queue.borrow().is_empty()
            || self.scheduler.is_manual_run_in_progress()
    }

    /// Run every enabled, detected analyzer against the whole open project
    /// (`Trigger::Manual`), one at a time. Findings replace each
    /// analyzer's previous rows in the shared store as each one finishes,
    /// so the Problems dock and the editor's squiggles fill in
    /// analyzer-by-analyzer rather than waiting for the slowest one.
    pub fn inspect_project(mut self: Pin<&mut Self>) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return errors::failure(errors::CODE_NO_PROJECT, "no project is open");
        };
        if self.is_inspecting() {
            return errors::failure(
                errors::CODE_REFUSED,
                "a project-wide analysis is already running",
            );
        }

        let draft = analysis_draft();
        let settings = crate::bridge::convert::load_resolved_settings();
        let interpreter = settings_model::php::resolve(&settings).interpreter;
        let planned: Vec<(analysis_core::AnalyzerDef, process_exec::host::ExecHost)> =
            contributed_analyzers()
                .iter()
                .filter(|c| draft.row(&c.id).is_some_and(|row| row.enabled))
                .map(|c| {
                    let host = crate::bridge::php::tool_host(
                        c.requires_interpreter.as_deref(),
                        &settings,
                        &root,
                    );
                    let def =
                        analysis_core::AnalyzerDef::from_contribution(c).with_ruleset_for(&root);
                    (def, host)
                })
                .collect();

        // Which of them are installed is a program lookup per analyzer —
        // a container start each in a `run`-mode interpreter — so it runs
        // off the Qt thread, and the batch starts when it is done.
        self.resolving.set(true);
        self.as_mut().analysis_started();
        let cache = self.program_cache.clone();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let runs: VecDeque<QueuedRun> = {
                let mut cache = cache.lock().expect("launch cache");
                planned
                    .into_iter()
                    .filter_map(|(analyzer, host)| {
                        let program = cached_program(
                            &mut cache,
                            &root,
                            &analyzer.id,
                            &analyzer.program_candidates,
                            &host,
                        )?;
                        let launch = analyzer.invocation(&program, &interpreter);
                        Some(QueuedRun {
                            analyzer,
                            host,
                            launch,
                        })
                    })
                    .collect()
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                service.resolving.set(false);
                if runs.is_empty() {
                    service.as_mut().analysis_finished();
                    service.as_mut().inspect_refused(QString::from(
                        "no enabled, installed analyzer is available for this project",
                    ));
                    return;
                }
                *service.queue.borrow_mut() = runs;
                service.run_next(root);
            });
        });
        ffi::FfiResult::default()
    }

    /// The editor's debounced buffer change: run every analyzer whose
    /// trigger is On Type against `text`.
    pub fn file_changed(self: Pin<&mut Self>, path: &QString, text: &QString) {
        self.schedule_file_analysis(FileEvent::Edit, path, text);
    }

    /// The editor wrote `path` to disk: run every analyzer whose trigger is
    /// On Type or On Save.
    pub fn file_saved(self: Pin<&mut Self>, path: &QString, text: &QString) {
        self.schedule_file_analysis(FileEvent::Save, path, text);
    }

    /// Which analyzers fire is `settings_model::analysis::file_jobs`'s rule;
    /// this only resolves each one's program, hands it the buffer the way
    /// its manifest says it can read one, and publishes what comes back.
    ///
    /// Finding a program can mean a container start, so the lookups (and
    /// the temp copies) happen on a worker that holds the launch cache for
    /// the whole trigger and queues the runs back before letting go: two
    /// triggers never probe the same analyzer twice, and their runs reach
    /// the scheduler in trigger order.
    fn schedule_file_analysis(
        mut self: Pin<&mut Self>,
        event: FileEvent,
        path: &QString,
        text: &QString,
    ) {
        let Some(root) = current_project_root() else {
            return;
        };
        let path = PathBuf::from(path.to_string());
        if !path.starts_with(&root) {
            return;
        }
        let language_id = syntax_core::language_for_path(&path).id();
        let contributions = contributed_analyzers();
        let settings = crate::bridge::convert::load_resolved_settings();
        let interpreter = settings_model::php::resolve(&settings).interpreter;
        let draft = settings_model::analysis::AnalysisDraft::new(&settings, &contributions);
        let planned: Vec<(analysis_core::AnalyzerDef, process_exec::host::ExecHost)> =
            settings_model::analysis::file_jobs(event, &language_id, &draft, &contributions)
                .into_iter()
                .filter_map(|job| contributions.iter().find(|c| c.id == job.analyzer_id))
                .map(|c| {
                    let host = crate::bridge::php::tool_host(
                        c.requires_interpreter.as_deref(),
                        &settings,
                        &root,
                    );
                    let def =
                        analysis_core::AnalyzerDef::from_contribution(c).with_ruleset_for(&root);
                    (def, host)
                })
                .collect();
        if planned.is_empty() {
            return;
        }
        let text = text.to_string();
        let cache = self.program_cache.clone();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let mut locked = cache.lock().expect("launch cache");
            let runs: Vec<FileRun> = planned
                .into_iter()
                .filter_map(|(analyzer, host)| {
                    let program = cached_program(
                        &mut locked,
                        &root,
                        &analyzer.id,
                        &analyzer.program_candidates,
                        &host,
                    )?;
                    FileRun::prepare(analyzer, host, &program, &interpreter, &root, &path, &text)
                })
                .collect();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                for run in runs {
                    service.as_mut().start_file_run(run, &root, &path);
                }
            });
            drop(locked);
        });
    }

    /// Hand one prepared per-file run to the scheduler.
    fn start_file_run(mut self: Pin<&mut Self>, run: FileRun, root: &Path, path: &Path) {
        let FileRun {
            analyzer,
            host,
            program,
            args,
            stdin,
            guard,
        } = run;
        let qt_thread = self.as_mut().qt_thread();
        let cache = self.program_cache.clone();
        let file = path.to_path_buf();
        self.scheduler.schedule_file_run(
            &analyzer.id.clone(),
            host,
            program,
            args,
            root,
            path,
            stdin,
            Duration::ZERO,
            FILE_RUN_TIMEOUT,
            move |result| {
                drop(guard); // the run is over; delete the temp copy
                if let Err(failure) = &result {
                    cache
                        .lock()
                        .expect("launch cache")
                        .note_failure(&analyzer.id, failure);
                }
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::AnalysisService>| {
                    publish_file_result(&service, &analyzer, &result, &file);
                    service.as_mut().diagnostics_changed();
                });
            },
        );
    }

    /// Pop and run the next queued analyzer, or announce the batch is
    /// finished when the queue is empty.
    fn run_next(mut self: Pin<&mut Self>, root: PathBuf) {
        let Some(QueuedRun {
            analyzer,
            host,
            launch: (program, prefix),
        }) = self.queue.borrow_mut().pop_front()
        else {
            self.as_mut().analysis_finished();
            return;
        };
        let args = [prefix, analyzer.project_run_args(&root)].concat();

        let qt_thread = self.as_mut().qt_thread();
        let id = analyzer.id.clone();
        self.as_mut().analyzer_started(QString::from(id.as_str()));
        let root_for_next = root.clone();
        let root_for_publish = root.clone();
        let host_for_publish = host.clone();
        let cache = self.program_cache.clone();
        let started = self.scheduler.run_manual(
            host,
            program,
            args,
            &root,
            MANUAL_RUN_TIMEOUT,
            move |result| {
                if let Err(failure) = &result {
                    cache
                        .lock()
                        .expect("launch cache")
                        .note_failure(&analyzer.id, failure);
                }
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::AnalysisService>| {
                    publish_result(
                        &service,
                        &analyzer,
                        &result,
                        &root_for_publish,
                        &host_for_publish,
                    );
                    // The store just changed for this analyzer's rows —
                    // tell the editor and the Problems dock, the same
                    // signal `LanguageService`/`BuildService` emit for
                    // their own writes (ADR-0046).
                    service.as_mut().diagnostics_changed();
                    let (ok, message) = match &result {
                        Ok(_) => (true, String::new()),
                        Err(analysis_core::RunFailure::NotFound) => {
                            (false, format!("{id}: program not found"))
                        }
                        Err(analysis_core::RunFailure::TimedOut) => {
                            (false, format!("{id}: timed out"))
                        }
                        Err(analysis_core::RunFailure::Io(msg)) => (false, format!("{id}: {msg}")),
                    };
                    service.as_mut().analyzer_finished(
                        QString::from(id.as_str()),
                        ok,
                        QString::from(message.as_str()),
                    );
                    service.run_next(root_for_next);
                });
            },
        );
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
    root: &Path,
    host: &process_exec::host::ExecHost,
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
        let local = analysis_core::locate_file_on(host, root, file);
        let uri = diagnostics_core::uri_from_path(&local.to_string_lossy());
        store.replace(&key, &uri, diagnostics);
    }
}

/// Publish a single-file run's findings as `file`'s rows for this analyzer,
/// leaving every other file's rows alone. The tool analysed exactly one
/// file (possibly under a temp-copy name), so every finding belongs to it
/// whatever path the report spells.
fn publish_file_result(
    service: &ffi::AnalysisService,
    analyzer: &analysis_core::AnalyzerDef,
    result: &analysis_core::RunResult,
    file: &Path,
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
    let reported: BTreeSet<&str> = findings.iter().map(|f| f.file.as_str()).collect();
    let diagnostics = reported
        .into_iter()
        .flat_map(|reported| analysis_core::to_diagnostics(&findings, reported, analyzer))
        .collect();
    let uri = diagnostics_core::uri_from_path(&file.to_string_lossy());
    service
        .store
        .borrow_mut()
        .replace(&source_key(&analyzer.id), &uri, diagnostics);
}

/// Rust side of the Analysis settings page's `AnalysisEditor` QObject (B9).
#[derive(Default)]
pub struct AnalysisEditorRust {
    draft: RefCell<Option<settings_model::analysis::AnalysisDraft>>,
    /// What was saved when the page opened, so `is_dirty` can tell a row
    /// the user edited from one they did not, the same reason
    /// `LanguageServerEditorRust::saved` exists.
    saved: RefCell<Option<settings_model::analysis::AnalysisDraft>>,
    /// The layer this draft came from and will be written back to
    /// (ADR-0022).
    scope: RefCell<settings_model::Scope>,
}

impl ffi::AnalysisEditor {
    /// Load the draft from `scope` — `"global"` or `"project"`. Same shape
    /// as `LanguageServerEditor::begin_edit`: the project's analyzer list
    /// is lifted into an otherwise-default `Settings` and lowered back out
    /// on commit.
    pub fn begin_edit(&self, scope: &QString) {
        let scope = crate::bridge::settings::scope_from_name(&scope.to_string());
        *self.scope.borrow_mut() = scope;
        let settings = match scope {
            settings_model::Scope::Project => app_config::Settings {
                analysis: app_config::AnalysisSettings {
                    analyzers: crate::bridge::convert::load_project_settings()
                        .analysis
                        .unwrap_or_default(),
                },
                ..app_config::Settings::default()
            },
            _ => app_config::load(&app_core::resolve_config_dir()).unwrap_or_default(),
        };
        let draft =
            settings_model::analysis::AnalysisDraft::new(&settings, &contributed_analyzers());
        *self.saved.borrow_mut() = Some(draft.clone());
        *self.draft.borrow_mut() = Some(draft);
    }

    pub fn rows(&self) -> Vec<ffi::FfiAnalysisRow> {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return Vec::new();
        };
        draft
            .rows()
            .iter()
            .map(|row| ffi::FfiAnalysisRow {
                id: QString::from(row.id.as_str()),
                name: QString::from(row.name.as_str()),
                enabled: row.enabled,
                trigger_id: QString::from(row.trigger.id()),
                trigger_label: QString::from(row.trigger.label()),
            })
            .collect()
    }

    pub fn set_enabled(&self, id: &QString, enabled: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_enabled(&id.to_string(), enabled);
        }
    }

    pub fn set_trigger(&self, id: &QString, trigger_id: &QString) {
        let Some(trigger) = settings_model::analysis::Trigger::from_id(&trigger_id.to_string())
        else {
            return;
        };
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_trigger(&id.to_string(), trigger);
        }
    }

    pub fn is_dirty(&self, id: &QString) -> bool {
        let id = id.to_string();
        let draft = self.draft.borrow();
        let saved = self.saved.borrow();
        match (draft.as_ref(), saved.as_ref()) {
            (Some(draft), Some(saved)) => draft.row(&id) != saved.row(&id),
            _ => false,
        }
    }

    pub fn commit(&self) {
        let Some(draft) = self.draft.borrow().clone() else {
            return;
        };
        if *self.scope.borrow() == settings_model::Scope::Project {
            let analyzers = draft.overrides();
            let _ = crate::bridge::settings::commit_to_project(move |project| {
                project.analysis = Some(analyzers);
            });
            *self.saved.borrow_mut() = Some(draft);
            return;
        }
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        draft.apply_to(&mut settings);
        let _ = app_config::save(&config_dir, &settings);
        *self.saved.borrow_mut() = Some(draft);
    }
}
