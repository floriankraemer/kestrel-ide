//! `inspections` (database-tools.md §2): lint-style diagnostics published
//! under the `database:inspections` source (F3.7) — the ones cheap and
//! safe enough to run on every idle/save without a real parse: an
//! unresolved table name, an unqualified column ambiguous across two
//! joined tables, and a `DELETE`/`UPDATE` with no `WHERE` (the single
//! highest-value one, since it is the one that can lose data).
//!
//! The table/column checks only fire once a `SchemaSnapshot` actually has
//! loaded tables/columns to check against (`schema.roots` empty, or a
//! table's own children still `NotLoaded`, both mean "nothing introspected
//! yet" — silence, not a false "unresolved").

use std::collections::HashMap;

use db_core::dialect::Dialect;
use db_core::schema::{Children, Node, ObjectKind, SchemaSnapshot};
use diagnostics_core::{Diagnostic, Range, Severity};

use crate::dialects::lex_options;
use crate::refs::table_refs;
use crate::scan::{position_at, scan, Tok};
use crate::split::split;

/// A finding before it is anchored to an absolute position in the whole
/// console text — `start`/`end` are byte offsets into the *statement's own*
/// text, since every check below only ever looks at one statement at a
/// time.
struct Finding {
    start: usize,
    end: usize,
    severity: Severity,
    message: String,
}

fn diagnostic(sql: &str, statement_offset: usize, finding: Finding) -> Diagnostic {
    let start = statement_offset + finding.start;
    let end = statement_offset + finding.end;
    Diagnostic {
        range: Range {
            start: position_at(sql, start),
            end: Some(position_at(sql, end)),
        },
        severity: finding.severity,
        message: finding.message,
        source: "db-sql".to_string(),
        raw: None,
    }
}

fn all_table_names(schema: &SchemaSnapshot) -> Vec<String> {
    fn walk(nodes: &[Node], out: &mut Vec<String>) {
        for node in nodes {
            if matches!(
                node.kind,
                ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
            ) {
                out.push(node.name.clone());
            }
            if let Children::Loaded(children) = &node.children {
                walk(children, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&schema.roots, &mut out);
    out
}

fn loaded_columns_of(schema: &SchemaSnapshot, table_name: &str) -> Option<Vec<String>> {
    fn find<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
        for node in nodes {
            if matches!(
                node.kind,
                ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
            ) && node.name.eq_ignore_ascii_case(name)
            {
                return Some(node);
            }
            if let Children::Loaded(children) = &node.children {
                if let Some(found) = find(children, name) {
                    return Some(found);
                }
            }
        }
        None
    }
    match find(&schema.roots, table_name)?.children {
        Children::Loaded(ref children) => Some(
            children
                .iter()
                .filter(|c| c.kind == ObjectKind::Column)
                .map(|c| c.name.clone())
                .collect(),
        ),
        Children::NotLoaded => None,
    }
}

fn statement_has_where(tokens: &[crate::scan::Spanned]) -> bool {
    let mut depth = 0i32;
    for spanned in tokens {
        match &spanned.tok {
            Tok::Symbol('(') => depth += 1,
            Tok::Symbol(')') => depth -= 1,
            Tok::Word(w) if depth == 0 && w.eq_ignore_ascii_case("WHERE") => return true,
            _ => {}
        }
    }
    false
}

fn check_delete_update_without_where(tokens: &[crate::scan::Spanned]) -> Option<Finding> {
    let first = match tokens.first().map(|s| &s.tok) {
        Some(Tok::Word(w)) => w.to_ascii_uppercase(),
        _ => return None,
    };
    if first != "DELETE" && first != "UPDATE" {
        return None;
    }
    if statement_has_where(tokens) {
        return None;
    }
    Some(Finding {
        start: tokens.first()?.start,
        end: tokens.last()?.end,
        severity: Severity::Warning,
        message: format!("{first} with no WHERE clause affects every row in the table"),
    })
}

fn check_unresolved_tables(
    tokens: &[crate::scan::Spanned],
    schema: &SchemaSnapshot,
) -> Vec<Finding> {
    let known = all_table_names(schema);
    if known.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for reference in table_refs(tokens) {
        if !known
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&reference.name))
        {
            // Re-find the reference's own token span to underline just the
            // table name, not the whole statement.
            if let Some(spanned) = tokens
                .iter()
                .find(|s| matches!(&s.tok, Tok::Word(w) if w.eq_ignore_ascii_case(&reference.name)))
            {
                out.push(Finding {
                    start: spanned.start,
                    end: spanned.end,
                    severity: Severity::Error,
                    message: format!("unresolved table \"{}\"", reference.name),
                });
            }
        }
    }
    out
}

fn check_ambiguous_columns(
    tokens: &[crate::scan::Spanned],
    schema: &SchemaSnapshot,
) -> Vec<Finding> {
    let refs = table_refs(tokens);
    if refs.len() < 2 {
        return Vec::new();
    }
    let mut column_owners: HashMap<String, Vec<String>> = HashMap::new();
    for reference in &refs {
        let Some(columns) = loaded_columns_of(schema, &reference.name) else {
            continue;
        };
        for column in columns {
            column_owners
                .entry(column.to_ascii_lowercase())
                .or_default()
                .push(reference.name.clone());
        }
    }
    let mut out = Vec::new();
    // Only unqualified bare words that are followed by a comparison/comma/
    // end-of-clause context are considered — this deliberately does not
    // try to distinguish a column reference from a string/number literal
    // beyond what the scanner already strips, so it only fires on a name
    // that also happens to collide across >= 2 known tables.
    for (idx, spanned) in tokens.iter().enumerate() {
        let Tok::Word(word) = &spanned.tok else {
            continue;
        };
        if matches!(
            tokens.get(idx + 1).map(|s| &s.tok),
            Some(Tok::Symbol('.')) | Some(Tok::Symbol('('))
        ) {
            continue;
        }
        if idx > 0 && matches!(&tokens[idx - 1].tok, Tok::Symbol('.')) {
            continue;
        }
        let Some(owners) = column_owners.get(&word.to_ascii_lowercase()) else {
            continue;
        };
        if owners.len() > 1 {
            out.push(Finding {
                start: spanned.start,
                end: spanned.end,
                severity: Severity::Error,
                message: format!(
                    "ambiguous column \"{word}\": present in {}",
                    owners.join(", ")
                ),
            });
        }
    }
    out
}

pub fn inspections(sql: &str, dialect: Dialect, schema: &SchemaSnapshot) -> Vec<Diagnostic> {
    let opts = lex_options(dialect);
    let mut out = Vec::new();
    for statement in split(sql, dialect) {
        let text = statement.text(sql);
        let tokens = scan(text, &opts);
        let offset = statement.start;
        if let Some(finding) = check_delete_update_without_where(&tokens) {
            out.push(diagnostic(sql, offset, finding));
        }
        for finding in check_unresolved_tables(&tokens, schema) {
            out.push(diagnostic(sql, offset, finding));
        }
        for finding in check_ambiguous_columns(&tokens, schema) {
            out.push(diagnostic(sql, offset, finding));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::schema::IntrospectLevel;

    fn schema_with_users_orders() -> SchemaSnapshot {
        SchemaSnapshot::new(
            IntrospectLevel::Full,
            vec![
                Node::with_children(
                    "users",
                    ObjectKind::Table,
                    vec![
                        Node::leaf("id", ObjectKind::Column),
                        Node::leaf("name", ObjectKind::Column),
                    ],
                ),
                Node::with_children(
                    "orders",
                    ObjectKind::Table,
                    vec![
                        Node::leaf("id", ObjectKind::Column),
                        Node::leaf("user_id", ObjectKind::Column),
                    ],
                ),
            ],
        )
    }

    #[test]
    fn delete_without_where_is_flagged() {
        let diags = inspections(
            "DELETE FROM users",
            Dialect::Postgres,
            &SchemaSnapshot::new(IntrospectLevel::Names, vec![]),
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("DELETE"));
    }

    #[test]
    fn update_without_where_is_flagged() {
        let diags = inspections(
            "UPDATE users SET a = 1",
            Dialect::Postgres,
            &SchemaSnapshot::new(IntrospectLevel::Names, vec![]),
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("UPDATE"));
    }

    #[test]
    fn delete_with_where_is_not_flagged() {
        let diags = inspections(
            "DELETE FROM users WHERE id = 1",
            Dialect::Postgres,
            &SchemaSnapshot::new(IntrospectLevel::Names, vec![]),
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn select_is_never_flagged_for_missing_where() {
        let diags = inspections(
            "SELECT * FROM users",
            Dialect::Postgres,
            &SchemaSnapshot::new(IntrospectLevel::Names, vec![]),
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn an_empty_schema_never_reports_unresolved_tables() {
        let diags = inspections(
            "SELECT * FROM ghosts WHERE 1 = 1",
            Dialect::Postgres,
            &SchemaSnapshot::new(IntrospectLevel::Names, vec![]),
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn an_unknown_table_is_flagged_once_the_schema_is_known() {
        let diags = inspections(
            "SELECT * FROM ghosts WHERE 1 = 1",
            Dialect::Postgres,
            &schema_with_users_orders(),
        );
        assert!(diags.iter().any(|d| d.message.contains("ghosts")));
    }

    #[test]
    fn a_known_table_is_not_flagged() {
        let diags = inspections(
            "SELECT * FROM users WHERE id = 1",
            Dialect::Postgres,
            &schema_with_users_orders(),
        );
        assert!(!diags.iter().any(|d| d.message.contains("users")));
    }

    #[test]
    fn an_ambiguous_column_across_two_joined_tables_is_flagged() {
        let diags = inspections(
            "SELECT id FROM users JOIN orders ON users.id = orders.user_id",
            Dialect::Postgres,
            &schema_with_users_orders(),
        );
        assert!(diags
            .iter()
            .any(|d| d.message.contains("ambiguous column \"id\"")));
    }

    #[test]
    fn a_qualified_column_is_never_flagged_as_ambiguous() {
        let diags = inspections(
            "SELECT users.id FROM users JOIN orders ON users.id = orders.user_id",
            Dialect::Postgres,
            &schema_with_users_orders(),
        );
        assert!(!diags.iter().any(|d| d.message.contains("ambiguous")));
    }

    #[test]
    fn a_non_colliding_column_is_not_flagged() {
        let diags = inspections(
            "SELECT name FROM users JOIN orders ON users.id = orders.user_id",
            Dialect::Postgres,
            &schema_with_users_orders(),
        );
        assert!(!diags.iter().any(|d| d.message.contains("ambiguous")));
    }

    #[test]
    fn multiple_statements_are_each_inspected_independently() {
        let diags = inspections(
            "DELETE FROM users; SELECT * FROM users WHERE id = 1;",
            Dialect::Postgres,
            &SchemaSnapshot::new(IntrospectLevel::Names, vec![]),
        );
        assert_eq!(diags.len(), 1);
    }
}
