//! The gutter's test history (PHPUnit editor runs plan, E8's G4): the last
//! result of each test, kept across restarts as PhpStorm keeps its test
//! history, so a test file shows its icons before anything ran in this
//! session.
//!
//! A finished run's leaf results are recorded under their node ids, newest
//! last, at most [`MAX_RESULTS`] of them; the history becomes a
//! [`TestTree`] of those leaves for [`crate::test_status::marker_statuses`],
//! which matches them to the file's current markers by symbol, so a test
//! that no longer exists simply has no marker to show on. A live run's
//! result for a marker supersedes the recorded one
//! ([`crate::test_status::with_history`]).
//!
//! Persistence is `app-config`'s [`TestHistorySettings`].

use std::path::{Path, PathBuf};

use app_config::test_history::{TestHistorySettings, TestResultSetting};
use process_exec::host::ExecHost;
use test_core::{
    NodeKind, TestFailure, TestId, TestLocation, TestNode, TestStatus, TestSymbol, TestTree,
};

/// Results kept per project: a few large suites' worth, a few hundred
/// kilobytes of TOML.
pub const MAX_RESULTS: usize = 2000;

/// The recorded results and the tree the gutter reads them through.
#[derive(Debug, Default)]
pub struct TestHistory {
    results: Vec<TestResultSetting>,
    tree: TestTree,
}

fn status_name(status: TestStatus) -> Option<&'static str> {
    match status {
        TestStatus::Passed => Some("passed"),
        TestStatus::Failed => Some("failed"),
        TestStatus::Skipped => Some("skipped"),
        TestStatus::Running | TestStatus::Pending => None,
    }
}

fn status_of(name: &str) -> Option<TestStatus> {
    match name {
        "passed" => Some(TestStatus::Passed),
        "failed" => Some(TestStatus::Failed),
        "skipped" => Some(TestStatus::Skipped),
        _ => None,
    }
}

/// The symbol as the settings row spells it: kind, class, name.
fn symbol_fields(symbol: &TestSymbol) -> (&'static str, &str, &str) {
    match symbol {
        TestSymbol::Class(class) => ("class", class, ""),
        TestSymbol::Method { class, method } => ("method", class, method),
        TestSymbol::Description(description) => ("description", description, ""),
        TestSymbol::PestClass(class) => ("pest_class", class, ""),
        TestSymbol::PestMethod { class, title } => ("pest_method", class, title),
    }
}

fn symbol_of(row: &TestResultSetting) -> Option<TestSymbol> {
    let (class, name) = (row.class.clone(), row.name.clone());
    Some(match row.symbol.as_str() {
        "class" => TestSymbol::Class(class),
        "method" => TestSymbol::Method {
            class,
            method: name,
        },
        "description" => TestSymbol::Description(class),
        "pest_class" => TestSymbol::PestClass(class),
        "pest_method" => TestSymbol::PestMethod { class, title: name },
        _ => return None,
    })
}

/// The leaf `row` restores; `None` for a row this build cannot read.
fn node_of(row: &TestResultSetting) -> Option<TestNode> {
    let status = status_of(&row.status)?;
    let symbol = symbol_of(row)?;
    let failure = (!row.message.is_empty()).then(|| TestFailure {
        message: row.message.clone(),
        details: String::new(),
    });
    Some(TestNode {
        id: TestId(row.id.clone()),
        name: row.id.rsplit("::").next().unwrap_or(&row.id).to_string(),
        kind: NodeKind::Test,
        status,
        duration_ms: (row.duration_ms > 0).then_some(row.duration_ms),
        failure,
        location: Some(TestLocation {
            file: row.file.clone(),
            line: None,
            symbol: Some(symbol),
        }),
        parent: None,
        children: Vec::new(),
        run: 0,
    })
}

/// The row recording `node`'s result, its file `file` on this machine;
/// `None` for a result that is not final or a test without a symbol.
fn row_of(node: &TestNode, file: &Path) -> Option<TestResultSetting> {
    let status = status_name(node.status)?;
    let symbol = node.location.as_ref()?.symbol.as_ref()?;
    let (kind, class, name) = symbol_fields(symbol);
    Some(TestResultSetting {
        id: node.id.0.clone(),
        file: file.to_string_lossy().into_owned(),
        symbol: kind.to_string(),
        class: class.to_string(),
        name: name.to_string(),
        status: status.to_string(),
        message: node
            .failure
            .as_ref()
            .map(|failure| crate::test_status::first_lines(&failure.message))
            .unwrap_or_default(),
        duration_ms: node.duration_ms.unwrap_or(0),
    })
}

impl TestHistory {
    /// The history `settings` holds, without the rows this build cannot
    /// read. A row for a deleted file or test stays until the cap evicts it:
    /// no open file has a marker it names, and checking every file on load
    /// would cost a stat per row on a `\\wsl.localhost` share.
    pub fn from_settings(settings: TestHistorySettings) -> Self {
        let results: Vec<TestResultSetting> = settings
            .results
            .into_iter()
            .filter(|row| node_of(row).is_some())
            .collect();
        let mut history = Self {
            results,
            tree: TestTree::new(),
        };
        history.truncate();
        history.rebuild();
        history
    }

    /// What to persist.
    pub fn settings(&self) -> TestHistorySettings {
        TestHistorySettings {
            results: self.results.clone(),
        }
    }

    /// The recorded results as a tree of leaves, located by local paths
    /// (map them with [`ExecHost::Local`]).
    pub fn tree(&self) -> &TestTree {
        &self.tree
    }

    /// Record every finished leaf of `tree`, each located through its own
    /// run's place (`host` and `work_dir` for a node from no known run).
    /// Whether anything changed, so an unchanged history is not rewritten.
    pub fn record(&mut self, tree: &TestTree, host: &ExecHost, work_dir: &Path) -> bool {
        let mut leaves: Vec<TestId> = tree
            .roots()
            .iter()
            .flat_map(|root| tree.leaves_under(root))
            .collect();
        leaves.dedup();
        let rows: Vec<TestResultSetting> = leaves
            .iter()
            .filter_map(|id| tree.node(id))
            .filter_map(|node| {
                let file: PathBuf = tree.local_path(node, host, work_dir)?;
                row_of(node, &file)
            })
            .collect();
        let before = self.results.clone();
        for row in rows {
            self.results.retain(|kept| kept.id != row.id);
            self.results.push(row);
        }
        self.truncate();
        let changed = self.results != before;
        if changed {
            self.rebuild();
        }
        changed
    }

    fn truncate(&mut self) {
        let excess = self.results.len().saturating_sub(MAX_RESULTS);
        self.results.drain(..excess);
    }

    fn rebuild(&mut self) {
        self.tree = TestTree::restored(self.results.iter().filter_map(node_of));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_core::TeamCityEvent;

    const FILE: &str = "/p/tests/GreeterTest.php";

    fn hint(symbol: &str) -> Option<TestLocation> {
        test_core::parse_location_hint(&format!(r"php_qn://{FILE}::{symbol}"))
    }

    /// A live tree with `testA` passed and `testB` failed.
    fn live_tree() -> TestTree {
        let mut tree = TestTree::new();
        let suite = Some(TestId(r"App\GreeterTest".into()));
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: r"App\GreeterTest".into(),
            location: hint(r"\App\GreeterTest"),
        });
        for (name, fails) in [("testA", false), ("testB", true)] {
            tree.apply(TeamCityEvent::TestStarted {
                parent: suite.clone(),
                name: name.into(),
                location: hint(&format!(r"\App\GreeterTest::{name}")),
            });
            if fails {
                tree.apply(TeamCityEvent::TestFailed {
                    parent: suite.clone(),
                    name: name.into(),
                    message: "Failed asserting that false is true.\nline two\nthree\nfour".into(),
                    details: String::new(),
                });
            }
            tree.apply(TeamCityEvent::TestFinished {
                parent: suite.clone(),
                name: name.into(),
                duration_ms: Some(7),
            });
        }
        tree
    }

    fn recorded() -> TestHistory {
        let mut history = TestHistory::default();
        assert!(history.record(&live_tree(), &ExecHost::Local, Path::new("/p")));
        history
    }

    #[test]
    fn a_finished_run_is_recorded_and_restored_as_leaves() {
        let history = recorded();
        let settings = history.settings();
        assert_eq!(settings.results.len(), 2);
        let failed = &settings.results[1];
        assert_eq!(failed.id, r"App\GreeterTest::testB");
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.symbol, "method");
        assert_eq!(failed.file, FILE);
        assert_eq!(
            failed.message,
            "Failed asserting that false is true.\nline two\nthree"
        );

        let restored = TestHistory::from_settings(settings);
        let nodes = restored
            .tree()
            .nodes_in_file(&ExecHost::Local, Path::new(""), Path::new(FILE));
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].status, TestStatus::Failed);
        assert_eq!(nodes[0].duration_ms, Some(7));
    }

    #[test]
    fn recording_the_same_results_again_changes_nothing() {
        let mut history = recorded();
        assert!(!history.record(&live_tree(), &ExecHost::Local, Path::new("/p")));
        assert!(!history.record(&TestTree::new(), &ExecHost::Local, Path::new("/p")));
    }

    #[test]
    fn a_running_test_is_not_recorded() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestStarted {
            parent: None,
            name: "testC".into(),
            location: hint(r"\App\GreeterTest::testC"),
        });
        let mut history = TestHistory::default();
        assert!(!history.record(&tree, &ExecHost::Local, Path::new("/p")));
    }

    #[test]
    fn unreadable_rows_are_dropped_on_load() {
        let mut settings = recorded().settings();
        settings.results.push(TestResultSetting {
            symbol: "something newer".into(),
            ..settings.results[0].clone()
        });
        settings.results.push(TestResultSetting {
            status: "running".into(),
            ..settings.results[0].clone()
        });
        let history = TestHistory::from_settings(settings);
        assert_eq!(history.settings().results.len(), 2);
    }

    #[test]
    fn a_recorded_test_that_no_longer_exists_shows_on_no_marker() {
        let history = recorded();
        let text = "<?php\nnamespace App;\nclass GreeterTest extends TestCase\n{\n    public function testRenamed() {}\n}\n";
        let nodes = history
            .tree()
            .nodes_in_file(&ExecHost::Local, Path::new(""), Path::new(FILE));
        let statuses = crate::test_status::marker_statuses(
            &crate::tests::markers(text),
            history.tree(),
            &nodes,
        );
        assert!(statuses
            .iter()
            .all(|s| s.state == crate::test_status::MarkerState::NeverRun));
    }

    #[test]
    fn the_oldest_results_go_past_the_cap() {
        let row = |n: usize| TestResultSetting {
            id: format!("T::test{n}"),
            file: FILE.into(),
            symbol: "method".into(),
            class: "T".into(),
            name: format!("test{n}"),
            status: "passed".into(),
            ..TestResultSetting::default()
        };
        let settings = TestHistorySettings {
            results: (0..MAX_RESULTS + 5).map(row).collect(),
        };
        let history = TestHistory::from_settings(settings);
        let results = history.settings().results;
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[0].id, "T::test5");
    }

    #[test]
    fn every_symbol_kind_round_trips() {
        let symbols = [
            TestSymbol::Class("A".into()),
            TestSymbol::Method {
                class: "A".into(),
                method: "m".into(),
            },
            TestSymbol::Description("it works".into()),
            TestSymbol::PestClass("A".into()),
            TestSymbol::PestMethod {
                class: "A".into(),
                title: "Works".into(),
            },
        ];
        for symbol in symbols {
            let (kind, class, name) = symbol_fields(&symbol);
            let row = TestResultSetting {
                symbol: kind.into(),
                class: class.into(),
                name: name.into(),
                ..TestResultSetting::default()
            };
            assert_eq!(symbol_of(&row), Some(symbol));
        }
    }
}
