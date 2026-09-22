//! HTML table export: one `<table>`, escaped so a cell's own text can
//! never inject markup.

use std::io::{self, Write};

use db_core::value::ColumnMeta;

use super::{ExportOptions, RowSource};

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn write(
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn Write,
    options: &ExportOptions,
) -> io::Result<()> {
    let rules = options.format_rules();
    writeln!(writer, "<table>")?;
    if options.header {
        writeln!(writer, "<thead><tr>")?;
        for column in columns {
            writeln!(writer, "<th>{}</th>", escape(&column.name))?;
        }
        writeln!(writer, "</tr></thead>")?;
    }
    writeln!(writer, "<tbody>")?;
    for batch in rows {
        for row in &batch.rows {
            writeln!(writer, "<tr>")?;
            for value in row {
                writeln!(writer, "<td>{}</td>", escape(&value.display(&rules)))?;
            }
            writeln!(writer, "</tr>")?;
        }
    }
    writeln!(writer, "</tbody></table>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::value::{RowBatch, Value};

    fn columns() -> Vec<ColumnMeta> {
        vec![ColumnMeta {
            name: "name".to_string(),
            type_name: "text".to_string(),
            nullable: false,
            origin: None,
        }]
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
    fn header_and_rows_render_as_a_table() {
        let text = export(
            vec![vec![Value::Text("alice".to_string())]],
            &ExportOptions::default(),
        );
        assert!(text.contains("<th>name</th>"));
        assert!(text.contains("<td>alice</td>"));
    }

    #[test]
    fn markup_in_a_cell_is_escaped_not_injected() {
        let text = export(
            vec![vec![Value::Text("<script>alert(1)</script>".to_string())]],
            &ExportOptions::default(),
        );
        assert!(!text.contains("<script>"));
        assert!(text.contains("&lt;script&gt;"));
    }

    #[test]
    fn an_ampersand_is_escaped_before_anything_else() {
        let text = export(
            vec![vec![Value::Text("a & b".to_string())]],
            &ExportOptions::default(),
        );
        assert!(text.contains("a &amp; b"));
    }

    #[test]
    fn header_false_omits_the_thead() {
        let options = ExportOptions {
            header: false,
            ..ExportOptions::default()
        };
        let text = export(vec![vec![Value::Text("x".to_string())]], &options);
        assert!(!text.contains("<thead>"));
    }
}
