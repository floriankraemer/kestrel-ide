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
    Running,
    Pending,
    Passed,
    Skipped,
}

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
    pub parent: Option<TestId>,
    pub children: Vec<TestId>,
}

/// Live counts across every node currently known, for the dock's toolbar
/// summary ("12 passed, 1 failed, 3 running").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestCounts {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub running: usize,
    pub pending: usize,
}

/// A test run's live tree. Starts empty; [`Self::apply`]/[`Self::apply_junit`]
/// are the only ways anything gets into it.
#[derive(Debug, Default)]
pub struct TestTree {
    nodes: HashMap<TestId, TestNode>,
    roots: Vec<TestId>,
}

impl TestTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop everything — what starting a new run means for the tree a
    /// previous run left behind. A fresh run's tree is never the old one
    /// with rows overwritten: a class that fails to bootstrap this time
    /// should not keep last run's green leaves under it.
    pub fn reset(&mut self) {
        self.nodes.clear();
        self.roots.clear();
    }

    pub fn node(&self, id: &TestId) -> Option<&TestNode> {
        self.nodes.get(id)
    }

    pub fn roots(&self) -> &[TestId] {
        &self.roots
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every leaf test currently `Failed`, in tree order — what "Rerun
    /// Failed" (D6) selects.
    pub fn failed_leaf_ids(&self) -> Vec<TestId> {
        self.roots
            .iter()
            .flat_map(|id| self.leaves_under(id))
            .filter(|id| {
                self.nodes
                    .get(id)
                    .is_some_and(|n| n.status == TestStatus::Failed)
            })
            .collect()
    }

    fn leaves_under(&self, id: &TestId) -> Vec<TestId> {
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
                parent,
                children: Vec::new(),
            },
        );
    }

    fn set_status(&mut self, id: &TestId, status: TestStatus) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = status;
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

    /// Apply one streamed TeamCity event, filling the tree incrementally
    /// as a run reports.
    pub fn apply(&mut self, event: TeamCityEvent) {
        match event {
            TeamCityEvent::SuiteStarted { parent, name } => {
                let id = TestId::child(parent.as_ref(), &name);
                self.insert(id.clone(), name, NodeKind::Suite, parent);
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
            TeamCityEvent::TestStarted { parent, name } => {
                let id = TestId::child(parent.as_ref(), &name);
                self.insert(id.clone(), name, NodeKind::Test, parent);
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
                    // for the same test (TeamCity's own protocol order);
                    // only promote to `Passed` if nothing already marked
                    // it `Failed`.
                    if node.status != TestStatus::Failed {
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
            } => {
                let id = TestId::child(parent.as_ref(), &name);
                self.insert(id.clone(), name, NodeKind::Test, parent);
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.failure = Some(TestFailure { message, details });
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
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent: None,
            name: "testTwo".into(),
            message: "expected 1, got 2".into(),
            details: "at FooTest.php:10".into(),
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
        });
        let parent = Some(id(suite));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "testA".into(),
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
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent: parent.clone(),
            name: "testB".into(),
            message: "boom".into(),
            details: String::new(),
        });
        assert_eq!(tree.node(&id(suite)).unwrap().status, TestStatus::Failed);
    }

    #[test]
    fn nested_suites_qualify_the_test_id() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "Outer".into(),
        });
        let outer = Some(id("Outer"));
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: outer.clone(),
            name: "Inner".into(),
        });
        let inner = Some(id("Outer::Inner"));
        tree.apply(TeamCityEvent::TestStarted {
            parent: inner,
            name: "testDeep".into(),
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
        });
        let parent = Some(id("S"));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "ok".into(),
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent: parent.clone(),
            name: "ok".into(),
            duration_ms: None,
        });
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "bad".into(),
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent,
            name: "bad".into(),
            message: "no".into(),
            details: String::new(),
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
    fn counts_only_leaf_tests_not_suites() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "S".into(),
        });
        let parent = Some(id("S"));
        tree.apply(TeamCityEvent::TestStarted {
            parent: parent.clone(),
            name: "a".into(),
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
        });
        tree.reset();
        assert!(tree.is_empty());
        assert!(tree.roots().is_empty());
    }
}
