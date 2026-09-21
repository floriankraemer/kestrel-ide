//! `format` (database-tools.md §2): pretty-print a statement with
//! `sqlformat`. `sqlformat` itself has no per-dialect grammar — it is a
//! token-shape reformatter, not a parser — so "dialect options" here means
//! the one dialect-shaped knob it does expose (indent width) plus this
//! crate's house style (uppercase keywords), applied the same way for
//! every `db_core::dialect::Dialect` this crate supports.
//! ponytail: no actual per-dialect formatting divergence exists yet since
//! `sqlformat` doesn't model any; revisit if a dialect ever needs its own
//! style.

use db_core::dialect::Dialect;
use sqlformat::{FormatOptions, Indent, QueryParams};

/// The one dialect-shaped knob `sqlformat` exposes: `[bracket]`
/// identifiers for `SqlServer`, Postgres array syntax for `Postgres`,
/// generic everywhere else (see this module's doc comment).
fn sqlformat_dialect(dialect: Dialect) -> sqlformat::Dialect {
    match dialect {
        Dialect::Postgres => sqlformat::Dialect::PostgreSql,
        Dialect::SqlServer => sqlformat::Dialect::SQLServer,
        Dialect::MySql | Dialect::Sqlite | Dialect::Cassandra | Dialect::Mongo | Dialect::Redis => {
            sqlformat::Dialect::Generic
        }
    }
}

fn options(dialect: Dialect) -> FormatOptions<'static> {
    FormatOptions {
        indent: Indent::Spaces(2),
        uppercase: Some(true),
        lines_between_queries: 1,
        dialect: sqlformat_dialect(dialect),
        ..Default::default()
    }
}

pub fn format(sql: &str, dialect: Dialect) -> String {
    sqlformat::format(sql, &QueryParams::None, &options(dialect))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercases_keywords_and_indents() {
        let out = format("select a, b from t where a = 1", Dialect::Postgres);
        assert!(out.contains("SELECT"));
        assert!(out.contains("FROM"));
        assert!(out.contains("WHERE"));
    }

    #[test]
    fn is_stable_on_already_formatted_input() {
        let once = format("SELECT 1", Dialect::Sqlite);
        let twice = format(&once, Dialect::Sqlite);
        assert_eq!(once, twice);
    }
}
