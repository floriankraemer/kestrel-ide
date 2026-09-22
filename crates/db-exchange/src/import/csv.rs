//! CSV source reading for import: parses the whole source once into a
//! header list plus raw-text rows — [`preview`] then samples the first
//! [`super::PREVIEW_SAMPLE_ROWS`] of those for display, and [`plan`]/
//! [`super::run`] work over the full set the caller kept.

use std::io::Read;

use csv::ReaderBuilder;

use super::{build_preview, ImportError, ImportOptions, ImportPreview};

/// Read every row of `reader` as raw text, honouring `options.header` and
/// `options.delimiter`. Synthesizes `col1`, `col2`, … column names when
/// there is no header row to read one from.
pub fn read(
    reader: impl Read,
    options: &ImportOptions,
) -> Result<(Vec<String>, Vec<Vec<String>>), ImportError> {
    let mut csv_reader = ReaderBuilder::new()
        .delimiter(options.delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(reader);

    let mut records = csv_reader.records();
    let header = if options.header {
        match records.next() {
            Some(record) => Some(record.map_err(|e| ImportError::Csv(e.to_string()))?),
            None => None,
        }
    } else {
        None
    };

    let mut rows = Vec::new();
    for record in records {
        let record = record.map_err(|e| ImportError::Csv(e.to_string()))?;
        rows.push(record.iter().map(str::to_string).collect());
    }

    let columns = match header {
        Some(record) => record.iter().map(str::to_string).collect(),
        None => {
            let width = rows.first().map(Vec::len).unwrap_or(0);
            (1..=width).map(|i| format!("col{i}")).collect()
        }
    };

    Ok((columns, rows))
}

/// Read `reader` and sample it into an [`ImportPreview`].
pub fn preview(reader: impl Read, options: &ImportOptions) -> Result<ImportPreview, ImportError> {
    let (columns, rows) = read(reader, options)?;
    Ok(build_preview(columns, &rows, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::DetectedType;

    #[test]
    fn header_row_names_the_columns() {
        let (columns, rows) = read(
            "id,name\n1,alice\n2,bob\n".as_bytes(),
            &ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
        assert_eq!(
            rows,
            vec![
                vec!["1".to_string(), "alice".to_string()],
                vec!["2".to_string(), "bob".to_string()]
            ]
        );
    }

    #[test]
    fn no_header_synthesizes_column_names() {
        let options = ImportOptions {
            header: false,
            ..ImportOptions::default()
        };
        let (columns, rows) = read("1,alice\n2,bob\n".as_bytes(), &options).unwrap();
        assert_eq!(columns, vec!["col1".to_string(), "col2".to_string()]);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_quoted_field_with_an_embedded_delimiter_is_one_cell() {
        let (_, rows) = read("id,note\n1,\"a,b\"\n".as_bytes(), &ImportOptions::default()).unwrap();
        assert_eq!(rows[0][1], "a,b");
    }

    #[test]
    fn preview_detects_types_and_samples_rows() {
        let text = "id,name\n1,alice\n2,bob\n";
        let result = preview(text.as_bytes(), &ImportOptions::default()).unwrap();
        assert_eq!(
            result.detected_types,
            vec![DetectedType::Int, DetectedType::Text]
        );
        assert_eq!(result.sample_rows.len(), 2);
    }

    #[test]
    fn a_tab_delimiter_is_honoured() {
        let options = ImportOptions {
            delimiter: b'\t',
            ..ImportOptions::default()
        };
        let (columns, rows) = read("id\tname\n1\talice\n".as_bytes(), &options).unwrap();
        assert_eq!(columns, vec!["id".to_string(), "name".to_string()]);
        assert_eq!(rows, vec![vec!["1".to_string(), "alice".to_string()]]);
    }
}
