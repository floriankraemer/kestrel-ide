//! JSON export: one row -> one JSON object, keyed by column name, either
//! wrapped in an array or emitted one-per-line (JSON Lines). `Value`'s own
//! recursive cases (`Array`/`Document`) map onto real nested JSON rather
//! than a stringified blob, since the whole point of a JSON export is a
//! consumer that already understands JSON structure.

use std::io::{self, Write};

use db_core::value::{ColumnMeta, FormatRules, Value};
use serde_json::{Map, Number, Value as Json};

use super::{ExportOptions, JsonShape, RowSource};

fn value_to_json(value: &Value, rules: &FormatRules) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Number((*i).into()),
        Value::Float(f) => Number::from_f64(*f).map(Json::Number).unwrap_or(Json::Null),
        // `Decimal` keeps arbitrary precision only as text — JSON has no
        // native decimal type, and re-parsing into a JSON number would
        // round it exactly the way `Value::Decimal`'s own doc comment
        // says never to.
        Value::Decimal(text) => Json::String(text.clone()),
        Value::Text(text) => Json::String(text.clone()),
        Value::Bytes(_) => Json::String(value.display(rules)),
        Value::Date(_) | Value::Time(_) | Value::DateTime(_) | Value::DateTimeTz(_) => {
            Json::String(value.display(rules))
        }
        Value::Uuid(id) => Json::String(id.to_string()),
        // Already-serialised JSON text: parse it back into a structured
        // value so it nests as real JSON rather than an escaped string;
        // fall back to the raw text if it somehow is not valid JSON.
        Value::Json(text) => serde_json::from_str(text).unwrap_or(Json::String(text.clone())),
        Value::Array(items) => Json::Array(
            items
                .iter()
                .map(|item| value_to_json(item, rules))
                .collect(),
        ),
        Value::Document(fields) => {
            let mut map = Map::new();
            for (name, field) in fields {
                map.insert(name.clone(), value_to_json(field, rules));
            }
            Json::Object(map)
        }
        Value::Other { display, .. } => Json::String(display.clone()),
    }
}

fn row_to_object(columns: &[ColumnMeta], row: &[Value], rules: &FormatRules) -> Json {
    let mut map = Map::new();
    for (column, value) in columns.iter().zip(row.iter()) {
        map.insert(column.name.clone(), value_to_json(value, rules));
    }
    Json::Object(map)
}

pub fn write(
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn Write,
    options: &ExportOptions,
) -> io::Result<()> {
    let rules = options.format_rules();
    let mut first = true;
    if options.json == JsonShape::Array {
        write!(writer, "[")?;
    }
    for batch in rows {
        for row in &batch.rows {
            let object = row_to_object(columns, row, &rules);
            match options.json {
                JsonShape::Array => {
                    if !first {
                        write!(writer, ",")?;
                    }
                    serde_json::to_writer(&mut *writer, &object).map_err(io::Error::other)?;
                }
                JsonShape::Lines => {
                    serde_json::to_writer(&mut *writer, &object).map_err(io::Error::other)?;
                    writeln!(writer)?;
                }
            }
            first = false;
        }
    }
    if options.json == JsonShape::Array {
        write!(writer, "]")?;
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
                name: "note".to_string(),
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
    fn array_mode_wraps_rows_in_a_json_array() {
        let text = export(
            vec![
                vec![Value::Int(1), Value::Text("a".to_string())],
                vec![Value::Int(2), Value::Null],
            ],
            &ExportOptions::default(),
        );
        let parsed: Json = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!([{"id": 1, "note": "a"}, {"id": 2, "note": null}])
        );
    }

    #[test]
    fn lines_mode_emits_one_object_per_line() {
        let options = ExportOptions {
            json: JsonShape::Lines,
            ..ExportOptions::default()
        };
        let text = export(
            vec![
                vec![Value::Int(1), Value::Text("a".to_string())],
                vec![Value::Int(2), Value::Text("b".to_string())],
            ],
            &options,
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            serde_json::from_str::<Json>(line).unwrap();
        }
    }

    #[test]
    fn an_embedded_quote_and_control_characters_are_escaped_not_broken() {
        let text = export(
            vec![vec![Value::Int(1), Value::Text("a\"b\nc".to_string())]],
            &ExportOptions::default(),
        );
        let parsed: Json = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed[0]["note"], Json::String("a\"b\nc".to_string()));
    }

    #[test]
    fn decimal_stays_exact_text_never_reparsed_as_a_float() {
        let text = export(
            vec![vec![
                Value::Int(1),
                Value::Decimal("12345678901234567890.123456789".to_string()),
            ]],
            &ExportOptions::default(),
        );
        assert!(text.contains("\"12345678901234567890.123456789\""));
    }

    #[test]
    fn document_and_array_values_nest_as_real_json_not_a_stringified_blob() {
        let text = export(
            vec![vec![
                Value::Int(1),
                Value::Document(vec![("a".to_string(), Value::Int(1))]),
            ]],
            &ExportOptions::default(),
        );
        let parsed: Json = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed[0]["note"], serde_json::json!({"a": 1}));
    }
}
