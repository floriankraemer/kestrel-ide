//! `TestService` (the PHP tooling plan's D4): runs the project's test
//! framework on worker threads through `test_core::runner::run`, streaming
//! TeamCity messages into a `test_core::TestTree`, and publishing failures
//! into the shared `diagnostics_core::DiagnosticStore` —
//! `DiagnosticsServiceRust`'s store, reused rather than duplicated (D3),
//! exactly as `AnalysisServiceRust` (B8) already does for analyzer
//! findings.
//!
//! # Threading
//!
//! One registered `#[qobject]` (ADR-0032) owning a `HashMap` of in-flight
//! runs, mirroring `BuildService`'s shape (`bridge::build`) rather than
//! `AnalysisService`'s queue: a test run is a single process read to
//! completion or a stop, not several short operations to serialize.
//!
//! Translation only, per `docs/architecture/layering.md`: which test
//! framework is contributed, how its output is parsed, and what tree state
//! a message produces are `test-core`'s and `plugin-api`'s; this adapter
//! only detects the framework, builds argv, and moves bytes between them.
//! Errors cross the seam as `FfiResult`'s typed code + message (ADR-0003).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;

use crate::bridge::errors;
use crate::bridge::ffi;
use crate::bridge::registry::SharedDiagnostics;

/// This source's key in the shared store (ADR-0046): distinct from an
/// analyzer's or a build's, so a test failure's rows for a file never
/// clobber (or get clobbered by) either.
fn source_key(framework_name: &str) -> String {
    format!("test:{framework_name}")
}

/// Rust side of the `TestService` QObject.
#[derive(Default)]
pub struct TestServiceRust {
    next_id: Cell<u64>,
    runs: RefCell<HashMap<u64, test_core::TestRunHandle>>,
    /// The current run's live tree. One at a time — the Tests dock shows
    /// one project's one framework's one tree, the same "one at a time"
    /// rule `BuildServiceRust::diagnostics` applies to a build's output.
    tree: RefCell<test_core::TestTree>,
    /// The framework last run, for the diagnostics `source` column and
    /// this store's key. Empty until the first run.
    framework_name: RefCell<String>,
    store: SharedDiagnostics,
}

fn current_project_root() -> Option<PathBuf> {
    crate::bridge::convert::current_project_root()
}

/// Every `test-frameworks` contribution any loaded plugin offers, with the
/// plugin that owns it — gathered fresh on every call so a plugin enabled
/// or disabled mid-session is picked up without a restart, the same
/// freshness `bridge::analysis::contributed_analyzers` gives analyzers.
/// The owner travels alongside its contribution because a run may need to
/// expand `${asset_dir}` in that plugin's own `args` (`junit-gradle`'s
/// `--init-script ${asset_dir}/ide-model.init.gradle`).
fn contributed_frameworks() -> Vec<(
    plugin_host::LoadedPlugin,
    plugin_api::TestFrameworkContribution,
)> {
    plugin_host::registry()
        .test_frameworks()
        .map(|(plugin, framework)| (plugin.clone(), framework.clone()))
        .collect()
}

/// The framework this project's run uses, per `test_core::select_framework`
/// (a Qt-free rule, unit-tested there): the first contribution whose
/// `requires_toolchain` (if any) this project actually has, whose
/// `output_format` this build can stream, and whose program resolves.
/// Reuses `analysis_core::find_program` rather than a second
/// candidate-search, the same reuse the manifest's own D7 note asks for.
fn detect_framework(
    root: &Path,
) -> Option<(
    plugin_host::LoadedPlugin,
    plugin_api::TestFrameworkContribution,
    PathBuf,
)> {
    let owners = contributed_frameworks();
    let contributions: Vec<plugin_api::TestFrameworkContribution> = owners
        .iter()
        .map(|(_, framework)| framework.clone())
        .collect();
    let detected: Vec<String> = run_core::toolchain::detect_toolchains(root)
        .into_iter()
        .map(|id| id.as_str().to_string())
        .collect();
    let detected_refs: Vec<&str> = detected.iter().map(String::as_str).collect();

    let (index, _, program) = test_core::select_framework(
        &contributions,
        &detected_refs,
        test_core::SUPPORTED_OUTPUT_FORMATS,
        |candidates| analysis_core::find_program(candidates, root),
    )?;
    let (plugin, framework) = owners.into_iter().nth(index)?;
    Some((plugin, framework, program))
}

fn to_ffi_kind(kind: test_core::NodeKind) -> ffi::FfiTestNodeKind {
    match kind {
        test_core::NodeKind::Suite => ffi::FfiTestNodeKind::Suite,
        test_core::NodeKind::Test => ffi::FfiTestNodeKind::Test,
    }
}

fn to_ffi_status(status: test_core::TestStatus) -> ffi::FfiTestStatusKind {
    match status {
        test_core::TestStatus::Failed => ffi::FfiTestStatusKind::Failed,
        test_core::TestStatus::Running => ffi::FfiTestStatusKind::Running,
        test_core::TestStatus::Pending => ffi::FfiTestStatusKind::Pending,
        test_core::TestStatus::Passed => ffi::FfiTestStatusKind::Passed,
        test_core::TestStatus::Skipped => ffi::FfiTestStatusKind::Skipped,
    }
}

fn to_ffi_node(node: &test_core::TestNode) -> ffi::FfiTestNode {
    ffi::FfiTestNode {
        id: QString::from(node.id.as_str()),
        parent_id: QString::from(
            node.parent
                .as_ref()
                .map(test_core::TestId::as_str)
                .unwrap_or(""),
        ),
        name: QString::from(node.name.as_str()),
        kind: to_ffi_kind(node.kind),
        status: to_ffi_status(node.status),
        duration_ms: node.duration_ms.map(|ms| ms as i64).unwrap_or(-1),
        has_failure: node.failure.is_some(),
    }
}

impl ffi::TestService {
    pub fn nodes(&self) -> Vec<ffi::FfiTestNode> {
        let tree = self.tree.borrow();
        // Every node the tree knows, walked breadth-first from the roots so
        // a suite always precedes its own children — what a `QTreeWidget`
        // needs in order to build parents before it is asked to parent a
        // child under them.
        let mut ids: Vec<test_core::TestId> = tree.roots().to_vec();
        let mut index = 0;
        while index < ids.len() {
            if let Some(node) = tree.node(&ids[index]) {
                ids.extend(node.children.iter().cloned());
            }
            index += 1;
        }
        ids.iter()
            .filter_map(|id| tree.node(id))
            .map(to_ffi_node)
            .collect()
    }

    pub fn failure_message(&self, node_id: &QString) -> QString {
        let id = test_core::TestId(node_id.to_string());
        let message = self
            .tree
            .borrow()
            .node(&id)
            .and_then(|node| node.failure.as_ref())
            .map(|failure| failure.message.clone())
            .unwrap_or_default();
        QString::from(message.as_str())
    }

    pub fn failure_details(&self, node_id: &QString) -> QString {
        let id = test_core::TestId(node_id.to_string());
        let details = self
            .tree
            .borrow()
            .node(&id)
            .and_then(|node| node.failure.as_ref())
            .map(|failure| failure.details.clone())
            .unwrap_or_default();
        QString::from(details.as_str())
    }

    /// Resolve a `file:line` at `byte_offset` into this node's failure
    /// details, the same contract `RunService::resolve_link` has for
    /// console output — reused directly rather than duplicated, since the
    /// underlying catalogue (`run_core::links::resolve_link`) is exactly
    /// the plan's stated reuse for this pane.
    pub fn resolve_failure_link(
        &self,
        node_id: &QString,
        byte_offset: u32,
    ) -> ffi::FfiResolvedLink {
        let Some(root) = current_project_root() else {
            return ffi::FfiResolvedLink::default();
        };
        let id = test_core::TestId(node_id.to_string());
        let details = self
            .tree
            .borrow()
            .node(&id)
            .and_then(|node| node.failure.as_ref())
            .map(|failure| failure.details.clone())
            .unwrap_or_default();
        match run_core::resolve_link(&details, byte_offset as usize, &root, None) {
            Some(link) => ffi::FfiResolvedLink {
                found: true,
                path: QString::from(link.path.display().to_string().as_str()),
                line: link.line,
                has_column: link.col.is_some(),
                column: link.col.unwrap_or(0),
            },
            None => ffi::FfiResolvedLink::default(),
        }
    }

    pub fn is_running(&self) -> bool {
        !self.runs.borrow().is_empty()
    }

    pub fn run_all(self: Pin<&mut Self>) -> ffi::FfiResult {
        self.start(None)
    }

    pub fn run_failed(self: Pin<&mut Self>) -> ffi::FfiResult {
        let ids = self.tree.borrow().failed_leaf_ids();
        if ids.is_empty() {
            return errors::failure(errors::CODE_REFUSED, "no failed tests to rerun");
        }
        let pattern = test_core::filter::for_many(&ids);
        self.start(Some(pattern))
    }

    pub fn run_node(self: Pin<&mut Self>, node_id: &QString) -> ffi::FfiResult {
        let id = test_core::TestId(node_id.to_string());
        let pattern = test_core::filter::for_node(&self.tree.borrow(), &id);
        self.start(Some(pattern))
    }

    fn start(mut self: Pin<&mut Self>, filter: Option<String>) -> ffi::FfiResult {
        if !self.runs.borrow().is_empty() {
            return errors::failure(errors::CODE_REFUSED, "a test run is already in progress");
        }
        let Some(root) = current_project_root() else {
            return errors::failure(errors::CODE_NO_PROJECT, "no project is open");
        };
        let Some((plugin, framework, program)) = detect_framework(&root) else {
            return errors::failure(
                errors::CODE_REFUSED,
                "no installed test framework is contributed for this project",
            );
        };

        // A test framework's args may name `${asset_dir}` (junit-gradle's
        // `--init-script ${asset_dir}/ide-model.init.gradle`) — the same
        // materialise-on-demand seam A3 built for a sync provider, reused
        // here rather than a second "where do a builtin plugin's files
        // live" mechanism.
        let config_dir = app_core::resolve_config_dir();
        let Ok(asset_dir) = plugin.asset_dir(&config_dir) else {
            return errors::failure(
                errors::CODE_REFUSED,
                "could not materialise this test framework's plugin assets",
            );
        };

        // A full run replaces the tree a previous run left behind; a
        // filtered rerun updates only the nodes it touches, so the rest of
        // the previous run's results stay visible (D6's "reruns exactly
        // the failed node", not "clears the dock").
        if filter.is_none() {
            self.tree.borrow_mut().reset();
        }
        *self.framework_name.borrow_mut() = framework.name.clone();
        // Diagnostics from the framework this run uses are recomputed as
        // events arrive (`republish`); a stale row from a source this
        // project no longer contributes would otherwise never be cleared.
        self.store
            .borrow_mut()
            .clear_source(&source_key(&framework.name));

        let mut args = plugin_host::expand_asset_dir(&framework.args, &asset_dir);
        if let Some(pattern) = filter {
            args = test_core::filter::apply_filter(
                &args,
                framework.filter_flag.as_deref(),
                framework.filter_template.as_deref(),
                &pattern,
            );
        }

        let Ok(output_format) = test_core::parse_output_format(&framework.output_format) else {
            return errors::failure(
                errors::CODE_REFUSED,
                "this test framework's output-format is not one this build understands",
            );
        };
        let report_glob = framework.report_glob.clone();

        let run_id = self.next_id.get() + 1;
        self.next_id.set(run_id);
        let handle = test_core::TestRunHandle::new();
        self.runs.borrow_mut().insert(run_id, handle.clone());

        let qt_thread = self.as_mut().qt_thread();
        self.as_mut().test_run_started();

        std::thread::spawn(move || {
            let mut sink = QtSink {
                qt_thread: qt_thread.clone(),
                ansi: run_core::AnsiStripper::default(),
            };
            let program_str = program.to_string_lossy().into_owned();
            let result = test_core::run(
                &handle,
                &program_str,
                &args,
                &root,
                output_format,
                report_glob.as_deref(),
                &mut sink,
            );
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::TestService>| {
                service.runs.borrow_mut().remove(&run_id);
                let (ok, message) = match result {
                    Ok(_) => (true, String::new()),
                    Err(test_core::RunFailure::NotFound) => {
                        (false, "test framework program not found".to_string())
                    }
                    Err(test_core::RunFailure::Io(msg)) => (false, msg),
                };
                service
                    .as_mut()
                    .test_run_finished(ok, QString::from(message.as_str()));
            });
        });
        ffi::FfiResult::default()
    }

    pub fn stop(self: Pin<&mut Self>) {
        for handle in self.runs.borrow().values() {
            handle.stop();
        }
    }
}

/// Regroup the current tree's failures by file and publish them under this
/// run's source key — the same "full regroup on every chunk" rule
/// `bridge::build::republish` uses, since a later event can add a failure
/// to a file an earlier one already reported on.
fn republish(service: &ffi::TestService) {
    let framework = service.framework_name.borrow().clone();
    let key = source_key(&framework);
    let grouped = test_core::diagnostics_by_file(&service.tree.borrow(), &framework);
    let mut store = service.store.borrow_mut();
    store.clear_source(&key);
    for (uri, diagnostics) in grouped {
        store.replace(&key, &uri, diagnostics);
    }
}

/// The `TestSink` that turns a running test process's chunks into Qt
/// signals. Every callback is queued rather than called directly: this
/// runs on the run's own thread.
struct QtSink {
    qt_thread: CxxQtThread<ffi::TestService>,
    /// PHPUnit colours its own progress output; the plan's "Reuse, not
    /// reinvention" section names this stripper for exactly that, applied
    /// here rather than in `test-core` since `test-core`'s layering row
    /// carries no adapter-layer crate (`run-core` included).
    ansi: run_core::AnsiStripper,
}

impl test_core::TestSink for QtSink {
    fn output(&mut self, text: &str) {
        let text = self.ansi.feed(text);
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::TestService>| {
                service
                    .as_mut()
                    .test_output_appended(QString::from(text.as_str()));
            });
    }

    fn event(&mut self, event: test_core::TeamCityEvent) {
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::TestService>| {
                service.tree.borrow_mut().apply(event);
                republish(&service);
                service.as_mut().test_tree_changed();
            });
    }

    fn junit(&mut self, cases: Vec<test_core::JUnitTestCase>) {
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::TestService>| {
                service.tree.borrow_mut().apply_junit(&cases);
                republish(&service);
                service.as_mut().test_tree_changed();
            });
    }
}
