//! What the editor gutter shows on a test marker: the last run's result
//! (PhpStorm's green check, red cross, grey "ignored" or plain run arrow).
//!
//! Markers come from the file's current text ([`crate::tests::markers`]);
//! results come from the run's [`TestTree`]. They meet through the
//! `locationHint` symbol each node carries, so a result follows its method
//! when edits move it, and a method that no longer exists simply has none.
//! A class or `describe` marker aggregates the tests inside its span.

use std::collections::HashSet;

use test_core::{TestId, TestNode, TestStatus, TestTree};

use crate::tests::{marker_names, markers, MarkerScope, TestMarker};

/// How many data sets a gutter menu lists before it says "…".
const MAX_MENU_DATA_SETS: usize = 15;
/// Lines of a failure message a tooltip keeps.
const TOOLTIP_MESSAGE_LINES: usize = 3;
const DATA_SET_INFIX: &str = " with data set ";

/// The icon a marker wears. Aggregates follow IntelliJ: running, then
/// failed, then ignored, then passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerState {
    NeverRun,
    Running,
    Passed,
    Failed,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerStatus {
    /// 0-based line, as [`TestMarker::line`].
    pub line: u32,
    pub state: MarkerState,
    /// Empty for [`MarkerState::NeverRun`].
    pub tooltip: String,
}

/// One data set of a method, to run on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSet {
    pub id: TestId,
    /// `testMethod with data set "name"`, as the Tests dock shows it.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataSetChoices {
    pub shown: Vec<DataSet>,
    /// How many more the cap left out.
    pub hidden: usize,
}

#[derive(Default)]
struct Tally {
    running: usize,
    failed: usize,
    ignored: usize,
    passed: usize,
    duration_ms: u64,
    first_failure: Option<String>,
    first_ignore: Option<String>,
}

impl Tally {
    fn add(&mut self, leaf: &TestNode) {
        let message = || leaf.failure.as_ref().map(|f| first_lines(&f.message));
        match leaf.status {
            TestStatus::Running => self.running += 1,
            TestStatus::Failed => {
                self.failed += 1;
                self.first_failure = self.first_failure.take().or_else(message);
            }
            TestStatus::Skipped => {
                self.ignored += 1;
                self.first_ignore = self.first_ignore.take().or_else(message);
            }
            TestStatus::Passed => {
                self.passed += 1;
                self.duration_ms += leaf.duration_ms.unwrap_or(0);
            }
            TestStatus::Pending => {}
        }
    }

    fn merge(&mut self, other: &Tally) {
        self.running += other.running;
        self.failed += other.failed;
        self.ignored += other.ignored;
        self.passed += other.passed;
        self.duration_ms += other.duration_ms;
        self.first_failure = self.first_failure.take().or(other.first_failure.clone());
        self.first_ignore = self.first_ignore.take().or(other.first_ignore.clone());
    }

    fn total(&self) -> usize {
        self.running + self.failed + self.ignored + self.passed
    }

    fn status(&self) -> (MarkerState, String) {
        if self.running > 0 {
            (MarkerState::Running, "Running…".into())
        } else if self.failed > 0 {
            let tooltip = match (&self.first_failure, self.failed) {
                (Some(message), 1) if !message.is_empty() => message.clone(),
                _ => format!("{} of {} tests failed", self.failed, self.total()),
            };
            (MarkerState::Failed, tooltip)
        } else if self.ignored > 0 {
            let tooltip = match &self.first_ignore {
                Some(message) if !message.is_empty() => format!("Ignored: {message}"),
                _ => "Ignored".into(),
            };
            (MarkerState::Ignored, tooltip)
        } else if self.passed > 0 {
            let tests = if self.passed > 1 {
                format!("{} tests, ", self.passed)
            } else {
                String::new()
            };
            (
                MarkerState::Passed,
                format!("Passed ({tests}{} ms)", self.duration_ms),
            )
        } else {
            (MarkerState::NeverRun, String::new())
        }
    }
}

fn first_lines(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(TOOLTIP_MESSAGE_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The distinct leaf tests, in tree order, of the nodes that name `marker`.
fn leaves_of<'t>(
    marker: &TestMarker,
    tree: &'t TestTree,
    file_nodes: &[&TestNode],
) -> Vec<&'t TestNode> {
    let mut seen = HashSet::new();
    file_nodes
        .iter()
        .filter(|node| {
            node.location
                .as_ref()
                .and_then(|l| l.symbol.as_ref())
                .is_some_and(|symbol| marker_names(marker, symbol))
        })
        .flat_map(|node| tree.leaves_under(&node.id))
        .filter(|id| seen.insert(id.clone()))
        .filter_map(|id| tree.node(&id))
        .collect()
}

/// The status of every marker in `text`. `file_nodes` is the tree's nodes
/// located in this file ([`TestTree::nodes_in_file`]).
pub fn marker_statuses(text: &str, tree: &TestTree, file_nodes: &[&TestNode]) -> Vec<MarkerStatus> {
    let all = markers(text);
    let tallies: Vec<Option<Tally>> = all
        .iter()
        .map(|marker| {
            (marker.scope == MarkerScope::Test).then(|| {
                let mut tally = Tally::default();
                leaves_of(marker, tree, file_nodes)
                    .into_iter()
                    .for_each(|leaf| tally.add(leaf));
                tally
            })
        })
        .collect();
    all.iter()
        .enumerate()
        .map(|(index, marker)| {
            let tally = match &tallies[index] {
                Some(own) => own.status(),
                None => {
                    let mut group = Tally::default();
                    for (inner, tally) in all.iter().zip(&tallies) {
                        let inside = inner.span.0 >= marker.span.0 && inner.span.1 <= marker.span.1;
                        if let Some(tally) = tally.as_ref().filter(|_| inside) {
                            group.merge(tally);
                        }
                    }
                    group.status()
                }
            };
            MarkerStatus {
                line: marker.line,
                state: tally.0,
                tooltip: tally.1,
            }
        })
        .collect()
}

/// The data sets the last run executed for the method marked on `line`;
/// empty for anything else.
pub fn data_sets(
    text: &str,
    line: u32,
    tree: &TestTree,
    file_nodes: &[&TestNode],
) -> DataSetChoices {
    let Some(marker) = markers(text)
        .into_iter()
        .find(|m| m.line == line && m.scope == MarkerScope::Test)
    else {
        return DataSetChoices::default();
    };
    let mut sets: Vec<DataSet> = leaves_of(&marker, tree, file_nodes)
        .into_iter()
        .filter(|leaf| leaf.name.contains(DATA_SET_INFIX))
        .map(|leaf| DataSet {
            id: leaf.id.clone(),
            label: leaf.name.clone(),
        })
        .collect();
    let hidden = sets.len().saturating_sub(MAX_MENU_DATA_SETS);
    sets.truncate(MAX_MENU_DATA_SETS);
    DataSetChoices {
        shown: sets,
        hidden,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use process_exec::host::ExecHost;
    use test_core::TeamCityEvent;

    use super::*;

    const FILE: &str = "/p/tests/GreeterTest.php";
    const SOURCE: &str = "<?php\nnamespace App;\nclass GreeterTest extends TestCase\n{\n    public function testA() {}\n    public function testB() {}\n    public function testSets() {}\n}\n";
    // Lines: class 2, testA 4, testB 5, testSets 6.

    fn hint(symbol: &str) -> Option<test_core::TestLocation> {
        test_core::parse_location_hint(&format!(r"php_qn://{FILE}::{symbol}"))
    }

    fn id(path: &str) -> TestId {
        TestId(path.to_string())
    }

    /// Run one test in the tree: `outcome` is `pass`, `fail:<msg>`, `skip` or `run`.
    fn run(tree: &mut TestTree, parent: &str, name: &str, outcome: &str, ms: u64) {
        let parent_id = Some(id(parent));
        let method = name.split(" with data set").next().unwrap();
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent_id.clone(),
            name: name.into(),
            location: hint(&format!(r"\App\GreeterTest::{method}")),
        });
        match outcome {
            "run" => return,
            "skip" => tree.apply(TeamCityEvent::TestIgnored {
                parent: parent_id.clone(),
                name: name.into(),
                message: "not yet".into(),
            }),
            fail if fail.starts_with("fail:") => tree.apply(TeamCityEvent::TestFailed {
                parent: parent_id.clone(),
                name: name.into(),
                message: fail[5..].into(),
                details: String::new(),
            }),
            _ => {}
        }
        tree.apply(TeamCityEvent::TestFinished {
            parent: parent_id,
            name: name.into(),
            duration_ms: Some(ms),
        });
    }

    fn tree_with_class() -> TestTree {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: r"App\GreeterTest".into(),
            location: hint(r"\App\GreeterTest"),
        });
        tree
    }

    fn statuses(tree: &TestTree) -> Vec<MarkerStatus> {
        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        marker_statuses(SOURCE, tree, &nodes)
    }

    fn state_at(statuses: &[MarkerStatus], line: u32) -> &MarkerStatus {
        statuses.iter().find(|s| s.line == line).unwrap()
    }

    const CLASS: &str = r"App\GreeterTest";

    #[test]
    fn markers_without_a_run_are_never_run() {
        let statuses = statuses(&TestTree::new());
        assert_eq!(statuses.len(), 4);
        assert!(statuses
            .iter()
            .all(|s| s.state == MarkerState::NeverRun && s.tooltip.is_empty()));
    }

    #[test]
    fn a_method_shows_its_result_and_the_class_aggregates_failed_over_passed() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "pass", 12);
        run(
            &mut tree,
            CLASS,
            "testB",
            "fail:Failed asserting that 1 is 2.\n\nat line 9",
            3,
        );
        let statuses = statuses(&tree);
        let a = state_at(&statuses, 4);
        assert_eq!(
            (a.state, a.tooltip.as_str()),
            (MarkerState::Passed, "Passed (12 ms)")
        );
        let b = state_at(&statuses, 5);
        assert_eq!(b.state, MarkerState::Failed);
        assert_eq!(b.tooltip, "Failed asserting that 1 is 2.\nat line 9");
        assert_eq!(state_at(&statuses, 6).state, MarkerState::NeverRun);
        let class = state_at(&statuses, 2);
        assert_eq!(class.state, MarkerState::Failed);
        assert_eq!(class.tooltip, b.tooltip);
    }

    #[test]
    fn several_failures_are_counted_in_the_tooltip() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "fail:x", 1);
        run(&mut tree, CLASS, "testB", "fail:y", 1);
        run(&mut tree, CLASS, "testSets", "pass", 1);
        assert_eq!(state_at(&statuses(&tree), 2).tooltip, "2 of 3 tests failed");
    }

    #[test]
    fn ignored_ranks_between_failed_and_passed() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "pass", 1);
        run(&mut tree, CLASS, "testB", "skip", 0);
        let statuses = statuses(&tree);
        let class = state_at(&statuses, 2);
        assert_eq!(class.state, MarkerState::Ignored);
        assert_eq!(class.tooltip, "Ignored: not yet");
        assert_eq!(state_at(&statuses, 5).state, MarkerState::Ignored);
    }

    #[test]
    fn a_class_of_passing_tests_is_passed_with_the_count() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "pass", 2);
        run(&mut tree, CLASS, "testB", "pass", 3);
        let statuses = statuses(&tree);
        let class = state_at(&statuses, 2);
        assert_eq!(
            (class.state, class.tooltip.as_str()),
            (MarkerState::Passed, "Passed (2 tests, 5 ms)")
        );
    }

    #[test]
    fn a_started_test_is_running_and_wins_over_failures() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "fail:boom", 1);
        run(&mut tree, CLASS, "testB", "run", 0);
        let statuses = statuses(&tree);
        assert_eq!(state_at(&statuses, 5).state, MarkerState::Running);
        assert_eq!(state_at(&statuses, 2).state, MarkerState::Running);
        assert_eq!(state_at(&statuses, 4).state, MarkerState::Failed);
    }

    #[test]
    fn data_sets_aggregate_into_their_method() {
        let mut tree = tree_with_class();
        let suite = format!(r"{CLASS}::testSets");
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: Some(id(CLASS)),
            name: "testSets".into(),
            location: hint(r"\App\GreeterTest::testSets"),
        });
        run(
            &mut tree,
            &suite,
            r#"testSets with data set "one""#,
            "pass",
            1,
        );
        run(
            &mut tree,
            &suite,
            r#"testSets with data set "two""#,
            "fail:no",
            1,
        );
        let statuses = statuses(&tree);
        let sets = state_at(&statuses, 6);
        assert_eq!(
            (sets.state, sets.tooltip.as_str()),
            (MarkerState::Failed, "no")
        );

        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        let choices = data_sets(SOURCE, 6, &tree, &nodes);
        let labels: Vec<_> = choices.shown.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                r#"testSets with data set "one""#,
                r#"testSets with data set "two""#
            ]
        );
        assert_eq!(choices.hidden, 0);
        assert!(data_sets(SOURCE, 4, &tree, &nodes).shown.is_empty());
        assert!(data_sets(SOURCE, 2, &tree, &nodes).shown.is_empty());
    }

    #[test]
    fn the_data_set_list_is_capped_with_the_overflow_counted() {
        let mut tree = tree_with_class();
        let suite = format!(r"{CLASS}::testSets");
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: Some(id(CLASS)),
            name: "testSets".into(),
            location: hint(r"\App\GreeterTest::testSets"),
        });
        for n in 0..20 {
            run(
                &mut tree,
                &suite,
                &format!("testSets with data set #{n}"),
                "pass",
                1,
            );
        }
        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        let choices = data_sets(SOURCE, 6, &tree, &nodes);
        assert_eq!(
            (choices.shown.len(), choices.hidden),
            (MAX_MENU_DATA_SETS, 5)
        );
    }

    #[test]
    fn a_result_follows_its_method_when_edits_move_it() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testB", "fail:boom", 1);
        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        let moved = SOURCE.replacen("<?php\n", "<?php\n// one\n// two\n", 1);
        let statuses = marker_statuses(&moved, &tree, &nodes);
        assert_eq!(state_at(&statuses, 7).state, MarkerState::Failed);
        assert_eq!(state_at(&statuses, 6).state, MarkerState::NeverRun);
    }
}
