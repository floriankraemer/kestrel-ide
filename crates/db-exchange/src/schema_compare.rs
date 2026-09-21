//! Schema compare + migration script (F6.2, database-tools-plan.md):
//! [`compare`] diffs two [`SchemaSnapshot`]s (tables/columns/indexes/
//! constraints structurally, views/routines as text blocks — the plan's
//! recorded debt: no structural view/routine diff), [`migration_script`]
//! turns that into DDL for tables/columns/indexes/constraints, and
//! [`ddl_pairs`] hands the DiffView a left/right DDL text pair per
//! changed object.
//!
//! Deviation from the plan doc's literal signature: [`ddl_pairs`] takes
//! `dialect` alongside the diff (DDL text needs a dialect to quote
//! against, and a [`SchemaDiff`] does not carry one).

use std::collections::{BTreeMap, BTreeSet};

use db_core::dialect::Dialect;

use crate::schema_model::{
    ColumnDef, ConstraintDef, IndexDef, SchemaSnapshot, TableDef, TextObject,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnChange {
    Added(ColumnDef),
    Dropped(ColumnDef),
    Changed { before: ColumnDef, after: ColumnDef },
}

impl ColumnChange {
    fn name(&self) -> &str {
        match self {
            ColumnChange::Added(c) | ColumnChange::Dropped(c) => &c.name,
            ColumnChange::Changed { after, .. } => &after.name,
        }
    }
}

/// Everything that changed about one table that exists on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDiff {
    pub table: String,
    pub columns: Vec<ColumnChange>,
    pub added_indexes: Vec<IndexDef>,
    pub dropped_indexes: Vec<IndexDef>,
    pub added_constraints: Vec<ConstraintDef>,
    pub dropped_constraints: Vec<ConstraintDef>,
    /// Kept alongside the field-level changes above so [`ddl_pairs`] can
    /// render a full before/after `CREATE TABLE` without needing the
    /// original snapshots back.
    pub before: TableDef,
    pub after: TableDef,
}

impl TableDiff {
    fn is_empty(&self) -> bool {
        self.columns.is_empty()
            && self.added_indexes.is_empty()
            && self.dropped_indexes.is_empty()
            && self.added_constraints.is_empty()
            && self.dropped_constraints.is_empty()
    }
}

/// A view or routine's before/after text, `None` on the side it does not
/// exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChange {
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchemaDiff {
    pub added_tables: Vec<TableDef>,
    pub dropped_tables: Vec<TableDef>,
    pub changed_tables: Vec<TableDiff>,
    pub views: Vec<TextChange>,
    pub routines: Vec<TextChange>,
}

impl SchemaDiff {
    pub fn is_empty(&self) -> bool {
        self.added_tables.is_empty()
            && self.dropped_tables.is_empty()
            && self.changed_tables.is_empty()
            && self.views.is_empty()
            && self.routines.is_empty()
    }
}

fn compare_columns(left: &TableDef, right: &TableDef) -> Vec<ColumnChange> {
    let left_cols: BTreeMap<&str, &ColumnDef> =
        left.columns.iter().map(|c| (c.name.as_str(), c)).collect();
    let right_cols: BTreeMap<&str, &ColumnDef> =
        right.columns.iter().map(|c| (c.name.as_str(), c)).collect();

    let mut changes = Vec::new();
    for (name, right_col) in &right_cols {
        match left_cols.get(name) {
            None => changes.push(ColumnChange::Added((*right_col).clone())),
            Some(left_col) if *left_col != *right_col => changes.push(ColumnChange::Changed {
                before: (*left_col).clone(),
                after: (*right_col).clone(),
            }),
            _ => {}
        }
    }
    for (name, left_col) in &left_cols {
        if !right_cols.contains_key(name) {
            changes.push(ColumnChange::Dropped((*left_col).clone()));
        }
    }
    changes.sort_by(|a, b| a.name().cmp(b.name()));
    changes
}

fn compare_table(left: &TableDef, right: &TableDef) -> TableDiff {
    let added_indexes: Vec<IndexDef> = right
        .indexes
        .iter()
        .filter(|i| !left.indexes.iter().any(|l| l.name == i.name))
        .cloned()
        .collect();
    let dropped_indexes: Vec<IndexDef> = left
        .indexes
        .iter()
        .filter(|i| !right.indexes.iter().any(|r| r.name == i.name))
        .cloned()
        .collect();
    let added_constraints: Vec<ConstraintDef> = right
        .constraints
        .iter()
        .filter(|c| {
            !left
                .constraints
                .iter()
                .any(|l| l.name == c.name && l.text == c.text)
        })
        .cloned()
        .collect();
    let dropped_constraints: Vec<ConstraintDef> = left
        .constraints
        .iter()
        .filter(|c| {
            !right
                .constraints
                .iter()
                .any(|r| r.name == c.name && r.text == c.text)
        })
        .cloned()
        .collect();

    TableDiff {
        table: left.name.clone(),
        columns: compare_columns(left, right),
        added_indexes,
        dropped_indexes,
        added_constraints,
        dropped_constraints,
        before: left.clone(),
        after: right.clone(),
    }
}

fn compare_text_objects(left: &[TextObject], right: &[TextObject]) -> Vec<TextChange> {
    let left_map: BTreeMap<&str, &str> = left
        .iter()
        .map(|o| (o.name.as_str(), o.definition.as_str()))
        .collect();
    let right_map: BTreeMap<&str, &str> = right
        .iter()
        .map(|o| (o.name.as_str(), o.definition.as_str()))
        .collect();
    let names: BTreeSet<&str> = left_map.keys().chain(right_map.keys()).copied().collect();

    let mut changes = Vec::new();
    for name in names {
        let before = left_map.get(name).copied();
        let after = right_map.get(name).copied();
        if before != after {
            changes.push(TextChange {
                name: name.to_string(),
                before: before.map(str::to_string),
                after: after.map(str::to_string),
            });
        }
    }
    changes.sort_by(|a, b| a.name.cmp(&b.name));
    changes
}

/// Diff `left` against `right` (`left` -> `right`, the direction
/// [`migration_script`] emits DDL in).
pub fn compare(left: &SchemaSnapshot, right: &SchemaSnapshot) -> SchemaDiff {
    let left_names: BTreeSet<&str> = left.tables.iter().map(|t| t.name.as_str()).collect();
    let right_names: BTreeSet<&str> = right.tables.iter().map(|t| t.name.as_str()).collect();

    let mut added_tables: Vec<TableDef> = right
        .tables
        .iter()
        .filter(|t| !left_names.contains(t.name.as_str()))
        .cloned()
        .collect();
    let mut dropped_tables: Vec<TableDef> = left
        .tables
        .iter()
        .filter(|t| !right_names.contains(t.name.as_str()))
        .cloned()
        .collect();
    added_tables.sort_by(|a, b| a.name.cmp(&b.name));
    dropped_tables.sort_by(|a, b| a.name.cmp(&b.name));

    let mut changed_tables: Vec<TableDiff> = left
        .tables
        .iter()
        .filter_map(|left_table| {
            right
                .table(&left_table.name)
                .map(|right_table| compare_table(left_table, right_table))
        })
        .filter(|diff| !diff.is_empty())
        .collect();
    changed_tables.sort_by(|a, b| a.table.cmp(&b.table));

    SchemaDiff {
        added_tables,
        dropped_tables,
        changed_tables,
        views: compare_text_objects(&left.views, &right.views),
        routines: compare_text_objects(&left.routines, &right.routines),
    }
}

fn column_ddl(dialect: Dialect, column: &ColumnDef) -> String {
    let nullability = if column.nullable { "" } else { " NOT NULL" };
    let default = column
        .default
        .as_ref()
        .map(|d| format!(" DEFAULT {d}"))
        .unwrap_or_default();
    format!(
        "{} {}{default}{nullability}",
        dialect.quote_ident(&column.name),
        column.type_name
    )
}

fn create_table_ddl(dialect: Dialect, table: &TableDef) -> String {
    let mut defs: Vec<String> = table
        .columns
        .iter()
        .map(|c| column_ddl(dialect, c))
        .collect();
    if !table.primary_key.is_empty() {
        let keys: Vec<String> = table
            .primary_key
            .iter()
            .map(|k| dialect.quote_ident(k))
            .collect();
        defs.push(format!("PRIMARY KEY ({})", keys.join(", ")));
    }
    format!(
        "CREATE TABLE {} (\n  {}\n)",
        dialect.quote_ident(&table.name),
        defs.join(",\n  ")
    )
}

/// Render `diff` as forward DDL (`left` -> `right`): dropped tables first
/// (nothing else references what no longer exists), then created tables,
/// then each changed table's column/index/constraint deltas. Views and
/// routines render as commented text blocks — the plan's recorded debt —
/// rather than structural `CREATE OR REPLACE` statements this crate does
/// not attempt to synthesize.
pub fn migration_script(diff: &SchemaDiff, dialect: Dialect) -> String {
    let mut lines = Vec::new();

    for table in &diff.dropped_tables {
        lines.push(format!("DROP TABLE {};", dialect.quote_ident(&table.name)));
    }
    for table in &diff.added_tables {
        lines.push(format!("{};", create_table_ddl(dialect, table)));
    }
    for table_diff in &diff.changed_tables {
        let table_ident = dialect.quote_ident(&table_diff.table);
        for change in &table_diff.columns {
            match change {
                ColumnChange::Added(column) => lines.push(format!(
                    "ALTER TABLE {table_ident} ADD COLUMN {};",
                    column_ddl(dialect, column)
                )),
                ColumnChange::Dropped(column) => lines.push(format!(
                    "ALTER TABLE {table_ident} DROP COLUMN {};",
                    dialect.quote_ident(&column.name)
                )),
                ColumnChange::Changed { after, .. } => lines.push(format!(
                    "ALTER TABLE {table_ident} ALTER COLUMN {};",
                    column_ddl(dialect, after)
                )),
            }
        }
        for index in &table_diff.dropped_indexes {
            lines.push(format!("DROP INDEX {};", dialect.quote_ident(&index.name)));
        }
        for index in &table_diff.added_indexes {
            let unique = if index.unique { "UNIQUE " } else { "" };
            let columns: Vec<String> = index
                .columns
                .iter()
                .map(|c| dialect.quote_ident(c))
                .collect();
            lines.push(format!(
                "CREATE {unique}INDEX {} ON {table_ident} ({});",
                dialect.quote_ident(&index.name),
                columns.join(", ")
            ));
        }
        for constraint in &table_diff.dropped_constraints {
            lines.push(format!(
                "ALTER TABLE {table_ident} DROP CONSTRAINT {};",
                dialect.quote_ident(&constraint.name)
            ));
        }
        for constraint in &table_diff.added_constraints {
            lines.push(format!(
                "ALTER TABLE {table_ident} ADD CONSTRAINT {} {};",
                dialect.quote_ident(&constraint.name),
                constraint.text
            ));
        }
    }
    for view in &diff.views {
        lines.push(format!(
            "-- view {} changed — review and apply by hand:\n-- before: {}\n-- after:  {}",
            view.name,
            view.before.as_deref().unwrap_or("(none)"),
            view.after.as_deref().unwrap_or("(none)")
        ));
    }
    for routine in &diff.routines {
        lines.push(format!(
            "-- routine {} changed — review and apply by hand:\n-- before: {}\n-- after:  {}",
            routine.name,
            routine.before.as_deref().unwrap_or("(none)"),
            routine.after.as_deref().unwrap_or("(none)")
        ));
    }
    lines.join("\n")
}

/// What one `ddl_pairs` row names: a table, or a view/routine (no table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    pub table: Option<String>,
    pub name: String,
}

/// One left/right DDL text pair per changed object, for the DiffView —
/// an empty string on the side an added/dropped table has nothing on.
pub fn ddl_pairs(diff: &SchemaDiff, dialect: Dialect) -> Vec<(ObjectRef, String, String)> {
    let mut pairs = Vec::new();
    for table in &diff.dropped_tables {
        pairs.push((
            ObjectRef {
                table: None,
                name: table.name.clone(),
            },
            create_table_ddl(dialect, table),
            String::new(),
        ));
    }
    for table in &diff.added_tables {
        pairs.push((
            ObjectRef {
                table: None,
                name: table.name.clone(),
            },
            String::new(),
            create_table_ddl(dialect, table),
        ));
    }
    for table_diff in &diff.changed_tables {
        pairs.push((
            ObjectRef {
                table: None,
                name: table_diff.table.clone(),
            },
            create_table_ddl(dialect, &table_diff.before),
            create_table_ddl(dialect, &table_diff.after),
        ));
    }
    for view in &diff.views {
        pairs.push((
            ObjectRef {
                table: None,
                name: view.name.clone(),
            },
            view.before.clone().unwrap_or_default(),
            view.after.clone().unwrap_or_default(),
        ));
    }
    for routine in &diff.routines {
        pairs.push((
            ObjectRef {
                table: None,
                name: routine.name.clone(),
            },
            routine.before.clone().unwrap_or_default(),
            routine.after.clone().unwrap_or_default(),
        ));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, type_name: &str, nullable: bool) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            type_name: type_name.to_string(),
            nullable,
            default: None,
        }
    }

    fn table(name: &str, columns: Vec<ColumnDef>) -> TableDef {
        TableDef {
            name: name.to_string(),
            columns,
            primary_key: vec!["id".to_string()],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
        }
    }

    #[test]
    fn an_added_table_is_reported_and_not_a_column_change() {
        let left = SchemaSnapshot::default();
        let right = SchemaSnapshot {
            tables: vec![table("users", vec![column("id", "INTEGER", false)])],
            ..Default::default()
        };
        let diff = compare(&left, &right);
        assert_eq!(diff.added_tables.len(), 1);
        assert_eq!(diff.added_tables[0].name, "users");
        assert!(diff.dropped_tables.is_empty());
        assert!(diff.changed_tables.is_empty());
    }

    #[test]
    fn a_dropped_table_is_reported() {
        let left = SchemaSnapshot {
            tables: vec![table("users", vec![column("id", "INTEGER", false)])],
            ..Default::default()
        };
        let right = SchemaSnapshot::default();
        let diff = compare(&left, &right);
        assert_eq!(diff.dropped_tables.len(), 1);
    }

    #[test]
    fn a_column_type_and_nullability_change_is_reported() {
        let left = SchemaSnapshot {
            tables: vec![table(
                "users",
                vec![
                    column("id", "INTEGER", false),
                    column("age", "INTEGER", true),
                ],
            )],
            ..Default::default()
        };
        let right = SchemaSnapshot {
            tables: vec![table(
                "users",
                vec![
                    column("id", "INTEGER", false),
                    column("age", "BIGINT", false),
                ],
            )],
            ..Default::default()
        };
        let diff = compare(&left, &right);
        assert_eq!(diff.changed_tables.len(), 1);
        let change = &diff.changed_tables[0].columns[0];
        assert!(matches!(change, ColumnChange::Changed { .. }));
    }

    #[test]
    fn an_identical_schema_produces_an_empty_diff() {
        let snapshot = SchemaSnapshot {
            tables: vec![table("users", vec![column("id", "INTEGER", false)])],
            ..Default::default()
        };
        let diff = compare(&snapshot, &snapshot);
        assert!(diff.is_empty());
    }

    #[test]
    fn migration_script_orders_drops_before_creates() {
        let left = SchemaSnapshot {
            tables: vec![table("old", vec![column("id", "INTEGER", false)])],
            ..Default::default()
        };
        let right = SchemaSnapshot {
            tables: vec![table("new", vec![column("id", "INTEGER", false)])],
            ..Default::default()
        };
        let script = migration_script(&compare(&left, &right), Dialect::Postgres);
        let drop_pos = script.find("DROP TABLE").unwrap();
        let create_pos = script.find("CREATE TABLE").unwrap();
        assert!(drop_pos < create_pos);
    }

    #[test]
    fn migration_script_emits_alter_statements_for_column_changes() {
        let left = SchemaSnapshot {
            tables: vec![table("users", vec![column("id", "INTEGER", false)])],
            ..Default::default()
        };
        let right = SchemaSnapshot {
            tables: vec![table(
                "users",
                vec![
                    column("id", "INTEGER", false),
                    column("email", "TEXT", true),
                ],
            )],
            ..Default::default()
        };
        let script = migration_script(&compare(&left, &right), Dialect::Postgres);
        assert!(script.contains("ALTER TABLE \"users\" ADD COLUMN \"email\" TEXT"));
    }

    #[test]
    fn a_view_change_renders_as_a_commented_text_block_not_ddl() {
        let left = SchemaSnapshot {
            views: vec![TextObject {
                name: "v1".to_string(),
                definition: "SELECT 1".to_string(),
            }],
            ..Default::default()
        };
        let right = SchemaSnapshot {
            views: vec![TextObject {
                name: "v1".to_string(),
                definition: "SELECT 2".to_string(),
            }],
            ..Default::default()
        };
        let diff = compare(&left, &right);
        assert_eq!(diff.views.len(), 1);
        let script = migration_script(&diff, Dialect::Postgres);
        assert!(script.contains("-- view v1 changed"));
        assert!(!script.to_uppercase().contains("CREATE OR REPLACE VIEW"));
    }

    #[test]
    fn ddl_pairs_gives_before_and_after_text_for_a_changed_table() {
        let left = SchemaSnapshot {
            tables: vec![table("users", vec![column("id", "INTEGER", false)])],
            ..Default::default()
        };
        let right = SchemaSnapshot {
            tables: vec![table(
                "users",
                vec![
                    column("id", "INTEGER", false),
                    column("email", "TEXT", true),
                ],
            )],
            ..Default::default()
        };
        let diff = compare(&left, &right);
        let pairs = ddl_pairs(&diff, Dialect::Postgres);
        assert_eq!(pairs.len(), 1);
        assert!(!pairs[0].1.contains("email"));
        assert!(pairs[0].2.contains("email"));
    }

    /// Read one open SQLite connection's tables into a [`SchemaSnapshot`]
    /// via `PRAGMA table_info`/`PRAGMA foreign_key_list` — a minimal,
    /// test-only stand-in for the real introspection a live `Connection`
    /// will grow later (F2/F7); it exists here only to prove `compare`
    /// against a real database, not just hand-built snapshots.
    fn snapshot_from_sqlite(conn: &rusqlite::Connection) -> SchemaSnapshot {
        let mut table_stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let table_names: Vec<String> = table_stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        let mut tables = Vec::new();
        for table_name in table_names {
            let mut col_stmt = conn
                .prepare(&format!("PRAGMA table_info({table_name})"))
                .unwrap();
            let mut columns = Vec::new();
            let mut primary_key = Vec::new();
            let rows = col_stmt
                .query_map([], |row| {
                    let name: String = row.get(1)?;
                    let type_name: String = row.get(2)?;
                    let not_null: i64 = row.get(3)?;
                    let pk: i64 = row.get(5)?;
                    Ok((name, type_name, not_null == 0, pk > 0))
                })
                .unwrap();
            for row in rows {
                let (name, type_name, nullable, pk) = row.unwrap();
                if pk {
                    primary_key.push(name.clone());
                }
                columns.push(ColumnDef {
                    name,
                    type_name,
                    nullable,
                    default: None,
                });
            }
            tables.push(TableDef {
                name: table_name,
                columns,
                primary_key,
                foreign_keys: vec![],
                indexes: vec![],
                constraints: vec![],
            });
        }
        SchemaSnapshot {
            tables,
            views: vec![],
            routines: vec![],
        }
    }

    #[test]
    fn a_real_sqlite_vs_sqlite_run_detects_an_added_column() {
        let left_conn = rusqlite::Connection::open_in_memory().unwrap();
        left_conn
            .execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
            .unwrap();
        let right_conn = rusqlite::Connection::open_in_memory().unwrap();
        right_conn
            .execute(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)",
                [],
            )
            .unwrap();

        let left = snapshot_from_sqlite(&left_conn);
        let right = snapshot_from_sqlite(&right_conn);
        let diff = compare(&left, &right);

        assert_eq!(diff.changed_tables.len(), 1);
        assert!(diff.changed_tables[0]
            .columns
            .iter()
            .any(|c| matches!(c, ColumnChange::Added(col) if col.name == "email")));

        let script = migration_script(&diff, Dialect::Sqlite);
        assert!(script.contains("ADD COLUMN \"email\""));
    }
}
