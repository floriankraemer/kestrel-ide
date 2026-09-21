//! SQL export: `INSERT`/`UPDATE` statements a user can paste into a
//! console and run. Every identifier goes through
//! [`Dialect::quote_ident`], every value through [`sql_literal`] —
//! neither is display text, both are what actually has to parse back as
//! the right SQL grammar (`quote_literal` doubles an embedded `'`,
//! [`sql_literal`] never hands a hex/blob value back unquoted).

use std::io::{self, Write};

use db_core::dialect::Dialect;
use db_core::value::{ColumnMeta, Value};

use super::{ExportOptions, RowSource, SqlMode};

/// Render `value` as a literal this dialect's parser accepts — never a
/// value bound as a driver parameter, since a paste-into-console export
/// has no parameter slot to bind against.
fn sql_literal(value: &Value, dialect: Dialect) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Decimal(text) => text.clone(),
        Value::Text(text) | Value::Json(text) => dialect.quote_literal(text),
        Value::Bytes(bytes) => hex_literal(bytes, dialect),
        Value::Date(date) => dialect.quote_literal(&date.to_string()),
        Value::Time(time) => dialect.quote_literal(&time.to_string()),
        Value::DateTime(dt) => dialect.quote_literal(&dt.to_string()),
        Value::DateTimeTz(dt) => dialect.quote_literal(&dt.to_rfc3339()),
        Value::Uuid(id) => dialect.quote_literal(&id.to_string()),
        Value::Array(_) | Value::Document(_) | Value::Other { .. } => {
            dialect.quote_literal(&value.display(&Default::default()))
        }
    }
}

/// A hex blob literal in the syntax each dialect's parser accepts —
/// SQLite/MySQL/Cassandra `X'..'`, SQL Server `0x..`, Postgres's own
/// `'\x..'` bytea escape. Mongo/Redis have no SQL grammar to begin with;
/// falling back to their own quoted-string form keeps this total rather
/// than panicking on a dialect this export format was never meant to
/// target.
fn hex_literal(bytes: &[u8], dialect: Dialect) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    match dialect {
        Dialect::SqlServer => format!("0x{hex}"),
        Dialect::Postgres => format!("'\\x{hex}'"),
        Dialect::MySql | Dialect::Sqlite | Dialect::Cassandra => format!("X'{hex}'"),
        Dialect::Mongo | Dialect::Redis => dialect.quote_literal(&hex),
    }
}

pub fn write(
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn Write,
    options: &ExportOptions,
) -> io::Result<()> {
    let table = options.dialect.quote_ident(&options.table_name);
    let quoted_columns: Vec<String> = columns
        .iter()
        .map(|c| options.dialect.quote_ident(&c.name))
        .collect();

    for batch in rows {
        for row in &batch.rows {
            match &options.sql {
                SqlMode::Insert => {
                    let values: Vec<String> = row
                        .iter()
                        .map(|value| sql_literal(value, options.dialect))
                        .collect();
                    writeln!(
                        writer,
                        "INSERT INTO {table} ({}) VALUES ({});",
                        quoted_columns.join(", "),
                        values.join(", ")
                    )?;
                }
                SqlMode::Update { key_columns } => {
                    let mut sets = Vec::new();
                    let mut wheres = Vec::new();
                    for (column, value) in columns.iter().zip(row.iter()) {
                        let rendered = sql_literal(value, options.dialect);
                        let ident = options.dialect.quote_ident(&column.name);
                        if key_columns.iter().any(|k| k == &column.name) {
                            wheres.push(format!("{ident} = {rendered}"));
                        } else {
                            sets.push(format!("{ident} = {rendered}"));
                        }
                    }
                    writeln!(
                        writer,
                        "UPDATE {table} SET {} WHERE {};",
                        sets.join(", "),
                        wheres.join(" AND ")
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::value::RowBatch;

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta {
                name: "id".to_string(),
                type_name: "int".to_string(),
                nullable: false,
            },
            ColumnMeta {
                name: "name".to_string(),
                type_name: "text".to_string(),
                nullable: true,
            },
        ]
    }

    fn export(rows: Vec<Vec<Value>>, options: &ExportOptions) -> String {
        let batch = RowBatch {
            columns: columns(),
            rows,
        };
        let mut out = Vec::new();
        write(&columns(), &mut vec![batch].into_iter(), &mut out, options).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn insert_mode_renders_one_statement_per_row() {
        let options = ExportOptions {
            table_name: "users".to_string(),
            dialect: Dialect::Postgres,
            ..ExportOptions::default()
        };
        let text = export(
            vec![vec![Value::Int(1), Value::Text("alice".to_string())]],
            &options,
        );
        assert_eq!(
            text,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES (1, 'alice');\n"
        );
    }

    #[test]
    fn update_mode_splits_key_and_non_key_columns() {
        let options = ExportOptions {
            table_name: "users".to_string(),
            dialect: Dialect::MySql,
            sql: SqlMode::Update {
                key_columns: vec!["id".to_string()],
            },
            ..ExportOptions::default()
        };
        let text = export(
            vec![vec![Value::Int(1), Value::Text("alice".to_string())]],
            &options,
        );
        assert_eq!(
            text,
            "UPDATE `users` SET `name` = 'alice' WHERE `id` = 1;\n"
        );
    }

    #[test]
    fn an_embedded_single_quote_is_doubled_never_breaking_out() {
        let options = ExportOptions {
            table_name: "t".to_string(),
            dialect: Dialect::Postgres,
            ..ExportOptions::default()
        };
        let text = export(
            vec![vec![
                Value::Int(1),
                Value::Text("Robert'); DROP TABLE students;--".to_string()),
            ]],
            &options,
        );
        assert_eq!(
            text,
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES (1, 'Robert''); DROP TABLE students;--');\n"
        );
    }

    #[test]
    fn a_table_name_that_could_break_out_of_quoting_is_neutralised() {
        let options = ExportOptions {
            table_name: "Robert\"); DROP TABLE students;--".to_string(),
            dialect: Dialect::Postgres,
            ..ExportOptions::default()
        };
        let text = export(vec![vec![Value::Int(1), Value::Null]], &options);
        assert!(text.starts_with("INSERT INTO \"Robert\"\"); DROP TABLE students;--\""));
    }

    #[test]
    fn null_renders_as_the_sql_keyword_never_a_string() {
        let options = ExportOptions {
            table_name: "t".to_string(),
            ..ExportOptions::default()
        };
        let text = export(vec![vec![Value::Int(1), Value::Null]], &options);
        assert!(text.contains("NULL"));
        assert!(!text.contains("'NULL'"));
    }

    #[test]
    fn bytes_render_as_a_hex_literal_per_dialect() {
        assert_eq!(hex_literal(&[0xab, 0xcd], Dialect::MySql), "X'abcd'");
        assert_eq!(hex_literal(&[0xab, 0xcd], Dialect::SqlServer), "0xabcd");
        assert_eq!(hex_literal(&[0xab, 0xcd], Dialect::Postgres), "'\\xabcd'");
    }
}
