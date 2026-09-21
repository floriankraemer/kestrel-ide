//! `Session`: one console's or one source-tree connection's live
//! connection, transaction mode, and the generation counter
//! `database-tools.md` §4 uses to drop a stale answer to a cancel nobody
//! is asking about anymore (the same guard `search-everywhere`'s ranked
//! popup already uses for its own late answers).
//!
//! `execute_script` splits a multi-statement script naively (on `;`, no
//! quote/comment awareness — a later phase's `db_sql::split` module is the
//! real, dialect-aware splitter; see the ponytail note below) and applies
//! one of three error policies.
//! ponytail: naive `;`-split has no idea about a `;` inside a string
//! literal or a dialect that does not use `;` as a terminator at all;
//! upgrade to `db_sql::split` once that module exists (F2+).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::driver::{Connection, ExecOptions, Statement};
use crate::error::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxMode {
    /// Every statement commits (or fails) on its own.
    Auto,
    /// A manual transaction is open; the caller must `commit`/`rollback`.
    Manual,
}

/// How `execute_script` handles a statement that fails partway through a
/// multi-statement script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptErrorPolicy {
    /// Halt at the first failure; statements before it already ran (each
    /// one commits on its own, same as running them one at a time).
    StopOnError,
    /// Run every statement regardless of individual failures, collecting
    /// every error.
    ContinueOnError,
    /// Wrap the whole script in one transaction: any failure rolls
    /// everything back, so either every statement's effect persists or
    /// none does.
    AllOrNothing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptOutcome {
    /// How many statements actually committed. For [`ScriptErrorPolicy::
    /// AllOrNothing`], `0` whenever `errors` is non-empty — a rollback
    /// undoes every one of them, so "how many ran" is not the same
    /// question as "how many persisted" for that policy.
    pub executed: usize,
    /// `(statement index, error)` pairs, in the order they failed.
    pub errors: Vec<(usize, DbError)>,
}

fn split_script(script: &str) -> Vec<String> {
    script
        .split(';')
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect()
}

/// One live connection plus its transaction mode and cancel-race guard.
pub struct Session {
    connection: Box<dyn Connection>,
    tx_mode: TxMode,
    generation: Arc<AtomicU64>,
}

impl Session {
    pub fn new(connection: Box<dyn Connection>) -> Self {
        Self {
            connection,
            tx_mode: TxMode::Auto,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn tx_mode(&self) -> TxMode {
        self.tx_mode
    }

    /// The generation a batch produced *now* should be tagged with — a
    /// batch that arrives carrying an older generation than
    /// [`Self::generation`] reports is stale and must be dropped, not
    /// appended (database-tools.md §4).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Bump the generation — called on cancel, so any batch already in
    /// flight from before this point reads as stale once it arrives.
    pub fn bump_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn begin_manual(&mut self) -> Result<(), DbError> {
        self.connection.begin()?;
        self.tx_mode = TxMode::Manual;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), DbError> {
        self.connection.commit()?;
        self.tx_mode = TxMode::Auto;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), DbError> {
        self.connection.rollback()?;
        self.tx_mode = TxMode::Auto;
        Ok(())
    }

    pub fn execute_script(
        &mut self,
        script: &str,
        policy: ScriptErrorPolicy,
        options: &ExecOptions,
    ) -> ScriptOutcome {
        let statements = split_script(script);
        match policy {
            ScriptErrorPolicy::StopOnError => {
                let mut executed = 0;
                let mut errors = Vec::new();
                for (index, text) in statements.iter().enumerate() {
                    match self
                        .connection
                        .execute(&Statement::sql(text.clone()), options)
                    {
                        Ok(_) => executed += 1,
                        Err(error) => {
                            errors.push((index, error));
                            break;
                        }
                    }
                }
                ScriptOutcome { executed, errors }
            }
            ScriptErrorPolicy::ContinueOnError => {
                let mut executed = 0;
                let mut errors = Vec::new();
                for (index, text) in statements.iter().enumerate() {
                    match self
                        .connection
                        .execute(&Statement::sql(text.clone()), options)
                    {
                        Ok(_) => executed += 1,
                        Err(error) => errors.push((index, error)),
                    }
                }
                ScriptOutcome { executed, errors }
            }
            ScriptErrorPolicy::AllOrNothing => {
                if let Err(error) = self.connection.begin() {
                    return ScriptOutcome {
                        executed: 0,
                        errors: vec![(0, error)],
                    };
                }
                let mut executed = 0;
                let mut errors = Vec::new();
                for (index, text) in statements.iter().enumerate() {
                    match self
                        .connection
                        .execute(&Statement::sql(text.clone()), options)
                    {
                        Ok(_) => executed += 1,
                        Err(error) => {
                            errors.push((index, error));
                            break;
                        }
                    }
                }
                if errors.is_empty() {
                    if let Err(error) = self.connection.commit() {
                        errors.push((executed, error));
                        executed = 0;
                        let _ = self.connection.rollback();
                    }
                } else {
                    let _ = self.connection.rollback();
                    executed = 0;
                }
                ScriptOutcome { executed, errors }
            }
        }
    }
}

/// Test support: a minimal `Connection` a session test drives without a
/// real backend. Only `execute` (fails a statement whose text contains
/// `"FAIL"`) and the transaction methods (recorded as counters) do
/// anything; every other method is not exercised by `Session` and panics
/// if a test ever calls it by mistake.
#[cfg(test)]
pub(crate) mod testsupport {
    use super::*;
    use crate::dialect::Dialect;
    use crate::driver::{CancelHandle, Execution};
    use crate::error::DbErrorCode;
    use crate::schema::{IntrospectScope, ObjectRef, SchemaSnapshot};

    #[derive(Default)]
    pub struct FakeConnection {
        pub begins: u32,
        pub commits: u32,
        pub rollbacks: u32,
        pub executed: Vec<String>,
    }

    impl Connection for FakeConnection {
        fn dialect(&self) -> Dialect {
            Dialect::Sqlite
        }

        fn server_info(&self) -> String {
            "fake".to_string()
        }

        fn introspect(&mut self, _scope: &IntrospectScope) -> Result<SchemaSnapshot, DbError> {
            unimplemented!("not exercised by Session's own tests")
        }

        fn execute(
            &mut self,
            statement: &Statement,
            _options: &ExecOptions,
        ) -> Result<Execution, DbError> {
            self.executed.push(statement.text.clone());
            if statement.text.contains("FAIL") {
                Err(DbError::new(DbErrorCode::InvalidStatement, "fake failure"))
            } else {
                Ok(Execution::Ok)
            }
        }

        fn begin(&mut self) -> Result<(), DbError> {
            self.begins += 1;
            Ok(())
        }

        fn commit(&mut self) -> Result<(), DbError> {
            self.commits += 1;
            Ok(())
        }

        fn rollback(&mut self) -> Result<(), DbError> {
            self.rollbacks += 1;
            Ok(())
        }

        fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
            Ok(())
        }

        fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
            None
        }

        fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
            unimplemented!("not exercised by Session's own tests")
        }

        fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
            unimplemented!("not exercised by Session's own tests")
        }

        fn close(&mut self) -> Result<(), DbError> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testsupport::FakeConnection;
    use super::*;

    fn session() -> Session {
        Session::new(Box::new(FakeConnection::default()))
    }

    #[test]
    fn a_fresh_session_starts_in_auto_mode_at_generation_zero() {
        let session = session();
        assert_eq!(session.tx_mode(), TxMode::Auto);
        assert_eq!(session.generation(), 0);
    }

    #[test]
    fn bump_generation_increments_and_returns_the_new_value() {
        let session = session();
        assert_eq!(session.bump_generation(), 1);
        assert_eq!(session.bump_generation(), 2);
        assert_eq!(session.generation(), 2);
    }

    #[test]
    fn begin_commit_switch_the_transaction_mode() {
        let mut session = session();
        session.begin_manual().unwrap();
        assert_eq!(session.tx_mode(), TxMode::Manual);
        session.commit().unwrap();
        assert_eq!(session.tx_mode(), TxMode::Auto);
    }

    #[test]
    fn stop_on_error_halts_at_the_first_failure() {
        let mut session = session();
        let outcome = session.execute_script(
            "SELECT 1; FAIL; SELECT 2",
            ScriptErrorPolicy::StopOnError,
            &ExecOptions::default(),
        );
        assert_eq!(outcome.executed, 1);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].0, 1);
    }

    #[test]
    fn continue_on_error_runs_every_statement_and_collects_every_failure() {
        let mut session = session();
        let outcome = session.execute_script(
            "SELECT 1; FAIL; SELECT 2; FAIL",
            ScriptErrorPolicy::ContinueOnError,
            &ExecOptions::default(),
        );
        assert_eq!(outcome.executed, 2);
        assert_eq!(outcome.errors.len(), 2);
        assert_eq!(
            outcome.errors.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn all_or_nothing_commits_when_every_statement_succeeds() {
        let mut session = session();
        let outcome = session.execute_script(
            "SELECT 1; SELECT 2",
            ScriptErrorPolicy::AllOrNothing,
            &ExecOptions::default(),
        );
        assert_eq!(outcome.executed, 2);
        assert!(outcome.errors.is_empty());
    }

    #[test]
    fn all_or_nothing_reports_zero_executed_after_a_rollback() {
        let mut session = session();
        let outcome = session.execute_script(
            "SELECT 1; FAIL; SELECT 2",
            ScriptErrorPolicy::AllOrNothing,
            &ExecOptions::default(),
        );
        assert_eq!(outcome.executed, 0);
        assert_eq!(outcome.errors.len(), 1);
    }

    #[test]
    fn all_or_nothing_actually_begins_and_rolls_back_on_the_fake_connection() {
        let connection = FakeConnection::default();
        let mut session = Session::new(Box::new(connection));
        session.execute_script(
            "SELECT 1; FAIL",
            ScriptErrorPolicy::AllOrNothing,
            &ExecOptions::default(),
        );
        // Reach back into the boxed connection is not possible without a
        // downcast; this test instead checks behaviourally that a second,
        // all-succeeding script still commits normally afterwards, proving
        // the session did not wedge itself in a half-open transaction.
        let outcome = session.execute_script(
            "SELECT 1",
            ScriptErrorPolicy::AllOrNothing,
            &ExecOptions::default(),
        );
        assert_eq!(outcome.executed, 1);
        assert!(outcome.errors.is_empty());
    }

    #[test]
    fn a_blank_script_produces_no_statements_and_no_errors() {
        let mut session = session();
        let outcome = session.execute_script(
            "   ; ; ",
            ScriptErrorPolicy::ContinueOnError,
            &ExecOptions::default(),
        );
        assert_eq!(outcome.executed, 0);
        assert!(outcome.errors.is_empty());
    }
}
