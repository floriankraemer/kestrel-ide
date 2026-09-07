//! The two sides of a diff laid out as one sequence of rows.
//!
//! This is the alignment model behind everything the viewer does with
//! "which line sits level with which": the unified viewer *is* this list
//! rendered top to bottom, the side-by-side viewer scrolls one pane to the
//! row its neighbour is showing, and the divider joins a hunk's rows on the
//! left to the same hunk's rows on the right. The view never derives any
//! of it — it indexes.

use std::ops::Range;

use super::Hunk;

/// What a row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// The same line on both sides.
    Context,
    /// A line only the new side has.
    Added,
    /// A line only the old side has.
    Removed,
}

/// One row of the aligned layout.
///
/// A modified hunk is its removed rows followed by its added rows — the
/// unified viewer's order, and the one JetBrains uses. `old_anchor` /
/// `new_anchor` name the line on each side this row sits level with: the
/// row's own line where it has one, otherwise the counterpart hunk's line
/// at the same offset (so the k-th removed line of a modification lines up
/// with the k-th added one), clamped into that side's line range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub kind: RowKind,
    pub old_anchor: usize,
    pub new_anchor: usize,
}

/// Lay out two sides of `old_line_count` and `new_line_count` lines, joined
/// by `hunks` (ascending and non-overlapping, as `diff_lines` produces).
pub fn diff_rows(old_line_count: usize, new_line_count: usize, hunks: &[Hunk]) -> Vec<DiffRow> {
    let mut rows = Vec::with_capacity(old_line_count.max(new_line_count));
    let mut old = 0;
    let mut new = 0;
    let context = |rows: &mut Vec<DiffRow>, old: Range<usize>, new: Range<usize>| {
        for (o, n) in old.zip(new) {
            rows.push(DiffRow {
                old: Some(o),
                new: Some(n),
                kind: RowKind::Context,
                old_anchor: o,
                new_anchor: n,
            });
        }
    };
    for hunk in hunks {
        context(&mut rows, old..hunk.old.start, new..hunk.new.start);
        for (k, o) in hunk.old.clone().enumerate() {
            rows.push(DiffRow {
                old: Some(o),
                new: None,
                kind: RowKind::Removed,
                old_anchor: o,
                new_anchor: level_with(&hunk.new, k, new_line_count),
            });
        }
        for (k, n) in hunk.new.clone().enumerate() {
            rows.push(DiffRow {
                old: None,
                new: Some(n),
                kind: RowKind::Added,
                old_anchor: level_with(&hunk.old, k, old_line_count),
                new_anchor: n,
            });
        }
        old = hunk.old.end;
        new = hunk.new.end;
    }
    context(&mut rows, old..old_line_count, new..new_line_count);
    rows
}

/// The line of `counterpart` at offset `k`, or its last line when it is
/// shorter, or where it would have started when it is empty — always inside
/// `0..line_count` (or 0 for an empty side).
fn level_with(counterpart: &Range<usize>, k: usize, line_count: usize) -> usize {
    let line = if counterpart.is_empty() {
        counterpart.start
    } else {
        (counterpart.start + k).min(counterpart.end - 1)
    };
    line.min(line_count.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_lines;

    fn rows_between(before: &str, after: &str) -> Vec<DiffRow> {
        let hunks = diff_lines(before, after).unwrap();
        diff_rows(before.lines().count(), after.lines().count(), &hunks)
    }

    fn kinds(rows: &[DiffRow]) -> Vec<RowKind> {
        rows.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn identical_texts_are_all_context() {
        let rows = rows_between("a\nb\n", "a\nb\n");
        assert_eq!(kinds(&rows), vec![RowKind::Context, RowKind::Context]);
        assert_eq!(rows[1].old, Some(1));
        assert_eq!(rows[1].new, Some(1));
    }

    #[test]
    fn a_pure_addition_sits_between_its_context() {
        let rows = rows_between("a\nc\n", "a\nb\nc\n");
        assert_eq!(
            kinds(&rows),
            vec![RowKind::Context, RowKind::Added, RowKind::Context]
        );
        assert_eq!(rows[1].old, None);
        assert_eq!(rows[1].new, Some(1));
        assert_eq!(
            rows[1].old_anchor, 1,
            "level with where `c` starts on the old side"
        );
        assert_eq!(rows[2].old, Some(1));
        assert_eq!(rows[2].new, Some(2));
    }

    #[test]
    fn a_pure_removal_anchors_to_where_it_happened_on_the_new_side() {
        let rows = rows_between("a\nb\nc\n", "a\nc\n");
        assert_eq!(
            kinds(&rows),
            vec![RowKind::Context, RowKind::Removed, RowKind::Context]
        );
        assert_eq!(rows[1].new, None);
        assert_eq!(rows[1].new_anchor, 1);
    }

    #[test]
    fn a_modification_is_its_removed_rows_then_its_added_rows() {
        let rows = rows_between("a\nb1\nb2\nz\n", "a\nc1\nc2\nc3\nz\n");
        assert_eq!(
            kinds(&rows),
            vec![
                RowKind::Context,
                RowKind::Removed,
                RowKind::Removed,
                RowKind::Added,
                RowKind::Added,
                RowKind::Added,
                RowKind::Context,
            ]
        );
        // The k-th removed line is level with the k-th added one …
        assert_eq!(rows[1].new_anchor, 1);
        assert_eq!(rows[2].new_anchor, 2);
        assert_eq!(rows[3].old_anchor, 1);
        assert_eq!(rows[4].old_anchor, 2);
        // … and the surplus added line with the last removed one.
        assert_eq!(rows[5].old_anchor, 2);
    }

    #[test]
    fn anchors_never_decrease_on_either_side() {
        let before = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let after = "1\nX\n3\n4\nY\nY2\n6\n7\n";
        let rows = rows_between(before, after);
        for pair in rows.windows(2) {
            assert!(pair[0].old_anchor <= pair[1].old_anchor, "{pair:?}");
            assert!(pair[0].new_anchor <= pair[1].new_anchor, "{pair:?}");
        }
    }

    #[test]
    fn every_line_of_both_sides_appears_exactly_once() {
        let before = "a\nb\nc\nd\ne\nf\n";
        let after = "a\nc\ne\nE\nf\ng\n";
        let rows = rows_between(before, after);
        let olds: Vec<usize> = rows.iter().filter_map(|r| r.old).collect();
        let news: Vec<usize> = rows.iter().filter_map(|r| r.new).collect();
        assert_eq!(olds, (0..6).collect::<Vec<_>>());
        assert_eq!(news, (0..6).collect::<Vec<_>>());
        let context = rows.iter().filter(|r| r.kind == RowKind::Context).count();
        assert_eq!(rows.len(), 6 + 6 - context);
    }

    #[test]
    fn an_empty_side_anchors_to_zero() {
        let rows = rows_between("", "a\nb\n");
        assert_eq!(kinds(&rows), vec![RowKind::Added, RowKind::Added]);
        assert_eq!(rows[1].old_anchor, 0);
        let rows = rows_between("a\n", "");
        assert_eq!(kinds(&rows), vec![RowKind::Removed]);
        assert_eq!(rows[0].new_anchor, 0);
    }
}
