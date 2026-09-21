//! CSV export: RFC 4180 quoting (embedded quotes, delimiters and newlines
//! all round-trip) via the `csv` crate rather than hand-rolled escaping —
//! quoting a text field correctly is exactly the kind of "looks easy,
//! isn't" logic a maintained crate already gets right.

use std::io::{self, Write};

use csv::{QuoteStyle, WriterBuilder};
use db_core::value::ColumnMeta;

use super::{csv_io_err, ExportOptions, RowSource};

pub fn write(
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn Write,
    options: &ExportOptions,
) -> io::Result<()> {
    let rules = options.format_rules();
    let mut csv_writer = WriterBuilder::new()
        .delimiter(options.delimiter)
        .quote_style(if options.quote_all {
            QuoteStyle::Always
        } else {
            QuoteStyle::Necessary
        })
        .from_writer(writer);

    if options.header {
        csv_writer
            .write_record(columns.iter().map(|c| c.name.as_str()))
            .map_err(csv_io_err)?;
    }
    for batch in rows {
        for row in &batch.rows {
            let record: Vec<String> = row.iter().map(|value| value.display(&rules)).collect();
            csv_writer.write_record(&record).map_err(csv_io_err)?;
        }
    }
    csv_writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::value::{RowBatch, Value};

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
    fn plain_values_need_no_quoting() {
        let text = export(
            vec![vec![Value::Int(1), Value::Text("hello".to_string())]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "id,note\n1,hello\n");
    }

    #[test]
    fn an_embedded_double_quote_is_rfc4180_escaped() {
        let text = export(
            vec![vec![Value::Int(1), Value::Text("a\"b".to_string())]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "id,note\n1,\"a\"\"b\"\n");
    }

    #[test]
    fn a_sql_injection_lookalike_stays_one_inert_field() {
        let text = export(
            vec![vec![
                Value::Int(1),
                Value::Text("Robert'); DROP TABLE students;--".to_string()),
            ]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "id,note\n1,Robert'); DROP TABLE students;--\n");
    }

    #[test]
    fn a_newline_inside_a_cell_is_quoted_not_split_into_a_new_row() {
        let text = export(
            vec![vec![Value::Int(1), Value::Text("line1\nline2".to_string())]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "id,note\n1,\"line1\nline2\"\n");
        assert_eq!(text.matches('\n').count(), 3);
    }

    #[test]
    fn a_nul_byte_survives_without_panicking() {
        let text = export(
            vec![vec![Value::Int(1), Value::Text("a\0b".to_string())]],
            &ExportOptions::default(),
        );
        assert!(text.contains("a\0b"));
    }

    #[test]
    fn a_comma_triggers_quoting_when_the_delimiter_is_a_comma() {
        let text = export(
            vec![vec![Value::Int(1), Value::Text("a,b".to_string())]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "id,note\n1,\"a,b\"\n");
    }

    #[test]
    fn quote_all_quotes_every_field_even_plain_ones() {
        let options = ExportOptions {
            quote_all: true,
            ..ExportOptions::default()
        };
        let text = export(
            vec![vec![Value::Int(1), Value::Text("x".to_string())]],
            &options,
        );
        assert_eq!(text, "\"id\",\"note\"\n\"1\",\"x\"\n");
    }

    #[test]
    fn no_header_omits_the_column_row() {
        let options = ExportOptions {
            header: false,
            ..ExportOptions::default()
        };
        let text = export(
            vec![vec![Value::Int(1), Value::Text("x".to_string())]],
            &options,
        );
        assert_eq!(text, "1,x\n");
    }

    #[test]
    fn null_renders_as_the_configured_null_text() {
        let options = ExportOptions {
            null_text: "\\N".to_string(),
            ..ExportOptions::default()
        };
        let text = export(vec![vec![Value::Int(1), Value::Null]], &options);
        assert_eq!(text, "id,note\n1,\\N\n");
    }

    #[test]
    fn bytes_render_as_full_hex_never_truncated() {
        let text = export(
            vec![vec![Value::Int(1), Value::Bytes(vec![0xab; 40])]],
            &ExportOptions::default(),
        );
        assert!(text.contains(&"ab".repeat(40)));
        assert!(!text.contains('\u{2026}'));
    }
}
