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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{RecordBatch, RecordBatchIterator, StringArray, StructArray};
    use arrow_buffer::OffsetBuffer;
    use arrow_schema::{DataType, Field, Fields, Schema};
    use db_core::schema::Children;
    use std::sync::Arc;

    /// Builds a one-catalog/one-schema/one-table/two-column `RecordBatch`
    /// matching `Connection::get_objects`' documented nested schema, by
    /// hand rather than through a real ADBC driver — the same "fake" the
    /// task calls for.
    fn one_catalog_one_schema_one_table_two_columns() -> RecordBatch {
        let column_name = StringArray::from(vec!["id", "name"]);
        let column_fields = Fields::from(vec![Field::new("column_name", DataType::Utf8, true)]);
        let columns_struct =
            StructArray::new(column_fields.clone(), vec![Arc::new(column_name)], None);
        let columns_list = ListArray_of_one(
            Field::new("item", DataType::Struct(column_fields), true),
            columns_struct,
            2,
        );

        let table_name = StringArray::from(vec!["t"]);
        let table_type = StringArray::from(vec!["TABLE"]);
        let table_fields = Fields::from(vec![
            Field::new("table_name", DataType::Utf8, true),
            Field::new("table_type", DataType::Utf8, true),
            Field::new("table_columns", columns_list.data_type().clone(), true),
        ]);
        let tables_struct = StructArray::new(
            table_fields.clone(),
            vec![
                Arc::new(table_name),
                Arc::new(table_type),
                Arc::new(columns_list),
            ],
            None,
        );
        let tables_list = ListArray_of_one(
            Field::new("item", DataType::Struct(table_fields), true),
            tables_struct,
            1,
        );

        let schema_name = StringArray::from(vec!["public"]);
        let schema_fields = Fields::from(vec![
            Field::new("db_schema_name", DataType::Utf8, true),
            Field::new("db_schema_tables", tables_list.data_type().clone(), true),
        ]);
        let schemas_struct = StructArray::new(
            schema_fields.clone(),
            vec![Arc::new(schema_name), Arc::new(tables_list)],
            None,
        );
        let schemas_list = ListArray_of_one(
            Field::new("item", DataType::Struct(schema_fields), true),
            schemas_struct,
            1,
        );

        let catalog_name = StringArray::from(vec!["main"]);
        let batch_schema = Arc::new(Schema::new(vec![
            Field::new("catalog_name", DataType::Utf8, true),
            Field::new("catalog_db_schemas", schemas_list.data_type().clone(), true),
        ]));
        RecordBatch::try_new(
            batch_schema,
            vec![Arc::new(catalog_name), Arc::new(schemas_list)],
        )
        .unwrap()
    }

    /// A single-row `ListArray` whose one list value is all of `values`
    /// (`len` items) — every list this fixture needs has exactly one row.
    #[allow(non_snake_case)]
    fn ListArray_of_one(
        item_field: Field,
        values: StructArray,
        len: i32,
    ) -> arrow_array::ListArray {
        arrow_array::ListArray::new(
            Arc::new(item_field),
            OffsetBuffer::new(vec![0i32, len].into()),
            Arc::new(values),
            None,
        )
    }

    #[test]
    fn decodes_catalogs_schemas_tables_and_columns_from_get_objects() {
        let batch = one_catalog_one_schema_one_table_two_columns();
        let schema = batch.schema();
        let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

        let snapshot = from_get_objects(Box::new(reader)).unwrap();

        assert_eq!(snapshot.roots.len(), 1);
        let catalog = &snapshot.roots[0];
        assert_eq!(catalog.name, "main");
        assert_eq!(catalog.kind, ObjectKind::Catalog);
        let Children::Loaded(schemas) = &catalog.children else {
            panic!("expected loaded schemas");
        };
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "public");
        let Children::Loaded(tables) = &schemas[0].children else {
            panic!("expected loaded tables");
        };
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "t");
        assert_eq!(tables[0].kind, ObjectKind::Table);
        let Children::Loaded(columns) = &tables[0].children else {
            panic!("expected loaded columns");
        };
        let column_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(column_names, vec!["id", "name"]);
        for column in columns {
            assert_eq!(column.kind, ObjectKind::Column);
        }
    }
}
