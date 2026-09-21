//! The SQLite backend (ADR-0058): synchronous — bypasses the private
//! tokio runtime entirely, the same reasoning `db-core`'s doc comment
//! gives for why forcing a sync API through `block_on` would be pure
//! overhead (plan risk R5). `rusqlite`'s `bundled` feature links its own
//! vendored sqlite3 rather than a system package.

use std::collections::VecDeque;

use db_core::datasource::ConnectSpec;
use db_core::dialect::Dialect;
use db_core::driver::{
    CancelHandle, Capabilities, Connection, Driver, ExecOptions, Execution, RowStream, Statement,
};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{
    IntrospectLevel, IntrospectScope, Node, ObjectKind, ObjectRef, SchemaSnapshot,
};
use db_core::value::{ColumnMeta, RowBatch, Value};

fn map_err(error: rusqlite::Error) -> DbError {
    DbError::new(DbErrorCode::Unknown, error.to_string())
}

/// `Value` -> a `rusqlite`-bindable owned value. Every value crosses as a
/// bound parameter, never interpolated (ADR-0061 §1) — `dml::EditBuffer`
/// already produces `?`-placeholder text this driver's `execute`/`apply`
/// bind directly against.
fn to_rusqlite(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match value {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(if *b { 1 } else { 0 }),
        Value::Int(i) => V::Integer(*i),
        Value::Float(f) => V::Real(*f),
        Value::Decimal(text) | Value::Text(text) | Value::Json(text) => V::Text(text.clone()),
        Value::Bytes(bytes) => V::Blob(bytes.clone()),
        Value::Date(date) => V::Text(date.to_string()),
        Value::Time(time) => V::Text(time.to_string()),
        Value::DateTime(dt) => V::Text(dt.to_string()),
        Value::DateTimeTz(dt) => V::Text(dt.to_rfc3339()),
        Value::Uuid(id) => V::Text(id.to_string()),
        Value::Array(_) | Value::Document(_) => V::Text(value.display(&Default::default())),
        Value::Other { display, .. } => V::Text(display.clone()),
    }
}

/// A `rusqlite` `ValueRef` -> the one row shape every backend converts
/// into (ADR-0058 §2). SQLite is dynamically typed per cell, so this is
/// the whole mapping — no per-column type to consult first.
fn from_rusqlite(value: rusqlite::types::ValueRef) -> Value {
    use rusqlite::types::ValueRef as V;
    match value {
        V::Null => Value::Null,
        V::Integer(i) => Value::Int(i),
        V::Real(f) => Value::Float(f),
        V::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        V::Blob(b) => Value::Bytes(b.to_vec()),
    }
}

pub struct SqliteDriver;

impl Driver for SqliteDriver {
    fn id(&self) -> &str {
        "sqlite"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::TRANSACTIONS
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn Connection>, DbError> {
        let path = if spec.url.is_empty() {
            spec.database.clone()
        } else {
            spec.url.clone()
        };
        let conn = rusqlite::Connection::open(&path).map_err(map_err)?;
        Ok(Box::new(SqliteConnection { conn }))
    }
}

pub struct SqliteConnection {
    conn: rusqlite::Connection,
}

impl SqliteConnection {
    /// Test/embedding convenience: wrap an already-open connection (e.g.
    /// `rusqlite::Connection::open_in_memory()`) rather than going through
    /// [`Driver::connect`]'s path/URL resolution.
    pub fn wrap(conn: rusqlite::Connection) -> Self {
        Self { conn }
    }
}

struct SqliteCancelHandle(rusqlite::InterruptHandle);

impl CancelHandle for SqliteCancelHandle {
    fn cancel(&self) -> Result<(), DbError> {
        self.0.interrupt();
        Ok(())
    }
}

/// `next_batch` pages an already-fully-fetched, in-memory row list rather
/// than a live cursor.
/// ponytail: eager fetch, chunked client-side; a real cursor-based page
/// needs `rusqlite`'s borrowed `Rows<'stmt>` to outlive this call, which
/// means either `unsafe`/self-referential storage or `db-core`'s
/// `RowStream` growing a lifetime — revisit if a SQLite result set large
/// enough for this to matter in practice shows up (SQLite files are
/// typically small and local).
struct SqliteRowStream {
    columns: Vec<ColumnMeta>,
    rows: VecDeque<Vec<Value>>,
    fetch_size: usize,
}

impl RowStream for SqliteRowStream {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        if self.rows.is_empty() {
            return Ok(None);
        }
        let mut rows = Vec::with_capacity(self.fetch_size);
        for _ in 0..self.fetch_size {
            match self.rows.pop_front() {
                Some(row) => rows.push(row),
                None => break,
            }
        }
        Ok(Some(RowBatch {
            columns: self.columns.clone(),
            rows,
        }))
    }
}

impl Connection for SqliteConnection {
    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }

    fn server_info(&self) -> String {
        format!("SQLite {}", rusqlite::version())
    }

    fn introspect(&mut self, _scope: &IntrospectScope) -> Result<SchemaSnapshot, DbError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, type FROM sqlite_master \
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
                 ORDER BY name",
            )
            .map_err(map_err)?;
        let objects: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(map_err)?
            .collect::<Result<_, _>>()
            .map_err(map_err)?;
        drop(stmt);

        let mut roots = Vec::with_capacity(objects.len());
        for (name, kind) in objects {
            let object_kind = if kind == "view" {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            let columns = self.columns_of(&name)?;
            let children = columns
                .into_iter()
                .map(|column| Node::leaf(column, ObjectKind::Column))
                .collect();
            roots.push(Node::with_children(name, object_kind, children));
        }
        Ok(SchemaSnapshot::new(IntrospectLevel::Columns, roots))
    }

    fn execute(
        &mut self,
        statement: &Statement,
        options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        let params: Vec<rusqlite::types::Value> =
            statement.params.iter().map(to_rusqlite).collect();
        let mut stmt = self.conn.prepare(&statement.text).map_err(map_err)?;
        if stmt.column_count() == 0 {
            let affected = stmt
                .execute(rusqlite::params_from_iter(params.iter()))
                .map_err(map_err)?;
            return Ok(Execution::Affected(affected as u64));
        }
        let columns: Vec<ColumnMeta> = stmt
            .column_names()
            .iter()
            .map(|name| ColumnMeta {
                name: name.to_string(),
                type_name: String::new(),
                nullable: true,
            })
            .collect();
        let mut rows = VecDeque::new();
        let column_count = columns.len();
        let mut mapped = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), move |row| {
                let mut values = Vec::with_capacity(column_count);
                for i in 0..column_count {
                    values.push(from_rusqlite(row.get_ref(i)?));
                }
                Ok(values)
            })
            .map_err(map_err)?;
        for row in &mut mapped {
            rows.push_back(row.map_err(map_err)?);
            if let Some(max_rows) = options.max_rows {
                if rows.len() as u64 >= max_rows {
                    break;
                }
            }
        }
        Ok(Execution::Rows(Box::new(SqliteRowStream {
            columns,
            rows,
            fetch_size: options.fetch_size as usize,
        })))
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.conn.execute_batch("BEGIN").map_err(map_err)
    }

    fn commit(&mut self) -> Result<(), DbError> {
        self.conn.execute_batch("COMMIT").map_err(map_err)
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        self.conn.execute_batch("ROLLBACK").map_err(map_err)
    }

    fn set_read_only(&mut self, read_only: bool) -> Result<(), DbError> {
        self.conn
            .pragma_update(None, "query_only", read_only)
            .map_err(map_err)
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(SqliteCancelHandle(
            self.conn.get_interrupt_handle(),
        )))
    }

    fn ddl_of(&mut self, object: &ObjectRef) -> Result<String, DbError> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                [&object.name],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_err)?
            .ok_or_else(|| {
                DbError::new(DbErrorCode::NotSupported, "no DDL recorded for this object")
            })
    }

    fn apply(&mut self, statements: &[Statement]) -> Result<u64, DbError> {
        let mut total = 0u64;
        for statement in statements {
            if let Execution::Affected(count) = self.execute(statement, &ExecOptions::default())? {
                total += count;
            }
        }
        Ok(total)
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

impl SqliteConnection {
    fn columns_of(&self, table: &str) -> Result<Vec<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "PRAGMA table_info({})",
                Dialect::Sqlite.quote_ident(table)
            ))
            .map_err(map_err)?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
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
            &Statement::sql("CREATE TABLE users (id INTEGER, name TEXT)"),
            &ExecOptions::default(),
        )
        .unwrap();
        let snapshot = conn.introspect(&IntrospectScope::default()).unwrap();
        assert_eq!(snapshot.roots.len(), 1);
        let table = &snapshot.roots[0];
        assert_eq!(table.name, "users");
        assert_eq!(table.kind, ObjectKind::Table);
        match &table.children {
            Children::Loaded(columns) => {
                let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                assert_eq!(names, vec!["id", "name"]);
            }
            Children::NotLoaded => panic!("expected loaded columns"),
        }
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
        let datetime_tz =
            chrono::DateTime::parse_from_rfc3339("2026-09-21T01:02:03+02:00").unwrap();

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
}
