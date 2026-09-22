//! The MySQL/MariaDB backend (ADR-0058, database-tools-plan §3/§13):
//! async, `block_on`'d against the private `"db-io"` runtime, `mysql_async`
//! 0.37 with `rustls`'s `ring` provider only (ADR-0061 §2/R2) and a pure
//! Rust compression codec (`flate2/rust_backend`) — no `aws-lc-rs`, no C
//! zlib, matching the same posture `postgres.rs` documents for its own
//! dependency choices.
//!
//! ## The parked cursor (F3d's own NFR, applied to a protocol that cannot
//! pipeline)
//!
//! Unlike `tokio-postgres`, which hands back an owned, `'static` row
//! stream from `Client::query_raw`, `mysql_async`'s `QueryResult` borrows
//! `&mut Conn` for as long as the result set is being read — the MySQL
//! wire protocol allows exactly one command in flight per connection at a
//! time. A live [`RowStream`] must still outlive the call that created it
//! (`db-core`'s trait object is `'static`), so [`MySqlConnection`] moves
//! its `Conn` *out* of a shared slot into the [`MySqlRowStream`] for as
//! long as the stream is open, and the stream hands it back — to the same
//! slot, so a second `MySqlConnection` method reusing `&mut self` sees it
//! again — once exhausted or dropped. `Arc<Mutex<..>>` rather than a raw
//! pointer/unsafe-lifetime trick (contrast `sqlite.rs`'s live cursor): the
//! two owners (`MySqlConnection`, `MySqlRowStream`) are independent Rust
//! values with independent drop timing, and safe shared ownership is the
//! straightforward tool for that — the `Mutex` is never contended (every
//! call funnels through the one worker thread the private runtime's
//! `block_on` calls happen on), so this is not a performance concern.
//!
//! A caller that runs a second `execute` while a previous stream is still
//! open (never drained, never dropped) gets a clear `ConnectionFailed`
//! rather than a hang or a silent wrong answer — the "drains or drops the
//! previous" rule the plan describes is what happens automatically once
//! the *caller* drops (or fully drains) the stream it already has, which
//! is how every consumer in this codebase actually uses one connection
//! (`db_core::session::Session` keeps at most one open result at a time).

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_util::{Stream, StreamExt};
use mysql_async::consts::ColumnType;
use mysql_async::prelude::Queryable;
use mysql_async::{Column, Conn, Opts, OptsBuilder, Params, SslOpts};

use db_core::datasource::{ConnectSpec, SslConfig, SslMode};
use db_core::dialect::Dialect;
use db_core::driver::{
    CancelHandle, Capabilities, Connection, Driver, ExecOptions, Execution, RowStream, Statement,
};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{
    ConstraintKind, IndexDetail, IntrospectLevel, IntrospectScope, Node, NodeDetail, ObjectKind,
    ObjectRef, SchemaSnapshot,
};
use db_core::value::{ColumnMeta, RowBatch, Value};

const NAMES_QUERY: &str = include_str!("introspect/mysql_names.sql");
const COLUMNS_QUERY: &str = include_str!("introspect/mysql_columns.sql");
const INDEXES_QUERY: &str = include_str!("introspect/mysql_indexes.sql");
const CONSTRAINTS_QUERY: &str = include_str!("introspect/mysql_constraints.sql");
const CHECKS_QUERY: &str = include_str!("introspect/mysql_checks.sql");

/// A boxed, lifetime-erased row stream — see `MySqlRowStream`'s own
/// field comments for why the stream this crate builds needs to be
/// `'static` at all.
type BoxedRowStream = Pin<Box<dyn Stream<Item = Result<Vec<Value>, DbError>> + Send>>;

/// One `mysql_columns.sql` row, before it becomes a `Node`.
type ColumnRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

fn io_err(message: impl std::fmt::Display) -> DbError {
    DbError::new(DbErrorCode::ConnectionFailed, message.to_string())
}

fn busy_err() -> DbError {
    DbError::new(
        DbErrorCode::ConnectionFailed,
        "connection is busy with a previous unfinished result — drain or drop it first",
    )
}

/// See this module's own doc comment for the per-mode reasoning (identical
/// to `postgres.rs`'s `tls_config_for`, just against `mysql_async`'s own
/// `SslOpts` builder rather than a `rustls::ClientConfig`).
fn ssl_opts_for(ssl: &SslConfig) -> SslOpts {
    let opts = SslOpts::default();
    match ssl.mode {
        SslMode::Disable => unreachable!("connect() only calls this for a non-Disable mode"),
        SslMode::Prefer | SslMode::Require => opts
            .with_danger_accept_invalid_certs(true)
            .with_danger_skip_domain_validation(true),
        SslMode::VerifyCa | SslMode::VerifyFull => match &ssl.ca_file {
            Some(path) => opts.with_root_certs(vec![std::path::PathBuf::from(path.clone()).into()]),
            // No `ca_file` configured: `mysql_async` falls back to the OS
            // trust store on its own (same default `postgres.rs`'s
            // `root_store_for` gives explicitly via `rustls-native-certs`).
            None => opts,
        },
    }
}

fn build_opts(spec: &ConnectSpec) -> Opts {
    let mut builder = OptsBuilder::default()
        .ip_or_hostname(spec.host.clone())
        .tcp_port(spec.port.unwrap_or(3306))
        .user(Some(spec.user.clone()))
        .pass(spec.password.clone());
    if !spec.database.is_empty() {
        builder = builder.db_name(Some(spec.database.clone()));
    }
    if spec.ssl.mode != SslMode::Disable {
        builder = builder.ssl_opts(Some(ssl_opts_for(&spec.ssl)));
    }
    builder.into()
}

/// `Value` -> a bound `mysql_async` parameter. Every value crosses as a
/// parameter, never interpolated (ADR-0061 §1) — constructed from
/// `mysql_common`'s own wire-level variants directly rather than via the
/// crate's `chrono`-feature `From` impls, since this driver deliberately
/// stays off that feature (fewer optional dependencies for the one crate
/// in this tree that would otherwise need it, YAGNI).
fn to_mysql_value(value: &Value) -> mysql_async::Value {
    use mysql_async::Value as MV;
    match value {
        Value::Null => MV::NULL,
        Value::Bool(b) => MV::Int(*b as i64),
        Value::Int(i) => MV::Int(*i),
        Value::Float(f) => MV::Double(*f),
        Value::Decimal(text) | Value::Text(text) | Value::Json(text) => {
            MV::Bytes(text.clone().into_bytes())
        }
        Value::Bytes(bytes) => MV::Bytes(bytes.clone()),
        Value::Uuid(id) => MV::Bytes(id.to_string().into_bytes()),
        Value::Date(date) => {
            use chrono::Datelike;
            MV::Date(
                date.year() as u16,
                date.month() as u8,
                date.day() as u8,
                0,
                0,
                0,
                0,
            )
        }
        Value::Time(time) => {
            use chrono::Timelike;
            MV::Time(
                false,
                0,
                time.hour() as u8,
                time.minute() as u8,
                time.second() as u8,
                time.nanosecond() / 1000,
            )
        }
        Value::DateTime(dt) => {
            use chrono::{Datelike, Timelike};
            MV::Date(
                dt.year() as u16,
                dt.month() as u8,
                dt.day() as u8,
                dt.hour() as u8,
                dt.minute() as u8,
                dt.second() as u8,
                dt.nanosecond() / 1000,
            )
        }
        other => MV::Bytes(other.display(&Default::default()).into_bytes()),
    }
}

/// A MySQL column's declared type -> the `Value` variant its cells decode
/// into — the driver boundary ADR-0058 §2 describes. Unsigned integers
/// stay `Value::Int` (a `u64` too large for `i64` is astronomically rare
/// in practice and falls back to `Other` rather than silently wrapping).
fn mysql_value_to_value(raw: mysql_async::Value, column: &Column) -> Value {
    use mysql_async::Value as MV;
    match raw {
        MV::NULL => Value::Null,
        MV::Bytes(bytes) => bytes_to_value(bytes, column),
        MV::Int(i) => Value::Int(i),
        MV::UInt(u) => match i64::try_from(u) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Other {
                type_name: "bigint unsigned".to_string(),
                display: u.to_string(),
            },
        },
        MV::Float(f) => Value::Float(f as f64),
        MV::Double(f) => Value::Float(f),
        MV::Date(year, month, day, hour, minute, second, micros) => {
            let Some(date) = chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)
            else {
                return Value::Other {
                    type_name: "date".to_string(),
                    display: format!("{year:04}-{month:02}-{day:02}"),
                };
            };
            if hour == 0
                && minute == 0
                && second == 0
                && micros == 0
                && matches!(
                    column.column_type(),
                    ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE
                )
            {
                return Value::Date(date);
            }
            let Some(time) = chrono::NaiveTime::from_hms_micro_opt(
                hour as u32,
                minute as u32,
                second as u32,
                micros,
            ) else {
                return Value::Other {
                    type_name: "datetime".to_string(),
                    display: format!(
                        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
                    ),
                };
            };
            Value::DateTime(date.and_time(time))
        }
        MV::Time(negative, days, hours, minutes, seconds, micros) => {
            // MySQL's TIME can exceed 24h and be negative — neither fits
            // `NaiveTime`; both are rare enough in practice (an interval,
            // not a wall-clock time) that `Other` with the verbatim text
            // is the honest answer rather than a lossy wraparound.
            let total_hours = days as i64 * 24 + hours as i64;
            if !negative && total_hours < 24 {
                if let Some(time) = chrono::NaiveTime::from_hms_micro_opt(
                    total_hours as u32,
                    minutes as u32,
                    seconds as u32,
                    micros,
                ) {
                    return Value::Time(time);
                }
            }
            let sign = if negative { "-" } else { "" };
            Value::Other {
                type_name: "time".to_string(),
                display: format!("{sign}{total_hours:02}:{minutes:02}:{seconds:02}.{micros:06}"),
            }
        }
    }
}

fn bytes_to_value(bytes: Vec<u8>, column: &Column) -> Value {
    use ColumnType::*;
    let type_name = column.column_type();
    match type_name {
        MYSQL_TYPE_JSON => Value::Json(String::from_utf8_lossy(&bytes).into_owned()),
        MYSQL_TYPE_NEWDECIMAL | MYSQL_TYPE_DECIMAL => {
            Value::Decimal(String::from_utf8_lossy(&bytes).into_owned())
        }
        MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB
        | MYSQL_TYPE_GEOMETRY => Value::Bytes(bytes),
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_STRING
            if column
                .flags()
                .contains(mysql_async::consts::ColumnFlags::BINARY_FLAG) =>
        {
            Value::Bytes(bytes)
        }
        _ => Value::Text(String::from_utf8_lossy(&bytes).into_owned()),
    }
}

pub struct MySqlDriver;

impl Driver for MySqlDriver {
    fn id(&self) -> &str {
        "mysql"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::TRANSACTIONS | Capabilities::SERVER_SIDE_CANCEL
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn Connection>, DbError> {
        let opts = build_opts(spec);
        let conn = crate::runtime()
            .block_on(Conn::new(opts.clone()))
            .map_err(io_err)?;
        let connection_id = conn.id();
        Ok(Box::new(MySqlConnection {
            slot: Arc::new(Mutex::new(Some(Box::new(conn)))),
            connection_id,
            opts,
        }))
    }
}

type ConnSlot = Arc<Mutex<Option<Box<Conn>>>>;

pub struct MySqlConnection {
    slot: ConnSlot,
    connection_id: u32,
    /// The options this connection was opened with — reused by
    /// [`MySqlCancelHandle`] to open a short-lived second connection for
    /// `KILL QUERY`, the same "server-side cancel needs its own
    /// connection" shape `postgres.rs`'s `PgServerCancel` documents.
    opts: Opts,
}

impl MySqlConnection {
    /// Takes the live `Conn` out of the shared slot — `Err(busy_err())`
    /// when a previous [`MySqlRowStream`] still holds it (see this
    /// module's own doc comment).
    fn take_conn(&self) -> Result<Box<Conn>, DbError> {
        self.slot.lock().unwrap().take().ok_or_else(busy_err)
    }

    fn return_conn(&self, conn: Box<Conn>) {
        *self.slot.lock().unwrap() = Some(conn);
    }

    fn with_conn<T>(&self, f: impl FnOnce(&mut Conn) -> Result<T, DbError>) -> Result<T, DbError> {
        let mut conn = self.take_conn()?;
        let result = f(&mut conn);
        self.return_conn(conn);
        result
    }
}

struct MySqlCancelHandle {
    connection_id: u32,
    opts: Opts,
}

impl CancelHandle for MySqlCancelHandle {
    fn cancel(&self) -> Result<(), DbError> {
        let id = self.connection_id;
        crate::runtime()
            .block_on(async {
                let mut kill_conn = Conn::new(self.opts.clone()).await?;
                kill_conn.query_drop(format!("KILL QUERY {id}")).await
            })
            .map_err(io_err)
    }
}

/// See this module's own doc comment for why this owns the `Conn` for as
/// long as it is open, and hands it back on exhaustion or drop.
struct MySqlRowStream {
    columns: Vec<ColumnMeta>,
    fetch_size: usize,
    max_rows: Option<u64>,
    fetched: u64,
    /// Borrows `query_result` (see that field's own comment) — dropped
    /// first, in [`Self::release`], before `query_result` and `conn`.
    stream: Option<BoxedRowStream>,
    /// Borrows `conn` (a stable heap address, see that field's own
    /// comment) — `stream` above is `QueryResult::stream`'s own
    /// `ResultSetStream`, lifetime-erased to `'static` the same way
    /// `query_result` itself was: both borrows are sound only because
    /// nothing else can reach `conn` while this struct holds it (the
    /// shared slot it came from is `None` until [`Self::release`] puts it
    /// back), the same argument `sqlite.rs`'s live cursor doc comment
    /// makes for its own boxed `Statement`/`Rows` pair.
    query_result:
        Option<Box<mysql_async::QueryResult<'static, 'static, mysql_async::BinaryProtocol>>>,
    conn: Option<Box<Conn>>,
    slot: ConnSlot,
}

impl MySqlRowStream {
    async fn pull_batch(
        stream: &mut BoxedRowStream,
        fetch_size: usize,
        max_rows: Option<u64>,
        fetched: &mut u64,
    ) -> Result<(Vec<Vec<Value>>, bool), DbError> {
        let mut out = Vec::with_capacity(fetch_size);
        let mut exhausted = false;
        while out.len() < fetch_size {
            if let Some(max) = max_rows {
                if *fetched >= max {
                    exhausted = true;
                    break;
                }
            }
            match stream.as_mut().next().await {
                Some(Ok(values)) => {
                    out.push(values);
                    *fetched += 1;
                }
                Some(Err(error)) => return Err(error),
                None => {
                    exhausted = true;
                    break;
                }
            }
        }
        Ok((out, exhausted))
    }

    /// Drops the borrowing stream half first, then returns the `Conn` it
    /// was borrowing to the shared slot — the exact order this module's
    /// doc comment requires, done explicitly rather than relying on field
    /// declaration order (which a later edit could silently reorder).
    fn release(&mut self) {
        self.stream = None;
        self.query_result = None;
        if let Some(conn) = self.conn.take() {
            *self.slot.lock().unwrap() = Some(conn);
        }
    }
}

impl Drop for MySqlRowStream {
    fn drop(&mut self) {
        self.release();
    }
}

impl RowStream for MySqlRowStream {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(None);
        };
        let (rows, exhausted) = crate::runtime().block_on(Self::pull_batch(
            stream,
            self.fetch_size,
            self.max_rows,
            &mut self.fetched,
        ))?;
        if exhausted {
            self.release();
        }
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(RowBatch {
            columns: self.columns.clone(),
            rows,
        }))
    }
}

fn column_meta(column: &Column) -> ColumnMeta {
    ColumnMeta {
        name: column.name_str().into_owned(),
        type_name: format!("{:?}", column.column_type()),
        nullable: !column
            .flags()
            .contains(mysql_async::consts::ColumnFlags::NOT_NULL_FLAG),
        origin: (!column.table_str().is_empty()).then(|| column.table_str().into_owned()),
    }
}

impl Connection for MySqlConnection {
    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }

    fn server_info(&self) -> String {
        "MySQL/MariaDB".to_string()
    }

    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        let rows: Vec<(String, String, String)> = self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.query_map(NAMES_QUERY, |(schema, name, kind)| (schema, name, kind)))
                .map_err(io_err)
        })?;

        let mut by_schema: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for (schema, name, kind) in rows {
            by_schema.entry(schema).or_default().push((name, kind));
        }

        let mut roots = Vec::with_capacity(by_schema.len());
        for (schema, objects) in by_schema {
            if let Some(filter) = &scope.schema {
                if filter != &schema {
                    continue;
                }
            }
            let mut children = Vec::with_capacity(objects.len());
            for (name, kind) in objects {
                if let Some(object_filter) = &scope.object {
                    if object_filter != &name {
                        continue;
                    }
                }
                let object_kind = if kind == "VIEW" {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                };
                let node = if level == IntrospectLevel::Names {
                    Node::leaf(name, object_kind)
                } else {
                    self.table_node(&schema, &name, object_kind, level)?
                };
                children.push(node);
            }
            roots.push(Node::with_children(schema, ObjectKind::Schema, children));
        }
        Ok(SchemaSnapshot::new(level, roots))
    }

    fn execute(
        &mut self,
        statement: &Statement,
        options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        let mut conn = self.take_conn()?;
        let params: Vec<mysql_async::Value> = statement.params.iter().map(to_mysql_value).collect();
        let text = statement.text.clone();

        let result = match crate::runtime()
            .block_on(async { conn.exec_iter(text, Params::Positional(params)).await })
        {
            Ok(result) => result,
            Err(error) => {
                self.return_conn(conn);
                return Err(io_err(error));
            }
        };

        let columns = result.columns();
        let Some(columns) = columns else {
            let affected = result.affected_rows();
            crate::runtime().block_on(async { result.drop_result().await.ok() });
            self.return_conn(conn);
            return Ok(Execution::Affected(affected));
        };
        let columns: Vec<ColumnMeta> = columns.iter().map(column_meta).collect();
        let raw_columns = columns.clone();

        // SAFETY: `result` borrows `&mut *conn`; `conn` is heap-boxed (a
        // stable address, exclusively ours — taken out of the shared slot
        // above, so nothing else can reach it) and moves, unmutated, into
        // `MySqlRowStream::conn` below, never before `MySqlRowStream::
        // release` (called on exhaustion and on `Drop`) has already
        // dropped both `stream` and `query_result` — the two borrowing
        // halves — first. See `MySqlRowStream`'s own field comments.
        let result: mysql_async::QueryResult<'static, 'static, mysql_async::BinaryProtocol> =
            unsafe { std::mem::transmute(result) };
        let mut result = Box::new(result);
        let result_ptr: *mut mysql_async::QueryResult<
            'static,
            'static,
            mysql_async::BinaryProtocol,
        > = &mut *result;
        // SAFETY: `result` is heap-allocated immediately above and never
        // moved again — only `Box<QueryResult>` (a pointer) moves from
        // here on, so the data `.stream()` is about to borrow stays put.
        let row_stream = crate::runtime()
            .block_on(unsafe { &mut *result_ptr }.stream::<mysql_async::Row>())
            .map_err(io_err)?;
        let Some(row_stream) = row_stream else {
            // No result set left to stream (already exhausted server-side)
            // — an empty `Rows` execution rather than a defect. `conn`
            // still moves into the stream (not back to the slot here):
            // `MySqlRowStream::release` (its `Drop`) is the one place
            // that returns it, whether or not a live `stream`/
            // `query_result` was ever attached.
            return Ok(Execution::Rows(Box::new(MySqlRowStream {
                columns,
                fetch_size: options.fetch_size.max(1) as usize,
                max_rows: options.max_rows,
                fetched: 0,
                stream: None,
                query_result: None,
                conn: Some(conn),
                slot: self.slot.clone(),
            })));
        };

        let mapped = row_stream.map(move |row_result| {
            row_result.map_err(io_err).map(|mut row: mysql_async::Row| {
                let mut values = Vec::with_capacity(raw_columns.len());
                for i in 0..raw_columns.len() {
                    let raw = row.take(i).unwrap_or(mysql_async::Value::NULL);
                    let column = row.columns_ref()[i].clone();
                    values.push(mysql_value_to_value(raw, &column));
                }
                values
            })
        });
        let boxed: Pin<Box<dyn Stream<Item = Result<Vec<Value>, DbError>> + Send + '_>> =
            Box::pin(mapped);
        // SAFETY: same argument as the `QueryResult` transmute above,
        // applied one level further in: this stream borrows `result`
        // (already pinned to a stable heap address), which in turn
        // borrows `conn` — both released, in order, by `release()`.
        let boxed: Pin<Box<dyn Stream<Item = Result<Vec<Value>, DbError>> + Send + 'static>> =
            unsafe { std::mem::transmute(boxed) };

        Ok(Execution::Rows(Box::new(MySqlRowStream {
            columns,
            fetch_size: options.fetch_size.max(1) as usize,
            max_rows: options.max_rows,
            fetched: 0,
            stream: Some(boxed),
            query_result: Some(result),
            conn: Some(conn),
            slot: self.slot.clone(),
        })))
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.query_drop("BEGIN"))
                .map_err(io_err)
        })
    }

    fn commit(&mut self) -> Result<(), DbError> {
        self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.query_drop("COMMIT"))
                .map_err(io_err)
        })
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.query_drop("ROLLBACK"))
                .map_err(io_err)
        })
    }

    fn set_read_only(&mut self, read_only: bool) -> Result<(), DbError> {
        let stmt = if read_only {
            "SET SESSION TRANSACTION READ ONLY"
        } else {
            "SET SESSION TRANSACTION READ WRITE"
        };
        self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.query_drop(stmt))
                .map_err(io_err)
        })
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(MySqlCancelHandle {
            connection_id: self.connection_id,
            opts: self.opts.clone(),
        }))
    }

    fn ddl_of(&mut self, object: &ObjectRef) -> Result<String, DbError> {
        let (statement, column_index) = show_create_statement_for(object);
        self.with_conn(|conn| {
            let row: Option<mysql_async::Row> = crate::runtime()
                .block_on(conn.query_first(statement))
                .map_err(io_err)?;
            let mut row = row.ok_or_else(|| {
                DbError::new(
                    DbErrorCode::NotSupported,
                    format!("no such table or view: {}", object.name),
                )
            })?;
            let ddl: String = row
                .take(column_index)
                .and_then(|value| mysql_async::from_value_opt::<String>(value).ok())
                .ok_or_else(|| io_err("SHOW CREATE returned no DDL text"))?;
            Ok(ddl)
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

/// [`MySqlConnection::ddl_of`]'s own `SHOW CREATE ...` statement/column-
/// index choice — pulled out (pure string building, no connection) so a
/// unit test can drive both the view and table/default branches.
fn show_create_statement_for(object: &ObjectRef) -> (String, usize) {
    let ident = Dialect::MySql.quote_ident(&object.name);
    match object.kind {
        Some(ObjectKind::View) => (format!("SHOW CREATE VIEW {ident}"), 1),
        _ => (format!("SHOW CREATE TABLE {ident}"), 1),
    }
}

impl MySqlConnection {
    /// One table/view's columns (F2.1's `Columns`/`Full` levels), fetched
    /// through [`COLUMNS_QUERY`] — the per-object round trip `introspect`
    /// only pays for the objects a scope actually narrows to.
    fn table_node(
        &self,
        schema: &str,
        name: &str,
        kind: ObjectKind,
        level: IntrospectLevel,
    ) -> Result<Node, DbError> {
        let rows: Vec<ColumnRow> = self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.exec_map(
                    COLUMNS_QUERY,
                    (schema, name),
                    |(
                        column_name,
                        column_type,
                        is_nullable,
                        column_key,
                        extra,
                        column_default,
                        comment,
                    )| {
                        (
                            column_name,
                            column_type,
                            is_nullable,
                            column_key,
                            extra,
                            column_default,
                            comment,
                        )
                    },
                ))
                .map_err(io_err)
        })?;
        let mut children: Vec<Node> = columns_to_nodes(rows);
        if level == IntrospectLevel::Full {
            children.extend(self.index_nodes(schema, name)?);
            children.extend(self.constraint_nodes(schema, name)?);
        }
        Ok(Node::with_children(name, kind, children))
    }

    fn index_nodes(&self, schema: &str, name: &str) -> Result<Vec<Node>, DbError> {
        let rows: Vec<(String, bool, Option<String>, String)> = self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.exec_map(
                    INDEXES_QUERY,
                    (schema, name),
                    |(index_name, is_unique, method, columns): (
                        String,
                        i64,
                        Option<String>,
                        String,
                    )| { (index_name, is_unique != 0, method, columns) },
                ))
                .map_err(io_err)
        })?;
        Ok(index_rows_to_nodes(rows))
    }

    /// `IntrospectLevel::Full`'s own constraints, via [`CONSTRAINTS_QUERY`]
    /// (PK/unique/FK) plus [`CHECKS_QUERY`] (CHECK, a separate round trip —
    /// see that query's own comment).
    fn constraint_nodes(&self, schema: &str, name: &str) -> Result<Vec<Node>, DbError> {
        let rows: Vec<ConstraintRow> = self.with_conn(|conn| {
            crate::runtime()
                .block_on(
                    conn.exec_map(CONSTRAINTS_QUERY, (schema, name), |row: ConstraintRow| row),
                )
                .map_err(io_err)
        })?;
        let mut nodes = constraint_rows_to_nodes(rows);

        let checks: Vec<(String, String)> = self.with_conn(|conn| {
            crate::runtime()
                .block_on(conn.exec_map(CHECKS_QUERY, (schema, name), |row| row))
                .map_err(io_err)
        })?;
        nodes.extend(check_rows_to_nodes(checks));
        Ok(nodes)
    }
}

/// One `mysql_constraints.sql` row, before it becomes a `Node` — a module
/// level alias (rather than local to [`MySqlConnection::constraint_nodes`])
/// so [`constraint_rows_to_nodes`] can share it with a unit test that feeds
/// hand-built rows, no server needed.
type ConstraintRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// [`MySqlConnection::table_node`]'s own row-to-`Node` mapping, pulled out
/// so a unit test can feed it hand-built [`ColumnRow`]s without a live
/// `Conn` — the same "separate the query from the row shape it feeds"
/// split this module already applies to its `Value` conversions.
fn columns_to_nodes(rows: Vec<ColumnRow>) -> Vec<Node> {
    rows.into_iter()
        .map(
            |(column_name, column_type, is_nullable, column_key, extra, default, comment)| {
                Node::leaf(column_name, ObjectKind::Column).with_detail(NodeDetail {
                    type_name: Some(column_type),
                    nullable: Some(is_nullable == "YES"),
                    default,
                    primary_key: column_key == "PRI",
                    auto_increment: Some(extra.contains("auto_increment")),
                    comment: (!comment.is_empty()).then_some(comment),
                    ..NodeDetail::default()
                })
            },
        )
        .collect()
}

/// [`MySqlConnection::index_nodes`]'s own row-to-`Node` mapping.
fn index_rows_to_nodes(rows: Vec<(String, bool, Option<String>, String)>) -> Vec<Node> {
    rows.into_iter()
        .map(|(index_name, unique, method, columns)| {
            Node::leaf(index_name, ObjectKind::Index).with_detail(NodeDetail {
                index: Some(IndexDetail {
                    columns: columns.split(',').map(str::to_string).collect(),
                    unique,
                    method,
                }),
                ..NodeDetail::default()
            })
        })
        .collect()
}

/// [`MySqlConnection::constraint_nodes`]'s own PK/unique/FK row-to-`Node`
/// mapping — `CHECK` constraints are a separate query/row shape, see
/// [`check_rows_to_nodes`].
fn constraint_rows_to_nodes(rows: Vec<ConstraintRow>) -> Vec<Node> {
    let mut nodes = Vec::new();
    for (
        constraint_name,
        constraint_type,
        columns,
        ref_schema,
        ref_table,
        ref_columns,
        on_update,
        on_delete,
    ) in rows
    {
        let columns: Vec<String> = columns.split(',').map(str::to_string).collect();
        let kind = match constraint_type.as_str() {
            "PRIMARY KEY" => ConstraintKind::PrimaryKey { columns },
            "UNIQUE" => ConstraintKind::Unique { columns },
            "FOREIGN KEY" => {
                let mut reference =
                    ObjectRef::new(ref_table.unwrap_or_default()).with_kind(ObjectKind::Table);
                if let Some(ref_schema) = ref_schema {
                    reference = reference.with_schema(ref_schema);
                }
                ConstraintKind::ForeignKey {
                    columns,
                    ref_table: reference,
                    ref_columns: ref_columns
                        .map(|cols| cols.split(',').map(str::to_string).collect())
                        .unwrap_or_default(),
                    on_delete: on_delete.filter(|rule| rule != "NO ACTION"),
                    on_update: on_update.filter(|rule| rule != "NO ACTION"),
                }
            }
            _ => continue,
        };
        nodes.push(
            Node::leaf(constraint_name, ObjectKind::Constraint).with_detail(NodeDetail {
                constraint: Some(kind),
                ..NodeDetail::default()
            }),
        );
    }
    nodes
}

/// [`MySqlConnection::constraint_nodes`]'s own `CHECK`-constraint row-to-
/// `Node` mapping (a separate query from the PK/unique/FK one above).
fn check_rows_to_nodes(checks: Vec<(String, String)>) -> Vec<Node> {
    checks
        .into_iter()
        .map(|(constraint_name, check_clause)| {
            Node::leaf(constraint_name, ObjectKind::Constraint).with_detail(NodeDetail {
                constraint: Some(ConstraintKind::Check { expr: check_clause }),
                ..NodeDetail::default()
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "mysql_tests.rs"]
mod tests;
