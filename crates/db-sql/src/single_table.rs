//! Whether one statement is a plain "every row/column of one table"
//! `SELECT` — the data editor's own editability gate (F4.1): a result is
//! only offered as an editable grid when it maps 1:1 onto exactly one
//! table's rows and columns, so a staged cell edit's column name always
//! names a real column of a real table (`db_core::dml::EditBuffer`'s own
//! assumption).
//!
//! `db_core::value::ColumnMeta::origin` is where this answer is *meant*
//! to come from long-term (a driver's own per-column table origin, e.g.
//! Postgres's row-description `table_oid`) — but `rusqlite` 0.32 exposes
//! no such API at all (checked: no `column_metadata`-shaped feature
//! exists in its feature list), so no driver in this codebase populates
//! `origin` yet. This module is `ui-shell`'s stand-in until a driver
//! does: the same statement text every driver already has in hand,
//! parsed once through `sqlparser` rather than a second hand-rolled
//! scanner.
//!
//! Deliberately conservative: a `JOIN`, a derived table, `DISTINCT`,
//! `GROUP BY`/`HAVING`, a set operation (`UNION`/`INTERSECT`/…), or any
//! projected item that is not a bare column reference (an expression, an
//! aggregate, a renaming alias) all answer `None` — the grid falls back
//! to read-only rather than risk staging an edit against a column that is
//! not really the table's own.

use db_core::dialect::Dialect;
use sqlparser::ast::{
    Expr, GroupByExpr, ObjectName, Query, SelectItem, SetExpr, Statement, TableFactor,
};
use sqlparser::parser::Parser;

use crate::dialects::sqlparser_dialect;

/// The one table a statement's rows/columns map onto 1:1, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

/// `sql` is exactly one statement's text (already split by
/// [`crate::split`]) — a multi-statement blob is never passed here.
pub fn of(sql: &str, dialect: Dialect) -> Option<TableRef> {
    let grammar = sqlparser_dialect(dialect);
    let statements = Parser::parse_sql(&*grammar, sql).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    single_table_of_query(query)
}

fn single_table_of_query(query: &Query) -> Option<TableRef> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    if select.distinct.is_some() {
        return None;
    }
    if select.having.is_some() {
        return None;
    }
    match &select.group_by {
        GroupByExpr::Expressions(exprs, modifiers) => {
            if !exprs.is_empty() || !modifiers.is_empty() {
                return None;
            }
        }
        GroupByExpr::All(_) => return None,
    }
    if select.from.len() != 1 {
        return None;
    }
    let table_with_joins = &select.from[0];
    if !table_with_joins.joins.is_empty() {
        return None;
    }
    let TableFactor::Table { name, .. } = &table_with_joins.relation else {
        return None;
    };
    if !select.projection.iter().all(is_plain_projection) {
        return None;
    }
    Some(table_ref_of(name))
}

fn is_plain_projection(item: &SelectItem) -> bool {
    matches!(
        item,
        SelectItem::Wildcard(_)
            | SelectItem::QualifiedWildcard(_, _)
            | SelectItem::UnnamedExpr(Expr::Identifier(_) | Expr::CompoundIdentifier(_))
    )
}

fn table_ref_of(name: &ObjectName) -> TableRef {
    let parts: Vec<String> = name
        .0
        .iter()
        .filter_map(|part| part.as_ident())
        .map(|ident| ident.value.clone())
        .collect();
    match parts.split_last() {
        Some((table, rest)) if !rest.is_empty() => TableRef {
            schema: Some(rest.join(".")),
            name: table.clone(),
        },
        Some((table, _)) => TableRef {
            schema: None,
            name: table.clone(),
        },
        None => TableRef {
            schema: None,
            name: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_select_star_is_a_single_table() {
        let table = of("SELECT * FROM users", Dialect::Postgres).unwrap();
        assert_eq!(table.name, "users");
        assert_eq!(table.schema, None);
    }

    #[test]
    fn a_schema_qualified_table_carries_its_schema() {
        let table = of("SELECT * FROM public.users", Dialect::Postgres).unwrap();
        assert_eq!(table.name, "users");
        assert_eq!(table.schema.as_deref(), Some("public"));
    }

    #[test]
    fn an_explicit_column_list_of_bare_identifiers_still_counts() {
        let table = of("SELECT id, name FROM users", Dialect::Postgres).unwrap();
        assert_eq!(table.name, "users");
    }

    #[test]
    fn a_where_clause_does_not_disqualify_it() {
        assert!(of("SELECT * FROM users WHERE id = 1", Dialect::Postgres).is_some());
    }

    #[test]
    fn a_join_is_not_a_single_table() {
        assert!(of(
            "SELECT * FROM users JOIN orders ON users.id = orders.user_id",
            Dialect::Postgres
        )
        .is_none());
    }

    #[test]
    fn a_derived_table_is_not_a_single_table() {
        assert!(of("SELECT * FROM (SELECT 1) t", Dialect::Postgres).is_none());
    }

    #[test]
    fn distinct_disqualifies_it() {
        assert!(of("SELECT DISTINCT name FROM users", Dialect::Postgres).is_none());
    }

    #[test]
    fn group_by_disqualifies_it() {
        assert!(of("SELECT name FROM users GROUP BY name", Dialect::Postgres).is_none());
    }

    #[test]
    fn a_computed_projection_disqualifies_it() {
        assert!(of("SELECT count(*) FROM users", Dialect::Postgres).is_none());
        assert!(of("SELECT id + 1 FROM users", Dialect::Postgres).is_none());
    }

    #[test]
    fn a_set_operation_is_not_a_single_table() {
        assert!(of(
            "SELECT * FROM users UNION SELECT * FROM admins",
            Dialect::Postgres
        )
        .is_none());
    }

    #[test]
    fn a_non_query_statement_is_none() {
        assert!(of("INSERT INTO users VALUES (1)", Dialect::Postgres).is_none());
    }

    #[test]
    fn an_unparseable_statement_is_none() {
        assert!(of("SELEKT * FORM users", Dialect::Postgres).is_none());
    }

    #[test]
    fn a_dialect_specific_quoted_identifier_still_resolves() {
        let table = of("SELECT * FROM `users`", Dialect::MySql).unwrap();
        assert_eq!(table.name, "users");
    }
}
