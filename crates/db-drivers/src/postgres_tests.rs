//! Unit tests for [`super::postgres`] — split out to keep
//! `postgres.rs` under the file-size ceiling (the coverage pass added
//! enough cases to cross it).
use super::*;

/// A `PostgresRowStream` sees only `Vec<Value>`, never a
/// `tokio_postgres::Row` (which has no public constructor) — so its
/// batching/`max_rows`/error-propagation logic is exercised here
/// against a fake `futures_util::stream::iter`, with no real server
/// needed; `db-integration`'s tests below cover the real wire mapping.
fn fake_stream(
    values: Vec<Result<Vec<Value>, DbError>>,
) -> Pin<Box<dyn Stream<Item = Result<Vec<Value>, DbError>> + Send>> {
    Box::pin(futures_util::stream::iter(values))
}

fn one_int_column() -> Vec<ColumnMeta> {
    vec![ColumnMeta {
        name: "n".to_string(),
        type_name: "int4".to_string(),
        nullable: true,
        origin: None,
    }]
}

#[test]
fn next_batch_pages_a_live_stream_by_fetch_size() {
    let values: Vec<Result<Vec<Value>, DbError>> =
        (0..5).map(|n| Ok(vec![Value::Int(n)])).collect();
    let options = ExecOptions {
        fetch_size: 2,
        ..Default::default()
    };
    let mut stream = PostgresRowStream::new(one_int_column(), &options, fake_stream(values));

    let first = stream.next_batch().unwrap().unwrap();
    assert_eq!(first.rows, vec![vec![Value::Int(0)], vec![Value::Int(1)]]);
    let second = stream.next_batch().unwrap().unwrap();
    assert_eq!(second.rows, vec![vec![Value::Int(2)], vec![Value::Int(3)]]);
    let third = stream.next_batch().unwrap().unwrap();
    assert_eq!(third.rows, vec![vec![Value::Int(4)]]);
    // Exhausted: further calls stay `None` without re-polling the stream.
    assert!(stream.next_batch().unwrap().is_none());
    assert!(stream.next_batch().unwrap().is_none());
}

#[test]
fn max_rows_caps_the_total_even_with_more_available_in_the_stream() {
    let values: Vec<Result<Vec<Value>, DbError>> =
        (0..10).map(|n| Ok(vec![Value::Int(n)])).collect();
    let options = ExecOptions {
        fetch_size: 3,
        max_rows: Some(4),
        ..Default::default()
    };
    let mut stream = PostgresRowStream::new(one_int_column(), &options, fake_stream(values));

    let mut seen = 0usize;
    while let Some(batch) = stream.next_batch().unwrap() {
        seen += batch.rows.len();
    }
    assert_eq!(seen, 4);
}

#[test]
fn an_error_mid_stream_surfaces_as_a_db_error() {
    let values = vec![
        Ok(vec![Value::Int(1)]),
        Err(DbError::new(DbErrorCode::Unknown, "boom")),
    ];
    let options = ExecOptions {
        fetch_size: 1,
        ..Default::default()
    };
    let mut stream = PostgresRowStream::new(one_int_column(), &options, fake_stream(values));

    let first = stream.next_batch().unwrap().unwrap();
    assert_eq!(first.rows, vec![vec![Value::Int(1)]]);
    let error = stream.next_batch().unwrap_err();
    assert_eq!(error.code, DbErrorCode::Unknown);
}

#[test]
fn an_empty_stream_yields_no_batches() {
    let mut stream = PostgresRowStream::new(
        one_int_column(),
        &ExecOptions::default(),
        fake_stream(vec![]),
    );
    assert!(stream.next_batch().unwrap().is_none());
}

#[test]
fn decode_numeric_zero() {
    // ndigits=0, weight=0, sign=0, dscale=0
    let raw = [0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(decode_numeric(&raw).unwrap(), "0");
}

#[test]
fn decode_numeric_positive_with_fraction() {
    // 123.45: digits [123, 4500], weight=0, sign=0x0000, dscale=2
    let mut raw = vec![];
    raw.extend_from_slice(&2i16.to_be_bytes()); // ndigits
    raw.extend_from_slice(&0i16.to_be_bytes()); // weight
    raw.extend_from_slice(&0u16.to_be_bytes()); // sign (positive)
    raw.extend_from_slice(&2u16.to_be_bytes()); // dscale
    raw.extend_from_slice(&123i16.to_be_bytes());
    raw.extend_from_slice(&4500i16.to_be_bytes());
    assert_eq!(decode_numeric(&raw).unwrap(), "123.45");
}

#[test]
fn decode_numeric_negative() {
    let mut raw = vec![];
    raw.extend_from_slice(&2i16.to_be_bytes());
    raw.extend_from_slice(&0i16.to_be_bytes());
    raw.extend_from_slice(&0x4000u16.to_be_bytes()); // negative
    raw.extend_from_slice(&2u16.to_be_bytes());
    raw.extend_from_slice(&123i16.to_be_bytes());
    raw.extend_from_slice(&4500i16.to_be_bytes());
    assert_eq!(decode_numeric(&raw).unwrap(), "-123.45");
}

#[test]
fn decode_numeric_nan() {
    let mut raw = vec![];
    raw.extend_from_slice(&0i16.to_be_bytes());
    raw.extend_from_slice(&0i16.to_be_bytes());
    raw.extend_from_slice(&0xC000u16.to_be_bytes());
    raw.extend_from_slice(&0u16.to_be_bytes());
    assert_eq!(decode_numeric(&raw).unwrap(), "NaN");
}

#[test]
fn decode_numeric_integer_no_scale() {
    // 42: digits [42], weight=0, dscale=0
    let mut raw = vec![];
    raw.extend_from_slice(&1i16.to_be_bytes());
    raw.extend_from_slice(&0i16.to_be_bytes());
    raw.extend_from_slice(&0u16.to_be_bytes());
    raw.extend_from_slice(&0u16.to_be_bytes());
    raw.extend_from_slice(&42i16.to_be_bytes());
    assert_eq!(decode_numeric(&raw).unwrap(), "42");
}

#[test]
fn decode_numeric_small_fraction_below_one() {
    // 0.001: digits [10], weight=-1, dscale=3
    // group value 10 at weight -1 means 10 * 10000^-1 = 0.001
    let mut raw = vec![];
    raw.extend_from_slice(&1i16.to_be_bytes());
    raw.extend_from_slice(&(-1i16).to_be_bytes());
    raw.extend_from_slice(&0u16.to_be_bytes());
    raw.extend_from_slice(&3u16.to_be_bytes());
    raw.extend_from_slice(&10i16.to_be_bytes());
    assert_eq!(decode_numeric(&raw).unwrap(), "0.001");
}

#[test]
fn introspection_fixture_text_shapes_the_expected_columns() {
    assert!(NAMES_QUERY.contains("pg_catalog.pg_class"));
    assert!(NAMES_QUERY.contains("relkind"));
    assert!(COLUMNS_QUERY.contains("pg_attribute"));
    assert!(COLUMNS_QUERY.contains("$1"));
    assert!(COLUMNS_QUERY.contains("$2"));
    assert!(COLUMNS_QUERY.contains("is_primary_key"));
}

#[test]
fn table_name_by_oid_query_targets_pg_class_by_oid() {
    assert!(TABLE_NAME_BY_OID_QUERY.contains("pg_catalog.pg_class"));
    assert!(TABLE_NAME_BY_OID_QUERY.contains("oid = $1"));
    assert!(TABLE_NAME_BY_OID_QUERY.contains("relname"));
}

#[test]
fn constraints_fixture_text_names_pg_constraint_and_the_fk_reference() {
    assert!(CONSTRAINTS_QUERY.contains("pg_catalog.pg_constraint"));
    assert!(CONSTRAINTS_QUERY.contains("confrelid"));
    assert!(CONSTRAINTS_QUERY.contains("confupdtype"));
    assert!(CONSTRAINTS_QUERY.contains("confdeltype"));
    assert!(CONSTRAINTS_QUERY.contains("pg_get_constraintdef"));
    assert!(CONSTRAINTS_QUERY.contains("$1"));
    assert!(CONSTRAINTS_QUERY.contains("$2"));
}

#[test]
fn indexes_fixture_text_names_pg_index_and_its_access_method() {
    assert!(INDEXES_QUERY.contains("pg_catalog.pg_index"));
    assert!(INDEXES_QUERY.contains("indisunique"));
    assert!(INDEXES_QUERY.contains("pg_am"));
    assert!(INDEXES_QUERY.contains("$1"));
    assert!(INDEXES_QUERY.contains("$2"));
}

#[test]
fn pgvalue_from_sql_maps_every_wire_type() {
    assert_eq!(
        PgValue::from_sql(&PgType::BOOL, &[1]).unwrap().0,
        Value::Bool(true)
    );
    assert_eq!(
        PgValue::from_sql(&PgType::INT2, &2i16.to_be_bytes())
            .unwrap()
            .0,
        Value::Int(2)
    );
    assert_eq!(
        PgValue::from_sql(&PgType::INT4, &4i32.to_be_bytes())
            .unwrap()
            .0,
        Value::Int(4)
    );
    assert_eq!(
        PgValue::from_sql(&PgType::INT8, &8i64.to_be_bytes())
            .unwrap()
            .0,
        Value::Int(8)
    );
    assert_eq!(
        PgValue::from_sql(&PgType::FLOAT4, &1.5f32.to_be_bytes())
            .unwrap()
            .0,
        Value::Float(1.5f32 as f64)
    );
    assert_eq!(
        PgValue::from_sql(&PgType::FLOAT8, &2.5f64.to_be_bytes())
            .unwrap()
            .0,
        Value::Float(2.5)
    );
    for ty in [PgType::TEXT, PgType::VARCHAR, PgType::BPCHAR, PgType::NAME] {
        assert_eq!(
            PgValue::from_sql(&ty, b"hi").unwrap().0,
            Value::Text("hi".to_string())
        );
    }
    assert_eq!(
        PgValue::from_sql(&PgType::BYTEA, &[1, 2, 3]).unwrap().0,
        Value::Bytes(vec![1, 2, 3])
    );
    let uuid = uuid::Uuid::nil();
    assert_eq!(
        PgValue::from_sql(&PgType::UUID, uuid.as_bytes()).unwrap().0,
        Value::Uuid(uuid)
    );
    assert_eq!(
        PgValue::from_sql(&PgType::JSON, b"{}").unwrap().0,
        Value::Json("{}".to_string())
    );
    let mut jsonb_bytes = vec![1u8];
    jsonb_bytes.extend_from_slice(b"{}");
    assert_eq!(
        PgValue::from_sql(&PgType::JSONB, &jsonb_bytes).unwrap().0,
        Value::Json("{}".to_string())
    );
    assert_eq!(
        PgValue::from_sql(&PgType::OID, &[0, 0, 0, 1]).unwrap().0,
        Value::Other {
            type_name: "oid".to_string(),
            display: String::from_utf8_lossy(&[0, 0, 0, 1]).into_owned(),
        }
    );
}

#[test]
fn pgvalue_from_sql_decodes_numeric_through_the_shared_decoder() {
    let raw: Vec<u8> = vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 5];
    let value = PgValue::from_sql(&PgType::NUMERIC, &raw).unwrap().0;
    assert_eq!(value, Value::Decimal("5".to_string()));
}

#[test]
fn pgvalue_from_sql_null_is_value_null_and_accepts_is_permissive() {
    assert_eq!(
        PgValue::from_sql_null(&PgType::TEXT).unwrap().0,
        Value::Null
    );
    assert!(PgValue::accepts(&PgType::TEXT));
    assert!(PgValue::accepts(&PgType::OID));
}

#[test]
fn pgparam_to_sql_encodes_every_value_variant() {
    let mut buf = BytesMut::new();
    assert!(matches!(
        PgParam(&Value::Null)
            .to_sql(&PgType::TEXT, &mut buf)
            .unwrap(),
        IsNull::Yes
    ));
    for value in [
        Value::Bool(true),
        Value::Int(3),
        Value::Float(1.5),
        Value::Text("hi".to_string()),
        Value::Decimal("1.5".to_string()),
        Value::Json("{}".to_string()),
        Value::Bytes(vec![1, 2]),
        Value::Uuid(uuid::Uuid::nil()),
        Value::Date(chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
        Value::Time(chrono::NaiveTime::from_hms_opt(1, 2, 3).unwrap()),
        Value::DateTime(
            chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .and_hms_opt(1, 2, 3)
                .unwrap(),
        ),
        Value::Bool(false),
    ] {
        let mut buf = BytesMut::new();
        let ty = match &value {
            Value::Date(_) => PgType::DATE,
            Value::Time(_) => PgType::TIME,
            Value::DateTime(_) => PgType::TIMESTAMP,
            Value::Uuid(_) => PgType::UUID,
            Value::Bytes(_) => PgType::BYTEA,
            _ => PgType::TEXT,
        };
        PgParam(&value).to_sql(&ty, &mut buf).unwrap();
    }
    assert!(<PgParam as ToSql>::accepts(&PgType::TEXT));
}

#[test]
fn pgparam_to_sql_falls_back_to_display_text_for_other_variants() {
    let mut buf = BytesMut::new();
    PgParam(&Value::Array(vec![Value::Int(1)]))
        .to_sql(&PgType::TEXT, &mut buf)
        .unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn constraint_kind_for_maps_primary_key_unique_and_check() {
    assert_eq!(
        constraint_kind_for(
            'p',
            vec!["id".to_string()],
            None,
            None,
            vec![],
            'a',
            'a',
            String::new(),
        ),
        Some(ConstraintKind::PrimaryKey {
            columns: vec!["id".to_string()]
        })
    );
    assert_eq!(
        constraint_kind_for(
            'u',
            vec!["sku".to_string()],
            None,
            None,
            vec![],
            'a',
            'a',
            String::new(),
        ),
        Some(ConstraintKind::Unique {
            columns: vec!["sku".to_string()]
        })
    );
    assert_eq!(
        constraint_kind_for(
            'c',
            vec![],
            None,
            None,
            vec![],
            'a',
            'a',
            "qty > 0".to_string(),
        ),
        Some(ConstraintKind::Check {
            expr: "qty > 0".to_string()
        })
    );
}

#[test]
fn constraint_kind_for_maps_a_foreign_key_with_its_schema_and_rules() {
    let kind = constraint_kind_for(
        'f',
        vec!["order_id".to_string()],
        Some("shop".to_string()),
        Some("orders".to_string()),
        vec!["id".to_string()],
        'c',
        'a',
        String::new(),
    )
    .unwrap();
    match kind {
        ConstraintKind::ForeignKey {
            ref_table,
            on_update,
            on_delete,
            ..
        } => {
            assert_eq!(ref_table.name, "orders");
            assert_eq!(ref_table.schema, Some("shop".to_string()));
            assert_eq!(on_update, Some("CASCADE".to_string()));
            assert_eq!(on_delete, None);
        }
        other => panic!("expected ForeignKey, got {other:?}"),
    }
}

#[test]
fn constraint_kind_for_returns_none_for_an_unmodelled_contype() {
    assert_eq!(
        constraint_kind_for('t', vec![], None, None, vec![], 'a', 'a', String::new()),
        None
    );
    assert_eq!(
        constraint_kind_for('x', vec![], None, None, vec![], 'a', 'a', String::new()),
        None
    );
}

#[test]
fn referential_action_maps_the_known_codes_and_omits_no_action() {
    assert_eq!(referential_action("a"), None);
    assert_eq!(referential_action(""), None);
    assert_eq!(referential_action("c"), Some("CASCADE".to_string()));
    assert_eq!(referential_action("n"), Some("SET NULL".to_string()));
    assert_eq!(referential_action("d"), Some("SET DEFAULT".to_string()));
    assert_eq!(referential_action("r"), Some("RESTRICT".to_string()));
}

#[test]
fn driver_id_and_capabilities() {
    let driver = PostgresDriver;
    assert_eq!(driver.id(), "postgresql");
    assert!(driver.capabilities().contains(Capabilities::TRANSACTIONS));
    assert!(driver.capabilities().contains(Capabilities::RETURNING));
}

#[test]
fn prefer_and_require_build_a_client_config_with_no_verification() {
    for mode in [SslMode::Prefer, SslMode::Require] {
        let ssl = SslConfig {
            mode,
            ca_file: None,
        };
        assert!(
            tls_config_for(&ssl).is_ok(),
            "{mode:?} should build fine with no CA file"
        );
    }
}

#[test]
fn verify_full_with_no_ca_file_falls_back_to_the_os_trust_store() {
    let ssl = SslConfig {
        mode: SslMode::VerifyFull,
        ca_file: None,
    };
    // The OS trust store is present on every builder/CI image this
    // repo targets; a genuinely empty store is the one case
    // `root_store_for` refuses.
    assert!(tls_config_for(&ssl).is_ok());
}

#[test]
fn verify_ca_with_a_missing_ca_file_is_reported_not_panicked() {
    let ssl = SslConfig {
        mode: SslMode::VerifyCa,
        ca_file: Some("/no/such/file.pem".to_string()),
    };
    let error = tls_config_for(&ssl).expect_err("a missing file must not silently succeed");
    assert_eq!(error.code, DbErrorCode::Io);
}

#[test]
fn verify_full_with_a_garbage_ca_file_is_reported_not_panicked() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("ca.pem");
    std::fs::write(&path, b"not a certificate").expect("writing the garbage file");
    let ssl = SslConfig {
        mode: SslMode::VerifyFull,
        ca_file: Some(path.to_string_lossy().into_owned()),
    };
    // `rustls_pemfile::certs` on a file with no PEM blocks yields no
    // certificates at all (not a parse error) — this must surface as
    // "no CA certificates available", not a silent empty-root config
    // that would accept nothing (`WebPkiServerVerifier::builder` on
    // an empty store, or `root_store_for`'s own explicit check).
    let error = tls_config_for(&ssl).expect_err("an empty root store must not silently succeed");
    assert_eq!(error.code, DbErrorCode::ConnectionFailed);
}

/// Only runs with `--features db-integration` against a real server
/// named by `IDE_DB_POSTGRES_URL` (`docs/architecture/db-integration.md`)
/// — skips itself (does not fail) when that env var is unset, so
/// `cargo test --features db-integration` still passes on a machine
/// with no Postgres container up.
#[cfg(feature = "db-integration")]
#[test]
fn connect_execute_and_introspect_against_a_real_server() {
    let Some(spec) = crate::testsupport::postgres_test_spec() else {
        eprintln!("IDE_DB_POSTGRES_URL not set — skipping");
        return;
    };
    let mut conn = PostgresDriver.connect(&spec).expect("connect");
    let Execution::Rows(mut stream) = conn
        .execute(&Statement::sql("SELECT 1"), &ExecOptions::default())
        .expect("execute")
    else {
        panic!("expected rows");
    };
    let batch = stream.next_batch().expect("batch").expect("some rows");
    assert_eq!(batch.rows, vec![vec![Value::Int(1)]]);

    let snapshot = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Names)
        .expect("introspect");
    assert_eq!(snapshot.level, IntrospectLevel::Names);

    let with_columns = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Columns)
        .expect("introspect at Columns level");
    assert_eq!(with_columns.level, IntrospectLevel::Columns);
}

/// `statm`'s resident-set field, in bytes — the same "read the kernel's
/// own accounting rather than estimate" approach a heap-growth NFR
/// check needs, gated behind `db-integration` since it needs a real,
/// million-row result to be meaningful.
#[cfg(feature = "db-integration")]
fn resident_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").expect("read /proc/self/statm");
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .expect("resident field")
        .parse()
        .expect("resident field is a number");
    pages * 4096
}

/// F3d's own NFR: a 1 000 000-row result's first page must be fast
/// (proves `execute` no longer blocks on draining the whole result
/// before returning) and later pages must not accumulate memory
/// (proves `next_batch` is paging a live stream, not re-chunking an
/// already-fully-materialised buffer — the defect `database-tools.md`
/// §4/§11 tracked against this driver).
#[cfg(feature = "db-integration")]
#[test]
fn a_million_row_generate_series_streams_a_fast_first_batch_with_bounded_memory() {
    let Some(spec) = crate::testsupport::postgres_test_spec() else {
        eprintln!("IDE_DB_POSTGRES_URL not set — skipping");
        return;
    };
    let mut conn = PostgresDriver.connect(&spec).expect("connect");
    let options = ExecOptions {
        fetch_size: 200,
        ..ExecOptions::default()
    };
    let Execution::Rows(mut stream) = conn
        .execute(
            &Statement::sql("SELECT * FROM generate_series(1, 1000000)"),
            &options,
        )
        .expect("execute")
    else {
        panic!("expected rows");
    };

    let started = std::time::Instant::now();
    let first = stream
        .next_batch()
        .expect("first batch")
        .expect("some rows");
    let elapsed = started.elapsed();
    assert_eq!(first.rows.len(), 200);
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "first batch took {elapsed:?}, expected < 100ms"
    );

    let baseline = resident_bytes();
    for _ in 0..3 {
        stream.next_batch().expect("batch").expect("some rows");
    }
    let growth = resident_bytes().saturating_sub(baseline);
    assert!(
        growth < 30 * 1024 * 1024,
        "RSS grew by {growth} bytes after three more pages, expected < 30MB"
    );
}

/// The parked-cursor rule `database-tools.md` §4 documents: a live
/// stream from one `execute` must not block or be disturbed by a
/// second `execute` on the same `PostgresConnection` — unlike SQLite,
/// Postgres needs no second connection for this, since tokio-postgres
/// pipelines concurrent commands on one `Client` transparently.
#[cfg(feature = "db-integration")]
#[test]
fn a_second_execute_does_not_disturb_an_open_stream() {
    let Some(spec) = crate::testsupport::postgres_test_spec() else {
        eprintln!("IDE_DB_POSTGRES_URL not set — skipping");
        return;
    };
    let mut conn = PostgresDriver.connect(&spec).expect("connect");
    let options = ExecOptions {
        fetch_size: 2,
        ..ExecOptions::default()
    };
    let Execution::Rows(mut stream) = conn
        .execute(
            &Statement::sql("SELECT * FROM generate_series(1, 5)"),
            &options,
        )
        .expect("execute")
    else {
        panic!("expected rows");
    };
    let first = stream.next_batch().expect("batch").expect("some rows");
    assert_eq!(first.rows, vec![vec![Value::Int(1)], vec![Value::Int(2)]]);

    conn.execute(&Statement::sql("SELECT 1"), &ExecOptions::default())
        .expect("second execute");

    let rest = stream.next_batch().expect("batch").expect("some rows");
    assert_eq!(rest.rows, vec![vec![Value::Int(3)], vec![Value::Int(4)]]);
    let last = stream.next_batch().expect("batch").expect("some rows");
    assert_eq!(last.rows, vec![vec![Value::Int(5)]]);
    assert!(stream.next_batch().expect("batch").is_none());
}
