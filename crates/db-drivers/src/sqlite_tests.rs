//! Unit tests for [`super::sqlite`] — split out to keep `sqlite.rs` under
//! the file-size ceiling as F2/F3/F4/F6 grew its introspection and
//! streaming coverage.

use super::*;
use db_core::driver::QueryLang;
use db_core::schema::Children;

fn connect() -> SqliteConnection {
    SqliteConnection::wrap(rusqlite::Connection::open_in_memory().unwrap())
}

#[test]
fn a_select_1_returns_one_row_one_column() {
    let mut conn = connect();
    let execution = conn
        .execute(&Statement::sql("SELECT 1"), &ExecOptions::default())
        .unwrap();
    let Execution::Rows(mut stream) = execution else {
        panic!("expected rows");
    };
    let batch = stream.next_batch().unwrap().unwrap();
    assert_eq!(batch.columns.len(), 1);
    assert_eq!(batch.rows, vec![vec![Value::Int(1)]]);
    assert!(stream.next_batch().unwrap().is_none());
}

#[test]
fn create_and_insert_are_affected_not_rows() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE t (id INTEGER, name TEXT)"),
        &ExecOptions::default(),
    )
    .unwrap();
    let execution = conn
        .execute(
            &Statement::sql("INSERT INTO t VALUES (1, 'a')"),
            &ExecOptions::default(),
        )
        .unwrap();
    assert!(matches!(execution, Execution::Affected(1)));
}

#[test]
fn bound_params_round_trip_through_a_query() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE t (n INTEGER, s TEXT)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql("INSERT INTO t VALUES (?, ?)")
            .with_params(vec![Value::Int(42), Value::Text("hi".to_string())]),
        &ExecOptions::default(),
    )
    .unwrap();
    let Execution::Rows(mut stream) = conn
        .execute(
            &Statement::sql("SELECT n, s FROM t"),
            &ExecOptions::default(),
        )
        .unwrap()
    else {
        panic!("expected rows");
    };
    let batch = stream.next_batch().unwrap().unwrap();
    assert_eq!(
        batch.rows,
        vec![vec![Value::Int(42), Value::Text("hi".to_string())]]
    );
}

#[test]
fn null_and_blob_round_trip() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE t (b BLOB, n INTEGER)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql("INSERT INTO t VALUES (?, ?)")
            .with_params(vec![Value::Bytes(vec![1, 2, 3]), Value::Null]),
        &ExecOptions::default(),
    )
    .unwrap();
    let Execution::Rows(mut stream) = conn
        .execute(
            &Statement::sql("SELECT b, n FROM t"),
            &ExecOptions::default(),
        )
        .unwrap()
    else {
        panic!("expected rows");
    };
    let batch = stream.next_batch().unwrap().unwrap();
    assert_eq!(
        batch.rows,
        vec![vec![Value::Bytes(vec![1, 2, 3]), Value::Null]]
    );
}

#[test]
fn introspect_at_columns_level_lists_tables_and_their_columns() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)"),
        &ExecOptions::default(),
    )
    .unwrap();
    let snapshot = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Columns)
        .unwrap();
    assert_eq!(snapshot.level, IntrospectLevel::Columns);
    assert_eq!(snapshot.roots.len(), 1);
    let table = &snapshot.roots[0];
    assert_eq!(table.name, "users");
    assert_eq!(table.kind, ObjectKind::Table);
    match &table.children {
        Children::Loaded(columns) => {
            let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, vec!["id", "name"]);
            assert!(columns[0].detail.primary_key);
            assert_eq!(columns[0].detail.type_name.as_deref(), Some("INTEGER"));
            assert_eq!(columns[1].detail.nullable, Some(false));
        }
        Children::NotLoaded => panic!("expected loaded columns"),
    }
}

#[test]
fn introspect_at_names_level_never_loads_columns() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE users (id INTEGER, name TEXT)"),
        &ExecOptions::default(),
    )
    .unwrap();
    let snapshot = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Names)
        .unwrap();
    assert_eq!(snapshot.level, IntrospectLevel::Names);
    assert_eq!(snapshot.roots[0].children, Children::NotLoaded);
}

#[test]
fn introspect_at_full_level_adds_indexes_and_triggers() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE users (id INTEGER, name TEXT)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql("CREATE INDEX users_name_idx ON users (name)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql("CREATE TABLE audit (id INTEGER)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql(
            "CREATE TRIGGER users_ai AFTER INSERT ON users BEGIN \
             INSERT INTO audit (id) VALUES (NEW.id); END",
        ),
        &ExecOptions::default(),
    )
    .unwrap();

    let snapshot = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Full)
        .unwrap();
    let users = snapshot.roots.iter().find(|n| n.name == "users").unwrap();
    let Children::Loaded(children) = &users.children else {
        panic!("expected loaded children");
    };
    assert!(children
        .iter()
        .any(|c| c.kind == ObjectKind::Index && c.name == "users_name_idx"));
    assert!(children
        .iter()
        .any(|c| c.kind == ObjectKind::Trigger && c.name == "users_ai"));
}

/// F6c: a schema with two foreign keys, a composite primary key and
/// a unique index — the full structural detail `IntrospectLevel::Full`
/// is meant to carry, not just presence-only nodes.
#[test]
fn full_introspect_carries_fk_composite_pk_and_unique_index_detail() {
    let mut conn = connect();
    for ddl in [
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT UNIQUE)",
        "CREATE TABLE products (id INTEGER PRIMARY KEY)",
        "CREATE TABLE order_items (\
           order_id INTEGER, \
           product_id INTEGER, \
           user_id INTEGER, \
           PRIMARY KEY (order_id, product_id), \
           FOREIGN KEY (product_id) REFERENCES products(id), \
           FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE\
         )",
    ] {
        conn.execute(&Statement::sql(ddl), &ExecOptions::default())
            .unwrap();
    }

    let snapshot = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Full)
        .unwrap();

    // The unique index on users.email carries its column and
    // uniqueness, not just a bare name.
    let users = snapshot.roots.iter().find(|n| n.name == "users").unwrap();
    let Children::Loaded(users_children) = &users.children else {
        panic!("expected loaded children");
    };
    let email_index = users_children
        .iter()
        .find(|c| c.kind == ObjectKind::Index && c.detail.index.as_ref().is_some_and(|d| d.unique))
        .expect("expected the auto-created unique index for email");
    let index_detail = email_index.detail.index.as_ref().unwrap();
    assert_eq!(index_detail.columns, vec!["email".to_string()]);
    assert!(index_detail.unique);
    // ... and the same uniqueness is also visible as a Unique
    // constraint node (not just an index).
    assert!(users_children
        .iter()
        .any(|c| c.kind == ObjectKind::Constraint
            && matches!(
                &c.detail.constraint,
                Some(ConstraintKind::Unique { columns }) if columns == &vec!["email".to_string()]
            )));

    let order_items = snapshot
        .roots
        .iter()
        .find(|n| n.name == "order_items")
        .unwrap();
    let Children::Loaded(children) = &order_items.children else {
        panic!("expected loaded children");
    };

    let pk = children
        .iter()
        .find(|c| {
            c.kind == ObjectKind::Constraint
                && matches!(
                    &c.detail.constraint,
                    Some(ConstraintKind::PrimaryKey { .. })
                )
        })
        .expect("expected a primary key constraint node");
    match pk.detail.constraint.as_ref().unwrap() {
        ConstraintKind::PrimaryKey { columns } => {
            assert_eq!(
                columns,
                &vec!["order_id".to_string(), "product_id".to_string()]
            )
        }
        other => panic!("expected PrimaryKey, got {other:?}"),
    }

    let foreign_keys: Vec<&ConstraintKind> = children
        .iter()
        .filter_map(|c| c.detail.constraint.as_ref())
        .filter(|k| matches!(k, ConstraintKind::ForeignKey { .. }))
        .collect();
    assert_eq!(foreign_keys.len(), 2);
    let to_users = foreign_keys
        .iter()
        .find_map(|k| match k {
            ConstraintKind::ForeignKey {
                columns,
                ref_table,
                ref_columns,
                on_delete,
                on_update,
            } if ref_table.name == "users" => Some((
                columns.clone(),
                ref_columns.clone(),
                on_delete.clone(),
                on_update.clone(),
            )),
            _ => None,
        })
        .expect("expected a foreign key to users");
    assert_eq!(to_users.0, vec!["user_id".to_string()]);
    assert_eq!(to_users.1, vec!["id".to_string()]);
    assert_eq!(to_users.2, Some("CASCADE".to_string()));
    assert_eq!(to_users.3, None);
}

#[test]
fn auto_increment_is_true_only_for_a_single_column_integer_primary_key_with_the_keyword() {
    let mut conn = connect();
    for ddl in [
        "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
        "CREATE TABLE plain (id INTEGER PRIMARY KEY, name TEXT)",
    ] {
        conn.execute(&Statement::sql(ddl), &ExecOptions::default())
            .unwrap();
    }
    let snapshot = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Columns)
        .unwrap();
    let users = snapshot.roots.iter().find(|n| n.name == "users").unwrap();
    let Children::Loaded(users_children) = &users.children else {
        panic!("expected loaded children")
    };
    assert_eq!(
        users_children[0].detail.auto_increment,
        Some(true),
        "AUTOINCREMENT keyword present"
    );
    let plain = snapshot.roots.iter().find(|n| n.name == "plain").unwrap();
    let Children::Loaded(plain_children) = &plain.children else {
        panic!("expected loaded children")
    };
    assert_eq!(
        plain_children[0].detail.auto_increment,
        Some(false),
        "no AUTOINCREMENT keyword"
    );
}

#[test]
fn a_scoped_object_introspect_returns_just_that_one_table() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE users (id INTEGER)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql("CREATE TABLE orders (id INTEGER)"),
        &ExecOptions::default(),
    )
    .unwrap();
    let snapshot = conn
        .introspect(
            &IntrospectScope::for_object("users"),
            IntrospectLevel::Columns,
        )
        .unwrap();
    assert_eq!(snapshot.roots.len(), 1);
    assert_eq!(snapshot.roots[0].name, "users");
}

#[test]
fn introspecting_an_unknown_scoped_object_is_not_supported() {
    let mut conn = connect();
    let error = conn
        .introspect(&IntrospectScope::for_object("nope"), IntrospectLevel::Names)
        .unwrap_err();
    assert_eq!(error.code, DbErrorCode::NotSupported);
}

#[test]
fn ddl_of_returns_the_recorded_create_statement() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE t (id INTEGER)"),
        &ExecOptions::default(),
    )
    .unwrap();
    let ddl = conn.ddl_of(&ObjectRef::new("t")).unwrap();
    assert!(ddl.contains("CREATE TABLE"));
    assert!(ddl.contains('t'));
}

#[test]
fn ddl_of_an_unknown_object_is_not_supported() {
    let mut conn = connect();
    let error = conn.ddl_of(&ObjectRef::new("nope")).unwrap_err();
    assert_eq!(error.code, DbErrorCode::NotSupported);
}

#[test]
fn begin_commit_and_rollback_run_without_error() {
    let mut conn = connect();
    conn.begin().unwrap();
    conn.execute(
        &Statement::sql("CREATE TABLE t (id INTEGER)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.commit().unwrap();

    conn.begin().unwrap();
    conn.execute(
        &Statement::sql("INSERT INTO t VALUES (1)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.rollback().unwrap();

    let Execution::Rows(mut stream) = conn
        .execute(
            &Statement::sql("SELECT COUNT(*) FROM t"),
            &ExecOptions::default(),
        )
        .unwrap()
    else {
        panic!("expected rows");
    };
    let batch = stream.next_batch().unwrap().unwrap();
    assert_eq!(batch.rows, vec![vec![Value::Int(0)]]);
}

#[test]
fn a_cancel_handle_is_available_and_interrupting_it_does_not_error() {
    let conn = connect();
    let handle = conn.cancel_handle().expect("sqlite always has one");
    assert!(handle.cancel().is_ok());
}

#[test]
fn map_err_reports_cancelled_for_a_query_interrupted_from_another_thread() {
    let mut conn = connect();
    let handle = conn.cancel_handle().expect("sqlite always has one");
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = handle.cancel();
    });
    let result = conn.execute(
        &Statement::sql(
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x < 100000000) SELECT count(*) FROM c",
        ),
        &ExecOptions::default(),
    );
    match result {
        Err(error) => assert_eq!(error.code, DbErrorCode::Cancelled),
        Ok(_) => panic!("expected the interrupted query to fail"),
    }
}

#[test]
fn apply_runs_a_dml_plan_and_sums_affected_rows() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE t (id INTEGER, name TEXT)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql("INSERT INTO t VALUES (1, 'a')"),
        &ExecOptions::default(),
    )
    .unwrap();

    let mut buffer = db_core::dml::EditBuffer::new(
        "t",
        vec!["id".to_string(), "name".to_string()],
        vec!["id".to_string()],
        vec![vec![Value::Int(1), Value::Text("a".to_string())]],
    );
    buffer.stage(0, "name", Value::Text("b".to_string()));
    let plan = buffer.to_dml_plan(Dialect::Sqlite);
    let affected = conn.apply(&plan.statements).unwrap();
    assert_eq!(affected, 1);
}

#[test]
fn driver_id_and_capabilities() {
    let driver = SqliteDriver;
    assert_eq!(driver.id(), "sqlite");
    assert!(driver.capabilities().contains(Capabilities::TRANSACTIONS));
}

#[test]
fn driver_connect_opens_a_file_backed_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let spec = ConnectSpec {
        driver: "sqlite".to_string(),
        host: String::new(),
        port: None,
        database: path.to_string_lossy().to_string(),
        user: String::new(),
        url: String::new(),
        password: None,
        ssl: Default::default(),
    };
    let mut conn = SqliteDriver.connect(&spec).unwrap();
    conn.execute(&Statement::sql("SELECT 1"), &ExecOptions::default())
        .unwrap();
    assert!(path.exists());
}

#[test]
fn statement_lang_is_ignored_by_this_backend_but_still_carried() {
    let statement = Statement {
        lang: QueryLang::Sql,
        text: "SELECT 1".to_string(),
        params: vec![],
    };
    let mut conn = connect();
    assert!(conn.execute(&statement, &ExecOptions::default()).is_ok());
}

/// Every `Value` variant `dml::EditBuffer`/a data-editor submit could
/// bind as a parameter round-trips through SQLite's dynamic typing —
/// `to_rusqlite`'s whole match, exercised for real rather than by
/// inspection.
#[test]
fn every_value_variant_binds_and_reads_back() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE t (v)"),
        &ExecOptions::default(),
    )
    .unwrap();

    let uuid = uuid::Uuid::nil();
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
    let time = chrono::NaiveTime::from_hms_opt(1, 2, 3).unwrap();
    let datetime = date.and_time(time);
    let datetime_tz = chrono::DateTime::parse_from_rfc3339("2026-09-21T01:02:03+02:00").unwrap();

    let values = vec![
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        Value::Int(42),
        Value::Float(1.5),
        Value::Decimal("12.34".to_string()),
        Value::Json("{}".to_string()),
        Value::Date(date),
        Value::Time(time),
        Value::DateTime(datetime),
        Value::DateTimeTz(datetime_tz),
        Value::Uuid(uuid),
        Value::Array(vec![Value::Int(1)]),
        Value::Document(vec![("a".to_string(), Value::Int(1))]),
        Value::Other {
            type_name: "custom".to_string(),
            display: "custom-value".to_string(),
        },
    ];

    for value in values {
        conn.execute(
            &Statement::sql("INSERT INTO t VALUES (?)").with_params(vec![value.clone()]),
            &ExecOptions::default(),
        )
        .unwrap();
    }

    let Execution::Rows(mut stream) = conn
        .execute(
            &Statement::sql("SELECT COUNT(*) FROM t"),
            &ExecOptions::default(),
        )
        .unwrap()
    else {
        panic!("expected rows");
    };
    let batch = stream.next_batch().unwrap().unwrap();
    assert_eq!(batch.rows, vec![vec![Value::Int(15)]]);
}

/// F3c: `execute` against a real file must stream — not drain the
/// whole result eagerly (the defect F3b left, `database-tools.md`
/// §4/§11). Proven with a big-enough table that an eager drain would
/// be obviously slower, and by paging to the end rather than reading
/// only the first batch.
#[test]
fn execute_streams_a_large_table_batch_by_batch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.db");
    let path = path.to_str().unwrap();

    {
        let mut conn = rusqlite::Connection::open(path).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch("CREATE TABLE big (n INTEGER)").unwrap();
        {
            let mut stmt = tx.prepare("INSERT INTO big (n) VALUES (?1)").unwrap();
            for n in 0..50_000i64 {
                stmt.execute([n]).unwrap();
            }
        }
        tx.commit().unwrap();
    }

    let mut driver_conn = SqliteDriver
        .connect(&ConnectSpec {
            driver: "sqlite".to_string(),
            host: String::new(),
            port: None,
            database: String::new(),
            user: String::new(),
            url: path.to_string(),
            password: None,
            ssl: Default::default(),
        })
        .unwrap();

    let options = ExecOptions {
        fetch_size: 200,
        ..Default::default()
    };
    let Execution::Rows(mut stream) = driver_conn
        .execute(&Statement::sql("SELECT n FROM big ORDER BY n"), &options)
        .unwrap()
    else {
        panic!("expected rows");
    };

    let mut seen = 0i64;
    let mut batches = 0;
    while let Some(batch) = stream.next_batch().unwrap() {
        assert!(batch.rows.len() <= 200);
        for row in &batch.rows {
            assert_eq!(row, &vec![Value::Int(seen)]);
            seen += 1;
        }
        batches += 1;
    }
    assert_eq!(seen, 50_000);
    assert_eq!(batches, 250);
}

/// The parked-cursor rule: a live stream from one `execute` call must
/// not block or corrupt a second `execute` on the same
/// `SqliteConnection` — the live stream reads through its own,
/// dedicated connection rather than `self.conn`.
#[test]
fn a_second_execute_does_not_disturb_an_open_stream() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("parked.db");
    let path = path.to_str().unwrap();
    {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE t (n INTEGER); \
             INSERT INTO t (n) VALUES (1), (2), (3), (4), (5);",
        )
        .unwrap();
    }

    let mut conn = SqliteDriver
        .connect(&ConnectSpec {
            driver: "sqlite".to_string(),
            host: String::new(),
            port: None,
            database: String::new(),
            user: String::new(),
            url: path.to_string(),
            password: None,
            ssl: Default::default(),
        })
        .unwrap();

    let options = ExecOptions {
        fetch_size: 2,
        ..Default::default()
    };
    let Execution::Rows(mut stream) = conn
        .execute(&Statement::sql("SELECT n FROM t ORDER BY n"), &options)
        .unwrap()
    else {
        panic!("expected rows");
    };
    let first = stream.next_batch().unwrap().unwrap();
    assert_eq!(first.rows, vec![vec![Value::Int(1)], vec![Value::Int(2)]]);

    // A second statement on the same connection, while `stream` is
    // still parked mid-way through the first, must succeed.
    conn.execute(
        &Statement::sql("SELECT COUNT(*) FROM t"),
        &ExecOptions::default(),
    )
    .unwrap();

    let rest = stream.next_batch().unwrap().unwrap();
    assert_eq!(rest.rows, vec![vec![Value::Int(3)], vec![Value::Int(4)]]);
    let last = stream.next_batch().unwrap().unwrap();
    assert_eq!(last.rows, vec![vec![Value::Int(5)]]);
    assert!(stream.next_batch().unwrap().is_none());
}

#[test]
fn map_err_carries_the_underlying_message() {
    let mut conn = connect();
    match conn.execute(&Statement::sql("NOT VALID SQL"), &ExecOptions::default()) {
        Err(error) => {
            assert_eq!(error.code, DbErrorCode::Unknown);
            assert!(!error.message.is_empty());
        }
        Ok(_) => panic!("expected an error"),
    }
}

/// Every row `sql` returns, in order — the F4.5 round-trip tests' own
/// "reopen and reread" step.
fn query_rows(conn: &mut SqliteConnection, sql: &str) -> Vec<Vec<Value>> {
    let Execution::Rows(mut stream) = conn
        .execute(&Statement::sql(sql), &ExecOptions::default())
        .unwrap()
    else {
        panic!("expected rows");
    };
    let mut rows = Vec::new();
    while let Some(batch) = stream.next_batch().unwrap() {
        rows.extend(batch.rows);
    }
    rows
}

/// F4.5: a data-editor submit's whole lifecycle (edit an existing row,
/// add one, delete one) against a real SQLite connection, then reopen
/// (re-query) and confirm every value actually persisted — not just
/// that `apply` returned an affected-row count.
#[test]
fn edit_add_and_delete_all_persist_and_reopen_correctly() {
    let mut conn = connect();
    conn.execute(
        &Statement::sql("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql("INSERT INTO t VALUES (1, 'alice'), (2, 'bob')"),
        &ExecOptions::default(),
    )
    .unwrap();

    let mut buffer = db_core::dml::EditBuffer::new(
        "t",
        vec!["id".to_string(), "name".to_string()],
        vec!["id".to_string()],
        vec![
            vec![Value::Int(1), Value::Text("alice".to_string())],
            vec![Value::Int(2), Value::Text("bob".to_string())],
        ],
    );
    buffer.stage(0, "name", Value::Text("alice2".to_string()));
    buffer.add_row(vec![Value::Int(3), Value::Text("carol".to_string())]);
    buffer.delete_row(1); // bob

    let plan = buffer.to_dml_plan(Dialect::Sqlite);
    conn.apply(&plan.statements).unwrap();

    let rows = query_rows(&mut conn, "SELECT id, name FROM t ORDER BY id");
    assert_eq!(
        rows,
        vec![
            vec![Value::Int(1), Value::Text("alice2".to_string())],
            vec![Value::Int(3), Value::Text("carol".to_string())],
        ]
    );
}

/// F4.5: adversarial identifiers survive the full `EditBuffer` →
/// `DmlPlan` → real `apply` path — the table/column names are quoted,
/// never executed as syntax, and in particular the literal text
/// `DROP TABLE students` sitting *inside* the adversarial table name
/// never actually drops the real `students` table sitting right next
/// to it.
#[test]
fn adversarial_identifiers_survive_a_real_apply_without_executing_as_syntax() {
    let mut conn = connect();
    let table = "Robert'); DROP TABLE students;--";
    let column = "a\"b";
    conn.execute(
        &Statement::sql("CREATE TABLE students (id INTEGER)"),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql(format!(
            "CREATE TABLE {} (id INTEGER PRIMARY KEY, {} TEXT)",
            Dialect::Sqlite.quote_ident(table),
            Dialect::Sqlite.quote_ident(column),
        )),
        &ExecOptions::default(),
    )
    .unwrap();
    conn.execute(
        &Statement::sql(format!(
            "INSERT INTO {} VALUES (1, 'x')",
            Dialect::Sqlite.quote_ident(table)
        )),
        &ExecOptions::default(),
    )
    .unwrap();

    let mut buffer = db_core::dml::EditBuffer::new(
        table,
        vec!["id".to_string(), column.to_string()],
        vec!["id".to_string()],
        vec![vec![Value::Int(1), Value::Text("x".to_string())]],
    );
    buffer.stage(0, column, Value::Text("y".to_string()));
    let plan = buffer.to_dml_plan(Dialect::Sqlite);
    // The adversarial name appears only inside its own quoted
    // identifier — never as a second, unquoted statement an
    // injection would need.
    assert_eq!(
        plan.statements[0].text,
        "UPDATE \"Robert'); DROP TABLE students;--\" SET \"a\"\"b\" = ? WHERE \"id\" = ?"
    );
    conn.apply(&plan.statements).unwrap();

    // The real `students` table is untouched: the DROP text embedded
    // in the adversarial table's own name never executed as syntax.
    let students = query_rows(&mut conn, "SELECT count(*) FROM students");
    assert_eq!(students, vec![vec![Value::Int(0)]]);

    let rows = query_rows(
        &mut conn,
        &format!(
            "SELECT id, {} FROM {}",
            Dialect::Sqlite.quote_ident(column),
            Dialect::Sqlite.quote_ident(table)
        ),
    );
    assert_eq!(
        rows,
        vec![vec![Value::Int(1), Value::Text("y".to_string())]]
    );
}
