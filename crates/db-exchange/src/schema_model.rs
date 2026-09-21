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
//! the dock's lazy tree into something it was never meant to be. Building
//! one of these from a live `Connection` (F2's richer introspection, or a
//! `ddl.rs` synthesizer) is a caller's job — this crate takes it as
//! already-fetched input.

/// One column of a [`TableDef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
}

/// A foreign key: this table's `columns` reference `ref_table`'s
/// `ref_columns`, position for position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
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
