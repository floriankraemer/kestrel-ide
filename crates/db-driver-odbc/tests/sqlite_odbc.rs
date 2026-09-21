//! Exercises `OdbcDriver` against the SQLite ODBC driver the
//! `linux-builder` image installs (`libsqliteodbc`, F8.6) — a real ODBC
//! round trip with no external DSN and no network, so it runs both as a
//! plain smoke test in `make test`'s default `cargo nextest run
//! --workspace` (skipping itself with a clear message if the driver is
//! missing, e.g. on a bare host) and, more strictly, under the
//! `db-integration` feature where its absence is a hard failure.
//!
//! `Driver=SQLite3;Database=<tmpfile>` needs no DSN registration — the
//! driver name alone is enough once `odbcinst.ini` lists it (F8.6).

use db_core::datasource::{ConnectSpec, SslConfig};
use db_core::driver::{Driver, ExecOptions, Execution, Statement};
use db_core::schema::IntrospectScope;

fn sqlite_odbc_driver_available() -> bool {
    std::fs::read_to_string("/etc/odbcinst.ini")
        .map(|contents| contents.contains("[SQLite3]"))
        .unwrap_or(false)
}

fn spec(database: &str) -> ConnectSpec {
    ConnectSpec {
        driver: "SQLite3".to_string(),
        host: String::new(),
        port: None,
        database: database.to_string(),
        user: String::new(),
        url: String::new(),
        password: None,
        ssl: SslConfig::default(),
    }
}

#[test]
fn smoke_select_and_ddl_round_trip_through_the_real_sqlite_odbc_driver() {
    if !sqlite_odbc_driver_available() {
        eprintln!(
            "skipping: no [SQLite3] entry in /etc/odbcinst.ini \
             (only present inside the linux-builder image, F8.6)"
        );
        return;
    }

    let db_file = tempfile::NamedTempFile::new().expect("tempfile");
    let path = db_file.path().to_str().expect("utf8 path").to_string();
    // `Preallocated`'s row_count() needs the file to already exist for
    // some SQLite ODBC driver builds; NamedTempFile already created it.

    let driver = db_driver_odbc::OdbcDriver::new();
    let mut conn = driver.connect(&spec(&path)).expect("connect");

    let create = Statement::sql("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)");
    match conn
        .execute(&create, &ExecOptions::default())
        .expect("create table")
    {
        Execution::Ok | Execution::Affected(_) => {}
        other => panic!(
            "unexpected execution for DDL: {other:?}",
            other = std::mem::discriminant(&other)
        ),
    }

    let insert = Statement::sql("INSERT INTO t (id, name) VALUES (1, 'alice')");
    let affected = match conn
        .execute(&insert, &ExecOptions::default())
        .expect("insert")
    {
        Execution::Affected(n) => n,
        Execution::Ok => 0, // SQLite's ODBC driver may not report a count for INSERT
        other => panic!(
            "unexpected execution for INSERT: kind={:?}",
            std::mem::discriminant(&other)
        ),
    };
    assert!(affected <= 1);

    let select = Statement::sql("SELECT id, name FROM t ORDER BY id");
    let execution = conn
        .execute(&select, &ExecOptions::default())
        .expect("select");
    let mut stream = match execution {
        Execution::Rows(stream) => stream,
        other => panic!(
            "expected Rows, got kind={:?}",
            std::mem::discriminant(&other)
        ),
    };
    let batch = stream
        .next_batch()
        .expect("first batch")
        .expect("at least one row");
    assert_eq!(batch.columns.len(), 2);
    assert_eq!(batch.rows.len(), 1);
    assert_eq!(batch.rows[0][0], db_core::value::Value::Int(1));
    match &batch.rows[0][1] {
        db_core::value::Value::Text(name) => assert_eq!(name, "alice"),
        other => panic!("expected Value::Text, got {other:?}"),
    }
    assert!(stream.next_batch().expect("end of stream").is_none());

    let snapshot = conn
        .introspect(&IntrospectScope::default())
        .expect("introspect");
    let table_names: Vec<&str> = snapshot
        .roots
        .iter()
        .flat_map(|catalog| match &catalog.children {
            db_core::schema::Children::Loaded(schemas) => schemas.iter(),
            db_core::schema::Children::NotLoaded => [].iter(),
        })
        .flat_map(|schema| match &schema.children {
            db_core::schema::Children::Loaded(tables) => tables.iter(),
            db_core::schema::Children::NotLoaded => [].iter(),
        })
        .map(|table| table.name.as_str())
        .collect();
    assert!(table_names.contains(&"t"), "tables seen: {table_names:?}");
}

#[cfg(feature = "db-integration")]
#[test]
fn the_sqlite_odbc_driver_must_be_present_under_db_integration() {
    assert!(
        sqlite_odbc_driver_available(),
        "db-integration expects libsqliteodbc + unixodbc-dev inside linux-builder (F8.6)"
    );
}
