//! F4.3's exact whole-result aggregate and forward FK navigation — split
//! out of `console.rs` once that module hit the file-size ceiling
//! (`scripts/check-file-size.sh`), the same reason `edit.rs` split out of
//! it earlier. Reads/mutates `console::Shared`/`ConsoleState`/
//! `ResultState` directly, same visibility rule `edit.rs`'s own doc
//! comment already states for the same reason.

use std::pin::Pin;
use std::time::Instant;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use db_core::dialect::Dialect;
use db_core::driver::{ExecOptions, Statement as DbStatement};
use db_core::result::ResultSet;
use db_core::value::{ColumnMeta, Value};

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};

use super::console::ResultState;
use super::edit::EditState;
use super::sessions::SessionCommand;

/// F4.3's exact-aggregate query: `SELECT op(col), COUNT(*) FROM (stmt) t`
/// — the second projection is always `COUNT(*)`, the footer's "computed
/// over all N rows" text.
fn aggregate_query(statement: &str, column: &str, op: ffi::FfiDbAggOp, dialect: Dialect) -> String {
    let quoted = dialect.quote_ident(column);
    let expr = match op {
        ffi::FfiDbAggOp::Sum => format!("SUM({quoted})"),
        ffi::FfiDbAggOp::Avg => format!("AVG({quoted})"),
        ffi::FfiDbAggOp::Min => format!("MIN({quoted})"),
        ffi::FfiDbAggOp::Max => format!("MAX({quoted})"),
        _ => format!("COUNT({quoted})"),
    };
    format!("SELECT {expr}, COUNT(*) FROM ({statement}) t")
}

/// Runs `sql` (already a complete, parameter-free statement — F4.3's
/// aggregate query, or a FK navigation target's own bound `SELECT`) to
/// its first row and reads the first one or two columns back — the
/// shared "run one scalar-ish query over its own connection" step both
/// [`run_aggregate`] and a future single-row lookup would need.
fn first_row(
    connection: &mut dyn db_core::driver::Connection,
    sql: &str,
) -> Result<Vec<Value>, String> {
    use db_core::driver::Execution;
    let execution = connection
        .execute(
            &DbStatement::sql(sql.to_string()),
            &ExecOptions {
                fetch_size: 1,
                max_rows: Some(1),
                ..ExecOptions::default()
            },
        )
        .map_err(|error| error.to_string())?;
    match execution {
        Execution::Rows(mut stream) => {
            let batch = stream
                .next_batch()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "this query returned no rows".to_string())?;
            batch
                .rows
                .into_iter()
                .next()
                .ok_or_else(|| "this query returned no rows".to_string())
        }
        _ => Err("this is not a query".to_string()),
    }
}

/// F4d's own "can this statement be wrapped as a derived table at all"
/// decision — `database-tools.md` §11's F4c debt row: `run_aggregate`
/// used to just report whatever error the wrapped query hit rather than
/// telling "not wrappable" apart from any other failure. A dialect with
/// no derived-table concept (Mongo/Redis — `query_lang() != Sql`) is
/// never wrappable regardless of statement text; a SQL dialect's own
/// statement is wrappable only when it is (whitespace/comments aside) a
/// plain `SELECT` — anything else (a script's non-`SELECT` last
/// statement, several statements joined, an already-non-`SELECT`
/// original) is refused *before* ever issuing a query, so a caller can
/// fall back to `ResultProvider::aggregate`'s fetched-rows estimate
/// instead of showing this as a query error.
fn aggregate_scope(statement_text: &str, dialect: Dialect) -> Result<(), &'static str> {
    if dialect.query_lang() != db_core::driver::QueryLang::Sql {
        return Err("this data source has no SELECT-shaped derived table to wrap");
    }
    let trimmed = statement_text.trim_start();
    let starts_with_select = trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("select");
    if !starts_with_select {
        return Err("this statement is not a plain SELECT");
    }
    Ok(())
}

/// [`ffi::ConsoleService::aggregate_exact`]'s own connect-run-close step,
/// split out so it never touches `ffi`/cxx-qt types (a plain `Result` a
/// unit test can drive against an in-memory SQLite table, the same split
/// `sql_script.rs`'s `run_statements_on` already uses for the same
/// reason).
fn run_aggregate(
    source_id: &str,
    statement_text: &str,
    column: &str,
    op: ffi::FfiDbAggOp,
    dialect: Dialect,
) -> Result<(String, u64), String> {
    let setting = super::service::configured_sources()
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("no data source with id '{source_id}' is configured"))?;
    let data_source = db_core::datasource::DataSource::from(&setting);
    let spec = db_core::datasource::ConnectSpec::from(
        &data_source,
        &super::service::secrets_for(source_id),
    );
    let mut connection = super::connect(&spec)?;
    let sql = aggregate_query(statement_text, column, op, dialect);
    let outcome = first_row(connection.as_mut(), &sql);
    let _ = connection.close();
    let row = outcome?;
    let value = row
        .first()
        .cloned()
        .unwrap_or(Value::Null)
        .display(&Default::default());
    let count = match row.get(1) {
        Some(Value::Int(n)) => *n as u64,
        _ => 0,
    };
    Ok((value, count))
}

/// `dialect.quote_ident` on every named segment of `object`, `.`-joined —
/// `db_core::ddl`'s own private `qualify` does the identical thing but is
/// not exported; small enough to keep a second copy here rather than
/// widen that module's own API for one caller outside it.
fn qualify_object_ref(dialect: Dialect, object: &db_core::schema::ObjectRef) -> String {
    let mut parts = Vec::new();
    if let Some(catalog) = &object.catalog {
        parts.push(dialect.quote_ident(catalog));
    }
    if let Some(schema) = &object.schema {
        parts.push(dialect.quote_ident(schema));
    }
    parts.push(dialect.quote_ident(&object.name));
    parts.join(".")
}

/// F4.3's FK navigation, forward direction: every target `column`'s cell
/// (its row's own values already in hand as `row_values`) offers —
/// `table_constraints`'s `ForeignKey` entries whose own `columns` include
/// `column`. Composite FKs bind every column in the constraint, not just
/// the one the cell belongs to, so the target statement is a correct
/// single-row lookup rather than an under-constrained one.
fn nav_targets_for_cell(
    table_constraints: &[db_core::schema::ConstraintKind],
    result_columns: &[ColumnMeta],
    row_values: &[Value],
    column: &str,
    dialect: Dialect,
) -> Vec<ffi::FfiDbNavTarget> {
    let mut targets = Vec::new();
    for constraint in table_constraints {
        let db_core::schema::ConstraintKind::ForeignKey {
            columns,
            ref_table,
            ref_columns,
            ..
        } = constraint
        else {
            continue;
        };
        if !columns.iter().any(|c| c == column) || columns.len() != ref_columns.len() {
            continue;
        }
        let mut conditions = Vec::new();
        let mut resolvable = true;
        for (fk_column, ref_column) in columns.iter().zip(ref_columns.iter()) {
            let Some(index) = result_columns.iter().position(|c| &c.name == fk_column) else {
                resolvable = false;
                break;
            };
            let Some(value) = row_values.get(index) else {
                resolvable = false;
                break;
            };
            if *value == Value::Null {
                // A NULL FK column names no row at all — nothing to
                // navigate to.
                resolvable = false;
                break;
            }
            conditions.push(format!(
                "{} = {}",
                dialect.quote_ident(ref_column),
                value.sql_literal(dialect)
            ));
        }
        if !resolvable || conditions.is_empty() {
            continue;
        }
        let table_ref = qualify_object_ref(dialect, ref_table);
        targets.push(ffi::FfiDbNavTarget {
            label: QString::from(format!("Go to {}", ref_table.name).as_str()),
            statement: QString::from(
                format!(
                    "SELECT * FROM {table_ref} WHERE {}",
                    conditions.join(" AND ")
                )
                .as_str(),
            ),
        });
    }
    targets
}

impl ffi::ConsoleService {
    /// F4.3's exact aggregate over `result_id`'s whole statement (not just
    /// the rows fetched so far, `ResultProvider::aggregate`'s own
    /// ponytail note) — runs on its own short-lived connection
    /// (`run_aggregate`'s own doc comment on why not the console's shared
    /// worker: an aggregate must not steal the console's one in-flight
    /// slot from whatever page the grid is still browsing). The outcome
    /// arrives through `aggregateComputed`.
    pub fn aggregate_exact(
        self: Pin<&mut Self>,
        result_id: u64,
        column: &QString,
        op: ffi::FfiDbAggOp,
    ) -> FfiResult {
        let column = column.to_string();
        let (source_id, statement_text, dialect) = {
            let shared = self.shared.borrow();
            let Some(result) = shared.results.get(&result_id) else {
                return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
            };
            if !result.columns.iter().any(|c| c.name == column) {
                return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such column");
            }
            let Some(console) = shared.consoles.get(&result.tab_id) else {
                return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "console detached");
            };
            (
                console.source_id.clone(),
                result.statement_text.clone(),
                console.dialect,
            )
        };
        // F4d: refuse before ever opening a connection when the
        // statement/dialect cannot be wrapped at all — the caller falls
        // back to `ResultProvider::aggregate`'s fetched-rows estimate
        // rather than waiting on a query this would only fail anyway.
        if let Err(reason) = aggregate_scope(&statement_text, dialect) {
            let mut this = self;
            this.as_mut().aggregate_computed(
                result_id,
                op,
                ffi::FfiDbAggregateOutcome {
                    ok: false,
                    value: QString::default(),
                    row_count: 0,
                    scope: ffi::FfiDbAggScope::FetchedRowsOnly,
                    reason: QString::from(reason),
                },
            );
            return FfiResult::default();
        }
        let mut this = self;
        let qt_thread = this.as_mut().qt_thread();
        let column_for_thread = column.clone();
        std::thread::spawn(move || {
            let outcome =
                run_aggregate(&source_id, &statement_text, &column_for_thread, op, dialect);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ConsoleService>| {
                let payload = match outcome {
                    Ok((value, count)) => ffi::FfiDbAggregateOutcome {
                        ok: true,
                        value: QString::from(value.as_str()),
                        row_count: count,
                        scope: ffi::FfiDbAggScope::Exact,
                        reason: QString::default(),
                    },
                    Err(message) => ffi::FfiDbAggregateOutcome {
                        ok: false,
                        value: QString::from(message.as_str()),
                        row_count: 0,
                        scope: ffi::FfiDbAggScope::Exact,
                        reason: QString::default(),
                    },
                };
                service.as_mut().aggregate_computed(result_id, op, payload);
            });
        });
        FfiResult::default()
    }

    /// F4.3's FK navigation, forward direction — see `FfiDbNavTarget`'s
    /// own doc comment (`ffi.rs`) on why it carries a complete statement
    /// rather than a table/column pair.
    pub fn cell_navigation(
        self: Pin<&mut Self>,
        result_id: u64,
        row: u64,
        column: &QString,
    ) -> Vec<ffi::FfiDbNavTarget> {
        let column = column.to_string();
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return Vec::new();
        };
        let Some(console) = shared.consoles.get(&result.tab_id) else {
            return Vec::new();
        };
        let row_values = result
            .set
            .batches()
            .iter()
            .flat_map(|batch| batch.rows.iter())
            .nth(row as usize);
        let Some(row_values) = row_values else {
            return Vec::new();
        };
        nav_targets_for_cell(
            &result.table_constraints,
            &result.columns,
            row_values,
            &column,
            console.dialect,
        )
    }

    /// Runs `target`'s own already-bound statement as a fresh result on
    /// `result_id`'s console — same shape as `apply_clauses`.
    pub fn go_to_nav_target(
        self: Pin<&mut Self>,
        result_id: u64,
        target: &ffi::FfiDbNavTarget,
    ) -> FfiResult {
        let statement = target.statement.to_string();
        let mut shared = self.shared.borrow_mut();
        let Some(existing) = shared.results.get(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let tab_id = existing.tab_id;
        let Some(console) = shared.consoles.get(&tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "console detached");
        };
        if let Err(error) = console.guard.check(&statement) {
            return errors::failure(errors::CODE_REFUSED, error.message);
        }
        let page_size = console.page_size;
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
                statement: DbStatement::sql(statement),
                options: ExecOptions {
                    fetch_size: page_size,
                    max_rows: None,
                    ..ExecOptions::default()
                },
            }) {
            Ok(()) => FfiResult {
                code: 0,
                message: QString::from(new_id.to_string().as_str()),
            },
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }
}

/// Reverse FK navigation's own connect-introspect-query step (F4d) — a
/// fresh, short-lived connection (same "never steal the console's shared
/// worker slot" reasoning `run_aggregate`'s own doc comment gives), a
/// whole-schema `Full`-level introspect (`IntrospectScope::default()`,
/// every catalog/schema this source has), then `db_core::schema::
/// find_referencing_columns` over the snapshot's roots and one `COUNT(*)`
/// per hit to build the label's row count.
/// ponytail: no cache — every call re-introspects the whole schema from
/// scratch, unlike the brief's "cached in the worker" ideal; a whole-
/// schema `Full` introspect is the same cost `edit.rs`'s own per-table one
/// already pays per candidate result, just wider, and this crate has no
/// existing per-source cache slot outside the worker's own (private to
/// `sessions.rs`) that this call could reuse without a larger
/// restructuring. Upgrade if a real user's schema is large enough to make
/// re-running this on every "Show referencing rows…" click noticeable.
fn run_referencing_targets(
    source_id: &str,
    table: &str,
    column: &str,
    dialect: Dialect,
    value: &Value,
) -> Result<Vec<ffi::FfiDbNavTarget>, String> {
    let setting = super::service::configured_sources()
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("no data source with id '{source_id}' is configured"))?;
    let data_source = db_core::datasource::DataSource::from(&setting);
    let spec = db_core::datasource::ConnectSpec::from(
        &data_source,
        &super::service::secrets_for(source_id),
    );
    let mut connection = super::connect(&spec)?;
    let snapshot = connection
        .introspect(
            &db_core::schema::IntrospectScope::default(),
            db_core::schema::IntrospectLevel::Full,
        )
        .map_err(|error| error.to_string());
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = connection.close();
            return Err(error);
        }
    };
    let hits = db_core::schema::find_referencing_columns(&snapshot.roots, table, column);
    let mut targets = Vec::with_capacity(hits.len());
    for hit in &hits {
        let table_ref = qualify_object_ref(dialect, &hit.table);
        let condition = format!(
            "{} = {}",
            dialect.quote_ident(&hit.column),
            value.sql_literal(dialect)
        );
        let count_sql = format!("SELECT COUNT(*) FROM {table_ref} WHERE {condition}");
        let count = match first_row(connection.as_mut(), &count_sql) {
            Ok(row) => match row.first() {
                Some(Value::Int(n)) => Some(*n),
                _ => None,
            },
            Err(_) => None,
        };
        let label = match count {
            Some(n) => format!("{n} rows in {}.{}", hit.table.name, hit.column),
            None => format!("Rows in {}.{}", hit.table.name, hit.column),
        };
        targets.push(ffi::FfiDbNavTarget {
            label: QString::from(label.as_str()),
            statement: QString::from(
                format!("SELECT * FROM {table_ref} WHERE {condition}").as_str(),
            ),
        });
    }
    let _ = connection.close();
    Ok(targets)
}

impl ffi::ConsoleService {
    /// See `ffi.rs`'s own doc comment on `referencingTargets` — reverse FK
    /// navigation (F4d).
    pub fn referencing_targets(
        self: Pin<&mut Self>,
        result_id: u64,
        row: u64,
        column: &QString,
    ) -> FfiResult {
        let column = column.to_string();
        let (source_id, dialect, table, value) = {
            let shared = self.shared.borrow();
            let Some(result) = shared.results.get(&result_id) else {
                return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
            };
            let Some(console) = shared.consoles.get(&result.tab_id) else {
                return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "console detached");
            };
            let Some(table) = db_sql::single_table::of(&result.statement_text, console.dialect)
            else {
                return errors::failure(
                    errors::CODE_INVALID_ARGUMENT,
                    "this result is not a single table",
                );
            };
            let Some(column_index) = result.columns.iter().position(|c| c.name == column) else {
                return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such column");
            };
            let Some(row_values) = result
                .set
                .batches()
                .iter()
                .flat_map(|batch| batch.rows.iter())
                .nth(row as usize)
            else {
                return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such row");
            };
            let Some(value) = row_values.get(column_index).cloned() else {
                return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such column");
            };
            (
                console.source_id.clone(),
                console.dialect,
                table.name,
                value,
            )
        };
        let mut this = self;
        let qt_thread = this.as_mut().qt_thread();
        let column_for_thread = column.clone();
        std::thread::spawn(move || {
            let outcome =
                run_referencing_targets(&source_id, &table, &column_for_thread, dialect, &value);
            let column_for_signal = column_for_thread.clone();
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ConsoleService>| {
                let targets = outcome.unwrap_or_default();
                service.as_mut().referencing_targets_ready(
                    result_id,
                    QString::from(column_for_signal.as_str()),
                    targets,
                );
            });
        });
        FfiResult::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_query_wraps_the_statement_and_always_counts_rows() {
        let sql = aggregate_query(
            "SELECT * FROM t",
            "amount",
            ffi::FfiDbAggOp::Sum,
            Dialect::Sqlite,
        );
        assert_eq!(
            sql,
            "SELECT SUM(\"amount\"), COUNT(*) FROM (SELECT * FROM t) t"
        );
    }

    #[test]
    fn aggregate_query_count_ignores_the_column_and_still_counts_rows() {
        let sql = aggregate_query(
            "SELECT * FROM t",
            "id",
            ffi::FfiDbAggOp::Count,
            Dialect::Sqlite,
        );
        assert_eq!(
            sql,
            "SELECT COUNT(\"id\"), COUNT(*) FROM (SELECT * FROM t) t"
        );
    }

    fn fk_constraint(
        column: &str,
        ref_table: &str,
        ref_column: &str,
    ) -> db_core::schema::ConstraintKind {
        db_core::schema::ConstraintKind::ForeignKey {
            columns: vec![column.to_string()],
            ref_table: db_core::schema::ObjectRef::new(ref_table),
            ref_columns: vec![ref_column.to_string()],
            on_delete: None,
            on_update: None,
        }
    }

    #[test]
    fn nav_targets_for_cell_binds_the_row_s_own_value() {
        let constraints = vec![fk_constraint("customer_id", "customers", "id")];
        let columns = vec![
            ColumnMeta::new("id", "int4", false),
            ColumnMeta::new("customer_id", "int4", true),
        ];
        let row = vec![Value::Int(1), Value::Int(42)];
        let targets = nav_targets_for_cell(
            &constraints,
            &columns,
            &row,
            "customer_id",
            Dialect::Postgres,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].statement.to_string(),
            "SELECT * FROM \"customers\" WHERE \"id\" = 42"
        );
        assert_eq!(targets[0].label.to_string(), "Go to customers");
    }

    #[test]
    fn nav_targets_for_cell_is_empty_for_a_non_fk_column() {
        let constraints = vec![fk_constraint("customer_id", "customers", "id")];
        let columns = vec![ColumnMeta::new("note", "text", true)];
        let row = vec![Value::Text("hi".to_string())];
        let targets = nav_targets_for_cell(&constraints, &columns, &row, "note", Dialect::Postgres);
        assert!(targets.is_empty());
    }

    #[test]
    fn nav_targets_for_cell_skips_a_null_fk_value() {
        let constraints = vec![fk_constraint("customer_id", "customers", "id")];
        let columns = vec![ColumnMeta::new("customer_id", "int4", true)];
        let row = vec![Value::Null];
        let targets = nav_targets_for_cell(
            &constraints,
            &columns,
            &row,
            "customer_id",
            Dialect::Postgres,
        );
        assert!(targets.is_empty());
    }
}
