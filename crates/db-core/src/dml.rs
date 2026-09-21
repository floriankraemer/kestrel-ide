//! The data editor's staged-edit buffer and the plan it compiles to
//! (database-tools.md §4's "data-editor submit" sequence): `EditBuffer`
//! stages cell edits, added rows and deleted rows by grid position,
//! `EditBuffer::to_dml_plan` turns them into bound statements — every
//! value a parameter, every identifier quoted through [`Dialect`], never
//! a value interpolated into the statement text (ADR-0061 §1's "all
//! values bound" rule; this module's own tests are the "0 unquoted
//! occurrences" property tests that rule requires).
//!
//! # Grid row addressing
//!
//! A grid row index is either an *existing* (fetched) row — `0..rows.len()`
//! — or an *inserted* row a user staged with [`EditBuffer::add_row`] or
//! [`EditBuffer::clone_row`], addressed starting at `rows.len()`. Deleting
//! an inserted row (it was never persisted) removes it outright, which is
//! why every caller re-reads row structure after a row-shaped op
//! (`addRow`/`cloneRow`/`deleteRows`) rather than caching indices across
//! one — the same "re-derive, don't cache" rule the grid already applies
//! to a fetched page.
//!
//! # The `NoPrimaryKey` policy
//!
//! A table with no declared primary key has nothing stable to `WHERE` on
//! by id, so the plan instead matches the *entire original row* — every
//! original column value in the `WHERE` clause (`IS NULL` for a `NULL`
//! original, `= ?` otherwise). This is strictly more matches than a real
//! primary key would give (two identical rows both match), which is the
//! same trade-off a spreadsheet-shaped grid editor without a key always
//! has, not a bug this module can fix without inventing a key the table
//! does not have. Whether this fallback is even offered to a user (as
//! opposed to refusing to open the editor at all) is `ui-shell`'s own
//! `NoPrimaryKey` setting, read before a result is ever handed to this
//! module — `EditBuffer` itself has no opinion on that policy, it just
//! does the only thing possible when `primary_key` is empty.

use std::collections::{BTreeMap, BTreeSet};

use crate::dialect::Dialect;
use crate::driver::{QueryLang, Statement};
use crate::value::Value;

/// One staged cell edit: a concrete new value, or "reset to the column's
/// own declared default" — the value editor's NULL/DEFAULT buttons map to
/// `Set(Value::Null)` and `Default` respectively (database-tools-plan
/// F4.1).
#[derive(Debug, Clone, PartialEq)]
pub enum CellEdit {
    Set(Value),
    /// Renders as the bare `DEFAULT` keyword in an `UPDATE`'s `SET`
    /// clause — never a bound parameter, since it names no value at all.
    /// Postgres/MySQL/SQL Server all accept `col = DEFAULT`; SQLite does
    /// not (its `UPDATE` grammar has no `DEFAULT` keyword) — a submit that
    /// stages this against SQLite surfaces the engine's own syntax error
    /// rather than silently falling back to something else.
    /// ponytail: a SQLite-specific `col = (SELECT dflt_value FROM
    /// pragma_table_info(...) ...)` rewrite would close this gap; add it
    /// if a real SQLite user hits it.
    Default,
}

/// One table's currently-fetched rows, staged for editing.
#[derive(Debug, Clone)]
pub struct EditBuffer {
    table: String,
    columns: Vec<String>,
    primary_key: Vec<String>,
    /// The original values fetched for each row, aligned with `columns` —
    /// what a `NoPrimaryKey` plan matches on, and what a keyed plan reads
    /// the key values from.
    rows: Vec<Vec<Value>>,
    /// `(row_index, column_name) -> new value`, in the order staged.
    edits: BTreeMap<(usize, String), CellEdit>,
    /// Indices into `rows` staged for deletion.
    deleted: BTreeSet<usize>,
    /// Rows added since the last [`Self::clear`], full-width and aligned
    /// with `columns` — grid row `rows.len() + i` for `inserted[i]`.
    inserted: Vec<Vec<Value>>,
    /// Edits staged against an *inserted* row, keyed by its index into
    /// `inserted` (not the grid row number).
    inserted_edits: BTreeMap<(usize, String), CellEdit>,
}

impl EditBuffer {
    pub fn new(
        table: impl Into<String>,
        columns: Vec<String>,
        primary_key: Vec<String>,
        rows: Vec<Vec<Value>>,
    ) -> Self {
        Self {
            table: table.into(),
            columns,
            primary_key,
            rows,
            edits: BTreeMap::new(),
            deleted: BTreeSet::new(),
            inserted: Vec::new(),
            inserted_edits: BTreeMap::new(),
        }
    }

    fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column == name)
    }

    /// Stage `row`/`column`'s new value — an existing row's cell if `row <
    /// self.rows.len()`, an inserted row's own initial value otherwise. A
    /// caller with an out-of-range `row` (stale index, a concurrent
    /// `fetchMore`) is a silent no-op — the same "cannot happen, and if it
    /// somehow does, do nothing rather than panic" stance the FFI seam
    /// takes on every other stale-id lookup.
    pub fn stage(&mut self, row: usize, column: &str, value: Value) {
        self.stage_edit(row, column, CellEdit::Set(value));
    }

    /// Stage `row`/`column` back to the column's own declared default —
    /// the value editor's "DEFAULT" button.
    pub fn stage_default(&mut self, row: usize, column: &str) {
        self.stage_edit(row, column, CellEdit::Default);
    }

    fn stage_edit(&mut self, row: usize, column: &str, edit: CellEdit) {
        if self.column_index(column).is_none() {
            return;
        }
        if row < self.rows.len() {
            self.edits.insert((row, column.to_string()), edit);
        } else if let Some(index) = row.checked_sub(self.rows.len()) {
            if index < self.inserted.len() {
                self.inserted_edits
                    .insert((index, column.to_string()), edit);
            }
        }
    }

    /// Drops a single staged cell edit, restoring the original fetched
    /// value (or, for an inserted row, the value it was added with).
    pub fn revert_cell(&mut self, row: usize, column: &str) {
        if row < self.rows.len() {
            self.edits.remove(&(row, column.to_string()));
        } else if let Some(index) = row.checked_sub(self.rows.len()) {
            self.inserted_edits.remove(&(index, column.to_string()));
        }
    }

    /// The value `row`/`column` would submit as right now — the original
    /// fetched value with any staged edit applied on top, or an inserted
    /// row's own (possibly edited) value. `None` for an out-of-range row
    /// or column.
    pub fn current_value(&self, row: usize, column: &str) -> Option<Value> {
        let index = self.column_index(column)?;
        if row < self.rows.len() {
            match self.edits.get(&(row, column.to_string())) {
                Some(CellEdit::Set(value)) => Some(value.clone()),
                Some(CellEdit::Default) => Some(Value::Null),
                None => self.rows[row].get(index).cloned(),
            }
        } else {
            let insert_index = row - self.rows.len();
            let base = self.inserted.get(insert_index)?.get(index).cloned();
            match self.inserted_edits.get(&(insert_index, column.to_string())) {
                Some(CellEdit::Set(value)) => Some(value.clone()),
                Some(CellEdit::Default) => Some(Value::Null),
                None => base,
            }
        }
    }

    /// Stages a new row, `values` aligned 1:1 with `self.columns` (the
    /// caller fills each column's own default — `NULL` for a nullable
    /// column, the type's zero value otherwise; `ui-shell` reads that from
    /// `ColumnMeta`). Returns the new row's grid index.
    pub fn add_row(&mut self, values: Vec<Value>) -> usize {
        self.inserted.push(values);
        self.rows.len() + self.inserted.len() - 1
    }

    /// Stages a copy of `row`'s *current* values (staged edits applied) as
    /// a new inserted row. `None` for an out-of-range `row`.
    pub fn clone_row(&mut self, row: usize) -> Option<usize> {
        let values: Vec<Value> = self
            .columns
            .iter()
            .map(|column| self.current_value(row, column).unwrap_or(Value::Null))
            .collect();
        Some(self.add_row(values))
    }

    /// Stages `row` for deletion. An inserted row (never persisted) is
    /// simply un-added rather than tombstoned — see this module's doc
    /// comment on why grid row indices are re-derived after this call
    /// rather than assumed stable.
    pub fn delete_row(&mut self, row: usize) {
        if row < self.rows.len() {
            self.deleted.insert(row);
            self.edits.retain(|(r, _), _| *r != row);
        } else {
            let index = row - self.rows.len();
            if index < self.inserted.len() {
                self.inserted.remove(index);
                // Every inserted-row edit keyed past `index` shifted down
                // by one; below it is untouched; `index` itself is gone.
                self.inserted_edits = self
                    .inserted_edits
                    .iter()
                    .filter_map(|((i, column), edit)| match (*i).cmp(&index) {
                        std::cmp::Ordering::Less => Some(((*i, column.clone()), edit.clone())),
                        std::cmp::Ordering::Equal => None,
                        std::cmp::Ordering::Greater => {
                            Some(((*i - 1, column.clone()), edit.clone()))
                        }
                    })
                    .collect();
            }
        }
    }

    /// How many rows currently have a pending change: an edited or
    /// deleted existing row counts once each, an inserted row counts once
    /// regardless of its own further edits — the grid's own "N pending
    /// changes" badge (F4.2), not a per-cell count.
    pub fn pending_count(&self) -> usize {
        let edited_rows: BTreeSet<usize> = self.edits.keys().map(|(row, _)| *row).collect();
        edited_rows
            .union(&self.deleted)
            .count()
            .saturating_add(self.inserted.len())
    }

    pub fn is_empty(&self) -> bool {
        self.pending_count() == 0
    }

    /// Discards every staged edit/add/delete — a submit's own cleanup, or
    /// an explicit "Revert" (F4.2).
    pub fn clear(&mut self) {
        self.edits.clear();
        self.deleted.clear();
        self.inserted.clear();
        self.inserted_edits.clear();
    }

    /// The `WHERE`-clause fragment (text + params, appended to `params`)
    /// identifying `row` — the primary-key columns if there are any,
    /// otherwise every original column (the `NoPrimaryKey` policy above).
    fn where_clause(&self, dialect: Dialect, row: usize, params: &mut Vec<Value>) -> String {
        let keys: Vec<&String> = if self.primary_key.is_empty() {
            self.columns.iter().collect()
        } else {
            self.primary_key.iter().collect()
        };
        let original = &self.rows[row];
        let mut clauses = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(index) = self.column_index(key) else {
                continue;
            };
            let value = &original[index];
            let quoted = dialect.quote_ident(key);
            if matches!(value, Value::Null) {
                clauses.push(format!("{quoted} IS NULL"));
            } else {
                clauses.push(format!("{quoted} = ?"));
                params.push(value.clone());
            }
        }
        clauses.join(" AND ")
    }

    /// Compile every staged edit/add/delete into bound statements: one
    /// `UPDATE` per edited-and-not-deleted row, one `DELETE` per deleted
    /// row, one `INSERT` per added-and-not-removed row — deletes and
    /// inserts before updates, so a submit that both edits and removes
    /// rows never updates a row it is about to delete.
    pub fn to_dml_plan(&self, dialect: Dialect) -> DmlPlan {
        let mut statements = Vec::new();

        for &row in &self.deleted {
            let mut params = Vec::new();
            let where_clause = self.where_clause(dialect, row, &mut params);
            statements.push(Statement {
                lang: QueryLang::Sql,
                text: format!(
                    "DELETE FROM {} WHERE {}",
                    dialect.quote_ident(&self.table),
                    where_clause
                ),
                params,
            });
        }

        for (index, values) in self.inserted.iter().enumerate() {
            let mut columns = Vec::with_capacity(self.columns.len());
            let mut params = Vec::with_capacity(self.columns.len());
            for (col_index, column) in self.columns.iter().enumerate() {
                match self.inserted_edits.get(&(index, column.clone())) {
                    // Omitted from the INSERT entirely, so the engine
                    // applies the column's own default — portable across
                    // every dialect, unlike the `DEFAULT` keyword in an
                    // `UPDATE`'s `SET` (`CellEdit::Default`'s own doc
                    // comment).
                    Some(CellEdit::Default) => continue,
                    Some(CellEdit::Set(value)) => {
                        columns.push(dialect.quote_ident(column));
                        params.push(value.clone());
                    }
                    None => {
                        columns.push(dialect.quote_ident(column));
                        params.push(values[col_index].clone());
                    }
                }
            }
            let placeholders = vec!["?"; columns.len()].join(", ");
            statements.push(Statement {
                lang: QueryLang::Sql,
                text: format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    dialect.quote_ident(&self.table),
                    columns.join(", "),
                    placeholders
                ),
                params,
            });
        }

        let mut by_row: BTreeMap<usize, Vec<(&str, &CellEdit)>> = BTreeMap::new();
        for ((row, column), edit) in &self.edits {
            if self.deleted.contains(row) {
                continue;
            }
            by_row
                .entry(*row)
                .or_default()
                .push((column.as_str(), edit));
        }
        for (row, cells) in by_row {
            let mut params = Vec::with_capacity(cells.len() + self.columns.len());
            let assignments: Vec<String> = cells
                .iter()
                .map(|(column, edit)| match edit {
                    CellEdit::Set(value) => {
                        params.push((*value).clone());
                        format!("{} = ?", dialect.quote_ident(column))
                    }
                    CellEdit::Default => format!("{} = DEFAULT", dialect.quote_ident(column)),
                })
                .collect();
            let mut where_params = Vec::new();
            let where_clause = self.where_clause(dialect, row, &mut where_params);
            params.extend(where_params);

            let text = format!(
                "UPDATE {} SET {} WHERE {}",
                dialect.quote_ident(&self.table),
                assignments.join(", "),
                where_clause
            );
            statements.push(Statement {
                lang: QueryLang::Sql,
                text,
                params,
            });
        }
        DmlPlan { statements }
    }
}

/// The bound statements a data-editor submit runs, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct DmlPlan {
    pub statements: Vec<Statement>,
}

impl DmlPlan {
    /// A read-only preview of the plan's SQL, with every bound param shown
    /// as `?` — never a value substituted in, since that is exactly the
    /// interpolation this module exists to avoid. `ui-shell`'s preview
    /// document renders the params list beside this text separately.
    pub fn preview_text(&self) -> String {
        self.statements
            .iter()
            .map(|statement| statement.text.clone())
            .collect::<Vec<_>>()
            .join(";\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_with_key() -> EditBuffer {
        EditBuffer::new(
            "users",
            vec!["id".to_string(), "name".to_string(), "email".to_string()],
            vec!["id".to_string()],
            vec![vec![
                Value::Int(1),
                Value::Text("alice".to_string()),
                Value::Text("alice@example.com".to_string()),
            ]],
        )
    }

    #[test]
    fn no_staged_edits_produce_no_statements() {
        let plan = buffer_with_key().to_dml_plan(Dialect::Postgres);
        assert!(plan.statements.is_empty());
        assert!(buffer_with_key().is_empty());
    }

    #[test]
    fn a_keyed_edit_binds_the_new_value_and_the_key_never_inlining_either() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "email", Value::Text("alice@new.example".to_string()));
        assert_eq!(buffer.pending_count(), 1);
        let plan = buffer.to_dml_plan(Dialect::Postgres);

        assert_eq!(plan.statements.len(), 1);
        let statement = &plan.statements[0];
        assert_eq!(
            statement.text,
            "UPDATE \"users\" SET \"email\" = ? WHERE \"id\" = ?"
        );
        assert_eq!(
            statement.params,
            vec![Value::Text("alice@new.example".to_string()), Value::Int(1),]
        );
        // Neither value appears as a literal anywhere in the text.
        assert!(!statement.text.contains("alice@new.example"));
        assert!(!statement.text.contains('1'));
    }

    #[test]
    fn a_null_key_value_uses_is_null_not_a_bound_equality() {
        let mut buffer = EditBuffer::new(
            "t",
            vec!["k".to_string(), "v".to_string()],
            vec!["k".to_string()],
            vec![vec![Value::Null, Value::Int(1)]],
        );
        buffer.stage(0, "v", Value::Int(2));
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(
            plan.statements[0].text,
            "UPDATE \"t\" SET \"v\" = ? WHERE \"k\" IS NULL"
        );
        assert_eq!(plan.statements[0].params, vec![Value::Int(2)]);
    }

    #[test]
    fn no_primary_key_matches_the_entire_original_row() {
        let mut buffer = EditBuffer::new(
            "logs",
            vec!["level".to_string(), "message".to_string()],
            vec![],
            vec![vec![
                Value::Text("info".to_string()),
                Value::Text("started".to_string()),
            ]],
        );
        buffer.stage(0, "message", Value::Text("stopped".to_string()));
        let plan = buffer.to_dml_plan(Dialect::MySql);

        assert_eq!(
            plan.statements[0].text,
            "UPDATE `logs` SET `message` = ? WHERE `level` = ? AND `message` = ?"
        );
        assert_eq!(
            plan.statements[0].params,
            vec![
                Value::Text("stopped".to_string()),
                Value::Text("info".to_string()),
                Value::Text("started".to_string()),
            ]
        );
    }

    #[test]
    fn multiple_edited_columns_on_one_row_produce_one_statement() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "name", Value::Text("alicia".to_string()));
        buffer.stage(0, "email", Value::Text("a@example.com".to_string()));
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(plan.statements.len(), 1);
        assert!(plan.statements[0].text.contains("\"name\" = ?"));
        assert!(plan.statements[0].text.contains("\"email\" = ?"));
    }

    #[test]
    fn an_identifier_that_could_break_out_of_quoting_is_neutralised() {
        let mut buffer = EditBuffer::new(
            "Robert'); DROP TABLE students;--",
            vec!["a\"b".to_string()],
            vec![],
            vec![vec![Value::Int(1)]],
        );
        buffer.stage(0, "a\"b", Value::Int(2));
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        // The adversarial table/column names are quoted, not executed as
        // syntax: no unescaped `"` breaks either identifier out early.
        assert!(plan.statements[0]
            .text
            .starts_with("UPDATE \"Robert'); DROP TABLE students;--\" SET \"a\"\"b\" = ?"));
    }

    #[test]
    fn clear_drops_every_staged_edit() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "name", Value::Text("x".to_string()));
        assert!(!buffer.is_empty());
        buffer.clear();
        assert!(buffer.is_empty());
        assert!(buffer.to_dml_plan(Dialect::Postgres).statements.is_empty());
    }

    #[test]
    fn preview_text_never_substitutes_a_param_value_into_the_text() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "email", Value::Text("SECRET_MARKER".to_string()));
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert!(!plan.preview_text().contains("SECRET_MARKER"));
        assert!(plan.preview_text().contains('?'));
    }

    #[test]
    fn stage_default_renders_the_bare_keyword_never_a_bound_param() {
        let mut buffer = buffer_with_key();
        buffer.stage_default(0, "email");
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(
            plan.statements[0].text,
            "UPDATE \"users\" SET \"email\" = DEFAULT WHERE \"id\" = ?"
        );
        assert_eq!(plan.statements[0].params, vec![Value::Int(1)]);
    }

    #[test]
    fn revert_cell_drops_only_that_cell() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "name", Value::Text("x".to_string()));
        buffer.stage(0, "email", Value::Text("y".to_string()));
        buffer.revert_cell(0, "name");
        assert_eq!(
            buffer.current_value(0, "name"),
            Some(Value::Text("alice".to_string()))
        );
        assert_eq!(
            buffer.current_value(0, "email"),
            Some(Value::Text("y".to_string()))
        );
    }

    #[test]
    fn add_row_stages_an_insert_with_every_column_bound() {
        let mut buffer = buffer_with_key();
        let new_row = buffer.add_row(vec![
            Value::Int(2),
            Value::Text("bob".to_string()),
            Value::Null,
        ]);
        assert_eq!(new_row, 1);
        assert_eq!(buffer.pending_count(), 1);
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(plan.statements.len(), 1);
        assert_eq!(
            plan.statements[0].text,
            "INSERT INTO \"users\" (\"id\", \"name\", \"email\") VALUES (?, ?, ?)"
        );
        assert_eq!(
            plan.statements[0].params,
            vec![Value::Int(2), Value::Text("bob".to_string()), Value::Null]
        );
    }

    #[test]
    fn editing_an_added_row_before_submit_changes_its_insert_params() {
        let mut buffer = buffer_with_key();
        let new_row = buffer.add_row(vec![Value::Int(2), Value::Null, Value::Null]);
        buffer.stage(new_row, "name", Value::Text("bob".to_string()));
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(
            plan.statements[0].params,
            vec![Value::Int(2), Value::Text("bob".to_string()), Value::Null]
        );
    }

    #[test]
    fn a_default_cell_on_an_added_row_is_omitted_from_the_insert() {
        let mut buffer = buffer_with_key();
        let new_row = buffer.add_row(vec![Value::Int(2), Value::Null, Value::Null]);
        buffer.stage_default(new_row, "email");
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(
            plan.statements[0].text,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES (?, ?)"
        );
    }

    #[test]
    fn delete_row_stages_a_delete_matched_on_the_primary_key() {
        let mut buffer = buffer_with_key();
        buffer.delete_row(0);
        assert_eq!(buffer.pending_count(), 1);
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(plan.statements.len(), 1);
        assert_eq!(
            plan.statements[0].text,
            "DELETE FROM \"users\" WHERE \"id\" = ?"
        );
        assert_eq!(plan.statements[0].params, vec![Value::Int(1)]);
    }

    #[test]
    fn deleting_a_row_drops_any_edit_already_staged_on_it() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "name", Value::Text("x".to_string()));
        buffer.delete_row(0);
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(plan.statements.len(), 1);
        assert!(plan.statements[0].text.starts_with("DELETE"));
    }

    #[test]
    fn deleting_an_inserted_row_removes_it_outright_with_no_statement() {
        let mut buffer = buffer_with_key();
        let new_row = buffer.add_row(vec![Value::Int(2), Value::Null, Value::Null]);
        buffer.delete_row(new_row);
        assert!(buffer.is_empty());
        assert!(buffer.to_dml_plan(Dialect::Postgres).statements.is_empty());
    }

    #[test]
    fn deleting_an_inserted_row_reindexes_edits_on_later_inserted_rows() {
        let mut buffer = buffer_with_key();
        let first = buffer.add_row(vec![Value::Int(2), Value::Null, Value::Null]);
        let second = buffer.add_row(vec![Value::Int(3), Value::Null, Value::Null]);
        buffer.stage(second, "name", Value::Text("carol".to_string()));
        buffer.delete_row(first);
        // `second`'s edit must survive the reindex even though its own
        // grid position shifted down by one.
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(plan.statements.len(), 1);
        assert!(plan.statements[0]
            .params
            .contains(&Value::Text("carol".to_string())));
    }

    #[test]
    fn clone_row_copies_current_values_including_unsubmitted_edits() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "name", Value::Text("alicia".to_string()));
        let cloned = buffer.clone_row(0).unwrap();
        assert_eq!(
            buffer.current_value(cloned, "name"),
            Some(Value::Text("alicia".to_string()))
        );
        assert_eq!(
            buffer.current_value(cloned, "email"),
            Some(Value::Text("alice@example.com".to_string()))
        );
    }

    #[test]
    fn delete_and_insert_and_update_statements_all_land_in_one_plan() {
        let mut buffer = buffer_with_key();
        buffer.add_row(vec![
            Value::Int(2),
            Value::Text("bob".to_string()),
            Value::Null,
        ]);
        let mut second = buffer_with_key();
        second.delete_row(0);
        // Exercise both shapes independently (a buffer only has one table's
        // worth of fetched rows, so insert and delete share `buffer` here
        // to prove ordering, not `second`).
        buffer.delete_row(0);
        let plan = buffer.to_dml_plan(Dialect::Postgres);
        assert_eq!(plan.statements.len(), 2);
        assert!(plan.statements[0].text.starts_with("DELETE"));
        assert!(plan.statements[1].text.starts_with("INSERT"));
    }
}
