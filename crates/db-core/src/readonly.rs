//! The client-side half of the read-only guarantee (ADR-0061 §3):
//! classify a statement, refuse it before it ever reaches a read-only
//! source's connection. This is deliberately *not* the real classifier —
//! `db_sql::classify` (a later phase) is the ≥ 40-test-case-per-dialect
//! implementation ADR-0061 §3 requires; this module only defines the hook
//! every consumer programs against, plus a minimal classifier so F1's
//! read-only checks have something to run before `db-sql` exists.
//!
//! Client classification is one of *two* independent checks the real
//! guarantee needs — the other is the engine's own read-only session flag
//! (`dialect::tx_read_only_stmt`, a later phase) — neither alone is
//! trusted as sufficient (ADR-0061 §3).

use crate::error::{DbError, DbErrorCode};

/// What a statement does, as far as the read-only guard is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Read,
    Write,
    Ddl,
    /// Could not be classified — treated as `Write` by the guard, the
    /// same fail-closed choice an unrecognised statement always gets.
    Unknown,
}

/// The hook a real classifier implements. `db_sql::classify` (a later
/// phase) replaces [`NaiveClassifier`] with a real per-dialect parser;
/// this trait is what lets `readonly::Guard` not care which one it holds.
pub trait Classifier: Send + Sync {
    fn classify(&self, statement: &str) -> StatementKind;
}

/// A first-word keyword check — enough to unblock F1's guard and its
/// tests, replaced by `db_sql::classify` once it lands (this is
/// deliberately not the ≥ 40-case-per-dialect classifier ADR-0061 §3
/// requires; see this module's doc comment).
pub struct NaiveClassifier;

impl Classifier for NaiveClassifier {
    fn classify(&self, statement: &str) -> StatementKind {
        let first_word = statement
            .trim_start()
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        match first_word.as_str() {
            "SELECT" | "WITH" | "EXPLAIN" | "SHOW" => StatementKind::Read,
            "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "CALL" | "REPLACE" => StatementKind::Write,
            "CREATE" | "ALTER" | "DROP" | "TRUNCATE" => StatementKind::Ddl,
            "" => StatementKind::Unknown,
            _ => StatementKind::Write,
        }
    }
}

/// Refuses a `Write`/`Ddl`-classified statement against a read-only
/// source. The data editor is hidden entirely on a read-only source
/// (ADR-0061 §3) — this guard is what a console (which cannot be hidden
/// the same way) checks before running a typed statement.
///
/// `read_only` is the source's own flag, captured once at construction:
/// a `Guard` built for a *writable* source lets every statement through
/// without even asking the classifier — this is the fix for the F3e
/// follow-up (database-tools-plan's "F3e follow-up" row): every earlier
/// caller built a `Guard` and called `check` unconditionally, which
/// refused a plain `INSERT` on a perfectly writable source. Every
/// consumer now routes through this one constructor rather than each
/// re-deriving its own "only if read-only" gate around `check` (the
/// `sql_script` module used to be the one caller that got this right by
/// gating its own call — that gate moves in here instead, so a future
/// caller cannot forget it).
pub struct Guard {
    read_only: bool,
    classifier: Box<dyn Classifier>,
}

impl Guard {
    pub fn new(read_only: bool, classifier: Box<dyn Classifier>) -> Self {
        Self {
            read_only,
            classifier,
        }
    }

    pub fn check(&self, statement: &str) -> Result<(), DbError> {
        if !self.read_only {
            return Ok(());
        }
        match self.classifier.classify(statement) {
            StatementKind::Read => Ok(()),
            StatementKind::Write | StatementKind::Ddl | StatementKind::Unknown => {
                Err(DbError::new(
                    DbErrorCode::ReadOnlyViolation,
                    "this data source is read-only",
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> Guard {
        Guard::new(true, Box::new(NaiveClassifier))
    }

    fn writable_guard() -> Guard {
        Guard::new(false, Box::new(NaiveClassifier))
    }

    #[test]
    fn a_select_passes() {
        assert!(guard().check("SELECT * FROM users").is_ok());
    }

    #[test]
    fn a_cte_wrapped_select_passes() {
        assert!(guard()
            .check("WITH t AS (SELECT 1) SELECT * FROM t")
            .is_ok());
    }

    #[test]
    fn an_insert_is_refused() {
        let error = guard().check("INSERT INTO users VALUES (1)").unwrap_err();
        assert_eq!(error.code, DbErrorCode::ReadOnlyViolation);
    }

    #[test]
    fn a_bare_call_is_refused() {
        assert!(guard().check("CALL do_something()").is_err());
    }

    #[test]
    fn ddl_is_refused() {
        assert!(guard().check("DROP TABLE users").is_err());
    }

    #[test]
    fn an_unclassifiable_statement_fails_closed() {
        assert!(guard().check("").is_err());
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert!(guard().check("select 1").is_ok());
        assert!(guard().check("insert into t values (1)").is_err());
    }

    /// The F3e follow-up this constructor fixes: a `Guard` built for a
    /// writable source must let a write/DDL/unknown statement straight
    /// through, not just a `SELECT`.
    #[test]
    fn a_writable_source_s_guard_passes_every_statement() {
        assert!(writable_guard()
            .check("INSERT INTO users VALUES (1)")
            .is_ok());
        assert!(writable_guard().check("DROP TABLE users").is_ok());
        assert!(writable_guard().check("").is_ok());
        assert!(writable_guard().check("CALL do_something()").is_ok());
    }

    #[test]
    fn a_read_only_source_s_guard_still_refuses_writes() {
        assert!(guard().check("INSERT INTO users VALUES (1)").is_err());
    }
}
