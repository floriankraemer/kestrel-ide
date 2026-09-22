//! XLSX export via `rust_xlsxwriter`: numbers and dates keep their Excel
//! type (so a spreadsheet's own SUM/date arithmetic works on the export
//! without a re-import step), everything else — including a `Bytes`
//! value, which XLSX has no binary cell type for — renders as text.

use std::io::{self, Write};

use db_core::value::{ColumnMeta, Value};
use rust_xlsxwriter::{Format, Workbook, XlsxError};

use super::{ExportOptions, RowSource};

fn xlsx_io_err(error: XlsxError) -> io::Error {
    io::Error::other(error)
}

/// Excel decides a numeric cell is a *date* only from its cell format
/// (`numFmt`) — writing the bare serial number with no format (as
/// `Worksheet::write`'s default `IntoExcelData` impl for a chrono type
/// does) round-trips as a plain number, not a date, in any reader
/// (calamine included) that goes by the format rather than guessing from
/// the value's magnitude. These three formats are what make
/// `Value::Date`/`Time`/`DateTime` come back as real dates on read-back.
struct DateFormats {
    date: Format,
    time: Format,
    datetime: Format,
}

impl DateFormats {
    fn new() -> Self {
        Self {
            date: Format::new().set_num_format("yyyy-mm-dd"),
            time: Format::new().set_num_format("hh:mm:ss"),
            datetime: Format::new().set_num_format("yyyy-mm-dd hh:mm:ss"),
        }
    }
}

pub fn write(
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn Write,
    options: &ExportOptions,
) -> io::Result<()> {
    let rules = options.format_rules();
    let formats = DateFormats::new();
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();

    let mut row_index: u32 = 0;
    if options.header {
        for (col_index, column) in columns.iter().enumerate() {
            sheet
                .write_string(row_index, col_index as u16, &column.name)
                .map_err(xlsx_io_err)?;
        }
        row_index += 1;
    }

    for batch in rows {
        for row in &batch.rows {
            for (col_index, value) in row.iter().enumerate() {
                write_cell(sheet, row_index, col_index as u16, value, &rules, &formats)?;
            }
            row_index += 1;
        }
    }

    let bytes = workbook.save_to_buffer().map_err(xlsx_io_err)?;
    writer.write_all(&bytes)
}

fn write_cell(
    sheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    value: &Value,
    rules: &db_core::value::FormatRules,
    formats: &DateFormats,
) -> io::Result<()> {
    match value {
        Value::Null => sheet
            .write_string(row, col, &rules.null_text)
            .map_err(xlsx_io_err)?,
        Value::Bool(b) => sheet.write_boolean(row, col, *b).map_err(xlsx_io_err)?,
        Value::Int(i) => sheet
            .write_number(row, col, *i as f64)
            .map_err(xlsx_io_err)?,
        Value::Float(f) => sheet.write_number(row, col, *f).map_err(xlsx_io_err)?,
        Value::Date(date) => sheet
            .write_with_format(row, col, date, &formats.date)
            .map_err(xlsx_io_err)?,
        Value::Time(time) => sheet
            .write_with_format(row, col, time, &formats.time)
            .map_err(xlsx_io_err)?,
        Value::DateTime(dt) => sheet
            .write_with_format(row, col, dt, &formats.datetime)
            .map_err(xlsx_io_err)?,
        Value::DateTimeTz(dt) => sheet
            .write_with_format(row, col, &dt.naive_local(), &formats.datetime)
            .map_err(xlsx_io_err)?,
        // Everything else (text, decimal-as-text, bytes-as-hex, uuid,
        // json, array/document/other) renders through the same display
        // rules every non-XLSX exporter uses, as a plain string cell.
        _ => sheet
            .write_string(row, col, value.display(rules))
            .map_err(xlsx_io_err)?,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::{open_workbook_from_rs, Data, Reader, Xlsx};
    use chrono::NaiveDate;
    use db_core::value::RowBatch;
    use std::io::Cursor;

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta {
                name: "id".to_string(),
                type_name: "int".to_string(),
                nullable: false,
                origin: None,
            },
            ColumnMeta {
                name: "when".to_string(),
                type_name: "date".to_string(),
                nullable: false,
                origin: None,
            },
        ]
    }

    fn export_bytes(rows: Vec<Vec<Value>>) -> Vec<u8> {
        let batch = RowBatch {
            columns: columns(),
            rows,
        };
        let mut out = Vec::new();
        write(
            &columns(),
            &mut vec![batch].into_iter(),
            &mut out,
            &ExportOptions::default(),
        )
        .unwrap();
        out
    }

    #[test]
    fn numbers_and_dates_round_trip_as_their_own_excel_type() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let bytes = export_bytes(vec![vec![Value::Int(42), Value::Date(date)]]);

        let mut workbook: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes)).unwrap();
        let sheet = workbook.worksheet_range("Sheet1").unwrap();
        // Header row.
        assert_eq!(
            sheet.get_value((0, 0)),
            Some(&Data::String("id".to_string()))
        );
        // Data row: a real number, not a numeric-looking string.
        assert_eq!(sheet.get_value((1, 0)), Some(&Data::Float(42.0)));
        assert!(matches!(sheet.get_value((1, 1)), Some(Data::DateTime(_))));
    }

    #[test]
    fn bytes_render_as_hex_text_not_a_binary_cell() {
        let bytes = export_bytes(vec![vec![Value::Bytes(vec![0xde, 0xad]), Value::Int(1)]]);
        let mut workbook: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes)).unwrap();
        let sheet = workbook.worksheet_range("Sheet1").unwrap();
        assert_eq!(
            sheet.get_value((1, 0)),
            Some(&Data::String("dead".to_string()))
        );
    }
}
