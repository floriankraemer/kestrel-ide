//! Mapping from `db_core::dialect::Dialect` (the backend-family enum every
//! other `db-core`-consuming crate already programs against) to the two
//! things this crate needs per dialect: a `sqlparser` grammar, and the
//! lexical quirks `scan`/`split` must honour (MySQL's `#` line comment and
//! `DELIMITER` directive, Postgres's `$$` dollar-quoting).
//!
//! `db_core::dialect::Dialect` also lists three non-SQL backends
//! (`Mongo`, `Redis`, `Cassandra`'s CQL is SQL-*like* but not what
//! `sqlparser` parses) — every function in this crate that takes a
//! `Dialect` treats those as the closest SQL dialect available
//! (`Cassandra` → generic, `Mongo`/`Redis` are never actually routed here;
//! `bridge/language/database.rs` only calls into `db-sql` for a SQL
//! source).

use db_core::dialect::Dialect;
use sqlparser::dialect::{
    Dialect as SqlParserDialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
};

/// Lexical quirks that vary by dialect and matter to `scan`/`split`
/// (everything else — string/comment syntax — is the same across every
/// dialect this crate supports).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexOptions {
    /// MySQL/MariaDB: `#` starts a line comment in addition to `--`.
    pub hash_comment: bool,
    /// Postgres: `$$`/`$tag$ ... $tag$` delimits a string that needs no
    /// internal escaping — common for function bodies.
    pub dollar_quotes: bool,
    /// MySQL/MariaDB: a `DELIMITER <token>` line changes the statement
    /// terminator `split` watches for, until the next `DELIMITER` line.
    pub delimiter_directive: bool,
}

pub fn lex_options(dialect: Dialect) -> LexOptions {
    match dialect {
        Dialect::MySql => LexOptions {
            hash_comment: true,
            dollar_quotes: false,
            delimiter_directive: true,
        },
        Dialect::Postgres => LexOptions {
            hash_comment: false,
            dollar_quotes: true,
            delimiter_directive: false,
        },
        Dialect::Sqlite
        | Dialect::SqlServer
        | Dialect::Cassandra
        | Dialect::Mongo
        | Dialect::Redis => LexOptions {
            hash_comment: false,
            dollar_quotes: false,
            delimiter_directive: false,
        },
    }
}

/// The `sqlparser` grammar to parse/format with. `SqlServer` has no
/// dedicated `sqlparser` dialect with full T-SQL coverage as of 0.63, so it
/// gets `GenericDialect` — permissive enough for `[bracket]` identifiers,
/// which is the one T-SQL lexical quirk that would otherwise break parsing.
pub fn sqlparser_dialect(dialect: Dialect) -> Box<dyn SqlParserDialect> {
    match dialect {
        Dialect::Postgres => Box::new(PostgreSqlDialect {}),
        Dialect::MySql => Box::new(MySqlDialect {}),
        Dialect::Sqlite => Box::new(SQLiteDialect {}),
        Dialect::SqlServer | Dialect::Cassandra | Dialect::Mongo | Dialect::Redis => {
            Box::new(GenericDialect {})
        }
    }
}
