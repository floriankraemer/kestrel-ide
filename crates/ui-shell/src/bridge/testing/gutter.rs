//! The test gutter's half of `TestService` (PHP parity plan T3): which
//! lines of a PHP file carry a Run/Debug marker, and starting the one
//! test a marker names.
//!
//! Translation only: where the markers go and what filter each runs is
//! `php_core::tests::markers`'s; the framework, its argv and its output
//! parsing are the ordinary `start` path's.

use std::pin::Pin;

use cxx_qt_lib::QString;

use super::{MarkerSelection, RerunSelection};
use crate::bridge::errors;
use crate::bridge::ffi;

fn is_php_file(path: &str) -> bool {
    syntax_core::language_for_path(std::path::Path::new(path)).id() == "php"
}

fn selection(path: String, marker: php_core::tests::TestMarker) -> MarkerSelection {
    MarkerSelection {
        is_class: marker.scope != php_core::tests::MarkerScope::Test,
        filter: marker.filter,
        name: marker.name,
        file: path,
    }
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
    }
}

fn no_marker() -> ffi::FfiResult {
    errors::failure(errors::CODE_REFUSED, "there is no test on this line")
}

impl ffi::TestService {
    /// The 0-based lines of `text` that carry a test marker; empty for a
    /// file that is not a PHP test.
    pub fn marker_lines(&self, path: &QString, text: &QString) -> Vec<u32> {
        if !is_php_file(&path.to_string()) {
            return Vec::new();
        }
        php_core::tests::markers(&text.to_string())
            .into_iter()
            .map(|marker| marker.line)
            .collect()
    }

    /// The last run's result for every marker in `text`, for the gutter
    /// icons; empty for a file that is not a PHP test.
    pub fn marker_statuses(&self, path: &QString, text: &QString) -> Vec<ffi::FfiMarkerStatus> {
        let path = path.to_string();
        if !is_php_file(&path) {
            return Vec::new();
        }
        let tree = self.tree.borrow();
        let nodes = self.nodes_in(&tree, &path);
        php_core::test_status::marker_statuses(&text.to_string(), &tree, &nodes)
            .into_iter()
            .map(|status| ffi::FfiMarkerStatus {
                line: status.line,
                state: to_ffi_marker_state(status.state),
                tooltip: QString::from(status.tooltip.as_str()),
            })
            .collect()
    }

    /// The data sets the last run executed for the method marked on `line`,
    /// each runnable on its own. An entry with an empty `id` stands for the
    /// ones the menu left out.
    pub fn marker_data_sets(
        &self,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> Vec<ffi::FfiDataSet> {
        let path = path.to_string();
        if !is_php_file(&path) {
            return Vec::new();
        }
        let tree = self.tree.borrow();
        let nodes = self.nodes_in(&tree, &path);
        let choices = php_core::test_status::data_sets(&text.to_string(), line, &tree, &nodes);
        let more = (choices.hidden > 0).then(|| ffi::FfiDataSet {
            id: QString::default(),
            label: QString::from(format!("… ({} more)", choices.hidden).as_str()),
        });
        choices
            .shown
            .into_iter()
            .map(|set| ffi::FfiDataSet {
                id: QString::from(set.id.as_str()),
                label: QString::from(set.label.as_str()),
            })
            .chain(more)
            .collect()
    }

    /// Run one data set of the last run, by the id `marker_data_sets` gave.
    pub fn run_data_set(self: Pin<&mut Self>, id: &QString) -> ffi::FfiResult {
        self.start(
            Some(RerunSelection::Node(test_core::TestId(id.to_string()))),
            Vec::new(),
            false,
        )
    }

    /// Stage one data set for debugging, like [`Self::prepare_debug_marker`].
    pub fn prepare_debug_data_set(self: Pin<&mut Self>, id: &QString) -> ffi::FfiResult {
        *self.pending_debug.borrow_mut() =
            Some(RerunSelection::Node(test_core::TestId(id.to_string())));
        ffi::FfiResult::default()
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
        let host = (*self.run_host.borrow())
            .clone()
            .unwrap_or_else(|| process_exec::host::ExecHost::for_path(&root));
        tree.nodes_in_file(&host, &root, std::path::Path::new(path))
    }

    /// The name a marker's menu shows, empty when `line` has no marker.
    pub fn marker_name(&self, path: &QString, text: &QString, line: u32) -> QString {
        marker_at(path, text, line)
            .map(|marker| QString::from(marker.name.as_str()))
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
            CaretRun::Run(m) if m.name == "testA"
        ));
        assert!(matches!(
            plan_caret_run("/p/ATest.php", TEST_FILE, 3, true, true),
            CaretRun::Debug(m) if m.name == "testA"
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
