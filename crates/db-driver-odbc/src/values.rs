//! Maps the standard ODBC C types to `db_core::Value` (F8.2): one
//! `ColumnPlan` per result column, chosen once from its `odbc_api::DataType`
//! at bind time, then `read_cell` pulls the typed data out of the bound
//! `AnyColumnBufferSlice` for each row.
//!
//! Decimal/Numeric columns are deliberately always bound as
//! [`odbc_api::buffers::BufferDesc::Text`] rather than through
//! `BufferDesc::from_data_type`'s narrower `I8`/`I32`/`Numeric` choices:
//! `db_core::Value::Decimal` is the server's own decimal text, verbatim
//! (`db-core/src/value.rs`'s doc comment) — a numeric buffer would round
//! it through a fixed-width type first.

use chrono::{NaiveDate, NaiveTime};
use db_core::value::Value;
use odbc_api::buffers::{AnyColumnBufferSlice, BufferDesc};
use odbc_api::sys::{Date as OdbcDate, Time as OdbcTime, Timestamp as OdbcTimestamp};
use odbc_api::DataType;

/// How one result column was bound, decided once from its `DataType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColKind {
    Bool,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    /// Decimal/Numeric and every other exotic type not covered below —
    /// carried as its driver-rendered text and either kept as
    /// [`Value::Decimal`] (numeric-shaped) or [`Value::Other`] (anything
    /// else), see [`decode_text`].
    Text {
        decimal: bool,
        type_name: String,
    },
    Binary,
    Date,
    Time,
    Timestamp,
}

/// Picks the buffer this column binds as, plus how to read it back.
pub fn plan_column(data_type: DataType, nullable: bool) -> (BufferDesc, ColKind) {
    match data_type {
        DataType::Decimal { precision, .. } | DataType::Numeric { precision, .. } => (
            BufferDesc::Text {
                max_str_len: precision.max(32) + 2, // sign + decimal point
            },
            ColKind::Text {
                decimal: true,
                type_name: "DECIMAL".to_string(),
            },
        ),
        DataType::Bit => (BufferDesc::Bit { nullable }, ColKind::Bool),
        DataType::TinyInt => (BufferDesc::I8 { nullable }, ColKind::I8),
        DataType::SmallInt => (BufferDesc::I16 { nullable }, ColKind::I16),
        DataType::Integer => (BufferDesc::I32 { nullable }, ColKind::I32),
        DataType::BigInt => (BufferDesc::I64 { nullable }, ColKind::I64),
        DataType::Real => (BufferDesc::F32 { nullable }, ColKind::F32),
        DataType::Float { .. } | DataType::Double => (BufferDesc::F64 { nullable }, ColKind::F64),
        DataType::Date => (BufferDesc::Date { nullable }, ColKind::Date),
        DataType::Time { .. } => (BufferDesc::Time { nullable }, ColKind::Time),
        DataType::Timestamp { .. } => (BufferDesc::Timestamp { nullable }, ColKind::Timestamp),
        DataType::Binary { length } | DataType::Varbinary { length } => (
            BufferDesc::Binary {
                max_bytes: length.map(|l| l.get()).unwrap_or(8192),
            },
            ColKind::Binary,
        ),
        DataType::LongVarbinary { length } => (
            BufferDesc::Binary {
                max_bytes: length.map(|l| l.get()).unwrap_or(1 << 20),
            },
            ColKind::Binary,
        ),
        DataType::Char { .. }
        | DataType::WChar { .. }
        | DataType::Varchar { .. }
        | DataType::WVarchar { .. }
        | DataType::LongVarchar { .. }
        | DataType::WLongVarchar { .. } => {
            let max_str_len = data_type.utf8_len().map(|l| l.get()).unwrap_or(4096);
            (
                BufferDesc::Text { max_str_len },
                ColKind::Text {
                    decimal: false,
                    type_name: "TEXT".to_string(),
                },
            )
        }
        other => {
            let max_str_len = other.utf8_len().map(|l| l.get()).unwrap_or(4096);
            (
                BufferDesc::Text { max_str_len },
                ColKind::Text {
                    decimal: false,
                    type_name: format!("{other:?}"),
                },
            )
        }
    }
}

fn odbc_date_to_value(date: OdbcDate) -> Value {
    match NaiveDate::from_ymd_opt(date.year as i32, date.month as u32, date.day as u32) {
        Some(date) => Value::Date(date),
        None => Value::Other {
            type_name: "DATE".to_string(),
            display: format!("{}-{:02}-{:02}", date.year, date.month, date.day),
        },
    }
}

fn odbc_time_to_value(time: OdbcTime) -> Value {
    match NaiveTime::from_hms_opt(time.hour as u32, time.minute as u32, time.second as u32) {
        Some(time) => Value::Time(time),
        None => Value::Other {
            type_name: "TIME".to_string(),
            display: format!("{:02}:{:02}:{:02}", time.hour, time.minute, time.second),
        },
    }
}

fn odbc_timestamp_to_value(ts: OdbcTimestamp) -> Value {
    let date = NaiveDate::from_ymd_opt(ts.year as i32, ts.month as u32, ts.day as u32);
    let time = NaiveTime::from_hms_nano_opt(
        ts.hour as u32,
        ts.minute as u32,
        ts.second as u32,
        ts.fraction,
    );
    match (date, time) {
        (Some(date), Some(time)) => Value::DateTime(date.and_time(time)),
        _ => Value::Other {
            type_name: "TIMESTAMP".to_string(),
            display: format!(
                "{}-{:02}-{:02} {:02}:{:02}:{:02}.{:09}",
                ts.year, ts.month, ts.day, ts.hour, ts.minute, ts.second, ts.fraction
            ),
        },
    }
}

fn decode_text(kind_is_decimal: bool, type_name: &str, text: Option<&[u8]>) -> Value {
    match text {
        None => Value::Null,
        Some(bytes) => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            if kind_is_decimal {
                Value::Decimal(text)
            } else if type_name == "TEXT" {
                Value::Text(text)
            } else {
                Value::Other {
                    type_name: type_name.to_string(),
                    display: text,
                }
            }
        }
    }
}

/// Reads row `row` of column bound as `kind` out of `slice`. Panics (via
/// the `expect`s) only if `kind` disagrees with how the column was
/// actually bound — a programmer error in [`plan_column`], never a
/// driver-data condition.
pub fn read_cell(kind: &ColKind, slice: AnyColumnBufferSlice<'_>, row: usize) -> Value {
    match kind {
        ColKind::Bool => {
            let bits = slice
                .as_nullable_slice::<odbc_api::Bit>()
                .expect("column bound as Bit");
            match bits.get(row) {
                Some(bit) => Value::Bool(bit.as_bool()),
                None => Value::Null,
            }
        }
        ColKind::I8 => read_nullable_int(slice, row, Value::Int),
        ColKind::I16 => read_nullable_int::<i16>(slice, row, |v| Value::Int(v as i64)),
        ColKind::I32 => read_nullable_int::<i32>(slice, row, |v| Value::Int(v as i64)),
        ColKind::I64 => read_nullable_int::<i64>(slice, row, Value::Int),
        ColKind::F32 => {
            let values = slice
                .as_nullable_slice::<f32>()
                .expect("column bound as F32");
            match values.get(row) {
                Some(v) => Value::Float(*v as f64),
                None => Value::Null,
            }
        }
        ColKind::F64 => {
            let values = slice
                .as_nullable_slice::<f64>()
                .expect("column bound as F64");
            match values.get(row) {
                Some(v) => Value::Float(*v),
                None => Value::Null,
            }
        }
        ColKind::Text { decimal, type_name } => {
            let text = slice.as_text().expect("column bound as Text");
            decode_text(*decimal, type_name, text.get(row))
        }
        ColKind::Binary => {
            let binary = slice.as_binary().expect("column bound as Binary");
            match binary.get(row) {
                Some(bytes) => Value::Bytes(bytes.to_vec()),
                None => Value::Null,
            }
        }
        ColKind::Date => {
            let values = slice
                .as_nullable_slice::<OdbcDate>()
                .expect("column bound as Date");
            match values.get(row) {
                Some(v) => odbc_date_to_value(*v),
                None => Value::Null,
            }
        }
        ColKind::Time => {
            let values = slice
                .as_nullable_slice::<OdbcTime>()
                .expect("column bound as Time");
            match values.get(row) {
                Some(v) => odbc_time_to_value(*v),
                None => Value::Null,
            }
        }
        ColKind::Timestamp => {
            let values = slice
                .as_nullable_slice::<OdbcTimestamp>()
                .expect("column bound as Timestamp");
            match values.get(row) {
                Some(v) => odbc_timestamp_to_value(*v),
                None => Value::Null,
            }
        }
    }
}

fn read_nullable_int<T>(
    slice: AnyColumnBufferSlice<'_>,
    row: usize,
    to_value: impl Fn(T) -> Value,
) -> Value
where
    T: odbc_api::Pod,
{
    let values = slice
        .as_nullable_slice::<T>()
        .expect("column bound as an integer type");
    match values.get(row) {
        Some(v) => to_value(*v),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_date_maps_to_value_date() {
        let value = odbc_date_to_value(OdbcDate {
            year: 2026,
            month: 9,
            day: 21,
        });
        assert_eq!(
            value,
            Value::Date(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap())
        );
    }

    #[test]
    fn an_impossible_date_falls_back_to_other_rather_than_panicking() {
        let value = odbc_date_to_value(OdbcDate {
            year: 2026,
            month: 2,
            day: 30,
        });
        match value {
            Value::Other { type_name, display } => {
                assert_eq!(type_name, "DATE");
                assert_eq!(display, "2026-02-30");
            }
            other => panic!("expected Value::Other, got {other:?}"),
        }
    }

    #[test]
    fn a_valid_time_maps_to_value_time() {
        let value = odbc_time_to_value(OdbcTime {
            hour: 13,
            minute: 5,
            second: 9,
        });
        assert_eq!(
            value,
            Value::Time(NaiveTime::from_hms_opt(13, 5, 9).unwrap())
        );
    }

    #[test]
    fn a_timestamp_carries_its_fractional_seconds_as_nanoseconds() {
        let value = odbc_timestamp_to_value(OdbcTimestamp {
            year: 2026,
            month: 9,
            day: 21,
            hour: 13,
            minute: 5,
            second: 9,
            fraction: 123_000_000,
        });
        let expected = NaiveDate::from_ymd_opt(2026, 9, 21)
            .unwrap()
            .and_hms_nano_opt(13, 5, 9, 123_000_000)
            .unwrap();
        assert_eq!(value, Value::DateTime(expected));
    }

    #[test]
    fn decimal_text_is_kept_verbatim_never_reparsed() {
        let text = b"12345678901234567890.123456789";
        let value = decode_text(true, "DECIMAL", Some(text));
        assert_eq!(
            value,
            Value::Decimal("12345678901234567890.123456789".to_string())
        );
    }

    #[test]
    fn a_null_text_cell_maps_to_value_null() {
        assert_eq!(decode_text(true, "DECIMAL", None), Value::Null);
        assert_eq!(decode_text(false, "TEXT", None), Value::Null);
    }

    #[test]
    fn an_unrecognised_type_keeps_both_its_name_and_display_text() {
        let value = decode_text(false, "INTERVAL_YEAR", Some(b"5"));
        assert_eq!(
            value,
            Value::Other {
                type_name: "INTERVAL_YEAR".to_string(),
                display: "5".to_string(),
            }
        );
    }

    #[test]
    fn plain_text_decodes_as_value_text() {
        let value = decode_text(false, "TEXT", Some(b"hello"));
        assert_eq!(value, Value::Text("hello".to_string()));
    }

    #[test]
    fn plan_column_forces_decimal_and_numeric_through_text_never_a_numeric_buffer() {
        let (desc, kind) = plan_column(
            DataType::Decimal {
                precision: 38,
                scale: 10,
            },
            true,
        );
        assert!(matches!(desc, BufferDesc::Text { .. }));
        assert!(matches!(kind, ColKind::Text { decimal: true, .. }));
    }

    #[test]
    fn plan_column_maps_integer_family_to_the_matching_int_buffer() {
        let (desc, kind) = plan_column(DataType::BigInt, true);
        assert!(matches!(desc, BufferDesc::I64 { nullable: true }));
        assert_eq!(kind, ColKind::I64);
    }
}
