//! The data editor's staged-edit buffer and the plan it compiles to
//! (database-tools.md §4's "data-editor submit" sequence): `EditBuffer`
//! stages cell edits by row/column, `DmlPlan::plan` turns them into bound
//! statements — every value a parameter, every identifier quoted through
//! [`Dialect`], never a value interpolated into the statement text
//! (ADR-0061 §1's "all values bound" rule; this module's own tests are
//! the "0 unquoted occurrences" property tests that rule requires).
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
//! does not have.

use std::collections::BTreeMap;

use crate::dialect::Dialect;
use crate::driver::{QueryLang, Statement};
use crate::value::Value;

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
    edits: BTreeMap<(usize, String), Value>,
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
        }
    }

    pub fn stage(&mut self, row: usize, column: &str, value: Value) {
        self.edits.insert((row, column.to_string()), value);
    }

    pub fn clear(&mut self) {
        self.edits.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column == name)
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

    /// Compile every staged edit into one `UPDATE` statement per touched
    /// row.
    pub fn to_dml_plan(&self, dialect: Dialect) -> DmlPlan {
        let mut by_row: BTreeMap<usize, Vec<(&str, &Value)>> = BTreeMap::new();
        for ((row, column), value) in &self.edits {
            by_row
                .entry(*row)
                .or_default()
                .push((column.as_str(), value));
        }

        let mut statements = Vec::with_capacity(by_row.len());
        for (row, cells) in by_row {
            let mut params = Vec::with_capacity(cells.len() + self.columns.len());
            let assignments: Vec<String> = cells
                .iter()
                .map(|(column, value)| {
                    params.push((*value).clone());
                    format!("{} = ?", dialect.quote_ident(column))
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
    }

    #[test]
    fn a_keyed_edit_binds_the_new_value_and_the_key_never_inlining_either() {
        let mut buffer = buffer_with_key();
        buffer.stage(0, "email", Value::Text("alice@new.example".to_string()));
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
}
