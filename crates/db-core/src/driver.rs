//! The seam every backend implements (ADR-0058 §1): `Driver` produces a
//! `Connection`, `Connection` is entirely blocking (`execute` returns once
//! the first batch, or the whole non-`Rows` result, is ready), and
//! `RowStream` pages further batches on demand. `db-core` describes the
//! shape only — nothing here runs anything.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::datasource::ConnectSpec;
use crate::dialect::Dialect;
use crate::error::DbError;
use crate::schema::{IntrospectLevel, IntrospectScope, ObjectRef, SchemaSnapshot};
use crate::value::{RowBatch, Value};

/// A bitflag set of what a backend's connection can do — a consumer checks
/// this before offering a UI affordance the backend cannot honour, the
/// same "an unsupported capability is absent" rule `dap-core` applies to a
/// debug adapter's undeclared capabilities.
///
/// Hand-rolled rather than the `bitflags` crate: `db-core` needs at most a
/// handful of flags and gains nothing from the macro's ceremony that a
/// dozen lines of `const`s and two trait impls don't already give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities(u32);

impl Capabilities {
    pub const NONE: Capabilities = Capabilities(0);
    pub const TRANSACTIONS: Capabilities = Capabilities(1 << 0);
    pub const RETURNING: Capabilities = Capabilities(1 << 1);
    pub const SERVER_SIDE_CANCEL: Capabilities = Capabilities(1 << 2);
    pub const SCHEMAS_WITHIN_CATALOG: Capabilities = Capabilities(1 << 3);
    pub const SAVEPOINTS: Capabilities = Capabilities(1 << 4);

    pub const fn contains(self, other: Capabilities) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Capabilities) -> Capabilities {
        Capabilities(self.0 | other.0)
    }
}

impl std::ops::BitOr for Capabilities {
    type Output = Capabilities;
    fn bitor(self, rhs: Capabilities) -> Capabilities {
        self.union(rhs)
    }
}

/// A statement's own query language — most backends speak one language
/// only, but the seam names it explicitly rather than assuming SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryLang {
    Sql,
    MongoShell,
    RedisCommand,
    Cql,
}

/// One statement to run: text plus its bound parameters — never a value
/// interpolated into `text` itself (ADR-0061 §1).
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub lang: QueryLang,
    pub text: String,
    pub params: Vec<Value>,
}

impl Statement {
    pub fn sql(text: impl Into<String>) -> Self {
        Self {
            lang: QueryLang::Sql,
            text: text.into(),
            params: Vec::new(),
        }
    }

    pub fn with_params(mut self, params: Vec<Value>) -> Self {
        self.params = params;
        self
    }
}

/// Per-execution tuning a caller can request; a backend that cannot honour
/// one (no server-side `max_rows`, say) approximates it client-side rather
/// than erroring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExecOptions {
    pub fetch_size: u32,
    pub max_rows: Option<u64>,
    pub timeout: Duration,
    pub read_only: bool,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            fetch_size: 200,
            max_rows: None,
            timeout: Duration::from_secs(30),
            read_only: false,
        }
    }
}

/// A statement's result shape.
pub enum Execution {
    /// A `SELECT`-shaped result: pull further pages through the stream.
    Rows(Box<dyn RowStream>),
    /// A row count, for an `INSERT`/`UPDATE`/`DELETE`-shaped statement.
    Affected(u64),
    /// A multi-statement script's per-statement results, in order.
    Multi(Vec<Execution>),
    /// Ran, produced no rows and no meaningful row count (DDL, `SET`, …).
    Ok,
}

/// Pages further batches of an in-flight `Rows` execution. Blocking:
/// `next_batch` returns once the next page is ready, or `Ok(None)` once
/// the result is exhausted.
pub trait RowStream: Send {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError>;
}

/// Server-side cancel, obtained *before* `execute` so it stays usable even
/// if the statement itself never returns (PG `cancel_query`, MySQL `KILL
/// QUERY` on a second connection, SQLite `interrupt`, Mongo `killOp`).
pub trait CancelHandle: Send + Sync {
    fn cancel(&self) -> Result<(), DbError>;
}

/// Consumer-side cancel: stops the caller from pulling further batches
/// regardless of whether the backend supports [`CancelHandle`] at all —
/// the second, always-available layer `database-tools.md` §4 describes.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A live connection to one backend. Entirely blocking — see this
/// module's doc comment.
pub trait Connection: Send {
    fn dialect(&self) -> Dialect;
    fn server_info(&self) -> String;
    /// Fetch `scope`'s objects at `level` (F2.1: `Names` stays a cheap,
    /// near-instant query even on a 5 000-table catalog — a backend that
    /// cannot cheaply distinguish levels may still fetch more than asked,
    /// but never less, so a caller that only needed `Names` always gets at
    /// least that much back).
    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError>;
    fn execute(
        &mut self,
        statement: &Statement,
        options: &ExecOptions,
    ) -> Result<Execution, DbError>;
    fn begin(&mut self) -> Result<(), DbError>;
    fn commit(&mut self) -> Result<(), DbError>;
    fn rollback(&mut self) -> Result<(), DbError>;
    fn set_read_only(&mut self, read_only: bool) -> Result<(), DbError>;
    /// `None` when this backend has no server-side cancel — the caller
    /// still has [`CancelToken`].
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>>;
    fn ddl_of(&mut self, object: &ObjectRef) -> Result<String, DbError>;
    /// Apply a `db_core::dml::DmlPlan`, returning the number of rows
    /// affected. Takes `&[Statement]` rather than the `dml` type itself,
    /// so this trait does not need to know about `EditBuffer`.
    fn apply(&mut self, statements: &[Statement]) -> Result<u64, DbError>;
    fn close(&mut self) -> Result<(), DbError>;
}

/// A backend descriptor plus the entry point that produces a [`Connection`].
pub trait Driver: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn Connection>, DbError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_union_and_contains() {
        let both = Capabilities::TRANSACTIONS | Capabilities::RETURNING;
        assert!(both.contains(Capabilities::TRANSACTIONS));
        assert!(both.contains(Capabilities::RETURNING));
        assert!(!both.contains(Capabilities::SERVER_SIDE_CANCEL));
        assert!(both.contains(Capabilities::NONE));
    }

    #[test]
    fn none_contains_only_none() {
        assert!(Capabilities::NONE.contains(Capabilities::NONE));
        assert!(!Capabilities::NONE.contains(Capabilities::TRANSACTIONS));
    }

    #[test]
    fn exec_options_default_matches_the_documented_fetch_size() {
        let options = ExecOptions::default();
        assert_eq!(options.fetch_size, 200);
        assert_eq!(options.max_rows, None);
        assert!(!options.read_only);
    }

    #[test]
    fn a_fresh_cancel_token_is_not_cancelled() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_token_clones_share_the_same_flag() {
        let token = CancelToken::new();
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn statement_builder_attaches_params() {
        let statement = Statement::sql("SELECT 1").with_params(vec![Value::Int(1)]);
        assert_eq!(statement.text, "SELECT 1");
        assert_eq!(statement.params, vec![Value::Int(1)]);
        assert_eq!(statement.lang, QueryLang::Sql);
    }
}
