//! Decodes `Connection::get_objects`' fixed nested schema (documented on
//! `adbc_core::options::Connection::get_objects`) into `SchemaSnapshot` —
//! catalogs/schemas/tables/columns only; constraints are a known gap (see
//! `crate::driver`'s doc comment).

use arrow_array::cast::AsArray;
use arrow_array::{Array, RecordBatchReader};

use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{IntrospectLevel, Node, ObjectKind, SchemaSnapshot};

fn string_at(array: &dyn Array, row: usize) -> String {
    if array.is_null(row) {
        return String::new();
    }
    array.as_string::<i32>().value(row).to_string()
}

fn columns_of(table_struct: &arrow_array::StructArray, row: usize) -> Vec<Node> {
    let Some(columns_field_index) = table_struct
        .fields()
        .iter()
        .position(|f| f.name() == "table_columns")
    else {
        return Vec::new();
    };
    let columns_list = table_struct.column(columns_field_index).as_list::<i32>();
    if columns_list.is_null(row) {
        return Vec::new();
    }
    let columns_struct = columns_list.value(row);
    let columns_struct = columns_struct.as_struct();
    let Some(name_index) = columns_struct
        .fields()
        .iter()
        .position(|f| f.name() == "column_name")
    else {
        return Vec::new();
    };
    let names = columns_struct.column(name_index);
    (0..columns_struct.len())
        .map(|i| Node::leaf(string_at(names.as_ref(), i), ObjectKind::Column))
        .collect()
}

fn tables_of(schema_struct: &arrow_array::StructArray, row: usize) -> Vec<Node> {
    let Some(tables_field_index) = schema_struct
        .fields()
        .iter()
        .position(|f| f.name() == "db_schema_tables")
    else {
        return Vec::new();
    };
    let tables_list = schema_struct.column(tables_field_index).as_list::<i32>();
    if tables_list.is_null(row) {
        return Vec::new();
    }
    let tables_struct = tables_list.value(row);
    let tables_struct = tables_struct.as_struct();
    let name_index = tables_struct
        .fields()
        .iter()
        .position(|f| f.name() == "table_name");
    let type_index = tables_struct
        .fields()
        .iter()
        .position(|f| f.name() == "table_type");
    (0..tables_struct.len())
        .map(|i| {
            let name = name_index
                .map(|idx| string_at(tables_struct.column(idx).as_ref(), i))
                .unwrap_or_default();
            let table_type = type_index
                .map(|idx| string_at(tables_struct.column(idx).as_ref(), i))
                .unwrap_or_default();
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            Node::with_children(name, kind, columns_of(tables_struct, i))
        })
        .collect()
}

fn schemas_of(catalog_struct_batch: &dyn Array, row: usize) -> Vec<Node> {
    let schemas_list = catalog_struct_batch.as_list::<i32>();
    if schemas_list.is_null(row) {
        return Vec::new();
    }
    let schemas_struct = schemas_list.value(row);
    let schemas_struct = schemas_struct.as_struct();
    let name_index = schemas_struct
        .fields()
        .iter()
        .position(|f| f.name() == "db_schema_name");
    (0..schemas_struct.len())
        .map(|i| {
            let name = name_index
                .map(|idx| string_at(schemas_struct.column(idx).as_ref(), i))
                .unwrap_or_default();
            Node::with_children(name, ObjectKind::Schema, tables_of(schemas_struct, i))
        })
        .collect()
}

pub fn from_get_objects(
    mut reader: Box<dyn RecordBatchReader + Send>,
) -> Result<SchemaSnapshot, DbError> {
    let mut roots = Vec::new();
    for batch in &mut reader {
        let batch = batch.map_err(|e| DbError::new(DbErrorCode::Unknown, e.to_string()))?;
        let Some(catalog_name_idx) = batch
            .schema()
            .fields()
            .iter()
            .position(|f| f.name() == "catalog_name")
        else {
            continue;
        };
        let Some(schemas_idx) = batch
            .schema()
            .fields()
            .iter()
            .position(|f| f.name() == "catalog_db_schemas")
        else {
            continue;
        };
        let catalog_names = batch.column(catalog_name_idx);
        let schemas_column = batch.column(schemas_idx);
        for row in 0..batch.num_rows() {
            let name = string_at(catalog_names.as_ref(), row);
            roots.push(Node::with_children(
                name,
                ObjectKind::Catalog,
                schemas_of(schemas_column.as_ref(), row),
            ));
        }
    }
    Ok(SchemaSnapshot::new(IntrospectLevel::Full, roots))
}
