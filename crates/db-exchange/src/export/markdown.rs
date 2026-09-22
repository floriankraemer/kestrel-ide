//! Markdown table export (GitHub-flavoured pipe tables): a cell's own `|`
//! and `\` are escaped, and an embedded newline is flattened to a space —
//! a pipe-table cell cannot contain a raw line break at all.

use std::io::{self, Write};

use db_core::value::ColumnMeta;

use super::{ExportOptions, RowSource};

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', " ")
}

fn row_line(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

pub fn write(
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn Write,
    options: &ExportOptions,
) -> io::Result<()> {
    let rules = options.format_rules();
    if options.header {
        let names: Vec<String> = columns.iter().map(|c| escape(&c.name)).collect();
        writeln!(writer, "{}", row_line(&names))?;
        let separators: Vec<String> = columns.iter().map(|_| "---".to_string()).collect();
        writeln!(writer, "{}", row_line(&separators))?;
    }
    for batch in rows {
        for row in &batch.rows {
            let cells: Vec<String> = row
                .iter()
                .map(|value| escape(&value.display(&rules)))
                .collect();
            writeln!(writer, "{}", row_line(&cells))?;
        }
    }
    Ok(())
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
    fn header_and_separator_and_row_render_a_pipe_table() {
        let text = export(
            vec![vec![Value::Text("alice".to_string())]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "| name |\n| --- |\n| alice |\n");
    }

    #[test]
    fn an_embedded_pipe_is_escaped_not_read_as_a_new_column() {
        let text = export(
            vec![vec![Value::Text("a|b".to_string())]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "| name |\n| --- |\n| a\\|b |\n");
    }

    #[test]
    fn an_embedded_newline_is_flattened_to_a_space() {
        let text = export(
            vec![vec![Value::Text("a\nb".to_string())]],
            &ExportOptions::default(),
        );
        assert_eq!(text, "| name |\n| --- |\n| a b |\n");
    }
}
