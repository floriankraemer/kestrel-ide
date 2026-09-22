//! `ResultProvider` (database-tools-plan F3.4/F3.5, F4c): a result's rows
//! and text/aggregate views — split out of `console.rs` once that module
//! hit the file-size ceiling (`scripts/check-file-size.sh`), the same
//! reason `edit.rs`/`navigate.rs` split out of it earlier. Reads/mutates
//! `console::Shared`/`ResultState` directly, same visibility rule those
//! sibling files' own doc comments already state for the same reason.

use std::collections::HashMap;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Instant;

use cxx_qt_lib::{QString, QStringList};

use db_core::driver::ExecOptions;
use db_core::driver::Statement as DbStatement;
use db_core::result::ResultSet;
use db_core::value::{ColumnMeta, Value};

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiDbColumn, FfiDbRow, FfiResult};

use super::console::{ResultState, Shared};
use super::edit::EditState;
use super::sessions::SessionCommand;

const CELL_SEP: char = '\u{1f}'; // unit separator — no `Value::display` text contains it.

/// `rowPage`'s own clamp — the same "a caller cannot ask for the whole
/// gigabyte in one FFI call" rule `MAX_HEX_ROWS_PER_REQUEST` already
/// applies to the hex viewer (`bridge::convert`'s own doc comment).
const MAX_ROWS_PER_REQUEST: u64 = 4096;

pub struct ResultProviderRust {
    pub(crate) shared: Rc<std::cell::RefCell<Shared>>,
    page_sizes: std::cell::RefCell<HashMap<u64, u32>>,
}

impl Default for ResultProviderRust {
    fn default() -> Self {
        Self {
            shared: crate::bridge::registry::shared_database_consoles(),
            page_sizes: std::cell::RefCell::default(),
        }
    }
}

fn row_to_ffi(row: &[Value], format: &db_core::value::FormatRules, flags: u8) -> FfiDbRow {
    let mut cells = String::new();
    let mut nulls = String::new();
    for (i, value) in row.iter().enumerate() {
        if i > 0 {
            cells.push(CELL_SEP);
            nulls.push(CELL_SEP);
        }
        cells.push_str(&value.display(format));
        nulls.push(if matches!(value, Value::Null) {
            '1'
        } else {
            '0'
        });
    }
    FfiDbRow {
        cells: QString::from(cells.as_str()),
        nulls: QString::from(nulls.as_str()),
        flags,
    }
}

/// `rowPage`/`rowValues`'s shared page walk: reads `count` (clamped) rows
/// of `result_id` starting at `first`, in FFI time bounded by the batches
/// actually touched (`rowPage`'s own doc comment on why — unchanged by
/// this split), rendering each row through `render` — the only thing the
/// two callers differ on (display text vs. a typed JSON encoding).
fn walk_row_range<T>(
    shared: &Shared,
    result_id: u64,
    first: u64,
    count: u64,
    mut render: impl FnMut(&[Value], u8) -> T,
) -> Vec<T> {
    let Some(result) = shared.results.get(&result_id) else {
        return Vec::new();
    };
    let count = count.min(MAX_ROWS_PER_REQUEST);
    let fetched_count = result.set.row_count() as u64;
    let mut out = Vec::with_capacity(count as usize);
    let mut skipped = 0u64;
    let mut grid_row = first;
    if first < fetched_count {
        'batches: for batch in result.set.batches() {
            let batch_len = batch.rows.len() as u64;
            if skipped + batch_len <= first {
                skipped += batch_len;
                continue;
            }
            let start_in_batch = first.saturating_sub(skipped) as usize;
            for row in &batch.rows[start_in_batch..] {
                if out.len() as u64 >= count {
                    break 'batches;
                }
                let flags = match &result.edit {
                    EditState::Editable(buffer) => buffer.row_flags(grid_row as usize),
                    _ => 0,
                };
                out.push(render(row, flags));
                grid_row += 1;
            }
            skipped += batch_len;
        }
    }
    // Rows staged past the fetched count (`addRow`/`cloneRow`) — read
    // straight from the buffer, one column at a time, since they were
    // never fetched from a driver at all.
    if let EditState::Editable(buffer) = &result.edit {
        let total = fetched_count + buffer.inserted_row_count() as u64;
        while (out.len() as u64) < count && grid_row < total {
            let values: Vec<Value> = result
                .columns
                .iter()
                .map(|c| {
                    buffer
                        .current_value(grid_row as usize, &c.name)
                        .unwrap_or(Value::Null)
                })
                .collect();
            out.push(render(&values, buffer.row_flags(grid_row as usize)));
            grid_row += 1;
        }
    }
    out
}

impl ffi::ResultProvider {
    pub fn columns(self: Pin<&mut Self>, result_id: u64) -> Vec<FfiDbColumn> {
        let shared = self.shared.borrow();
        shared
            .results
            .get(&result_id)
            .map(|result| {
                result
                    .columns
                    .iter()
                    .map(|c| FfiDbColumn {
                        name: QString::from(c.name.as_str()),
                        type_name: QString::from(c.type_name.as_str()),
                        nullable: c.nullable,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn row_count(self: Pin<&mut Self>, result_id: u64) -> u64 {
        let shared = self.shared.borrow();
        shared
            .results
            .get(&result_id)
            .map(|result| {
                let fetched = result.set.row_count() as u64;
                let inserted = match &result.edit {
                    EditState::Editable(buffer) => buffer.inserted_row_count() as u64,
                    _ => 0,
                };
                fetched + inserted
            })
            .unwrap_or(0)
    }

    /// Reads `count` (clamped) rows starting at `first`, in FFI time
    /// bounded by the batches actually touched rather than the whole
    /// result — walking `result.set.batches()` in order and skipping
    /// whole untouched batches by their row count, never flattening the
    /// full result into one `Vec` first (the ≤16ms-per-page NFR,
    /// `database-tools.md` §8).
    /// ponytail: still a linear scan of the batches *before* the page
    /// (cheap — an integer add per batch, not a row copy), not a
    /// prefix-sum index; revisit if a profile ever shows this mattering
    /// at the batch counts a page near the far end of a huge result
    /// produces.
    pub fn row_page(self: Pin<&mut Self>, result_id: u64, first: u64, count: u64) -> Vec<FfiDbRow> {
        let shared = self.shared.borrow();
        let format = db_core::value::FormatRules::default();
        walk_row_range(&shared, result_id, first, count, |row, flags| {
            row_to_ffi(row, &format, flags)
        })
    }

    /// See `ffi.rs`'s own doc comment on `rowValues` — the same page as
    /// `rowPage`, one JSON-encoded row per entry instead of rendered
    /// text.
    pub fn row_values(self: Pin<&mut Self>, result_id: u64, first: u64, count: u64) -> QStringList {
        let shared = self.shared.borrow();
        walk_row_range(&shared, result_id, first, count, |row, _flags| {
            QString::from(db_core::value::row_to_json(row).as_str())
        })
        .into_iter()
        .collect()
    }

    pub fn fetch_more(self: Pin<&mut Self>, result_id: u64) -> FfiResult {
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        if result.done {
            return FfiResult::default();
        }
        let Some(console) = shared.consoles.get(&result.tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "console detached");
        };
        match console.worker.send(SessionCommand::FetchMore) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }

    /// `true` when a further `fetchMore` could return more rows — the
    /// grid's own `canFetchMore` (`ResultTableModel`'s doc comment).
    pub fn fetch_more_available(self: Pin<&mut Self>, result_id: u64) -> bool {
        let shared = self.shared.borrow();
        shared
            .results
            .get(&result_id)
            .is_some_and(|result| !result.done)
    }

    pub fn set_page_size(self: Pin<&mut Self>, result_id: u64, size: u32) {
        self.page_sizes.borrow_mut().insert(result_id, size.max(1));
    }

    pub fn text_view(
        self: Pin<&mut Self>,
        result_id: u64,
        format: ffi::FfiDbTextFormat,
    ) -> QString {
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return QString::default();
        };
        let rules = db_core::value::FormatRules::default();
        let rows: Vec<&Vec<Value>> = result
            .set
            .batches()
            .iter()
            .flat_map(|batch| batch.rows.iter())
            .collect();
        let text = match format {
            ffi::FfiDbTextFormat::Json => {
                let objects: Vec<String> = rows
                    .iter()
                    .map(|row| {
                        let fields: Vec<String> = result
                            .columns
                            .iter()
                            .zip(row.iter())
                            .map(|(col, value)| {
                                format!("{:?}:{:?}", col.name, value.display(&rules))
                            })
                            .collect();
                        format!("{{{}}}", fields.join(","))
                    })
                    .collect();
                format!("[{}]", objects.join(","))
            }
            ffi::FfiDbTextFormat::Tsv => render_delimited(&result.columns, &rows, &rules, '\t'),
            _ => render_delimited(&result.columns, &rows, &rules, ','),
        };
        QString::from(text.as_str())
    }

    /// A best-effort aggregate over the rows fetched *so far* — not the
    /// whole result if paging has not reached the end yet.
    /// ponytail: client-side over the current page cache rather than a
    /// re-executed `SELECT agg(col) FROM (...)`; upgrade once a console
    /// needs an exact aggregate over an unfetched tail.
    pub fn aggregate(
        self: Pin<&mut Self>,
        result_id: u64,
        column: &QString,
        op: ffi::FfiDbAggOp,
    ) -> QString {
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return QString::default();
        };
        let column = column.to_string();
        let Some(index) = result.columns.iter().position(|c| c.name == column) else {
            return QString::default();
        };
        let numbers: Vec<f64> = result
            .set
            .batches()
            .iter()
            .flat_map(|batch| batch.rows.iter())
            .filter_map(|row| match row.get(index) {
                Some(Value::Int(n)) => Some((*n) as f64),
                Some(Value::Float(n)) => Some(*n),
                Some(Value::Decimal(text)) => text.parse::<f64>().ok(),
                _ => None,
            })
            .collect();
        let text = match op {
            ffi::FfiDbAggOp::Sum => numbers.iter().sum::<f64>().to_string(),
            ffi::FfiDbAggOp::Avg => {
                if numbers.is_empty() {
                    "0".to_string()
                } else {
                    (numbers.iter().sum::<f64>() / numbers.len() as f64).to_string()
                }
            }
            ffi::FfiDbAggOp::Min => numbers
                .iter()
                .cloned()
                .fold(f64::INFINITY, f64::min)
                .to_string(),
            ffi::FfiDbAggOp::Max => numbers
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
                .to_string(),
            _ => result.set.row_count().to_string(),
        };
        QString::from(text.as_str())
    }

    /// Re-executes `resultId`'s original statement with `WHERE`/`ORDER BY`
    /// applied through `db_sql::apply_clauses` — a plain `SELECT` gets its
    /// own clauses extended in place; anything else falls back to that
    /// module's derived-table wrapper (its own doc comment says which is
    /// which), on the same console worker that ran the original statement.
    /// `ResultProvider` cannot call `ConsoleService::execute` directly
    /// (they are two separate QObjects — see this module's own doc
    /// comment on why `Shared` exists), so this sends the `Execute`
    /// command straight to the worker; the worker's replies still land on
    /// `ConsoleService::apply_event` regardless of who sent the command
    /// (its `on_event` closure was bound to that QObject once, at
    /// `attach` time), so `rowsAppended`/`executionFinished` still fire
    /// exactly as they do for a plain "Run".
    /// ponytail: does not validate `where_clause`/`order_by` beyond what
    /// the database itself rejects; the read-only guard still runs below,
    /// so this cannot smuggle a write.
    pub fn apply_clauses(
        self: Pin<&mut Self>,
        result_id: u64,
        where_clause: &QString,
        order_by: &QString,
    ) -> FfiResult {
        let where_clause = where_clause.to_string();
        let order_by = order_by.to_string();
        let mut shared = self.shared.borrow_mut();
        let Some(existing) = shared.results.get(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let tab_id = existing.tab_id;
        let Some(console) = shared.consoles.get(&tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "console detached");
        };
        let statement = db_sql::apply_clauses(
            &existing.statement_text,
            &where_clause,
            &order_by,
            console.dialect,
        );
        if let Err(error) = console.guard.check(&statement) {
            return errors::failure(errors::CODE_REFUSED, error.message);
        }
        let page_size = console.page_size;
        let dialect = console.dialect;
        let new_id = shared.alloc_result_id();
        shared.results.insert(
            new_id,
            ResultState {
                tab_id,
                columns: Vec::new(),
                set: ResultSet::new(),
                done: false,
                reported: false,
                cap_reached: false,
                affected: None,
                error: None,
                started_at: Instant::now(),
                elapsed_ms: None,
                statement_text: statement.clone(),
                edit: EditState::Unknown,
                table_constraints: Vec::new(),
            },
        );
        shared.consoles.get_mut(&tab_id).unwrap().current_result = Some(new_id);
        match shared.consoles[&tab_id]
            .worker
            .send(SessionCommand::Execute {
                statement: DbStatement::for_dialect(dialect, statement),
                options: ExecOptions {
                    fetch_size: page_size,
                    max_rows: None,
                    ..ExecOptions::default()
                },
            }) {
            // The new result id, in `message` — `ResultProvider` has no
            // `executionStarted` signal of its own to announce it with
            // (this module's own doc comment on why not), so the grid
            // view reads it from here instead.
            // ponytail: a string-encoded id in an otherwise-human-message
            // field is a crutch; upgrade to a small `{code, message,
            // resultId}` struct if `ResultProvider` ever needs to return
            // one from more than this one call.
            Ok(()) => FfiResult {
                code: 0,
                message: QString::from(new_id.to_string().as_str()),
            },
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }
}

fn render_delimited(
    columns: &[ColumnMeta],
    rows: &[&Vec<Value>],
    rules: &db_core::value::FormatRules,
    sep: char,
) -> String {
    let mut out = String::new();
    out.push_str(
        &columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(&sep.to_string()),
    );
    for row in rows {
        out.push('\n');
        let cells: Vec<String> = row.iter().map(|v| v.display(rules)).collect();
        out.push_str(&cells.join(&sep.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_result(rows: Vec<Vec<Value>>) -> Shared {
        let columns = vec![
            ColumnMeta::new("id", "int4", false),
            ColumnMeta::new("note", "text", true),
        ];
        let mut set = ResultSet::new();
        let cap = db_core::result::MemoryCap::from_mib(64);
        set.append_batch(
            db_core::value::RowBatch {
                columns: columns.clone(),
                rows,
            },
            cap,
        )
        .unwrap();
        let mut shared = Shared::default();
        shared.results.insert(
            1,
            ResultState {
                tab_id: 1,
                columns,
                set,
                done: true,
                reported: true,
                cap_reached: false,
                affected: None,
                error: None,
                started_at: Instant::now(),
                elapsed_ms: Some(0),
                statement_text: "SELECT * FROM t".to_string(),
                edit: EditState::Unknown,
                table_constraints: Vec::new(),
            },
        );
        shared
    }

    #[test]
    fn row_values_preserves_the_real_value_not_display_text() {
        let shared = fake_result(vec![
            vec![Value::Int(1), Value::Null],
            vec![Value::Int(2), Value::Bytes(vec![0xde, 0xad])],
        ]);
        let encoded = walk_row_range(&shared, 1, 0, 10, |row, _flags| {
            db_core::value::row_to_json(row)
        });
        assert_eq!(encoded.len(), 2);
        let first = db_core::value::row_from_json(&encoded[0]).unwrap();
        assert_eq!(first, vec![Value::Int(1), Value::Null]);
        let second = db_core::value::row_from_json(&encoded[1]).unwrap();
        assert_eq!(second, vec![Value::Int(2), Value::Bytes(vec![0xde, 0xad])]);
    }

    #[test]
    fn walk_row_range_clamps_to_the_available_rows() {
        let shared = fake_result(vec![vec![Value::Int(1), Value::Null]]);
        let out = walk_row_range(&shared, 1, 0, 100, |row, _flags| row.len());
        assert_eq!(out, vec![2]);
        let empty = walk_row_range(&shared, 42, 0, 10, |_row, _flags| 0);
        assert!(empty.is_empty());
    }
}
