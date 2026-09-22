//! `Value`: the one row/document shape every backend converts into
//! (ADR-0058 §2), and `RowBatch`/`ColumnMeta`, the page of rows a
//! `RowStream` hands back.
//!
//! `Decimal` is a `String`, deliberately: a driver's own arbitrary-precision
//! decimal is preserved exactly as the server sent it rather than rounded
//! through a Rust decimal type's own precision limits (ADR-0058 §2, the
//! same "delegate and never model" instinct `build-core` applies to a
//! build tool's own numbers).

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};

/// One cell's value, wide enough for every native driver's row *and* a
/// MongoDB document *and* a Redis value — `Document`/`Array` are the two
/// recursive cases the NoSQL backends need and the SQL backends never
/// produce.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// The server's own decimal text, verbatim — never parsed into a
    /// fixed-point Rust type.
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Date(NaiveDate),
    Time(NaiveTime),
    DateTime(NaiveDateTime),
    DateTimeTz(DateTime<FixedOffset>),
    Uuid(uuid::Uuid),
    /// Already-serialised JSON text (Postgres `json`/`jsonb`, a MongoDB
    /// sub-document that a caller chose to keep opaque, …).
    Json(String),
    Array(Vec<Value>),
    /// An ordered field list — a MongoDB document, or any nested composite
    /// a relational value never needs.
    Document(Vec<(String, Value)>),
    /// A backend-specific type this enum has no case for (a Postgres range
    /// type, a custom enum, …): the driver's own name for it plus a
    /// pre-rendered display string, so the grid can still show *something*
    /// rather than fail the whole row.
    Other {
        type_name: String,
        display: String,
    },
}

/// How [`Value::display`] renders a cell as text — the result grid's and
/// the export path's shared formatting rules, so the two never drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatRules {
    pub null_text: String,
    pub date_format: String,
    pub time_format: String,
    pub datetime_format: String,
    /// How many bytes of a `Bytes` value are hex-rendered before an
    /// ellipsis — the grid shows a preview, not a multi-megabyte BLOB
    /// inline.
    pub bytes_preview_len: usize,
}

impl Default for FormatRules {
    fn default() -> Self {
        Self {
            null_text: "NULL".to_string(),
            date_format: "%Y-%m-%d".to_string(),
            time_format: "%H:%M:%S".to_string(),
            datetime_format: "%Y-%m-%d %H:%M:%S".to_string(),
            bytes_preview_len: 16,
        }
    }
}

fn hex_preview(bytes: &[u8], max_len: usize) -> String {
    let truncated = bytes.len() > max_len;
    let shown = &bytes[..bytes.len().min(max_len)];
    let mut text: String = shown.iter().map(|b| format!("{b:02x}")).collect();
    if truncated {
        text.push('\u{2026}'); // ellipsis
    }
    text
}

impl Value {
    /// Render this value as the grid/export text a user reads — never
    /// used for anything bound to a statement (params are bound as
    /// [`Value`]s, this is display only, see `dml`'s doc comment).
    pub fn display(&self, rules: &FormatRules) -> String {
        match self {
            Value::Null => rules.null_text.clone(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) => value.to_string(),
            Value::Decimal(text) => text.clone(),
            Value::Text(text) => text.clone(),
            Value::Bytes(bytes) => hex_preview(bytes, rules.bytes_preview_len),
            Value::Date(date) => date.format(&rules.date_format).to_string(),
            Value::Time(time) => time.format(&rules.time_format).to_string(),
            Value::DateTime(dt) => dt.format(&rules.datetime_format).to_string(),
            Value::DateTimeTz(dt) => dt.format(&rules.datetime_format).to_string(),
            Value::Uuid(id) => id.to_string(),
            Value::Json(text) => text.clone(),
            Value::Array(items) => {
                let rendered: Vec<String> = items.iter().map(|item| item.display(rules)).collect();
                format!("[{}]", rendered.join(", "))
            }
            Value::Document(fields) => {
                let rendered: Vec<String> = fields
                    .iter()
                    .map(|(name, value)| format!("{name}: {}", value.display(rules)))
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
            Value::Other { display, .. } => display.clone(),
        }
    }

    /// A rough byte estimate for [`crate::result::MemoryCap`] accounting —
    /// exact enough to stop fetching before the cap is badly overshot,
    /// never claimed to be the value's true heap size.
    pub fn approx_size(&self) -> usize {
        match self {
            Value::Null | Value::Bool(_) => 1,
            Value::Int(_) | Value::Float(_) => 8,
            Value::Decimal(text) | Value::Text(text) | Value::Json(text) => text.len(),
            Value::Bytes(bytes) => bytes.len(),
            Value::Date(_) => 4,
            Value::Time(_) => 8,
            Value::DateTime(_) | Value::DateTimeTz(_) => 12,
            Value::Uuid(_) => 16,
            Value::Array(items) => items.iter().map(Value::approx_size).sum(),
            Value::Document(fields) => fields
                .iter()
                .map(|(name, value)| name.len() + value.approx_size())
                .sum(),
            Value::Other { type_name, display } => type_name.len() + display.len(),
        }
    }

    /// Render `self` as a literal `dialect`'s own parser accepts — never
    /// `display` text, which is meant to be read, not parsed back (F4.3's
    /// FK navigation binds a cell's already-known value into a `WHERE`
    /// this way, the same "a value this crate itself produced, never
    /// user-typed text" trust boundary `db_exchange::export::sql`'s own
    /// `sql_literal` doc comment draws for the same reason — that
    /// module's private copy predates this one and is out of this
    /// phase's file list to unify with).
    pub fn sql_literal(&self, dialect: crate::dialect::Dialect) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Decimal(text) => text.clone(),
            Value::Text(text) | Value::Json(text) => dialect.quote_literal(text),
            Value::Bytes(bytes) => {
                let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                match dialect {
                    crate::dialect::Dialect::SqlServer => format!("0x{hex}"),
                    crate::dialect::Dialect::Postgres => format!("'\\x{hex}'"),
                    _ => format!("X'{hex}'"),
                }
            }
            Value::Date(date) => dialect.quote_literal(&date.to_string()),
            Value::Time(time) => dialect.quote_literal(&time.to_string()),
            Value::DateTime(dt) => dialect.quote_literal(&dt.to_string()),
            Value::DateTimeTz(dt) => dialect.quote_literal(&dt.to_rfc3339()),
            Value::Uuid(id) => dialect.quote_literal(&id.to_string()),
            Value::Array(_) | Value::Document(_) | Value::Other { .. } => {
                dialect.quote_literal(&self.display(&Default::default()))
            }
        }
    }
}

/// Serialises one row's [`Value`]s losslessly (`serde`'s own tagged
/// representation of the enum, not [`Value::display`] text) — the FFI
/// seam's typed-row encoding (database-tools-plan F4c): a compact
/// JSON-per-row string rather than a new cxx-qt struct, since `Value`'s
/// recursive `Array`/`Document` cases and its dozen scalar variants have
/// no shape cxx's shared-struct rules accept directly. `ui_shell::bridge::
/// database::console::ResultProvider::row_values` produces this, `db_
/// exchange`-backed export consumes it back through [`row_from_json`] —
/// never rendered through `display` first, so a cell's real type (an
/// `Int`, not the text `"42"`) survives the round trip.
pub fn row_to_json(row: &[Value]) -> String {
    serde_json::to_string(row).unwrap_or_else(|_| "[]".to_string())
}

/// The inverse of [`row_to_json`]. `Err` (never a panic) on malformed
/// JSON — the FFI caller already trusts its own encoder, but a corrupt or
/// truncated string must still fail cleanly rather than unwrap.
pub fn row_from_json(text: &str) -> Result<Vec<Value>, String> {
    serde_json::from_str(text).map_err(|error| format!("not a valid encoded row: {error}"))
}

/// One column's shape, as introspection or an executed statement reports
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    /// The single table this column's *values* came from, when a driver
    /// can report it (SQLite's `column_table_name`, Postgres's row
    /// description `table_oid`) — `None` for a computed expression, an
    /// aggregate, a join's ambiguous column, or a backend that cannot say
    /// (Mongo/Redis/Cassandra never populate this).
    ///
    /// This is what the data editor (F4.1) decides editability from: a
    /// result is only offered as a grid to edit when every column's
    /// `origin` names the *same* table (database-tools.md §4's submit
    /// sequence) — never guessed by re-parsing the statement text, which
    /// is exactly the kind of re-derivation `dialect.rs`'s own doc
    /// comment already rules out for identifier quoting.
    pub origin: Option<String>,
}

impl ColumnMeta {
    /// A column with no known table origin — every call site that does
    /// not (yet) report one uses this rather than repeating the `None`
    /// literal, so the day a driver learns to report it, only its own
    /// constructor needs to change.
    pub fn new(name: impl Into<String>, type_name: impl Into<String>, nullable: bool) -> Self {
        Self {
            name: name.into(),
            type_name: type_name.into(),
            nullable,
            origin: None,
        }
    }
}

/// Parses `text` (as a user typed it into a cell/value editor) into a
/// [`Value`] shaped by `type_name` — the data editor's own "coerce before
/// bind" step (F4.1): a cell's new value is always bound as a typed
/// parameter, never interpolated as text (the same rule `dml.rs` applies
/// to every staged edit). `type_name` is matched loosely (case-insensitive
/// substring) since every backend spells its own types differently
/// (`INTEGER` vs `int4` vs `bigint`); an unrecognised type falls back to
/// `Text`, which still binds safely, just without the stronger type.
///
/// Reformats `text` as pretty-printed (2-space indented) JSON — the value
/// editor's own JSON pretty-print toggle (F4.1). `Err` (the parse error)
/// when `text` is not valid JSON; the editor keeps showing the raw text
/// in that case rather than losing what the user typed.
///
/// XML gets no equivalent helper yet: this crate has no XML parser
/// dependency, and F4.1's own XML content is rare enough (a handful of
/// database columns store it, none of this codebase's own fixtures do)
/// that pulling one in for a pretty-print button alone is not yet earned
/// (YAGNI) — add it if a real user asks.
pub fn pretty_json(text: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("not valid JSON: {error}"))?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

/// `Err` carries the message the editing cell shows — never a panic, and
/// never a value that silently became something the user did not type.
pub fn parse_text(text: &str, type_name: &str) -> Result<Value, String> {
    let type_name = type_name.to_ascii_lowercase();
    let contains = |needle: &str| type_name.contains(needle);
    if contains("bool") {
        return match text.trim().to_ascii_lowercase().as_str() {
            "true" | "t" | "1" | "yes" => Ok(Value::Bool(true)),
            "false" | "f" | "0" | "no" => Ok(Value::Bool(false)),
            _ => Err(format!("'{text}' is not a boolean (try true/false)")),
        };
    }
    if contains("int") || contains("serial") {
        return text
            .trim()
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("'{text}' is not a whole number"));
    }
    if contains("float") || contains("double") || contains("real") {
        return text
            .trim()
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("'{text}' is not a number"));
    }
    if contains("numeric") || contains("decimal") || contains("money") {
        // Verbatim, like every other `Decimal` — only shape-checked, never
        // reparsed through a lossy float (this module's own doc comment).
        return text
            .trim()
            .parse::<f64>()
            .map(|_| Value::Decimal(text.trim().to_string()))
            .map_err(|_| format!("'{text}' is not a decimal number"));
    }
    if contains("uuid") {
        return uuid::Uuid::parse_str(text.trim())
            .map(Value::Uuid)
            .map_err(|_| format!("'{text}' is not a valid UUID"));
    }
    if contains("json") {
        return serde_json::from_str::<serde_json::Value>(text)
            .map(|_| Value::Json(text.to_string()))
            .map_err(|error| format!("'{text}' is not valid JSON: {error}"));
    }
    if contains("blob") || contains("bytea") || contains("binary") || contains("varbinary") {
        return hex_text_to_bytes(text).map(Value::Bytes);
    }
    if contains("timestamp") || contains("datetime") {
        if let Ok(dt) = DateTime::parse_from_rfc3339(text.trim()) {
            return Ok(Value::DateTimeTz(dt));
        }
        return NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S")
            .or_else(|_| NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M:%S"))
            .map(Value::DateTime)
            .map_err(|_| format!("'{text}' is not a recognised date/time"));
    }
    if contains("date") {
        return NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
            .map(Value::Date)
            .map_err(|_| format!("'{text}' is not a date (expected YYYY-MM-DD)"));
    }
    if contains("time") {
        return NaiveTime::parse_from_str(text.trim(), "%H:%M:%S")
            .map(Value::Time)
            .map_err(|_| format!("'{text}' is not a time (expected HH:MM:SS)"));
    }
    Ok(Value::Text(text.to_string()))
}

/// `true` for a column `type_name` [`parse_text`] coerces through
/// [`parse_hex`] — the same vocabulary that function's own `blob`/
/// `bytea`/`binary`/`varbinary` branch matches, exposed so a caller
/// outside this module (the value editor's own hex-edit mode, F4c) can
/// ask "does this column need hex, not plain text" without re-deriving
/// that vocabulary itself (the exact re-derivation `ColumnMeta::origin`'s
/// own doc comment already rules out for a different question). Matched
/// case-insensitively, same as `parse_text`.
pub fn is_binary_type(type_name: &str) -> bool {
    let type_name = type_name.to_ascii_lowercase();
    ["blob", "bytea", "binary", "varbinary"]
        .iter()
        .any(|needle| type_name.contains(needle))
}

/// [`parse_text`]'s own hex-decoding step, factored out so the value
/// editor's hex-edit mode (F4c) can validate what a user is typing
/// against the exact same rule `parse_text` binds with — never a second,
/// slightly different regex/parser that could accept text `parse_text`
/// would then reject at Save time.
fn hex_text_to_bytes(text: &str) -> Result<Vec<u8>, String> {
    let trimmed = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("\\x");
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    parse_hex(trimmed).ok_or_else(|| format!("'{text}' is not valid hex"))
}

/// The value editor's own live-validation step for a binary (`Bytes`)
/// cell (F4c, closing the gap `ValueEditorDialog`'s own doc comment used
/// to describe): `Err` carries the same message [`parse_text`] would
/// reject the same text with, so a user sees the problem as they type
/// rather than only once they hit Save.
pub fn validate_hex_text(text: &str) -> Result<(), String> {
    hex_text_to_bytes(text).map(|_| ())
}

/// `text.len()` alone is a *byte* length, and byte-index slicing a `&str`
/// panics when a slice boundary lands inside a multi-byte UTF-8 character
/// — a real crash the `security-expert`'s F4.5 review found (a value
/// editor cell typed as e.g. `"1€2€"` has an even byte length but no
/// 2-byte-aligned char boundary at all). Requiring every byte to be an
/// ASCII hex digit first guarantees every 2-byte chunk is already a valid
/// boundary, so the slicing below can never panic.
fn parse_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// A page of rows a [`crate::driver::RowStream`] hands back, one column
/// list shared by every row in the batch.
#[derive(Debug, Clone, PartialEq)]
pub struct RowBatch {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
}

impl RowBatch {
    /// The sum of every cell's [`Value::approx_size`] plus each column
    /// name's length once — what [`crate::result::ResultSet::append_batch`]
    /// charges against the memory cap.
    pub fn approx_size(&self) -> usize {
        let columns_size: usize = self.columns.iter().map(|c| c.name.len()).sum();
        let rows_size: usize = self
            .rows
            .iter()
            .flat_map(|row| row.iter())
            .map(Value::approx_size)
            .sum();
        columns_size + rows_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> FormatRules {
        FormatRules::default()
    }

    #[test]
    fn sql_literal_quotes_text_and_leaves_numbers_bare() {
        assert_eq!(
            Value::Text("O'Brien".to_string()).sql_literal(crate::dialect::Dialect::Postgres),
            "'O''Brien'"
        );
        assert_eq!(
            Value::Int(42).sql_literal(crate::dialect::Dialect::Postgres),
            "42"
        );
        assert_eq!(
            Value::Null.sql_literal(crate::dialect::Dialect::Postgres),
            "NULL"
        );
    }

    #[test]
    fn sql_literal_renders_bytes_per_dialect() {
        let bytes = Value::Bytes(vec![0xde, 0xad]);
        assert_eq!(
            bytes.sql_literal(crate::dialect::Dialect::Sqlite),
            "X'dead'"
        );
        assert_eq!(
            bytes.sql_literal(crate::dialect::Dialect::Postgres),
            "'\\xdead'"
        );
        assert_eq!(
            bytes.sql_literal(crate::dialect::Dialect::SqlServer),
            "0xdead"
        );
    }

    #[test]
    fn null_renders_the_configured_text() {
        assert_eq!(Value::Null.display(&rules()), "NULL");
        let custom = FormatRules {
            null_text: "<null>".to_string(),
            ..rules()
        };
        assert_eq!(Value::Null.display(&custom), "<null>");
    }

    #[test]
    fn bool_int_float_render_plainly() {
        assert_eq!(Value::Bool(true).display(&rules()), "true");
        assert_eq!(Value::Int(-42).display(&rules()), "-42");
        assert_eq!(Value::Float(1.5).display(&rules()), "1.5");
    }

    #[test]
    fn decimal_is_shown_verbatim_never_reparsed() {
        // A value no f64 could round-trip exactly proves this is text, not
        // a parsed-and-reformatted number.
        let value = Value::Decimal("12345678901234567890.123456789".to_string());
        assert_eq!(value.display(&rules()), "12345678901234567890.123456789");
    }

    #[test]
    fn text_renders_as_is() {
        assert_eq!(Value::Text("hello".to_string()).display(&rules()), "hello");
    }

    #[test]
    fn bytes_render_as_a_hex_preview_with_ellipsis_past_the_limit() {
        let short = Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(short.display(&rules()), "deadbeef");

        let long = Value::Bytes(vec![0xab; 20]);
        let rendered = long.display(&rules());
        assert!(rendered.starts_with(&"ab".repeat(16)));
        assert!(rendered.ends_with('\u{2026}'));
    }

    #[test]
    fn date_time_datetime_use_the_configured_formats() {
        let date = Value::Date(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
        assert_eq!(date.display(&rules()), "2026-09-21");

        let time = Value::Time(NaiveTime::from_hms_opt(13, 5, 9).unwrap());
        assert_eq!(time.display(&rules()), "13:05:09");

        let dt = Value::DateTime(
            NaiveDate::from_ymd_opt(2026, 9, 21)
                .unwrap()
                .and_hms_opt(13, 5, 9)
                .unwrap(),
        );
        assert_eq!(dt.display(&rules()), "2026-09-21 13:05:09");
    }

    #[test]
    fn datetime_tz_renders_in_its_own_offset() {
        let dt = DateTime::parse_from_rfc3339("2026-09-21T13:05:09+02:00").unwrap();
        assert_eq!(
            Value::DateTimeTz(dt).display(&rules()),
            "2026-09-21 13:05:09"
        );
    }

    #[test]
    fn uuid_renders_the_canonical_hyphenated_form() {
        let id = uuid::Uuid::nil();
        assert_eq!(
            Value::Uuid(id).display(&rules()),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn json_renders_the_serialised_text_unchanged() {
        assert_eq!(
            Value::Json("{\"a\":1}".to_string()).display(&rules()),
            "{\"a\":1}"
        );
    }

    #[test]
    fn array_renders_each_item_through_display() {
        let value = Value::Array(vec![
            Value::Int(1),
            Value::Null,
            Value::Text("x".to_string()),
        ]);
        assert_eq!(value.display(&rules()), "[1, NULL, x]");
    }

    #[test]
    fn document_renders_each_field_name_and_value() {
        let value = Value::Document(vec![
            ("a".to_string(), Value::Int(1)),
            ("b".to_string(), Value::Null),
        ]);
        assert_eq!(value.display(&rules()), "{a: 1, b: NULL}");
    }

    #[test]
    fn other_renders_its_pre_built_display_string() {
        let value = Value::Other {
            type_name: "int4range".to_string(),
            display: "[1,10)".to_string(),
        };
        assert_eq!(value.display(&rules()), "[1,10)");
    }

    #[test]
    fn row_batch_approx_size_sums_columns_and_cells() {
        let batch = RowBatch {
            columns: vec![ColumnMeta::new("id", "int", false)],
            rows: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        };
        assert_eq!(batch.approx_size(), "id".len() + 8 + 8);
    }

    #[test]
    fn parse_text_coerces_every_supported_type_name() {
        assert_eq!(parse_text("42", "int4").unwrap(), Value::Int(42));
        assert_eq!(parse_text("-7", "BIGINT").unwrap(), Value::Int(-7));
        assert_eq!(parse_text("1.5", "float8").unwrap(), Value::Float(1.5));
        assert_eq!(parse_text("true", "boolean").unwrap(), Value::Bool(true));
        assert_eq!(parse_text("0", "bool").unwrap(), Value::Bool(false));
        assert_eq!(
            parse_text("12345678901234567890.5", "numeric(30,5)").unwrap(),
            Value::Decimal("12345678901234567890.5".to_string())
        );
        assert_eq!(
            parse_text("00000000-0000-0000-0000-000000000000", "uuid").unwrap(),
            Value::Uuid(uuid::Uuid::nil())
        );
        assert_eq!(
            parse_text("{\"a\":1}", "jsonb").unwrap(),
            Value::Json("{\"a\":1}".to_string())
        );
        assert_eq!(
            parse_text("2026-09-21", "date").unwrap(),
            Value::Date(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap())
        );
        assert_eq!(
            parse_text("13:05:09", "time").unwrap(),
            Value::Time(NaiveTime::from_hms_opt(13, 5, 9).unwrap())
        );
        assert_eq!(
            parse_text("2026-09-21 13:05:09", "timestamp").unwrap(),
            Value::DateTime(
                NaiveDate::from_ymd_opt(2026, 9, 21)
                    .unwrap()
                    .and_hms_opt(13, 5, 9)
                    .unwrap()
            )
        );
        assert_eq!(
            parse_text("deadbeef", "bytea").unwrap(),
            Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
        );
        assert_eq!(
            parse_text("0xdeadbeef", "blob").unwrap(),
            Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
        );
    }

    #[test]
    fn parse_text_falls_back_to_text_for_an_unrecognised_type() {
        assert_eq!(
            parse_text("hello", "hstore").unwrap(),
            Value::Text("hello".to_string())
        );
    }

    #[test]
    fn parse_text_rejects_what_does_not_shape_check_with_a_readable_message() {
        assert!(parse_text("not a number", "int4").is_err());
        assert!(parse_text("not a uuid", "uuid").is_err());
        assert!(parse_text("{broken", "json").is_err());
        assert!(parse_text("zz", "bytea").is_err());
        assert!(parse_text("2026-13-99", "date").is_err());
        let error = parse_text("nope", "int4").unwrap_err();
        assert!(error.contains("nope"));
    }

    #[test]
    fn parse_text_never_panics_on_adversarial_input() {
        // NUL bytes, unicode, empty text — must return `Err`, never panic.
        assert!(parse_text("a\0b", "int4").is_err());
        assert!(parse_text("", "int4").is_err());
        assert!(parse_text("héllo", "uuid").is_err());
        // Text falls back cleanly even for adversarial content.
        assert_eq!(
            parse_text("Robert'); DROP TABLE students;--", "text").unwrap(),
            Value::Text("Robert'); DROP TABLE students;--".to_string())
        );
    }

    /// `security-expert`'s F4.5 finding: a multi-byte UTF-8 character
    /// padded to an even *byte* length has no 2-byte-aligned char
    /// boundary at all — `parse_hex`'s old byte-index slicing panicked
    /// on this instead of returning `Err`.
    #[test]
    fn parse_text_rejects_non_ascii_bytea_text_instead_of_panicking() {
        assert!(parse_text("1€2€", "bytea").is_err());
        assert!(parse_text("€€", "blob").is_err());
    }

    #[test]
    fn pretty_json_indents_a_compact_object() {
        let pretty = pretty_json("{\"a\":1,\"b\":[2,3]}").unwrap();
        assert_eq!(pretty, "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}");
    }

    #[test]
    fn pretty_json_rejects_invalid_json_without_panicking() {
        assert!(pretty_json("{not json").is_err());
        assert!(pretty_json("").is_err());
    }

    #[test]
    fn row_to_json_round_trips_every_variant_losslessly() {
        let row = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(-7),
            Value::Float(1.5),
            Value::Decimal("12345678901234567890.5".to_string()),
            Value::Text("hé llo".to_string()),
            Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
            Value::Date(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()),
            Value::Time(NaiveTime::from_hms_opt(13, 5, 9).unwrap()),
            Value::DateTime(
                NaiveDate::from_ymd_opt(2026, 9, 21)
                    .unwrap()
                    .and_hms_opt(13, 5, 9)
                    .unwrap(),
            ),
            Value::Uuid(uuid::Uuid::nil()),
            Value::Json("{\"a\":1}".to_string()),
            Value::Array(vec![Value::Int(1), Value::Null]),
            Value::Document(vec![("a".to_string(), Value::Int(1))]),
            Value::Other {
                type_name: "int4range".to_string(),
                display: "[1,10)".to_string(),
            },
        ];
        let encoded = row_to_json(&row);
        let decoded = row_from_json(&encoded).unwrap();
        assert_eq!(decoded, row);
    }

    #[test]
    fn row_from_json_rejects_malformed_input_without_panicking() {
        assert!(row_from_json("not json").is_err());
        assert!(row_from_json("").is_err());
    }

    #[test]
    fn is_binary_type_matches_every_blob_like_type_name_case_insensitively() {
        for type_name in ["BLOB", "bytea", "VARBINARY(255)", "binary(16)"] {
            assert!(is_binary_type(type_name), "{type_name} should be binary");
        }
        for type_name in ["int4", "text", "TIMESTAMP"] {
            assert!(
                !is_binary_type(type_name),
                "{type_name} should not be binary"
            );
        }
    }

    #[test]
    fn validate_hex_text_accepts_what_parse_text_would_also_accept() {
        for text in ["deadbeef", "0xDEADBEEF", "\\xdead", "", "  "] {
            assert!(validate_hex_text(text).is_ok(), "{text} should validate");
            assert!(parse_text(text, "bytea").is_ok());
        }
    }

    #[test]
    fn validate_hex_text_rejects_what_parse_text_would_also_reject() {
        for text in ["zz", "abc", "1€2€"] {
            let hex_err = validate_hex_text(text);
            let parse_err = parse_text(text, "bytea");
            assert!(hex_err.is_err(), "{text} should not validate");
            assert!(parse_err.is_err());
            assert_eq!(hex_err.unwrap_err(), parse_err.unwrap_err());
        }
    }
}
