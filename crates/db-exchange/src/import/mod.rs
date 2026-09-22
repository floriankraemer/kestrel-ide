//! CSV/XLSX import (F5.2, database-tools-plan.md): `preview` samples a
//! source and guesses each column's type, a user-editable [`Mapping`]
//! says what to do with each source column, [`plan`] compiles that into
//! a `CREATE TABLE` plus batched, parameterised `INSERT`s — never an
//! inlined literal, the same ADR-0061 §1 rule [`db_core::dml`] follows —
//! and [`run`] applies the plan through a live [`Connection`].
//!
//! Deviation from the plan doc's literal signature: `plan` takes the
//! *full* row set and an [`ImportOptions`]/`batch_size` alongside the
//! preview, not the preview alone — [`ImportPreview::sample_rows`] is
//! deliberately capped for display, and a real import has to insert every
//! row, not just the sample.

pub mod csv;
pub mod xlsx;

use chrono::NaiveDate;
use db_core::dialect::Dialect;
use db_core::driver::{CancelToken, Connection, ExecOptions, Execution, QueryLang, Statement};
use db_core::error::DbError;

/// How many rows a preview shows before "and N more" — enough to see the
/// shape of the data without reading a multi-gigabyte file into memory
/// just to open the import dialog.
pub const PREVIEW_SAMPLE_ROWS: usize = 50;

/// Rows-per-`INSERT` a real import batches at, unless the caller asks for
/// a different size.
pub const DEFAULT_BATCH_SIZE: usize = 500;

/// Why a source could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    Csv(String),
    Xlsx(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Csv(message) => write!(f, "CSV import: {message}"),
            ImportError::Xlsx(message) => write!(f, "XLSX import: {message}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// The type [`plan`]'s coercion step tries to parse each source cell as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedType {
    Int,
    Float,
    Bool,
    Date,
    Text,
}

/// Source-parsing knobs shared by CSV and XLSX (XLSX ignores `delimiter`).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportOptions {
    pub header: bool,
    pub null_text: String,
    pub delimiter: u8,
    pub date_format: String,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            header: true,
            null_text: String::new(),
            delimiter: b',',
            date_format: "%Y-%m-%d".to_string(),
        }
    }
}

/// A sampled look at a source: its columns, the first
/// [`PREVIEW_SAMPLE_ROWS`] rows (as raw text, before coercion), and each
/// column's guessed type.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPreview {
    pub columns: Vec<String>,
    pub sample_rows: Vec<Vec<String>>,
    pub detected_types: Vec<DetectedType>,
}

/// Build an [`ImportPreview`] from an already-read `(columns, rows)` pair
/// — `csv::preview`/`xlsx::preview`'s own shared tail, made `pub` rather
/// than `pub(crate)` so a caller that already called `read` for `plan`'s
/// sake (`ui-shell`'s `ExchangeService::importRun`, which needs the full
/// row set `preview` alone never reads) is not forced to read the source
/// twice just to get the same [`ImportPreview`] `plan` takes.
pub fn build_preview(
    columns: Vec<String>,
    rows: &[Vec<String>],
    options: &ImportOptions,
) -> ImportPreview {
    let detected_types: Vec<DetectedType> = (0..columns.len())
        .map(|col_index| {
            let values: Vec<&str> = rows
                .iter()
                .filter_map(|row| row.get(col_index).map(String::as_str))
                .collect();
            detect_type(&values, &options.null_text, &options.date_format)
        })
        .collect();
    let sample_rows = rows.iter().take(PREVIEW_SAMPLE_ROWS).cloned().collect();
    ImportPreview {
        columns,
        sample_rows,
        detected_types,
    }
}

/// The narrowest type every non-null value in `values` parses as, tried
/// most-specific first; `Text` never fails, so this always returns
/// something.
pub fn detect_type(values: &[&str], null_text: &str, date_format: &str) -> DetectedType {
    for candidate in [
        DetectedType::Int,
        DetectedType::Float,
        DetectedType::Bool,
        DetectedType::Date,
    ] {
        let all_fit = values
            .iter()
            .filter(|value| !value.is_empty() && **value != null_text)
            .all(|value| fits(value, candidate, date_format));
        if all_fit {
            return candidate;
        }
    }
    DetectedType::Text
}

fn fits(value: &str, candidate: DetectedType, date_format: &str) -> bool {
    match candidate {
        DetectedType::Int => value.parse::<i64>().is_ok(),
        DetectedType::Float => value.parse::<f64>().is_ok(),
        DetectedType::Bool => matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "1" | "0" | "yes" | "no"
        ),
        DetectedType::Date => NaiveDate::parse_from_str(value, date_format).is_ok(),
        DetectedType::Text => true,
    }
}

/// Coerce one raw cell into a [`db_core::value::Value`] under `coercion`
/// — falls back to `Text` rather than erroring when a cell does not
/// actually fit the column's detected type (a ragged CSV's odd row should
/// not abort the whole import).
pub fn coerce(
    raw: &str,
    coercion: DetectedType,
    null_text: &str,
    date_format: &str,
) -> db_core::value::Value {
    use db_core::value::Value;
    if raw.is_empty() || raw == null_text {
        return Value::Null;
    }
    match coercion {
        DetectedType::Int => raw
            .parse::<i64>()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(raw.to_string())),
        DetectedType::Float => raw
            .parse::<f64>()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::Text(raw.to_string())),
        DetectedType::Bool => match raw.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Value::Bool(true),
            "false" | "0" | "no" => Value::Bool(false),
            _ => Value::Text(raw.to_string()),
        },
        DetectedType::Date => NaiveDate::parse_from_str(raw, date_format)
            .map(Value::Date)
            .unwrap_or_else(|_| Value::Text(raw.to_string())),
        DetectedType::Text => Value::Text(raw.to_string()),
    }
}

/// How one source column is imported.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnMapping {
    pub source_col: String,
    pub target_col: String,
    pub type_coercion: DetectedType,
    pub skip: bool,
}

/// A user-editable mapping from source columns to target columns.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mapping {
    pub columns: Vec<ColumnMapping>,
}

impl Mapping {
    /// The identity mapping a preview suggests before the user edits
    /// anything: every source column maps onto a same-named target column
    /// under its detected type.
    pub fn from_preview(preview: &ImportPreview) -> Self {
        let columns = preview
            .columns
            .iter()
            .zip(preview.detected_types.iter())
            .map(|(name, detected)| ColumnMapping {
                source_col: name.clone(),
                target_col: name.clone(),
                type_coercion: *detected,
                skip: false,
            })
            .collect();
        Self { columns }
    }
}

fn column_ddl_type(dialect: Dialect, detected: DetectedType) -> &'static str {
    use DetectedType::*;
    match (dialect, detected) {
        (Dialect::Sqlite, Int | Bool) => "INTEGER",
        (Dialect::Sqlite, Float) => "REAL",
        (Dialect::Sqlite, Date | Text) => "TEXT",
        (Dialect::MySql, Int) => "BIGINT",
        (Dialect::MySql, Float) => "DOUBLE",
        (Dialect::MySql, Bool) => "TINYINT(1)",
        (Dialect::MySql, Date) => "DATE",
        (Dialect::MySql, Text) => "TEXT",
        (Dialect::SqlServer, Int) => "BIGINT",
        (Dialect::SqlServer, Float) => "FLOAT",
        (Dialect::SqlServer, Bool) => "BIT",
        (Dialect::SqlServer, Date) => "DATE",
        (Dialect::SqlServer, Text) => "NVARCHAR(MAX)",
        (Dialect::Cassandra, Int) => "bigint",
        (Dialect::Cassandra, Float) => "double",
        (Dialect::Cassandra, Bool) => "boolean",
        (Dialect::Cassandra, Date) => "date",
        (Dialect::Cassandra, Text) => "text",
        // Postgres, and Mongo/Redis (which have no SQL DDL to begin with —
        // falling back to Postgres-shaped types keeps this total rather
        // than requiring a separate no-DDL path this crate does not need
        // yet).
        (_, Int) => "BIGINT",
        (_, Float) => "DOUBLE PRECISION",
        (_, Bool) => "BOOLEAN",
        (_, Date) => "DATE",
        (_, Text) => "TEXT",
    }
}

/// One batched `INSERT` — a single multi-row `VALUES` statement, every
/// cell a bound parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct DmlBatch {
    pub statement: Statement,
}

/// The compiled import: an optional `CREATE TABLE` (`None` only when
/// every column is skipped, so there is nothing to create) and the
/// batched `INSERT`s that load the data.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportPlan {
    pub create_table: Option<String>,
    pub batches: Vec<DmlBatch>,
}

/// Compile `rows` (the *full* source, not just `preview.sample_rows`)
/// into an [`ImportPlan`] under `mapping`, targeting `target_table` in
/// `dialect`.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    preview: &ImportPreview,
    rows: &[Vec<String>],
    mapping: &Mapping,
    target_table: &str,
    dialect: Dialect,
    options: &ImportOptions,
    batch_size: usize,
) -> ImportPlan {
    let active: Vec<&ColumnMapping> = mapping.columns.iter().filter(|c| !c.skip).collect();

    let create_table = if active.is_empty() {
        None
    } else {
        let column_defs: Vec<String> = active
            .iter()
            .map(|c| {
                format!(
                    "{} {}",
                    dialect.quote_ident(&c.target_col),
                    column_ddl_type(dialect, c.type_coercion)
                )
            })
            .collect();
        Some(format!(
            "CREATE TABLE {} ({})",
            dialect.quote_ident(target_table),
            column_defs.join(", ")
        ))
    };

    let source_indices: Vec<usize> = active
        .iter()
        .map(|c| {
            preview
                .columns
                .iter()
                .position(|name| name == &c.source_col)
                .unwrap_or(0)
        })
        .collect();
    let column_list: Vec<String> = active
        .iter()
        .map(|c| dialect.quote_ident(&c.target_col))
        .collect();

    let batch_size = batch_size.max(1);
    let batches = rows
        .chunks(batch_size)
        .map(|chunk| {
            let mut params = Vec::with_capacity(chunk.len() * active.len());
            let mut value_groups = Vec::with_capacity(chunk.len());
            for row in chunk {
                let placeholders: Vec<&str> = active.iter().map(|_| "?").collect();
                value_groups.push(format!("({})", placeholders.join(", ")));
                for (mapping_col, &source_index) in active.iter().zip(source_indices.iter()) {
                    let raw = row.get(source_index).map(String::as_str).unwrap_or("");
                    params.push(coerce(
                        raw,
                        mapping_col.type_coercion,
                        &options.null_text,
                        &options.date_format,
                    ));
                }
            }
            let text = format!(
                "INSERT INTO {} ({}) VALUES {}",
                dialect.quote_ident(target_table),
                column_list.join(", "),
                value_groups.join(", ")
            );
            DmlBatch {
                statement: Statement {
                    lang: QueryLang::Sql,
                    text,
                    params,
                },
            }
        })
        .collect();

    ImportPlan {
        create_table,
        batches,
    }
}

/// Apply `plan` through `connection`: the `CREATE TABLE` first (if any),
/// then each batch in order, stopping (without erroring) as soon as
/// `cancel` is set. Returns the total rows inserted.
pub fn run(
    plan: &ImportPlan,
    connection: &mut dyn Connection,
    cancel: &CancelToken,
) -> Result<u64, DbError> {
    if let Some(ddl) = &plan.create_table {
        connection.execute(&Statement::sql(ddl.clone()), &ExecOptions::default())?;
    }
    let mut affected = 0;
    for batch in &plan.batches {
        if cancel.is_cancelled() {
            break;
        }
        if let Execution::Affected(rows) =
            connection.execute(&batch.statement, &ExecOptions::default())?
        {
            affected += rows;
        }
    }
    Ok(affected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_type_picks_the_narrowest_type_every_value_fits() {
        assert_eq!(
            detect_type(&["1", "2", "3"], "", "%Y-%m-%d"),
            DetectedType::Int
        );
        assert_eq!(
            detect_type(&["1", "2.5"], "", "%Y-%m-%d"),
            DetectedType::Float
        );
        assert_eq!(
            detect_type(&["true", "false"], "", "%Y-%m-%d"),
            DetectedType::Bool
        );
        assert_eq!(
            detect_type(&["2026-09-21", "2026-01-01"], "", "%Y-%m-%d"),
            DetectedType::Date
        );
        assert_eq!(
            detect_type(&["1", "abc"], "", "%Y-%m-%d"),
            DetectedType::Text
        );
    }

    #[test]
    fn detect_type_ignores_null_text_when_judging_fit() {
        assert_eq!(
            detect_type(&["1", "", "3"], "", "%Y-%m-%d"),
            DetectedType::Int
        );
        assert_eq!(
            detect_type(&["1", "\\N", "3"], "\\N", "%Y-%m-%d"),
            DetectedType::Int
        );
    }

    #[test]
    fn coerce_falls_back_to_text_rather_than_erroring_on_a_mismatch() {
        let value = coerce("abc", DetectedType::Int, "", "%Y-%m-%d");
        assert_eq!(value, db_core::value::Value::Text("abc".to_string()));
    }

    #[test]
    fn coerce_maps_empty_or_null_text_to_null() {
        assert_eq!(
            coerce("", DetectedType::Int, "", "%Y-%m-%d"),
            db_core::value::Value::Null
        );
        assert_eq!(
            coerce("\\N", DetectedType::Text, "\\N", "%Y-%m-%d"),
            db_core::value::Value::Null
        );
    }

    fn sample_preview() -> ImportPreview {
        ImportPreview {
            columns: vec!["id".to_string(), "name".to_string()],
            sample_rows: vec![vec!["1".to_string(), "alice".to_string()]],
            detected_types: vec![DetectedType::Int, DetectedType::Text],
        }
    }

    #[test]
    fn plan_batches_never_inline_a_literal_value() {
        let preview = sample_preview();
        let mapping = Mapping::from_preview(&preview);
        let rows = vec![
            vec![
                "1".to_string(),
                "Robert'); DROP TABLE students;--".to_string(),
            ],
            vec!["2".to_string(), "bob".to_string()],
        ];
        let import_plan = plan(
            &preview,
            &rows,
            &mapping,
            "people",
            Dialect::Sqlite,
            &ImportOptions::default(),
            10,
        );
        assert_eq!(import_plan.batches.len(), 1);
        let statement = &import_plan.batches[0].statement;
        assert!(!statement.text.contains("Robert"));
        assert!(statement.text.contains("VALUES (?, ?), (?, ?)"));
        assert_eq!(statement.params.len(), 4);
    }

    #[test]
    fn plan_splits_rows_across_batches_by_batch_size() {
        let preview = sample_preview();
        let mapping = Mapping::from_preview(&preview);
        let rows: Vec<Vec<String>> = (0..5)
            .map(|i| vec![i.to_string(), format!("n{i}")])
            .collect();
        let import_plan = plan(
            &preview,
            &rows,
            &mapping,
            "people",
            Dialect::Sqlite,
            &ImportOptions::default(),
            2,
        );
        assert_eq!(import_plan.batches.len(), 3);
    }

    #[test]
    fn a_skipped_column_is_absent_from_create_table_and_inserts() {
        let preview = sample_preview();
        let mut mapping = Mapping::from_preview(&preview);
        mapping.columns[1].skip = true;
        let rows = vec![vec!["1".to_string(), "alice".to_string()]];
        let import_plan = plan(
            &preview,
            &rows,
            &mapping,
            "people",
            Dialect::Sqlite,
            &ImportOptions::default(),
            10,
        );
        let ddl = import_plan.create_table.unwrap();
        assert!(!ddl.contains("name"));
        assert!(!import_plan.batches[0].statement.text.contains("name"));
    }

    #[test]
    fn every_column_skipped_produces_no_create_table() {
        let preview = sample_preview();
        let mut mapping = Mapping::from_preview(&preview);
        for column in &mut mapping.columns {
            column.skip = true;
        }
        let import_plan = plan(
            &preview,
            &[],
            &mapping,
            "people",
            Dialect::Sqlite,
            &ImportOptions::default(),
            10,
        );
        assert!(import_plan.create_table.is_none());
    }
}
