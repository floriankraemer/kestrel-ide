//! Dialect-aware `WHERE`/`ORDER BY` injection (database-tools-plan F3e):
//! the grid's filter/sort bar re-runs a console's original statement with
//! extra clauses applied. A plain `SELECT` gets its own clauses extended in
//! place; anything this module cannot prove safe to rewrite that way (a set
//! operation, a CTE, or a request that would collide with an `ORDER BY`/
//! `LIMIT` the statement already has) falls back to wrapping the whole
//! thing as a derived table instead — the same fallback
//! `ConsoleService::apply_clauses` used unconditionally before this module
//! existed (`database-tools.md` §11's F3 debt entry).
//!
//! `where_clause`/`order_by` are bare SQL fragments the user typed into the
//! grid's own filter/sort fields — passed through exactly as given, never
//! escaped or validated beyond what the database itself rejects, the same
//! trust boundary every other statement `db-sql` hands to a driver already
//! has (the caller's own read-only guard is what stands between this and a
//! write, not this module).

use db_core::dialect::Dialect;

use crate::dialects::lex_options;
use crate::scan::{scan, Tok};

/// Where a fresh `WHERE (…)` this module adds should go, and what (if
/// anything) already occupies that clause.
struct Plan {
    /// Byte range of an existing top-level `WHERE`'s keyword-to-clause-end
    /// span, if the statement already has one.
    existing_where: Option<(usize, usize)>,
    /// Byte offset to splice a brand new `WHERE` in front of, when there is
    /// no existing one — the first top-level `GROUP`/`HAVING`/`ORDER`/
    /// `LIMIT`, or the trimmed statement's end.
    insert_where_at: usize,
    has_order_by: bool,
    has_limit: bool,
}

/// `None` when `statement` is not a plain `SELECT` this module can safely
/// extend in place — a leading `WITH` (a CTE's outer clauses are not this
/// statement's own top level) or a top-level `UNION`/`INTERSECT`/`EXCEPT`
/// (a set operation's `WHERE` belongs to one arm, not the statement as a
/// whole).
fn plan(statement: &str, dialect: Dialect) -> Option<Plan> {
    let opts = lex_options(dialect);
    let toks = scan(statement, &opts);
    let mut depth: i32 = 0;
    let mut positions: Vec<(usize, usize, String, i32)> = Vec::new();
    for spanned in &toks {
        match &spanned.tok {
            Tok::Symbol('(') => depth += 1,
            Tok::Symbol(')') => depth -= 1,
            Tok::Word(word) => {
                positions.push((spanned.start, spanned.end, word.to_ascii_uppercase(), depth));
            }
            _ => {}
        }
    }
    let first_word = positions.first().map(|(_, _, w, _)| w.as_str());
    if first_word != Some("SELECT") {
        return None; // `WITH ...`, or not a `SELECT` at all.
    }
    let is_top = |depth_at: i32| depth_at == 0;
    let has_set_op = positions
        .iter()
        .any(|(_, _, w, d)| is_top(*d) && matches!(w.as_str(), "UNION" | "INTERSECT" | "EXCEPT"));
    if has_set_op {
        return None;
    }
    let is_later_clause = |w: &str| matches!(w, "GROUP" | "HAVING" | "ORDER" | "LIMIT");
    let mut existing_where = None;
    let mut insert_where_at = None;
    let mut has_order_by = false;
    let mut has_limit = false;
    for (i, (start, _end, word, d)) in positions.iter().enumerate() {
        if !is_top(*d) {
            continue;
        }
        if word == "WHERE" && existing_where.is_none() {
            let clause_end = positions[i + 1..]
                .iter()
                .find(|(_, _, w, d2)| is_top(*d2) && is_later_clause(w))
                .map(|(s, _, _, _)| *s)
                .unwrap_or(statement.trim_end().len());
            existing_where = Some((*start, clause_end));
        }
        if is_later_clause(word) && insert_where_at.is_none() {
            insert_where_at = Some(*start);
        }
        has_order_by |= word == "ORDER";
        has_limit |= word == "LIMIT";
    }
    Some(Plan {
        existing_where,
        insert_where_at: insert_where_at.unwrap_or_else(|| statement.trim_end().len()),
        has_order_by,
        has_limit,
    })
}

/// Applies `where_clause`/`order_by` (either may be empty, meaning "leave
/// that clause alone") to `statement`, in whichever of the two shapes this
/// module's own doc comment describes is safe.
pub fn apply(statement: &str, where_clause: &str, order_by: &str, dialect: Dialect) -> String {
    let where_clause = where_clause.trim();
    let order_by = order_by.trim();
    if where_clause.is_empty() && order_by.is_empty() {
        return statement.to_string();
    }
    let wants_order_by = !order_by.is_empty();
    match plan(statement, dialect) {
        Some(p) if !(wants_order_by && (p.has_order_by || p.has_limit)) => {
            apply_in_place(statement, &p, where_clause, order_by)
        }
        _ => wrap(statement, where_clause, order_by),
    }
}

fn apply_in_place(statement: &str, plan: &Plan, where_clause: &str, order_by: &str) -> String {
    let mut out = if where_clause.is_empty() {
        statement.to_string()
    } else if let Some((start, end)) = plan.existing_where {
        // The keyword `WHERE` itself is 5 bytes; and-ing keeps the existing
        // condition's own text, parenthesised the same as the new one so
        // neither side's precedence bleeds into the other.
        let existing_condition = statement[start + "WHERE".len()..end].trim();
        format!(
            "{}WHERE ({existing_condition}) AND ({where_clause}){}",
            &statement[..start],
            &statement[end..],
        )
    } else {
        let prefix = statement[..plan.insert_where_at].trim_end();
        let suffix = statement[plan.insert_where_at..].trim_start();
        if suffix.is_empty() {
            format!("{prefix} WHERE ({where_clause})")
        } else {
            format!("{prefix} WHERE ({where_clause}) {suffix}")
        }
    };
    if !order_by.is_empty() {
        out = format!("{} ORDER BY {order_by}", out.trim_end());
    }
    out.trim_end().to_string()
}

/// The universal fallback: every mainstream SQL dialect accepts a derived
/// table with an explicit `AS <alias>` (MySQL requires the alias at all;
/// Postgres and SQLite merely allow it), so this needs no per-dialect
/// branch — `dialect` is still a parameter so a future dialect that truly
/// differs has somewhere to plug in without changing every caller.
fn wrap(statement: &str, where_clause: &str, order_by: &str) -> String {
    let mut out = format!("SELECT * FROM ({statement}) AS database_tools_clause_view");
    if !where_clause.is_empty() {
        out.push_str(&format!(" WHERE {where_clause}"));
    }
    if !order_by.is_empty() {
        out.push_str(&format!(" ORDER BY {order_by}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_where_to_a_plain_select_with_no_where() {
        assert_eq!(
            apply("SELECT * FROM t", "x > 1", "", Dialect::Postgres),
            "SELECT * FROM t WHERE (x > 1)"
        );
    }

    #[test]
    fn appends_order_by_to_a_plain_select() {
        assert_eq!(
            apply("SELECT * FROM t", "", "y DESC", Dialect::Postgres),
            "SELECT * FROM t ORDER BY y DESC"
        );
    }

    #[test]
    fn ands_a_new_where_onto_an_existing_one() {
        assert_eq!(
            apply("SELECT * FROM t WHERE a = 1", "b = 2", "", Dialect::MySql),
            "SELECT * FROM t WHERE (a = 1) AND (b = 2)"
        );
    }

    #[test]
    fn inserts_where_before_group_by_and_keeps_it_intact() {
        assert_eq!(
            apply(
                "SELECT a, count(*) FROM t GROUP BY a",
                "a > 1",
                "",
                Dialect::Sqlite
            ),
            "SELECT a, count(*) FROM t WHERE (a > 1) GROUP BY a"
        );
    }

    #[test]
    fn inserts_where_before_order_by_when_only_where_is_requested() {
        assert_eq!(
            apply("SELECT * FROM t ORDER BY a", "a > 1", "", Dialect::Postgres),
            "SELECT * FROM t WHERE (a > 1) ORDER BY a"
        );
    }

    #[test]
    fn wraps_when_a_new_order_by_would_collide_with_an_existing_one() {
        let result = apply(
            "SELECT * FROM t ORDER BY a",
            "",
            "b DESC",
            Dialect::Postgres,
        );
        assert_eq!(
            result,
            "SELECT * FROM (SELECT * FROM t ORDER BY a) AS database_tools_clause_view ORDER BY b DESC"
        );
    }

    #[test]
    fn wraps_when_a_new_order_by_would_collide_with_an_existing_limit() {
        let result = apply("SELECT * FROM t LIMIT 10", "", "b", Dialect::MySql);
        assert!(result.starts_with("SELECT * FROM (SELECT * FROM t LIMIT 10) AS "));
    }

    #[test]
    fn wraps_a_union() {
        let result = apply(
            "SELECT a FROM t UNION SELECT a FROM u",
            "a > 1",
            "",
            Dialect::Postgres,
        );
        assert!(result.starts_with("SELECT * FROM (SELECT a FROM t UNION SELECT a FROM u) AS "));
        assert!(result.ends_with("WHERE a > 1"));
    }

    #[test]
    fn wraps_a_cte() {
        let result = apply(
            "WITH c AS (SELECT 1) SELECT * FROM c",
            "1 = 1",
            "",
            Dialect::Postgres,
        );
        assert!(result.starts_with("SELECT * FROM (WITH c AS"));
    }

    #[test]
    fn a_where_inside_a_subquery_is_not_mistaken_for_the_top_level_one() {
        // The inner `WHERE` sits at depth 1 — the top-level clause is added
        // fresh, not merged with it.
        assert_eq!(
            apply(
                "SELECT * FROM (SELECT * FROM t WHERE x = 1) s",
                "y = 2",
                "",
                Dialect::Postgres
            ),
            "SELECT * FROM (SELECT * FROM t WHERE x = 1) s WHERE (y = 2)"
        );
    }

    #[test]
    fn an_injection_shaped_fragment_is_passed_through_unchanged() {
        // The user's own SQL — not this module's job to sanitise it, only
        // the caller's read-only guard's (this module's own doc comment).
        let fragment = "1=1; DROP TABLE users; --";
        assert_eq!(
            apply("SELECT * FROM t", fragment, "", Dialect::Sqlite),
            format!("SELECT * FROM t WHERE ({fragment})")
        );
    }

    #[test]
    fn no_clauses_requested_returns_the_statement_unchanged() {
        assert_eq!(
            apply("SELECT * FROM t", "", "", Dialect::Postgres),
            "SELECT * FROM t"
        );
    }
}
