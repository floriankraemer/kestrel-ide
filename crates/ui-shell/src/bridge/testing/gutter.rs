//! The test gutter's half of `TestService` (PHP parity plan T3): which
//! lines of a PHP file carry a Run/Debug marker, and starting the one
//! test a marker names.
//!
//! Translation only: where the markers go and what filter each runs is
//! `php_core::tests::markers`'s; the framework, its argv and its output
//! parsing are the ordinary `start` path's.

use std::path::{Path, PathBuf};
use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use super::{MarkerSelection, RerunSelection};
use crate::bridge::errors;
use crate::bridge::ffi;

/// Files whose parsed markers the gutter keeps.
const MAX_CACHED_FILES: usize = 64;

/// The project's recorded test results (G4), read on a worker (#437).
#[derive(Default)]
pub(super) enum HistoryState {
    /// Not asked for yet, or the last read failed: the next paint asks.
    #[default]
    Absent,
    Loading(PathBuf),
    Loaded(PathBuf, php_core::test_history::TestHistory),
}

impl HistoryState {
    /// The loaded history of the project at `root`.
    pub(super) fn of(&self, root: &Path) -> Option<&php_core::test_history::TestHistory> {
        match self {
            Self::Loaded(loaded, history) if loaded == root => Some(history),
            _ => None,
        }
    }
}

pub(super) fn is_php_file(path: &str) -> bool {
    syntax_core::language_for_path(std::path::Path::new(path)).id() == "php"
}

fn selection(path: String, marker: php_core::tests::TestMarker) -> MarkerSelection {
    MarkerSelection { marker, file: path }
}

fn marker_at(path: &QString, text: &QString, line: u32) -> Option<MarkerSelection> {
    let path = path.to_string();
    if !is_php_file(&path) {
        return None;
    }
    php_core::tests::markers(&text.to_string())
        .into_iter()
        .find(|marker| marker.line == line)
        .map(|marker| selection(path, marker))
}

/// What a caret-aware Run File / Debug File does.
#[derive(Debug, PartialEq, Eq)]
enum CaretRun {
    Run(MarkerSelection),
    Debug(MarkerSelection),
    /// Not a PHP test file: the ordinary run of the file.
    PlainRun,
    NoTestToDebug,
    DebugUnavailable,
}

/// The test-file-or-fallback decision; the file is parsed once.
fn plan_caret_run(
    path: &str,
    text: &str,
    line: u32,
    debug: bool,
    debug_available: bool,
) -> CaretRun {
    let marker = is_php_file(path)
        .then(|| php_core::tests::marker_at(text, line))
        .flatten()
        .map(|marker| selection(path.to_string(), marker));
    match (marker, debug) {
        (Some(marker), false) => CaretRun::Run(marker),
        (Some(_), true) if !debug_available => CaretRun::DebugUnavailable,
        (Some(marker), true) => CaretRun::Debug(marker),
        (None, false) => CaretRun::PlainRun,
        (None, true) => CaretRun::NoTestToDebug,
    }
}

fn to_ffi_marker_state(state: php_core::test_status::MarkerState) -> ffi::FfiMarkerState {
    use php_core::test_status::MarkerState;
    match state {
        MarkerState::NeverRun => ffi::FfiMarkerState::NeverRun,
        MarkerState::Running => ffi::FfiMarkerState::Running,
        MarkerState::Passed => ffi::FfiMarkerState::Passed,
        MarkerState::Failed => ffi::FfiMarkerState::Failed,
        MarkerState::Ignored => ffi::FfiMarkerState::Ignored,
        MarkerState::Terminated => ffi::FfiMarkerState::Terminated,
    }
}

fn no_marker() -> ffi::FfiResult {
    errors::failure(errors::CODE_REFUSED, "there is no test on this line")
}

impl ffi::TestService {
    /// The markers of `text`, parsed once per distinct text of `path`.
    pub(super) fn markers_of(
        &self,
        path: &str,
        text: &str,
    ) -> std::rc::Rc<Vec<php_core::tests::TestMarker>> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let key = hasher.finish();
        let mut cache = self.marker_cache.borrow_mut();
        if let Some((cached, markers)) = cache.get(path).filter(|(cached, _)| *cached == key) {
            let _ = cached;
            return markers.clone();
        }
        // ponytail: a plain cap, no LRU; the gutter only asks for open files.
        if cache.len() >= MAX_CACHED_FILES {
            cache.clear();
        }
        let markers = std::rc::Rc::new(php_core::tests::markers(text));
        cache.insert(path.to_string(), (key, markers.clone()));
        markers
    }

    /// The 0-based lines of `text` that carry a test marker; empty for a
    /// file that is not a PHP test.
    pub fn marker_lines(&self, path: &QString, text: &QString) -> Vec<u32> {
        let path = path.to_string();
        if !is_php_file(&path) {
            return Vec::new();
        }
        self.markers_of(&path, &text.to_string())
            .iter()
            .map(|marker| marker.line)
            .collect()
    }

    /// The last result for every marker in `text`, for the gutter icons:
    /// this session's run, else the project's recorded history (G4); empty
    /// for a file that is not a PHP test or that neither has a node in.
    pub fn marker_statuses(&self, path: &QString, text: &QString) -> Vec<ffi::FfiMarkerStatus> {
        let path = path.to_string();
        if !is_php_file(&path) {
            return Vec::new();
        }
        let Some(root) = super::current_project_root() else {
            return Vec::new();
        };
        self.load_history(&root);
        let history = self.history.borrow();
        let recorded_tree = history.of(&root).map(|history| history.tree());
        let recorded_nodes = recorded_tree
            .map(|tree| {
                tree.nodes_in_file(
                    &process_exec::host::ExecHost::Local,
                    Path::new(""),
                    Path::new(&path),
                )
            })
            .unwrap_or_default();
        let tree = self.tree.borrow();
        let nodes = self.nodes_in(&tree, &path);
        if nodes.is_empty() && recorded_nodes.is_empty() {
            return Vec::new();
        }
        let markers = self.markers_of(&path, &text.to_string());
        use php_core::test_status::Results;
        let live = Results {
            tree: &tree,
            file_nodes: &nodes,
        };
        let recorded = Results {
            tree: recorded_tree.unwrap_or(&tree),
            file_nodes: &recorded_nodes,
        };
        php_core::test_status::marker_statuses_with_history(&markers, live, recorded)
            .into_iter()
            .map(|status| {
                let comparison = status.comparison.unwrap_or_default();
                ffi::FfiMarkerStatus {
                    line: status.line,
                    state: to_ffi_marker_state(status.state),
                    total: status.total,
                    failed: status.failed,
                    passed: status.passed,
                    duration_ms: status.duration_ms,
                    message: QString::from(status.message.as_str()),
                    expected: QString::from(comparison.expected.as_str()),
                    actual: QString::from(comparison.actual.as_str()),
                }
            })
            .collect()
    }

    /// The data sets the last run executed for the method marked on `line`,
    /// each runnable on its own.
    pub fn marker_data_sets(
        &self,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> ffi::FfiDataSetChoices {
        let path = path.to_string();
        if !is_php_file(&path) {
            return ffi::FfiDataSetChoices::default();
        }
        let tree = self.tree.borrow();
        let nodes = self.nodes_in(&tree, &path);
        let markers = self.markers_of(&path, &text.to_string());
        let choices = php_core::test_status::data_sets(&markers, line, &tree, &nodes);
        ffi::FfiDataSetChoices {
            sets: choices
                .shown
                .into_iter()
                .map(|set| ffi::FfiDataSet {
                    id: QString::from(set.id.as_str()),
                    label: QString::from(set.label.as_str()),
                })
                .collect(),
            hidden: u32::try_from(choices.hidden).unwrap_or(u32::MAX),
        }
    }

    /// Run one data set of the last run, by the id `marker_data_sets` gave.
    pub fn run_data_set(self: Pin<&mut Self>, id: &QString) -> ffi::FfiResult {
        self.start(
            Some(RerunSelection::DataSet(test_core::TestId(id.to_string()))),
            Vec::new(),
            false,
        )
    }

    /// Stage one data set for debugging, like [`Self::prepare_debug_marker`].
    pub fn prepare_debug_data_set(self: Pin<&mut Self>, id: &QString) -> ffi::FfiResult {
        *self.pending_debug.borrow_mut() =
            Some(RerunSelection::DataSet(test_core::TestId(id.to_string())));
        ffi::FfiResult::default()
    }

    /// Start reading the project's recorded test results on a worker, once
    /// per project; the gutter repaints when they arrive (#437). A read that
    /// failed is retried on the next paint, and nothing overwrites the file
    /// unread: a save moves an unreadable one aside.
    // ponytail: a corrupt file is re-read on every paint until a run's save
    // moves it aside; cheap next to the paint, and it heals itself.
    fn load_history(&self, root: &Path) {
        let mut state = self.history.borrow_mut();
        if matches!(&*state, HistoryState::Loading(loading) if loading == root)
            || state.of(root).is_some()
        {
            return;
        }
        *state = HistoryState::Loading(root.to_path_buf());
        let qt_thread = self.qt_thread();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let loaded = app_config::test_history::load(&root).ok();
            let _ = qt_thread.queue(move |service: Pin<&mut ffi::TestService>| {
                service.history_arrived(&root, loaded);
            });
        });
    }

    /// The project's recorded results arrived (`None`: the file could not
    /// be read): merged into whatever this session recorded meanwhile.
    fn history_arrived(
        self: Pin<&mut Self>,
        root: &Path,
        loaded: Option<app_config::test_history::TestHistorySettings>,
    ) {
        {
            let mut state = self.history.borrow_mut();
            match (&mut *state, loaded) {
                (HistoryState::Loaded(at, history), Some(settings)) if at == root => {
                    history.merge(settings);
                }
                (HistoryState::Loading(at), Some(settings)) if at == root => {
                    *state = HistoryState::Loaded(
                        root.to_path_buf(),
                        php_core::test_history::TestHistory::from_settings(settings),
                    );
                }
                (HistoryState::Loading(at), None) if at == root => *state = HistoryState::Absent,
                // Another project is open now, or the read failed after a
                // run recorded results of its own.
                _ => return,
            }
        }
        self.test_history_loaded();
    }

    /// Record the finished run's results for the gutter's next session, and
    /// persist them off the Qt thread when they changed: merged with what
    /// the file holds by then, so concurrent saves and other IDE instances
    /// keep each other's results. A run of a project that is no longer open
    /// records nothing.
    pub(super) fn record_history(&self, root: &Path) {
        if super::current_project_root().as_deref() != Some(root) {
            return;
        }
        let (host, work_dir) = self.run_context(root);
        let mut state = self.history.borrow_mut();
        // Still loading, or unread: the load (or the save's merge) adds the
        // file's results to these.
        if state.of(root).is_none() {
            *state = HistoryState::Loaded(
                root.to_path_buf(),
                php_core::test_history::TestHistory::default(),
            );
        }
        let HistoryState::Loaded(_, history) = &mut *state else {
            return;
        };
        let now = php_core::test_history::now_ms();
        if !history.record(&self.tree.borrow(), &host, &work_dir, now) {
            return;
        }
        let ours = history.settings();
        let root = root.to_path_buf();
        let qt_thread = self.qt_thread();
        // The history is a convenience: a failed write only means the next
        // session starts without these icons.
        std::thread::spawn(move || {
            let saved = app_config::test_history::update(&root, |on_disk| {
                php_core::test_history::merged(on_disk, &ours)
            });
            if let Ok(saved) = saved {
                let _ = qt_thread.queue(move |service: Pin<&mut ffi::TestService>| {
                    service.history_arrived(&root, Some(saved));
                });
            }
        });
    }

    /// The files the recorded results name, on this machine.
    pub(super) fn history_files(&self, root: &Path) -> Vec<PathBuf> {
        self.history
            .borrow()
            .of(root)
            .map(|history| {
                history
                    .tree()
                    .located_files(&process_exec::host::ExecHost::Local, Path::new(""))
            })
            .unwrap_or_default()
    }

    /// The tree's nodes located in `path`, mapped through the last run's host.
    fn nodes_in<'t>(
        &self,
        tree: &'t test_core::TestTree,
        path: &str,
    ) -> Vec<&'t test_core::TestNode> {
        let Some(root) = super::current_project_root() else {
            return Vec::new();
        };
        let (host, work_dir) = self.run_context(&root);
        tree.nodes_in_file(&host, &work_dir, std::path::Path::new(path))
    }

    /// The name a marker's menu shows, empty when `line` has no marker.
    pub fn marker_name(&self, path: &QString, text: &QString, line: u32) -> QString {
        marker_at(path, text, line)
            .map(|marker| QString::from(marker.marker.name.as_str()))
            .unwrap_or_default()
    }

    /// Run (or, for `debug`, stage for `DebugService`) what the caret on
    /// `line` is in. `CODE_NOT_A_TEST` tells the view to fall back to a
    /// plain run of the file; success on `debug` means the view now starts
    /// the debug session, which calls [`Self::run_pending_with_env`].
    pub fn run_caret_context(
        self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        line: u32,
        debug: bool,
        debug_available: bool,
    ) -> ffi::FfiResult {
        match plan_caret_run(
            &path.to_string(),
            &text.to_string(),
            line,
            debug,
            debug_available,
        ) {
            CaretRun::Run(marker) => {
                self.start(Some(RerunSelection::Marker(marker)), Vec::new(), false)
            }
            CaretRun::Debug(marker) => {
                *self.pending_debug.borrow_mut() = Some(RerunSelection::Marker(marker));
                ffi::FfiResult::default()
            }
            CaretRun::PlainRun => errors::failure(errors::CODE_NOT_A_TEST, "not a test file"),
            CaretRun::NoTestToDebug => errors::failure(
                errors::CODE_REFUSED,
                "There is no test in this file to debug.",
            ),
            CaretRun::DebugUnavailable => {
                errors::failure(errors::CODE_REFUSED, "Debugging is not available.")
            }
        }
    }

    /// Run the test the marker on `line` names.
    pub fn run_marker(
        self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> ffi::FfiResult {
        match marker_at(path, text, line) {
            Some(marker) => self.start(Some(RerunSelection::Marker(marker)), Vec::new(), false),
            None => no_marker(),
        }
    }

    /// Remember the test a Debug click chose, until the listener is up and
    /// [`Self::run_pending_with_env`] is called.
    pub fn prepare_debug_marker(
        self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> ffi::FfiResult {
        match marker_at(path, text, line) {
            Some(marker) => {
                *self.pending_debug.borrow_mut() = Some(RerunSelection::Marker(marker));
                ffi::FfiResult::default()
            }
            None => no_marker(),
        }
    }

    /// Start the remembered test with the Xdebug environment
    /// (`[[key, value], ...]`) `DebugService` computed.
    pub fn run_pending_with_env(self: Pin<&mut Self>, env_json: &QString) -> ffi::FfiResult {
        let Ok(env) = serde_json::from_str::<Vec<(String, String)>>(&env_json.to_string()) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                "the extra environment is not a list of pairs",
            );
        };
        let Some(selection) = self.pending_debug.borrow_mut().take() else {
            return errors::failure(errors::CODE_REFUSED, "no test is waiting to be debugged");
        };
        self.start(Some(selection), env, false)
    }
}

impl ffi::TestService {
    pub fn run_all_with_coverage(self: Pin<&mut Self>) -> ffi::FfiResult {
        self.start(None, Vec::new(), true)
    }

    pub fn run_marker_with_coverage(
        self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> ffi::FfiResult {
        match marker_at(path, text, line) {
            Some(marker) => self.start(Some(RerunSelection::Marker(marker)), Vec::new(), true),
            None => no_marker(),
        }
    }

    /// 0-based lines of `path` whose hit count satisfies `wanted`.
    fn coverage_lines(&self, path: &QString, wanted: impl Fn(u64) -> bool) -> Vec<u32> {
        let path = path.to_string();
        self.coverage
            .borrow()
            .as_ref()
            .and_then(|coverage| coverage.file(std::path::Path::new(&path)).cloned())
            .map(|file| {
                file.lines
                    .iter()
                    .filter(|(_, hits)| wanted(**hits))
                    .map(|(line, _)| line.saturating_sub(1))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn covered_lines(&self, path: &QString) -> Vec<u32> {
        self.coverage_lines(path, |hits| hits > 0)
    }

    pub fn uncovered_lines(&self, path: &QString) -> Vec<u32> {
        self.coverage_lines(path, |hits| hits == 0)
    }

    pub fn coverage_rows(&self) -> Vec<ffi::FfiCoverageRow> {
        let Some(root) = super::current_project_root() else {
            return Vec::new();
        };
        let coverage = self.coverage.borrow();
        let Some(coverage) = coverage.as_ref() else {
            return Vec::new();
        };
        coverage
            .rows(&root)
            .into_iter()
            .map(|row| ffi::FfiCoverageRow {
                path: QString::from(row.path.to_string_lossy().replace('\\', "/").as_str()),
                abs_path: QString::from(root.join(&row.path).to_string_lossy().as_ref()),
                is_file: row.is_file,
                covered: row.covered as u32,
                total: row.total as u32,
                percent: row.percent(),
                has_lines: row.has_lines(),
            })
            .collect()
    }

    pub fn clear_coverage(mut self: Pin<&mut Self>) {
        *self.coverage.borrow_mut() = None;
        self.as_mut().coverage_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FILE: &str =
        "<?php\nclass ATest extends TestCase\n{\n    public function testA() {}\n}\n";

    #[test]
    fn a_test_file_runs_or_stages_the_caret_scope() {
        assert!(matches!(
            plan_caret_run("/p/ATest.php", TEST_FILE, 3, false, true),
            CaretRun::Run(m) if m.marker.name == "testA"
        ));
        assert!(matches!(
            plan_caret_run("/p/ATest.php", TEST_FILE, 3, true, true),
            CaretRun::Debug(m) if m.marker.name == "testA"
        ));
    }

    #[test]
    fn a_non_test_file_runs_plain_and_refuses_debug() {
        let plain = "<?php\nclass A {}\n";
        assert_eq!(
            plan_caret_run("/p/A.php", plain, 0, false, true),
            CaretRun::PlainRun
        );
        assert_eq!(
            plan_caret_run("/p/a.txt", TEST_FILE, 3, false, true),
            CaretRun::PlainRun
        );
        assert_eq!(
            plan_caret_run("/p/A.php", plain, 0, true, true),
            CaretRun::NoTestToDebug
        );
    }

    #[test]
    fn debugging_without_a_debug_service_is_a_typed_refusal() {
        assert_eq!(
            plan_caret_run("/p/ATest.php", TEST_FILE, 3, true, false),
            CaretRun::DebugUnavailable
        );
    }
}
