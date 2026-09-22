//! `classify` (database-tools.md §2): what a single statement *does* —
//! read, write, DDL, transaction/session control, or unclassifiable — the
//! call `db_core::readonly::Guard` and the script runner's per-statement
//! bookkeping both need. Deliberately keyword/structure-based rather than
//! a full parse: a console's content is often not fully valid SQL (a
//! statement mid-edit among several finished ones), and classification
//! must keep working on the finished ones regardless.

use db_core::dialect::Dialect;
use db_core::readonly::{Classifier, StatementKind};

use crate::dialects::lex_options;
use crate::scan::{scan, Spanned, Tok};

/// The richer classification this crate computes — a superset of
/// `db_core::readonly::StatementKind`, which has no `Tx` case (see
/// [`SqlClassifier`]'s trait impl for how the two meet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    Read,
    Write,
    Ddl,
    /// Transaction or session control: `BEGIN`/`COMMIT`/`ROLLBACK`/
    /// `SAVEPOINT`/`RELEASE`/`SET`/`USE`/a read-only `PRAGMA`. None of
    /// these touch table data, so the read-only guard treats this the
    /// same as `Read` (see [`SqlClassifier`]).
    Tx,
    Unknown,
}

const WRITE_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT", "CALL", "LOAD", "COPY", "EXEC",
    "EXECUTE", "DO",
];
const DDL_KEYWORDS: &[&str] = &[
    "CREATE", "ALTER", "DROP", "TRUNCATE", "RENAME", "GRANT", "REVOKE", "COMMENT", "ATTACH",
    "DETACH",
];
const TX_KEYWORDS: &[&str] = &[
    "BEGIN",
    "START",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
    "SET",
    "USE",
];
const READ_KEYWORDS: &[&str] = &["SHOW", "EXPLAIN", "DESCRIBE", "DESC"];

fn is_kw(word: &str, list: &[&str]) -> bool {
    list.iter().any(|kw| word.eq_ignore_ascii_case(kw))
}

/// `SELECT ... [INTO [OUTFILE|DUMPFILE] target] ...`: a plain `SELECT` is
/// `Read`; `SELECT ... INTO new_table ...` (Postgres/T-SQL "select into a
/// new table") is a `Write` since it creates and populates a table; MySQL's
/// `SELECT ... INTO OUTFILE/DUMPFILE 'path'` writes to the *filesystem*,
/// not the database, so it stays `Read` as far as the data source goes.
fn classify_select(rest: &[Spanned]) -> Classification {
    let mut depth = 0i32;
    let mut i = 0;
    while i < rest.len() {
        match &rest[i].tok {
            Tok::Symbol('(') => depth += 1,
            Tok::Symbol(')') => depth -= 1,
            Tok::Word(w) if depth == 0 && w.eq_ignore_ascii_case("INTO") => {
                let next = rest.get(i + 1).map(|s| &s.tok);
                let is_file_target = matches!(next, Some(Tok::Word(n)) if n.eq_ignore_ascii_case("OUTFILE") || n.eq_ignore_ascii_case("DUMPFILE"));
                return if is_file_target {
                    Classification::Read
                } else {
                    Classification::Write
                };
            }
            _ => {}
        }
        i += 1;
    }
    Classification::Read
}

/// `WITH [RECURSIVE] name AS (...) [, name2 AS (...)] <real statement>`:
/// every CTE body sits inside balanced parens (depth > 0), so the real
/// command keyword is the first `SELECT`/`INSERT`/`UPDATE`/`DELETE`/
/// `MERGE` this sees back at depth 0.
fn classify_with(rest: &[Spanned]) -> Classification {
    let mut depth = 0i32;
    for spanned in rest {
        match &spanned.tok {
            Tok::Symbol('(') => depth += 1,
            Tok::Symbol(')') => depth -= 1,
            Tok::Word(w) if depth == 0 => {
                let upper = w.to_ascii_uppercase();
                match upper.as_str() {
                    "SELECT" => return Classification::Read,
                    "INSERT" | "UPDATE" | "DELETE" | "MERGE" => return Classification::Write,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Classification::Unknown
}

/// `PRAGMA name` (a read) vs `PRAGMA name = value` (a session-level set,
/// `Tx`) — SQLite's one keyword that means either depending on shape.
fn classify_pragma(rest: &[Spanned]) -> Classification {
    if rest.iter().any(|t| matches!(&t.tok, Tok::Symbol('='))) {
        Classification::Tx
    } else {
        Classification::Read
    }
}

pub fn classify(statement: &str, dialect: Dialect) -> Classification {
    let opts = lex_options(dialect);
    let toks = scan(statement, &opts);
    let Some(Tok::Word(first)) = toks.first().map(|s| &s.tok) else {
        return Classification::Unknown;
    };
    let upper = first.to_ascii_uppercase();
    match upper.as_str() {
        "SELECT" => classify_select(&toks[1..]),
        "WITH" => classify_with(&toks[1..]),
        "PRAGMA" => classify_pragma(&toks[1..]),
        _ if is_kw(&upper, WRITE_KEYWORDS) => Classification::Write,
        _ if is_kw(&upper, DDL_KEYWORDS) => Classification::Ddl,
        _ if is_kw(&upper, TX_KEYWORDS) => Classification::Tx,
        _ if is_kw(&upper, READ_KEYWORDS) => Classification::Read,
        _ => Classification::Unknown,
    }
}

/// The `db_core::readonly::Classifier` this crate implements (replacing
/// `db_core::readonly::NaiveClassifier` once this crate is wired in — see
/// that module's doc comment). `Classification::Tx` maps to
/// `StatementKind::Read`: transaction/session control never touches table
/// data, so it is allowed on a read-only source the same as a `SELECT`.
pub struct SqlClassifier {
    pub dialect: Dialect,
}

impl Classifier for SqlClassifier {
    fn classify(&self, statement: &str) -> StatementKind {
        match classify(statement, self.dialect) {
            Classification::Read | Classification::Tx => StatementKind::Read,
            Classification::Write => StatementKind::Write,
            Classification::Ddl => StatementKind::Ddl,
            Classification::Unknown => StatementKind::Unknown,
        }
    }
}

/// Dispatches classification by family (F7b): `SqlClassifier` for every
/// SQL-shaped dialect (Cassandra/CQL included — its keyword grammar is
/// SQL-like enough for the same keyword scan), `db_sql::mongo`/`resp`'s
/// own read-only tables for Mongo/Redis text. Replaces a bare
/// `SqlClassifier` on a console attached to a NoSQL source, which would
/// otherwise fail every statement closed (`db.coll.find(…)` starts with
/// neither a SQL keyword nor a recognised Redis command name, so
/// `SqlClassifier` reports `Unknown` — the guard's fail-closed default —
/// and blocks reads on a read-only Mongo/Redis source).
pub struct FamilyClassifier {
    pub dialect: Dialect,
}

impl Classifier for FamilyClassifier {
    fn classify(&self, statement: &str) -> StatementKind {
        match self.dialect {
            Dialect::Mongo => classify_mongo(statement),
            Dialect::Redis => classify_redis(statement),
            _ => SqlClassifier {
                dialect: self.dialect,
            }
            .classify(statement),
        }
    }
}

fn classify_mongo(statement: &str) -> StatementKind {
    match crate::mongo::parse(statement) {
        Ok(crate::mongo::MongoCommand::Sugar { method, .. }) => {
            if crate::mongo::is_read_only_method(&method) {
                StatementKind::Read
            } else {
                StatementKind::Write
            }
        }
        Ok(crate::mongo::MongoCommand::RunCommand(document)) => {
            match crate::mongo::run_command_name(&document) {
                Some(name) if crate::mongo::is_read_only_run_command(name) => StatementKind::Read,
                Some(_) => StatementKind::Write,
                None => StatementKind::Unknown,
            }
        }
        Err(_) => StatementKind::Unknown,
    }
}

fn classify_redis(statement: &str) -> StatementKind {
    match crate::resp::tokenize(statement) {
        Ok(tokens) => match tokens.first() {
            Some(command) if crate::resp::is_read_only_command(command) => StatementKind::Read,
            Some(_) => StatementKind::Write,
            None => StatementKind::Unknown,
        },
        Err(_) => StatementKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Classification::*;

    fn c(sql: &str, dialect: Dialect) -> Classification {
        classify(sql, dialect)
    }

    // A shared matrix run against every SQL dialect this crate supports,
    // plus a handful of dialect-specific cases below it.
    fn common_cases() -> Vec<(&'static str, Classification)> {
        vec![
            ("SELECT * FROM t", Read),
            ("select * from t", Read),
            ("  SELECT 1", Read),
            ("SELECT * FROM t WHERE id = 1", Read),
            ("SELECT * FROM t JOIN u ON t.id = u.id", Read),
            ("WITH x AS (SELECT 1) SELECT * FROM x", Read),
            ("WITH RECURSIVE x AS (SELECT 1) SELECT * FROM x", Read),
            (
                "WITH x AS (SELECT 1), y AS (SELECT 2) SELECT * FROM x, y",
                Read,
            ),
            ("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x", Write),
            ("WITH x AS (SELECT 1) UPDATE t SET a = 1", Write),
            ("WITH x AS (SELECT 1) DELETE FROM t", Write),
            ("INSERT INTO t VALUES (1)", Write),
            ("UPDATE t SET a = 1", Write),
            ("UPDATE t SET a = 1 WHERE id = 1", Write),
            ("DELETE FROM t", Write),
            ("DELETE FROM t WHERE id = 1", Write),
            ("CALL do_something()", Write),
            ("CREATE TABLE t (id int)", Ddl),
            ("ALTER TABLE t ADD COLUMN c int", Ddl),
            ("DROP TABLE t", Ddl),
            ("TRUNCATE TABLE t", Ddl),
            ("RENAME TABLE t TO u", Ddl),
            ("GRANT SELECT ON t TO u", Ddl),
            ("REVOKE SELECT ON t FROM u", Ddl),
            ("BEGIN", Tx),
            ("BEGIN TRANSACTION", Tx),
            ("START TRANSACTION", Tx),
            ("COMMIT", Tx),
            ("ROLLBACK", Tx),
            ("SAVEPOINT sp1", Tx),
            ("RELEASE sp1", Tx),
            ("SET autocommit = 0", Tx),
            ("EXPLAIN SELECT * FROM t", Read),
            ("SHOW TABLES", Read),
            ("", Unknown),
            ("   ", Unknown),
            ("; garbage only symbols )", Unknown),
        ]
    }

    #[test]
    fn postgres_matrix() {
        for (sql, expected) in common_cases() {
            assert_eq!(c(sql, Dialect::Postgres), expected, "postgres: {sql:?}");
        }
        assert_eq!(c("COPY t TO '/tmp/t.csv'", Dialect::Postgres), Write);
        assert_eq!(c("DO $$ BEGIN NULL; END $$", Dialect::Postgres), Write);
        assert_eq!(
            c("SELECT * INTO new_table FROM old_table", Dialect::Postgres),
            Write
        );
    }

    #[test]
    fn mysql_matrix() {
        for (sql, expected) in common_cases() {
            assert_eq!(c(sql, Dialect::MySql), expected, "mysql: {sql:?}");
        }
        assert_eq!(c("REPLACE INTO t VALUES (1)", Dialect::MySql), Write);
        assert_eq!(
            c("LOAD DATA INFILE '/tmp/t.csv' INTO TABLE t", Dialect::MySql),
            Write
        );
        assert_eq!(
            c("SELECT * FROM t INTO OUTFILE '/tmp/t.csv'", Dialect::MySql),
            Read
        );
        assert_eq!(c("USE mydb", Dialect::MySql), Tx);
    }

    #[test]
    fn sqlite_matrix() {
        for (sql, expected) in common_cases() {
            assert_eq!(c(sql, Dialect::Sqlite), expected, "sqlite: {sql:?}");
        }
        assert_eq!(c("PRAGMA table_info(t)", Dialect::Sqlite), Read);
        assert_eq!(c("PRAGMA foreign_keys = ON", Dialect::Sqlite), Tx);
        assert_eq!(c("ATTACH DATABASE 'x.db' AS x", Dialect::Sqlite), Ddl);
    }

    #[test]
    fn sqlserver_matrix() {
        for (sql, expected) in common_cases() {
            assert_eq!(c(sql, Dialect::SqlServer), expected, "sqlserver: {sql:?}");
        }
        assert_eq!(c("EXEC do_something", Dialect::SqlServer), Write);
        assert_eq!(c("EXECUTE do_something", Dialect::SqlServer), Write);
    }

    #[test]
    fn client_classifier_maps_tx_to_read() {
        let classifier = SqlClassifier {
            dialect: Dialect::Postgres,
        };
        assert_eq!(classifier.classify("COMMIT"), StatementKind::Read);
        assert_eq!(classifier.classify("SELECT 1"), StatementKind::Read);
        assert_eq!(
            classifier.classify("INSERT INTO t VALUES (1)"),
            StatementKind::Write
        );
        assert_eq!(classifier.classify("DROP TABLE t"), StatementKind::Ddl);
        assert_eq!(classifier.classify(""), StatementKind::Unknown);
    }

    #[test]
    fn a_semicolon_inside_a_string_is_not_mistaken_for_a_boundary() {
        // classify receives one already-split statement, but must still
        // not choke if it contains a quoted `;` (it never re-splits).
        assert_eq!(c("INSERT INTO t VALUES (';')", Dialect::Postgres), Write);
    }

    #[test]
    fn family_classifier_reads_a_mongo_find_but_writes_an_insert() {
        let classifier = FamilyClassifier {
            dialect: Dialect::Mongo,
        };
        assert_eq!(
            classifier.classify(r#"db.users.find({"active": true})"#),
            StatementKind::Read
        );
        assert_eq!(
            classifier.classify(r#"db.users.insertOne({"a": 1})"#),
            StatementKind::Write
        );
        assert_eq!(
            classifier.classify(r#"{"find": "users"}"#),
            StatementKind::Read
        );
        assert_eq!(
            classifier.classify(r#"{"insert": "users", "documents": []}"#),
            StatementKind::Write
        );
        assert_eq!(
            classifier.classify("not mongo at all"),
            StatementKind::Unknown
        );
    }

    #[test]
    fn family_classifier_reads_a_redis_get_but_writes_a_set() {
        let classifier = FamilyClassifier {
            dialect: Dialect::Redis,
        };
        assert_eq!(classifier.classify("GET foo"), StatementKind::Read);
        assert_eq!(classifier.classify("set foo bar"), StatementKind::Write);
    }

    #[test]
    fn family_classifier_still_uses_the_sql_classifier_for_cassandra() {
        let classifier = FamilyClassifier {
            dialect: Dialect::Cassandra,
        };
        assert_eq!(
            classifier.classify("SELECT * FROM ks.t"),
            StatementKind::Read
        );
        assert_eq!(
            classifier.classify("INSERT INTO ks.t (id) VALUES (1)"),
            StatementKind::Write
        );
    }
}
