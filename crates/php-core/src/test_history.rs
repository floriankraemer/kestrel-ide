//! The gutter's test history (PHPUnit editor runs plan, E8's G4): the last
//! result of each test, kept across restarts as PhpStorm keeps its test
//! history, so a test file shows its icons before anything ran in this
//! session.
//!
//! A finished run's leaf results are recorded under their node ids, newest
//! last, at most [`MAX_RESULTS_PER_FILE`] per test file and [`MAX_RESULTS`]
//! in all; the history becomes a
//! [`TestTree`] of those leaves for [`crate::test_status::marker_statuses`],
//! which matches them to the file's current markers by symbol, so a test
//! that no longer exists simply has no marker to show on. A live run's
//! result for a test supersedes the recorded one
//! ([`crate::test_status::marker_statuses_with_history`]).
//!
//! Persistence is `app-config`'s [`TestHistorySettings`]. Every row carries
//! when it was recorded, so two writers (two runs' saves, two IDE instances
//! on one project) merge rather than erase each other: [`merged`] keeps the
//! newer row per test, whichever order the saves land in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use app_config::test_history::{TestHistorySettings, TestResultSetting};
use process_exec::host::ExecHost;
use test_core::{
    Comparison, NodeKind, TestFailure, TestId, TestLocation, TestNode, TestStatus, TestSymbol,
    TestTree,
};

/// Results kept per project: several large suites' worth, a couple of
/// megabytes of TOML, read on a worker.
pub const MAX_RESULTS: usize = 10_000;
/// Results kept per test file, so one file's huge data provider cannot push
/// every other file's results out.
pub const MAX_RESULTS_PER_FILE: usize = 1_000;

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
        TestStatus::Terminated => Some("terminated"),
        TestStatus::Running | TestStatus::Pending => None,
    }
}

fn status_of(name: &str) -> Option<TestStatus> {
    match name {
        "passed" => Some(TestStatus::Passed),
        "failed" => Some(TestStatus::Failed),
        "skipped" => Some(TestStatus::Skipped),
        "terminated" => Some(TestStatus::Terminated),
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
    let comparison = (!row.expected.is_empty() || !row.actual.is_empty()).then(|| Comparison {
        expected: row.expected.clone(),
        actual: row.actual.clone(),
    });
    let failure = (!row.message.is_empty()).then(|| TestFailure {
        message: row.message.clone(),
        details: String::new(),
        comparison,
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

/// The row recording `node`'s result, its file `file` on this machine,
/// recorded at `now` (ms since the epoch); `None` for a result that is not
/// final or a test without a symbol.
fn row_of(node: &TestNode, file: &Path, now: u64) -> Option<TestResultSetting> {
    let status = status_name(node.status)?;
    let symbol = node.location.as_ref()?.symbol.as_ref()?;
    let (kind, class, name) = symbol_fields(symbol);
    let comparison = node.failure.as_ref().and_then(|f| f.comparison.as_ref());
    // A comparison can hold a whole serialized object; the tooltip keeps a
    // few lines of it, and so does the history.
    let side = |pick: fn(&Comparison) -> &str| {
        comparison
            .map(|c| crate::test_status::first_lines(pick(c)))
            .unwrap_or_default()
    };
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
        expected: side(|c| &c.expected),
        actual: side(|c| &c.actual),
        recorded_at: now,
    })
}

/// Whether two rows record the same result, whenever each was recorded.
fn same_result(a: &TestResultSetting, b: &TestResultSetting) -> bool {
    TestResultSetting {
        recorded_at: 0,
        ..a.clone()
    } == TestResultSetting {
        recorded_at: 0,
        ..b.clone()
    }
}

/// `ours` merged into `on_disk`: per test id the newer row (ours on a tie),
/// oldest first, capped. What a save writes over whatever another writer
/// left on disk.
pub fn merged(on_disk: TestHistorySettings, ours: &TestHistorySettings) -> TestHistorySettings {
    let mut rows: Vec<TestResultSetting> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for row in on_disk
        .results
        .into_iter()
        .chain(ours.results.iter().cloned())
    {
        match at.get(&row.id) {
            Some(&index) if rows[index].recorded_at > row.recorded_at => {}
            Some(&index) => rows[index] = row,
            None => {
                at.insert(row.id.clone(), rows.len());
                rows.push(row);
            }
        }
    }
    rows.sort_by_key(|row| row.recorded_at);
    TestHistorySettings {
        results: capped(rows),
    }
}

/// `rows` (oldest first) without the oldest past each file's cap and the
/// overall cap.
fn capped(rows: Vec<TestResultSetting>) -> Vec<TestResultSetting> {
    let mut per_file: HashMap<&str, usize> = HashMap::new();
    let mut keep = vec![false; rows.len()];
    for (index, row) in rows.iter().enumerate().rev() {
        let count = per_file.entry(row.file.as_str()).or_default();
        *count += 1;
        keep[index] = *count <= MAX_RESULTS_PER_FILE;
    }
    let mut kept: Vec<TestResultSetting> = rows
        .into_iter()
        .zip(keep)
        .filter(|(_, keep)| *keep)
        .map(|(row, _)| row)
        .collect();
    let excess = kept.len().saturating_sub(MAX_RESULTS);
    kept.drain(..excess);
    kept
}

/// Now, in ms since the epoch: when a row is recorded.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl TestHistory {
    /// The history `settings` holds, without the rows this build cannot
    /// read. A row for a deleted file or test stays until the cap evicts it:
    /// no open file has a marker it names, and checking every file on load
    /// would cost a stat per row on a `\\wsl.localhost` share.
    pub fn from_settings(settings: TestHistorySettings) -> Self {
        let mut history = Self::default();
        history.merge(settings);
        history
    }

    /// Take in rows another writer recorded (the file as loaded, or as a
    /// save left it): the newer row per test wins.
    pub fn merge(&mut self, settings: TestHistorySettings) {
        let readable = TestHistorySettings {
            results: settings
                .results
                .into_iter()
                .filter(|row| node_of(row).is_some())
                .collect(),
        };
        let ours = self.settings();
        self.results = merged(readable, &ours).results;
        self.rebuild();
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
    /// run's place (`host` and `work_dir` for a node from no known run), as
    /// recorded at `now`. Whether anything changed, so an unchanged history
    /// is not rewritten.
    pub fn record(&mut self, tree: &TestTree, host: &ExecHost, work_dir: &Path, now: u64) -> bool {
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
                row_of(node, &file, now)
            })
            .collect();
        let index: HashMap<&str, &TestResultSetting> = self
            .results
            .iter()
            .map(|row| (row.id.as_str(), row))
            .collect();
        let fresh: Vec<TestResultSetting> = rows
            .into_iter()
            .filter(|row| {
                !index
                    .get(row.id.as_str())
                    .is_some_and(|kept| same_result(kept, row))
            })
            .collect();
        if fresh.is_empty() {
            return false;
        }
        let ids: std::collections::HashSet<&str> =
            fresh.iter().map(|row| row.id.as_str()).collect();
        let mut results: Vec<TestResultSetting> = self
            .results
            .iter()
            .filter(|kept| !ids.contains(kept.id.as_str()))
            .cloned()
            .collect();
        results.extend(fresh);
        self.results = capped(results);
        self.rebuild();
        true
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
    const NOW: u64 = 1_000;

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
                    comparison: Some(Comparison {
                        expected: "true".into(),
                        actual: "false".into(),
                    }),
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
        assert!(history.record(&live_tree(), &ExecHost::Local, Path::new("/p"), NOW));
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
        assert!(!history.record(&live_tree(), &ExecHost::Local, Path::new("/p"), NOW));
        assert!(!history.record(&TestTree::new(), &ExecHost::Local, Path::new("/p"), NOW));
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
        assert!(!history.record(&tree, &ExecHost::Local, Path::new("/p"), NOW));
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

    fn row(n: usize, file: &str, at: u64) -> TestResultSetting {
        TestResultSetting {
            id: format!("T::test{n}"),
            file: file.into(),
            symbol: "method".into(),
            class: "T".into(),
            name: format!("test{n}"),
            status: "passed".into(),
            recorded_at: at,
            ..TestResultSetting::default()
        }
    }

    #[test]
    fn the_oldest_results_go_past_the_cap() {
        let settings = TestHistorySettings {
            results: (0..MAX_RESULTS + 5)
                .map(|n| row(n, &format!("/p/T{}.php", n % 20), n as u64))
                .collect(),
        };
        let history = TestHistory::from_settings(settings);
        let results = history.settings().results;
        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[0].id, "T::test5");
    }

    #[test]
    fn one_file_past_its_cap_keeps_the_other_files_results() {
        let mut results = vec![row(0, "/p/Other.php", 0)];
        results.extend((1..=MAX_RESULTS_PER_FILE + 3).map(|n| row(n, FILE, n as u64)));
        let history = TestHistory::from_settings(TestHistorySettings { results });
        let results = history.settings().results;
        assert_eq!(results.len(), MAX_RESULTS_PER_FILE + 1);
        assert_eq!(results[0].id, "T::test0");
        assert_eq!(results[1].id, "T::test4");
    }

    #[test]
    fn a_merge_keeps_the_newer_row_of_each_test_from_either_writer() {
        let disk = TestHistorySettings {
            results: vec![row(1, FILE, 5), row(2, FILE, 9)],
        };
        let ours = TestHistorySettings {
            results: vec![
                TestResultSetting {
                    status: "failed".into(),
                    ..row(1, FILE, 7)
                },
                TestResultSetting {
                    status: "failed".into(),
                    ..row(2, FILE, 3)
                },
                row(3, FILE, 8),
            ],
        };
        let merged = merged(disk.clone(), &ours);
        let status = |id: &str| {
            merged
                .results
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.status.clone())
        };
        assert_eq!(status("T::test1").as_deref(), Some("failed"));
        assert_eq!(status("T::test2").as_deref(), Some("passed"));
        assert_eq!(status("T::test3").as_deref(), Some("passed"));
        // Oldest first, and the order the two land in does not matter.
        let ids = |settings: &TestHistorySettings| -> Vec<String> {
            settings.results.iter().map(|row| row.id.clone()).collect()
        };
        assert_eq!(ids(&merged), ["T::test1", "T::test3", "T::test2"]);
        assert_eq!(ids(&super::merged(ours, &disk)), ids(&merged));
    }

    #[test]
    fn a_loaded_history_merges_into_one_recorded_meanwhile() {
        let mut history = recorded();
        history.merge(TestHistorySettings {
            results: vec![TestResultSetting {
                id: r"App\GreeterTest::testA".into(),
                status: "failed".into(),
                ..row(9, FILE, NOW - 1)
            }],
        });
        let results = history.settings().results;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|row| row.recorded_at == NOW));
    }

    #[test]
    fn a_comparison_and_a_terminated_test_round_trip() {
        let settings = recorded().settings();
        assert_eq!(
            (
                settings.results[1].expected.as_str(),
                settings.results[1].actual.as_str()
            ),
            ("true", "false")
        );
        let restored = TestHistory::from_settings(settings);
        let node = restored
            .tree()
            .node(&TestId(r"App\GreeterTest::testB".into()))
            .unwrap();
        assert_eq!(
            node.failure.as_ref().unwrap().comparison,
            Some(Comparison {
                expected: "true".into(),
                actual: "false".into()
            })
        );
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestStarted {
            parent: None,
            name: "testC".into(),
            location: hint(r"\App\GreeterTest::testC"),
        });
        tree.finish_run();
        let mut history = TestHistory::default();
        assert!(history.record(&tree, &ExecHost::Local, Path::new("/p"), NOW));
        assert_eq!(history.settings().results[0].status, "terminated");
        let restored = TestHistory::from_settings(history.settings());
        assert_eq!(restored.tree().roots().len(), 1);
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
