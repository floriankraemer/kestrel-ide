//! XLSX source reading for import, via `calamine`: the first worksheet's
//! rows become the same raw-text shape [`super::csv`] produces, so
//! [`super::plan`]/[`super::run`] never need to know which format the
//! data came from.

use std::io::{Read, Seek};

use calamine::{Data, DataType, Reader, Xlsx};

use super::{build_preview, ImportError, ImportOptions, ImportPreview};

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => f.to_string(),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => cell
            .as_datetime()
            .map(|dt| dt.to_string())
            .unwrap_or_else(|| dt.to_string()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(error) => format!("{error:?}"),
    }
}

/// Read the first worksheet of `source` as raw text, same shape as
/// [`super::csv::read`].
pub fn read(
    source: impl Read + Seek,
    options: &ImportOptions,
) -> Result<(Vec<String>, Vec<Vec<String>>), ImportError> {
    let mut workbook = Xlsx::new(source).map_err(|e| ImportError::Xlsx(e.to_string()))?;
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| ImportError::Xlsx("workbook has no worksheets".to_string()))?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .map_err(|e| ImportError::Xlsx(e.to_string()))?;

    let mut rows_iter = range.rows();
    let header = if options.header {
        rows_iter.next()
    } else {
        None
    };

    let rows: Vec<Vec<String>> = rows_iter
        .map(|row| row.iter().map(cell_text).collect())
        .collect();

    let columns = match header {
        Some(row) => row.iter().map(cell_text).collect(),
        None => {
            let width = rows.first().map(Vec::len).unwrap_or(0);
            (1..=width).map(|i| format!("col{i}")).collect()
        }
    };

    Ok((columns, rows))
}

/// Read `source` and sample it into an [`ImportPreview`].
pub fn preview(
    source: impl Read + Seek,
    options: &ImportOptions,
) -> Result<ImportPreview, ImportError> {
    let (columns, rows) = read(source, options)?;
    Ok(build_preview(columns, &rows, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::DetectedType;
    use rust_xlsxwriter::Workbook;
    use std::io::Cursor;

    fn workbook_bytes(rows: &[[&str; 2]]) -> Vec<u8> {
        let mut workbook = Workbook::new();
        let sheet = workbook.add_worksheet();
        for (row_index, row) in rows.iter().enumerate() {
            for (col_index, cell) in row.iter().enumerate() {
                sheet
                    .write_string(row_index as u32, col_index as u16, *cell)
                    .unwrap();
            }
        }
        workbook.save_to_buffer().unwrap()
    }

    #[test]
    fn header_row_names_the_columns() {
        let bytes = workbook_bytes(&[["id", "name"], ["1", "alice"]]);
        let (columns, rows) = read(Cursor::new(bytes), &ImportOptions::default()).unwrap();
        assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
        assert_eq!(rows, vec![vec!["1".to_string(), "alice".to_string()]]);
    }

    #[test]
    fn preview_detects_types_from_worksheet_cells() {
        let bytes = workbook_bytes(&[["id", "name"], ["1", "alice"], ["2", "bob"]]);
        let result = preview(Cursor::new(bytes), &ImportOptions::default()).unwrap();
        assert_eq!(
            result.detected_types,
            vec![DetectedType::Int, DetectedType::Text]
        );
        assert_eq!(result.sample_rows.len(), 2);
    }

    #[test]
    fn no_header_synthesizes_column_names() {
        let bytes = workbook_bytes(&[["1", "alice"]]);
        let options = ImportOptions {
            header: false,
            ..ImportOptions::default()
        };
        let (columns, _) = read(Cursor::new(bytes), &options).unwrap();
        assert_eq!(columns, vec!["col1".to_string(), "col2".to_string()]);
    }
}
