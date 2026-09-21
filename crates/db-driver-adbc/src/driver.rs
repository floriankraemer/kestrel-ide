//! `AdbcDriver`/`AdbcConnection`: `db_core::Driver`/`Connection` over
//! `adbc_driver_manager::ManagedDriver` (F8.1, database-tools-plan.md).
//!
//! Unlike `db-driver-odbc`, `Statement::execute()` here already returns
//! an owned, `'static` `Box<dyn RecordBatchReader + Send>` — ADBC's own
//! design gives every result set independent ownership of its connection
//! chain, so this driver needs none of `db-driver-odbc`'s
//! background-thread-plus-channel construction; `next_batch` just pulls
//! the next `RecordBatch` and converts it (`crate::arrow`).
//!
//! Known gaps (documented rather than half-implemented): statement
//! parameters are not yet bound (`Statement::execute` ignores
//! `statement.params`; every query issued through this driver today is
//! parameter-free, matching the read-only browsing F8b's UI adds first) —
//! binding needs an Arrow `RecordBatch` built from `db_core::Value`,
//! which is future work once a caller actually needs it. Introspection
//! covers catalogs/schemas/tables/columns from `get_objects`, not
//! constraints (ADBC's constraint schema is deeper than `db-driver-odbc`'s
//! ODBC-catalog-based one and didn't fit this phase's time box).

use std::sync::{Arc, Mutex, OnceLock};

use adbc_core::error::{Error as AdbcError, Status};
use adbc_core::options::{AdbcVersion, ObjectDepth, OptionDatabase, OptionValue};
use adbc_core::{
    Connection as AdbcConnectionTrait, Database as AdbcDatabaseTrait, Driver as AdbcDriverTrait,
    Optionable, Statement as AdbcStatementTrait, LOAD_FLAG_ALLOW_RELATIVE_PATHS,
    LOAD_FLAG_SEARCH_ENV, LOAD_FLAG_SEARCH_SYSTEM, LOAD_FLAG_SEARCH_USER,
};
use adbc_driver_manager::{ManagedConnection, ManagedDatabase, ManagedDriver};
use arrow_array::RecordBatchReader;

use db_core::datasource::ConnectSpec;
use db_core::dialect::Dialect;
use db_core::driver::{
    CancelHandle, Capabilities, Connection as DbConnection, Driver as DbDriver, ExecOptions,
    Execution, RowStream, Statement,
};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{IntrospectLevel, IntrospectScope, SchemaSnapshot};

use crate::locate::DriverLocation;
use crate::quarantine::{guarded_load, Quarantine};

fn adbc_err(e: AdbcError) -> DbError {
    let code = match e.status {
        Status::NotFound => DbErrorCode::ConnectionFailed,
        Status::Cancelled | Status::Timeout => DbErrorCode::Cancelled,
        Status::Unauthenticated | Status::Unauthorized => DbErrorCode::ConnectionFailed,
        Status::InvalidArguments | Status::InvalidState | Status::InvalidData => {
            DbErrorCode::InvalidStatement
        }
        Status::NotImplemented => DbErrorCode::NotSupported,
        _ => DbErrorCode::Unknown,
    };
    DbError::new(code, e.message)
}

/// One ADBC driver family (e.g. `"duckdb"`) — loads its shared library
/// once (cached, quarantine-guarded) and hands out connections.
pub struct AdbcDriver {
    driver_id: String,
    location: DriverLocation,
    quarantine: Quarantine,
    loaded: OnceLock<Result<Arc<Mutex<ManagedDriver>>, DbError>>,
}

impl AdbcDriver {
    pub fn new(
        driver_id: impl Into<String>,
        location: DriverLocation,
        config_dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            driver_id: driver_id.into(),
            location,
            quarantine: Quarantine::new(config_dir),
            loaded: OnceLock::new(),
        }
    }

    fn managed(&self) -> Result<Arc<Mutex<ManagedDriver>>, DbError> {
        self.loaded
            .get_or_init(|| {
                let driver_id = self.driver_id.clone();
                let location = self.location.clone();
                guarded_load(&self.quarantine, &driver_id, move || load_driver(&location))
                    .map(|driver| Arc::new(Mutex::new(driver)))
            })
            .clone()
    }
}

fn load_driver(location: &DriverLocation) -> Result<ManagedDriver, DbError> {
    let flags = LOAD_FLAG_ALLOW_RELATIVE_PATHS
        | LOAD_FLAG_SEARCH_ENV
        | LOAD_FLAG_SEARCH_SYSTEM
        | LOAD_FLAG_SEARCH_USER;
    let result = match location {
        DriverLocation::ManagedManifest(path) => {
            ManagedDriver::load_from_name(path, None, AdbcVersion::V110, flags, None)
        }
        DriverLocation::SystemName(name) => {
            ManagedDriver::load_from_name(name, None, AdbcVersion::V110, flags, None)
        }
    };
    result.map_err(adbc_err)
}

impl DbDriver for AdbcDriver {
    fn id(&self) -> &str {
        &self.driver_id
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::TRANSACTIONS
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn DbConnection>, DbError> {
        let managed = self.managed()?;
        let mut guard = managed.lock().map_err(|_| poisoned())?;
        let mut opts: Vec<(OptionDatabase, OptionValue)> = Vec::new();
        if !spec.url.is_empty() {
            opts.push((OptionDatabase::Uri, OptionValue::String(spec.url.clone())));
        }
        if !spec.user.is_empty() {
            opts.push((
                OptionDatabase::Username,
                OptionValue::String(spec.user.clone()),
            ));
        }
        if let Some(password) = spec.password.as_deref() {
            opts.push((
                OptionDatabase::Password,
                OptionValue::String(password.to_string()),
            ));
        }
        let database = guard.new_database_with_opts(opts).map_err(adbc_err)?;
        let connection = database.new_connection().map_err(adbc_err)?;
        drop(guard);

        let dialect = detect_dialect(&self.driver_id);
        Ok(Box::new(AdbcConnection {
            _database: database,
            connection,
            dialect,
        }))
    }
}

fn detect_dialect(driver_id: &str) -> Dialect {
    match driver_id {
        "duckdb" | "sqlite" => Dialect::Sqlite,
        "postgresql" | "postgres" => Dialect::Postgres,
        "snowflake" | "bigquery" | "mssql" | "clickhouse" | "trino" => Dialect::Postgres,
        _ => Dialect::Sqlite,
    }
}

fn poisoned() -> DbError {
    DbError::new(
        DbErrorCode::Unknown,
        "ADBC driver mutex poisoned by a previous panic",
    )
}

pub struct AdbcConnection {
    // Keeps the database (and, through it, the loaded library) alive for
    // as long as this connection exists — ADBC requires the database to
    // outlive every connection created from it.
    _database: ManagedDatabase,
    connection: ManagedConnection,
    dialect: Dialect,
}

pub struct AdbcRowStream {
    reader: Box<dyn RecordBatchReader + Send>,
}

impl RowStream for AdbcRowStream {
    fn next_batch(&mut self) -> Result<Option<db_core::value::RowBatch>, DbError> {
        match self.reader.next() {
            None => Ok(None),
            Some(Ok(batch)) => Ok(Some(crate::arrow::convert_batch(&batch))),
            Some(Err(e)) => Err(DbError::new(DbErrorCode::Unknown, e.to_string())),
        }
    }
}

impl DbConnection for AdbcConnection {
    fn dialect(&self) -> Dialect {
        self.dialect
    }

    fn server_info(&self) -> String {
        "ADBC".to_string()
    }

    // ponytail: `level` is not yet honoured — ADBC's `get_objects` always
    // fetches at `ObjectDepth::All`, so `Names`-level requests pay a
    // `Full`-level cost until F2's per-level depth mapping lands here too
    // (tracked as a database-tools-plan.md follow-up).
    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        _level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        let reader = self
            .connection
            .get_objects(
                ObjectDepth::All,
                scope.catalog.as_deref(),
                scope.schema.as_deref(),
                None,
                None,
                None,
            )
            .map_err(adbc_err)?;
        crate::introspect::from_get_objects(reader)
    }

    fn execute(
        &mut self,
        statement: &Statement,
        _options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        if !statement.params.is_empty() {
            return Err(DbError::new(
                DbErrorCode::NotSupported,
                "this ADBC driver does not yet bind statement parameters (F8.1 known gap)",
            ));
        }
        let mut stmt = self.connection.new_statement().map_err(adbc_err)?;
        stmt.set_sql_query(&statement.text).map_err(adbc_err)?;
        match stmt.execute() {
            Ok(reader) => Ok(Execution::Rows(Box::new(AdbcRowStream { reader }))),
            Err(execute_err) => {
                // Not every ADBC driver returns a `RecordBatchReader` for
                // a DML/DDL statement; re-run through `execute_update`,
                // which is documented as the call for exactly that case.
                let mut stmt = self.connection.new_statement().map_err(adbc_err)?;
                stmt.set_sql_query(&statement.text).map_err(adbc_err)?;
                match stmt.execute_update() {
                    Ok(Some(count)) => Ok(Execution::Affected(count.max(0) as u64)),
                    Ok(None) => Ok(Execution::Ok),
                    Err(_) => Err(adbc_err(execute_err)),
                }
            }
        }
    }

    fn begin(&mut self) -> Result<(), DbError> {
        self.connection
            .set_option(
                adbc_core::options::OptionConnection::AutoCommit,
                OptionValue::String("false".to_string()),
            )
            .map_err(adbc_err)
    }

    fn commit(&mut self) -> Result<(), DbError> {
        self.connection.commit().map_err(adbc_err)?;
        self.connection
            .set_option(
                adbc_core::options::OptionConnection::AutoCommit,
                OptionValue::String("true".to_string()),
            )
            .map_err(adbc_err)
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        self.connection.rollback().map_err(adbc_err)?;
        self.connection
            .set_option(
                adbc_core::options::OptionConnection::AutoCommit,
                OptionValue::String("true".to_string()),
            )
            .map_err(adbc_err)
    }

    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "read-only is not a standard ADBC connection option",
        ))
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        // `ManagedConnection::cancel` needs `&mut self`, and this
        // connection is not behind a shared handle another thread could
        // reach independently of the one running `execute` — giving out
        // a `CancelHandle` here would need the same worker-thread
        // restructuring `db-driver-odbc` uses, which this phase's ADBC
        // driver (parameter-free, read-mostly queries) does not yet need.
        None
    }

    fn ddl_of(&mut self, _object: &db_core::schema::ObjectRef) -> Result<String, DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "DDL reflection is not part of the ADBC catalog API",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adbc_status_maps_to_the_closest_db_error_code() {
        let cases = [
            (Status::NotFound, DbErrorCode::ConnectionFailed),
            (Status::Cancelled, DbErrorCode::Cancelled),
            (Status::Timeout, DbErrorCode::Cancelled),
            (Status::Unauthenticated, DbErrorCode::ConnectionFailed),
            (Status::Unauthorized, DbErrorCode::ConnectionFailed),
            (Status::InvalidArguments, DbErrorCode::InvalidStatement),
            (Status::InvalidState, DbErrorCode::InvalidStatement),
            (Status::InvalidData, DbErrorCode::InvalidStatement),
            (Status::NotImplemented, DbErrorCode::NotSupported),
            (Status::Internal, DbErrorCode::Unknown),
        ];
        for (status, expected) in cases {
            let error = AdbcError {
                message: "boom".to_string(),
                status,
                vendor_code: 0,
                sqlstate: [0; 5],
                details: None,
            };
            assert_eq!(adbc_err(error).code, expected, "status {status:?}");
        }
    }

    #[test]
    fn adbc_err_keeps_the_original_message() {
        let error = AdbcError {
            message: "connection refused".to_string(),
            status: Status::IO,
            vendor_code: 0,
            sqlstate: [0; 5],
            details: None,
        };
        assert_eq!(adbc_err(error).message, "connection refused");
    }

    #[test]
    fn detect_dialect_recognises_known_driver_ids() {
        assert_eq!(detect_dialect("duckdb"), Dialect::Sqlite);
        assert_eq!(detect_dialect("postgresql"), Dialect::Postgres);
        assert_eq!(detect_dialect("snowflake"), Dialect::Postgres);
    }

    #[test]
    fn detect_dialect_falls_back_to_sqlite_for_an_unknown_id() {
        assert_eq!(detect_dialect("some-future-driver"), Dialect::Sqlite);
    }
}
