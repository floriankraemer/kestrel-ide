//! A simple lane layout for the commit-log graph column (R7's "simple lane
//! graph" — not a full DAG-curve renderer, just which column each commit
//! sits in and which columns its parents continue into, the way `git log
//! --graph`'s ASCII art reduces to before it gets drawn).
//!
//! Pure and view-free on purpose: `vcs-core`'s job is to say *which lane*,
//! not to paint one — the delegate in `ui-shell/cpp` turns this into pixels.

use crate::history::LogEntry;

/// One commit's position in the lane graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneCommit {
    pub id: String,
    /// The column this commit's dot sits in.
    pub lane: usize,
    /// The column each of this commit's parents continues in, same order
    /// as `LogEntry::parent_ids` — the first is "straight down", any
    /// further one is a merge line branching off to another lane.
    pub parent_lanes: Vec<usize>,
}

/// Assigns each commit in `entries` (newest first, as [`Repository::log`]
/// already orders them) a lane, and each of its parents a lane to draw a
/// line into.
///
/// The algorithm is the textbook "active lanes" walk: each lane tracks
/// which commit id it is waiting for next. A commit takes over the lane
/// that was waiting for it (or opens a new one, for the second and later
/// parent of a merge, or for a commit nothing so far pointed at — a branch
/// tip with no descendant in this slice). Its first parent continues in the
/// same lane; every other parent gets a lane of its own, freed again the
/// first time some other commit is found waiting for nothing.
pub fn lanes(entries: &[LogEntry]) -> Vec<LaneCommit> {
    // `lane_waiting_for[i]` is the commit id lane `i` is waiting to draw
    // down to, or `None` for a freed lane a later commit may reuse.
    let mut lane_waiting_for: Vec<Option<String>> = Vec::new();
    let mut result = Vec::with_capacity(entries.len());

    for entry in entries {
        // The lane already waiting for this commit, or a fresh one.
        let lane = lane_waiting_for
            .iter()
            .position(|w| w.as_deref() == Some(entry.id.as_str()))
            .unwrap_or_else(|| {
                let free = lane_waiting_for.iter().position(Option::is_none);
                match free {
                    Some(i) => i,
                    None => {
                        lane_waiting_for.push(None);
                        lane_waiting_for.len() - 1
                    }
                }
            });

        let mut parent_lanes = Vec::with_capacity(entry.parent_ids.len());
        for (i, parent_id) in entry.parent_ids.iter().enumerate() {
            if i == 0 {
                lane_waiting_for[lane] = Some(parent_id.clone());
                parent_lanes.push(lane);
                continue;
            }
            // A second-or-later parent (a merge): reuse a lane already
            // waiting for this exact parent (two merges converging on the
            // same ancestor), else the first free lane, else a new one.
            let existing = lane_waiting_for
                .iter()
                .position(|w| w.as_deref() == Some(parent_id.as_str()));
            let merge_lane = existing.unwrap_or_else(|| {
                let free = lane_waiting_for.iter().position(Option::is_none);
                match free {
                    Some(i) => {
                        lane_waiting_for[i] = Some(parent_id.clone());
                        i
                    }
                    None => {
                        lane_waiting_for.push(Some(parent_id.clone()));
                        lane_waiting_for.len() - 1
                    }
                }
            });
            parent_lanes.push(merge_lane);
        }
        if entry.parent_ids.is_empty() {
            // A root commit frees its lane — nothing continues below it.
            lane_waiting_for[lane] = None;
        }

        result.push(LaneCommit {
            id: entry.id.clone(),
            lane,
            parent_lanes,
        });
    }

    result
}

/// Convenience for a caller that only wants "how many lanes does this graph
/// need" — a delegate's column width.
pub fn lane_count(commits: &[LaneCommit]) -> usize {
    commits.iter().map(|c| c.lane).max().map_or(0, |m| m + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, parents: &[&str]) -> LogEntry {
        LogEntry {
            id: id.to_string(),
            summary: id.to_string(),
            author_name: "Test".to_string(),
            author_email: "test@example.com".to_string(),
            author_time: 0,
            parent_ids: parents.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn a_linear_history_stays_in_lane_zero() {
        let entries = vec![entry("c", &["b"]), entry("b", &["a"]), entry("a", &[])];
        let result = lanes(&entries);
        assert!(result.iter().all(|c| c.lane == 0));
        assert_eq!(lane_count(&result), 1);
    }

    #[test]
    fn a_root_commit_has_no_parent_lanes() {
        let entries = vec![entry("a", &[])];
        let result = lanes(&entries);
        assert_eq!(result[0].lane, 0);
        assert!(result[0].parent_lanes.is_empty());
    }

    #[test]
    fn a_merge_commit_opens_a_second_lane_for_its_side_branch() {
        // c is a merge of b (first parent, main) and f (second parent, a
        // feature branch never seen again in this slice).
        let entries = vec![entry("c", &["b", "f"]), entry("b", &["a"]), entry("a", &[])];
        let result = lanes(&entries);
        assert_eq!(result[0].lane, 0);
        assert_eq!(result[0].parent_lanes, vec![0, 1]);
    }

    #[test]
    fn two_diverging_then_reconverging_branches_reuse_the_freed_lane() {
        // c1 (lane 0) and c2 (lane 1) both descend from base; base is a
        // root so its lane frees, proving lane reuse rather than
        // ever-growing lane count.
        let entries = vec![
            entry("c2", &["base"]),
            entry("c1", &["base"]),
            entry("base", &[]),
        ];
        let result = lanes(&entries);
        // c2 opens lane 0 (nothing was waiting for it), c1 needs a second,
        // distinct lane since lane 0 is now waiting for "base" via c2.
        assert_ne!(result[0].lane, result[1].lane);
        assert_eq!(lane_count(&result), 2);
    }
}
