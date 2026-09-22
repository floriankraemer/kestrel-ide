//! Export formats (F5.1, database-tools-plan.md): every format in this
//! module takes the same three things — the result's columns (once), a
//! stream of already-fetched [`RowBatch`]es, and a writer — so a caller
//! never has to special-case a format to drive it, and "copy as" is just
//! the same call aimed at a `String` instead of a file.
//!
//! Cell rendering goes through [`db_core::value::Value::display`] with
//! [`ExportOptions::format_rules`] (full fidelity: no byte-preview
//! truncation, unlike the result grid) for every format except SQL, which
//! needs a bindable *literal* rather than display text, and XLSX, which
//! needs Excel's own typed cells for numbers/dates/booleans.

use std::io;

use db_core::dialect::Dialect;
use db_core::value::{ColumnMeta, FormatRules, RowBatch};

pub mod csv;
pub mod html;
pub mod json;
pub mod markdown;
pub mod sql;
pub mod tsv;
pub mod xlsx;

/// A format's row source: the already-fetched batches, in order. Never
/// re-iterable — a single pass is what every format below needs, and
/// requiring more would force a caller to buffer a result set it may not
/// be able to hold twice.
pub type RowSource<'a> = &'a mut dyn Iterator<Item = RowBatch>;

/// `Array`: one JSON array of row objects, `[ {..}, {..} ]`.
/// `Lines`: one JSON object per line (JSON Lines / NDJSON) — the shape a
/// downstream tool can stream without holding the whole export in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonShape {
    Array,
    Lines,
}

/// Which statement shape [`sql`] emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlMode {
    Insert,
    /// `UPDATE <table> SET <non-key columns> WHERE <key_columns>`, one
    /// statement per row.
    Update {
        key_columns: Vec<String>,
    },
}

/// Every exporter's shared knobs. Not every field applies to every format
/// (`delimiter` is CSV/TSV-only, `json`/`sql` each name the one format
/// they configure) — a format simply ignores what it does not need,
/// rather than the caller assembling a different options type per format.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOptions {
    pub header: bool,
    pub null_text: String,
    pub quote_all: bool,
    pub delimiter: u8,
    pub date_format: String,
    pub json: JsonShape,
    pub sql: SqlMode,
    pub table_name: String,
    pub dialect: Dialect,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            header: true,
            null_text: "NULL".to_string(),
            quote_all: false,
            delimiter: b',',
            date_format: "%Y-%m-%d".to_string(),
            json: JsonShape::Array,
            sql: SqlMode::Insert,
            table_name: "export".to_string(),
            dialect: Dialect::Sqlite,
        }
    }
}

impl ExportOptions {
    /// The [`FormatRules`] every non-SQL, non-XLSX exporter renders cells
    /// through: full fidelity — `bytes_preview_len: usize::MAX` means
    /// [`Value::display`](db_core::value::Value::display) never truncates
    /// a `Bytes` value with an ellipsis, since an export must not silently
    /// drop data the way the result grid's own preview may.
    pub(crate) fn format_rules(&self) -> FormatRules {
        FormatRules {
            null_text: self.null_text.clone(),
            date_format: self.date_format.clone(),
            time_format: "%H:%M:%S".to_string(),
            datetime_format: format!("{} %H:%M:%S", self.date_format),
            bytes_preview_len: usize::MAX,
        }
    }
}

/// Every format this module offers, for a caller (F5b's `ExchangeService`)
/// that picks one at runtime from a dialog's combo box rather than
/// calling one module's `write` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Json,
    Markdown,
    Html,
    Sql,
    Xlsx,
}

/// Dispatch to the one format module `format` names — every format
/// module shares this exact signature (see each module's own doc
/// comment), so this is the whole dispatch, not a wrapper hiding
/// per-format special cases.
pub fn write_format(
    format: Format,
    columns: &[ColumnMeta],
    rows: RowSource,
    writer: &mut dyn io::Write,
    options: &ExportOptions,
) -> io::Result<()> {
    match format {
        Format::Csv => csv::write(columns, rows, writer, options),
        Format::Tsv => tsv::write(columns, rows, writer, options),
        Format::Json => json::write(columns, rows, writer, options),
        Format::Markdown => markdown::write(columns, rows, writer, options),
        Format::Html => html::write(columns, rows, writer, options),
        Format::Sql => sql::write(columns, rows, writer, options),
        Format::Xlsx => xlsx::write(columns, rows, writer, options),
    }
}

/// Map a `csv`-crate error onto the [`io::Result`] every exporter returns
/// — the crate's own `Error` is not an `io::Error`, but every case it can
/// produce here is one (the write side; the read side is `import::csv`'s
/// own concern).
pub(crate) fn csv_io_err(error: ::csv::Error) -> io::Error {
    io::Error::other(error)
}
