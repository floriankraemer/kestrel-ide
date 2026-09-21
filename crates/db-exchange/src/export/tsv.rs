//! Tab-separated export: [`csv`](super::csv) with the delimiter forced to
//! a tab, regardless of what the caller's [`ExportOptions::delimiter`] is
//! set to — the same RFC 4180 quoting rules apply (a tab or newline inside
//! a field still gets quoted), so no separate escaping logic is needed.

use std::io::{self, Write};

use db_core::value::ColumnMeta;

use super::{csv, ExportOptions, RowSource};

pub fn write(
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn Write,
    options: &ExportOptions,
) -> io::Result<()> {
    let options = ExportOptions {
        delimiter: b'\t',
        ..options.clone()
    };
    csv::write(columns, rows, writer, &options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::value::{RowBatch, Value};

    #[test]
    fn fields_are_tab_separated() {
        let columns = vec![
            ColumnMeta {
                name: "a".to_string(),
                type_name: "int".to_string(),
                nullable: false,
            },
            ColumnMeta {
                name: "b".to_string(),
                type_name: "text".to_string(),
                nullable: false,
            },
        ];
        let batch = RowBatch {
            columns: columns.clone(),
            rows: vec![vec![Value::Int(1), Value::Text("x".to_string())]],
        };
        let mut out = Vec::new();
        write(
            &columns,
            &mut vec![batch].into_iter(),
            &mut out,
            &ExportOptions::default(),
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "a\tb\n1\tx\n");
    }

    #[test]
    fn a_tab_inside_a_cell_is_quoted() {
        let columns = vec![ColumnMeta {
            name: "a".to_string(),
            type_name: "text".to_string(),
            nullable: false,
        }];
        let batch = RowBatch {
            columns: columns.clone(),
            rows: vec![vec![Value::Text("x\ty".to_string())]],
        };
        let mut out = Vec::new();
        write(
            &columns,
            &mut vec![batch].into_iter(),
            &mut out,
            &ExportOptions::default(),
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "a\n\"x\ty\"\n");
    }
}
