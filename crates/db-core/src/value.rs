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
            columns: vec![ColumnMeta {
                name: "id".to_string(),
                type_name: "int".to_string(),
                nullable: false,
            }],
            rows: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        };
        assert_eq!(batch.approx_size(), "id".len() + 8 + 8);
    }
}
