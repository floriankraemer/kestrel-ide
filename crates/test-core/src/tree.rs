//! The suite/class/method hierarchy a test run fills in as it goes (D2).
//!
//! [`TestTree`] is a plain data structure with one mutation entry point per
//! source format — [`TestTree::apply`] for one streamed
//! [`crate::teamcity::TeamCityEvent`], [`TestTree::apply_junit`] for a whole
//! batch of [`crate::junit::JUnitTestCase`] — so both formats fill the same
//! shape and every other consumer (the Tests dock, rerun selectors, D3's
//! diagnostics conversion) reads one tree regardless of which format
//! produced it.
//!
//! Node order is insertion order, not alphabetical: that is the order
//! PHPUnit actually ran the suites in, and a tree that reorders itself
//! while a run is in flight is worse than one that doesn't.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use process_exec::host::ExecHost;

use crate::location::TestLocation;
use crate::teamcity::TeamCityEvent;

/// A node's identity, qualified by its ancestor suite names joined with
/// `::` (e.g. `Full\Namespace\ClassTest::testMethod`) so two classes with a
/// same-named method never collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TestId(pub String);

impl TestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn child(parent: Option<&TestId>, name: &str) -> Self {
        match parent {
            Some(parent) => TestId(format!("{}::{name}", parent.0)),
            None => TestId(name.to_string()),
        }
    }
}

/// Where a node stands. Ordered worst-first so a suite's aggregate status
/// (the worst of its children) is a `min` over this order — see
/// [`TestTree::recompute_ancestors`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TestStatus {
    Failed,
    /// The run ended (Stop, a fatal error, `exit()`) before the test
    /// reported a result: IntelliJ's "terminated", not a failure.
    Terminated,
    Running,
    Pending,
    Passed,
    Skipped,
}

/// The failure a test gets when its run ends without a result for it.
pub const UNFINISHED_MESSAGE: &str = "Test did not finish";

/// Whether a node is a leaf test or a suite/class grouping others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Suite,
    Test,
}

/// A failing (or errored) test's message and the raw detail text — a
/// stack trace or an assertion diff, whatever the framework sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFailure {
    pub message: String,
    pub details: String,
    /// What an equality assertion expected and got, when the framework
    /// said (TeamCity's `expected`/`actual`): the diff viewer's input.
    pub comparison: Option<Comparison>,
}

/// An equality assertion's two sides, as the framework printed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    pub expected: String,
    pub actual: String,
}

/// One node of the tree: a suite/class or a single test method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestNode {
    pub id: TestId,
    pub name: String,
    pub kind: NodeKind,
    pub status: TestStatus,
    pub duration_ms: Option<u64>,
    pub failure: Option<TestFailure>,
    /// Where the node lives in source, when the framework said so.
    pub location: Option<TestLocation>,
    pub parent: Option<TestId>,
    pub children: Vec<TestId>,
    /// The run that last reported this node ([`TestTree::begin_run`]): a
    /// rerun from the tree runs the way that run did.
    pub run: u64,
}

/// Live counts across every node currently known, for the dock's toolbar
/// summary ("12 passed, 1 failed, 3 running").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestCounts {
    pub passed: usize,
    pub failed: usize,
    pub terminated: usize,
    pub skipped: usize,
    pub running: usize,
    pub pending: usize,
}

/// Where a run executed: the host and working directory its printed paths
/// are relative to (#433). The tree keeps one per run it still holds, so a
/// node from an earlier run maps back through that run's host and directory,
/// not the latest run's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlace {
    pub host: ExecHost,
    pub work_dir: PathBuf,
    /// The framework that ran it: the `source` its failures are listed under.
    pub framework: String,
}

/// A test run's live tree. Starts empty; [`Self::apply`]/[`Self::apply_junit`]
/// are the only ways anything gets into it.
#[derive(Debug, Default)]
pub struct TestTree {
    nodes: HashMap<TestId, TestNode>,
    roots: Vec<TestId>,
    /// The run whose events are being applied.
    current_run: u64,
    /// Where each run still in the tree executed.
    places: HashMap<u64, RunPlace>,
}

impl TestTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// A tree of `leaves` alone, each a root, as a test history restores
    /// them: results kept from earlier sessions, located by local paths.
    pub fn restored(leaves: impl IntoIterator<Item = TestNode>) -> Self {
        let mut tree = Self::default();
        for leaf in leaves {
            if tree.nodes.contains_key(&leaf.id) {
                continue;
            }
            tree.roots.push(leaf.id.clone());
            tree.nodes.insert(leaf.id.clone(), leaf);
        }
        tree
    }

    /// Drop everything — what starting a new run means for the tree a
    /// previous run left behind. A fresh run's tree is never the old one
    /// with rows overwritten: a class that fails to bootstrap this time
    /// should not keep last run's green leaves under it.
    pub fn reset(&mut self) {
        self.nodes.clear();
        self.roots.clear();
    }

    /// The events applied from now on come from `run`, executed at `place`.
    /// Places of runs no node is from any more are dropped.
    pub fn begin_run(&mut self, run: u64, place: RunPlace) {
        self.current_run = run;
        let held: std::collections::HashSet<u64> =
            self.nodes.values().map(|node| node.run).collect();
        self.places.retain(|run, _| held.contains(run));
        self.places.insert(run, place);
    }

    /// Where the run that last reported `node` executed; `None` for a node
    /// applied outside any [`Self::begin_run`].
    pub fn place_of(&self, node: &TestNode) -> Option<&RunPlace> {
        self.places.get(&node.run)
    }

    /// The framework of the run that last reported `node`, else `framework`.
    pub fn framework_of<'a>(&'a self, node: &TestNode, framework: &'a str) -> &'a str {
        self.place_of(node)
            .map(|place| place.framework.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(framework)
    }

    /// Every framework a run still in the tree ran with.
    pub fn frameworks(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.places.values().map(|p| p.framework.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// `node`'s run's host and working directory, else the given ones.
    fn place_or<'a>(
        &'a self,
        node: &TestNode,
        host: &'a ExecHost,
        work_dir: &'a Path,
    ) -> (&'a ExecHost, &'a Path) {
        self.place_of(node).map_or((host, work_dir), |place| {
            (&place.host, place.work_dir.as_path())
        })
    }

    /// The runs that last reported the leaf tests at or below `ids`, each
    /// once, in tree order.
    pub fn runs_of(&self, ids: &[TestId]) -> Vec<u64> {
        let mut runs = Vec::new();
        for leaf in ids.iter().flat_map(|id| self.leaves_under(id)) {
            if let Some(node) = self.nodes.get(&leaf) {
                if !runs.contains(&node.run) {
                    runs.push(node.run);
                }
            }
        }
        runs
    }

    /// Whether any node was last reported by `run`.
    pub fn holds_run(&self, run: u64) -> bool {
        self.nodes.values().any(|node| node.run == run)
    }

    pub fn node(&self, id: &TestId) -> Option<&TestNode> {
        self.nodes.get(id)
    }

    /// Every node whose source is `file` on this machine, in tree order —
    /// what the editor gutter reads to paint a test file's last result.
    /// Each node's paths map through its own run's place; `host` and
    /// `work_dir` stand in for a node from no known run (see
    /// [`TestLocation::local_path`]).
    pub fn nodes_in_file(&self, host: &ExecHost, work_dir: &Path, file: &Path) -> Vec<&TestNode> {
        let mut found = Vec::new();
        let mut pending: Vec<&TestId> = self.roots.iter().rev().collect();
        while let Some(id) = pending.pop() {
            let Some(node) = self.nodes.get(id) else {
                continue;
            };
            pending.extend(node.children.iter().rev());
            if self.local_path(node, host, work_dir).as_deref() == Some(file) {
                found.push(node);
            }
        }
        found
    }

    /// Every distinct file any node is located in, on this machine: one pass
    /// that lets a caller skip the files a run never touched before paying
    /// for [`Self::nodes_in_file`] on each.
    pub fn located_files(&self, host: &ExecHost, work_dir: &Path) -> Vec<PathBuf> {
        let files: std::collections::BTreeSet<PathBuf> = self
            .nodes
            .values()
            .filter_map(|node| self.local_path(node, host, work_dir))
            .collect();
        files.into_iter().collect()
    }

    /// `node`'s source file on this machine, through its own run's place
    /// (`host` and `work_dir` for a node from no known run); `None` without
    /// a located file.
    pub fn local_path(&self, node: &TestNode, host: &ExecHost, work_dir: &Path) -> Option<PathBuf> {
        let location = node.location.as_ref().filter(|l| !l.file.is_empty())?;
        let (host, work_dir) = self.place_or(node, host, work_dir);
        Some(location.local_path(host, work_dir))
    }

    /// The working directory of the run that last reported `node`, else
    /// `work_dir`: what a relative path in its failure is relative to.
    pub fn work_dir_of<'a>(&'a self, node: &TestNode, work_dir: &'a Path) -> &'a Path {
        self.place_of(node)
            .map_or(work_dir, |place| place.work_dir.as_path())
    }

    /// The host of the run that last reported `node`, else `host`.
    pub fn host_of<'a>(&'a self, node: &TestNode, host: &'a ExecHost) -> &'a ExecHost {
        self.place_of(node).map_or(host, |place| &place.host)
    }

    pub fn roots(&self) -> &[TestId] {
        &self.roots
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every leaf test currently `Failed` or `Terminated`, in tree order —
    /// what "Rerun Failed" (D6) selects, as IntelliJ's reruns a terminated
    /// test too.
    pub fn failed_leaf_ids(&self) -> Vec<TestId> {
        self.roots
            .iter()
            .flat_map(|id| self.leaves_under(id))
            .filter(|id| {
                self.nodes.get(id).is_some_and(|n| {
                    matches!(n.status, TestStatus::Failed | TestStatus::Terminated)
                })
            })
            .collect()
    }

    /// The leaf tests at or below `id`, in tree order.
    pub fn leaves_under(&self, id: &TestId) -> Vec<TestId> {
        let Some(node) = self.nodes.get(id) else {
            return Vec::new();
        };
        if node.kind == NodeKind::Test {
            return vec![id.clone()];
        }
        node.children
            .iter()
            .flat_map(|child| self.leaves_under(child))
            .collect()
    }

    /// Every node currently carrying a failure — what D3's diagnostics
    /// conversion iterates, rather than every node, since a passing test
    /// has nothing to report.
    pub fn failing_nodes(&self) -> Vec<&TestNode> {
        self.nodes
            .values()
            .filter(|n| n.failure.is_some())
            .collect()
    }

    pub fn counts(&self) -> TestCounts {
        let mut counts = TestCounts::default();
        for node in self.nodes.values() {
            if node.kind != NodeKind::Test {
                continue;
            }
            match node.status {
                TestStatus::Passed => counts.passed += 1,
                TestStatus::Failed => counts.failed += 1,
                TestStatus::Terminated => counts.terminated += 1,
                TestStatus::Skipped => counts.skipped += 1,
                TestStatus::Running => counts.running += 1,
                TestStatus::Pending => counts.pending += 1,
            }
        }
        counts
    }

    fn insert(&mut self, id: TestId, name: String, kind: NodeKind, parent: Option<TestId>) {
        if self.nodes.contains_key(&id) {
            return;
        }
        match &parent {
            Some(parent_id) => {
                if let Some(parent_node) = self.nodes.get_mut(parent_id) {
                    parent_node.children.push(id.clone());
                }
            }
            None => self.roots.push(id.clone()),
        }
        self.nodes.insert(
            id.clone(),
            TestNode {
                id,
                name,
                kind,
                status: TestStatus::Pending,
                duration_ms: None,
                failure: None,
                location: None,
                parent,
                children: Vec::new(),
                run: self.current_run,
            },
        );
    }

    fn set_location(&mut self, id: &TestId, location: Option<TestLocation>) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.location = location.or(node.location.take());
        }
    }

    fn set_status(&mut self, id: &TestId, status: TestStatus) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = status;
            node.run = self.current_run;
        }
        self.recompute_ancestors(id);
    }

    /// After a leaf's status changes, walk up and recompute every ancestor
    /// suite's status as the worst of its children (per [`TestStatus`]'s
    /// ordering) — a suite is `Failed` if anything under it failed, else
    /// `Running` if anything is still running, and so on down to
    /// `Skipped` only when every child is.
    fn recompute_ancestors(&mut self, id: &TestId) {
        let Some(mut parent) = self.nodes.get(id).and_then(|n| n.parent.clone()) else {
            return;
        };
        while let Some(node) = self.nodes.get(&parent) {
            let worst = node
                .children
                .iter()
                .filter_map(|child| self.nodes.get(child))
                .map(|child| child.status)
                .min();
            let Some(worst) = worst else { break };
            if let Some(node) = self.nodes.get_mut(&parent) {
                node.status = worst;
            }
            match self.nodes.get(&parent).and_then(|n| n.parent.clone()) {
                Some(next) => parent = next,
                None => break,
            }
        }
    }

    /// The run has ended: whatever is still `Running` never reported a
    /// result (Stop, a fatal error, a killed container). A leaf becomes
    /// `Terminated`, so no aggregate keeps claiming progress; a suite that
    /// never got a child goes back to `Pending`.
    pub fn finish_run(&mut self) {
        let unfinished: Vec<TestId> = self
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Test && n.status == TestStatus::Running)
            .map(|n| n.id.clone())
            .collect();
        for id in unfinished {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.failure = Some(TestFailure {
                    message: UNFINISHED_MESSAGE.to_string(),
                    details: String::new(),
                    comparison: None,
                });
            }
            self.set_status(&id, TestStatus::Terminated);
        }
        for node in self.nodes.values_mut() {
            if node.kind == NodeKind::Suite && node.status == TestStatus::Running {
                node.status = TestStatus::Pending;
            }
        }
    }

    /// Apply one streamed TeamCity event, filling the tree incrementally
    /// as a run reports.
    pub fn apply(&mut self, event: TeamCityEvent) {
        match event {
            TeamCityEvent::SuiteStarted {
                parent,
                name,
                location,
            } => {
                let id = TestId::child(parent.as_ref(), &name);
                self.insert(id.clone(), name, NodeKind::Suite, parent);
                self.set_location(&id, location);
                self.set_status(&id, TestStatus::Running);
            }
            TeamCityEvent::SuiteFinished { parent, name } => {
                // Nothing to do: the suite's status is already the worst
                // of its children via `recompute_ancestors`, and its
                // identity is derivable, so this event exists only for a
                // caller that wants to know the suite is done — no caller
                // does yet.
                let _ = TestId::child(parent.as_ref(), &name);
            }
            TeamCityEvent::TestStarted {
                parent,
                name,
                location,
            } => {
                let id = TestId::child(parent.as_ref(), &name);
                self.insert(id.clone(), name, NodeKind::Test, parent);
                self.set_location(&id, location);
                self.set_status(&id, TestStatus::Running);
            }
            TeamCityEvent::TestFinished {
                parent,
                name,
                duration_ms,
            } => {
                let id = TestId::child(parent.as_ref(), &name);
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.duration_ms = duration_ms;
                    // A `testFailed` message always precedes `testFinished`
                    // for the same test (TeamCity's own protocol order), and
                    // so does `testIgnored`; only promote to `Passed` if
                    // nothing already marked it `Failed` or `Skipped`.
                    if !matches!(node.status, TestStatus::Failed | TestStatus::Skipped) {
                        node.status = TestStatus::Passed;
                    }
                }
                self.recompute_ancestors(&id);
            }
            TeamCityEvent::TestFailed {
                parent,
                name,
                message,
                details,
                comparison,
            } => {
                let id = TestId::child(parent.as_ref(), &name);
                self.insert(id.clone(), name, NodeKind::Test, parent);
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.failure = Some(TestFailure {
                        message,
                        details,
                        comparison,
                    });
                }
                self.set_status(&id, TestStatus::Failed);
            }
            TeamCityEvent::TestIgnored {
                parent,
                name,
                message,
            } => {
                let id = TestId::child(parent.as_ref(), &name);
                self.insert(id.clone(), name.clone(), NodeKind::Test, parent);
                if let Some(node) = self.nodes.get_mut(&id) {
                    if !message.is_empty() {
                        node.failure = Some(TestFailure {
                            message,
                            details: String::new(),
                            comparison: None,
                        });
                    }
                }
                self.set_status(&id, TestStatus::Skipped);
            }
        }
    }

    /// Bulk-fill the tree from a finished JUnit-XML report — the fallback
    /// format (D2), which carries no per-test timeline, only the end
    /// state, so it fills the tree in one pass rather than incrementally.
    pub fn apply_junit(&mut self, cases: &[crate::junit::JUnitTestCase]) {
        for case in cases {
            let suite_id = TestId(case.suite.clone());
            self.insert(suite_id.clone(), case.suite.clone(), NodeKind::Suite, None);
            let test_id = TestId::child(Some(&suite_id), &case.name);
            self.insert(
                test_id.clone(),
                case.name.clone(),
                NodeKind::Test,
                Some(suite_id),
            );
            if let Some(node) = self.nodes.get_mut(&test_id) {
                node.duration_ms = case.duration_ms;
                node.failure = case.failure.clone();
            }
            self.set_status(&test_id, case.status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> TestId {
        TestId(s.to_string())
    }

    #[test]
    fn a_test_started_and_finished_reports_passed() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestStarted {
            parent: None,
            name: "testOne".into(),
            location: None,
        });
        assert_eq!(
            tree.node(&id("testOne")).unwrap().status,
            TestStatus::Running
        );
        tree.apply(TeamCityEvent::TestFinished {
            parent: None,
            name: "testOne".into(),
            duration_ms: Some(12),
        });
        let node = tree.node(&id("testOne")).unwrap();
        assert_eq!(node.status, TestStatus::Passed);
        assert_eq!(node.duration_ms, Some(12));
    }

    #[test]
    fn a_failed_message_before_finished_wins_over_finished() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestStarted {
            parent: None,
            name: "testTwo".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent: None,
            name: "testTwo".into(),
            message: "expected 1, got 2".into(),
            details: "at FooTest.php:10".into(),
            comparison: None,
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent: None,
            name: "testTwo".into(),
            duration_ms: Some(5),
        });
        let node = tree.node(&id("testTwo")).unwrap();
        assert_eq!(node.status, TestStatus::Failed);
        assert_eq!(node.failure.as_ref().unwrap().message, "expected 1, got 2");
    }

    #[test]
    fn a_suite_status_is_the_worst_of_its_tests() {
        let mut tree = TestTree::new();
        let suite = "Tests\\FooTest";
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: suite.into(),
            location: None,
        });
        let parent = Some(id(suite));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "testA".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent: parent.clone(),
            name: "testA".into(),
            duration_ms: Some(1),
        });
        assert_eq!(tree.node(&id(suite)).unwrap().status, TestStatus::Passed);

        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "testB".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent: parent.clone(),
            name: "testB".into(),
            message: "boom".into(),
            details: String::new(),
            comparison: None,
        });
        assert_eq!(tree.node(&id(suite)).unwrap().status, TestStatus::Failed);
    }

    #[test]
    fn nested_suites_qualify_the_test_id() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "Outer".into(),
            location: None,
        });
        let outer = Some(id("Outer"));
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: outer.clone(),
            name: "Inner".into(),
            location: None,
        });
        let inner = Some(id("Outer::Inner"));
        tree.apply(TeamCityEvent::TestStarted {
            parent: inner,
            name: "testDeep".into(),
            location: None,
        });
        assert!(tree.node(&id("Outer::Inner::testDeep")).is_some());
        assert_eq!(tree.roots(), &[id("Outer")]);
    }

    #[test]
    fn failed_leaf_ids_lists_only_failed_tests_not_suites() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "S".into(),
            location: None,
        });
        let parent = Some(id("S"));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "ok".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent: parent.clone(),
            name: "ok".into(),
            duration_ms: None,
        });
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "bad".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent,
            name: "bad".into(),
            message: "no".into(),
            details: String::new(),
            comparison: None,
        });
        assert_eq!(tree.failed_leaf_ids(), vec![id("S::bad")]);
    }

    #[test]
    fn ignored_tests_are_skipped() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestIgnored {
            parent: None,
            name: "testSkip".into(),
            message: "not ready".into(),
        });
        assert_eq!(
            tree.node(&id("testSkip")).unwrap().status,
            TestStatus::Skipped
        );
    }

    #[test]
    fn a_finished_event_after_an_ignored_one_keeps_the_test_skipped() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestIgnored {
            parent: None,
            name: "testSkip".into(),
            message: "not ready".into(),
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent: None,
            name: "testSkip".into(),
            duration_ms: Some(1),
        });
        assert_eq!(
            tree.node(&id("testSkip")).unwrap().status,
            TestStatus::Skipped
        );
    }

    #[test]
    fn counts_only_leaf_tests_not_suites() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "S".into(),
            location: None,
        });
        let parent = Some(id("S"));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "a".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent,
            name: "a".into(),
            duration_ms: None,
        });
        let counts = tree.counts();
        assert_eq!(counts.passed, 1);
        assert_eq!(counts.failed, 0);
    }

    #[test]
    fn reset_clears_everything() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestStarted {
            parent: None,
            name: "a".into(),
            location: None,
        });
        tree.reset();
        assert!(tree.is_empty());
        assert!(tree.roots().is_empty());
    }
    #[test]
    fn finishing_a_run_terminates_the_tests_that_never_reported() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "S".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "Empty".into(),
            location: None,
        });
        let parent = Some(id("S"));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "ok".into(),
            location: None,
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent: parent.clone(),
            name: "ok".into(),
            duration_ms: Some(1),
        });
        tree.apply(TeamCityEvent::TestStarted {
            parent,
            name: "dies".into(),
            location: None,
        });
        tree.finish_run();
        let dies = tree.node(&id("S::dies")).unwrap();
        assert_eq!(dies.status, TestStatus::Terminated);
        assert_eq!(dies.failure.as_ref().unwrap().message, UNFINISHED_MESSAGE);
        assert_eq!(tree.node(&id("S::ok")).unwrap().status, TestStatus::Passed);
        assert_eq!(tree.node(&id("S")).unwrap().status, TestStatus::Terminated);
        assert_eq!(tree.node(&id("Empty")).unwrap().status, TestStatus::Pending);
        assert_eq!((tree.counts().running, tree.counts().terminated), (0, 1));
        // Run Failed reruns a terminated test as IntelliJ's does.
        assert_eq!(tree.failed_leaf_ids(), [id("S::dies")]);
    }

    fn local_place(dir: &str) -> RunPlace {
        RunPlace {
            host: ExecHost::Local,
            work_dir: PathBuf::from(dir),
            framework: "PHPUnit".into(),
        }
    }

    #[test]
    fn each_node_maps_its_paths_through_the_place_of_its_own_run() {
        let pest = |file: &str, name: &str| TeamCityEvent::TestStarted {
            parent: None,
            name: name.into(),
            location: crate::location::parse_location_hint(&format!("pest_qn://{file}")),
        };
        let mut tree = TestTree::new();
        tree.begin_run(1, local_place("/p/packages/a"));
        tree.apply(pest("tests/ATest.php", "a"));
        tree.begin_run(2, local_place("/p/packages/b"));
        tree.apply(pest("tests/BTest.php", "b"));
        let latest = (ExecHost::Local, Path::new("/p/packages/b"));

        let a = tree.node(&id("a")).unwrap().clone();
        assert_eq!(
            tree.local_path(&a, &latest.0, latest.1),
            Some(PathBuf::from("/p/packages/a/tests/ATest.php"))
        );
        assert_eq!(tree.work_dir_of(&a, latest.1), Path::new("/p/packages/a"));
        assert_eq!(
            tree.nodes_in_file(
                &latest.0,
                latest.1,
                Path::new("/p/packages/a/tests/ATest.php")
            )
            .len(),
            1
        );
        assert_eq!(
            tree.located_files(&latest.0, latest.1),
            [
                PathBuf::from("/p/packages/a/tests/ATest.php"),
                PathBuf::from("/p/packages/b/tests/BTest.php")
            ]
        );

        // A whole run replaces the tree: the earlier run's place goes too.
        tree.reset();
        tree.begin_run(3, local_place("/p"));
        assert!(tree.place_of(&a).is_none());
    }

    #[test]
    fn nodes_in_file_lists_the_located_nodes_in_tree_order() {
        use crate::teamcity::TeamCityParser;
        let mut parser = TeamCityParser::new();
        let events = parser.feed(
            "##teamcity[testSuiteStarted name='T' locationHint='php_qn:///p/T.php::\\T']\n\
             ##teamcity[testStarted name='testA' locationHint='php_qn:///p/T.php::\\T::testA']\n\
             ##teamcity[testFinished name='testA' duration='1']\n\
             ##teamcity[testSuiteStarted name='U' locationHint='php_qn:///p/U.php::\\U']\n\
             ##teamcity[testStarted name='testB']\n",
        );
        let mut tree = TestTree::new();
        events.into_iter().for_each(|e| tree.apply(e));
        let found = tree.nodes_in_file(&ExecHost::Local, Path::new("/p"), Path::new("/p/T.php"));
        let ids: Vec<(&str, TestStatus)> =
            found.iter().map(|n| (n.id.as_str(), n.status)).collect();
        assert_eq!(
            ids,
            [("T", TestStatus::Running), ("T::testA", TestStatus::Passed)]
        );
        assert!(tree
            .nodes_in_file(&ExecHost::Local, Path::new("/p"), Path::new("/p/none.php"))
            .is_empty());
        assert_eq!(
            tree.located_files(&ExecHost::Local, Path::new("/p")),
            [PathBuf::from("/p/T.php"), PathBuf::from("/p/U.php")]
        );
    }

    #[test]
    fn each_leaf_remembers_the_run_that_last_reported_it() {
        let started = |name: &str| TeamCityEvent::TestStarted {
            parent: Some(id("Suite")),
            name: name.into(),
            location: None,
        };
        let mut tree = TestTree::new();
        tree.begin_run(1, local_place("/p"));
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "Suite".into(),
            location: None,
        });
        tree.apply(started("testA"));
        tree.apply(started("testB"));
        tree.begin_run(2, local_place("/p"));
        tree.apply(started("testB"));

        assert_eq!(tree.runs_of(&[id("Suite::testA")]), [1]);
        assert_eq!(tree.runs_of(&[id("Suite::testB")]), [2]);
        assert_eq!(tree.runs_of(&[id("Suite")]), [1, 2]);
        assert!(tree.holds_run(1) && tree.holds_run(2) && !tree.holds_run(3));
    }
}
