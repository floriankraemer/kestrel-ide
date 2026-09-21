//! The schema tree every backend's introspection fills in
//! (database-tools.md §6): `ObjectKind` (the relational kinds plus the
//! NoSQL ones as variants of the same enum, not a second tree),
//! `ObjectRef` (what "go to DDL" or "drop" names), `Node` (the tree the
//! Database dock renders) and `IntrospectLevel`/`IntrospectScope` (how
//! much of it a query fetches).

/// Every kind of object the tree can show, across every backend family —
/// see database-tools.md §6 for the full table (kind, applies-to, typical
/// actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Catalog,
    Schema,
    Table,
    View,
    MaterializedView,
    Column,
    Index,
    Constraint,
    Trigger,
    Routine,
    Sequence,
    Type,
    Role,
    User,
    /// MongoDB collection.
    Collection,
    /// A MongoDB field, schema inferred from a sample document.
    Field,
    /// Cassandra/Scylla keyspace.
    Keyspace,
    /// A Redis key namespace (a `:`-delimited prefix grouping, not a
    /// backend-reported object).
    KeyNamespace,
    /// A single Redis key, typed by [`RedisType`].
    Key(RedisType),
    /// A user-defined grouping the tree renders but no backend reports.
    Group,
}

/// Redis's own key types, since a `Key` node's edit affordance depends on
/// which one it is (a string editor vs. a list/hash/set/sorted-set/stream
/// grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedisType {
    String,
    List,
    Hash,
    Set,
    SortedSet,
    Stream,
}

/// What a Go-to-DDL, rename, drop or edit-data action names — a path from
/// the server root down to one object, catalog/schema optional (a SQLite
/// file has neither, a Mongo collection has no schema).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObjectRef {
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub name: String,
    pub kind: Option<ObjectKind>,
}

impl ObjectRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn with_catalog(mut self, catalog: impl Into<String>) -> Self {
        self.catalog = Some(catalog.into());
        self
    }

    pub fn with_kind(mut self, kind: ObjectKind) -> Self {
        self.kind = Some(kind);
        self
    }
}

/// How much of a subtree was fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Children {
    /// Not fetched yet — the tree asks for it on expand.
    NotLoaded,
    Loaded(Vec<Node>),
}

/// One row the Database dock renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub kind: ObjectKind,
    pub children: Children,
}

impl Node {
    pub fn leaf(name: impl Into<String>, kind: ObjectKind) -> Self {
        Self {
            name: name.into(),
            kind,
            children: Children::NotLoaded,
        }
    }

    pub fn with_children(name: impl Into<String>, kind: ObjectKind, children: Vec<Node>) -> Self {
        Self {
            name: name.into(),
            kind,
            children: Children::Loaded(children),
        }
    }
}

/// How much of a subtree an introspection query fetches eagerly.
///
/// `Names` must stay a cheap, near-instant query even on a 5 000-table
/// catalog (the NFR table's introspection target, ADR-0058) — it is the
/// level a tree's initial expand always uses, `Columns`/`Full` only once
/// the user actually opens a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrospectLevel {
    /// Object names only — no columns, no DDL.
    Names,
    /// Names plus each table/view's column list.
    Columns,
    /// Everything: columns, indexes, constraints, triggers, DDL text.
    Full,
}

/// One introspection request: where to look, and how deep.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntrospectScope {
    pub catalog: Option<String>,
    pub schema: Option<String>,
}

/// One introspection call's result: the roots the query covered, at the
/// level it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSnapshot {
    pub level: IntrospectLevel,
    pub roots: Vec<Node>,
}

impl SchemaSnapshot {
    pub fn new(level: IntrospectLevel, roots: Vec<Node>) -> Self {
        Self { level, roots }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_ref_carries_only_what_it_is_given() {
        let reference = ObjectRef::new("users")
            .with_schema("public")
            .with_kind(ObjectKind::Table);
        assert_eq!(reference.name, "users");
        assert_eq!(reference.schema.as_deref(), Some("public"));
        assert_eq!(reference.catalog, None);
        assert_eq!(reference.kind, Some(ObjectKind::Table));
    }

    #[test]
    fn a_leaf_node_starts_not_loaded() {
        let node = Node::leaf("users", ObjectKind::Table);
        assert_eq!(node.children, Children::NotLoaded);
    }

    #[test]
    fn a_node_with_children_is_loaded() {
        let node = Node::with_children(
            "public",
            ObjectKind::Schema,
            vec![Node::leaf("users", ObjectKind::Table)],
        );
        match node.children {
            Children::Loaded(children) => assert_eq!(children.len(), 1),
            Children::NotLoaded => panic!("expected loaded children"),
        }
    }

    #[test]
    fn redis_key_kinds_are_distinct_variants() {
        assert_ne!(
            ObjectKind::Key(RedisType::String),
            ObjectKind::Key(RedisType::Hash)
        );
        assert_eq!(
            ObjectKind::Key(RedisType::List),
            ObjectKind::Key(RedisType::List)
        );
    }

    #[test]
    fn a_snapshot_remembers_the_level_it_was_fetched_at() {
        let snapshot = SchemaSnapshot::new(IntrospectLevel::Names, vec![]);
        assert_eq!(snapshot.level, IntrospectLevel::Names);
        assert!(snapshot.roots.is_empty());
    }
}
