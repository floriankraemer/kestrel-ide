//! `Dialect`: identifier and literal quoting per backend family — the one
//! place this rule lives (ADR-0058's "a consumer never re-derives a rule a
//! driver already encodes"), so `db-sql`'s DDL generator and `db-core`'s
//! own `dml::plan` never hand-roll a quoting rule apiece.

/// How a dialect delimits an identifier that needs quoting (mixed case, a
/// keyword, a space, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteStyle {
    /// `"ident"`, doubled `"` to escape (ANSI SQL, Postgres, SQLite).
    Double,
    /// `` `ident` ``, doubled `` ` `` to escape (MySQL/MariaDB).
    Backtick,
    /// `[ident]`, doubled `]` to escape (SQL Server).
    Bracket,
    /// No quoting at all (a store with no SQL identifier grammar —
    /// MongoDB, Redis).
    None,
}

/// A backend family's own SQL/identifier conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    MySql,
    SqlServer,
    Sqlite,
    Cassandra,
    Mongo,
    Redis,
}

impl Dialect {
    fn quote_style(self) -> QuoteStyle {
        match self {
            Dialect::Postgres | Dialect::Sqlite | Dialect::Cassandra => QuoteStyle::Double,
            Dialect::MySql => QuoteStyle::Backtick,
            Dialect::SqlServer => QuoteStyle::Bracket,
            Dialect::Mongo | Dialect::Redis => QuoteStyle::None,
        }
    }

    /// Quote `ident` for use as an identifier in this dialect, escaping any
    /// embedded delimiter by doubling it — never by stripping or
    /// backslash-escaping, both of which change what the identifier
    /// actually names.
    pub fn quote_ident(self, ident: &str) -> String {
        match self.quote_style() {
            QuoteStyle::Double => format!("\"{}\"", ident.replace('"', "\"\"")),
            QuoteStyle::Backtick => format!("`{}`", ident.replace('`', "``")),
            QuoteStyle::Bracket => format!("[{}]", ident.replace(']', "]]")),
            QuoteStyle::None => ident.to_string(),
        }
    }

    /// Quote `value` as a single-quoted string literal, escaping an
    /// embedded `'` by doubling it (the one escape every SQL dialect this
    /// enum lists shares). Only `db_core::dml`'s `WHERE` clause for a
    /// `NoPrimaryKey`-policy row match ever calls this directly — every
    /// bound parameter goes through the driver's own placeholder, never a
    /// literal (ADR-0061 §1's "all values bound" rule).
    pub fn quote_literal(self, value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_and_sqlite_use_double_quotes() {
        assert_eq!(Dialect::Postgres.quote_ident("users"), "\"users\"");
        assert_eq!(Dialect::Sqlite.quote_ident("users"), "\"users\"");
    }

    #[test]
    fn mysql_uses_backticks() {
        assert_eq!(Dialect::MySql.quote_ident("users"), "`users`");
    }

    #[test]
    fn sql_server_uses_brackets() {
        assert_eq!(Dialect::SqlServer.quote_ident("users"), "[users]");
    }

    #[test]
    fn mongo_and_redis_quote_nothing() {
        assert_eq!(Dialect::Mongo.quote_ident("users"), "users");
        assert_eq!(Dialect::Redis.quote_ident("users"), "users");
    }

    #[test]
    fn an_embedded_double_quote_is_escaped_by_doubling() {
        assert_eq!(Dialect::Postgres.quote_ident("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn an_embedded_backtick_is_escaped_by_doubling() {
        assert_eq!(Dialect::MySql.quote_ident("a`b"), "`a``b`");
    }

    #[test]
    fn an_embedded_bracket_is_escaped_by_doubling() {
        assert_eq!(Dialect::SqlServer.quote_ident("a]b"), "[a]]b]");
    }

    #[test]
    fn an_injection_attempt_stays_inert_inside_the_quoted_identifier() {
        let ident = Dialect::Postgres.quote_ident("Robert'); DROP TABLE students;--");
        // The whole payload is still one quoted identifier: no unescaped
        // `"` appears inside it that could close the identifier early.
        let inner = &ident[1..ident.len() - 1];
        assert!(!inner.contains('"'));
        assert_eq!(ident, "\"Robert'); DROP TABLE students;--\"");
    }

    #[test]
    fn a_nul_byte_survives_quoting_without_panicking() {
        let ident = Dialect::MySql.quote_ident("a\0b");
        assert_eq!(ident, "`a\0b`");
    }

    #[test]
    fn unicode_quote_lookalikes_are_not_treated_as_the_real_delimiter() {
        // “smart quotes”, never the dialect's own ASCII delimiter — must
        // pass through unescaped since they cannot break out of anything.
        let ident = Dialect::Postgres.quote_ident("a\u{201c}b\u{201d}");
        assert_eq!(ident, "\"a\u{201c}b\u{201d}\"");
    }

    #[test]
    fn quote_literal_escapes_an_embedded_single_quote() {
        assert_eq!(
            Dialect::Postgres.quote_literal("Robert'); DROP TABLE students;--"),
            "'Robert''); DROP TABLE students;--'"
        );
    }

    #[test]
    fn quote_literal_round_trips_plain_text() {
        assert_eq!(Dialect::Sqlite.quote_literal("hello"), "'hello'");
    }
}
