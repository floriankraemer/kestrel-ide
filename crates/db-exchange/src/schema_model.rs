//! A fully-fetched schema snapshot: tables with columns/PK/FKs/indexes,
//! plus view and routine text — what [`crate::er_diagram`] and
//! [`crate::schema_compare`] both need and [`db_core::schema::SchemaSnapshot`]
//! deliberately does not carry.
//!
//! `db_core`'s own `SchemaSnapshot`/`Node` is the Database dock's
//! lazily-expanded tree (names and kinds only, `IntrospectLevel` decides
//! how much of it a query fetches — see `database-tools.md` §6). An ER
//! diagram or a compare needs the *whole* structure of a table at once
//! (every column's type/nullability, every FK's target, every index), so
//! this module defines its own eagerly-fetched shape rather than growing
//! the dock's lazy tree into something it was never meant to be.
//! [`from_snapshot`] is how a caller (`ui-shell`'s `ExchangeService`)
//! turns a live `Session`'s `IntrospectLevel::Full` tree into one of
//! these; a hand-built one (tests, or another future source) is just as
//! valid an input to [`crate::er_diagram`]/[`crate::schema_compare`].

use db_core::schema::{
    Children, ConstraintKind, IntrospectLevel, IntrospectScope, Node, ObjectKind, ObjectRef,
};
use db_core::session::Session;

/// One column of a [`TableDef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
}

/// A table reference, schema-qualified where the source carries a schema
/// (Postgres) and bare where it does not (SQLite has none) — carried as
/// its own small type rather than a bare `String` so a foreign key
/// referencing a same-named table in a *different* schema is never
/// silently confused with the wrong one ([`TableDef::matches_ref`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

impl TableRef {
    pub fn bare(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    /// The dotted display form (`schema.table`, or just `table` with no
    /// schema) — what an ER diagram label or a migration script's
    /// `REFERENCES` clause shows.
    pub fn qualified(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// A foreign key: this table's `columns` reference `ref_table`'s
/// `ref_columns`, position for position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub columns: Vec<String>,
    pub ref_table: TableRef,
    pub ref_columns: Vec<String>,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintDef {
    pub name: String,
    /// The constraint's own DDL fragment (`CHECK (...)`, …) — kept as
    /// text rather than modelled, the same "recorded debt" the plan doc
    /// accepts for routines/views.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDef {
    pub name: String,
    /// The schema this table lives in, `None` where the backend has no
    /// such concept (SQLite) or the snapshot was not scoped to one.
    pub schema: Option<String>,
    pub columns: Vec<ColumnDef>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<IndexDef>,
    pub constraints: Vec<ConstraintDef>,
}

impl TableDef {
    pub fn column(&self, name: &str) -> Option<&ColumnDef> {
        self.columns.iter().find(|c| c.name == name)
    }

    pub fn table_ref(&self) -> TableRef {
        TableRef {
            schema: self.schema.clone(),
            name: self.name.clone(),
        }
    }

    /// Whether `reference` names this table — by name always, and by
    /// schema too when `reference` carries one (an unqualified reference
    /// matches any schema, the common single-schema case).
    pub fn matches_ref(&self, reference: &TableRef) -> bool {
        self.name == reference.name
            && reference
                .schema
                .as_ref()
                .is_none_or(|schema| self.schema.as_deref() == Some(schema.as_str()))
    }

    /// Whether `columns` (in this table) are fully covered by some
    /// unique index or unique/primary-key constraint — a foreign key
    /// whose own columns satisfy this is a one-to-one relationship, not
    /// one-to-many ([`crate::er_diagram`]).
    pub fn columns_are_unique(&self, columns: &[String]) -> bool {
        let as_set: std::collections::BTreeSet<&str> = columns.iter().map(String::as_str).collect();
        if as_set.len() == 1 {
            let pk_set: std::collections::BTreeSet<&str> =
                self.primary_key.iter().map(String::as_str).collect();
            if pk_set == as_set {
                return true;
            }
        }
        self.indexes.iter().any(|index| {
            index.unique
                && index
                    .columns
                    .iter()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>()
                    == as_set
        })
    }
}

/// A view or routine, kept as its own definition text — `schema_compare`
/// diffs these as text blocks (the plan's recorded debt: no structural
/// view/routine diff).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextObject {
    pub name: String,
    pub definition: String,
}

/// A fully-fetched schema: every table (with its columns/keys/indexes),
/// view and routine at once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchemaSnapshot {
    pub tables: Vec<TableDef>,
    pub views: Vec<TextObject>,
    pub routines: Vec<TextObject>,
}

impl SchemaSnapshot {
    pub fn table(&self, name: &str) -> Option<&TableDef> {
        self.tables.iter().find(|t| t.name == name)
    }
}

/// Fetch a fully-fetched [`SchemaSnapshot`] from `session` (`Introspect
/// ::Full`, every schema/catalog the session can see) — the one call an
/// ER diagram, a schema compare or a dump's structural preview all start
/// from.
pub fn fetch(session: &mut Session) -> Result<SchemaSnapshot, db_core::error::DbError> {
    let snapshot = session.introspect(&IntrospectScope::default(), IntrospectLevel::Full)?;
    Ok(from_snapshot(&snapshot.roots, session))
}

/// Turn a `db_core::schema` node tree (`IntrospectLevel::Full`, from
/// [`Session::introspect`]) into this crate's own eagerly-fetched shape.
/// `session` is only used for `ddl_of` (a view/routine's definition text
/// is not carried on the tree itself, `database-tools.md` §6's recorded
/// scope) — table/column/FK/index/constraint detail all come from the
/// tree's own `NodeDetail`.
pub fn from_snapshot(roots: &[Node], session: &mut Session) -> SchemaSnapshot {
    let mut tables = Vec::new();
    let mut views = Vec::new();
    let mut routines = Vec::new();
    collect_objects(roots, None, session, &mut tables, &mut views, &mut routines);
    tables.sort_by(|a, b| {
        (a.schema.as_deref(), a.name.as_str()).cmp(&(b.schema.as_deref(), b.name.as_str()))
    });
    SchemaSnapshot {
        tables,
        views,
        routines,
    }
}

fn collect_objects(
    nodes: &[Node],
    schema: Option<&str>,
    session: &mut Session,
    tables: &mut Vec<TableDef>,
    views: &mut Vec<TextObject>,
    routines: &mut Vec<TextObject>,
) {
    for node in nodes {
        match node.kind {
            ObjectKind::Schema => {
                if let Children::Loaded(children) = &node.children {
                    collect_objects(children, Some(&node.name), session, tables, views, routines);
                }
                continue;
            }
            ObjectKind::Table => tables.push(to_table_def(node, schema)),
            ObjectKind::View => views.push(to_text_object(node, session, ObjectKind::View)),
            ObjectKind::Routine => {
                routines.push(to_text_object(node, session, ObjectKind::Routine))
            }
            _ => {}
        }
        if let Children::Loaded(children) = &node.children {
            collect_objects(children, schema, session, tables, views, routines);
        }
    }
}

fn ref_table_of(reference: &ObjectRef) -> TableRef {
    TableRef {
        schema: reference.schema.clone(),
        name: reference.name.clone(),
    }
}

fn to_table_def(node: &Node, schema: Option<&str>) -> TableDef {
    let mut columns = Vec::new();
    let mut primary_key = Vec::new();
    let mut indexes = Vec::new();
    let mut constraints = Vec::new();
    let mut foreign_keys = Vec::new();
    if let Children::Loaded(children) = &node.children {
        for child in children {
            match child.kind {
                ObjectKind::Column => {
                    let detail = &child.detail;
                    if detail.primary_key {
                        primary_key.push(child.name.clone());
                    }
                    columns.push(ColumnDef {
                        name: child.name.clone(),
                        type_name: detail.type_name.clone().unwrap_or_default(),
                        nullable: detail.nullable.unwrap_or(true),
                        default: detail.default.clone(),
                    });
                }
                ObjectKind::Index => {
                    let detail = child.detail.index.clone().unwrap_or_default();
                    indexes.push(IndexDef {
                        name: child.name.clone(),
                        columns: detail.columns,
                        unique: detail.unique,
                        method: detail.method,
                    });
                }
                ObjectKind::Constraint => match &child.detail.constraint {
                    Some(ConstraintKind::ForeignKey {
                        columns,
                        ref_table,
                        ref_columns,
                        on_delete,
                        on_update,
                    }) => foreign_keys.push(ForeignKey {
                        columns: columns.clone(),
                        ref_table: ref_table_of(ref_table),
                        ref_columns: ref_columns.clone(),
                        on_delete: on_delete.clone(),
                        on_update: on_update.clone(),
                    }),
                    Some(ConstraintKind::PrimaryKey { columns }) => {
                        constraints.push(ConstraintDef {
                            name: child.name.clone(),
                            text: format!("PRIMARY KEY ({})", columns.join(", ")),
                        });
                    }
                    Some(ConstraintKind::Unique { columns }) => {
                        constraints.push(ConstraintDef {
                            name: child.name.clone(),
                            text: format!("UNIQUE ({})", columns.join(", ")),
                        });
                    }
                    Some(ConstraintKind::Check { expr }) => {
                        constraints.push(ConstraintDef {
                            name: child.name.clone(),
                            text: format!("CHECK ({expr})"),
                        });
                    }
                    None => constraints.push(ConstraintDef {
                        name: child.name.clone(),
                        text: String::new(),
                    }),
                },
                _ => {}
            }
        }
    }
    TableDef {
        name: node.name.clone(),
        schema: schema.map(str::to_string),
        columns,
        primary_key,
        foreign_keys,
        indexes,
        constraints,
    }
}

fn to_text_object(node: &Node, session: &mut Session, kind: ObjectKind) -> TextObject {
    let object = ObjectRef::new(node.name.clone()).with_kind(kind);
    let definition = session.ddl_of(&object).unwrap_or_default();
    TextObject {
        name: node.name.clone(),
        definition,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::driver::{ExecOptions, Statement};

    fn sqlite_session() -> Session {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        Session::new(Box::new(db_drivers::sqlite::SqliteConnection::wrap(conn)))
    }

    fn exec(session: &mut Session, sql: &str) {
        session
            .execute(&Statement::sql(sql), &ExecOptions::default())
            .unwrap();
    }

    #[test]
    fn from_snapshot_maps_foreign_keys_composite_pk_and_unique_index() {
        let mut session = sqlite_session();
        exec(
            &mut session,
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT UNIQUE)",
        );
        exec(
            &mut session,
            "CREATE TABLE products (id INTEGER PRIMARY KEY)",
        );
        exec(
            &mut session,
            "CREATE TABLE order_items (\
               order_id INTEGER, product_id INTEGER, user_id INTEGER, \
               PRIMARY KEY (order_id, product_id), \
               FOREIGN KEY (product_id) REFERENCES products(id), \
               FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE\
             )",
        );

        let snapshot = fetch(&mut session).unwrap();
        let order_items = snapshot.table("order_items").unwrap();
        assert_eq!(
            order_items.primary_key,
            vec!["order_id".to_string(), "product_id".to_string()]
        );
        assert_eq!(order_items.foreign_keys.len(), 2);
        let to_users = order_items
            .foreign_keys
            .iter()
            .find(|fk| fk.ref_table.name == "users")
            .unwrap();
        assert_eq!(to_users.columns, vec!["user_id".to_string()]);
        assert_eq!(to_users.on_delete.as_deref(), Some("CASCADE"));

        let users = snapshot.table("users").unwrap();
        assert!(users
            .indexes
            .iter()
            .any(|i| i.unique && i.columns == vec!["email".to_string()]));
        assert!(users.columns_are_unique(&["email".to_string()]));
        assert!(users.columns_are_unique(&["id".to_string()]));
        assert!(!users.columns_are_unique(&["nonexistent".to_string()]));
    }

    #[test]
    fn a_table_ref_matches_by_name_and_optionally_by_schema() {
        let table = TableDef {
            name: "users".to_string(),
            schema: Some("public".to_string()),
            columns: vec![],
            primary_key: vec![],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
        };
        assert!(table.matches_ref(&TableRef::bare("users")));
        assert!(table.matches_ref(&TableRef {
            schema: Some("public".to_string()),
            name: "users".to_string()
        }));
        assert!(!table.matches_ref(&TableRef {
            schema: Some("other".to_string()),
            name: "users".to_string()
        }));
    }

    #[test]
    fn a_table_ref_renders_its_qualified_display_form() {
        assert_eq!(TableRef::bare("users").qualified(), "users");
        assert_eq!(
            TableRef {
                schema: Some("public".to_string()),
                name: "users".to_string()
            }
            .qualified(),
            "public.users"
        );
    }
}
