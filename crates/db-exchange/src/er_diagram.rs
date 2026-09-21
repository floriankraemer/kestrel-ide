//! ER diagram (F6.1): a [`SchemaSnapshot`] (optionally scoped to one
//! table plus its FK neighbours) -> Mermaid `erDiagram` text, rendered in
//! the preview dock the same way any other `.mmd` document is
//! (`database-tools.md` §4's "ER diagram" note: `PreviewService::render`
//! with a synthetic path, never a file on disk).

use std::collections::BTreeSet;

use crate::schema_model::{SchemaSnapshot, TableDef};

/// What part of a snapshot to draw: the whole schema, or one table plus
/// every table it's connected to by an FK (either direction) — a "show me
/// just this corner" scope for a large schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramScope {
    Schema,
    TableNeighbours(String),
}

/// Sanitize a name for use as a Mermaid identifier: anything other than
/// an ASCII letter, digit or underscore becomes `_`, and a leading digit
/// gets a `_` prefix. Mermaid's `erDiagram` entity/attribute names have no
/// quoting syntax of their own, so an odd identifier (spaces, punctuation)
/// cannot round-trip byte-for-byte — this keeps the diagram parseable
/// rather than emitting Mermaid syntax it would refuse to render.
fn mermaid_ident(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

fn tables_in_scope<'a>(snapshot: &'a SchemaSnapshot, scope: &DiagramScope) -> Vec<&'a TableDef> {
    match scope {
        DiagramScope::Schema => snapshot.tables.iter().collect(),
        DiagramScope::TableNeighbours(name) => {
            let Some(root) = snapshot.table(name) else {
                return Vec::new();
            };
            let mut names: BTreeSet<&str> = BTreeSet::new();
            names.insert(root.name.as_str());
            for fk in &root.foreign_keys {
                names.insert(fk.ref_table.as_str());
            }
            for table in &snapshot.tables {
                if table
                    .foreign_keys
                    .iter()
                    .any(|fk| fk.ref_table == root.name)
                {
                    names.insert(table.name.as_str());
                }
            }
            snapshot
                .tables
                .iter()
                .filter(|t| names.contains(t.name.as_str()))
                .collect()
        }
    }
}

/// Render `snapshot` (under `scope`) as Mermaid `erDiagram` text.
///
/// Deterministic: tables sort by name, and relationship lines are emitted
/// in sorted order too — so a golden-file test (or a diff between two
/// diagram runs) never flickers on iteration order alone.
pub fn to_mermaid(snapshot: &SchemaSnapshot, scope: &DiagramScope) -> String {
    let mut tables = tables_in_scope(snapshot, scope);
    tables.sort_by(|a, b| a.name.cmp(&b.name));
    let in_scope: BTreeSet<&str> = tables.iter().map(|t| t.name.as_str()).collect();

    let mut lines = vec!["erDiagram".to_string()];
    for table in &tables {
        lines.push(format!("    {} {{", mermaid_ident(&table.name)));
        for column in &table.columns {
            let mut flags = Vec::new();
            if table.primary_key.contains(&column.name) {
                flags.push("PK");
            }
            if table
                .foreign_keys
                .iter()
                .any(|fk| fk.columns.contains(&column.name))
            {
                flags.push("FK");
            }
            let suffix = if flags.is_empty() {
                String::new()
            } else {
                format!(" {}", flags.join(","))
            };
            lines.push(format!(
                "        {} {}{suffix}",
                mermaid_ident(&column.type_name),
                mermaid_ident(&column.name)
            ));
        }
        lines.push("    }".to_string());
    }

    let mut relationships = Vec::new();
    for table in &tables {
        for fk in &table.foreign_keys {
            if !in_scope.contains(fk.ref_table.as_str()) {
                continue;
            }
            let nullable = fk
                .columns
                .iter()
                .filter_map(|name| table.column(name))
                .any(|column| column.nullable);
            let right_token = if nullable { "o{" } else { "|{" };
            relationships.push(format!(
                "    {} ||--{right_token} {} : \"{}\"",
                mermaid_ident(&fk.ref_table),
                mermaid_ident(&table.name),
                fk.columns.join(",")
            ));
        }
    }
    relationships.sort();
    lines.extend(relationships);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_model::{ColumnDef, ForeignKey};

    fn column(name: &str, type_name: &str, nullable: bool) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            type_name: type_name.to_string(),
            nullable,
            default: None,
        }
    }

    fn snapshot() -> SchemaSnapshot {
        let users = TableDef {
            name: "users".to_string(),
            columns: vec![
                column("id", "INTEGER", false),
                column("name", "TEXT", false),
            ],
            primary_key: vec!["id".to_string()],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
        };
        let orders = TableDef {
            name: "orders".to_string(),
            columns: vec![
                column("id", "INTEGER", false),
                column("user_id", "INTEGER", true),
            ],
            primary_key: vec!["id".to_string()],
            foreign_keys: vec![ForeignKey {
                columns: vec!["user_id".to_string()],
                ref_table: "users".to_string(),
                ref_columns: vec!["id".to_string()],
            }],
            indexes: vec![],
            constraints: vec![],
        };
        SchemaSnapshot {
            tables: vec![orders, users],
            views: vec![],
            routines: vec![],
        }
    }

    #[test]
    fn entities_and_columns_render_with_pk_and_fk_flags() {
        let text = to_mermaid(&snapshot(), &DiagramScope::Schema);
        assert!(text.starts_with("erDiagram\n"));
        assert!(text.contains("INTEGER id PK"));
        assert!(text.contains("INTEGER user_id FK"));
    }

    #[test]
    fn a_relationship_line_renders_cardinality_from_fk_nullability() {
        let text = to_mermaid(&snapshot(), &DiagramScope::Schema);
        assert!(text.contains("users ||--o{ orders : \"user_id\""));
    }

    #[test]
    fn a_non_nullable_fk_renders_one_or_many_not_zero_or_many() {
        let mut snap = snapshot();
        snap.tables[0].columns[1].nullable = false; // orders.user_id
        let text = to_mermaid(&snap, &DiagramScope::Schema);
        assert!(text.contains("users ||--|{ orders"));
    }

    #[test]
    fn tables_render_in_sorted_name_order_regardless_of_input_order() {
        let text = to_mermaid(&snapshot(), &DiagramScope::Schema);
        let orders_pos = text.find("orders {").unwrap();
        let users_pos = text.find("users {").unwrap();
        assert!(orders_pos < users_pos);
    }

    #[test]
    fn table_neighbours_scope_excludes_unrelated_tables() {
        let mut snap = snapshot();
        snap.tables.push(TableDef {
            name: "unrelated".to_string(),
            columns: vec![column("id", "INTEGER", false)],
            primary_key: vec!["id".to_string()],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
        });
        let text = to_mermaid(&snap, &DiagramScope::TableNeighbours("orders".to_string()));
        assert!(text.contains("orders"));
        assert!(text.contains("users"));
        assert!(!text.contains("unrelated"));
    }

    #[test]
    fn an_identifier_with_odd_characters_stays_a_valid_mermaid_identifier() {
        assert_eq!(mermaid_ident("my table"), "my_table");
        assert_eq!(mermaid_ident("2fa"), "_2fa");
        assert_eq!(
            mermaid_ident("Robert'); DROP TABLE students;--"),
            "Robert____DROP_TABLE_students___"
        );
    }

    #[test]
    fn an_unknown_scope_root_renders_an_empty_diagram_rather_than_panicking() {
        let text = to_mermaid(
            &snapshot(),
            &DiagramScope::TableNeighbours("missing".to_string()),
        );
        assert_eq!(text, "erDiagram");
    }
}
