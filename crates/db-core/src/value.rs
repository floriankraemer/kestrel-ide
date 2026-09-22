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
#[derive(Debug, Clone, PartialEq)]
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
        let trimmed = text
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("\\x");
        if trimmed.is_empty() {
            return Ok(Value::Bytes(Vec::new()));
        }
        return parse_hex(trimmed)
            .map(Value::Bytes)
            .ok_or_else(|| format!("'{text}' is not valid hex"));
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

fn parse_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
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
}
