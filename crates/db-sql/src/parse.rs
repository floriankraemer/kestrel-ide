//! `parse` (database-tools.md §2): a real grammar check via `sqlparser`,
//! for the cases `classify`'s keyword scan can't catch — a misspelled
//! keyword, an unbalanced paren, a missing `FROM`. Errors become
//! `diagnostics_core::Diagnostic`s so they can be published under the
//! `database:inspections` source (F3.7) the same way every other
//! diagnostic in this codebase is.

use db_core::dialect::Dialect;
use diagnostics_core::{Diagnostic, Position, Range, Severity};
use sqlparser::ast::Statement;
use sqlparser::parser::Parser;

use crate::dialects::sqlparser_dialect;

/// Parses one statement's text. `sqlparser::parser::ParserError` carries no
/// structured position in the 0.63 API this crate pins to, so a failed
/// parse is reported at the statement's own start — precise enough to
/// underline the right statement in a multi-statement console, not
/// precise enough to underline the exact token.
/// ponytail: whole-statement position on error, narrow it if `sqlparser`
/// ever exposes a token span on `ParserError`.
pub fn parse(
    sql: &str,
    dialect: Dialect,
    start: Position,
) -> Result<Vec<Statement>, Box<Diagnostic>> {
    let grammar = sqlparser_dialect(dialect);
    Parser::parse_sql(&*grammar, sql).map_err(|error| {
        Box::new(Diagnostic {
            range: Range { start, end: None },
            severity: Severity::Error,
            message: error.to_string(),
            source: "db-sql".to_string(),
            raw: None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> Position {
        Position {
            line: 0,
            character: 0,
        }
    }

    #[test]
    fn a_valid_select_parses() {
        let statements = parse("SELECT * FROM t", Dialect::Postgres, zero()).unwrap();
        assert_eq!(statements.len(), 1);
    }

    #[test]
    fn an_invalid_statement_produces_a_diagnostic() {
        let error = parse("SELEKT * FROM t", Dialect::Postgres, zero()).unwrap_err();
        assert_eq!(error.severity, Severity::Error);
        assert_eq!(error.source, "db-sql");
        assert!(!error.message.is_empty());
    }

    #[test]
    fn the_diagnostic_position_is_the_caller_supplied_statement_start() {
        let start = Position {
            line: 3,
            character: 5,
        };
        let error = parse("garbage(((", Dialect::Postgres, start).unwrap_err();
        assert_eq!(error.range.start, start);
    }

    #[test]
    fn each_dialect_grammar_parses_its_own_valid_statement() {
        assert!(parse("SELECT 1", Dialect::Postgres, zero()).is_ok());
        assert!(parse("SELECT 1", Dialect::MySql, zero()).is_ok());
        assert!(parse("SELECT 1", Dialect::Sqlite, zero()).is_ok());
        assert!(parse("SELECT 1", Dialect::SqlServer, zero()).is_ok());
    }
}
