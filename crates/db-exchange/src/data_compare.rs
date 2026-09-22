//! Key-aligned data compare (F6.3, database-tools-plan.md): both sides
//! sorted by `key_columns` (in Rust — a merge-walk needs a total order
//! regardless of whether the driver could `ORDER BY` for it), producing
//! a [`DataDiffSummary`] plus a canonical TSV text per side for
//! [`editor_core::diff`] — the existing DiffView renders that the same
//! way it renders any other two-text diff, never a bespoke grid diff.

use std::cmp::Ordering;

use db_core::value::{ColumnMeta, FormatRules, Value};

/// `key_columns` name the row identity two sides are aligned on;
/// `float_tolerance` treats two floats within it as equal rather than a
/// spurious "changed" from binary rounding; `trim` ignores leading/
/// trailing whitespace on text cells (a common false-positive source
/// between two exports of the same data).
#[derive(Debug, Clone, PartialEq)]
pub struct DataCompareOptions {
    pub key_columns: Vec<String>,
    pub float_tolerance: f64,
    pub trim: bool,
}

impl Default for DataCompareOptions {
    fn default() -> Self {
        Self {
            key_columns: Vec::new(),
            float_tolerance: 0.0,
            trim: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataDiffSummary {
    pub only_left: usize,
    pub only_right: usize,
    pub changed: usize,
    pub equal: usize,
}

/// A total order over [`Value`]s for the merge-walk's key comparison —
/// numeric when both sides are numeric, `Null` sorts first, everything
/// else falls back to its own display text (stable and deterministic,
/// if not numerically meaningful for a non-numeric key).
fn value_cmp(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal),
        _ => a
            .display(&FormatRules::default())
            .cmp(&b.display(&FormatRules::default())),
    }
}

fn key_cmp(a: &[&Value], b: &[&Value]) -> Ordering {
    for (left, right) in a.iter().zip(b.iter()) {
        match value_cmp(left, right) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    a.len().cmp(&b.len())
}

fn cell_equal(a: &Value, b: &Value, options: &DataCompareOptions) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => (x - y).abs() <= options.float_tolerance,
        (Value::Text(x), Value::Text(y)) if options.trim => x.trim() == y.trim(),
        _ => a == b,
    }
}

fn rows_equal(a: &[Value], b: &[Value], options: &DataCompareOptions) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| cell_equal(x, y, options))
}

fn key_indices(columns: &[ColumnMeta], key_columns: &[String]) -> Vec<usize> {
    key_columns
        .iter()
        .filter_map(|name| columns.iter().position(|c| &c.name == name))
        .collect()
}

fn row_key<'a>(row: &'a [Value], key_indices: &[usize]) -> Vec<&'a Value> {
    key_indices.iter().map(|&i| &row[i]).collect()
}

fn tsv_line(row: &[Value], rules: &FormatRules) -> String {
    row.iter()
        .map(|v| v.display(rules))
        .collect::<Vec<_>>()
        .join("\t")
}

fn tsv_text(columns: &[ColumnMeta], rows: &[Vec<Value>]) -> String {
    let rules = FormatRules {
        bytes_preview_len: usize::MAX,
        ..FormatRules::default()
    };
    let mut lines = vec![columns
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join("\t")];
    for row in rows {
        lines.push(tsv_line(row, &rules));
    }
    lines.join("\n") + "\n"
}

/// Compare `left` and `right` (already-fetched rows — a caller that wants
/// a truly streaming compare over a huge table pages both sides through
/// this in `key_columns`-sorted chunks, which is exactly what an `ORDER
/// BY key` server-side cursor gives it) under `options`, keyed by
/// `options.key_columns`.
///
/// Returns the summary counts plus a canonical, key-sorted TSV per side —
/// feed both into [`editor_core::diff::diff_lines`] to render the actual
/// row-level diff.
pub fn compare(
    columns: &[ColumnMeta],
    mut left: Vec<Vec<Value>>,
    mut right: Vec<Vec<Value>>,
    options: &DataCompareOptions,
) -> (DataDiffSummary, String, String) {
    let indices = key_indices(columns, &options.key_columns);
    left.sort_by(|a, b| key_cmp(&row_key(a, &indices), &row_key(b, &indices)));
    right.sort_by(|a, b| key_cmp(&row_key(a, &indices), &row_key(b, &indices)));

    let mut summary = DataDiffSummary::default();
    let (mut li, mut ri) = (0usize, 0usize);
    while li < left.len() && ri < right.len() {
        let ordering = key_cmp(
            &row_key(&left[li], &indices),
            &row_key(&right[ri], &indices),
        );
        match ordering {
            Ordering::Less => {
                summary.only_left += 1;
                li += 1;
            }
            Ordering::Greater => {
                summary.only_right += 1;
                ri += 1;
            }
            Ordering::Equal => {
                if rows_equal(&left[li], &right[ri], options) {
                    summary.equal += 1;
                } else {
                    summary.changed += 1;
                }
                li += 1;
                ri += 1;
            }
        }
    }
    summary.only_left += left.len() - li;
    summary.only_right += right.len() - ri;

    (summary, tsv_text(columns, &left), tsv_text(columns, &right))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta {
                name: "id".to_string(),
                type_name: "int".to_string(),
                nullable: false,
                origin: None,
            },
            ColumnMeta {
                name: "name".to_string(),
                type_name: "text".to_string(),
                nullable: false,
                origin: None,
            },
        ]
    }

    fn options() -> DataCompareOptions {
        DataCompareOptions {
            key_columns: vec!["id".to_string()],
            ..DataCompareOptions::default()
        }
    }

    #[test]
    fn identical_rows_on_both_sides_are_all_equal() {
        let rows = vec![
            vec![Value::Int(1), Value::Text("a".to_string())],
            vec![Value::Int(2), Value::Text("b".to_string())],
        ];
        let (summary, left_tsv, right_tsv) = compare(&columns(), rows.clone(), rows, &options());
        assert_eq!(
            summary,
            DataDiffSummary {
                equal: 2,
                ..Default::default()
            }
        );
        assert_eq!(left_tsv, right_tsv);
    }

    #[test]
    fn a_row_only_on_the_left_counts_as_only_left() {
        let left = vec![vec![Value::Int(1), Value::Text("a".to_string())]];
        let right = vec![];
        let (summary, _, _) = compare(&columns(), left, right, &options());
        assert_eq!(summary.only_left, 1);
        assert_eq!(summary.only_right, 0);
    }

    #[test]
    fn a_row_only_on_the_right_counts_as_only_right() {
        let left = vec![];
        let right = vec![vec![Value::Int(1), Value::Text("a".to_string())]];
        let (summary, _, _) = compare(&columns(), left, right, &options());
        assert_eq!(summary.only_right, 1);
    }

    #[test]
    fn a_row_with_the_same_key_but_different_data_counts_as_changed() {
        let left = vec![vec![Value::Int(1), Value::Text("a".to_string())]];
        let right = vec![vec![Value::Int(1), Value::Text("b".to_string())]];
        let (summary, _, _) = compare(&columns(), left, right, &options());
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.equal, 0);
    }

    #[test]
    fn rows_are_key_aligned_regardless_of_input_order() {
        let left = vec![
            vec![Value::Int(2), Value::Text("b".to_string())],
            vec![Value::Int(1), Value::Text("a".to_string())],
        ];
        let right = vec![
            vec![Value::Int(1), Value::Text("a".to_string())],
            vec![Value::Int(2), Value::Text("b".to_string())],
        ];
        let (summary, _, _) = compare(&columns(), left, right, &options());
        assert_eq!(
            summary,
            DataDiffSummary {
                equal: 2,
                ..Default::default()
            }
        );
    }

    #[test]
    fn float_tolerance_treats_a_small_difference_as_equal() {
        let cols = vec![
            ColumnMeta {
                name: "id".to_string(),
                type_name: "int".to_string(),
                nullable: false,
                origin: None,
            },
            ColumnMeta {
                name: "amount".to_string(),
                type_name: "float".to_string(),
                nullable: false,
                origin: None,
            },
        ];
        let left = vec![vec![Value::Int(1), Value::Float(1.000_001)]];
        let right = vec![vec![Value::Int(1), Value::Float(1.000_002)]];
        let opts = DataCompareOptions {
            key_columns: vec!["id".to_string()],
            float_tolerance: 0.001,
            trim: false,
        };
        let (summary, _, _) = compare(&cols, left, right, &opts);
        assert_eq!(summary.equal, 1);
        assert_eq!(summary.changed, 0);
    }

    #[test]
    fn trim_ignores_leading_and_trailing_whitespace() {
        let left = vec![vec![Value::Int(1), Value::Text(" a ".to_string())]];
        let right = vec![vec![Value::Int(1), Value::Text("a".to_string())]];
        let opts = DataCompareOptions {
            key_columns: vec!["id".to_string()],
            float_tolerance: 0.0,
            trim: true,
        };
        let (summary, _, _) = compare(&columns(), left, right, &opts);
        assert_eq!(summary.equal, 1);
    }

    #[test]
    fn the_canonical_tsv_is_stable_and_diffable() {
        let left = vec![vec![Value::Int(1), Value::Text("a".to_string())]];
        let right = vec![vec![Value::Int(1), Value::Text("b".to_string())]];
        let (_, left_tsv, right_tsv) = compare(&columns(), left, right, &options());
        assert_eq!(left_tsv, "id\tname\n1\ta\n");
        assert_eq!(right_tsv, "id\tname\n1\tb\n");
        let hunks = editor_core::diff::diff_lines(&left_tsv, &right_tsv).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, editor_core::diff::HunkKind::Modified);
    }
}
