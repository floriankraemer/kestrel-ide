//! `navigation` (database-tools.md §2): the identifier under the caret,
//! as an `ObjectRef` — what "Go to DDL" (F3.7) resolves against. Reads
//! only the qualifier chain immediately in front of the word (`schema.`,
//! `catalog.schema.`); it does not need `table_refs`' alias resolution
//! because "Go to DDL" always targets the identifier actually written,
//! never an alias.

use db_core::schema::{ObjectKind, ObjectRef, SchemaSnapshot};

use crate::scan::word_at;

/// Walks backwards from `word_start`, collecting `.`-separated
/// identifiers that immediately precede it (no whitespace) — `a.b.c` when
/// the caret is on `c`.
fn qualifiers_before(sql: &str, word_start: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut pos = word_start;
    loop {
        if pos == 0 || !sql[..pos].ends_with('.') {
            break;
        }
        let dot = pos - 1;
        let Some((start, end, word)) = word_at(sql, dot) else {
            break;
        };
        if end != dot {
            break;
        }
        parts.push(word);
        pos = start;
    }
    parts.reverse();
    parts
}

/// Finds a node named `name` anywhere in `schema`'s tree (only descending
/// into already-`Loaded` children — this never triggers a fetch), and
/// returns its kind if found.
fn find_kind(schema: &SchemaSnapshot, name: &str) -> Option<ObjectKind> {
    fn walk(nodes: &[db_core::schema::Node], name: &str) -> Option<ObjectKind> {
        for node in nodes {
            if node.name.eq_ignore_ascii_case(name) {
                return Some(node.kind);
            }
            if let db_core::schema::Children::Loaded(children) = &node.children {
                if let Some(kind) = walk(children, name) {
                    return Some(kind);
                }
            }
        }
        None
    }
    walk(&schema.roots, name)
}

/// The identifier under `offset`, as an `ObjectRef` — `None` if the caret
/// is not on an identifier at all.
pub fn navigate(sql: &str, offset: usize, schema: Option<&SchemaSnapshot>) -> Option<ObjectRef> {
    let (start, _end, word) = word_at(sql, offset)?;
    let qualifiers = qualifiers_before(sql, start);
    let mut reference = ObjectRef::new(word.clone());
    match qualifiers.len() {
        0 => {}
        1 => reference = reference.with_schema(qualifiers[0].clone()),
        _ => {
            reference = reference
                .with_catalog(qualifiers[qualifiers.len() - 2].clone())
                .with_schema(qualifiers[qualifiers.len() - 1].clone());
        }
    }
    if let Some(schema) = schema {
        if let Some(kind) = find_kind(schema, &word) {
            reference = reference.with_kind(kind);
        }
    }
    Some(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::schema::Node;

    fn snapshot() -> SchemaSnapshot {
        SchemaSnapshot::new(
            db_core::schema::IntrospectLevel::Names,
            vec![Node::with_children(
                "public",
                ObjectKind::Schema,
                vec![Node::leaf("users", ObjectKind::Table)],
            )],
        )
    }

    #[test]
    fn a_bare_identifier_becomes_an_unqualified_object_ref() {
        let sql = "SELECT * FROM users";
        let offset = sql.find("users").unwrap() + 2;
        let reference = navigate(sql, offset, None).unwrap();
        assert_eq!(reference.name, "users");
        assert_eq!(reference.schema, None);
    }

    #[test]
    fn a_schema_qualified_identifier_carries_its_schema() {
        let sql = "SELECT * FROM public.users";
        let offset = sql.find("users").unwrap() + 2;
        let reference = navigate(sql, offset, None).unwrap();
        assert_eq!(reference.name, "users");
        assert_eq!(reference.schema.as_deref(), Some("public"));
    }

    #[test]
    fn a_catalog_schema_qualified_identifier_carries_both() {
        let sql = "SELECT * FROM mydb.public.users";
        let offset = sql.find("users").unwrap() + 2;
        let reference = navigate(sql, offset, None).unwrap();
        assert_eq!(reference.catalog.as_deref(), Some("mydb"));
        assert_eq!(reference.schema.as_deref(), Some("public"));
    }

    #[test]
    fn caret_outside_any_identifier_is_none() {
        let sql = "SELECT * FROM users";
        // offset 8 sits in the whitespace between `*` and `FROM`.
        assert_eq!(navigate(sql, 8, None), None);
    }

    #[test]
    fn the_kind_is_filled_in_from_a_matching_schema_node() {
        let sql = "SELECT * FROM users";
        let offset = sql.find("users").unwrap() + 2;
        let reference = navigate(sql, offset, Some(&snapshot())).unwrap();
        assert_eq!(reference.kind, Some(ObjectKind::Table));
    }

    #[test]
    fn the_kind_is_none_when_nothing_in_the_schema_matches() {
        let sql = "SELECT * FROM ghosts";
        let offset = sql.find("ghosts").unwrap() + 2;
        let reference = navigate(sql, offset, Some(&snapshot())).unwrap();
        assert_eq!(reference.kind, None);
    }
}
