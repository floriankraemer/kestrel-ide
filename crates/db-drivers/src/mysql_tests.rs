//! Unit tests for [`super::mysql`] — split out to keep
//! `mysql.rs` under the file-size ceiling (the coverage pass added
//! enough cases to cross it).
use super::*;

#[test]
fn driver_id_and_capabilities() {
    let driver = MySqlDriver;
    assert_eq!(driver.id(), "mysql");
    assert!(driver.capabilities().contains(Capabilities::TRANSACTIONS));
    assert!(driver
        .capabilities()
        .contains(Capabilities::SERVER_SIDE_CANCEL));
}

#[test]
fn build_opts_defaults_the_port_to_3306() {
    let spec = ConnectSpec {
        driver: "mysql".to_string(),
        host: "localhost".to_string(),
        port: None,
        database: "shop".to_string(),
        user: "root".to_string(),
        url: String::new(),
        password: None,
        ssl: SslConfig {
            mode: SslMode::Disable,
            ca_file: None,
        },
    };
    let opts = build_opts(&spec);
    assert_eq!(opts.tcp_port(), 3306);
    assert_eq!(opts.db_name(), Some("shop"));
}

#[test]
fn introspection_fixture_text_shapes_the_expected_columns() {
    assert!(NAMES_QUERY.contains("information_schema.tables"));
    assert!(NAMES_QUERY.contains("table_schema"));
    assert!(COLUMNS_QUERY.contains("information_schema.columns"));
    assert!(COLUMNS_QUERY.contains("column_key"));
    assert!(COLUMNS_QUERY.contains("extra"));
    assert!(INDEXES_QUERY.contains("information_schema.statistics"));
    assert!(INDEXES_QUERY.contains("GROUP_CONCAT"));
    assert!(CONSTRAINTS_QUERY.contains("information_schema.table_constraints"));
    assert!(CONSTRAINTS_QUERY.contains("referential_constraints"));
    assert!(CHECKS_QUERY.contains("check_constraints"));
}

#[test]
fn to_mysql_value_maps_every_scalar_variant() {
    assert!(matches!(
        to_mysql_value(&Value::Null),
        mysql_async::Value::NULL
    ));
    assert!(matches!(
        to_mysql_value(&Value::Int(42)),
        mysql_async::Value::Int(42)
    ));
    assert!(matches!(
        to_mysql_value(&Value::Bool(true)),
        mysql_async::Value::Int(1)
    ));
}

#[test]
fn to_mysql_value_maps_text_decimal_and_json_to_bytes() {
    for value in [
        Value::Decimal("3.14".to_string()),
        Value::Text("hi".to_string()),
        Value::Json("{}".to_string()),
    ] {
        match to_mysql_value(&value) {
            mysql_async::Value::Bytes(bytes) => {
                assert!(!bytes.is_empty());
            }
            other => panic!("expected Bytes, got {other:?}"),
        }
    }
}

#[test]
fn to_mysql_value_maps_bytes_uuid_date_time_and_datetime() {
    assert!(matches!(
        to_mysql_value(&Value::Bytes(vec![1, 2, 3])),
        mysql_async::Value::Bytes(_)
    ));
    assert!(matches!(
        to_mysql_value(&Value::Uuid(uuid::Uuid::nil())),
        mysql_async::Value::Bytes(_)
    ));
    let date = chrono::NaiveDate::from_ymd_opt(2024, 3, 5).unwrap();
    assert!(matches!(
        to_mysql_value(&Value::Date(date)),
        mysql_async::Value::Date(2024, 3, 5, 0, 0, 0, 0)
    ));
    let time = chrono::NaiveTime::from_hms_opt(1, 2, 3).unwrap();
    assert!(matches!(
        to_mysql_value(&Value::Time(time)),
        mysql_async::Value::Time(false, 0, 1, 2, 3, 0)
    ));
    let dt = date.and_time(time);
    assert!(matches!(
        to_mysql_value(&Value::DateTime(dt)),
        mysql_async::Value::Date(2024, 3, 5, 1, 2, 3, 0)
    ));
}

#[test]
fn to_mysql_value_falls_back_to_display_text_for_other_variants() {
    match to_mysql_value(&Value::Array(vec![Value::Int(1)])) {
        mysql_async::Value::Bytes(_) => {}
        other => panic!("expected Bytes, got {other:?}"),
    }
}

fn column_of(column_type: ColumnType, flags: mysql_async::consts::ColumnFlags) -> Column {
    Column::new(column_type)
        .with_flags(flags)
        .with_table(b"widgets")
        .with_name(b"col")
}

#[test]
fn bytes_to_value_maps_json_decimal_and_blob_types() {
    use mysql_async::consts::ColumnFlags;
    assert_eq!(
        bytes_to_value(
            b"{\"a\":1}".to_vec(),
            &column_of(ColumnType::MYSQL_TYPE_JSON, ColumnFlags::empty())
        ),
        Value::Json("{\"a\":1}".to_string())
    );
    assert_eq!(
        bytes_to_value(
            b"3.50".to_vec(),
            &column_of(ColumnType::MYSQL_TYPE_NEWDECIMAL, ColumnFlags::empty())
        ),
        Value::Decimal("3.50".to_string())
    );
    assert_eq!(
        bytes_to_value(
            b"3.50".to_vec(),
            &column_of(ColumnType::MYSQL_TYPE_DECIMAL, ColumnFlags::empty())
        ),
        Value::Decimal("3.50".to_string())
    );
    for column_type in [
        ColumnType::MYSQL_TYPE_TINY_BLOB,
        ColumnType::MYSQL_TYPE_MEDIUM_BLOB,
        ColumnType::MYSQL_TYPE_LONG_BLOB,
        ColumnType::MYSQL_TYPE_BLOB,
        ColumnType::MYSQL_TYPE_GEOMETRY,
    ] {
        assert_eq!(
            bytes_to_value(vec![1, 2, 3], &column_of(column_type, ColumnFlags::empty())),
            Value::Bytes(vec![1, 2, 3])
        );
    }
}

#[test]
fn bytes_to_value_treats_binary_flagged_strings_as_bytes_and_plain_ones_as_text() {
    use mysql_async::consts::ColumnFlags;
    assert_eq!(
        bytes_to_value(
            vec![1, 2],
            &column_of(ColumnType::MYSQL_TYPE_VARCHAR, ColumnFlags::BINARY_FLAG)
        ),
        Value::Bytes(vec![1, 2])
    );
    assert_eq!(
        bytes_to_value(
            b"hi".to_vec(),
            &column_of(ColumnType::MYSQL_TYPE_VARCHAR, ColumnFlags::empty())
        ),
        Value::Text("hi".to_string())
    );
    assert_eq!(
        bytes_to_value(
            b"hi".to_vec(),
            &column_of(ColumnType::MYSQL_TYPE_STRING, ColumnFlags::empty())
        ),
        Value::Text("hi".to_string())
    );
}

#[test]
fn mysql_value_to_value_maps_null_int_uint_and_floats() {
    use mysql_async::consts::ColumnFlags;
    let column = column_of(ColumnType::MYSQL_TYPE_LONG, ColumnFlags::empty());
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::NULL, &column),
        Value::Null
    );
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::Int(-7), &column),
        Value::Int(-7)
    );
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::UInt(42), &column),
        Value::Int(42)
    );
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::UInt(u64::MAX), &column),
        Value::Other {
            type_name: "bigint unsigned".to_string(),
            display: u64::MAX.to_string(),
        }
    );
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::Float(1.5), &column),
        Value::Float(1.5f32 as f64)
    );
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::Double(2.5), &column),
        Value::Float(2.5)
    );
}

#[test]
fn mysql_value_to_value_maps_date_only_when_the_column_type_is_a_date() {
    use mysql_async::consts::ColumnFlags;
    let date_column = column_of(ColumnType::MYSQL_TYPE_DATE, ColumnFlags::empty());
    assert_eq!(
        mysql_value_to_value(
            mysql_async::Value::Date(2024, 3, 5, 0, 0, 0, 0),
            &date_column
        ),
        Value::Date(chrono::NaiveDate::from_ymd_opt(2024, 3, 5).unwrap())
    );

    let datetime_column = column_of(ColumnType::MYSQL_TYPE_DATETIME, ColumnFlags::empty());
    assert_eq!(
        mysql_value_to_value(
            mysql_async::Value::Date(2024, 3, 5, 1, 2, 3, 4),
            &datetime_column
        ),
        Value::DateTime(
            chrono::NaiveDate::from_ymd_opt(2024, 3, 5)
                .unwrap()
                .and_hms_micro_opt(1, 2, 3, 4)
                .unwrap()
        )
    );

    // A zeroed time on a DATE-typed column still becomes a bare `Date`,
    // but the same all-zero time on a non-DATE column (e.g. TIMESTAMP)
    // is read back as midnight, not silently truncated.
    assert_eq!(
        mysql_value_to_value(
            mysql_async::Value::Date(2024, 3, 5, 0, 0, 0, 0),
            &datetime_column
        ),
        Value::DateTime(
            chrono::NaiveDate::from_ymd_opt(2024, 3, 5)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        )
    );
}

#[test]
fn mysql_value_to_value_reports_an_impossible_date_as_other() {
    use mysql_async::consts::ColumnFlags;
    let column = column_of(ColumnType::MYSQL_TYPE_DATE, ColumnFlags::empty());
    let value = mysql_value_to_value(mysql_async::Value::Date(2024, 2, 30, 0, 0, 0, 0), &column);
    assert_eq!(
        value,
        Value::Other {
            type_name: "date".to_string(),
            display: "2024-02-30".to_string(),
        }
    );
}

#[test]
fn mysql_value_to_value_maps_a_normal_time_and_reports_overflow_as_other() {
    use mysql_async::consts::ColumnFlags;
    let column = column_of(ColumnType::MYSQL_TYPE_TIME, ColumnFlags::empty());
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::Time(false, 0, 1, 2, 3, 4), &column),
        Value::Time(chrono::NaiveTime::from_hms_micro_opt(1, 2, 3, 4).unwrap())
    );
    // Negative (an interval before a reference point) is out of
    // `NaiveTime`'s range and falls back to `Other` with verbatim text.
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::Time(true, 0, 1, 2, 3, 4), &column),
        Value::Other {
            type_name: "time".to_string(),
            display: "-01:02:03.000004".to_string(),
        }
    );
    // 30 hours total (a day plus 6 hours) exceeds 24h and also falls back.
    assert_eq!(
        mysql_value_to_value(mysql_async::Value::Time(false, 1, 6, 0, 0, 0), &column),
        Value::Other {
            type_name: "time".to_string(),
            display: "30:00:00.000000".to_string(),
        }
    );
}

#[test]
fn ssl_opts_for_prefer_and_require_accept_any_certificate() {
    let ssl = db_core::datasource::SslConfig {
        mode: SslMode::Prefer,
        ca_file: None,
    };
    let _ = ssl_opts_for(&ssl); // must not panic

    let ssl = db_core::datasource::SslConfig {
        mode: SslMode::Require,
        ca_file: None,
    };
    let _ = ssl_opts_for(&ssl);
}

#[test]
fn ssl_opts_for_verify_modes_use_the_configured_ca_file_or_fall_back() {
    for mode in [SslMode::VerifyCa, SslMode::VerifyFull] {
        let ssl = db_core::datasource::SslConfig {
            mode,
            ca_file: Some("/etc/ssl/ca.pem".to_string()),
        };
        let _ = ssl_opts_for(&ssl);

        let ssl = db_core::datasource::SslConfig {
            mode,
            ca_file: None,
        };
        let _ = ssl_opts_for(&ssl);
    }
}

#[test]
fn build_opts_enables_ssl_opts_when_ssl_is_not_disabled() {
    let spec = ConnectSpec {
        driver: "mysql".to_string(),
        host: "db.internal".to_string(),
        port: Some(3307),
        database: String::new(),
        user: "root".to_string(),
        url: String::new(),
        password: Some("secret".to_string()),
        ssl: SslConfig {
            mode: SslMode::Require,
            ca_file: None,
        },
    };
    let opts = build_opts(&spec);
    assert_eq!(opts.tcp_port(), 3307);
    assert_eq!(opts.db_name(), None);
    assert!(opts.ssl_opts().is_some());
}

/// `MySqlRowStream` only ever sees already-mapped `Vec<Value>` rows
/// through its `stream` field, never a live `mysql_async::Row` (which
/// this crate never constructs by hand) — so its batching/`max_rows`/
/// error-propagation/release logic is exercised here against a fake
/// `futures_util::stream::iter`, no real server needed, the same
/// pattern `postgres.rs`'s own `fake_stream` test helper uses for
/// `PostgresRowStream`.
fn fake_row_stream(
    values: Vec<Result<Vec<Value>, DbError>>,
    fetch_size: usize,
    max_rows: Option<u64>,
) -> MySqlRowStream {
    MySqlRowStream {
        columns: vec![ColumnMeta {
            name: "n".to_string(),
            type_name: "int".to_string(),
            nullable: true,
            origin: None,
        }],
        fetch_size,
        max_rows,
        fetched: 0,
        stream: Some(Box::pin(futures_util::stream::iter(values))),
        query_result: None,
        conn: None,
        slot: Arc::new(Mutex::new(None)),
    }
}

#[test]
fn row_stream_pages_by_fetch_size_and_releases_on_exhaustion() {
    let values: Vec<Result<Vec<Value>, DbError>> =
        (0..5).map(|n| Ok(vec![Value::Int(n)])).collect();
    let mut stream = fake_row_stream(values, 2, None);

    let first = stream.next_batch().unwrap().unwrap();
    assert_eq!(first.rows, vec![vec![Value::Int(0)], vec![Value::Int(1)]]);
    let second = stream.next_batch().unwrap().unwrap();
    assert_eq!(second.rows, vec![vec![Value::Int(2)], vec![Value::Int(3)]]);
    let third = stream.next_batch().unwrap().unwrap();
    assert_eq!(third.rows, vec![vec![Value::Int(4)]]);
    assert!(stream.next_batch().unwrap().is_none());
    // Exhaustion already released the stream half.
    assert!(stream.stream.is_none());
}

#[test]
fn row_stream_max_rows_caps_the_total() {
    let values: Vec<Result<Vec<Value>, DbError>> =
        (0..10).map(|n| Ok(vec![Value::Int(n)])).collect();
    let mut stream = fake_row_stream(values, 3, Some(4));

    let mut seen = 0usize;
    while let Some(batch) = stream.next_batch().unwrap() {
        seen += batch.rows.len();
    }
    assert_eq!(seen, 4);
}

#[test]
fn row_stream_surfaces_a_mid_stream_error() {
    let values = vec![
        Ok(vec![Value::Int(1)]),
        Err(DbError::new(DbErrorCode::Unknown, "boom")),
    ];
    let mut stream = fake_row_stream(values, 1, None);
    let first = stream.next_batch().unwrap().unwrap();
    assert_eq!(first.rows, vec![vec![Value::Int(1)]]);
    let error = stream.next_batch().unwrap_err();
    assert_eq!(error.code, DbErrorCode::Unknown);
}

#[test]
fn row_stream_with_no_live_stream_yields_no_batches() {
    let mut stream = fake_row_stream(vec![], 2, None);
    assert!(stream.next_batch().unwrap().is_none());
    // A second call after the stream field is already `None` short-circuits.
    stream.stream = None;
    assert!(stream.next_batch().unwrap().is_none());
}

#[test]
fn show_create_statement_for_a_view_targets_show_create_view() {
    let object = ObjectRef::new("v_active_users").with_kind(ObjectKind::View);
    let (statement, column_index) = show_create_statement_for(&object);
    assert_eq!(statement, "SHOW CREATE VIEW `v_active_users`");
    assert_eq!(column_index, 1);
}

#[test]
fn show_create_statement_for_a_table_or_unspecified_kind_targets_show_create_table() {
    let object = ObjectRef::new("orders").with_kind(ObjectKind::Table);
    let (statement, _) = show_create_statement_for(&object);
    assert_eq!(statement, "SHOW CREATE TABLE `orders`");

    let object = ObjectRef::new("orders");
    let (statement, _) = show_create_statement_for(&object);
    assert_eq!(statement, "SHOW CREATE TABLE `orders`");
}

#[test]
fn columns_to_nodes_maps_key_default_and_auto_increment_detail() {
    let rows: Vec<ColumnRow> = vec![
        (
            "id".to_string(),
            "int".to_string(),
            "NO".to_string(),
            "PRI".to_string(),
            "auto_increment".to_string(),
            None,
            String::new(),
        ),
        (
            "name".to_string(),
            "varchar(255)".to_string(),
            "YES".to_string(),
            String::new(),
            String::new(),
            Some("''".to_string()),
            "a comment".to_string(),
        ),
    ];
    let nodes = columns_to_nodes(rows);
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].name, "id");
    let detail = &nodes[0].detail;
    assert_eq!(detail.type_name, Some("int".to_string()));
    assert_eq!(detail.nullable, Some(false));
    assert!(detail.primary_key);
    assert_eq!(detail.auto_increment, Some(true));
    assert_eq!(detail.comment, None);

    assert_eq!(nodes[1].name, "name");
    let detail = &nodes[1].detail;
    assert_eq!(detail.nullable, Some(true));
    assert!(!detail.primary_key);
    assert_eq!(detail.auto_increment, Some(false));
    assert_eq!(detail.default, Some("''".to_string()));
    assert_eq!(detail.comment, Some("a comment".to_string()));
}

#[test]
fn index_rows_to_nodes_splits_the_comma_joined_column_list() {
    let rows = vec![(
        "idx_name".to_string(),
        true,
        Some("BTREE".to_string()),
        "a,b".to_string(),
    )];
    let nodes = index_rows_to_nodes(rows);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "idx_name");
    let index = nodes[0].detail.index.as_ref().unwrap();
    assert!(index.unique);
    assert_eq!(index.method, Some("BTREE".to_string()));
    assert_eq!(index.columns, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn constraint_rows_to_nodes_maps_primary_key_unique_and_foreign_key() {
    let rows: Vec<ConstraintRow> = vec![
        (
            "PRIMARY".to_string(),
            "PRIMARY KEY".to_string(),
            "id".to_string(),
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "uq_sku".to_string(),
            "UNIQUE".to_string(),
            "sku".to_string(),
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "fk_order".to_string(),
            "FOREIGN KEY".to_string(),
            "order_id".to_string(),
            Some("shop".to_string()),
            Some("orders".to_string()),
            Some("id".to_string()),
            Some("CASCADE".to_string()),
            Some("NO ACTION".to_string()),
        ),
    ];
    let nodes = constraint_rows_to_nodes(rows);
    assert_eq!(nodes.len(), 3);
    assert!(matches!(
        nodes[0].detail.constraint,
        Some(ConstraintKind::PrimaryKey { .. })
    ));
    assert!(matches!(
        nodes[1].detail.constraint,
        Some(ConstraintKind::Unique { .. })
    ));
    match &nodes[2].detail.constraint {
        Some(ConstraintKind::ForeignKey {
            ref_table,
            ref_columns,
            on_update,
            on_delete,
            ..
        }) => {
            assert_eq!(ref_table.name, "orders");
            assert_eq!(ref_table.schema, Some("shop".to_string()));
            assert_eq!(ref_columns, &vec!["id".to_string()]);
            assert_eq!(on_update, &Some("CASCADE".to_string()));
            // "NO ACTION" is filtered out, matching Postgres's own rule.
            assert_eq!(on_delete, &None);
        }
        other => panic!("expected ForeignKey, got {other:?}"),
    }
}

#[test]
fn constraint_rows_to_nodes_skips_an_unrecognised_constraint_type() {
    let rows: Vec<ConstraintRow> = vec![(
        "chk_positive".to_string(),
        "CHECK".to_string(),
        "qty".to_string(),
        None,
        None,
        None,
        None,
        None,
    )];
    assert!(constraint_rows_to_nodes(rows).is_empty());
}

#[test]
fn check_rows_to_nodes_maps_the_check_expression() {
    let nodes = check_rows_to_nodes(vec![("chk_qty".to_string(), "qty > 0".to_string())]);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "chk_qty");
    assert!(matches!(
        nodes[0].detail.constraint,
        Some(ConstraintKind::Check { .. })
    ));
}

#[test]
fn column_meta_reports_name_type_nullability_and_origin() {
    use mysql_async::consts::ColumnFlags;
    let column = Column::new(ColumnType::MYSQL_TYPE_VARCHAR)
        .with_flags(ColumnFlags::NOT_NULL_FLAG)
        .with_table(b"widgets")
        .with_name(b"sku");
    let meta = column_meta(&column);
    assert_eq!(meta.name, "sku");
    assert!(!meta.nullable);
    assert_eq!(meta.origin, Some("widgets".to_string()));

    let nullable_no_table = Column::new(ColumnType::MYSQL_TYPE_VARCHAR).with_name(b"x");
    let meta = column_meta(&nullable_no_table);
    assert!(meta.nullable);
    assert_eq!(meta.origin, None);
}

/// Both real-server tests below run this same body once per engine
/// (`IDE_DB_MYSQL_URL` and `IDE_DB_MARIADB_URL`) rather than one test
/// function apiece — the plan asks both engines be exercised, and a
/// shared body is the one place that stays true if either changes.
#[cfg(feature = "db-integration")]
fn for_each_configured_engine(run: impl Fn(&str, db_core::datasource::ConnectSpec)) {
    let mut any = false;
    for env_var in ["IDE_DB_MYSQL_URL", "IDE_DB_MARIADB_URL"] {
        match crate::testsupport::mysql_test_spec(env_var) {
            Some(spec) => {
                any = true;
                run(env_var, spec);
            }
            None => eprintln!("{env_var} not set — skipping"),
        }
    }
    if !any {
        eprintln!("neither IDE_DB_MYSQL_URL nor IDE_DB_MARIADB_URL is set — skipping entirely");
    }
}

#[cfg(feature = "db-integration")]
#[test]
fn connect_execute_and_introspect_against_a_real_server() {
    for_each_configured_engine(|env_var, spec| {
        let mut conn = MySqlDriver.connect(&spec).unwrap_or_else(|e| {
            panic!("{env_var}: connect failed: {e:?}");
        });
        let Execution::Rows(mut stream) = conn
            .execute(&Statement::sql("SELECT 1"), &ExecOptions::default())
            .expect("execute")
        else {
            panic!("{env_var}: expected rows");
        };
        let batch = stream.next_batch().expect("batch").expect("some rows");
        assert_eq!(batch.rows, vec![vec![Value::Int(1)]]);

        let snapshot = conn
            .introspect(&IntrospectScope::default(), IntrospectLevel::Names)
            .expect("introspect");
        assert_eq!(snapshot.level, IntrospectLevel::Names);
    });
}

#[cfg(feature = "db-integration")]
#[test]
fn a_million_row_recursive_cte_streams_a_fast_first_batch() {
    for_each_configured_engine(|env_var, spec| {
        let mut conn = MySqlDriver.connect(&spec).expect("connect");
        // MySQL's own default `cte_max_recursion_depth` is 1000 —
        // MariaDB has no such variable at all (unlimited by default),
        // so this `SET` is allowed to fail there; either way the
        // actual query below is what this test cares about.
        let _ = conn.execute(
            &Statement::sql("SET SESSION cte_max_recursion_depth = 1000000"),
            &ExecOptions::default(),
        );
        let options = ExecOptions {
            fetch_size: 200,
            ..ExecOptions::default()
        };
        let Execution::Rows(mut stream) = conn
            .execute(
                &Statement::sql(
                    "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 1000000) \
                     SELECT n FROM seq",
                ),
                &options,
            )
            .expect("execute")
        else {
            panic!("{env_var}: expected rows");
        };
        let started = std::time::Instant::now();
        let first = stream
            .next_batch()
            .expect("first batch")
            .expect("some rows");
        assert_eq!(first.rows.len(), 200);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(300),
            "{env_var}: first batch took {:?}",
            started.elapsed()
        );
    });
}

#[cfg(feature = "db-integration")]
#[test]
fn cancel_stops_a_sleeping_query_within_a_second_and_a_half() {
    for_each_configured_engine(|env_var, spec| {
        let mut conn = MySqlDriver.connect(&spec).expect("connect");
        let cancel = conn
            .cancel_handle()
            .expect("mysql supports server-side cancel");
        let started = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            conn.execute(&Statement::sql("SELECT SLEEP(30)"), &ExecOptions::default())
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        cancel.cancel().expect("cancel");
        let _ = handle.join().expect("thread joins");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1500),
            "{env_var}: cancel took {:?}",
            started.elapsed()
        );
    });
}
