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
use cxx_qt_lib::{QString, QStringList};

use crate::bridge::errors;
use crate::bridge::ffi;
use crate::bridge::registry::SharedDiagnostics;

mod configuration;
pub(crate) use configuration::{preview as configuration_preview, run_for_before_launch};
mod gutter;
mod navigation;
mod tree;

/// This source's key in the shared store (ADR-0046): distinct from an
/// analyzer's or a build's, so a test failure's rows for a file never
/// clobber (or get clobbered by) either.
fn source_key(framework_name: &str) -> String {
    format!("test:{framework_name}")
}

/// A file's markers with the hash of the text they were parsed from.
type CachedMarkers = (u64, std::rc::Rc<Vec<php_core::tests::TestMarker>>);

/// Who waits for a run to finish: a before-launch task (#416), told `Ok`
/// only when the framework exits 0.
type Waiter = std::sync::mpsc::Sender<Result<(), String>>;

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
    /// Why the last run's framework cannot rerun from the tree (empty when
    /// it can) — what the tree's Rerun action greys out on.
    rerun_block: RefCell<String>,
    /// Where the last run executed, so the paths its failures print map
    /// back to local files (ADR-0067). `None` until the first run.
    run_host: RefCell<Option<process_exec::host::ExecHost>>,
    /// The last run's working directory (a monorepo package's, #424), which
    /// its relative paths are relative to. `None` until the first run.
    run_dir: RefCell<Option<PathBuf>>,
    /// The filter a gutter Debug click chose, run once the PHP listener is
    /// up (T3): Xdebug does not retry, so the run starts after it.
    pending_debug: RefCell<Option<RerunSelection>>,
    /// The last coverage run's report, local paths (T5).
    coverage: RefCell<Option<test_core::coverage::Coverage>>,
    /// Parsed markers per open file, keyed by the text they came from: the
    /// gutter repaints on every streamed event, the text rarely changes.
    marker_cache: RefCell<HashMap<String, CachedMarkers>>,
    /// A configuration run that does not allow parallel runs, started once
    /// the run it stopped has finished, with whether it collects coverage.
    queued: RefCell<Option<(RerunSelection, bool)>>,
    /// The test packages of right-clicked project tree rows (E6).
    tree_packages: RefCell<tree::PackageCache>,
    /// The run the open project tree menu offers, for its click (E6).
    tree_offer: RefCell<Option<tree::TreeOffer>>,
    /// Go to Test's searches: one at a time, the latest answers (E7).
    navigation: RefCell<navigation::NavigationRequests>,
    /// Which tests cover which class, per project (#426).
    covers: std::sync::Arc<php_core::test_navigation::CoversCache>,
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
///
/// A run in `work_dir` (a monorepo package, #424) uses the package's own
/// program and toolchains first, then the project's.
///
/// `local_only` probes for the program on this machine only, never inside a
/// WSL distribution or a container: what a preview can afford on every
/// keystroke.
fn detect_framework(
    root: &Path,
    work_dir: &Path,
    settings: &app_config::Settings,
    local_only: bool,
) -> Option<(
    plugin_host::LoadedPlugin,
    plugin_api::TestFrameworkContribution,
    PathBuf,
    process_exec::host::ExecHost,
)> {
    let owners = contributed_frameworks();
    let contributions: Vec<plugin_api::TestFrameworkContribution> = owners
        .iter()
        .map(|(_, framework)| framework.clone())
        .collect();
    let mut detected: Vec<String> = Vec::new();
    for dir in [work_dir, root] {
        for id in run_core::toolchain::detect_toolchains(dir) {
            let id = id.as_str().to_string();
            if !detected.contains(&id) {
                detected.push(id);
            }
        }
    }
    let detected_refs: Vec<&str> = detected.iter().map(String::as_str).collect();

    let (index, _, program) = test_core::select_framework(
        &contributions,
        &detected_refs,
        test_core::SUPPORTED_OUTPUT_FORMATS,
        |framework| {
            let host = if local_only {
                process_exec::host::ExecHost::Local
            } else {
                framework_host(framework, settings, root)
            };
            let candidates = &framework.program_candidates;
            analysis_core::find_program_on(candidates, work_dir, &host).or_else(|| {
                (work_dir != root)
                    .then(|| analysis_core::find_program_on(candidates, root, &host))
                    .flatten()
            })
        },
    )?;
    let (plugin, framework) = owners.into_iter().nth(index)?;
    let host = framework_host(&framework, settings, root);
    Some((plugin, framework, program, host))
}

/// Where `framework` runs: the PHP interpreter's host when it requires one
/// (ADR-0067), the project's own otherwise.
fn framework_host(
    framework: &plugin_api::TestFrameworkContribution,
    settings: &app_config::Settings,
    root: &Path,
) -> process_exec::host::ExecHost {
    crate::bridge::php::tool_host(framework.requires_interpreter.as_deref(), settings, root)
}

/// The detected framework, ready for a run's arguments.
struct Framework {
    contribution: plugin_api::TestFrameworkContribution,
    program: PathBuf,
    host: process_exec::host::ExecHost,
    dialect: test_core::filter::FilterDialect,
    /// The framework's own arguments, `${asset_dir}` expanded.
    args: Vec<String>,
}

impl Framework {
    /// The framework a run in `work_dir` of the project uses, or why there
    /// is none.
    fn detect(
        root: &Path,
        work_dir: &Path,
        settings: &app_config::Settings,
        local_only: bool,
    ) -> Result<Self, &'static str> {
        let (plugin, contribution, program, host) =
            detect_framework(root, work_dir, settings, local_only)
                .ok_or("no installed test framework is contributed for this project")?;
        // A test framework's args may name `${asset_dir}` (junit-gradle's
        // `--init-script ${asset_dir}/ide-model.init.gradle`) — the same
        // materialise-on-demand seam A3 built for a sync provider.
        let asset_dir = plugin
            .asset_dir(&app_core::resolve_config_dir())
            .map_err(|_| "could not materialise this test framework's plugin assets")?;
        // Which target-selection syntax this framework's rerun flag/template
        // speaks: PHPUnit's PCRE `--filter` is not the only dialect once a
        // framework runs on the JVM (review finding 1).
        let dialect =
            test_core::filter::parse_filter_dialect(contribution.filter_dialect.as_deref())
                .map_err(|_| {
                    "this test framework's filter-dialect is not one this build understands"
                })?;
        let args = plugin_host::expand_asset_dir(&contribution.args, &asset_dir);
        Ok(Self {
            contribution,
            program,
            host,
            dialect,
            args,
        })
    }

    /// The arguments a `phpunit` configuration's scope adds to the
    /// framework's own.
    fn scoped(&self, run: &configuration::ConfigRun) -> Result<Vec<String>, String> {
        test_core::scope::scope_args(
            &self.args,
            self.dialect,
            self.contribution.filter_flag.as_deref(),
            self.contribution.filter_template.as_deref(),
            &run.plan.scope,
            &run.plan.options,
        )
        .map_err(|error| error.to_string())
    }

    /// The program and arguments as launched: a PHP framework runs under the
    /// configured interpreter, which is what makes it run inside that
    /// interpreter's container.
    fn command(
        &self,
        settings: &app_config::Settings,
        args: Vec<String>,
    ) -> (PathBuf, Vec<String>) {
        match self.contribution.requires_interpreter.as_deref() {
            Some("php") => {
                let interpreter = settings_model::php::resolve(settings).interpreter;
                let (program, prefix) =
                    analysis_core::php_invocation(&self.program, Some(&interpreter));
                (program, [prefix, args].concat())
            }
            _ => (self.program.clone(), args),
        }
    }
}

/// What `start` was asked to rerun, carried from `run_failed`/`run_node`
/// through to the point the framework (and so its filter dialect) is known
/// — building the actual pattern has to wait until then (review finding 1),
/// so this just carries the *un*-interpreted request. Translation-only, no
/// rule of its own: `test_core::filter` decides what a `Failed`/`Node`
/// selection turns into for a given dialect.
enum RerunSelection {
    Failed(Vec<test_core::TestId>),
    Node(test_core::TestId),
    /// A test the editor named, from a gutter marker (T3); becomes a
    /// temporary `phpunit` configuration when the run starts.
    Marker(MarkerSelection),
    /// One data set of the last run; becomes a temporary `phpunit`
    /// configuration, or a plain node rerun when it is not a data-set row.
    DataSet(test_core::TestId),
    /// A `phpunit` run configuration (E4, ADR-0073); a temporary one is
    /// remembered when the run starts.
    Configuration(Box<run_core::RunConfig>),
}

/// A gutter marker's test, kept until the run starts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MarkerSelection {
    marker: php_core::tests::TestMarker,
    /// The test file, absolute as the editor spells it.
    file: String,
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
    /// Make this the service a `phpunit` before-launch task runs on, from
    /// whichever thread runs the tasks (`configuration::run_for_before_launch`).
    pub fn register_before_launch_runner(self: Pin<&mut Self>) {
        crate::bridge::registry::set_test_service(self.qt_thread());
    }

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
        let (_, work_dir) = self.run_context(&root);
        let id = test_core::TestId(node_id.to_string());
        let details = self
            .tree
            .borrow()
            .node(&id)
            .and_then(|node| node.failure.as_ref())
            .map(|failure| failure.details.clone())
            .unwrap_or_default();
        match run_core::resolve_link(&details, byte_offset as usize, &work_dir, None) {
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

    /// The node's location, and the host/work dir that map its paths.
    fn located(
        &self,
        node_id: &QString,
    ) -> Option<(
        test_core::TestLocation,
        process_exec::host::ExecHost,
        PathBuf,
    )> {
        let root = current_project_root()?;
        let location = self
            .tree
            .borrow()
            .node(&test_core::TestId(node_id.to_string()))?
            .location
            .clone()
            .filter(|location| !location.file.is_empty())?;
        let (host, work_dir) = self.run_context(&root);
        Some((location, host, work_dir))
    }

    /// The last run's host and working directory, which its printed paths
    /// are relative to: the project's own before any run.
    fn run_context(&self, root: &Path) -> (process_exec::host::ExecHost, PathBuf) {
        let host = (*self.run_host.borrow())
            .clone()
            .unwrap_or_else(|| process_exec::host::ExecHost::for_path(root));
        let work_dir = (*self.run_dir.borrow())
            .clone()
            .unwrap_or_else(|| root.to_path_buf());
        (host, work_dir)
    }

    /// The node's file on this machine; empty without a location. No I/O.
    pub fn node_source_path(&self, node_id: &QString) -> QString {
        self.located(node_id)
            .map(|(location, host, root)| location.local_path(&host, &root))
            .map(|path| QString::from(path.display().to_string().as_str()))
            .unwrap_or_default()
    }

    /// Where `node_id` lives in source: `test_core::source_target`'s rule,
    /// with the PHP symbol lookup and the editor's unsaved text plugged in.
    pub fn node_source(
        &self,
        node_id: &QString,
        open_text: &QString,
        has_open_text: bool,
    ) -> ffi::FfiResolvedLink {
        let Some((location, host, root)) = self.located(node_id) else {
            return ffi::FfiResolvedLink::default();
        };
        let open = has_open_text.then(|| open_text.to_string());
        let target = test_core::source_target(
            &location,
            &host,
            &root,
            |_| open.clone(),
            php_core::tests::symbol_line,
        );
        target.map_or_else(ffi::FfiResolvedLink::default, |target| {
            ffi::FfiResolvedLink {
                found: true,
                path: QString::from(target.path.display().to_string().as_str()),
                line: target.line,
                has_column: false,
                column: 0,
            }
        })
    }

    pub fn tested_files(&self) -> QStringList {
        let Some(root) = current_project_root() else {
            return QStringList::default();
        };
        let (host, work_dir) = self.run_context(&root);
        self.tree
            .borrow()
            .located_files(&host, &work_dir)
            .iter()
            .map(|path| QString::from(path.display().to_string().as_str()))
            .collect()
    }

    pub fn is_running(&self) -> bool {
        !self.runs.borrow().is_empty()
    }

    pub fn run_all(self: Pin<&mut Self>) -> ffi::FfiResult {
        self.start(None, Vec::new(), false)
    }

    pub fn run_failed(self: Pin<&mut Self>) -> ffi::FfiResult {
        let ids = self.tree.borrow().failed_leaf_ids();
        if ids.is_empty() {
            return errors::failure(errors::CODE_REFUSED, "no failed tests to rerun");
        }
        self.start(Some(RerunSelection::Failed(ids)), Vec::new(), false)
    }

    /// Why a node cannot be rerun from the tree; empty when it can.
    pub fn rerun_block(&self) -> QString {
        QString::from(self.rerun_block.borrow().as_str())
    }

    pub fn run_node(self: Pin<&mut Self>, node_id: &QString) -> ffi::FfiResult {
        let id = test_core::TestId(node_id.to_string());
        self.start(Some(RerunSelection::Node(id)), Vec::new(), false)
    }

    fn start(
        self: Pin<&mut Self>,
        selection: Option<RerunSelection>,
        env: Vec<(String, String)>,
        with_coverage: bool,
    ) -> ffi::FfiResult {
        self.start_with(selection, env, with_coverage, None)
    }

    /// [`Self::start`], telling `waiter` how the run ended.
    fn start_with(
        mut self: Pin<&mut Self>,
        selection: Option<RerunSelection>,
        mut env: Vec<(String, String)>,
        with_coverage: bool,
        waiter: Option<Waiter>,
    ) -> ffi::FfiResult {
        if !self.runs.borrow().is_empty() {
            return errors::failure(errors::CODE_REFUSED, "a test run is already in progress");
        }
        let Some(root) = current_project_root() else {
            return errors::failure(errors::CODE_NO_PROJECT, "no project is open");
        };
        // Editor runs are temporary `phpunit` configurations, so they show in
        // the configuration list and rerun like any other (ADR-0073).
        let selection = match self.resolve_editor_run(selection, &root) {
            Ok(selection) => selection,
            Err(reason) => return errors::failure(errors::CODE_REFUSED, reason),
        };
        let config_run = match &selection {
            Some(RerunSelection::Configuration(config)) => {
                match configuration::resolve(config, &root) {
                    Ok(run) => Some(run),
                    Err(reason) => return errors::failure(errors::CODE_REFUSED, reason),
                }
            }
            _ => None,
        };
        let settings = configuration::settings_for(config_run.as_ref());
        env.extend(config_run.iter().flat_map(|run| run.env.clone()));
        // A monorepo package's configuration runs in the package (#424).
        let relative_work_dir = config_run
            .as_ref()
            .and_then(|run| run.plan.working_directory.clone())
            .unwrap_or_default();
        let work_dir = root.join(&relative_work_dir);
        let framework = match Framework::detect(&root, &work_dir, &settings, false) {
            Ok(framework) => framework,
            Err(reason) => return errors::failure(errors::CODE_REFUSED, reason),
        };

        // A whole run replaces the tree a previous run left behind; a
        // narrowed rerun updates only the nodes it touches, so the rest of
        // the previous run's results stay visible (D6's "reruns exactly
        // the failed node", not "clears the dock").
        let whole_run = match (&selection, &config_run) {
            (None, _) => true,
            (_, Some(run)) => matches!(
                run.plan.scope,
                test_core::scope::TestScope::Config { .. }
                    | test_core::scope::TestScope::Directory { .. }
            ),
            _ => false,
        };
        if whole_run {
            self.tree.borrow_mut().reset();
        }
        *self.framework_name.borrow_mut() = framework.contribution.name.clone();
        *self.run_host.borrow_mut() = Some(framework.host.clone());
        *self.run_dir.borrow_mut() = Some(work_dir.clone());
        // Diagnostics from the framework this run uses are recomputed as
        // events arrive (`republish`); a stale row from a source this
        // project no longer contributes would otherwise never be cleared.
        self.store
            .borrow_mut()
            .clear_source(&source_key(&framework.contribution.name));

        // Surefire's `-Dtest=` and Gradle's `--tests` each need their own
        // pattern shape built from the node being rerun, not a PHPUnit-
        // shaped one that would compile fine and match nothing.
        *self.rerun_block.borrow_mut() =
            test_core::filter::tree_rerun_block(framework.contribution.filter_dialect.as_deref())
                .unwrap_or_default();
        let dialect = framework.dialect;
        let tree_rerun = matches!(
            selection,
            Some(RerunSelection::Failed(_) | RerunSelection::Node(_))
        );
        if let Some(reason) = test_core::filter::tree_rerun_refusal(dialect).filter(|_| tree_rerun)
        {
            return errors::failure(errors::CODE_REFUSED, reason);
        }
        let patterns = match &selection {
            Some(RerunSelection::Failed(ids)) => test_core::filter::for_many(ids, dialect),
            Some(RerunSelection::Node(id)) => {
                test_core::filter::for_node(&self.tree.borrow(), id, dialect)
            }
            _ => Vec::new(),
        };

        let mut args = match &config_run {
            Some(run) => match framework.scoped(run) {
                Ok(args) => args,
                Err(error) => return errors::failure(errors::CODE_REFUSED, error),
            },
            None => test_core::filter::apply_filter(
                &framework.args,
                framework.contribution.filter_flag.as_deref(),
                framework.contribution.filter_template.as_deref(),
                &patterns,
            ),
        };
        if with_coverage {
            if framework.contribution.coverage_args.is_empty() {
                return errors::failure(
                    errors::CODE_REFUSED,
                    "this test framework cannot collect coverage",
                );
            }
            args.extend(test_core::coverage::args_in(
                &framework.contribution.coverage_args,
                &relative_work_dir,
            ));
            env.extend(test_core::coverage::env());
            // A report from an earlier run must never pass for this one's.
            let _ = std::fs::remove_file(root.join(test_core::coverage::REPORT_PATH));
        }
        let (program, args) = framework.command(&settings, args);

        let Ok(output_format) =
            test_core::parse_output_format(&framework.contribution.output_format)
        else {
            return errors::failure(
                errors::CODE_REFUSED,
                "this test framework's output-format is not one this build understands",
            );
        };
        let report_glob = framework.contribution.report_glob.clone();
        let host = framework.host;

        let run_id = self.next_id.get() + 1;
        self.next_id.set(run_id);
        let handle = test_core::TestRunHandle::new();
        self.runs.borrow_mut().insert(run_id, handle.clone());

        let remembered = match &selection {
            Some(RerunSelection::Configuration(config))
                if config.temporary && configuration::remember(config, &root).is_ok() =>
            {
                Some(config.id.clone())
            }
            _ => None,
        };
        let qt_thread = self.as_mut().qt_thread();
        self.as_mut().test_run_started();
        if let Some(config_id) = remembered {
            self.as_mut()
                .configuration_remembered(QString::from(config_id.as_str()));
        }

        std::thread::spawn(move || {
            let mut sink = QtSink {
                qt_thread: qt_thread.clone(),
                ansi: run_core::AnsiStripper::default(),
                psr4_roots: php_core::composer::ComposerJson::read(&work_dir)
                    .ok()
                    .flatten()
                    .map(|composer| composer.psr4)
                    .unwrap_or_default(),
                work_dir: work_dir.clone(),
            };
            let program_str = program.to_string_lossy().into_owned();
            let result = test_core::run_on_env(
                &host,
                &handle,
                &program_str,
                &args,
                &env,
                &work_dir,
                output_format,
                report_glob.as_deref(),
                &mut sink,
            );
            let coverage = with_coverage.then(|| {
                let xml = std::fs::read_to_string(root.join(test_core::coverage::REPORT_PATH));
                xml.ok()
                    .and_then(|xml| test_core::coverage::parse_clover(&xml).ok())
                    .map(|coverage| coverage.map_paths_from(&host))
            });
            let matched_nothing = result
                .as_ref()
                .is_ok_and(|code| handle.matched_nothing(*code));
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::TestService>| {
                service.runs.borrow_mut().remove(&run_id);
                // Whatever never reported (Stop, a fatal error, a killed
                // container) must not stay "running" on a gutter icon.
                service.tree.borrow_mut().finish_run();
                service.as_mut().test_tree_changed();
                if let Some(coverage) = coverage {
                    if coverage.is_none() {
                        service.as_mut().test_output_appended(QString::from(
                            "\nNo coverage report was written. Is Xdebug (coverage mode) or PCOV installed?\n",
                        ));
                    }
                    *service.coverage.borrow_mut() = coverage;
                    service.as_mut().coverage_changed();
                }
                let (ok, message) = match &result {
                    Ok(_) => (true, String::new()),
                    Err(test_core::RunFailure::NotFound) => {
                        (false, "test framework program not found".to_string())
                    }
                    Err(test_core::RunFailure::Io(msg)) => (false, msg.clone()),
                };
                if let Some(waiter) = waiter {
                    let _ = waiter.send(match result {
                        Ok(Some(0)) => Ok(()),
                        Ok(Some(code)) => Err(format!("exit {code}")),
                        Ok(None) => Err("the run was stopped".to_string()),
                        Err(_) => Err(message.clone()),
                    });
                }
                service.as_mut().test_run_finished(
                    ok,
                    matched_nothing,
                    QString::from(message.as_str()),
                );
                service.as_mut().start_queued();
            });
        });
        ffi::FfiResult::default()
    }

    /// Start the configuration run that waited for the previous one to stop.
    fn start_queued(mut self: Pin<&mut Self>) {
        let Some((selection, with_coverage)) = self.queued.borrow_mut().take() else {
            return;
        };
        let result = self
            .as_mut()
            .start(Some(selection), Vec::new(), with_coverage);
        if result.code != errors::CODE_OK {
            self.as_mut().test_output_appended(result.message);
        }
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
    let mut store = service.store.borrow_mut();
    store.clear_source(&key);
    // Locating a JVM failure's real source file (test-core's `diagnostics`
    // module, review finding 3) needs the project root to resolve a class
    // name to a path; with no project open there is nothing to republish
    // against in the first place, so clearing the stale source above is all
    // this call does.
    let Some(root) = current_project_root() else {
        return;
    };
    let (host, work_dir) = service.run_context(&root);
    let grouped =
        test_core::diagnostics_by_file_on(&host, &service.tree.borrow(), &framework, &work_dir);
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
    /// A Pest class-based test names its class and no file; the PSR-4
    /// roots of the run's package find the file.
    psr4_roots: Vec<php_core::composer::Psr4Root>,
    work_dir: PathBuf,
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
        let event = php_core::tests::locate_event(event, &self.psr4_roots, &self.work_dir);
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
