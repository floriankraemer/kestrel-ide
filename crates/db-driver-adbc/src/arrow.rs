//! Converts an Arrow `RecordBatch` into `db_core::value::RowBatch`
//! (F8.1) — the only Arrow-aware code in the workspace (layering.md's
//! `db-driver-adbc` row). One branch per `DataType`, each covered by a
//! unit test that builds the corresponding array in memory (no ADBC
//! driver, no network).

use arrow_array::cast::AsArray;
use arrow_array::types::*;
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, TimeUnit};

use db_core::value::{ColumnMeta, RowBatch, Value};

/// `RecordBatch` → `db_core::value::RowBatch`: same column list, one
/// `Value` per cell via [`value_at`].
pub fn convert_batch(batch: &RecordBatch) -> RowBatch {
    let columns: Vec<ColumnMeta> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| ColumnMeta {
            name: field.name().clone(),
            type_name: format!("{:?}", field.data_type()),
            nullable: field.is_nullable(),
            origin: None,
        })
        .collect();

    let num_rows = batch.num_rows();
    let mut rows = Vec::with_capacity(num_rows);
    for row in 0..num_rows {
        let mut values = Vec::with_capacity(batch.num_columns());
        for column in batch.columns() {
            values.push(value_at(column.as_ref(), row));
        }
        rows.push(values);
    }

    RowBatch { columns, rows }
}

fn decimal_to_string(digits: &str, negative: bool, scale: i8) -> String {
    let sign = if negative { "-" } else { "" };
    if scale <= 0 {
        // A non-positive scale means the integer already represents the
        // full magnitude (e.g. scale -2 => value * 100); this driver has
        // no fractional part to insert, so it renders the digits as-is.
        return format!("{sign}{digits}");
    }
    let scale = scale as usize;
    if digits.len() <= scale {
        let padded = format!("{:0>width$}", digits, width = scale + 1);
        let (whole, frac) = padded.split_at(padded.len() - scale);
        format!("{sign}{whole}.{frac}")
    } else {
        let (whole, frac) = digits.split_at(digits.len() - scale);
        format!("{sign}{whole}.{frac}")
    }
}

fn other(field_type: &DataType, display: String) -> Value {
    Value::Other {
        type_name: format!("{field_type:?}"),
        display,
    }
}

/// Reads row `row` of `array` as a `db_core::Value`.
pub fn value_at(array: &dyn Array, row: usize) -> Value {
    if array.is_null(row) {
        return Value::Null;
    }
    match array.data_type() {
        DataType::Null => Value::Null,
        DataType::Boolean => Value::Bool(array.as_boolean().value(row)),
        DataType::Int8 => Value::Int(array.as_primitive::<Int8Type>().value(row) as i64),
        DataType::Int16 => Value::Int(array.as_primitive::<Int16Type>().value(row) as i64),
        DataType::Int32 => Value::Int(array.as_primitive::<Int32Type>().value(row) as i64),
        DataType::Int64 => Value::Int(array.as_primitive::<Int64Type>().value(row)),
        DataType::UInt8 => Value::Int(array.as_primitive::<UInt8Type>().value(row) as i64),
        DataType::UInt16 => Value::Int(array.as_primitive::<UInt16Type>().value(row) as i64),
        DataType::UInt32 => Value::Int(array.as_primitive::<UInt32Type>().value(row) as i64),
        DataType::UInt64 => Value::Int(array.as_primitive::<UInt64Type>().value(row) as i64),
        DataType::Float32 => Value::Float(array.as_primitive::<Float32Type>().value(row) as f64),
        DataType::Float64 => Value::Float(array.as_primitive::<Float64Type>().value(row)),
        DataType::Decimal128(_, scale) => {
            let raw = array.as_primitive::<Decimal128Type>().value(row);
            let negative = raw < 0;
            let digits = raw.unsigned_abs().to_string();
            Value::Decimal(decimal_to_string(&digits, negative, *scale))
        }
        DataType::Decimal256(_, scale) => {
            let raw = array.as_primitive::<Decimal256Type>().value(row);
            let negative = raw.is_negative();
            let digits = raw.wrapping_abs().to_string();
            Value::Decimal(decimal_to_string(&digits, negative, *scale))
        }
        DataType::Utf8 => Value::Text(array.as_string::<i32>().value(row).to_string()),
        DataType::LargeUtf8 => Value::Text(array.as_string::<i64>().value(row).to_string()),
        DataType::Binary => Value::Bytes(array.as_binary::<i32>().value(row).to_vec()),
        DataType::LargeBinary => Value::Bytes(array.as_binary::<i64>().value(row).to_vec()),
        DataType::Date32 => {
            let raw = array.as_primitive::<Date32Type>().value(row);
            match arrow_array::temporal_conversions::date32_to_datetime(raw) {
                Some(dt) => Value::Date(dt.date()),
                None => other(array.data_type(), raw.to_string()),
            }
        }
        DataType::Date64 => {
            let raw = array.as_primitive::<Date64Type>().value(row);
            match arrow_array::temporal_conversions::date64_to_datetime(raw) {
                Some(dt) => Value::Date(dt.date()),
                None => other(array.data_type(), raw.to_string()),
            }
        }
        DataType::Time32(TimeUnit::Second) => {
            let raw = array.as_primitive::<Time32SecondType>().value(row);
            time_value(
                arrow_array::temporal_conversions::time32s_to_time(raw),
                array.data_type(),
                raw,
            )
        }
        DataType::Time32(TimeUnit::Millisecond) => {
            let raw = array.as_primitive::<Time32MillisecondType>().value(row);
            time_value(
                arrow_array::temporal_conversions::time32ms_to_time(raw),
                array.data_type(),
                raw,
            )
        }
        DataType::Time64(TimeUnit::Microsecond) => {
            let raw = array.as_primitive::<Time64MicrosecondType>().value(row);
            time_value(
                arrow_array::temporal_conversions::time64us_to_time(raw),
                array.data_type(),
                raw,
            )
        }
        DataType::Time64(TimeUnit::Nanosecond) => {
            let raw = array.as_primitive::<Time64NanosecondType>().value(row);
            time_value(
                arrow_array::temporal_conversions::time64ns_to_time(raw),
                array.data_type(),
                raw,
            )
        }
        DataType::Time32(_) | DataType::Time64(_) => {
            other(array.data_type(), "<unsupported time unit>".to_string())
        }
        DataType::Timestamp(unit, tz) => timestamp_value(array, *unit, tz.as_deref(), row),
        DataType::List(_) => {
            let list = array.as_list::<i32>();
            let element = list.value(row);
            Value::Array(
                (0..element.len())
                    .map(|i| value_at(element.as_ref(), i))
                    .collect(),
            )
        }
        DataType::LargeList(_) => {
            let list = array.as_list::<i64>();
            let element = list.value(row);
            Value::Array(
                (0..element.len())
                    .map(|i| value_at(element.as_ref(), i))
                    .collect(),
            )
        }
        DataType::Struct(_) => {
            let s = array.as_struct();
            let fields: Vec<(String, Value)> = s
                .fields()
                .iter()
                .zip(s.columns())
                .map(|(field, column)| (field.name().clone(), value_at(column.as_ref(), row)))
                .collect();
            Value::Document(fields)
        }
        other_type => other(other_type, format!("{other_type:?}")),
    }
}

fn time_value(
    time: Option<chrono::NaiveTime>,
    data_type: &DataType,
    raw: impl std::fmt::Display,
) -> Value {
    match time {
        Some(t) => Value::Time(t),
        None => other(data_type, raw.to_string()),
    }
}

fn timestamp_value(array: &dyn Array, unit: TimeUnit, tz: Option<&str>, row: usize) -> Value {
    let naive = match unit {
        TimeUnit::Second => arrow_array::temporal_conversions::as_datetime::<TimestampSecondType>(
            array.as_primitive::<TimestampSecondType>().value(row),
        ),
        TimeUnit::Millisecond => {
            arrow_array::temporal_conversions::as_datetime::<TimestampMillisecondType>(
                array.as_primitive::<TimestampMillisecondType>().value(row),
            )
        }
        TimeUnit::Microsecond => {
            arrow_array::temporal_conversions::as_datetime::<TimestampMicrosecondType>(
                array.as_primitive::<TimestampMicrosecondType>().value(row),
            )
        }
        TimeUnit::Nanosecond => {
            arrow_array::temporal_conversions::as_datetime::<TimestampNanosecondType>(
                array.as_primitive::<TimestampNanosecondType>().value(row),
            )
        }
    };
    let Some(naive) = naive else {
        return other(array.data_type(), "<out of range timestamp>".to_string());
    };
    match tz {
        None => Value::DateTime(naive),
        Some(tz) => match tz.parse::<chrono::FixedOffset>() {
            Ok(offset) => {
                Value::DateTimeTz(chrono::DateTime::from_naive_utc_and_offset(naive, offset))
            }
            // A named zone ("UTC", "America/New_York", ...) rather than a
            // fixed offset: this driver has no IANA tz database dependency
            // (`chrono-tz` is not part of this crate's dependency list),
            // so it renders the value pre-formatted rather than adding one
            // just for this branch.
            Err(_) => other(
                array.data_type(),
                format!("{} {tz}", naive.format("%Y-%m-%d %H:%M:%S%.f")),
            ),
        },
    }
}

/// Builds a one-column, one-row `RecordBatch` for a test — every unit
/// test below uses this instead of a fixture file, since Arrow arrays are
/// built in memory either way.
#[cfg(test)]
fn single_column_batch(name: &str, array: std::sync::Arc<dyn Array>) -> RecordBatch {
    use arrow_schema::{Field, Schema};
    use std::sync::Arc;
    let field = Field::new(name, array.data_type().clone(), true);
    let schema = Arc::new(Schema::new(vec![field]));
    RecordBatch::try_new(schema, vec![array]).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::builder::{Int32Builder, ListBuilder, StructBuilder};
    use arrow_array::{
        BooleanArray, Date32Array, Decimal128Array, Decimal256Array, Float64Array, Int64Array,
        StringArray, TimestampMicrosecondArray,
    };
    use arrow_buffer::i256;
    use arrow_schema::Fields;
    use std::sync::Arc;

    #[test]
    fn null_is_null_regardless_of_declared_type() {
        let array = Int64Array::from(vec![None]);
        let batch = single_column_batch("n", Arc::new(array));
        let converted = convert_batch(&batch);
        assert_eq!(converted.rows[0][0], Value::Null);
    }

    #[test]
    fn boolean_maps_to_value_bool() {
        let array = BooleanArray::from(vec![true]);
        let batch = single_column_batch("b", Arc::new(array));
        assert_eq!(convert_batch(&batch).rows[0][0], Value::Bool(true));
    }

    #[test]
    fn int64_maps_to_value_int() {
        let array = Int64Array::from(vec![42]);
        let batch = single_column_batch("i", Arc::new(array));
        assert_eq!(convert_batch(&batch).rows[0][0], Value::Int(42));
    }

    #[test]
    fn float64_maps_to_value_float() {
        let array = Float64Array::from(vec![1.5]);
        let batch = single_column_batch("f", Arc::new(array));
        assert_eq!(convert_batch(&batch).rows[0][0], Value::Float(1.5));
    }

    #[test]
    fn utf8_maps_to_value_text() {
        let array = StringArray::from(vec!["hello"]);
        let batch = single_column_batch("s", Arc::new(array));
        assert_eq!(
            convert_batch(&batch).rows[0][0],
            Value::Text("hello".to_string())
        );
    }

    #[test]
    fn decimal128_is_rendered_with_its_scale_never_reparsed_as_a_float() {
        let array = Decimal128Array::from(vec![123456789i128])
            .with_precision_and_scale(18, 4)
            .unwrap();
        let batch = single_column_batch("d", Arc::new(array));
        assert_eq!(
            convert_batch(&batch).rows[0][0],
            Value::Decimal("12345.6789".to_string())
        );
    }

    #[test]
    fn decimal128_keeps_the_sign_of_a_negative_value() {
        let array = Decimal128Array::from(vec![-12345i128])
            .with_precision_and_scale(10, 2)
            .unwrap();
        let batch = single_column_batch("d", Arc::new(array));
        assert_eq!(
            convert_batch(&batch).rows[0][0],
            Value::Decimal("-123.45".to_string())
        );
    }

    #[test]
    fn decimal256_is_rendered_with_its_scale() {
        let array = Decimal256Array::from(vec![i256::from_i128(123456)])
            .with_precision_and_scale(30, 3)
            .unwrap();
        let batch = single_column_batch("d", Arc::new(array));
        assert_eq!(
            convert_batch(&batch).rows[0][0],
            Value::Decimal("123.456".to_string())
        );
    }

    #[test]
    fn date32_maps_to_value_date() {
        let expected = chrono::NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        let days_since_epoch = (expected - epoch).num_days() as i32;
        let array = Date32Array::from(vec![days_since_epoch]);
        let batch = single_column_batch("d", Arc::new(array));
        assert_eq!(convert_batch(&batch).rows[0][0], Value::Date(expected));
    }

    #[test]
    fn timestamp_without_timezone_maps_to_value_datetime() {
        let array = TimestampMicrosecondArray::from(vec![1_758_452_709_000_000]);
        let batch = single_column_batch("t", Arc::new(array));
        match &convert_batch(&batch).rows[0][0] {
            Value::DateTime(_) => {}
            other => panic!("expected Value::DateTime, got {other:?}"),
        }
    }

    #[test]
    fn timestamp_with_a_fixed_offset_maps_to_value_datetime_tz() {
        let array = TimestampMicrosecondArray::from(vec![1_758_452_709_000_000])
            .with_timezone("+02:00".to_string());
        let batch = single_column_batch("t", Arc::new(array));
        match &convert_batch(&batch).rows[0][0] {
            Value::DateTimeTz(_) => {}
            other => panic!("expected Value::DateTimeTz, got {other:?}"),
        }
    }

    #[test]
    fn list_maps_to_value_array_of_its_elements() {
        let mut builder = ListBuilder::new(Int32Builder::new());
        builder.values().append_value(1);
        builder.values().append_value(2);
        builder.append(true);
        let array = builder.finish();
        let batch = single_column_batch("l", Arc::new(array));
        assert_eq!(
            convert_batch(&batch).rows[0][0],
            Value::Array(vec![Value::Int(1), Value::Int(2)])
        );
    }

    #[test]
    fn struct_maps_to_value_document_with_field_names() {
        let fields = Fields::from(vec![
            arrow_schema::Field::new("x", DataType::Int32, false),
            arrow_schema::Field::new("y", DataType::Int32, false),
        ]);
        let mut builder = StructBuilder::new(
            fields.clone(),
            vec![Box::new(Int32Builder::new()), Box::new(Int32Builder::new())],
        );
        builder
            .field_builder::<Int32Builder>(0)
            .unwrap()
            .append_value(1);
        builder
            .field_builder::<Int32Builder>(1)
            .unwrap()
            .append_value(2);
        builder.append(true);
        let array = builder.finish();
        let batch = single_column_batch("s", Arc::new(array));
        assert_eq!(
            convert_batch(&batch).rows[0][0],
            Value::Document(vec![
                ("x".to_string(), Value::Int(1)),
                ("y".to_string(), Value::Int(2)),
            ])
        );
    }

    #[test]
    fn an_unsupported_type_falls_back_to_other_with_a_display_string() {
        // Interval types have no `Value` case; confirm the fallback still
        // carries a usable name and display rather than panicking.
        use arrow_array::IntervalYearMonthArray;
        let array = IntervalYearMonthArray::from(vec![5]);
        let batch = single_column_batch("iv", Arc::new(array));
        match &convert_batch(&batch).rows[0][0] {
            Value::Other { type_name, .. } => assert!(type_name.contains("Interval")),
            other => panic!("expected Value::Other, got {other:?}"),
        }
    }
}
