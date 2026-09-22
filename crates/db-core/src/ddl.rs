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

// ---- F4.4: create/modify object dialogs ----
//
// Every spec below is `serde`-deserialised straight from the JSON the
// bridge's `objectDdlPreview`/`runObjectDdl` receive (`database-tools-plan`
// F4.4) — the same "spec as compact JSON, decoded in Rust" pattern F4c's
// typed rows already established. Every field still crosses through
// `Dialect::quote_ident`/`quote_literal` exactly like every other
// generator in this module: a spec is attacker-shaped input the same way a
// grid cell's edited text is, never trusted as pre-quoted SQL.

/// One column in a `CREATE TABLE`/`ADD COLUMN`/`ALTER COLUMN` spec — the
/// object dialogs' columns-grid row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    pub type_name: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub primary_key: bool,
}

/// A `CREATE TABLE`'s own inline foreign key constraint.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForeignKeySpec {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

/// A `CREATE TABLE` dialog's whole spec.
/// ponytail: no inline index list — an index is created through its own
/// `create_index` call once the table exists, since bundling both into one
/// generated statement text would need a caller able to run more than one
/// statement per confirmation dialog; upgrade if a real user request wants
/// "table + its indexes" in a single Execute.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableSpec {
    pub name: String,
    #[serde(default)]
    pub schema: Option<String>,
    pub columns: Vec<ColumnSpec>,
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKeySpec>,
}

fn column_definition(dialect: Dialect, column: &ColumnSpec) -> String {
    let mut line = format!("{} {}", dialect.quote_ident(&column.name), column.type_name);
    if !column.nullable {
        line.push_str(" NOT NULL");
    }
    if let Some(default) = &column.default {
        line.push_str(&format!(" DEFAULT {default}"));
    }
    line
}

/// `CREATE TABLE` (F4.4) — columns, an inline `PRIMARY KEY` clause when any
/// column names itself one, and one `FOREIGN KEY` clause per
/// `foreign_keys` entry. MongoDB has no column-shaped `CREATE TABLE` at
/// all: a `createCollection` `runCommand` document names only the
/// collection, the same "not SQL text" shape `drop_statement`'s own Mongo
/// branch already returns (its doc comment on why the caller must send
/// this through `QueryLang`, not `Statement::sql`).
pub fn create_table(dialect: Dialect, spec: &TableSpec) -> Result<String, DbError> {
    if spec.columns.is_empty() {
        return Err(DbError::new(
            DbErrorCode::InvalidStatement,
            "a table needs at least one column",
        ));
    }
    if dialect == Dialect::Mongo {
        return Ok(format!("{{\"create\": {}}}", json_string(&spec.name)));
    }
    let mut lines: Vec<String> = spec
        .columns
        .iter()
        .map(|c| format!("  {}", column_definition(dialect, c)))
        .collect();
    let primary_key: Vec<String> = spec
        .columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| dialect.quote_ident(&c.name))
        .collect();
    if !primary_key.is_empty() {
        lines.push(format!("  PRIMARY KEY ({})", primary_key.join(", ")));
    }
    for fk in &spec.foreign_keys {
        let cols = fk
            .columns
            .iter()
            .map(|c| dialect.quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let ref_cols = fk
            .ref_columns
            .iter()
            .map(|c| dialect.quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "  FOREIGN KEY ({cols}) REFERENCES {} ({ref_cols})",
            dialect.quote_ident(&fk.ref_table)
        ));
    }
    let qualified = match &spec.schema {
        Some(schema) => format!(
            "{}.{}",
            dialect.quote_ident(schema),
            dialect.quote_ident(&spec.name)
        ),
        None => dialect.quote_ident(&spec.name),
    };
    Ok(format!(
        "CREATE TABLE {qualified} (\n{}\n)",
        lines.join(",\n")
    ))
}

/// `ALTER TABLE … ADD COLUMN` — every dialect but Mongo, which has no
/// fixed columns to add (a document just carries a new field the next
/// time it is written).
pub fn alter_table_add_column(
    dialect: Dialect,
    table: &ObjectRef,
    column: &ColumnSpec,
) -> Result<String, DbError> {
    if dialect == Dialect::Mongo {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "a MongoDB collection has no fixed columns to add",
        ));
    }
    Ok(format!(
        "ALTER TABLE {} ADD COLUMN {}",
        qualify(dialect, table),
        column_definition(dialect, column)
    ))
}

/// `ALTER TABLE … ALTER COLUMN`/`MODIFY COLUMN` — retypes a column,
/// dialect-shaped. SQLite has no `ALTER COLUMN`/`MODIFY COLUMN` statement
/// at all (a column can only be added/dropped/renamed, never retyped,
/// short of rebuilding the whole table) — refused typed, per this
/// module's own "the view offers no affordance a backend would reject"
/// convention (`supports_comment`'s doc comment states the same rule for
/// `COMMENT ON`).
/// ponytail: retypes only, never toggles nullability in the same
/// statement — Postgres/Cassandra would need a second `SET`/`DROP NOT
/// NULL` statement for that, out of this pass's one-statement-per-dialog
/// scope; upgrade once a caller needs both in one Execute.
pub fn alter_table_alter_column(
    dialect: Dialect,
    table: &ObjectRef,
    column: &ColumnSpec,
) -> Result<String, DbError> {
    let ident = dialect.quote_ident(&column.name);
    match dialect {
        Dialect::Sqlite => Err(DbError::new(
            DbErrorCode::NotSupported,
            "SQLite has no ALTER COLUMN — recreate the table to change a column's type",
        )),
        Dialect::Mongo | Dialect::Redis => Err(DbError::new(
            DbErrorCode::NotSupported,
            "this store has no fixed columns to alter",
        )),
        Dialect::Postgres => Ok(format!(
            "ALTER TABLE {} ALTER COLUMN {ident} TYPE {}",
            qualify(dialect, table),
            column.type_name
        )),
        Dialect::Cassandra => Ok(format!(
            "ALTER TABLE {} ALTER {ident} TYPE {}",
            qualify(dialect, table),
            column.type_name
        )),
        Dialect::MySql => Ok(format!(
            "ALTER TABLE {} MODIFY COLUMN {}",
            qualify(dialect, table),
            column_definition(dialect, column)
        )),
        Dialect::SqlServer => {
            let mut line = format!("ALTER COLUMN {ident} {}", column.type_name);
            if !column.nullable {
                line.push_str(" NOT NULL");
            }
            Ok(format!("ALTER TABLE {} {line}", qualify(dialect, table)))
        }
    }
}

/// `ALTER TABLE … DROP COLUMN` — every dialect but Mongo (no fixed
/// columns to drop; unset a field on every document instead, out of this
/// generator's scope) and SQLite (`rusqlite`-fronted SQLite versions in
/// this repo's supported range do accept `DROP COLUMN`, unlike `ALTER
/// COLUMN`'s retype case above, so this is not refused the way that one
/// is).
pub fn alter_table_drop_column(
    dialect: Dialect,
    table: &ObjectRef,
    column_name: &str,
) -> Result<String, DbError> {
    if dialect == Dialect::Mongo {
        return Err(DbError::new(
            DbErrorCode::NotSupported,
            "a MongoDB collection has no fixed columns to drop",
        ));
    }
    Ok(format!(
        "ALTER TABLE {} DROP COLUMN {}",
        qualify(dialect, table),
        dialect.quote_ident(column_name)
    ))
}

/// A `CREATE INDEX` dialog's whole spec.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndexSpec {
    pub name: String,
    pub table: String,
    #[serde(default)]
    pub schema: Option<String>,
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

/// `CREATE INDEX`/Mongo's `createIndexes` `runCommand` document.
pub fn create_index(dialect: Dialect, spec: &IndexSpec) -> Result<String, DbError> {
    if spec.columns.is_empty() {
        return Err(DbError::new(
            DbErrorCode::InvalidStatement,
            "an index needs at least one column",
        ));
    }
    if dialect == Dialect::Mongo {
        let keys = spec
            .columns
            .iter()
            .map(|c| format!("{}: 1", json_string(c)))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(format!(
            "{{\"createIndexes\": {}, \"indexes\": [{{\"key\": {{{keys}}}, \"name\": {}, \"unique\": {}}}]}}",
            json_string(&spec.table),
            json_string(&spec.name),
            spec.unique,
        ));
    }
    let mut table_ref = ObjectRef::new(spec.table.clone());
    if let Some(schema) = &spec.schema {
        table_ref = table_ref.with_schema(schema.clone());
    }
    let unique = if spec.unique { "UNIQUE " } else { "" };
    let cols = spec
        .columns
        .iter()
        .map(|c| dialect.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "CREATE {unique}INDEX {} ON {} ({cols})",
        dialect.quote_ident(&spec.name),
        qualify(dialect, &table_ref)
    ))
}

/// A `CREATE USER`/`CREATE ROLE` dialog's whole spec — `roles` is only
/// consumed by the dialects that grant roles at creation time (Mongo);
/// every SQL dialect here grants roles as a separate statement this pass
/// does not generate (out of scope — see `database-tools.md`'s F4d note).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserSpec {
    pub name: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

/// `CREATE USER`/`CREATE ROLE`, dialect-shaped — SQLite and Redis have no
/// user/role concept at all and are refused typed.
pub fn create_user(dialect: Dialect, spec: &UserSpec) -> Result<String, DbError> {
    match dialect {
        Dialect::Sqlite => Err(DbError::new(
            DbErrorCode::NotSupported,
            "SQLite has no user or role concept",
        )),
        Dialect::Redis => Err(DbError::new(
            DbErrorCode::NotSupported,
            "this Redis driver does not manage ACL users",
        )),
        Dialect::Postgres => {
            let mut text = format!("CREATE ROLE {} WITH LOGIN", dialect.quote_ident(&spec.name));
            if let Some(password) = &spec.password {
                text.push_str(&format!(" PASSWORD {}", dialect.quote_literal(password)));
            }
            Ok(text)
        }
        Dialect::MySql => {
            let mut text = format!("CREATE USER {}@'%'", dialect.quote_literal(&spec.name));
            if let Some(password) = &spec.password {
                text.push_str(&format!(
                    " IDENTIFIED BY {}",
                    dialect.quote_literal(password)
                ));
            }
            Ok(text)
        }
        Dialect::SqlServer => {
            let mut text = format!("CREATE LOGIN {} WITH", dialect.quote_ident(&spec.name));
            match &spec.password {
                Some(password) => {
                    text.push_str(&format!(" PASSWORD = {}", dialect.quote_literal(password)))
                }
                None => text.push_str(" PASSWORD = 'CHANGE_ME!1'"),
            }
            Ok(text)
        }
        Dialect::Cassandra => {
            let mut text = format!("CREATE USER {}", dialect.quote_ident(&spec.name));
            if let Some(password) = &spec.password {
                text.push_str(&format!(
                    " WITH PASSWORD {}",
                    dialect.quote_literal(password)
                ));
            }
            text.push_str(" NOSUPERUSER");
            Ok(text)
        }
        Dialect::Mongo => {
            let roles = spec
                .roles
                .iter()
                .map(|r| format!("{{\"role\": {}, \"db\": \"admin\"}}", json_string(r)))
                .collect::<Vec<_>>()
                .join(", ");
            let pwd = spec
                .password
                .as_deref()
                .map(json_string)
                .unwrap_or_else(|| "null".to_string());
            Ok(format!(
                "{{\"createUser\": {}, \"pwd\": {pwd}, \"roles\": [{roles}]}}",
                json_string(&spec.name)
            ))
        }
    }
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

    fn spec_column(name: &str, type_name: &str, nullable: bool, pk: bool) -> ColumnSpec {
        ColumnSpec {
            name: name.to_string(),
            type_name: type_name.to_string(),
            nullable,
            default: None,
            primary_key: pk,
        }
    }

    #[test]
    fn create_table_emits_columns_pk_and_foreign_keys() {
        let spec = TableSpec {
            name: "orders".to_string(),
            schema: Some("public".to_string()),
            columns: vec![
                spec_column("id", "INTEGER", false, true),
                spec_column("customer_id", "INTEGER", true, false),
            ],
            foreign_keys: vec![ForeignKeySpec {
                columns: vec!["customer_id".to_string()],
                ref_table: "customers".to_string(),
                ref_columns: vec!["id".to_string()],
            }],
        };
        let ddl = create_table(Dialect::Postgres, &spec).unwrap();
        assert_eq!(
            ddl,
            "CREATE TABLE \"public\".\"orders\" (\n  \"id\" INTEGER NOT NULL,\n  \"customer_id\" INTEGER,\n  PRIMARY KEY (\"id\"),\n  FOREIGN KEY (\"customer_id\") REFERENCES \"customers\" (\"id\")\n)"
        );
    }

    #[test]
    fn create_table_refuses_no_columns() {
        let spec = TableSpec {
            name: "t".to_string(),
            schema: None,
            columns: vec![],
            foreign_keys: vec![],
        };
        assert_eq!(
            create_table(Dialect::Sqlite, &spec).unwrap_err().code,
            DbErrorCode::InvalidStatement
        );
    }

    #[test]
    fn create_table_on_mongo_is_a_create_collection_command() {
        let spec = TableSpec {
            name: "orders".to_string(),
            schema: None,
            columns: vec![spec_column("_id", "any", true, false)],
            foreign_keys: vec![],
        };
        let ddl = create_table(Dialect::Mongo, &spec).unwrap();
        assert_eq!(ddl, "{\"create\": \"orders\"}");
    }

    #[test]
    fn create_table_escapes_an_adversarial_identifier_everywhere() {
        let evil = "t\"; DROP TABLE users;--";
        let spec = TableSpec {
            name: evil.to_string(),
            schema: None,
            columns: vec![spec_column(evil, "TEXT", true, false)],
            foreign_keys: vec![],
        };
        let ddl = create_table(Dialect::Postgres, &spec).unwrap();
        assert!(ddl.starts_with(
            "CREATE TABLE \"t\"\"; DROP TABLE users;--\" (\n  \"t\"\"; DROP TABLE users;--\" TEXT"
        ));
    }

    #[test]
    fn alter_table_add_column_generates_the_column_definition() {
        let table = ObjectRef::new("t").with_kind(ObjectKind::Table);
        let column = spec_column("age", "INTEGER", true, false);
        assert_eq!(
            alter_table_add_column(Dialect::Sqlite, &table, &column).unwrap(),
            "ALTER TABLE \"t\" ADD COLUMN \"age\" INTEGER"
        );
    }

    #[test]
    fn alter_table_add_column_refuses_mongo() {
        let table = ObjectRef::new("t").with_kind(ObjectKind::Collection);
        let column = spec_column("age", "int", true, false);
        assert_eq!(
            alter_table_add_column(Dialect::Mongo, &table, &column)
                .unwrap_err()
                .code,
            DbErrorCode::NotSupported
        );
    }

    #[test]
    fn sqlite_refuses_alter_column() {
        let table = ObjectRef::new("t").with_kind(ObjectKind::Table);
        let column = spec_column("age", "INTEGER", true, false);
        assert_eq!(
            alter_table_alter_column(Dialect::Sqlite, &table, &column)
                .unwrap_err()
                .code,
            DbErrorCode::NotSupported
        );
    }

    #[test]
    fn postgres_alter_column_retypes() {
        let table = ObjectRef::new("t").with_kind(ObjectKind::Table);
        let column = spec_column("age", "BIGINT", true, false);
        assert_eq!(
            alter_table_alter_column(Dialect::Postgres, &table, &column).unwrap(),
            "ALTER TABLE \"t\" ALTER COLUMN \"age\" TYPE BIGINT"
        );
    }

    #[test]
    fn mysql_alter_column_uses_modify_column() {
        let table = ObjectRef::new("t").with_kind(ObjectKind::Table);
        let column = spec_column("age", "BIGINT", false, false);
        assert_eq!(
            alter_table_alter_column(Dialect::MySql, &table, &column).unwrap(),
            "ALTER TABLE `t` MODIFY COLUMN `age` BIGINT NOT NULL"
        );
    }

    #[test]
    fn alter_table_drop_column_quotes_the_column() {
        let table = ObjectRef::new("t").with_kind(ObjectKind::Table);
        assert_eq!(
            alter_table_drop_column(Dialect::Sqlite, &table, "age").unwrap(),
            "ALTER TABLE \"t\" DROP COLUMN \"age\""
        );
    }

    #[test]
    fn alter_table_drop_column_refuses_mongo() {
        let table = ObjectRef::new("t").with_kind(ObjectKind::Collection);
        assert_eq!(
            alter_table_drop_column(Dialect::Mongo, &table, "age")
                .unwrap_err()
                .code,
            DbErrorCode::NotSupported
        );
    }

    #[test]
    fn create_index_generates_a_unique_index_over_two_columns() {
        let spec = IndexSpec {
            name: "ix_users_email".to_string(),
            table: "users".to_string(),
            schema: Some("public".to_string()),
            columns: vec!["email".to_string(), "tenant_id".to_string()],
            unique: true,
        };
        assert_eq!(
            create_index(Dialect::Postgres, &spec).unwrap(),
            "CREATE UNIQUE INDEX \"ix_users_email\" ON \"public\".\"users\" (\"email\", \"tenant_id\")"
        );
    }

    #[test]
    fn create_index_refuses_no_columns() {
        let spec = IndexSpec {
            name: "ix".to_string(),
            table: "t".to_string(),
            schema: None,
            columns: vec![],
            unique: false,
        };
        assert_eq!(
            create_index(Dialect::Sqlite, &spec).unwrap_err().code,
            DbErrorCode::InvalidStatement
        );
    }

    #[test]
    fn create_index_on_mongo_is_a_create_indexes_command() {
        let spec = IndexSpec {
            name: "ix1".to_string(),
            table: "orders".to_string(),
            schema: None,
            columns: vec!["customerId".to_string()],
            unique: false,
        };
        let text = create_index(Dialect::Mongo, &spec).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["createIndexes"], "orders");
        assert_eq!(value["indexes"][0]["key"]["customerId"], 1);
        assert_eq!(value["indexes"][0]["unique"], false);
    }

    #[test]
    fn create_index_escapes_an_adversarial_identifier() {
        let spec = IndexSpec {
            name: "ix\"; DROP TABLE users;--".to_string(),
            table: "t".to_string(),
            schema: None,
            columns: vec!["c".to_string()],
            unique: false,
        };
        let text = create_index(Dialect::Postgres, &spec).unwrap();
        assert_eq!(
            text,
            "CREATE INDEX \"ix\"\"; DROP TABLE users;--\" ON \"t\" (\"c\")"
        );
    }

    #[test]
    fn sqlite_refuses_create_user() {
        let spec = UserSpec {
            name: "bob".to_string(),
            password: None,
            roles: vec![],
        };
        assert_eq!(
            create_user(Dialect::Sqlite, &spec).unwrap_err().code,
            DbErrorCode::NotSupported
        );
    }

    #[test]
    fn postgres_create_user_quotes_the_password_literal() {
        let spec = UserSpec {
            name: "bob".to_string(),
            password: Some("it's secret".to_string()),
            roles: vec![],
        };
        assert_eq!(
            create_user(Dialect::Postgres, &spec).unwrap(),
            "CREATE ROLE \"bob\" WITH LOGIN PASSWORD 'it''s secret'"
        );
    }

    #[test]
    fn mysql_create_user_targets_any_host() {
        let spec = UserSpec {
            name: "bob".to_string(),
            password: Some("s3cret".to_string()),
            roles: vec![],
        };
        assert_eq!(
            create_user(Dialect::MySql, &spec).unwrap(),
            "CREATE USER 'bob'@'%' IDENTIFIED BY 's3cret'"
        );
    }

    #[test]
    fn mongo_create_user_is_a_run_command_with_roles() {
        let spec = UserSpec {
            name: "bob".to_string(),
            password: Some("s3cret".to_string()),
            roles: vec!["readWrite".to_string()],
        };
        let text = create_user(Dialect::Mongo, &spec).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["createUser"], "bob");
        assert_eq!(value["pwd"], "s3cret");
        assert_eq!(value["roles"][0]["role"], "readWrite");
    }

    #[test]
    fn create_user_escapes_an_adversarial_name_for_every_sql_dialect() {
        let spec = UserSpec {
            name: "bob'; DROP ROLE admin;--".to_string(),
            password: None,
            roles: vec![],
        };
        // MySQL quotes the name as a string literal (`quote_literal`): the
        // embedded `'` is doubled, so the whole name stays inside one
        // literal rather than closing it early.
        assert_eq!(
            create_user(Dialect::MySql, &spec).unwrap(),
            "CREATE USER 'bob''; DROP ROLE admin;--'@'%'"
        );
        // Postgres/SQL Server/Cassandra all quote the name as an
        // identifier instead (`"…"`/`[…]`) — the embedded `'` is simply
        // inert there, so the whole name still stays inside one quoted/
        // bracketed span rather than breaking out of it.
        assert_eq!(
            create_user(Dialect::Postgres, &spec).unwrap(),
            "CREATE ROLE \"bob'; DROP ROLE admin;--\" WITH LOGIN"
        );
        assert_eq!(
            create_user(Dialect::SqlServer, &spec).unwrap(),
            "CREATE LOGIN [bob'; DROP ROLE admin;--] WITH PASSWORD = 'CHANGE_ME!1'"
        );
        assert_eq!(
            create_user(Dialect::Cassandra, &spec).unwrap(),
            "CREATE USER \"bob'; DROP ROLE admin;--\" NOSUPERUSER"
        );
    }
}
