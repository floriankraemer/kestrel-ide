//! The Tests dock's rows, and which of them changed since the view last
//! drew them (#462).
//!
//! The dock used to clear and rebuild its whole tree for every streamed
//! event, which is quadratic in the suite's size: 514 tests took 42 s.
//! [`PublishedRows::update`] answers with only the rows the view does not
//! have yet or has stale, so the view's work per event is the handful of
//! nodes the event touched (the test and its suites), not the whole tree.

use std::collections::HashMap;

use crate::tree::{NodeKind, TestId, TestStatus, TestTree};

/// One node as the view draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub id: TestId,
    pub parent: Option<TestId>,
    /// The node's position among its parent's children (or the roots).
    pub index: usize,
    pub name: String,
    pub kind: NodeKind,
    pub status: TestStatus,
    pub duration_ms: Option<u64>,
    pub has_failure: bool,
    /// The run that last reported the node: a rerun that ends exactly as
    /// the previous run did still counts as a change, so the view (and the
    /// E2E marks it writes per changed row) sees the node was run again.
    pub run: u64,
}

/// What the view has to apply to catch up with the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowChanges {
    /// The view's rows no longer fit the tree (a new run replaced it, a
    /// node moved): drop them all and build `rows` from scratch.
    pub reset: bool,
    /// Parents before their children, so the view can insert each row
    /// under a parent it already has.
    pub rows: Vec<TreeRow>,
}

/// The rows the view was last given.
#[derive(Debug, Default)]
pub struct PublishedRows {
    rows: HashMap<TestId, TreeRow>,
}

impl PublishedRows {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows of `tree` the view lacks or has stale, remembered as given.
    ///
    /// ponytail: walks the whole tree per call, a cheap Rust pass next to the
    /// Qt work it saves; have the tree record the ids it touched if a suite
    /// ever makes this pass show up.
    pub fn update(&mut self, tree: &TestTree) -> RowChanges {
        let current = rows_of(tree);
        let reset = current.len() < self.rows.len()
            || self.rows.keys().any(|id| tree.node(id).is_none())
            || current.iter().any(|row| {
                self.rows
                    .get(&row.id)
                    .is_some_and(|old| old.parent != row.parent || old.index != row.index)
            });
        if reset {
            self.rows.clear();
        }
        let rows: Vec<TreeRow> = current
            .into_iter()
            .filter(|row| self.rows.get(&row.id) != Some(row))
            .collect();
        for row in &rows {
            self.rows.insert(row.id.clone(), row.clone());
        }
        RowChanges { reset, rows }
    }
}

/// Every node of `tree`, breadth-first from the roots so a suite always
/// precedes its own children.
pub fn rows_of(tree: &TestTree) -> Vec<TreeRow> {
    let mut rows: Vec<TreeRow> = Vec::new();
    let mut queue: Vec<(TestId, usize)> = tree.roots().iter().cloned().zip(0..).collect();
    let mut next = 0;
    while next < queue.len() {
        let (id, index) = queue[next].clone();
        next += 1;
        let Some(node) = tree.node(&id) else {
            continue;
        };
        queue.extend(node.children.iter().cloned().zip(0..));
        rows.push(TreeRow {
            id,
            parent: node.parent.clone(),
            index,
            name: node.name.clone(),
            kind: node.kind,
            status: node.status,
            duration_ms: node.duration_ms,
            has_failure: node.failure.is_some(),
            run: node.run,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teamcity::TeamCityEvent;

    fn suite(name: &str) -> TeamCityEvent {
        TeamCityEvent::SuiteStarted {
            parent: None,
            name: name.into(),
            location: None,
        }
    }

    fn started(suite: &str, name: &str) -> TeamCityEvent {
        TeamCityEvent::TestStarted {
            parent: Some(TestId(suite.into())),
            name: name.into(),
            location: None,
        }
    }

    fn finished(suite: &str, name: &str) -> TeamCityEvent {
        TeamCityEvent::TestFinished {
            parent: Some(TestId(suite.into())),
            name: name.into(),
            duration_ms: Some(1),
        }
    }

    fn names(changes: &RowChanges) -> Vec<&str> {
        changes.rows.iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn the_first_update_gives_every_row_parents_first() {
        let mut tree = TestTree::new();
        tree.apply(suite("S"));
        tree.apply(started("S", "a"));
        tree.apply(started("S", "b"));
        let changes = PublishedRows::new().update(&tree);
        assert!(!changes.reset);
        assert_eq!(names(&changes), ["S", "a", "b"]);
        assert_eq!(changes.rows[2].index, 1);
        assert_eq!(changes.rows[2].parent, Some(TestId("S".into())));
    }

    #[test]
    fn an_event_gives_only_the_rows_it_touched() {
        let mut tree = TestTree::new();
        let mut published = PublishedRows::new();
        tree.apply(suite("S"));
        tree.apply(started("S", "a"));
        published.update(&tree);
        tree.apply(finished("S", "a"));
        tree.apply(started("S", "b"));
        let changes = published.update(&tree);
        assert!(!changes.reset);
        // `a` passed, `b` arrived; the suite's status did not change (it is
        // still running `b`).
        assert_eq!(names(&changes), ["a", "b"]);
        assert_eq!(published.update(&tree), RowChanges::default());
    }

    #[test]
    fn a_rerun_that_ends_the_same_way_still_changes_its_row() {
        let mut tree = TestTree::new();
        let mut published = PublishedRows::new();
        tree.apply(suite("S"));
        tree.apply(started("S", "a"));
        tree.apply(finished("S", "a"));
        published.update(&tree);
        tree.begin_run(
            2,
            crate::RunPlace {
                host: process_exec::host::ExecHost::Local,
                work_dir: "/p".into(),
                framework: "phpunit".into(),
            },
        );
        tree.apply(started("S", "a"));
        tree.apply(finished("S", "a"));
        assert_eq!(names(&published.update(&tree)), ["a"]);
    }

    #[test]
    fn a_replaced_tree_resets_the_view() {
        let mut tree = TestTree::new();
        let mut published = PublishedRows::new();
        tree.apply(suite("S"));
        tree.apply(started("S", "a"));
        published.update(&tree);
        tree.reset();
        tree.apply(suite("T"));
        let changes = published.update(&tree);
        assert!(changes.reset);
        assert_eq!(names(&changes), ["T"]);
        tree.reset();
        assert!(published.update(&tree).reset, "an emptied tree");
    }

    #[test]
    fn a_large_run_costs_the_view_work_in_proportion_to_its_events() {
        // The #462 suite size: per event the view gets the touched test and
        // at most its suite, never the whole tree, so the total stays linear.
        const SUITES: usize = 10;
        const TESTS_PER_SUITE: usize = 60;
        let mut tree = TestTree::new();
        let mut published = PublishedRows::new();
        let mut events = 0;
        let mut rows_given = 0;
        let mut largest = 0;
        for s in 0..SUITES {
            let suite_name = format!("Suite{s}");
            let mut batch = vec![suite(&suite_name)];
            for t in 0..TESTS_PER_SUITE {
                let test = format!("test{t}");
                batch.push(started(&suite_name, &test));
                batch.push(finished(&suite_name, &test));
            }
            for event in batch {
                tree.apply(event);
                let changes = published.update(&tree);
                assert!(!changes.reset);
                events += 1;
                rows_given += changes.rows.len();
                largest = largest.max(changes.rows.len());
            }
        }
        assert_eq!(rows_of(&tree).len(), SUITES * (TESTS_PER_SUITE + 1));
        assert!(largest <= 2, "one event touched {largest} rows");
        assert!(
            rows_given <= 2 * events,
            "{rows_given} rows for {events} events"
        );
    }
}
