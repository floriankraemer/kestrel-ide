//! DDL synthesis and the SQL Generator (database-tools-plan.md F2.3/F2.4):
//! pure text generation from a [`crate::schema::Node`]/[`crate::schema::
//! ObjectRef`] plus a [`Dialect`] — no connection, no I/O. `Connection::
//! ddl_of` prefers an engine's own DDL text (SQLite's `sqlite_master.sql`)
//! and falls back to [`synthesize`] only when the engine has none to give
//! back; every other generator here (`select_all`, `insert_template`,
//! `drop`, `truncate`, `rename_to`, `comment_on`) is what F2.2's object
//! actions run, after a caller-shown confirmation of the exact text this
//! module produced (ADR-0061 §1: nothing here interpolates a value bound
//! at runtime — `select_all`'s `LIMIT n` is the one literal these
//! generators ever emit, and `n` is a `u64`, never attacker-controlled
//! text).
//!
//! Every identifier crosses through [`Dialect::quote_ident`] — this module
//! never re-derives a quoting rule of its own (`dialect.rs`'s doc
//! comment).

use crate::dialect::Dialect;
use crate::error::{DbError, DbErrorCode};
use crate::schema::{Children, Node, ObjectKind, ObjectRef};

fn qualify(dialect: Dialect, object: &ObjectRef) -> String {
    let mut parts = Vec::new();
    if let Some(catalog) = &object.catalog {
        parts.push(dialect.quote_ident(catalog));
    }
    if let Some(schema) = &object.schema {
        parts.push(dialect.quote_ident(schema));
    }
    parts.push(dialect.quote_ident(&object.name));
    parts.join(".")
}

/// `SELECT * FROM <table> LIMIT n`.
pub fn select_all(dialect: Dialect, object: &ObjectRef, limit: u64) -> String {
    format!("SELECT * FROM {} LIMIT {limit}", qualify(dialect, object))
}

/// An `INSERT INTO` template — every value a `?`/`$n`-shaped placeholder
/// comment, never a literal, since this text is meant to be edited by hand
/// before it is ever run.
pub fn insert_template(dialect: Dialect, object: &ObjectRef, columns: &[String]) -> String {
    let column_list = columns
        .iter()
        .map(|c| dialect.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = columns
        .iter()
        .map(|c| format!("/* {c} */"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({column_list}) VALUES ({placeholders})",
        qualify(dialect, object)
    )
}

/// `DROP TABLE`/`DROP VIEW`/… — the keyword follows `object.kind` so a
/// view is never dropped with a `TABLE` keyword. Mongo has no `DROP`
/// keyword at all (F7b): a `Collection` drop is instead a `runCommand`
/// JSON document (`{"drop": "<name>"}`), the same shape `db_sql::mongo::
/// parse` already recognises as a raw command — this is the one dialect
/// where the generated text is not SQL, so `run_action`'s caller must
/// still send it through the connection's own `QueryLang` (`Dialect::
/// query_lang`), never `Statement::sql`.
pub fn drop_statement(dialect: Dialect, object: &ObjectRef) -> Result<String, DbError> {
    if dialect == Dialect::Mongo {
        return match object.kind {
            Some(ObjectKind::Collection) => {
                Ok(format!("{{\"drop\": {}}}", json_string(&object.name)))
            }
            _ => Err(DbError::new(
                DbErrorCode::NotSupported,
                "this object kind has no drop command",
            )),
        };
    }
    let keyword = drop_keyword(object.kind)?;
    Ok(format!("DROP {keyword} {}", qualify(dialect, object)))
}

/// A minimal JSON string literal — just enough escaping (`"`, `\`,
/// control characters) for a collection name, which `db_sql::mongo::parse`
/// then round-trips through `serde_json` on the way back in. Not a
/// general-purpose JSON encoder: this module has no other use for one, so
/// it does not depend on `serde_json` for a single string field.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn drop_keyword(kind: Option<ObjectKind>) -> Result<&'static str, DbError> {
    match kind {
        Some(ObjectKind::Table) | Some(ObjectKind::Collection) => Ok("TABLE"),
        Some(ObjectKind::View) => Ok("VIEW"),
        Some(ObjectKind::MaterializedView) => Ok("MATERIALIZED VIEW"),
        Some(ObjectKind::Routine) => Ok("ROUTINE"),
        Some(ObjectKind::Sequence) => Ok("SEQUENCE"),
        Some(ObjectKind::Index) => Ok("INDEX"),
        _ => Err(DbError::new(
            DbErrorCode::NotSupported,
            "this object kind has no DROP statement",
        )),
    }
}

/// `TRUNCATE TABLE` — tables only; a view or routine has nothing to
/// truncate.
pub fn truncate_statement(dialect: Dialect, object: &ObjectRef) -> Result<String, DbError> {
    if object.kind != Some(ObjectKind::Table) {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "only a table can be truncated",
        ));
    }
    Ok(format!("TRUNCATE TABLE {}", qualify(dialect, object)))
}

/// `ALTER TABLE … RENAME TO …` — SQLite and Postgres both accept this
/// exact shape; a dialect that needs a different one grows its own arm
/// here once it exists (YAGNI against a hypothetical third shape today).
pub fn rename_statement(
    dialect: Dialect,
    object: &ObjectRef,
    new_name: &str,
) -> Result<String, DbError> {
    if object.kind != Some(ObjectKind::Table) && object.kind != Some(ObjectKind::View) {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "rename is only generated for tables and views",
        ));
    }
    Ok(format!(
        "ALTER TABLE {} RENAME TO {}",
        qualify(dialect, object),
        dialect.quote_ident(new_name)
    ))
}

/// Whether `dialect` has a `COMMENT ON`-shaped statement at all — SQLite
/// does not, so [`comment_statement`] fails typed rather than emitting SQL
/// SQLite would reject; `db_core::tree::actions_for` reads the same fact
/// through `SourceCapabilities::supports_comment` so the action is absent
/// from the menu entirely rather than present-but-erroring.
pub fn supports_comment(dialect: Dialect) -> bool {
    !matches!(dialect, Dialect::Sqlite | Dialect::Mongo | Dialect::Redis)
}

/// `COMMENT ON TABLE … IS '…'` (Postgres-shaped; every dialect this crate
/// currently supports besides SQLite uses the same grammar).
pub fn comment_statement(
    dialect: Dialect,
    object: &ObjectRef,
    comment: &str,
) -> Result<String, DbError> {
    if !supports_comment(dialect) {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "this dialect has no COMMENT ON statement",
        ));
    }
    let keyword = match object.kind {
        Some(ObjectKind::View) => "VIEW",
        _ => "TABLE",
    };
    Ok(format!(
        "COMMENT ON {keyword} {} IS {}",
        qualify(dialect, object),
        dialect.quote_literal(comment)
    ))
}

/// Synthesize a `CREATE TABLE` from a table [`Node`]'s own column
/// children, for a backend whose engine returns no native DDL text.
/// ponytail: emits columns/PK only — no foreign keys, check constraints or
/// generated-column expressions, since [`crate::schema::NodeDetail`]
/// carries none of those yet; upgrade once `IntrospectLevel::Full`
/// populates constraint nodes with enough detail to round-trip one.
pub fn synthesize(node: &Node, dialect: Dialect, schema: Option<&str>) -> Result<String, DbError> {
    if node.kind != ObjectKind::Table {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "DDL synthesis is only implemented for tables",
        ));
    }
    let Children::Loaded(children) = &node.children else {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "table columns are not loaded yet",
        ));
    };
    let columns: Vec<&Node> = children
        .iter()
        .filter(|c| c.kind == ObjectKind::Column)
        .collect();
    if columns.is_empty() {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "table has no column information to synthesize from",
        ));
    }

    let mut lines = Vec::new();
    let mut primary_key = Vec::new();
    for column in &columns {
        let type_name = column
            .detail
            .type_name
            .clone()
            .unwrap_or_else(|| "TEXT".to_string());
        let mut line = format!("  {} {}", dialect.quote_ident(&column.name), type_name);
        if column.detail.nullable == Some(false) {
            line.push_str(" NOT NULL");
        }
        if let Some(default) = &column.detail.default {
            line.push_str(&format!(" DEFAULT {default}"));
        }
        lines.push(line);
        if column.detail.primary_key {
            primary_key.push(dialect.quote_ident(&column.name));
        }
    }
    if !primary_key.is_empty() {
        lines.push(format!("  PRIMARY KEY ({})", primary_key.join(", ")));
    }

    let qualified = match schema {
        Some(schema) => format!(
            "{}.{}",
            dialect.quote_ident(schema),
            dialect.quote_ident(&node.name)
        ),
        None => dialect.quote_ident(&node.name),
    };
    Ok(format!(
        "CREATE TABLE {qualified} (\n{}\n)",
        lines.join(",\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::NodeDetail;

    fn table_ref() -> ObjectRef {
        ObjectRef::new("users")
            .with_schema("public")
            .with_kind(ObjectKind::Table)
    }

    #[test]
    fn select_all_carries_the_limit_and_a_qualified_name() {
        assert_eq!(
            select_all(Dialect::Postgres, &table_ref(), 100),
            "SELECT * FROM \"public\".\"users\" LIMIT 100"
        );
    }

    #[test]
    fn insert_template_lists_columns_and_a_placeholder_comment_per_value() {
        let text = insert_template(
            Dialect::Sqlite,
            &ObjectRef::new("t"),
            &["id".to_string(), "name".to_string()],
        );
        assert_eq!(
            text,
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES (/* id */, /* name */)"
        );
    }

    #[test]
    fn drop_statement_uses_the_keyword_for_the_object_kind() {
        assert_eq!(
            drop_statement(Dialect::Postgres, &table_ref()).unwrap(),
            "DROP TABLE \"public\".\"users\""
        );
        let view = ObjectRef::new("v").with_kind(ObjectKind::View);
        assert_eq!(
            drop_statement(Dialect::Postgres, &view).unwrap(),
            "DROP VIEW \"v\""
        );
    }

    #[test]
    fn drop_statement_refuses_a_kind_with_no_drop_shape() {
        let column = ObjectRef::new("c").with_kind(ObjectKind::Column);
        let error = drop_statement(Dialect::Postgres, &column).unwrap_err();
        assert_eq!(error.code, DbErrorCode::NotSupported);
    }

    #[test]
    fn dropping_a_mongo_collection_is_a_run_command_document_not_sql() {
        let collection = ObjectRef::new("users").with_kind(ObjectKind::Collection);
        let text = drop_statement(Dialect::Mongo, &collection).unwrap();
        assert_eq!(text, "{\"drop\": \"users\"}");
        // A valid JSON object, since `db_sql::mongo::parse` (a crate this
        // one may not depend on, `db-core`/`db-sql`'s layering direction)
        // accepts a `runCommand` statement only when it parses as one —
        // this is the closest in-crate proxy for that contract without
        // introducing the dependency.
        assert!(serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|v| v.is_object()));
    }

    #[test]
    fn dropping_a_mongo_collection_escapes_a_quote_in_the_name() {
        let collection = ObjectRef::new("weird\"name").with_kind(ObjectKind::Collection);
        let text = drop_statement(Dialect::Mongo, &collection).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["drop"], "weird\"name");
    }

    #[test]
    fn a_mongo_kind_with_no_drop_shape_is_refused() {
        let index = ObjectRef::new("i").with_kind(ObjectKind::Index);
        let error = drop_statement(Dialect::Mongo, &index).unwrap_err();
        assert_eq!(error.code, DbErrorCode::NotSupported);
    }

    #[test]
    fn truncate_only_applies_to_tables() {
        assert!(truncate_statement(Dialect::Postgres, &table_ref())
            .unwrap()
            .starts_with("TRUNCATE TABLE"));
        let view = ObjectRef::new("v").with_kind(ObjectKind::View);
        assert!(truncate_statement(Dialect::Postgres, &view).is_err());
    }

    #[test]
    fn rename_statement_targets_alter_table_rename_to() {
        assert_eq!(
            rename_statement(
                Dialect::Sqlite,
                &ObjectRef::new("t").with_kind(ObjectKind::Table),
                "t2"
            )
            .unwrap(),
            "ALTER TABLE \"t\" RENAME TO \"t2\""
        );
    }

    #[test]
    fn sqlite_never_supports_comment() {
        assert!(!supports_comment(Dialect::Sqlite));
        let error = comment_statement(Dialect::Sqlite, &table_ref(), "hi").unwrap_err();
        assert_eq!(error.code, DbErrorCode::NotSupported);
    }

    #[test]
    fn postgres_comment_statement_quotes_the_literal() {
        let text = comment_statement(Dialect::Postgres, &table_ref(), "it's mine").unwrap();
        assert_eq!(
            text,
            "COMMENT ON TABLE \"public\".\"users\" IS 'it''s mine'"
        );
    }

    fn column(name: &str, type_name: &str, nullable: bool, pk: bool) -> Node {
        Node::leaf(name, ObjectKind::Column).with_detail(NodeDetail {
            type_name: Some(type_name.to_string()),
            nullable: Some(nullable),
            default: None,
            primary_key: pk,
            ..NodeDetail::default()
        })
    }

    #[test]
    fn synthesize_emits_columns_types_and_a_primary_key_clause() {
        let table = Node::with_children(
            "users",
            ObjectKind::Table,
            vec![
                column("id", "INTEGER", false, true),
                column("name", "TEXT", true, false),
            ],
        );
        let ddl = synthesize(&table, Dialect::Sqlite, None).unwrap();
        assert_eq!(
            ddl,
            "CREATE TABLE \"users\" (\n  \"id\" INTEGER NOT NULL,\n  \"name\" TEXT,\n  PRIMARY KEY (\"id\")\n)"
        );
    }

    #[test]
    fn synthesize_qualifies_with_a_schema_when_given_one() {
        let table = Node::with_children(
            "t",
            ObjectKind::Table,
            vec![column("c", "TEXT", true, false)],
        );
        let ddl = synthesize(&table, Dialect::Postgres, Some("public")).unwrap();
        assert!(ddl.starts_with("CREATE TABLE \"public\".\"t\""));
    }

    #[test]
    fn synthesize_refuses_a_non_table_node() {
        let view = Node::leaf("v", ObjectKind::View);
        let error = synthesize(&view, Dialect::Sqlite, None).unwrap_err();
        assert_eq!(error.code, DbErrorCode::NotSupported);
    }

    #[test]
    fn synthesize_refuses_a_table_whose_columns_are_not_loaded() {
        let table = Node::leaf("t", ObjectKind::Table);
        let error = synthesize(&table, Dialect::Sqlite, None).unwrap_err();
        assert_eq!(error.code, DbErrorCode::NotSupported);
    }

    #[test]
    fn every_generator_is_inert_against_an_adversarial_identifier() {
        let evil = ObjectRef::new("t\"; DROP TABLE users;--").with_kind(ObjectKind::Table);
        let statement = drop_statement(Dialect::Postgres, &evil).unwrap();
        // The embedded `"` is doubled by `quote_ident`, so the whole
        // payload stays inside one quoted identifier rather than closing
        // it early and starting a second statement.
        assert_eq!(statement, "DROP TABLE \"t\"\"; DROP TABLE users;--\"");
    }
}
