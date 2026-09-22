//! The data editor (database-tools-plan F4.1/F4.2): editability decisions,
//! `ConsoleService::dmlPreview`/`submit`, and `ResultProvider`'s staged-
//! edit surface — split out of `console.rs` once F4 pushed that module
//! past the file-size ceiling (`scripts/check-file-size.sh`).
//!
//! Reads/mutates `console::Shared`/`ConsoleState`/`ResultState` directly
//! (all `pub(crate)`, see `ConsoleState`'s own doc comment in `console.rs`)
//! rather than through an accessor layer — this module is `console.rs`'s
//! own extension, not an independent consumer of its state.

use std::pin::Pin;
use std::rc::Rc;
use std::time::Instant;

use cxx_qt_lib::QString;

use db_core::dialect::Dialect;
use db_core::dml::EditBuffer;
use db_core::driver::{ExecOptions, Statement as DbStatement};
use db_core::error::DbError;
use db_core::readonly::Guard;
use db_core::result::ResultSet;
use db_core::value::{ColumnMeta, Value};
use db_sql::classify::SqlClassifier;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};

use super::console::{IntrospectPurpose, ResultState};
use super::sessions::SessionCommand;

/// The data editor's editability decision for one result (F4.1).
pub(crate) enum EditState {
    /// Not yet decided — no columns yet, or a `Full`-level introspect is
    /// still in flight (`ConsoleState::pending_introspects`).
    Unknown,
    /// Read-only, with the reason the grid shows.
    NotEditable(String),
    Editable(EditBuffer),
}

// ---- ConsoleService: dmlPreview / submit / editability ----

impl ffi::ConsoleService {
    /// Renders `result_id`'s currently-staged edits as SQL text (bound
    /// params shown as `?`, `db_core::dml::DmlPlan::preview_text`'s own
    /// doc comment) and opens it as a read-only `db-dml` virtual document
    /// — the same "Go to DDL" scheme, so re-previewing the same result
    /// focuses the same tab rather than opening a duplicate.
    pub fn dml_preview(mut self: Pin<&mut Self>, result_id: u64) -> FfiResult {
        let (buffer_empty, preview) = {
            let shared = self.shared.borrow();
            let Some(result) = shared.results.get(&result_id) else {
                return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
            };
            let EditState::Editable(buffer) = &result.edit else {
                return errors::failure(errors::CODE_REFUSED, "this result is not editable");
            };
            let dialect = shared
                .consoles
                .get(&result.tab_id)
                .map(|c| c.dialect)
                .unwrap_or(Dialect::Sqlite);
            (
                buffer.is_empty(),
                buffer.to_dml_plan(dialect).preview_text(),
            )
        };
        if buffer_empty {
            return errors::failure(
                errors::CODE_REFUSED,
                "there are no pending changes to preview",
            );
        }
        let key = format!("result-{result_id}.sql");
        let opened = self
            .session
            .borrow_mut()
            .open_virtual_document("db-dml", &key, &preview);
        self.as_mut().virtual_document_opened(
            opened.id.raw(),
            QString::from(opened.title.as_str()),
            opened.newly_opened,
        );
        FfiResult::default()
    }

    /// Runs `result_id`'s staged edits (F4.2): compiles them to a
    /// `DmlPlan` and sends it to the console's own worker as a
    /// [`SessionCommand::Apply`] — the outcome (and the buffer clear/page
    /// refresh it triggers) arrives asynchronously through
    /// [`apply_submit_outcome`], reported through
    /// [`ffi::ConsoleService::submit_finished`].
    pub fn submit(self: Pin<&mut Self>, result_id: u64) -> FfiResult {
        let (tab_id, statements) = {
            let shared = self.shared.borrow();
            let Some(result) = shared.results.get(&result_id) else {
                return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
            };
            let EditState::Editable(buffer) = &result.edit else {
                return errors::failure(errors::CODE_REFUSED, "this result is not editable");
            };
            if buffer.is_empty() {
                return errors::failure(
                    errors::CODE_REFUSED,
                    "there are no pending changes to submit",
                );
            }
            let dialect = shared
                .consoles
                .get(&result.tab_id)
                .map(|c| c.dialect)
                .unwrap_or(Dialect::Sqlite);
            (result.tab_id, buffer.to_dml_plan(dialect).statements)
        };
        let mut shared = self.shared.borrow_mut();
        let Some(console) = shared.consoles.get_mut(&tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "no such console");
        };
        if console.submitting.is_some() {
            return errors::failure(errors::CODE_REFUSED, "a submit is already in progress");
        }
        // The same client-side read-only check every other console-run
        // path already runs before dispatch (`begin_run`/`apply_clauses`
        // in `console.rs`) — a `security-expert` F4.5 finding: `submit`
        // was the one path that compiled straight to `SessionCommand::
        // Apply` without ever consulting `console.guard`, so a source
        // whose read-only flag flips after a grid is already open (its
        // editability decision is cached in `ResultState::edit` and
        // never re-checked) had nothing left stopping the write.
        for statement in &statements {
            if let Err(error) = console.guard.check(&statement.text) {
                return errors::failure(errors::CODE_REFUSED, error.message);
            }
        }
        match console.worker.send(SessionCommand::Apply { statements }) {
            Ok(()) => {
                console.submitting = Some(result_id);
                FfiResult::default()
            }
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }

    /// Decides `result_id`'s editability (F4.1) the first time its
    /// columns are known — a no-op on every later batch (checked via
    /// `EditState::Unknown`, and this is only ever called from a `Rows`
    /// batch's own handler). A read-only source or a statement
    /// `db_sql::single_table::of` cannot resolve settles immediately;
    /// otherwise this dispatches a `Full`-level introspect scoped to that
    /// one table and leaves the decision `Unknown` until
    /// [`apply_edit_lookup`] resolves it.
    pub(crate) fn start_editability_check(mut self: Pin<&mut Self>, tab_id: u64, result_id: u64) {
        let dispatch = {
            let mut shared = self.shared.borrow_mut();
            let Some(result) = shared.results.get(&result_id) else {
                return;
            };
            if !matches!(result.edit, EditState::Unknown) || result.columns.is_empty() {
                return;
            }
            let statement_text = result.statement_text.clone();
            let Some(console) = shared.consoles.get(&tab_id) else {
                return;
            };
            let source_id = console.source_id.clone();
            let dialect = console.dialect;
            let read_only = super::service::configured_sources()
                .into_iter()
                .find(|s| s.id == source_id)
                .map(|s| s.read_only)
                .unwrap_or(false);
            if read_only {
                shared.results.get_mut(&result_id).unwrap().edit =
                    EditState::NotEditable("this data source is read-only".to_string());
                None
            } else {
                match db_sql::single_table::of(&statement_text, dialect) {
                    Some(table) => {
                        let scope = db_core::schema::IntrospectScope {
                            schema: table.schema.clone(),
                            object: Some(table.name.clone()),
                            ..Default::default()
                        };
                        let console = shared.consoles.get_mut(&tab_id).unwrap();
                        let sent = console.worker.send(SessionCommand::Introspect {
                            scope,
                            level: db_core::schema::IntrospectLevel::Full,
                            force: false,
                        });
                        // Only queued once the dispatch actually succeeded
                        // — a failed `send` produces no reply, so nothing
                        // must ever try to pop this from the front of
                        // `pending_introspects` later (`apply_event`'s own
                        // doc comment on why that queue must stay exactly
                        // in dispatch order).
                        if sent.is_ok() {
                            console
                                .pending_introspects
                                .push_back(IntrospectPurpose::EditLookup(result_id, table));
                        }
                        Some(sent)
                    }
                    None => {
                        shared.results.get_mut(&result_id).unwrap().edit = EditState::NotEditable(
                            "this result does not map onto a single table".to_string(),
                        );
                        None
                    }
                }
            }
        };
        match dispatch {
            None => {
                let (editable, reason) = {
                    let shared = self.shared.borrow();
                    match shared.results.get(&result_id).map(|r| &r.edit) {
                        Some(EditState::NotEditable(reason)) => (false, reason.clone()),
                        _ => return,
                    }
                };
                self.as_mut().editability_changed(
                    result_id,
                    editable,
                    QString::from(reason.as_str()),
                );
            }
            Some(Err(error)) => {
                if let Some(result) = self.shared.borrow_mut().results.get_mut(&result_id) {
                    result.edit = EditState::NotEditable(error.to_string());
                }
                self.as_mut().editability_changed(
                    result_id,
                    false,
                    QString::from(error.to_string().as_str()),
                );
            }
            Some(Ok(())) => {}
        }
    }

    /// Re-runs `statement_text` as a fresh result on `tab_id`'s worker — a
    /// submit's own "refresh (re-run current page)" step
    /// (database-tools.md §4), the same "new id, same worker, same
    /// `apply_event` routing" shape `apply_clauses` (`console.rs`) already
    /// uses.
    pub(crate) fn rerun_for_refresh(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        statement_text: String,
    ) -> Option<u64> {
        let mut shared = self.shared.borrow_mut();
        let console = shared.consoles.get(&tab_id)?;
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
                statement_text: statement_text.clone(),
                edit: EditState::Unknown,
                table_constraints: Vec::new(),
            },
        );
        shared.consoles.get_mut(&tab_id).unwrap().current_result = Some(new_id);
        let sent = shared.consoles[&tab_id]
            .worker
            .send(SessionCommand::Execute {
                statement: DbStatement::sql(statement_text),
                options: ExecOptions {
                    fetch_size: page_size,
                    max_rows: None,
                    ..ExecOptions::default()
                },
            });
        drop(shared);
        if sent.is_ok() {
            self.as_mut().execution_started(tab_id, new_id, 1, 1);
            Some(new_id)
        } else {
            None
        }
    }
}

/// Resolves the `Full`-level introspect [`ffi::ConsoleService::
/// start_editability_check`] dispatched for `result_id`/`table` — the
/// caller (`apply_event`'s `Introspected` branch, `console.rs`) already
/// popped `console.pending_introspects`'s front entry to get here, so
/// this never needs to touch that queue itself.
pub(crate) fn apply_edit_lookup(
    mut service: Pin<&mut ffi::ConsoleService>,
    tab_id: u64,
    result_id: u64,
    table: db_sql::single_table::TableRef,
    result: Result<db_core::schema::SchemaSnapshot, DbError>,
) {
    let (no_pk_policy, columns) = {
        let shared = service.shared.borrow();
        let source_id = shared
            .consoles
            .get(&tab_id)
            .map(|c| c.source_id.clone())
            .unwrap_or_default();
        let policy = super::service::configured_sources()
            .into_iter()
            .find(|s| s.id == source_id)
            .map(|s| s.no_primary_key_policy_or_default().to_string())
            .unwrap_or_else(|| app_config::database::DEFAULT_NO_PRIMARY_KEY_POLICY.to_string());
        let columns = shared
            .results
            .get(&result_id)
            .map(|r| r.columns.clone())
            .unwrap_or_default();
        (policy, columns)
    };
    let constraints = match &result {
        Ok(snapshot) => table_constraints(snapshot, &table),
        Err(_) => Vec::new(),
    };
    let edit = match result {
        Ok(snapshot) => resolve_editability(&snapshot, &table, &columns, &no_pk_policy),
        Err(error) => EditState::NotEditable(error.to_string()),
    };
    let (editable, reason) = match &edit {
        EditState::Editable(_) => (true, String::new()),
        EditState::NotEditable(reason) => (false, reason.clone()),
        EditState::Unknown => (false, String::new()),
    };
    if let Some(r) = service.shared.borrow_mut().results.get_mut(&result_id) {
        r.edit = edit;
        r.table_constraints = constraints;
    }
    service
        .as_mut()
        .editability_changed(result_id, editable, QString::from(reason.as_str()));
}

/// Turns a `Full`-level snapshot scoped to `table` into an editability
/// decision: finds the matching table node, reads its columns' own
/// `detail.primary_key` (already populated by both real drivers'
/// `Full`-level introspection), keeps only the primary-key columns that
/// are actually present in `columns` (the result's own projection — a
/// key column the query never selected cannot be bound into a `WHERE`),
/// Every [`db_core::schema::ConstraintKind`] `table`'s own `Constraint`
/// children carry (F4.3's FK navigation) — the table node's own
/// `Constraint`-kind children hold one each (`db_core::schema`'s own
/// per-backend introspection, F6c), not the column children
/// `resolve_editability` reads `primary_key`/`nullable` detail from.
fn table_constraints(
    snapshot: &db_core::schema::SchemaSnapshot,
    table: &db_sql::single_table::TableRef,
) -> Vec<db_core::schema::ConstraintKind> {
    let Some(node) = snapshot.roots.iter().find(|n| n.name == table.name) else {
        return Vec::new();
    };
    let db_core::schema::Children::Loaded(children) = &node.children else {
        return Vec::new();
    };
    children
        .iter()
        .filter(|child| child.kind == db_core::schema::ObjectKind::Constraint)
        .filter_map(|child| child.detail.constraint.clone())
        .collect()
}

/// and applies `no_pk_policy` (`"refuse"` or `"all_columns_where"`, F4.2)
/// when none remain.
fn resolve_editability(
    snapshot: &db_core::schema::SchemaSnapshot,
    table: &db_sql::single_table::TableRef,
    columns: &[ColumnMeta],
    no_pk_policy: &str,
) -> EditState {
    let Some(node) = snapshot.roots.iter().find(|n| n.name == table.name) else {
        return EditState::NotEditable("could not determine this table's primary key".to_string());
    };
    let db_core::schema::Children::Loaded(children) = &node.children else {
        return EditState::NotEditable("could not determine this table's primary key".to_string());
    };
    let column_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let primary_key: Vec<String> = children
        .iter()
        .filter(|child| child.detail.primary_key && column_names.contains(&child.name))
        .map(|child| child.name.clone())
        .collect();
    let table_has_a_key_at_all = children.iter().any(|child| child.detail.primary_key);
    if primary_key.is_empty() && table_has_a_key_at_all {
        // The table has a primary key, but this result's own projection
        // did not select every one of its columns — cannot bind a
        // `WHERE` on a key this result never fetched.
        return EditState::NotEditable(
            "this result does not include every primary-key column".to_string(),
        );
    }
    if primary_key.is_empty() && no_pk_policy != "all_columns_where" {
        return EditState::NotEditable("this table has no primary key (refuse policy)".to_string());
    }
    EditState::Editable(EditBuffer::new(
        table.name.clone(),
        column_names,
        primary_key,
        Vec::new(),
    ))
}

/// [`super::sessions::SessionEvent::Applied`]'s handler (F4.2's submit):
/// clears the buffer, reports through `submitFinished`, and re-runs the
/// original statement as a fresh result on success so the grid shows the
/// now-persisted rows — `database-tools.md` §4's "refresh (re-run current
/// page)" step.
pub(crate) fn apply_submit_outcome(
    mut service: Pin<&mut ffi::ConsoleService>,
    tab_id: u64,
    result: Result<u64, DbError>,
) {
    let Some(result_id) = ({
        let mut shared = service.shared.borrow_mut();
        shared
            .consoles
            .get_mut(&tab_id)
            .and_then(|console| console.submitting.take())
    }) else {
        return;
    };
    match result {
        Ok(affected) => {
            let statement_text = {
                let mut shared = service.shared.borrow_mut();
                if let Some(r) = shared.results.get_mut(&result_id) {
                    if let EditState::Editable(buffer) = &mut r.edit {
                        buffer.clear();
                    }
                }
                shared
                    .results
                    .get(&result_id)
                    .map(|r| r.statement_text.clone())
            };
            service.as_mut().submit_finished(
                result_id,
                true,
                QString::from(format!("{affected} row(s) applied").as_str()),
            );
            if let Some(statement_text) = statement_text {
                let new_id = service.as_mut().rerun_for_refresh(tab_id, statement_text);
                if let Some(new_id) = new_id {
                    service.as_mut().result_refreshed(result_id, new_id);
                }
            }
        }
        Err(error) => {
            service.as_mut().submit_finished(
                result_id,
                false,
                QString::from(error.message.as_str()),
            );
        }
    }
}

// ---- ResultProvider: staged-edit surface (F4.1/F4.2) ----

impl ffi::ResultProvider {
    /// Whether `result_id` currently offers editing at all — a single-
    /// table result, on a writable source, with a usable primary key (or
    /// the `all_columns_where` `NoPrimaryKey` policy opted in) — see
    /// `EditState`'s own doc comment for the full decision. `false` while
    /// the decision is still `Unknown` (the grid re-checks once
    /// `ConsoleService::editabilityChanged` fires).
    pub fn is_editable(self: Pin<&mut Self>, result_id: u64) -> bool {
        let shared = self.shared.borrow();
        matches!(
            shared.results.get(&result_id).map(|r| &r.edit),
            Some(EditState::Editable(_))
        )
    }

    /// Why `result_id` is not editable, empty once it is (or while still
    /// unknown) — the grid's own read-only banner text.
    pub fn not_editable_reason(self: Pin<&mut Self>, result_id: u64) -> QString {
        let shared = self.shared.borrow();
        match shared.results.get(&result_id).map(|r| &r.edit) {
            Some(EditState::NotEditable(reason)) => QString::from(reason.as_str()),
            _ => QString::default(),
        }
    }

    /// How many rows currently have a pending edit/add/delete —
    /// `EditBuffer::pending_count`'s own doc comment.
    pub fn pending_count(self: Pin<&mut Self>, result_id: u64) -> u64 {
        let shared = self.shared.borrow();
        match shared.results.get(&result_id).map(|r| &r.edit) {
            Some(EditState::Editable(buffer)) => buffer.pending_count() as u64,
            _ => 0,
        }
    }

    /// Stages `row`/`column`'s new value, coerced from `text` through
    /// `db_core::value::parse_text` using that column's own declared
    /// type — the cell/value-editor's "coerce before bind" step (F4.1).
    /// `Err` carries the message the cell shows on a rejected coercion
    /// (an unparseable int, a malformed UUID, …); the buffer is
    /// untouched in that case.
    pub fn set_cell(
        self: Pin<&mut Self>,
        result_id: u64,
        row: u64,
        column: &QString,
        text: &QString,
    ) -> FfiResult {
        let column = column.to_string();
        let text = text.to_string();
        let mut shared = self.shared.borrow_mut();
        let Some(result) = shared.results.get_mut(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let Some(type_name) = result
            .columns
            .iter()
            .find(|c| c.name == column)
            .map(|c| c.type_name.clone())
        else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such column");
        };
        let rows = current_rows(result);
        let EditState::Editable(buffer) = &mut result.edit else {
            return errors::failure(errors::CODE_REFUSED, "this result is not editable");
        };
        buffer.sync_rows(rows);
        match db_core::value::parse_text(&text, &type_name) {
            Ok(value) => {
                buffer.stage(row as usize, &column, value);
                FfiResult::default()
            }
            Err(message) => errors::failure(errors::CODE_INVALID_ARGUMENT, message),
        }
    }

    /// Stages `row`/`column` as `NULL` — the value editor's NULL button.
    pub fn set_null(self: Pin<&mut Self>, result_id: u64, row: u64, column: &QString) -> FfiResult {
        let column = column.to_string();
        let mut shared = self.shared.borrow_mut();
        let Some(result) = shared.results.get_mut(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let rows = current_rows(result);
        let EditState::Editable(buffer) = &mut result.edit else {
            return errors::failure(errors::CODE_REFUSED, "this result is not editable");
        };
        buffer.sync_rows(rows);
        buffer.stage(row as usize, &column, Value::Null);
        FfiResult::default()
    }

    /// Stages `row`/`column` back to the column's own declared default —
    /// the value editor's DEFAULT button (`db_core::dml::CellEdit::
    /// Default`'s own doc comment on how this renders).
    pub fn set_default(
        self: Pin<&mut Self>,
        result_id: u64,
        row: u64,
        column: &QString,
    ) -> FfiResult {
        let column = column.to_string();
        let mut shared = self.shared.borrow_mut();
        let Some(result) = shared.results.get_mut(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let rows = current_rows(result);
        let EditState::Editable(buffer) = &mut result.edit else {
            return errors::failure(errors::CODE_REFUSED, "this result is not editable");
        };
        buffer.sync_rows(rows);
        buffer.stage_default(row as usize, &column);
        FfiResult::default()
    }

    /// Drops a single staged cell edit.
    pub fn revert_cell(
        self: Pin<&mut Self>,
        result_id: u64,
        row: u64,
        column: &QString,
    ) -> FfiResult {
        let column = column.to_string();
        let mut shared = self.shared.borrow_mut();
        let Some(result) = shared.results.get_mut(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let EditState::Editable(buffer) = &mut result.edit else {
            return errors::failure(errors::CODE_REFUSED, "this result is not editable");
        };
        buffer.revert_cell(row as usize, &column);
        FfiResult::default()
    }

    /// Stages a new row (Alt+Insert), every column defaulted from its own
    /// `ColumnMeta` (`NULL` when nullable, the type's zero value
    /// otherwise). The new row's grid index travels back in `message` —
    /// the same convention `applyClauses` (`console.rs`) already uses for
    /// a value this call alone produces.
    pub fn add_row(self: Pin<&mut Self>, result_id: u64) -> FfiResult {
        let mut shared = self.shared.borrow_mut();
        let Some(result) = shared.results.get_mut(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let rows = current_rows(result);
        let columns = result.columns.clone();
        let EditState::Editable(buffer) = &mut result.edit else {
            return errors::failure(errors::CODE_REFUSED, "this result is not editable");
        };
        buffer.sync_rows(rows);
        let values = columns
            .iter()
            .map(|c| default_cell_value(&c.type_name, c.nullable))
            .collect();
        let new_row = buffer.add_row(values);
        FfiResult {
            code: 0,
            message: QString::from(new_row.to_string().as_str()),
        }
    }

    /// Stages a copy of `row` as a new row — the new row's grid index
    /// travels back in `message`, same as `addRow`.
    pub fn clone_row(self: Pin<&mut Self>, result_id: u64, row: u64) -> FfiResult {
        let mut shared = self.shared.borrow_mut();
        let Some(result) = shared.results.get_mut(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let rows = current_rows(result);
        let EditState::Editable(buffer) = &mut result.edit else {
            return errors::failure(errors::CODE_REFUSED, "this result is not editable");
        };
        buffer.sync_rows(rows);
        match buffer.clone_row(row as usize) {
            Some(new_row) => FfiResult {
                code: 0,
                message: QString::from(new_row.to_string().as_str()),
            },
            None => errors::failure(errors::CODE_INVALID_ARGUMENT, "no such row"),
        }
    }

    /// Stages every row in `rows` for deletion (Ctrl+Delete) — processed
    /// highest index first, so deleting an *inserted* row (removed
    /// outright, `EditBuffer::delete_row`'s own doc comment) never shifts
    /// a still-to-process index out from under this call.
    pub fn delete_rows(self: Pin<&mut Self>, result_id: u64, rows: Vec<u64>) -> FfiResult {
        let mut shared = self.shared.borrow_mut();
        let Some(result) = shared.results.get_mut(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let current = current_rows(result);
        let EditState::Editable(buffer) = &mut result.edit else {
            return errors::failure(errors::CODE_REFUSED, "this result is not editable");
        };
        buffer.sync_rows(current);
        let mut sorted = rows;
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        sorted.dedup();
        for row in sorted {
            buffer.delete_row(row as usize);
        }
        FfiResult::default()
    }

    /// Discards every staged edit/add/delete for `result_id` (F4.2's
    /// "Revert").
    pub fn revert(self: Pin<&mut Self>, result_id: u64) {
        if let Some(result) = self.shared.borrow_mut().results.get_mut(&result_id) {
            if let EditState::Editable(buffer) = &mut result.edit {
                buffer.clear();
            }
        }
    }

    /// See `db_core::value::pretty_json`'s own doc comment.
    pub fn pretty_json(self: Pin<&mut Self>, text: &QString) -> FfiResult {
        match db_core::value::pretty_json(&text.to_string()) {
            Ok(pretty) => FfiResult {
                code: 0,
                message: QString::from(pretty.as_str()),
            },
            Err(message) => errors::failure(errors::CODE_INVALID_ARGUMENT, message),
        }
    }
}

/// The F4a residual (database-tools-plan's "F4a follow-up" row,
/// database-tools.md §11): a data source's `read_only` setting can flip
/// after a console is already attached and a result already decided
/// editable — `ConsoleState::guard` and `ResultState::edit` are both
/// decided once and never re-checked on their own. `DataSourceEditor::
/// commit` (`settings.rs`) calls this for every source whose `read_only`
/// flag it just changed.
///
/// Every open console on `source_id` gets a fresh [`Guard`] built from
/// `read_only` — closing the write-through gap immediately, the
/// security-critical direction. Every already-decided [`EditState`] on
/// those consoles' results is invalidated too: turning read-only *on*
/// demotes an `Editable` result straight to `NotEditable` (no live
/// `ConsoleService` pin reaches this free function to re-emit
/// `editabilityChanged`, so the grid's next poll of `isEditable`/
/// `notEditableReason` is what actually reflects it — both already read
/// `EditState` fresh on every call, never a value cached on the C++
/// side); turning it *off* only resets a read-only-caused `NotEditable`
/// back to `Unknown` rather than eagerly re-deciding `Editable` — the
/// `Full`-level introspect that decision needs runs from a live
/// `ConsoleService`, so this leaves it to the next execute/refresh on
/// that console, same as opening the result the first time.
pub(crate) fn rebuild_guards_for_source(
    shared: &Rc<std::cell::RefCell<super::console::Shared>>,
    source_id: &str,
    read_only: bool,
) {
    let mut shared = shared.borrow_mut();
    let affected_tabs: Vec<u64> = shared
        .consoles
        .iter()
        .filter(|(_, console)| console.source_id == source_id)
        .map(|(tab_id, _)| *tab_id)
        .collect();
    for tab_id in &affected_tabs {
        if let Some(console) = shared.consoles.get_mut(tab_id) {
            console.guard = Guard::new(
                read_only,
                Box::new(SqlClassifier {
                    dialect: console.dialect,
                }),
            );
        }
    }
    for result in shared.results.values_mut() {
        if !affected_tabs.contains(&result.tab_id) {
            continue;
        }
        let current = std::mem::replace(&mut result.edit, EditState::Unknown);
        result.edit = match current {
            EditState::Editable(_) if read_only => {
                EditState::NotEditable("this data source is now read-only".to_string())
            }
            EditState::NotEditable(reason) if !read_only && reason.contains("read-only") => {
                EditState::Unknown
            }
            other => other,
        };
    }
}

/// Every row currently fetched for `result`, flattened in batch order —
/// what `EditBuffer::sync_rows` needs before a mutating call, so a row
/// paged in after editability was first decided is never out of the
/// buffer's own reach (`EditBuffer::sync_rows`'s own doc comment).
fn current_rows(result: &ResultState) -> Vec<Vec<Value>> {
    result
        .set
        .batches()
        .iter()
        .flat_map(|batch| batch.rows.iter().cloned())
        .collect()
}

/// A new row's own starting value for a column this codebase has no user
/// input for yet — `NULL` when the column allows it, otherwise the
/// type's zero value, matched the same loose, case-insensitive way
/// `db_core::value::parse_text` matches a type name.
fn default_cell_value(type_name: &str, nullable: bool) -> Value {
    if nullable {
        return Value::Null;
    }
    let type_name = type_name.to_ascii_lowercase();
    if type_name.contains("int") {
        Value::Int(0)
    } else if type_name.contains("float")
        || type_name.contains("double")
        || type_name.contains("real")
    {
        Value::Float(0.0)
    } else if type_name.contains("bool") {
        Value::Bool(false)
    } else if type_name.contains("numeric") || type_name.contains("decimal") {
        Value::Decimal("0".to_string())
    } else {
        Value::Text(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users_columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta::new("id", "int4", false),
            ColumnMeta::new("name", "text", true),
        ]
    }

    fn table_snapshot(
        pk_columns: &[&str],
        all_columns: &[&str],
    ) -> db_core::schema::SchemaSnapshot {
        let children: Vec<db_core::schema::Node> = all_columns
            .iter()
            .map(|name| {
                db_core::schema::Node::leaf(*name, db_core::schema::ObjectKind::Column).with_detail(
                    db_core::schema::NodeDetail {
                        primary_key: pk_columns.contains(name),
                        ..Default::default()
                    },
                )
            })
            .collect();
        db_core::schema::SchemaSnapshot::new(
            db_core::schema::IntrospectLevel::Full,
            vec![db_core::schema::Node::with_children(
                "users",
                db_core::schema::ObjectKind::Table,
                children,
            )],
        )
    }

    #[test]
    fn table_constraints_reads_the_table_s_own_constraint_children() {
        let fk = db_core::schema::ConstraintKind::ForeignKey {
            columns: vec!["customer_id".to_string()],
            ref_table: db_core::schema::ObjectRef::new("customers"),
            ref_columns: vec!["id".to_string()],
            on_delete: None,
            on_update: None,
        };
        let snapshot = db_core::schema::SchemaSnapshot::new(
            db_core::schema::IntrospectLevel::Full,
            vec![db_core::schema::Node::with_children(
                "users",
                db_core::schema::ObjectKind::Table,
                vec![
                    db_core::schema::Node::leaf("id", db_core::schema::ObjectKind::Column),
                    db_core::schema::Node::leaf(
                        "users_fk_0",
                        db_core::schema::ObjectKind::Constraint,
                    )
                    .with_detail(db_core::schema::NodeDetail {
                        constraint: Some(fk.clone()),
                        ..Default::default()
                    }),
                ],
            )],
        );
        let constraints = table_constraints(&snapshot, &table_ref());
        assert_eq!(constraints, vec![fk]);
    }

    #[test]
    fn table_constraints_is_empty_for_a_missing_table() {
        let snapshot = db_core::schema::SchemaSnapshot::new(
            db_core::schema::IntrospectLevel::Full,
            vec![db_core::schema::Node::leaf(
                "other",
                db_core::schema::ObjectKind::Table,
            )],
        );
        assert!(table_constraints(&snapshot, &table_ref()).is_empty());
    }

    fn table_ref() -> db_sql::single_table::TableRef {
        db_sql::single_table::TableRef {
            schema: None,
            name: "users".to_string(),
        }
    }

    #[test]
    fn a_table_with_its_primary_key_selected_is_editable() {
        let snapshot = table_snapshot(&["id"], &["id", "name"]);
        let edit = resolve_editability(&snapshot, &table_ref(), &users_columns(), "refuse");
        assert!(matches!(edit, EditState::Editable(_)));
    }

    #[test]
    fn a_missing_table_node_is_not_editable() {
        let snapshot = db_core::schema::SchemaSnapshot::new(
            db_core::schema::IntrospectLevel::Full,
            vec![db_core::schema::Node::leaf(
                "other",
                db_core::schema::ObjectKind::Table,
            )],
        );
        let edit = resolve_editability(&snapshot, &table_ref(), &users_columns(), "refuse");
        assert!(matches!(edit, EditState::NotEditable(_)));
    }

    #[test]
    fn no_primary_key_refuses_by_default() {
        let snapshot = table_snapshot(&[], &["id", "name"]);
        let edit = resolve_editability(&snapshot, &table_ref(), &users_columns(), "refuse");
        assert!(
            matches!(edit, EditState::NotEditable(reason) if reason.contains("no primary key"))
        );
    }

    #[test]
    fn no_primary_key_is_editable_under_the_all_columns_where_policy() {
        let snapshot = table_snapshot(&[], &["id", "name"]);
        let edit = resolve_editability(
            &snapshot,
            &table_ref(),
            &users_columns(),
            "all_columns_where",
        );
        assert!(matches!(edit, EditState::Editable(_)));
    }

    #[test]
    fn a_primary_key_column_not_selected_by_the_query_is_not_editable() {
        // The table's real key is `id`, but this result only selected
        // `name` — nothing to bind a `WHERE` on.
        let snapshot = table_snapshot(&["id"], &["id", "name"]);
        let columns = vec![ColumnMeta::new("name", "text", true)];
        let edit = resolve_editability(&snapshot, &table_ref(), &columns, "refuse");
        assert!(matches!(edit, EditState::NotEditable(_)));
    }

    #[test]
    fn default_cell_value_uses_null_for_a_nullable_column() {
        assert_eq!(default_cell_value("int4", true), Value::Null);
    }

    fn test_console(shared: &Rc<std::cell::RefCell<super::super::console::Shared>>) -> u64 {
        use super::super::sessions::SessionWorker;
        use db_core::driver::{Connection, Driver};
        use db_core::session::Session;

        let spec = db_core::datasource::ConnectSpec {
            driver: "sqlite".to_string(),
            host: String::new(),
            port: None,
            database: ":memory:".to_string(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: Default::default(),
        };
        let connection: Box<dyn Connection> =
            db_drivers::sqlite::SqliteDriver.connect(&spec).unwrap();
        let session = Session::new(connection);
        let worker = SessionWorker::spawn(session, |_event| {});
        let tab_id = 1;
        shared.borrow_mut().consoles.insert(
            tab_id,
            super::super::console::ConsoleState {
                source_id: "src-1".to_string(),
                worker,
                dialect: Dialect::Sqlite,
                guard: Guard::new(
                    false,
                    Box::new(SqlClassifier {
                        dialect: Dialect::Sqlite,
                    }),
                ),
                tx_mode: ffi::FfiDbTxMode::Auto,
                script_policy: ffi::FfiDbScriptPolicy::StopOnError,
                page_size: 100,
                history: false,
                current_result: None,
                pending: None,
                schemas: Vec::new(),
                pending_introspects: Default::default(),
                submitting: None,
            },
        );
        tab_id
    }

    fn test_result(
        shared: &Rc<std::cell::RefCell<super::super::console::Shared>>,
        tab_id: u64,
        edit: EditState,
    ) -> u64 {
        let mut shared = shared.borrow_mut();
        let result_id = shared.alloc_result_id();
        shared.results.insert(
            result_id,
            ResultState {
                tab_id,
                columns: Vec::new(),
                set: ResultSet::new(),
                done: true,
                reported: true,
                cap_reached: false,
                affected: None,
                error: None,
                started_at: Instant::now(),
                elapsed_ms: None,
                statement_text: "SELECT * FROM t".to_string(),
                edit,
                table_constraints: Vec::new(),
            },
        );
        result_id
    }

    #[test]
    fn turning_read_only_on_demotes_an_editable_result_and_tightens_the_guard() {
        let shared = Rc::new(std::cell::RefCell::new(
            super::super::console::Shared::default(),
        ));
        let tab_id = test_console(&shared);
        let result_id = test_result(
            &shared,
            tab_id,
            EditState::Editable(EditBuffer::new(
                "t".to_string(),
                vec!["id".to_string()],
                vec!["id".to_string()],
                Vec::new(),
            )),
        );

        rebuild_guards_for_source(&shared, "src-1", true);

        let shared_ref = shared.borrow();
        assert!(shared_ref.consoles[&tab_id]
            .guard
            .check("DELETE FROM t")
            .is_err());
        assert!(
            matches!(&shared_ref.results[&result_id].edit, EditState::NotEditable(reason) if reason.contains("read-only"))
        );
    }

    #[test]
    fn turning_read_only_off_lets_a_demoted_result_be_re_checked() {
        let shared = Rc::new(std::cell::RefCell::new(
            super::super::console::Shared::default(),
        ));
        let tab_id = test_console(&shared);
        let result_id = test_result(
            &shared,
            tab_id,
            EditState::NotEditable("this data source is read-only".to_string()),
        );

        rebuild_guards_for_source(&shared, "src-1", false);

        let shared_ref = shared.borrow();
        assert!(shared_ref.consoles[&tab_id]
            .guard
            .check("DELETE FROM t")
            .is_ok());
        assert!(matches!(
            &shared_ref.results[&result_id].edit,
            EditState::Unknown
        ));
    }

    #[test]
    fn a_console_on_a_different_source_is_left_untouched() {
        let shared = Rc::new(std::cell::RefCell::new(
            super::super::console::Shared::default(),
        ));
        let tab_id = test_console(&shared);
        let result_id = test_result(
            &shared,
            tab_id,
            EditState::Editable(EditBuffer::new(
                "t".to_string(),
                vec!["id".to_string()],
                vec!["id".to_string()],
                Vec::new(),
            )),
        );

        rebuild_guards_for_source(&shared, "some-other-source", true);

        let shared_ref = shared.borrow();
        assert!(shared_ref.consoles[&tab_id]
            .guard
            .check("DELETE FROM t")
            .is_ok());
        assert!(matches!(
            &shared_ref.results[&result_id].edit,
            EditState::Editable(_)
        ));
    }

    #[test]
    fn default_cell_value_zero_values_by_loose_type_match() {
        assert_eq!(default_cell_value("int4", false), Value::Int(0));
        assert_eq!(default_cell_value("bigint", false), Value::Int(0));
        assert_eq!(
            default_cell_value("double precision", false),
            Value::Float(0.0)
        );
        assert_eq!(default_cell_value("boolean", false), Value::Bool(false));
        assert_eq!(
            default_cell_value("numeric(10,2)", false),
            Value::Decimal("0".to_string())
        );
        assert_eq!(
            default_cell_value("varchar(255)", false),
            Value::Text(String::new())
        );
    }
}
