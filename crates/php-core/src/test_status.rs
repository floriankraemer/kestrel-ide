//! What the editor gutter shows on a test marker: the last run's result
//! (PhpStorm's green check, red cross, grey "ignored" or plain run arrow).
//!
//! Markers come from the file's current text ([`crate::tests::markers`]);
//! results come from the run's [`TestTree`]. They meet through the
//! `locationHint` symbol each node carries, so a result follows its method
//! when edits move it, and a method that no longer exists simply has none.
//! A class or `describe` marker aggregates the tests inside its span, and
//! the tests the framework reported on the class itself (a failing
//! `setUpBeforeClass` is a `testFailed` named after the class).
//!
//! The result is typed counts and a message, never prose: the view words
//! them (and translates them).

use std::collections::HashSet;

use test_core::{Comparison, TestId, TestNode, TestStatus, TestSymbol, TestTree};

use crate::tests::{marker_names, MarkerScope, TestMarker};

/// How many data sets a gutter menu lists before it says "…".
const MAX_MENU_DATA_SETS: usize = 15;
/// Lines of a failure message a tooltip keeps.
const TOOLTIP_MESSAGE_LINES: usize = 3;

/// The icon a marker wears. Aggregates follow IntelliJ: running, then
/// failed, then ignored, then passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerState {
    NeverRun,
    Running,
    Passed,
    Failed,
    /// A test the run ended before it reported (Stop, `exit()`).
    Terminated,
    Ignored,
}

/// A marker's result, as numbers the view words itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerStatus {
    /// 0-based line, as [`TestMarker::line`].
    pub line: u32,
    pub state: MarkerState,
    /// Tests behind the marker (a method with data sets counts each set).
    pub total: u32,
    pub failed: u32,
    pub passed: u32,
    /// Of the passed tests.
    pub duration_ms: u64,
    /// The failure message when exactly one test failed, the first ignore
    /// reason when the marker is ignored; at most three lines, else empty.
    pub message: String,
    /// That one failure's expected/actual, when its assertion compared two.
    pub comparison: Option<Comparison>,
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
    running: u32,
    failed: u32,
    terminated: u32,
    ignored: u32,
    passed: u32,
    duration_ms: u64,
    first_failure: Option<(String, Option<Comparison>)>,
    first_ignore: Option<String>,
}

impl Tally {
    fn of(leaves: &[&TestNode]) -> Self {
        let mut tally = Self::default();
        leaves.iter().for_each(|leaf| tally.add(leaf));
        tally
    }

    fn add(&mut self, leaf: &TestNode) {
        let message = || leaf.failure.as_ref().map(|f| first_lines(&f.message));
        match leaf.status {
            TestStatus::Running => self.running += 1,
            TestStatus::Failed => {
                self.failed += 1;
                let failure = || {
                    leaf.failure
                        .as_ref()
                        .map(|f| (first_lines(&f.message), f.comparison.clone()))
                };
                self.first_failure = self.first_failure.take().or_else(failure);
            }
            TestStatus::Terminated => self.terminated += 1,
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

    fn state(&self) -> MarkerState {
        if self.running > 0 {
            MarkerState::Running
        } else if self.failed > 0 {
            MarkerState::Failed
        } else if self.terminated > 0 {
            MarkerState::Terminated
        } else if self.ignored > 0 {
            MarkerState::Ignored
        } else if self.passed > 0 {
            MarkerState::Passed
        } else {
            MarkerState::NeverRun
        }
    }

    fn status(&self, line: u32) -> MarkerStatus {
        let state = self.state();
        let (message, comparison) = match state {
            MarkerState::Failed if self.failed == 1 => self.first_failure.clone().unzip(),
            MarkerState::Ignored => (self.first_ignore.clone(), None),
            _ => (None, None),
        };
        MarkerStatus {
            line,
            state,
            total: self.running + self.failed + self.terminated + self.ignored + self.passed,
            failed: self.failed,
            passed: self.passed,
            duration_ms: self.duration_ms,
            message: message.unwrap_or_default(),
            comparison: comparison.flatten(),
        }
    }
}

pub(crate) fn first_lines(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(TOOLTIP_MESSAGE_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The distinct leaf tests, in tree order, of the nodes that name `marker`.
fn leaves_of(marker: &TestMarker, tree: &TestTree, file_nodes: &[&TestNode]) -> Vec<TestId> {
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
        .collect()
}

/// Each marker's distinct leaf tests: its own, and for a class or
/// `describe()` those of every test marker inside it.
fn marker_leaves(
    markers: &[TestMarker],
    tree: &TestTree,
    file_nodes: &[&TestNode],
) -> Vec<Vec<TestId>> {
    if file_nodes.is_empty() {
        return vec![Vec::new(); markers.len()];
    }
    let own: Vec<Vec<TestId>> = markers
        .iter()
        .map(|marker| leaves_of(marker, tree, file_nodes))
        .collect();
    markers
        .iter()
        .enumerate()
        .map(|(index, marker)| {
            let mut ids = own[index].clone();
            if marker.scope != MarkerScope::Test {
                for (inner, inner_ids) in markers.iter().zip(&own) {
                    let inside = inner.scope == MarkerScope::Test
                        && inner.span.0 >= marker.span.0
                        && inner.span.1 <= marker.span.1;
                    if inside {
                        ids.extend(inner_ids.iter().cloned());
                    }
                }
                let mut seen = HashSet::new();
                ids.retain(|id| seen.insert(id.clone()));
            }
            ids
        })
        .collect()
}

/// One source of results: a tree and its nodes located in the marked file
/// ([`TestTree::nodes_in_file`]).
#[derive(Clone, Copy)]
pub struct Results<'t> {
    pub tree: &'t TestTree,
    pub file_nodes: &'t [&'t TestNode],
}

/// The status of every marker. `file_nodes` is the tree's nodes located in
/// this file ([`TestTree::nodes_in_file`]).
pub fn marker_statuses(
    markers: &[TestMarker],
    tree: &TestTree,
    file_nodes: &[&TestNode],
) -> Vec<MarkerStatus> {
    let results = Results { tree, file_nodes };
    marker_statuses_with_history(
        markers,
        results,
        Results {
            file_nodes: &[],
            ..results
        },
    )
}

/// [`marker_statuses`] over the live run's results and the recorded ones
/// (`crate::test_history`), merged per test: a live result supersedes the
/// recorded result of the same test, and every other recorded test still
/// counts, so a class one of whose tests ran this session still shows the
/// recorded failure of another.
pub fn marker_statuses_with_history(
    markers: &[TestMarker],
    live: Results<'_>,
    recorded: Results<'_>,
) -> Vec<MarkerStatus> {
    let live_ids = marker_leaves(markers, live.tree, live.file_nodes);
    let recorded_ids = marker_leaves(markers, recorded.tree, recorded.file_nodes);
    markers
        .iter()
        .zip(live_ids.iter().zip(&recorded_ids))
        .map(|(marker, (live_ids, recorded_ids))| {
            let ran: HashSet<&TestId> = live_ids.iter().collect();
            let leaves: Vec<&TestNode> = live_ids
                .iter()
                .filter_map(|id| live.tree.node(id))
                .chain(
                    recorded_ids
                        .iter()
                        .filter(|id| !ran.contains(id))
                        .filter_map(|id| recorded.tree.node(id)),
                )
                .collect();
            Tally::of(&leaves).status(marker.line)
        })
        .collect()
}

/// The data sets the last run executed for the PHPUnit method marked on
/// `line`;
/// empty for anything else.
pub fn data_sets(
    markers: &[TestMarker],
    line: u32,
    tree: &TestTree,
    file_nodes: &[&TestNode],
) -> DataSetChoices {
    let Some(marker) = markers
        .iter()
        .find(|m| m.line == line && m.scope == MarkerScope::Test)
    else {
        return DataSetChoices::default();
    };
    let mut sets: Vec<DataSet> = leaves_of(marker, tree, file_nodes)
        .into_iter()
        .filter_map(|id| tree.node(&id))
        .filter(|leaf| test_core::scope::data_set_of(&leaf.name).is_some())
        // Only a PHPUnit method's set runs alone: Pest's `--filter` matches
        // a call's description without its data set.
        .filter(|leaf| {
            leaf.location
                .as_ref()
                .is_some_and(|l| matches!(l.symbol, Some(TestSymbol::Method { .. })))
        })
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
    use crate::tests::markers;

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
                comparison: None,
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

    fn statuses_of(text: &str, tree: &TestTree) -> Vec<MarkerStatus> {
        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        marker_statuses(&markers(text), tree, &nodes)
    }

    fn statuses(tree: &TestTree) -> Vec<MarkerStatus> {
        statuses_of(SOURCE, tree)
    }

    fn sets_of(text: &str, line: u32, tree: &TestTree) -> DataSetChoices {
        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        data_sets(&markers(text), line, tree, &nodes)
    }

    fn state_at(statuses: &[MarkerStatus], line: u32) -> &MarkerStatus {
        statuses.iter().find(|s| s.line == line).unwrap()
    }

    const CLASS: &str = r"App\GreeterTest";

    fn merged(text: &str, live: &TestTree, recorded: &TestTree) -> Vec<MarkerStatus> {
        let host = ExecHost::for_path(Path::new("/p"));
        let live_nodes = live.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        let recorded_nodes = recorded.nodes_in_file(&host, Path::new("/p"), Path::new(FILE));
        marker_statuses_with_history(
            &markers(text),
            Results {
                tree: live,
                file_nodes: &live_nodes,
            },
            Results {
                tree: recorded,
                file_nodes: &recorded_nodes,
            },
        )
    }

    #[test]
    fn a_live_result_supersedes_the_recorded_one_per_test() {
        let mut live = tree_with_class();
        let mut recorded = tree_with_class();
        run(&mut live, CLASS, "testA", "pass", 1);
        run(&mut recorded, CLASS, "testA", "fail:old", 1);
        run(&mut recorded, CLASS, "testB", "fail:kept", 1);
        let merged = merged(SOURCE, &live, &recorded);
        assert_eq!(state_at(&merged, 4).state, MarkerState::Passed);
        assert_eq!(state_at(&merged, 5).state, MarkerState::Failed);
        assert_eq!(state_at(&merged, 5).message, "kept");
        assert_eq!(state_at(&merged, 6).state, MarkerState::NeverRun);
        // The class counts testA's live pass and testB's recorded failure.
        let class = state_at(&merged, 2);
        assert_eq!(
            (class.state, class.passed, class.failed),
            (MarkerState::Failed, 1, 1)
        );
    }

    #[test]
    fn a_data_set_run_this_session_keeps_the_other_recorded_sets() {
        let mut live = tree_with_class();
        let mut recorded = tree_with_class();
        run(
            &mut live,
            CLASS,
            r#"testSets with data set "one""#,
            "pass",
            1,
        );
        run(
            &mut recorded,
            CLASS,
            r#"testSets with data set "one""#,
            "fail:old",
            1,
        );
        run(
            &mut recorded,
            CLASS,
            r#"testSets with data set "two""#,
            "fail:two",
            1,
        );
        let sets = state_at(&merged(SOURCE, &live, &recorded), 6).clone();
        assert_eq!(
            (sets.state, sets.total, sets.passed, sets.failed),
            (MarkerState::Failed, 2, 1, 1)
        );
        assert_eq!(sets.message, "two");
    }

    #[test]
    fn a_test_the_run_never_finished_is_terminated_not_failed() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "pass", 1);
        run(&mut tree, CLASS, "testB", "run", 1);
        tree.finish_run();
        let statuses = statuses(&tree);
        assert_eq!(state_at(&statuses, 5).state, MarkerState::Terminated);
        assert_eq!(state_at(&statuses, 2).state, MarkerState::Terminated);
    }

    #[test]
    fn a_single_comparison_failure_carries_expected_and_actual() {
        let mut tree = tree_with_class();
        let parent = Some(id(CLASS));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "testA".into(),
            location: hint(r"\App\GreeterTest::testA"),
        });
        let comparison = test_core::Comparison {
            expected: "'expected text'".into(),
            actual: "'actual text'".into(),
        };
        tree.apply(TeamCityEvent::TestFailed {
            parent,
            name: "testA".into(),
            message: "Failed asserting that two strings are identical.".into(),
            details: String::new(),
            comparison: Some(comparison.clone()),
        });
        let statuses = statuses(&tree);
        assert_eq!(state_at(&statuses, 4).comparison, Some(comparison));
        assert_eq!(state_at(&statuses, 5).comparison, None);
    }

    #[test]
    fn markers_without_a_run_are_never_run() {
        let statuses = statuses(&TestTree::new());
        assert_eq!(statuses.len(), 4);
        assert!(statuses
            .iter()
            .all(|s| s.state == MarkerState::NeverRun && s.message.is_empty()));
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
            (a.state, a.passed, a.total, a.duration_ms),
            (MarkerState::Passed, 1, 1, 12)
        );
        let b = state_at(&statuses, 5);
        assert_eq!(b.state, MarkerState::Failed);
        assert_eq!(b.message, "Failed asserting that 1 is 2.\nat line 9");
        assert_eq!(state_at(&statuses, 6).state, MarkerState::NeverRun);
        let class = state_at(&statuses, 2);
        assert_eq!(class.state, MarkerState::Failed);
        assert_eq!(class.message, b.message);
    }

    #[test]
    fn several_failures_are_counted_in_the_tooltip() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "fail:x", 1);
        run(&mut tree, CLASS, "testB", "fail:y", 1);
        run(&mut tree, CLASS, "testSets", "pass", 1);
        let class = state_at(&statuses(&tree), 2).clone();
        assert_eq!((class.failed, class.total), (2, 3));
        assert_eq!(class.message, "");
    }

    #[test]
    fn ignored_ranks_between_failed_and_passed() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testA", "pass", 1);
        run(&mut tree, CLASS, "testB", "skip", 0);
        let statuses = statuses(&tree);
        let class = state_at(&statuses, 2);
        assert_eq!(class.state, MarkerState::Ignored);
        assert_eq!(class.message, "not yet");
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
            (class.state, class.passed, class.duration_ms),
            (MarkerState::Passed, 2, 5)
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
            (sets.state, sets.message.as_str()),
            (MarkerState::Failed, "no")
        );

        let choices = sets_of(SOURCE, 6, &tree);
        let labels: Vec<_> = choices.shown.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                r#"testSets with data set "one""#,
                r#"testSets with data set "two""#
            ]
        );
        assert_eq!(choices.hidden, 0);
        assert!(sets_of(SOURCE, 4, &tree).shown.is_empty());
        assert!(sets_of(SOURCE, 2, &tree).shown.is_empty());
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
        let choices = sets_of(SOURCE, 6, &tree);
        assert_eq!(
            (choices.shown.len(), choices.hidden),
            (MAX_MENU_DATA_SETS, 5)
        );
    }

    #[test]
    fn a_result_follows_its_method_when_edits_move_it() {
        let mut tree = tree_with_class();
        run(&mut tree, CLASS, "testB", "fail:boom", 1);
        let moved = SOURCE.replacen("<?php\n", "<?php\n// one\n// two\n", 1);
        let statuses = statuses_of(&moved, &tree);
        assert_eq!(state_at(&statuses, 7).state, MarkerState::Failed);
        assert_eq!(state_at(&statuses, 6).state, MarkerState::NeverRun);
    }

    // Real output of `phpunit --teamcity` (12.0.9) and `pest --teamcity`
    // (5.3), captured in the scratch projects the review findings named.
    const PHPUNIT_CLASS_LEVEL_ERROR: &str =
        include_str!("fixtures/phpunit12_class_level_error.txt");
    const PHPUNIT_FATAL_EXIT: &str = include_str!("fixtures/phpunit12_fatal_exit.txt");
    const PEST_FILE_BASED: &str = include_str!("fixtures/pest_file_based.txt");
    const PEST_CLASS_BASED: &str = include_str!("fixtures/pest_class_based.txt");

    fn replayed(output: &str) -> TestTree {
        let mut tree = TestTree::new();
        let mut parser = test_core::TeamCityParser::new();
        for event in parser.feed(output) {
            tree.apply(event);
        }
        tree
    }

    fn statuses_for(file: &str, text: &str, tree: &TestTree) -> Vec<MarkerStatus> {
        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(file));
        marker_statuses(&markers(text), tree, &nodes)
    }

    const BEFORE_CLASS: &str = "<?php\nnamespace App;\nuse PHPUnit\\Framework\\TestCase;\nfinal class BeforeClassTest extends TestCase\n{\n    public static function setUpBeforeClass(): void {}\n    public function testA(): void {}\n}\n";

    #[test]
    fn a_class_level_error_marks_the_class_failed() {
        // `setUpBeforeClass` threw: PHPUnit sends one `testFailed` named
        // after the class and no test of it ever starts.
        let tree = replayed(PHPUNIT_CLASS_LEVEL_ERROR);
        let got = statuses_for("/p/tests/BeforeClassTest.php", BEFORE_CLASS, &tree);
        let class = state_at(&got, 3);
        assert_eq!(
            (class.state, class.failed, class.message.as_str()),
            (
                MarkerState::Failed,
                1,
                "RuntimeException: boom in setUpBeforeClass"
            )
        );
        assert_eq!(state_at(&got, 6).state, MarkerState::NeverRun);
    }

    #[test]
    fn a_run_that_dies_mid_test_leaves_no_marker_running_once_finished() {
        let mut tree = replayed(PHPUNIT_FATAL_EXIT);
        let source = "<?php\nnamespace App;\nuse PHPUnit\\Framework\\TestCase;\nfinal class FatalTest extends TestCase\n{\n    public function testDies(): void {}\n}\n";
        let before = statuses_for("/p/tests/FatalTest.php", source, &tree);
        assert_eq!(state_at(&before, 5).state, MarkerState::Running);
        assert_eq!(state_at(&before, 3).state, MarkerState::Running);
        tree.finish_run();
        let after = statuses_for("/p/tests/FatalTest.php", source, &tree);
        assert_eq!(state_at(&after, 5).state, MarkerState::Terminated);
        assert_eq!(state_at(&after, 3).state, MarkerState::Terminated);
    }

    const PEST_SOURCE: &str = "<?php
test('adds', function () {})->with([1, 2]);
test('named sets', function () {})->with(['one' => 1, 'two' => 2]);
it('says hello with (parens) in it', function () {});
describe('Greeter', function () {
    it('greets', function () {});
    describe('loud', function () {
        it('shouts', function () {});
    });
});
test('has with data set in name', function () {});
";
    const PEST_FILE: &str = "/p/tests/GreetTest.php";

    #[test]
    fn pest_calls_describes_and_data_sets_get_their_results() {
        let tree = replayed(PEST_FILE_BASED);
        let got = statuses_for(PEST_FILE, PEST_SOURCE, &tree);
        let adds = state_at(&got, 1);
        assert_eq!(
            (adds.state, adds.passed, adds.total),
            (MarkerState::Passed, 2, 2)
        );
        assert_eq!(state_at(&got, 2).total, 2);
        assert_eq!(state_at(&got, 3).state, MarkerState::Passed);
        // The describes: `loud` holds the failing `it`, `Greeter` both.
        let shouts = state_at(&got, 7);
        assert_eq!(
            (shouts.state, shouts.message.as_str()),
            (
                MarkerState::Failed,
                "Failed asserting that 1 is identical to 2."
            )
        );
        assert_eq!(state_at(&got, 6).state, MarkerState::Failed);
        let greeter = state_at(&got, 4);
        assert_eq!(
            (greeter.state, greeter.failed, greeter.total),
            (MarkerState::Failed, 1, 2)
        );
        // A name that contains the words of a data set is a test, not a row.
        let odd = state_at(&got, 10);
        assert_eq!((odd.state, odd.total), (MarkerState::Passed, 1));
    }

    #[test]
    fn pest_data_set_rows_are_not_offered_on_their_own() {
        // Pest's `--filter` cannot select one row (it matches the call's
        // description without the set), so the menu offers none.
        let tree = replayed(PEST_FILE_BASED);
        let host = ExecHost::for_path(Path::new("/p"));
        let nodes = tree.nodes_in_file(&host, Path::new("/p"), Path::new(PEST_FILE));
        let all = markers(PEST_SOURCE);
        for line in [1, 2, 3, 10] {
            assert!(data_sets(&all, line, &tree, &nodes).shown.is_empty());
        }
        // The rows still count on the call's marker.
        let statuses = marker_statuses(&all, &tree, &nodes);
        assert_eq!(state_at(&statuses, 1).total, 2);
    }

    #[test]
    fn a_nested_describes_it_does_not_light_up_the_same_named_it_of_another_describe() {
        let source = "<?php\ndescribe('a', function () {\n    it('x', function () {});\n});\ndescribe('b', function () {\n    it('x', function () {});\n});\n";
        let tree = replayed(
            "##teamcity[testStarted name='`a` → it x' locationHint='pest_qn://tests/GreetTest.php::`a` → it x']\n\
             ##teamcity[testFinished name='`a` → it x' duration='3']\n",
        );
        let got = statuses_for(PEST_FILE, source, &tree);
        assert_eq!(state_at(&got, 2).state, MarkerState::Passed);
        assert_eq!(state_at(&got, 5).state, MarkerState::NeverRun);
        assert_eq!(state_at(&got, 1).state, MarkerState::Passed);
        assert_eq!(state_at(&got, 4).state, MarkerState::NeverRun);
    }

    #[test]
    fn results_of_the_same_class_in_another_file_stay_out() {
        let tree = replayed(&format!(
            "##teamcity[testStarted name='testA' locationHint='php_qn:///p/other/GreeterTest.php::\\App\\GreeterTest::testA']\n\
             ##teamcity[testFailed name='testA' message='no' details='']\n\
             ##teamcity[testFinished name='testA' duration='1']\n\
             ##teamcity[testStarted name='testB' locationHint='php_qn://{FILE}::\\App\\GreeterTest::testB']\n\
             ##teamcity[testFinished name='testB' duration='1']\n"
        ));
        let got = statuses(&tree);
        assert_eq!(state_at(&got, 4).state, MarkerState::NeverRun);
        assert_eq!(state_at(&got, 5).state, MarkerState::Passed);
    }

    #[test]
    fn a_class_based_pest_run_reaches_the_phpunit_file_through_psr4() {
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join("tests/ClassTest.php");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let source = "<?php\nnamespace Tests;\nuse PHPUnit\\Framework\\TestCase;\nfinal class ClassTest extends TestCase\n{\n    public function testXyzAbc(): void {}\n    public function test_snake_case(): void {}\n    #[Test]\n    public function attributeOne(): void {}\n    public function testFails(): void {}\n}\n";
        std::fs::write(&file, source).unwrap();
        let roots = vec![crate::composer::Psr4Root {
            prefix: "Tests\\".into(),
            dir: "tests/".into(),
            dev: true,
        }];
        let dir = project.path().to_path_buf();
        let locator =
            crate::tests::EventLocator::new(vec![(dir.clone(), roots)], ExecHost::Local, dir);
        let mut tree = TestTree::new();
        let mut parser = test_core::TeamCityParser::new();
        for event in parser.feed(PEST_CLASS_BASED) {
            tree.apply(locator.locate_event(event));
        }
        let host = ExecHost::Local;
        let nodes = tree.nodes_in_file(&host, project.path(), &file);
        let got = marker_statuses(&markers(source), &tree, &nodes);
        let state = |line| state_at(&got, line).state;
        assert_eq!(state(3), MarkerState::Failed, "class");
        assert_eq!(state(5), MarkerState::Passed, "testXyzAbc");
        assert_eq!(state(6), MarkerState::Passed, "test_snake_case");
        assert_eq!(state(8), MarkerState::Passed, "#[Test] attributeOne");
        assert_eq!(state(9), MarkerState::Failed, "testFails");
    }

    #[test]
    fn an_invalid_data_provider_marks_its_test_failed_and_leaves_the_others() {
        use crate::invalid_provider::InvalidProviderScanner;
        const OUTPUT: &str = include_str!("fixtures/phpunit12_invalid_data_provider.txt");
        const SOURCE: &str = "<?php\nnamespace App\\Tests;\nuse PHPUnit\\Framework\\TestCase;\nfinal class ProviderTest extends TestCase\n{\n    public function testGood(): void {}\n    public function testBad(): void {}\n}\n";
        let locator = crate::tests::EventLocator::new(
            Vec::new(),
            ExecHost::Local,
            std::path::PathBuf::from("/w"),
        );
        let (mut tree, mut scanner) = (TestTree::new(), InvalidProviderScanner::default());
        let mut parser = test_core::TeamCityParser::new();
        for event in parser.feed(OUTPUT) {
            scanner.observe(&event);
            tree.apply(event);
        }
        for event in scanner.feed(OUTPUT, &locator) {
            tree.apply(event);
        }
        let host = ExecHost::for_path(Path::new("/w"));
        let nodes = tree.nodes_in_file(
            &host,
            Path::new("/w"),
            Path::new("/w/tests/ProviderTest.php"),
        );
        let got = marker_statuses(&markers(SOURCE), &tree, &nodes);
        assert_eq!(state_at(&got, 5).state, MarkerState::Passed, "testGood");
        let bad = state_at(&got, 6);
        assert_eq!((bad.state, bad.failed), (MarkerState::Failed, 1), "testBad");
        assert!(bad.message.contains("does not exist"), "{}", bad.message);
        assert_eq!(state_at(&got, 3).state, MarkerState::Failed, "class");
    }
}
