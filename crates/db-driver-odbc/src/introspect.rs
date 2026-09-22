//! `tables()`/`columns()`/`primary_keys()`/`foreign_keys()` → `SchemaSnapshot`
//! (F8.2), all four exposed directly by `odbc-api`'s safe `Connection`
//! wrappers around `SQLTables`/`SQLColumns`/`SQLPrimaryKeys`/`SQLForeignKeys`
//! — no raw `odbc_sys` calls needed (a positive deviation from the plan's
//! "use raw calls for PK/FK if needed": odbc-api 29 already has them).
//!
//! `IntrospectScope` carries no depth field of its own (only
//! catalog/schema filters) — `db_core::schema`'s own doc comment puts the
//! "stay near-instant on a large catalog" requirement on `IntrospectLevel`
//! instead, which is the caller's concern when it decides how far to walk
//! the returned tree. This driver always fetches one table's columns and
//! keys eagerly (both are one ODBC round trip per table regardless), so
//! every call returns an `IntrospectLevel::Full` snapshot.

use std::collections::BTreeMap;
use std::sync::Mutex;

use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{IntrospectLevel, IntrospectScope, Node, ObjectKind, SchemaSnapshot};

use odbc_api::Connection as OdbcRawConnection;

fn odbc_error(e: odbc_api::Error) -> DbError {
    DbError::new(DbErrorCode::Unknown, e.to_string())
}

pub fn introspect(
    shared: &Mutex<OdbcRawConnection<'static>>,
    scope: &IntrospectScope,
) -> Result<SchemaSnapshot, DbError> {
    let guard = shared.lock().map_err(|_| {
        DbError::new(
            DbErrorCode::Unknown,
            "ODBC connection mutex poisoned by a previous panic",
        )
    })?;

    let catalog_filter = scope.catalog.as_deref().unwrap_or("");
    let schema_filter = scope.schema.as_deref().unwrap_or("");

    // `catalog -> schema -> [(table, table_type)]`, in catalog order — a
    // `BTreeMap` keeps every level's iteration order stable across runs,
    // which matters for a tree the user expects to stop jumping around
    // between refreshes.
    let mut tree: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>> = BTreeMap::new();
    let rows = guard
        .tables(catalog_filter, schema_filter, "", "TABLE,VIEW")
        .map_err(odbc_error)?;
    for row in rows {
        let row = row.map_err(odbc_error)?;
        let catalog = row
            .catalog
            .as_str()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();
        let schema = row
            .schema
            .as_str()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();
        let table = row
            .table
            .as_str()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();
        let table_type = row
            .table_type
            .as_str()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();
        tree.entry(catalog)
            .or_default()
            .entry(schema)
            .or_default()
            .push((table, table_type));
    }

    let mut roots = Vec::with_capacity(tree.len());
    for (catalog, schemas) in tree {
        let mut schema_nodes = Vec::with_capacity(schemas.len());
        for (schema, tables) in schemas {
            let mut table_nodes = Vec::with_capacity(tables.len());
            for (table, table_type) in tables {
                let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                };
                table_nodes.push(build_table_node(&guard, &catalog, &schema, &table, kind)?);
            }
            schema_nodes.push(Node::with_children(schema, ObjectKind::Schema, table_nodes));
        }
        roots.push(Node::with_children(
            catalog,
            ObjectKind::Catalog,
            schema_nodes,
        ));
    }

    Ok(SchemaSnapshot::new(IntrospectLevel::Full, roots))
}

fn build_table_node(
    guard: &OdbcRawConnection<'static>,
    catalog: &str,
    schema: &str,
    table: &str,
    kind: ObjectKind,
) -> Result<Node, DbError> {
    let mut children = Vec::new();

    let columns = guard
        .columns(catalog, schema, table, "")
        .map_err(odbc_error)?;
    for row in columns {
        let row = row.map_err(odbc_error)?;
        let name = row
            .column_name
            .as_str()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();
        children.push(Node::leaf(name, ObjectKind::Column));
    }

    let primary_keys = guard
        .primary_keys(Some(catalog), Some(schema), table)
        .map_err(odbc_error)?;
    let mut pk_columns = Vec::new();
    for row in primary_keys {
        let row = row.map_err(odbc_error)?;
        pk_columns.push(
            row.column
                .as_str()
                .ok()
                .flatten()
                .unwrap_or_default()
                .to_string(),
        );
    }
    if !pk_columns.is_empty() {
        children.push(Node::leaf(
            format!("PRIMARY KEY ({})", pk_columns.join(", ")),
            ObjectKind::Constraint,
        ));
    }

    let foreign_keys = guard
        .foreign_keys("", "", table, "", "", "")
        .map_err(odbc_error)?;
    for row in foreign_keys {
        let row = row.map_err(odbc_error)?;
        let fk_table = row
            .fk_table
            .as_str()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string();
        // `foreign_keys` with only `pk_table_name` set returns both the
        // keys this table exports (as a PK) and the ones it imports (as
        // an FK); keep this node to the ones this table actually declares.
        if fk_table != table {
            continue;
        }
        let fk_column = row.fk_column.as_str().ok().flatten().unwrap_or_default();
        let pk_table = row.pk_table.as_str().ok().flatten().unwrap_or_default();
        let pk_column = row.pk_column.as_str().ok().flatten().unwrap_or_default();
        children.push(Node::leaf(
            format!("FOREIGN KEY {fk_column} -> {pk_table}.{pk_column}"),
            ObjectKind::Constraint,
        ));
    }

    Ok(Node::with_children(table.to_string(), kind, children))
}
