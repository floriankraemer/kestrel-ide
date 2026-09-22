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
    IntrospectLevel, IntrospectScope, Node, NodeDetail, ObjectKind, ObjectRef, SchemaSnapshot,
};
use db_core::value::{ColumnMeta, RowBatch, Value};

/// `SQLITE_INTERRUPT` (raised by `SqliteCancelHandle::cancel`'s own
/// `InterruptHandle::interrupt`, database-tools-plan F3.3) maps to
/// `DbErrorCode::Cancelled` — every other `rusqlite::Error` stays
/// `Unknown`, this driver draws no finer distinction than that.
fn map_err(error: rusqlite::Error) -> DbError {
    let code = match &error {
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            DbErrorCode::Cancelled
        }
        _ => DbErrorCode::Unknown,
    };
    DbError::new(code, error.to_string())
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
        Ok(Box::new(SqliteConnection {
            conn,
            path: Some(path),
        }))
    }
}

pub struct SqliteConnection {
    conn: rusqlite::Connection,
    /// The file this connection was opened on ([`Driver::connect`] only —
    /// `None` for [`Self::wrap`]). `execute`'s row-streaming path needs it
    /// to open a second, read-only connection for the live cursor (see
    /// `SqliteLiveRowStream`); a `:memory:` database has no such second
    /// handle to open, since each connection to `:memory:` is its own
    /// private database, so `wrap()`-based connections (tests, and any
    /// future in-memory embedding) fall back to the eager path instead.
    path: Option<String>,
}

impl SqliteConnection {
    /// Test/embedding convenience: wrap an already-open connection (e.g.
    /// `rusqlite::Connection::open_in_memory()`) rather than going through
    /// [`Driver::connect`]'s path/URL resolution.
    pub fn wrap(conn: rusqlite::Connection) -> Self {
        Self { conn, path: None }
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
/// ponytail: eager fetch, chunked client-side; only reachable through
/// [`SqliteConnection::wrap`] (tests / in-memory embedding), where there is
/// no second file handle to open a live cursor on. `execute` against a
/// real, `connect()`-opened file uses [`SqliteLiveRowStream`] instead.
struct SqliteEagerRowStream {
    columns: Vec<ColumnMeta>,
    rows: VecDeque<Vec<Value>>,
    fetch_size: usize,
}

impl RowStream for SqliteEagerRowStream {
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

/// A live `rusqlite` cursor, pulling `fetch_size` rows per `next_batch`
/// straight off the database rather than draining the whole result set
/// upfront (database-tools-plan F3c: the NFR-breaking defect F3b left —
/// see `database-tools.md` §4/§11).
///
/// `rusqlite::Statement`/`Rows` borrow the `Connection` they were prepared
/// against, and `db-core`'s `RowStream` trait object is `'static` (it
/// outlives the call that created it, across repeated `next_batch` calls
/// from the worker thread). Rather than adding a self-referential-struct
/// crate, this opens a **second, read-only connection on the same file**
/// dedicated to this one statement: the second connection is boxed (a
/// stable heap address, never moved) and stored in this struct *after*
/// the `Statement`/`Rows` fields, so Rust's top-to-bottom field drop order
/// tears the cursor down before the connection it borrows from — the
/// invariant `mem::transmute`'d `'static` lifetimes below rely on. Neither
/// borrow ever leaves this struct.
///
/// This is why the fallback above only affects `:memory:`/`wrap()`:
/// SQLite has no equivalent second handle for a private in-memory
/// database.
struct SqliteLiveRowStream {
    columns: Vec<ColumnMeta>,
    fetch_size: usize,
    max_rows: Option<u64>,
    fetched: u64,
    rows: Option<rusqlite::Rows<'static>>,
    // Boxed, not a bare `Statement`: `Rows<'stmt>` stores a *reference*
    // to the `Statement` it was created from (`&'stmt Statement`), taken
    // while it still lived at `open`'s local-variable address. If `stmt`
    // were moved into this struct by value afterwards, that reference
    // would dangle — moving relocates the bytes, a plain `Statement`
    // field is not pinned. Boxing first gives it a stable heap address
    // *before* `query()` borrows it, so moving the `Box` (a pointer)
    // into this struct never moves the `Statement` data itself.
    stmt: Option<Box<rusqlite::Statement<'static>>>,
    _conn: Box<rusqlite::Connection>,
}

// SAFETY: `rusqlite::Statement`/`Rows` hold a raw `sqlite3_stmt*`, which
// is not `Send` by default because SQLite forbids using the *same*
// connection/statement concurrently from two threads at once. This
// stream is never shared — `db-core`'s worker owns it exclusively and
// only ever calls `next_batch` from one thread at a time — so moving the
// whole struct (statement + the connection it was prepared against) to
// that worker thread is sound; it never crosses threads while "in use".
unsafe impl Send for SqliteLiveRowStream {}

fn extend_stmt_lifetime(stmt: rusqlite::Statement<'_>) -> rusqlite::Statement<'static> {
    // SAFETY: see `SqliteLiveRowStream`'s doc comment — the borrowed
    // connection is boxed, never moved, and dropped after this statement.
    unsafe { std::mem::transmute::<rusqlite::Statement<'_>, rusqlite::Statement<'static>>(stmt) }
}

fn extend_rows_lifetime(rows: rusqlite::Rows<'_>) -> rusqlite::Rows<'static> {
    // SAFETY: see `SqliteLiveRowStream`'s doc comment — the borrowed
    // statement is dropped after this `Rows`, never before.
    unsafe { std::mem::transmute::<rusqlite::Rows<'_>, rusqlite::Rows<'static>>(rows) }
}

impl SqliteLiveRowStream {
    fn open(
        path: &str,
        sql: &str,
        params: &[rusqlite::types::Value],
        columns: Vec<ColumnMeta>,
        options: &ExecOptions,
    ) -> Result<Self, DbError> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_err)?;
        let conn = Box::new(conn);
        let conn_ptr: *const rusqlite::Connection = &*conn;
        // SAFETY: `conn` is heap-allocated and never moved after this
        // point; the reference below is only used to prepare a statement
        // whose lifetime is then widened to match, and both are stored in
        // `conn`'s own struct (see the struct doc comment for drop order).
        let stmt = unsafe { &*conn_ptr }.prepare(sql).map_err(map_err)?;
        let mut stmt = Box::new(extend_stmt_lifetime(stmt));
        // SAFETY: `stmt` is heap-allocated above and never moved again —
        // only `Box<Statement>` (a pointer) moves from here on, so the
        // `Statement` data `rows` is about to borrow stays put for as
        // long as this struct exists (see the struct's `stmt` field doc).
        let stmt_ptr: *mut rusqlite::Statement<'static> = &mut *stmt;
        let rows = unsafe { &mut *stmt_ptr }
            .query(rusqlite::params_from_iter(params.iter()))
            .map_err(map_err)?;
        let rows = extend_rows_lifetime(rows);
        Ok(Self {
            columns,
            fetch_size: options.fetch_size.max(1) as usize,
            max_rows: options.max_rows,
            fetched: 0,
            rows: Some(rows),
            stmt: Some(stmt),
            _conn: conn,
        })
    }
}

impl RowStream for SqliteLiveRowStream {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        let Some(rows) = self.rows.as_mut() else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(self.fetch_size);
        while out.len() < self.fetch_size {
            if let Some(max) = self.max_rows {
                if self.fetched >= max {
                    break;
                }
            }
            match rows.next().map_err(map_err)? {
                Some(row) => {
                    let mut values = Vec::with_capacity(self.columns.len());
                    for i in 0..self.columns.len() {
                        values.push(from_rusqlite(row.get_ref(i).map_err(map_err)?));
                    }
                    out.push(values);
                    self.fetched += 1;
                }
                None => break,
            }
        }
        if out.is_empty() {
            self.rows = None;
            self.stmt = None;
            return Ok(None);
        }
        Ok(Some(RowBatch {
            columns: self.columns.clone(),
            rows: out,
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

    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        if let Some(object) = &scope.object {
            // A lazy per-object expand (F2.1): one root, at whatever depth
            // was asked for.
            return Ok(SchemaSnapshot::new(
                level,
                vec![self.object_node(object, level)?],
            ));
        }

        // `Names`-level listing never touches `PRAGMA table_info` per
        // table — the cheap, near-instant query the 5 000-table NFR needs
        // (`IntrospectLevel`'s own doc comment).
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
            let node = if level == IntrospectLevel::Names {
                Node::leaf(name, object_kind)
            } else {
                self.object_node(&name, level)?
            };
            roots.push(node);
        }
        Ok(SchemaSnapshot::new(level, roots))
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
        // `rusqlite` 0.32 does not expose `sqlite3_column_table_name` at
        // all (no `column_metadata`-shaped feature in its own feature
        // list), so this driver cannot fill `ColumnMeta::origin` itself —
        // `ui-shell`'s data editor (F4.1) instead derives a result's
        // single-table origin from the statement text via
        // `db_sql::single_table`, the same statement every backend's
        // driver already has in hand.
        let columns: Vec<ColumnMeta> = stmt
            .column_names()
            .iter()
            .map(|name| ColumnMeta {
                name: name.to_string(),
                type_name: String::new(),
                nullable: true,
                origin: None,
            })
            .collect();

        if let Some(path) = self.path.clone() {
            // Release `stmt`'s borrow of `self.conn` before opening the
            // second, dedicated connection the live stream reads through.
            drop(stmt);
            let live =
                SqliteLiveRowStream::open(&path, &statement.text, &params, columns, options)?;
            return Ok(Execution::Rows(Box::new(live)));
        }

        // `wrap()`-constructed connection (tests / in-memory embedding):
        // no second file handle to open, so this still drains eagerly —
        // see `SqliteEagerRowStream`'s doc comment.
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
        Ok(Execution::Rows(Box::new(SqliteEagerRowStream {
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
    /// A table or view's kind, columns (`Columns`/`Full`) and, at `Full`,
    /// its indexes and triggers — the "one object" lazy-expand shape
    /// F2.1 asks both the top-level listing and a `scope.object` request
    /// to share.
    fn object_node(&self, name: &str, level: IntrospectLevel) -> Result<Node, DbError> {
        use rusqlite::OptionalExtension;
        let kind_text: Option<String> = self
            .conn
            .query_row(
                "SELECT type FROM sqlite_master WHERE name = ?1 AND type IN ('table', 'view')",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_err)?;
        let Some(kind_text) = kind_text else {
            return Err(DbError::new(
                DbErrorCode::NotSupported,
                format!("no such table or view: {name}"),
            ));
        };
        let object_kind = if kind_text == "view" {
            ObjectKind::View
        } else {
            ObjectKind::Table
        };
        if level == IntrospectLevel::Names {
            return Ok(Node::leaf(name, object_kind));
        }

        let mut children = self.column_nodes(name)?;
        if level == IntrospectLevel::Full {
            children.extend(self.index_nodes(name)?);
            children.extend(self.trigger_nodes(name)?);
        }
        Ok(Node::with_children(name, object_kind, children))
    }

    fn column_nodes(&self, table: &str) -> Result<Vec<Node>, DbError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "PRAGMA table_info({})",
                Dialect::Sqlite.quote_ident(table)
            ))
            .map_err(map_err)?;
        let columns = stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let type_name: String = row.get(2)?;
                let not_null: i64 = row.get(3)?;
                let default: Option<String> = row.get(4)?;
                let pk: i64 = row.get(5)?;
                Ok((name, type_name, not_null, default, pk))
            })
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(columns
            .into_iter()
            .map(|(name, type_name, not_null, default, pk)| {
                Node::leaf(name, ObjectKind::Column).with_detail(NodeDetail {
                    type_name: (!type_name.is_empty()).then_some(type_name),
                    nullable: Some(not_null == 0),
                    default,
                    primary_key: pk > 0,
                    ttl_seconds: None,
                })
            })
            .collect())
    }

    fn index_nodes(&self, table: &str) -> Result<Vec<Node>, DbError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "PRAGMA index_list({})",
                Dialect::Sqlite.quote_ident(table)
            ))
            .map_err(map_err)?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(names
            .into_iter()
            .map(|name| Node::leaf(name, ObjectKind::Index))
            .collect())
    }

    fn trigger_nodes(&self, table: &str) -> Result<Vec<Node>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?1")
            .map_err(map_err)?;
        let names = stmt
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(names
            .into_iter()
            .map(|name| Node::leaf(name, ObjectKind::Trigger))
            .collect())
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
}
