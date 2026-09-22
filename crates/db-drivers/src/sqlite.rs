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
    ConstraintKind, IntrospectLevel, IntrospectScope, Node, NodeDetail, ObjectKind, ObjectRef,
    SchemaSnapshot,
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
            children.extend(self.constraint_nodes(name)?);
            children.extend(self.trigger_nodes(name)?);
        }
        Ok(Node::with_children(name, object_kind, children))
    }

    /// Whether `table`'s own `CREATE TABLE` text uses the
    /// `INTEGER PRIMARY KEY AUTOINCREMENT` form — the only case SQLite
    /// auto-generates a column's value (a plain `INTEGER PRIMARY KEY`
    /// aliases `rowid` but never guarantees monotonic reuse-free values).
    /// A single substring search on `sqlite_master.sql`, not a parse —
    /// good enough for the one keyword this needs and never mistaken for
    /// column-level detail no cheaper query would give us anyway.
    fn table_has_autoincrement(&self, table: &str) -> bool {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1 AND type = 'table'",
                [table],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .ok()
            .flatten()
            .flatten()
            .is_some_and(|sql| sql.to_uppercase().contains("AUTOINCREMENT"))
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
        // AUTOINCREMENT only ever applies to a single-column INTEGER
        // PRIMARY KEY, so this is cheap to compute once per table rather
        // than per column.
        let single_int_pk = columns.iter().filter(|(_, _, _, _, pk)| *pk > 0).count() == 1;
        let has_autoincrement = single_int_pk && self.table_has_autoincrement(table);
        Ok(columns
            .into_iter()
            .map(|(name, type_name, not_null, default, pk)| {
                Node::leaf(name, ObjectKind::Column).with_detail(NodeDetail {
                    type_name: (!type_name.is_empty()).then_some(type_name),
                    nullable: Some(not_null == 0),
                    default,
                    primary_key: pk > 0,
                    auto_increment: (pk > 0).then_some(has_autoincrement),
                    ..NodeDetail::default()
                })
            })
            .collect())
    }

    /// One index's row from `PRAGMA index_list`: name, whether it is
    /// unique, and its `origin` (`c` a plain `CREATE INDEX`, `u` a
    /// `UNIQUE` column/table constraint, `pk` the primary key's own
    /// index) — `constraint_nodes` uses `origin` to avoid reporting a
    /// `UNIQUE` constraint's backing index twice, once as an index and
    /// once as a constraint with no columns of its own.
    fn index_rows(&self, table: &str) -> Result<Vec<(String, bool, String)>, DbError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "PRAGMA index_list({})",
                Dialect::Sqlite.quote_ident(table)
            ))
            .map_err(map_err)?;
        let rows = stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let unique: i64 = row.get(2)?;
                let origin: String = row.get(3)?;
                Ok((name, unique != 0, origin))
            })
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(rows)
    }

    fn index_columns(&self, index: &str) -> Result<Vec<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "PRAGMA index_info({})",
                Dialect::Sqlite.quote_ident(index)
            ))
            .map_err(map_err)?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(2))
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(columns)
    }

    fn index_nodes(&self, table: &str) -> Result<Vec<Node>, DbError> {
        let mut nodes = Vec::new();
        for (name, unique, _origin) in self.index_rows(table)? {
            let columns = self.index_columns(&name)?;
            nodes.push(Node::leaf(name, ObjectKind::Index).with_detail(NodeDetail {
                index: Some(db_core::schema::IndexDetail {
                    columns,
                    unique,
                    method: Some("btree".to_string()),
                }),
                ..NodeDetail::default()
            }));
        }
        Ok(nodes)
    }

    /// Primary key, unique and foreign key constraints as `Constraint`
    /// nodes — SQLite exposes no `CHECK` constraint listing short of
    /// parsing `sqlite_master.sql` itself, so `Check` constraints are
    /// left out here (database-tools.md §11).
    fn constraint_nodes(&self, table: &str) -> Result<Vec<Node>, DbError> {
        let mut nodes = Vec::new();

        let pk_columns: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "PRAGMA table_info({})",
                    Dialect::Sqlite.quote_ident(table)
                ))
                .map_err(map_err)?;
            let mut ordered = stmt
                .query_map([], |row| {
                    let name: String = row.get(1)?;
                    let pk: i64 = row.get(5)?;
                    Ok((pk, name))
                })
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;
            ordered.retain(|(pk, _)| *pk > 0);
            ordered.sort_by_key(|(pk, _)| *pk);
            ordered.into_iter().map(|(_, name)| name).collect()
        };
        if !pk_columns.is_empty() {
            nodes.push(
                Node::leaf(format!("{table}_pkey"), ObjectKind::Constraint).with_detail(
                    NodeDetail {
                        constraint: Some(ConstraintKind::PrimaryKey {
                            columns: pk_columns,
                        }),
                        ..NodeDetail::default()
                    },
                ),
            );
        }

        for (name, unique, origin) in self.index_rows(table)? {
            if origin == "u" && unique {
                let columns = self.index_columns(&name)?;
                nodes.push(
                    Node::leaf(name, ObjectKind::Constraint).with_detail(NodeDetail {
                        constraint: Some(ConstraintKind::Unique { columns }),
                        ..NodeDetail::default()
                    }),
                );
            }
        }

        let mut fk_stmt = self
            .conn
            .prepare(&format!(
                "PRAGMA foreign_key_list({})",
                Dialect::Sqlite.quote_ident(table)
            ))
            .map_err(map_err)?;
        let fk_rows = fk_stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let seq: i64 = row.get(1)?;
                let ref_table: String = row.get(2)?;
                let from: String = row.get(3)?;
                let to: String = row.get(4)?;
                let on_update: String = row.get(5)?;
                let on_delete: String = row.get(6)?;
                Ok((id, seq, ref_table, from, to, on_update, on_delete))
            })
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        struct FkGroup {
            ref_table: String,
            columns: Vec<(i64, String, String)>,
            on_update: String,
            on_delete: String,
        }
        let mut by_id: std::collections::BTreeMap<i64, FkGroup> = std::collections::BTreeMap::new();
        for (id, seq, ref_table, from, to, on_update, on_delete) in fk_rows {
            let entry = by_id.entry(id).or_insert_with(|| FkGroup {
                ref_table,
                columns: Vec::new(),
                on_update,
                on_delete,
            });
            entry.columns.push((seq, from, to));
        }
        for (
            id,
            FkGroup {
                ref_table,
                mut columns,
                on_update,
                on_delete,
            },
        ) in by_id
        {
            columns.sort_by_key(|(seq, _, _)| *seq);
            let cols = columns;
            let columns: Vec<String> = cols.iter().map(|(_, from, _)| from.clone()).collect();
            let ref_columns: Vec<String> = cols.iter().map(|(_, _, to)| to.clone()).collect();
            let none_action = |a: &str| a.is_empty() || a.eq_ignore_ascii_case("NO ACTION");
            nodes.push(
                Node::leaf(format!("{table}_fk_{id}"), ObjectKind::Constraint).with_detail(
                    NodeDetail {
                        constraint: Some(ConstraintKind::ForeignKey {
                            columns,
                            ref_table: ObjectRef::new(ref_table).with_kind(ObjectKind::Table),
                            ref_columns,
                            on_delete: (!none_action(&on_delete)).then_some(on_delete),
                            on_update: (!none_action(&on_update)).then_some(on_update),
                        }),
                        ..NodeDetail::default()
                    },
                ),
            );
        }
        Ok(nodes)
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
#[path = "sqlite_tests.rs"]
mod tests;
