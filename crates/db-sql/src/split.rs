//! Statement splitting (database-tools.md §2's `split`): breaks a console's
//! or a script file's text into individual statements on `;` (or, for
//! MySQL, whatever a `DELIMITER` directive currently names), staying blind
//! to a `;` inside a string, a comment, or a Postgres dollar-quoted body.
//! Each returned [`Statement`] is what "run statement at caret" and the
//! script runner both key off — a byte range plus its start position, in
//! `diagnostics_core::Position`'s 0-based-line / UTF-16-column counting so
//! a `db-sql` diagnostic and a split statement's position never need two
//! different conventions.

use db_core::dialect::Dialect;
use diagnostics_core::Position;

use crate::dialects::lex_options;
use crate::scan::{position_at, skip_ignorable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub start: usize,
    pub end: usize,
    pub start_pos: Position,
}

impl Statement {
    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.start..self.end]
    }
}

fn at_line_start(sql: &str, i: usize) -> bool {
    i == 0 || sql.as_bytes().get(i - 1) == Some(&b'\n')
}

/// `DELIMITER <token>` on its own line (MySQL's `mysql` CLI convention,
/// used to let a script embed `;`-terminated statements inside a stored
/// routine body). Recognised only at the very start of a line — the
/// convention every MySQL script this crate has seen also follows.
/// ponytail: line-start-only match, loosen if a real script needs
/// `DELIMITER` after leading whitespace.
fn match_delimiter_directive(rest: &str) -> Option<(String, usize)> {
    const KW: &str = "DELIMITER";
    if rest.len() < KW.len() || !rest[..KW.len()].eq_ignore_ascii_case(KW) {
        return None;
    }
    let after_kw = &rest[KW.len()..];
    if !after_kw.starts_with(char::is_whitespace) {
        return None;
    }
    let trimmed = after_kw.trim_start();
    let token_len = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    if token_len == 0 {
        return None;
    }
    let consumed_before_newline = KW.len() + (after_kw.len() - trimmed.len()) + token_len;
    let line_end = rest.find('\n').map(|p| p + 1).unwrap_or(rest.len());
    Some((
        trimmed[..token_len].to_string(),
        line_end.max(consumed_before_newline),
    ))
}

fn push_if_nonblank(out: &mut Vec<Statement>, sql: &str, start: usize, end: usize) {
    let text = &sql[start..end];
    let trimmed_end = end - (text.len() - text.trim_end().len());
    let trimmed_start = start + (text.len() - text.trim_start().len());
    if trimmed_start >= trimmed_end {
        return;
    }
    out.push(Statement {
        start: trimmed_start,
        end: trimmed_end,
        start_pos: position_at(sql, trimmed_start),
    });
}

pub fn split(sql: &str, dialect: Dialect) -> Vec<Statement> {
    let opts = lex_options(dialect);
    let mut delimiter = ";".to_string();
    let mut statements = Vec::new();
    let mut stmt_start: Option<usize> = None;
    let mut i = 0;
    while i < sql.len() {
        if opts.delimiter_directive && at_line_start(sql, i) {
            if let Some((new_delimiter, consumed)) = match_delimiter_directive(&sql[i..]) {
                if let Some(start) = stmt_start.take() {
                    push_if_nonblank(&mut statements, sql, start, i);
                }
                delimiter = new_delimiter;
                i += consumed;
                continue;
            }
        }
        if let Some(skip_to) = skip_ignorable(sql, i, &opts) {
            if stmt_start.is_none() {
                stmt_start = Some(i);
            }
            i = skip_to.max(i + 1);
            continue;
        }
        if !delimiter.is_empty() && sql[i..].starts_with(delimiter.as_str()) {
            if let Some(start) = stmt_start.take() {
                push_if_nonblank(&mut statements, sql, start, i);
            }
            i += delimiter.len();
            continue;
        }
        let ch = sql[i..].chars().next().unwrap();
        if !ch.is_whitespace() && stmt_start.is_none() {
            stmt_start = Some(i);
        }
        i += ch.len_utf8();
    }
    if let Some(start) = stmt_start {
        push_if_nonblank(&mut statements, sql, start, sql.len());
    }
    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sql: &str, dialect: Dialect) -> Vec<&str> {
        split(sql, dialect).iter().map(|s| s.text(sql)).collect()
    }

    #[test]
    fn splits_two_plain_statements() {
        assert_eq!(
            texts("SELECT 1; SELECT 2;", Dialect::Postgres),
            vec!["SELECT 1", "SELECT 2"]
        );
    }

    #[test]
    fn a_trailing_statement_with_no_terminator_is_kept() {
        assert_eq!(
            texts("SELECT 1; SELECT 2", Dialect::Postgres),
            vec!["SELECT 1", "SELECT 2"]
        );
    }

    #[test]
    fn a_semicolon_inside_a_string_does_not_split() {
        assert_eq!(
            texts("SELECT ';'; SELECT 2;", Dialect::Postgres),
            vec!["SELECT ';'", "SELECT 2"]
        );
    }

    #[test]
    fn a_semicolon_inside_a_line_comment_does_not_split() {
        assert_eq!(
            texts("SELECT 1 -- a;b\n; SELECT 2;", Dialect::Postgres),
            vec!["SELECT 1 -- a;b", "SELECT 2"]
        );
    }

    #[test]
    fn a_semicolon_inside_a_block_comment_does_not_split() {
        assert_eq!(
            texts("SELECT 1 /* a;b */; SELECT 2;", Dialect::Postgres),
            vec!["SELECT 1 /* a;b */", "SELECT 2"]
        );
    }

    #[test]
    fn a_semicolon_inside_a_dollar_quoted_body_does_not_split() {
        assert_eq!(
            texts(
                "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql;",
                Dialect::Postgres
            )
            .len(),
            1
        );
    }

    #[test]
    fn mysql_hash_comment_semicolon_does_not_split() {
        assert_eq!(
            texts("SELECT 1 # a;b\n; SELECT 2;", Dialect::MySql),
            vec!["SELECT 1 # a;b", "SELECT 2"]
        );
    }

    #[test]
    fn mysql_delimiter_directive_changes_the_terminator() {
        let sql = "DELIMITER $$\nSELECT 1; SELECT 2$$\nDELIMITER ;\nSELECT 3;";
        assert_eq!(
            texts(sql, Dialect::MySql),
            vec!["SELECT 1; SELECT 2", "SELECT 3"]
        );
    }

    #[test]
    fn delimiter_directive_is_ignored_outside_mysql() {
        // Postgres has no `DELIMITER` directive — the literal text is just
        // an (invalid) statement, and `;` still splits normally.
        // No semicolon precedes `SELECT 1`, so the literal `DELIMITER $$`
        // text and the `SELECT 1` after it are one un-terminated-until-the-
        // end statement, split into exactly one piece by the trailing `;`.
        let sql = "DELIMITER $$\nSELECT 1;";
        assert_eq!(split(sql, Dialect::Postgres).len(), 1);
    }

    #[test]
    fn blank_input_yields_no_statements() {
        assert_eq!(split("   \n\t ", Dialect::Postgres), vec![]);
    }

    #[test]
    fn start_position_is_zero_based_line_and_utf16_column() {
        let statements = split("SELECT 1;\nSELECT 2;", Dialect::Postgres);
        assert_eq!(statements[1].start_pos.line, 1);
        assert_eq!(statements[1].start_pos.character, 0);
    }
}
