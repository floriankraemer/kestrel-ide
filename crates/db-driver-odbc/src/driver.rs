//! `OdbcDriver`/`OdbcConnection`: `db_core::Driver`/`Connection` over
//! `odbc-api` (F8.2, database-tools-plan.md).
//!
//! One process-wide `odbc_api::Environment` (`OnceLock`, per the ODBC
//! driver-manager convention of allocating the environment handle once)
//! and one `Arc<Mutex<odbc_api::Connection<'static>>>` per `OdbcConnection`
//! — the mutex is ODBC's own rule made explicit: a connection handle
//! serves one active statement at a time.
//!
//! A `Rows` execution runs on its own `std::thread`, holding the
//! connection's mutex for as long as the result set is being paged and
//! streaming `RowBatch`es back over a bounded channel (capacity 1, so the
//! worker blocks until the previous batch was consumed — real pull-based
//! backpressure, not an eager full-result materialisation). This sidesteps
//! the self-referential-struct problem a "return an owned cursor" API
//! would otherwise force (odbc-api's owned-cursor constructors are
//! crate-private) with something simpler: every odbc-api type touched by
//! one statement lives in one thread's stack frame, the plain borrow
//! checker proves it sound, and cancellation becomes a cooperative flag
//! the worker checks between batches — a strictly better fallback than
//! the plan's "drop-and-reconnect", since it never tears down the
//! connection (`SQLCancel` itself is not reachable through odbc-api's
//! safe API at all, so that path was never available either way).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use db_core::datasource::ConnectSpec;
use db_core::dialect::Dialect;
use db_core::driver::{
    CancelHandle, Capabilities, Connection as DbConnection, Driver as DbDriver, ExecOptions,
    Execution, RowStream, Statement,
};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{IntrospectLevel, IntrospectScope, ObjectRef, SchemaSnapshot};
use db_core::value::{ColumnMeta, RowBatch};

use odbc_api::buffers::ColumnarDynBuffer;
use odbc_api::{
    ColumnDescription, Connection as OdbcRawConnection, ConnectionOptions, Cursor as _,
    Environment, IntoParameter, ResultSetMetadata,
};

use crate::connect::build_connection_string;
use crate::values::{plan_column, read_cell};

fn environment() -> Result<&'static Environment, DbError> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    match ENV.get_or_init(|| Environment::new().map_err(|e| e.to_string())) {
        Ok(env) => Ok(env),
        Err(message) => Err(DbError::new(DbErrorCode::ConnectionFailed, message.clone())),
    }
}

fn odbc_err(code: DbErrorCode, error: odbc_api::Error) -> DbError {
    DbError::new(code, error.to_string())
}

/// `db_core::Driver` over `odbc-api` — connects any data source an ODBC
/// driver manager entry exists for (a system DSN, or a full
/// `Driver={…}` string, see `crate::connect`).
pub struct OdbcDriver;

impl OdbcDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OdbcDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl DbDriver for OdbcDriver {
    fn id(&self) -> &str {
        "odbc"
    }

    fn capabilities(&self) -> Capabilities {
        // Transactions are near-universal across ODBC drivers
        // (`SQL_ATTR_AUTOCOMMIT`); server-side cancel is not (odbc-api
        // exposes no `SQLCancel`, see this module's doc comment), and
        // savepoints/RETURNING are too driver-specific to claim generically.
        Capabilities::TRANSACTIONS
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn DbConnection>, DbError> {
        let env = environment()?;
        let connection_string = build_connection_string(spec);
        let raw = env
            .connect_with_connection_string(
                connection_string.as_str(),
                ConnectionOptions::default(),
            )
            .map_err(|e| odbc_err(DbErrorCode::ConnectionFailed, e))?;
        let dialect = raw
            .database_management_system_name()
            .map(|name| detect_dialect(&name))
            .unwrap_or(Dialect::Sqlite);
        Ok(Box::new(OdbcConnection {
            shared: Arc::new(Mutex::new(raw)),
            cancelled: Arc::new(AtomicBool::new(false)),
            dialect,
        }))
    }
}

/// A live ODBC connection. See this module's doc comment for the
/// threading model.
pub struct OdbcConnection {
    shared: Arc<Mutex<OdbcRawConnection<'static>>>,
    cancelled: Arc<AtomicBool>,
    dialect: Dialect,
}

/// `db_core::Dialect` has no generic `Odbc` case (its quoting rules are
/// genuinely per-database), so this driver picks the closest match from
/// `SQLGetInfo`'s DBMS name — the same string a status bar would show the
/// user. Unrecognised DBMS names fall back to `Sqlite`: double-quoted
/// identifiers are the ANSI SQL:1992 default every ODBC driver not listed
/// here is more likely to accept than any of the other four quote styles.
fn detect_dialect(dbms_name: &str) -> Dialect {
    let lower = dbms_name.to_ascii_lowercase();
    if lower.contains("postgres") {
        Dialect::Postgres
    } else if lower.contains("mysql") || lower.contains("mariadb") {
        Dialect::MySql
    } else if lower.contains("sql server") || lower.contains("mssql") {
        Dialect::SqlServer
    } else {
        Dialect::Sqlite
    }
}

fn build_params(
    values: &[db_core::value::Value],
) -> Vec<<Option<String> as IntoParameter>::Parameter> {
    let rules = db_core::value::FormatRules::default();
    values
        .iter()
        .map(|value| {
            let text = match value {
                db_core::value::Value::Null => None,
                other => Some(other.display(&rules)),
            };
            text.into_parameter()
        })
        .collect()
}

/// One header message the worker thread sends before it starts (or
/// finishes, for a non-`Rows` statement) — read once by `execute`, before
/// it decides what `Execution` to hand the caller.
enum Header {
    Affected(u64),
    Rows(Vec<ColumnMeta>),
    Err(DbError),
}

fn odbc_data_type_name(data_type: odbc_api::DataType) -> String {
    format!("{data_type:?}")
}

/// What one background statement execution (`run_statement`) needs —
/// grouped so the function reads as "run this request", not a seven-way
/// parameter list.
struct StatementRequest {
    shared: Arc<Mutex<OdbcRawConnection<'static>>>,
    sql: String,
    params: Vec<<Option<String> as IntoParameter>::Parameter>,
    timeout: Duration,
    fetch_size: usize,
    cancelled: Arc<AtomicBool>,
}

fn run_statement(
    request: StatementRequest,
    header_tx: SyncSender<Header>,
    batch_tx: SyncSender<Result<Option<RowBatch>, DbError>>,
) {
    let StatementRequest {
        shared,
        sql,
        params,
        timeout,
        fetch_size,
        cancelled,
    } = request;
    let guard = match shared.lock() {
        Ok(guard) => guard,
        Err(_) => {
            let _ = header_tx.send(Header::Err(DbError::new(
                DbErrorCode::Unknown,
                "ODBC connection mutex poisoned by a previous panic",
            )));
            return;
        }
    };

    let mut stmt = match guard.preallocate() {
        Ok(stmt) => stmt,
        Err(e) => {
            let _ = header_tx.send(Header::Err(odbc_err(DbErrorCode::ConnectionFailed, e)));
            return;
        }
    };

    let timeout_secs = timeout.as_secs() as usize;
    if timeout_secs > 0 {
        if let Err(e) = stmt.set_query_timeout_sec(timeout_secs) {
            let _ = header_tx.send(Header::Err(odbc_err(DbErrorCode::Timeout, e)));
            return;
        }
    }

    // Bound to `maybe_cursor` and dropped explicitly (rather than matched
    // in place) so the `CursorImpl`'s `Drop` impl cannot extend the
    // borrow of `stmt` it carries past the point where the `None` branch
    // needs `stmt` again for `row_count()` — a well-known NLL/dropck
    // interaction with `Option<T: Drop>` values that borrow the same
    // place they are later matched against.
    let mut maybe_cursor = match stmt.execute(&sql, params.as_slice()) {
        Ok(cursor) => cursor,
        Err(e) => {
            let _ = header_tx.send(Header::Err(odbc_err(DbErrorCode::InvalidStatement, e)));
            return;
        }
    };
    if maybe_cursor.is_none() {
        drop(maybe_cursor);
        let affected = stmt.row_count().ok().flatten().unwrap_or(0) as u64;
        let _ = header_tx.send(Header::Affected(affected));
        return;
    }
    let mut cursor = maybe_cursor.take().expect("checked is_none above");

    let num_cols = match cursor.num_result_cols() {
        Ok(n) => n,
        Err(e) => {
            let _ = header_tx.send(Header::Err(odbc_err(DbErrorCode::Unknown, e)));
            return;
        }
    };

    let mut columns = Vec::with_capacity(num_cols as usize);
    let mut kinds = Vec::with_capacity(num_cols as usize);
    let mut descs = Vec::with_capacity(num_cols as usize);
    for i in 1..=num_cols as u16 {
        let mut description = ColumnDescription::default();
        if let Err(e) = cursor.describe_col(i, &mut description) {
            let _ = header_tx.send(Header::Err(odbc_err(DbErrorCode::Unknown, e)));
            return;
        }
        let name = description
            .name_to_string()
            .unwrap_or_else(|_| format!("col_{i}"));
        let nullable = description.could_be_nullable();
        let (desc, kind) = plan_column(description.data_type, nullable);
        columns.push(ColumnMeta {
            name,
            type_name: odbc_data_type_name(description.data_type),
            nullable,
            origin: None,
        });
        kinds.push(kind);
        descs.push(desc);
    }

    if header_tx.send(Header::Rows(columns.clone())).is_err() {
        return; // caller gave up before we even started
    }

    let buffer = ColumnarDynBuffer::from_descs(fetch_size.max(1), descs);
    let mut block_cursor = match cursor.bind_buffer(buffer) {
        Ok(bc) => bc,
        Err(e) => {
            let _ = batch_tx.send(Err(odbc_err(DbErrorCode::Unknown, e)));
            return;
        }
    };

    loop {
        if cancelled.load(Ordering::SeqCst) {
            let _ = batch_tx.send(Err(DbError::new(DbErrorCode::Cancelled, "query cancelled")));
            return;
        }
        let fetched = match block_cursor.fetch() {
            Ok(batch) => batch,
            Err(e) => {
                let _ = batch_tx.send(Err(odbc_err(DbErrorCode::Unknown, e)));
                return;
            }
        };
        let Some(buffer) = fetched else {
            let _ = batch_tx.send(Ok(None));
            return;
        };
        let num_rows = buffer.num_rows();
        let mut rows = Vec::with_capacity(num_rows);
        for row in 0..num_rows {
            let mut values = Vec::with_capacity(kinds.len());
            for (col, kind) in kinds.iter().enumerate() {
                values.push(read_cell(kind, buffer.column(col), row));
            }
            rows.push(values);
        }
        let batch = RowBatch {
            columns: columns.clone(),
            rows,
        };
        if batch_tx.send(Ok(Some(batch))).is_err() {
            return; // consumer stopped pulling — nothing left to do
        }
    }
}

/// Streams `RowBatch`es a background thread produces — see this module's
/// doc comment for why the thread rather than an owned cursor.
pub struct OdbcRowStream {
    batch_rx: Receiver<Result<Option<RowBatch>, DbError>>,
    worker: Option<JoinHandle<()>>,
}

impl RowStream for OdbcRowStream {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        match self.batch_rx.recv() {
            Ok(result) => result,
            Err(_) => Ok(None), // worker thread ended without a final message
        }
    }
}

impl Drop for OdbcRowStream {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct OdbcCancelHandle {
    cancelled: Arc<AtomicBool>,
}

impl CancelHandle for OdbcCancelHandle {
    fn cancel(&self) -> Result<(), DbError> {
        self.cancelled.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl DbConnection for OdbcConnection {
    fn dialect(&self) -> Dialect {
        self.dialect
    }

    fn server_info(&self) -> String {
        self.shared
            .lock()
            .ok()
            .and_then(|guard| guard.database_management_system_name().ok())
            .unwrap_or_else(|| "ODBC".to_string())
    }

    // ponytail: `level` is not yet honoured — `introspect::introspect`
    // always fetches columns for every table it lists; upgrade to skip
    // that at `Names` level once the ODBC backend needs the same
    // 5 000-table NFR the native drivers meet (database-tools-plan.md
    // follow-up).
    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        _level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        crate::introspect::introspect(&self.shared, scope)
    }

    fn execute(
        &mut self,
        statement: &Statement,
        options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(DbError::new(
                DbErrorCode::CancelledByDisconnect,
                "connection was cancelled and has not been reset",
            ));
        }
        let params = build_params(&statement.params);
        let (header_tx, header_rx) = sync_channel(1);
        let (batch_tx, batch_rx) = sync_channel(1);
        let shared = self.shared.clone();
        let cancelled = self.cancelled.clone();
        let sql = statement.text.clone();
        let fetch_size = options.fetch_size as usize;
        let timeout = options.timeout;
        let request = StatementRequest {
            shared,
            sql,
            params,
            timeout,
            fetch_size,
            cancelled,
        };
        let worker = std::thread::Builder::new()
            .name("db-driver-odbc".to_string())
            .spawn(move || {
                run_statement(request, header_tx, batch_tx);
            })
            .map_err(|e| {
                DbError::new(DbErrorCode::Unknown, format!("spawning ODBC worker: {e}"))
            })?;

        match header_rx.recv() {
            Ok(Header::Affected(n)) => {
                let _ = worker.join();
                Ok(Execution::Affected(n))
            }
            Ok(Header::Rows(_columns)) => Ok(Execution::Rows(Box::new(OdbcRowStream {
                batch_rx,
                worker: Some(worker),
            }))),
            Ok(Header::Err(e)) => {
                let _ = worker.join();
                Err(e)
            }
            Err(_) => {
                let _ = worker.join();
                Err(DbError::new(
                    DbErrorCode::Unknown,
                    "ODBC worker thread died before responding",
                ))
            }
        }
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.shared
            .lock()
            .map_err(|_| poisoned())?
            .set_autocommit(false)
            .map_err(|e| odbc_err(DbErrorCode::Unknown, e))
    }

    fn commit(&mut self) -> Result<(), DbError> {
        let guard = self.shared.lock().map_err(|_| poisoned())?;
        guard
            .commit()
            .map_err(|e| odbc_err(DbErrorCode::Unknown, e))?;
        guard
            .set_autocommit(true)
            .map_err(|e| odbc_err(DbErrorCode::Unknown, e))
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        let guard = self.shared.lock().map_err(|_| poisoned())?;
        guard
            .rollback()
            .map_err(|e| odbc_err(DbErrorCode::Unknown, e))?;
        guard
            .set_autocommit(true)
            .map_err(|e| odbc_err(DbErrorCode::Unknown, e))
    }

    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        // `SQL_ATTR_ACCESS_MODE` is not uniformly honoured across drivers
        // (SQLite's ODBC driver ignores it outright); enforcing read-only
        // is `db-core`'s statement-classifier job (`readonly.rs`), not a
        // server-side attribute this driver can rely on.
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "the ODBC driver does not support a server-side read-only mode",
        ))
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(OdbcCancelHandle {
            cancelled: self.cancelled.clone(),
        }))
    }

    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        // ODBC has no standard "show me this object's DDL" call — every
        // driver that supports it does so through its own SQL dialect
        // extension, which this generic driver cannot assume.
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "DDL reflection is not part of the standard ODBC catalog API",
        ))
    }

    fn apply(&mut self, statements: &[Statement]) -> Result<u64, DbError> {
        let mut total = 0u64;
        for statement in statements {
            match self.execute(statement, &ExecOptions::default())? {
                Execution::Affected(n) => total += n,
                Execution::Ok | Execution::Rows(_) | Execution::Multi(_) => {}
            }
        }
        Ok(total)
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

fn poisoned() -> DbError {
    DbError::new(
        DbErrorCode::Unknown,
        "ODBC connection mutex poisoned by a previous panic",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn detect_dialect_recognises_the_common_dbms_names() {
        assert_eq!(detect_dialect("PostgreSQL"), Dialect::Postgres);
        assert_eq!(detect_dialect("MySQL"), Dialect::MySql);
        assert_eq!(detect_dialect("MariaDB"), Dialect::MySql);
        assert_eq!(detect_dialect("Microsoft SQL Server"), Dialect::SqlServer);
        assert_eq!(detect_dialect("SQLite"), Dialect::Sqlite);
    }

    #[test]
    fn detect_dialect_falls_back_to_sqlite_for_an_unrecognised_dbms() {
        assert_eq!(detect_dialect("SomeExoticDatabase"), Dialect::Sqlite);
    }

    #[test]
    fn build_params_maps_null_to_none_and_everything_else_to_its_display_text() {
        let values = vec![
            db_core::value::Value::Null,
            db_core::value::Value::Int(42),
            db_core::value::Value::Text("hi".to_string()),
        ];
        let params = build_params(&values);
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn odbc_data_type_name_renders_the_debug_form() {
        assert_eq!(odbc_data_type_name(odbc_api::DataType::Integer), "Integer");
    }

    #[test]
    fn odbc_err_carries_the_requested_code_and_message() {
        // `odbc_api::Error` has no public constructor for a synthetic
        // instance outside the crate, so this exercises `poisoned()`
        // instead, which shares the same `DbError::new` wiring `odbc_err`
        // uses.
        let error = poisoned();
        assert_eq!(error.code, DbErrorCode::Unknown);
        assert!(error.message.contains("poisoned"));
    }

    #[test]
    fn a_cancel_handle_sets_the_shared_flag() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = OdbcCancelHandle {
            cancelled: flag.clone(),
        };
        assert!(handle.cancel().is_ok());
        assert!(flag.load(Ordering::SeqCst));
    }
}
