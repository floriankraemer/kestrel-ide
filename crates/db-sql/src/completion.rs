//! `completion` (database-tools.md §2): schema-aware completion for
//! `bridge/language/database.rs`'s `database_completion` (F3.7).
//! Two-tier context: after `FROM`/`JOIN`/`INTO`/`UPDATE` suggests
//! tables/views; after `SELECT`/`WHERE`/`ON`/`SET` suggests columns of
//! whatever tables are in scope (resolved through [`crate::refs`]'s alias
//! map, so `u.` after `FROM users u` narrows to `users`' own columns).
//! Falls back to dialect keywords everywhere else.

use db_core::dialect::Dialect;
use db_core::schema::{Children, Node, ObjectKind, SchemaSnapshot};

use crate::dialects::lex_options;
use crate::refs::table_refs;
use crate::scan::{scan, Tok};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Table,
    View,
    Column,
    Keyword,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// Present for a `Column` item qualified by the table it belongs to
    /// (`users.id`), so two same-named columns from different joined
    /// tables stay distinguishable in the popup.
    pub detail: Option<String>,
}

/// Which position-in-statement the completion request sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    TableName,
    ColumnName,
    Other,
}

const TABLE_POSITION_KEYWORDS: &[&str] = &["FROM", "JOIN", "INTO", "UPDATE"];
const COLUMN_POSITION_KEYWORDS: &[&str] = &["SELECT", "WHERE", "ON", "SET", "AND", "OR", "BY"];

/// The nearest keyword at or before `offset` among the ones that fix a
/// completion context, scanning the *whole* text (not stopping at
/// `offset`'s statement) since a later table can still be the answer for
/// e.g. an `UPDATE ... SET col = 1 WHERE <here>` column position.
fn context_at(tokens: &[crate::scan::Spanned], offset: usize) -> Context {
    let mut found = Context::Other;
    for spanned in tokens {
        if spanned.start > offset {
            break;
        }
        if let Tok::Word(word) = &spanned.tok {
            let upper = word.to_ascii_uppercase();
            if TABLE_POSITION_KEYWORDS.contains(&upper.as_str()) {
                found = Context::TableName;
            } else if COLUMN_POSITION_KEYWORDS.contains(&upper.as_str()) {
                found = Context::ColumnName;
            }
        }
    }
    found
}

/// The identifier prefix already typed immediately before `offset`
/// (completion's "narrow to what's typed so far"), and, when it is
/// qualified (`alias.partial`), the qualifier in front of the `.`.
fn prefix_before(sql: &str, offset: usize) -> (Option<String>, String) {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let word_start = sql[..offset.min(sql.len())]
        .rfind(|c: char| !is_word_char(c))
        .map(|p| p + 1)
        .unwrap_or(0);
    let typed = sql[word_start..offset.min(sql.len())].to_string();
    if word_start > 0 && sql[..word_start].ends_with('.') {
        let dot = word_start - 1;
        let qual_start = sql[..dot]
            .rfind(|c: char| !is_word_char(c))
            .map(|p| p + 1)
            .unwrap_or(0);
        return (Some(sql[qual_start..dot].to_string()), typed);
    }
    (None, typed)
}

fn table_and_view_names(schema: &SchemaSnapshot) -> Vec<String> {
    fn walk(nodes: &[Node], out: &mut Vec<String>) {
        for node in nodes {
            if matches!(
                node.kind,
                ObjectKind::Table
                    | ObjectKind::View
                    | ObjectKind::MaterializedView
                    | ObjectKind::Collection
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

/// Mongo console completion (F7b): collection names after `db.`, sugar
/// method names (`db_sql::mongo::SUGAR_METHODS`) after `db.<collection>.`
/// — no keyword fallback (`db_sql::dialects`' generic SQL keyword list
/// means nothing to a Mongo console).
fn mongo_completion(text: &str, offset: usize, schema: &SchemaSnapshot) -> Vec<CompletionItem> {
    let (qualifier, prefix) = prefix_before(text, offset);
    let Some(qualifier) = qualifier else {
        return Vec::new();
    };
    if qualifier == "db" {
        return rank_and_wrap(table_and_view_names(schema), &prefix, CompletionKind::Table, None);
    }
    // Method position only when the text right before `<qualifier>.`
    // literally reads `db.` — `head.ends_with(...)` sidesteps re-deriving
    // `prefix_before`'s own word-boundary scan a second level back.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let before = &text[..offset.min(text.len())];
    let word_start = before
        .rfind(|c: char| !is_word_char(c))
        .map(|p| p + 1)
        .unwrap_or(0);
    let head = &before[..word_start];
    if head.ends_with(&format!("db.{qualifier}.")) {
        return rank_and_wrap(
            crate::mongo::SUGAR_METHODS
                .iter()
                .map(|m| m.to_string())
                .collect(),
            &prefix,
            CompletionKind::Keyword,
            None,
        );
    }
    Vec::new()
}

/// Redis console completion (F7b): command names
/// (`db_sql::resp::command_names`) plus key names from the snapshot's own
/// `KeyNamespace`/`Key` nodes — both ranked together against whatever
/// prefix is typed, since a Redis line has no fixed "command position vs.
/// key position" grammar the way SQL's `FROM`/`SELECT` does.
fn redis_completion(text: &str, offset: usize, schema: &SchemaSnapshot) -> Vec<CompletionItem> {
    let (_, prefix) = prefix_before(text, offset);
    let mut items = rank_and_wrap(
        crate::resp::command_names()
            .into_iter()
            .map(str::to_string)
            .collect(),
        &prefix,
        CompletionKind::Keyword,
        None,
    );
    items.extend(rank_and_wrap(
        key_names(schema),
        &prefix,
        CompletionKind::Table,
        None,
    ));
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

fn key_names(schema: &SchemaSnapshot) -> Vec<String> {
    fn walk(nodes: &[Node], out: &mut Vec<String>) {
        for node in nodes {
            if matches!(
                node.kind,
                ObjectKind::KeyNamespace | ObjectKind::Key(_)
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

/// Ranks every `candidate` against `prefix` (dropping non-matches),
/// wrapping each survivor as a `kind`-tagged [`CompletionItem`] — the
/// small piece [`mongo_completion`]/[`redis_completion`] share so neither
/// hand-rolls the same rank-then-wrap loop [`completion`]'s own SQL path
/// already has inline.
fn rank_and_wrap(
    candidates: Vec<String>,
    prefix: &str,
    kind: CompletionKind,
    detail: Option<String>,
) -> Vec<CompletionItem> {
    let mut items: Vec<(u8, CompletionItem)> = candidates
        .into_iter()
        .filter_map(|label| {
            rank(&label, prefix).map(|r| {
                (
                    r,
                    CompletionItem {
                        label,
                        kind,
                        detail: detail.clone(),
                    },
                )
            })
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    items.into_iter().map(|(_, item)| item).collect()
}

fn columns_of(schema: &SchemaSnapshot, table_name: &str) -> Vec<String> {
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
    let Some(table) = find(&schema.roots, table_name) else {
        return Vec::new();
    };
    match &table.children {
        Children::Loaded(children) => children
            .iter()
            .filter(|c| c.kind == ObjectKind::Column)
            .map(|c| c.name.clone())
            .collect(),
        Children::NotLoaded => Vec::new(),
    }
}

/// `0` for an exact-prefix match, `1` for a contains-only match — the
/// ranking `database-tools.md` §2 asks for, applied as a stable sort key
/// (ties keep alphabetical order via the label itself).
fn rank(label: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return Some(1);
    }
    let lower_label = label.to_ascii_lowercase();
    let lower_prefix = prefix.to_ascii_lowercase();
    if lower_label.starts_with(&lower_prefix) {
        Some(0)
    } else if lower_label.contains(&lower_prefix) {
        Some(1)
    } else {
        None
    }
}

pub fn completion(
    text: &str,
    offset: usize,
    schema: &SchemaSnapshot,
    dialect: Dialect,
) -> Vec<CompletionItem> {
    match dialect {
        Dialect::Mongo => return mongo_completion(text, offset, schema),
        Dialect::Redis => return redis_completion(text, offset, schema),
        _ => {}
    }
    let opts = lex_options(dialect);
    let tokens = scan(text, &opts);
    let context = context_at(&tokens, offset);
    let (qualifier, prefix) = prefix_before(text, offset);

    let mut items: Vec<(u8, CompletionItem)> = Vec::new();

    match context {
        Context::TableName => {
            for name in table_and_view_names(schema) {
                if let Some(r) = rank(&name, &prefix) {
                    items.push((
                        r,
                        CompletionItem {
                            label: name,
                            kind: CompletionKind::Table,
                            detail: None,
                        },
                    ));
                }
            }
        }
        Context::ColumnName => {
            let refs = table_refs(&tokens);
            let candidate_tables: Vec<&str> = if let Some(qualifier) = &qualifier {
                refs.iter()
                    .filter(|r| {
                        r.alias.as_deref() == Some(qualifier.as_str()) || r.name == *qualifier
                    })
                    .map(|r| r.name.as_str())
                    .collect()
            } else {
                refs.iter().map(|r| r.name.as_str()).collect()
            };
            for table in candidate_tables {
                for column in columns_of(schema, table) {
                    if let Some(r) = rank(&column, &prefix) {
                        items.push((
                            r,
                            CompletionItem {
                                label: column,
                                kind: CompletionKind::Column,
                                detail: Some(table.to_string()),
                            },
                        ));
                    }
                }
            }
        }
        Context::Other => {}
    }

    if qualifier.is_none() {
        for keyword in sqlparser::keywords::ALL_KEYWORDS {
            if let Some(r) = rank(keyword, &prefix) {
                items.push((
                    r.saturating_add(1),
                    CompletionItem {
                        label: keyword.to_string(),
                        kind: CompletionKind::Keyword,
                        detail: None,
                    },
                ));
            }
        }
    }

    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    items.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::schema::IntrospectLevel;

    fn users_orders_schema() -> SchemaSnapshot {
        SchemaSnapshot::new(
            IntrospectLevel::Full,
            vec![Node::with_children(
                "public",
                ObjectKind::Schema,
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
            )],
        )
    }

    #[test]
    fn after_from_suggests_tables() {
        let schema = users_orders_schema();
        let sql = "SELECT * FROM ";
        let items = completion(sql, sql.len(), &schema, Dialect::Postgres);
        assert!(items
            .iter()
            .any(|i| i.label == "users" && i.kind == CompletionKind::Table));
        assert!(items
            .iter()
            .any(|i| i.label == "orders" && i.kind == CompletionKind::Table));
    }

    #[test]
    fn after_select_suggests_columns_of_the_from_table() {
        let schema = users_orders_schema();
        let sql = "SELECT  FROM users";
        let items = completion(sql, 7, &schema, Dialect::Postgres);
        let cols: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::Column)
            .map(|i| i.label.as_str())
            .collect();
        assert!(cols.contains(&"id"));
        assert!(cols.contains(&"name"));
        assert!(!cols.contains(&"user_id"));
    }

    #[test]
    fn a_qualified_prefix_narrows_to_one_tables_columns() {
        let schema = users_orders_schema();
        let sql = "SELECT u. FROM users u, orders o";
        let offset = sql.find("u.").unwrap() + 2;
        let items = completion(sql, offset, &schema, Dialect::Postgres);
        let cols: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::Column)
            .map(|i| i.label.as_str())
            .collect();
        assert!(cols.contains(&"id"));
        assert!(cols.contains(&"name"));
        assert!(!cols.contains(&"user_id"));
    }

    #[test]
    fn where_position_also_suggests_columns() {
        let schema = users_orders_schema();
        let sql = "SELECT * FROM users WHERE ";
        let items = completion(sql, sql.len(), &schema, Dialect::Postgres);
        assert!(items
            .iter()
            .any(|i| i.label == "id" && i.kind == CompletionKind::Column));
    }

    #[test]
    fn exact_prefix_ranks_before_contains_only() {
        let schema = users_orders_schema();
        let sql = "SELECT * FROM us";
        let items = completion(sql, sql.len(), &schema, Dialect::Postgres);
        let names: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::Table)
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(names.first(), Some(&"users"));
    }

    #[test]
    fn keywords_are_offered_and_ranked_after_schema_matches() {
        let schema = users_orders_schema();
        let sql = "SEL";
        let items = completion(sql, sql.len(), &schema, Dialect::Postgres);
        assert!(items.iter().any(|i| i.label == "SELECT"));
    }

    #[test]
    fn an_empty_schema_still_offers_keywords() {
        let schema = SchemaSnapshot::new(IntrospectLevel::Names, vec![]);
        let items = completion("SEL", 3, &schema, Dialect::Postgres);
        assert!(items.iter().any(|i| i.label == "SELECT"));
    }

    fn mongo_schema() -> SchemaSnapshot {
        SchemaSnapshot::new(
            IntrospectLevel::Names,
            vec![Node::leaf("users", ObjectKind::Collection)],
        )
    }

    #[test]
    fn after_db_dot_suggests_collection_names() {
        let schema = mongo_schema();
        let text = "db.us";
        let items = completion(text, text.len(), &schema, Dialect::Mongo);
        assert!(items.iter().any(|i| i.label == "users"));
    }

    #[test]
    fn after_db_collection_dot_suggests_sugar_methods() {
        let schema = mongo_schema();
        let text = "db.users.fi";
        let items = completion(text, text.len(), &schema, Dialect::Mongo);
        assert!(items.iter().any(|i| i.label == "find"));
        assert!(items.iter().any(|i| i.label == "findOne"));
        // No SQL keyword leakage onto a Mongo console.
        assert!(!items.iter().any(|i| i.label == "SELECT"));
    }

    #[test]
    fn mongo_completion_outside_a_db_call_is_empty() {
        let schema = mongo_schema();
        let text = "{ping: 1}";
        assert!(completion(text, text.len(), &schema, Dialect::Mongo).is_empty());
    }

    fn redis_schema() -> SchemaSnapshot {
        SchemaSnapshot::new(
            IntrospectLevel::Names,
            vec![Node::leaf("session:42", ObjectKind::Key(db_core::schema::RedisType::String))],
        )
    }

    #[test]
    fn redis_completion_offers_command_names() {
        let schema = redis_schema();
        let text = "GE";
        let items = completion(text, text.len(), &schema, Dialect::Redis);
        assert!(items.iter().any(|i| i.label == "GET"));
    }

    #[test]
    fn redis_completion_offers_key_names_from_the_snapshot() {
        let schema = redis_schema();
        let text = "GET sess";
        let items = completion(text, text.len(), &schema, Dialect::Redis);
        assert!(items.iter().any(|i| i.label == "session:42"));
    }
}
