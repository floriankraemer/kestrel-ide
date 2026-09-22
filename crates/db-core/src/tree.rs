//! The Database dock's tree, flattened (database-tools-plan.md F2.2): one
//! [`TreeRow`] per visible row, in render order, the same "decisions live
//! here, words live in the view" split `container_core::tree` already
//! uses for the Containers dock.
//!
//! [`flatten`] turns a [`crate::schema::SchemaSnapshot`]'s roots into rows
//! honouring a [`GroupMode`] (folders-by-kind, IntelliJ's default, vs
//! flat), an [`ObjectTypeFilter`] (which kinds show at all), a pattern
//! filter (`kind:pattern` or a plain substring), and a [`SortOrder`].
//! [`actions_for`] is the one place that decides which affordances a row
//! offers — the view never encodes a business decision about what a
//! table vs. a read-only source can do (`CLAUDE.md`'s humble-view rule).

use crate::schema::{Children, Node, ObjectKind};

/// Which of a row's possible actions apply — a bitflag set for the same
/// reason `driver::Capabilities` is one: a handful of flags, no value from
/// the `bitflags` crate's macro ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActionSet(u32);

impl ActionSet {
    pub const NONE: ActionSet = ActionSet(0);
    pub const OPEN_CONSOLE: ActionSet = ActionSet(1 << 0);
    pub const EDIT_DATA: ActionSet = ActionSet(1 << 1);
    pub const GO_TO_DDL: ActionSet = ActionSet(1 << 2);
    pub const COPY_NAME: ActionSet = ActionSet(1 << 3);
    pub const COPY_QUALIFIED_NAME: ActionSet = ActionSet(1 << 4);
    pub const REFRESH: ActionSet = ActionSet(1 << 5);
    pub const RENAME: ActionSet = ActionSet(1 << 6);
    pub const DROP: ActionSet = ActionSet(1 << 7);
    pub const TRUNCATE: ActionSet = ActionSet(1 << 8);
    pub const COMMENT: ActionSet = ActionSet(1 << 9);
    pub const GENERATE_DDL: ActionSet = ActionSet(1 << 10);
    pub const ER_DIAGRAM: ActionSet = ActionSet(1 << 11);
    /// Export the row's data (database-tools-plan F5b.3) — a relation's
    /// rows out to CSV/TSV/JSON/Markdown/SQL/XLSX. Offered whether or not
    /// the source is read-only: reading rows out is never a write.
    pub const EXPORT_DATA: ActionSet = ActionSet(1 << 12);
    /// Import rows into the table from a file (F5b.3) — a write, so gated
    /// by `!caps.read_only` like `EDIT_DATA`.
    pub const IMPORT_DATA: ActionSet = ActionSet(1 << 13);
    /// Copy the table's rows to another table, same or a different source
    /// (F5b.3) — reads this table and writes the destination, so gated by
    /// the *destination*'s capabilities in the dialog, not this row's; the
    /// row itself only needs to be a real table to offer the entry.
    pub const COPY_TABLE: ActionSet = ActionSet(1 << 14);
    /// Delete a Redis key (F7b) — a write, gated by `!caps.read_only` like
    /// every other destructive action.
    pub const DELETE_KEY: ActionSet = ActionSet(1 << 15);
    /// Set a Redis key's TTL (F7b) — a write, same gate as `DELETE_KEY`.
    pub const TTL_SET: ActionSet = ActionSet(1 << 16);
    /// Create a new table under a schema/catalog/source root (F4.4) — a
    /// write, so gated by `!caps.read_only` like every other DDL action.
    pub const CREATE_TABLE: ActionSet = ActionSet(1 << 17);
    /// Open the Modify Table dialog (add/alter/drop a column, F4.4) on an
    /// existing table.
    pub const MODIFY_TABLE: ActionSet = ActionSet(1 << 18);
    /// Add a column to an existing table (F4.4) — offered alongside
    /// `MODIFY_TABLE` rather than folded into it, since a caller may want
    /// the narrower "just add one column" dialog directly from the
    /// context menu.
    pub const ADD_COLUMN: ActionSet = ActionSet(1 << 19);
    /// Create an index on an existing table (F4.4).
    pub const CREATE_INDEX: ActionSet = ActionSet(1 << 20);
    /// Create a user/role at the source's own root (F4.4) — offered
    /// wherever `CREATE_TABLE` is, since both are schema/source-level DDL
    /// rather than one table's own action.
    pub const CREATE_USER: ActionSet = ActionSet(1 << 21);

    pub const fn contains(self, other: ActionSet) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: ActionSet) -> ActionSet {
        ActionSet(self.0 | other.0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl std::ops::BitOr for ActionSet {
    type Output = ActionSet;
    fn bitor(self, rhs: ActionSet) -> ActionSet {
        self.union(rhs)
    }
}

/// Whether the tree groups a schema's children into per-kind folders
/// (IntelliJ's default) or renders every object as a flat sibling list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMode {
    ByObjectType,
    Flat,
}

/// A synthetic folder [`flatten`] inserts under [`GroupMode::ByObjectType`]
/// — not a `db_core::schema::ObjectKind` itself, since no backend ever
/// reports a "Tables" folder as an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKind {
    Tables,
    Views,
    MaterializedViews,
    Procedures,
    Functions,
    Sequences,
    Types,
}

impl FolderKind {
    pub fn label(self) -> &'static str {
        match self {
            FolderKind::Tables => "Tables",
            FolderKind::Views => "Views",
            FolderKind::MaterializedViews => "Materialized Views",
            FolderKind::Procedures => "Procedures",
            FolderKind::Functions => "Functions",
            FolderKind::Sequences => "Sequences",
            FolderKind::Types => "Types",
        }
    }
}

/// One flattened row the Database dock renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// Nesting depth from the source root (0 = a catalog/schema root).
    pub depth: u32,
    /// A stable id: the `/`-joined path from the source root down to this
    /// row (schema/table/column names — a synthetic folder's id is its
    /// [`FolderKind::label`]). Ids are never numeric indices, so an
    /// expand/collapse or a filter change never reshuffles what an
    /// already-open node id refers to.
    pub node_id: String,
    pub kind: RowKind,
    pub label: String,
    /// A short right-aligned hint (a column's type, a routine's return
    /// type) — empty when the kind has none.
    pub detail: String,
    pub actions: ActionSet,
    pub expandable: bool,
    /// Whether an expandable row's children are already fetched — the
    /// tree asks for them on first expand otherwise (`Children::NotLoaded`
    /// surfaced up through here).
    pub loaded: bool,
    /// The real ancestry of schema-object names down to this row —
    /// `node_id` with every synthetic folder segment stripped out, so a
    /// caller building an `IntrospectScope`/`ObjectRef` for "expand" or
    /// "Go to DDL" never has to guess which `/`-separated segment of
    /// `node_id` was a folder label and which was a real object name.
    pub object_path: Vec<String>,
    /// Whether this row is (part of) its table's primary key — carried
    /// straight from `Node::detail::primary_key` so the dock can pick the
    /// dedicated PK icon for a column without re-deriving the notion of
    /// "primary key" itself. `false` for every non-column row.
    pub primary_key: bool,
}

/// A tree row is either a real schema object or a synthetic grouping
/// folder [`flatten`] inserts under [`GroupMode::ByObjectType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Object(ObjectKind),
    Folder(FolderKind),
}

/// Which object kinds are allowed to show at all — every kind included by
/// default (`ObjectTypeFilter::all()`), narrowed by unchecking one in the
/// filter UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectTypeFilter {
    excluded: Vec<ObjectKind>,
}

impl ObjectTypeFilter {
    pub fn all() -> Self {
        Self {
            excluded: Vec::new(),
        }
    }

    pub fn excluding(kinds: Vec<ObjectKind>) -> Self {
        Self { excluded: kinds }
    }

    fn allows(&self, kind: ObjectKind) -> bool {
        !self.excluded.contains(&kind)
    }
}

/// A pattern filter: `kind:pattern` narrows to one kind's rows matching a
/// regex (`table:-payment_.*` — a leading `-` negates), a plain string is
/// a case-insensitive substring speed-search over every row's label.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatternFilter {
    raw: String,
}

enum ParsedPattern<'a> {
    Substring(&'a str),
    Kind {
        kind_prefix: &'a str,
        negate: bool,
        pattern: String,
    },
}

impl PatternFilter {
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }

    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }

    fn parse(&self) -> ParsedPattern<'_> {
        if let Some((prefix, rest)) = self.raw.split_once(':') {
            let (negate, pattern) = match rest.strip_prefix('-') {
                Some(stripped) => (true, stripped.to_string()),
                None => (false, rest.to_string()),
            };
            return ParsedPattern::Kind {
                kind_prefix: prefix,
                negate,
                pattern,
            };
        }
        ParsedPattern::Substring(&self.raw)
    }

    /// Whether `label` (a row of `kind_prefix`, e.g. `"table"`) matches.
    fn matches(&self, label: &str, kind_prefix: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        match self.parse() {
            ParsedPattern::Substring(text) => label.to_lowercase().contains(&text.to_lowercase()),
            ParsedPattern::Kind {
                kind_prefix: wanted,
                negate,
                pattern,
            } => {
                if !kind_prefix.eq_ignore_ascii_case(wanted) {
                    // A kind-scoped pattern only ever narrows rows of its
                    // own kind — every other kind's rows pass through
                    // unaffected, so `table:foo` never hides a column
                    // named `foo`.
                    return true;
                }
                let is_match = regex_lite_match(&pattern, label);
                is_match != negate
            }
        }
    }
}

/// A tiny, dependency-free regex subset good enough for the identifier
/// patterns a "table:-payment_.*" filter actually needs: `.` (any char),
/// `*` (zero or more of the preceding atom), literal characters
/// otherwise, anchored at both ends.
/// ponytail: no character classes, alternation or anchors beyond
/// full-string match; upgrade to the `regex` crate if a user pattern ever
/// needs more than this (identifier globs rarely do).
fn regex_lite_match(pattern: &str, text: &str) -> bool {
    fn match_here(pattern: &[char], text: &[char]) -> bool {
        if pattern.is_empty() {
            return text.is_empty();
        }
        if pattern.len() >= 2 && pattern[1] == '*' {
            let atom = pattern[0];
            // Zero occurrences.
            if match_here(&pattern[2..], text) {
                return true;
            }
            // One or more occurrences.
            let mut i = 0;
            while i < text.len() && (atom == '.' || text[i] == atom) {
                i += 1;
                if match_here(&pattern[2..], &text[i..]) {
                    return true;
                }
            }
            return false;
        }
        if text.is_empty() {
            return false;
        }
        if pattern[0] == '.' || pattern[0] == text[0] {
            return match_here(&pattern[1..], &text[1..]);
        }
        false
    }
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    match_here(&pattern_chars, &text_chars)
}

/// Natural (`table2` before `table10`) vs plain alphabetical sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Natural,
    Alphabetical,
}

fn natural_key(name: &str) -> Vec<(String, u64)> {
    let mut key = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.peek().copied() {
        if c.is_ascii_digit() {
            let mut digits = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    digits.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            key.push((String::new(), digits.parse().unwrap_or(0)));
        } else {
            let mut text = String::new();
            while let Some(&t) = chars.peek() {
                if !t.is_ascii_digit() {
                    text.push(t);
                    chars.next();
                } else {
                    break;
                }
            }
            key.push((text.to_lowercase(), 0));
        }
    }
    key
}

fn sort_nodes(nodes: &mut [&Node], order: SortOrder) {
    match order {
        SortOrder::Alphabetical => nodes.sort_by_key(|a| a.name.to_lowercase()),
        SortOrder::Natural => nodes.sort_by_key(|a| natural_key(&a.name)),
    }
}

/// A fixed render order for grouping folders — IntelliJ shows Tables
/// before Views before routines, and a stable order matters more than any
/// particular one, so `flatten` never lets it drift with insertion order.
fn folder_order(kind: FolderKind) -> u8 {
    match kind {
        FolderKind::Tables => 0,
        FolderKind::Views => 1,
        FolderKind::MaterializedViews => 2,
        FolderKind::Procedures => 3,
        FolderKind::Functions => 4,
        FolderKind::Sequences => 5,
        FolderKind::Types => 6,
    }
}

fn folder_for(kind: ObjectKind) -> Option<FolderKind> {
    match kind {
        ObjectKind::Table => Some(FolderKind::Tables),
        ObjectKind::View => Some(FolderKind::Views),
        ObjectKind::MaterializedView => Some(FolderKind::MaterializedViews),
        // `separate_routines` decides Procedures vs a single Routines
        // folder at the call site (`flatten`'s parameter), so this bare
        // mapping always answers "Procedures" and the caller folds it
        // back into one folder when the toggle is off.
        ObjectKind::Routine => Some(FolderKind::Procedures),
        ObjectKind::Sequence => Some(FolderKind::Sequences),
        ObjectKind::Type => Some(FolderKind::Types),
        _ => None,
    }
}

fn kind_prefix(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Catalog => "catalog",
        ObjectKind::Schema => "schema",
        ObjectKind::Table => "table",
        ObjectKind::View => "view",
        ObjectKind::MaterializedView => "materialized-view",
        ObjectKind::Column => "column",
        ObjectKind::Index => "index",
        ObjectKind::Constraint => "constraint",
        ObjectKind::Trigger => "trigger",
        ObjectKind::Routine => "routine",
        ObjectKind::Sequence => "sequence",
        ObjectKind::Type => "type",
        ObjectKind::Role => "role",
        ObjectKind::User => "user",
        ObjectKind::Collection => "collection",
        ObjectKind::Field => "field",
        ObjectKind::Keyspace => "keyspace",
        ObjectKind::KeyNamespace => "key-namespace",
        ObjectKind::Key(_) => "key",
        ObjectKind::Group => "group",
    }
}

/// Whether the data source itself is read-only — narrows [`actions_for`]
/// the same way a filesystem-readonly project narrows the file tree's
/// context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceCapabilities {
    pub read_only: bool,
    /// The dialect supports a `COMMENT ON`-shaped statement at all (SQLite
    /// does not) — `ObjectKind`-independent, since it is a whole-backend
    /// trait, not a per-kind one.
    pub supports_comment: bool,
}

/// Which actions a row of `kind` offers, given the source's capabilities —
/// the one place a rename/drop/truncate/comment affordance is decided; the
/// view only renders what this returns.
pub fn actions_for(kind: ObjectKind, caps: SourceCapabilities) -> ActionSet {
    use ObjectKind::*;
    let mut actions = ActionSet::NONE;
    let is_relation = matches!(kind, Table | View | MaterializedView | Collection);
    let is_schema_like = matches!(kind, Catalog | Schema | Keyspace);
    let is_routine = matches!(kind, Routine);

    if is_relation || is_schema_like || is_routine {
        actions =
            actions | ActionSet::REFRESH | ActionSet::COPY_NAME | ActionSet::COPY_QUALIFIED_NAME;
    }
    if is_relation {
        actions =
            actions | ActionSet::OPEN_CONSOLE | ActionSet::GO_TO_DDL | ActionSet::GENERATE_DDL;
    }
    if matches!(kind, Table) {
        actions = actions | ActionSet::EDIT_DATA | ActionSet::ER_DIAGRAM;
    }
    // `Collection` (MongoDB) gets no `EDIT_DATA` bit here: F4 (parallel
    // phase F4b) owns the document grid editor and is not landed on this
    // branch yet — wiring the bit on before the editor exists would offer
    // a menu entry that does nothing. No `ER_DIAGRAM` either: a schemaless
    // collection has no foreign keys for the ER builder to draw (F6's ER
    // diagram is relational-only, database-tools.md §6).
    if is_relation {
        actions = actions | ActionSet::EXPORT_DATA;
    }
    if matches!(kind, Table) {
        actions = actions | ActionSet::COPY_TABLE;
    }
    if is_routine {
        actions = actions | ActionSet::GO_TO_DDL | ActionSet::GENERATE_DDL;
    }
    if matches!(kind, KeyNamespace | Key(_)) {
        // Redis has no schema-shaped "go to DDL"/rename — a namespace is a
        // `:`-grouping the tree itself invented (database-tools.md §6),
        // and a key's only object-level actions are deleting it and
        // setting its TTL, both writes.
        actions = actions | ActionSet::REFRESH | ActionSet::COPY_NAME;
        if matches!(kind, Key(_)) {
            actions = actions | ActionSet::OPEN_CONSOLE;
        }
        if !caps.read_only && matches!(kind, Key(_)) {
            actions = actions | ActionSet::DELETE_KEY | ActionSet::TTL_SET;
        }
    }

    if !caps.read_only {
        if is_relation || is_routine {
            actions = actions | ActionSet::DROP;
        }
        if matches!(kind, Table | View | MaterializedView) || is_routine {
            // No rename for `Collection`: Mongo's `renameCollection` is an
            // admin-database command most drivers keep separate from the
            // ordinary CRUD surface `db-drivers::mongodb` implements, and
            // it is not in this phase's scope — drop is enough for now.
            actions = actions | ActionSet::RENAME;
        }
        if matches!(kind, Table) {
            actions = actions | ActionSet::TRUNCATE | ActionSet::IMPORT_DATA;
        }
        if (is_relation || is_routine) && caps.supports_comment {
            actions = actions | ActionSet::COMMENT;
        }
        // F4.4's object dialogs: a schema/catalog/keyspace root offers
        // "Create Table…"/"Create User…" (both are schema-level DDL, not
        // one table's own action); an existing table offers "Modify
        // Table…"/"Add Column…"/"Create Index…".
        if is_schema_like {
            actions = actions | ActionSet::CREATE_TABLE | ActionSet::CREATE_USER;
        }
        if matches!(kind, Table) {
            actions =
                actions | ActionSet::MODIFY_TABLE | ActionSet::ADD_COLUMN | ActionSet::CREATE_INDEX;
        }
    }
    actions
}

/// Grouping/filter/sort knobs [`flatten`] applies together — bundled so a
/// caller (the bridge's `setGrouping`/`setFilter`) has one thing to hold
/// rather than four positional arguments growing unreadable.
#[derive(Debug, Clone)]
pub struct FlattenOptions {
    pub group_mode: GroupMode,
    pub separate_routines: bool,
    pub object_types: ObjectTypeFilter,
    pub pattern: PatternFilter,
    pub sort: SortOrder,
    pub caps: SourceCapabilities,
}

impl Default for FlattenOptions {
    fn default() -> Self {
        Self {
            group_mode: GroupMode::ByObjectType,
            separate_routines: false,
            object_types: ObjectTypeFilter::all(),
            pattern: PatternFilter::default(),
            sort: SortOrder::Natural,
            caps: SourceCapabilities {
                read_only: false,
                supports_comment: true,
            },
        }
    }
}

/// Flatten a schema snapshot's roots into the rows the dock renders, in
/// order, honouring every knob in `options`.
pub fn flatten(roots: &[Node], options: &FlattenOptions) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut refs: Vec<&Node> = roots.iter().collect();
    sort_nodes(&mut refs, options.sort);
    push_children(&refs, "", &[], 0, options, &mut rows);
    rows
}

fn push_children(
    nodes: &[&Node],
    parent_id: &str,
    object_path: &[String],
    depth: u32,
    options: &FlattenOptions,
    out: &mut Vec<TreeRow>,
) {
    let filtered: Vec<&Node> = nodes
        .iter()
        .filter(|node| {
            options.object_types.allows(node.kind)
                && options.pattern.matches(&node.name, kind_prefix(node.kind))
        })
        .copied()
        .collect();

    match options.group_mode {
        GroupMode::Flat => {
            for node in filtered {
                push_node(node, parent_id, object_path, depth, options, out);
            }
        }
        GroupMode::ByObjectType => {
            // Non-groupable kinds (schemas, columns, indexes, …) render
            // directly; groupable kinds (tables/views/routines/…) collect
            // into one folder per kind, in a fixed, stable folder order.
            let mut direct = Vec::new();
            let mut folders: Vec<(FolderKind, Vec<&Node>)> = Vec::new();
            for node in filtered {
                match folder_for(node.kind) {
                    Some(mut folder) => {
                        if node.kind == ObjectKind::Routine && options.separate_routines {
                            // A routine's own DDL decides Procedure vs
                            // Function; `db_core::schema::Node` carries no
                            // such distinction today, so both share one
                            // "Procedures" folder until a backend reports
                            // the split — tracked as a follow-up, not a
                            // silent misgroup (there is only one folder to
                            // put it in either way).
                            folder = FolderKind::Procedures;
                        }
                        match folders.iter_mut().find(|(kind, _)| *kind == folder) {
                            Some((_, bucket)) => bucket.push(node),
                            None => folders.push((folder, vec![node])),
                        }
                    }
                    None => direct.push(node),
                }
            }
            for node in direct {
                push_node(node, parent_id, object_path, depth, options, out);
            }
            folders.sort_by_key(|(kind, _)| folder_order(*kind));
            for (folder, mut bucket) in folders {
                sort_nodes(&mut bucket, options.sort);
                let folder_id = if parent_id.is_empty() {
                    folder.label().to_string()
                } else {
                    format!("{parent_id}/{}", folder.label())
                };
                out.push(TreeRow {
                    depth,
                    node_id: folder_id.clone(),
                    kind: RowKind::Folder(folder),
                    label: folder.label().to_string(),
                    detail: String::new(),
                    actions: ActionSet::NONE,
                    expandable: true,
                    loaded: true,
                    // A folder is never a real object — its children's
                    // own ancestry picks up exactly where it left off.
                    object_path: object_path.to_vec(),
                    primary_key: false,
                });
                for node in bucket {
                    push_node(node, &folder_id, object_path, depth + 1, options, out);
                }
            }
        }
    }
}

fn push_node(
    node: &Node,
    parent_id: &str,
    object_path: &[String],
    depth: u32,
    options: &FlattenOptions,
    out: &mut Vec<TreeRow>,
) {
    let node_id = if parent_id.is_empty() {
        node.name.clone()
    } else {
        format!("{parent_id}/{}", node.name)
    };
    let mut child_path = object_path.to_vec();
    child_path.push(node.name.clone());
    let (expandable, loaded, children) = match &node.children {
        Children::NotLoaded => (is_expandable_kind(node.kind), false, None),
        Children::Loaded(children) => (
            !children.is_empty() || is_expandable_kind(node.kind),
            true,
            Some(children),
        ),
    };
    out.push(TreeRow {
        depth,
        node_id: node_id.clone(),
        kind: RowKind::Object(node.kind),
        label: node.name.clone(),
        detail: String::new(),
        actions: actions_for(node.kind, options.caps),
        expandable,
        loaded,
        object_path: child_path.clone(),
        primary_key: node.kind == ObjectKind::Column && node.detail.primary_key,
    });
    if let Some(children) = children {
        let mut refs: Vec<&Node> = children.iter().collect();
        sort_nodes(&mut refs, options.sort);
        push_children(&refs, &node_id, &child_path, depth + 1, options, out);
    }
}

fn is_expandable_kind(kind: ObjectKind) -> bool {
    matches!(
        kind,
        ObjectKind::Catalog
            | ObjectKind::Schema
            | ObjectKind::Table
            | ObjectKind::View
            | ObjectKind::MaterializedView
            | ObjectKind::Collection
            | ObjectKind::Keyspace
            | ObjectKind::KeyNamespace
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::RedisType;

    fn table(name: &str, columns: Vec<&str>) -> Node {
        Node::with_children(
            name,
            ObjectKind::Table,
            columns
                .into_iter()
                .map(|c| Node::leaf(c, ObjectKind::Column))
                .collect(),
        )
    }

    fn caps() -> SourceCapabilities {
        SourceCapabilities {
            read_only: false,
            supports_comment: true,
        }
    }

    #[test]
    fn flatten_groups_tables_and_views_into_separate_folders_by_default() {
        let roots = vec![
            table("users", vec!["id"]),
            Node::leaf("active_users", ObjectKind::View),
        ];
        let options = FlattenOptions::default();
        let rows = flatten(&roots, &options);
        let folder_labels: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r.kind {
                RowKind::Folder(f) => Some(f.label()),
                _ => None,
            })
            .collect();
        assert_eq!(folder_labels, vec!["Tables", "Views"]);
        let users_row = rows.iter().find(|r| r.label == "users").unwrap();
        assert_eq!(users_row.depth, 1);
        assert_eq!(users_row.node_id, "Tables/users");
    }

    #[test]
    fn flat_grouping_renders_every_object_as_a_direct_sibling() {
        let roots = vec![
            table("users", vec!["id"]),
            Node::leaf("active_users", ObjectKind::View),
        ];
        let options = FlattenOptions {
            group_mode: GroupMode::Flat,
            ..FlattenOptions::default()
        };
        let rows = flatten(&roots, &options);
        assert!(!rows.iter().any(|r| matches!(r.kind, RowKind::Folder(_))));
        assert_eq!(rows.iter().filter(|r| r.depth == 0).count(), 2);
    }

    #[test]
    fn a_primary_key_column_row_carries_primary_key_true_and_a_plain_one_does_not() {
        let mut node = table("users", vec!["id", "name"]);
        let Children::Loaded(children) = &mut node.children else {
            unreachable!()
        };
        children[0].detail.primary_key = true;
        let rows = flatten(&[node], &FlattenOptions::default());
        let id = rows.iter().find(|r| r.label == "id").expect("id column");
        let name = rows
            .iter()
            .find(|r| r.label == "name")
            .expect("name column");
        assert!(id.primary_key);
        assert!(!name.primary_key);
    }

    #[test]
    fn a_folder_row_is_never_a_primary_key() {
        let roots = vec![table("users", vec!["id"])];
        let rows = flatten(&roots, &FlattenOptions::default());
        let folder = rows
            .iter()
            .find(|r| matches!(r.kind, RowKind::Folder(_)))
            .expect("a Tables folder");
        assert!(!folder.primary_key);
    }

    #[test]
    fn expanding_a_loaded_table_yields_its_columns_as_child_rows() {
        let roots = vec![table("users", vec!["id", "name"])];
        let rows = flatten(&roots, &FlattenOptions::default());
        let columns: Vec<&str> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Object(ObjectKind::Column)))
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(columns, vec!["id", "name"]);
    }

    #[test]
    fn a_not_loaded_table_is_expandable_but_reports_unloaded() {
        let roots = vec![Node::leaf("users", ObjectKind::Table)];
        let rows = flatten(&roots, &FlattenOptions::default());
        let users_row = rows.iter().find(|r| r.label == "users").unwrap();
        assert!(users_row.expandable);
        assert!(!users_row.loaded);
    }

    #[test]
    fn a_column_is_never_expandable() {
        let roots = vec![table("users", vec!["id"])];
        let rows = flatten(&roots, &FlattenOptions::default());
        let id_row = rows.iter().find(|r| r.label == "id").unwrap();
        assert!(!id_row.expandable);
    }

    #[test]
    fn object_type_filter_excludes_a_whole_kind() {
        let roots = vec![
            table("users", vec!["id"]),
            Node::leaf("active_users", ObjectKind::View),
        ];
        let options = FlattenOptions {
            object_types: ObjectTypeFilter::excluding(vec![ObjectKind::View]),
            ..FlattenOptions::default()
        };
        let rows = flatten(&roots, &options);
        assert!(!rows.iter().any(|r| r.label == "active_users"));
        assert!(rows.iter().any(|r| r.label == "users"));
    }

    #[test]
    fn a_plain_pattern_is_a_case_insensitive_substring_over_every_row() {
        let roots = vec![table("Users", vec!["id"]), table("orders", vec!["id"])];
        let options = FlattenOptions {
            pattern: PatternFilter::new("user"),
            ..FlattenOptions::default()
        };
        let rows = flatten(&roots, &options);
        let labels: Vec<&str> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Object(ObjectKind::Table)))
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Users"]);
    }

    #[test]
    fn a_kind_scoped_negated_pattern_hides_matching_rows_of_that_kind_only() {
        let roots = vec![table("payment_log", vec!["id"]), table("users", vec!["id"])];
        let options = FlattenOptions {
            pattern: PatternFilter::new("table:-payment_.*"),
            ..FlattenOptions::default()
        };
        let rows = flatten(&roots, &options);
        let labels: Vec<&str> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Object(ObjectKind::Table)))
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(labels, vec!["users"]);
    }

    #[test]
    fn a_kind_scoped_pattern_never_matches_a_different_kind() {
        let roots = vec![table("users", vec!["id_column"])];
        let options = FlattenOptions {
            pattern: PatternFilter::new("table:users"),
            group_mode: GroupMode::Flat,
            ..FlattenOptions::default()
        };
        let rows = flatten(&roots, &options);
        // The column `id_column` is a child of `users`, not itself a
        // `table`-kind row, so a `table:` pattern must not keep it merely
        // because its parent matched at a different depth.
        assert!(rows.iter().any(|r| r.label == "id_column"));
        let table_rows: Vec<&str> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Object(ObjectKind::Table)))
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(table_rows, vec!["users"]);
    }

    #[test]
    fn natural_sort_orders_table2_before_table10() {
        let roots = vec![table("table10", vec![]), table("table2", vec![])];
        let rows = flatten(&roots, &FlattenOptions::default());
        let labels: Vec<&str> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Object(ObjectKind::Table)))
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(labels, vec!["table2", "table10"]);
    }

    #[test]
    fn alphabetical_sort_orders_table10_before_table2() {
        let roots = vec![table("table10", vec![]), table("table2", vec![])];
        let options = FlattenOptions {
            sort: SortOrder::Alphabetical,
            ..FlattenOptions::default()
        };
        let rows = flatten(&roots, &options);
        let labels: Vec<&str> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Object(ObjectKind::Table)))
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(labels, vec!["table10", "table2"]);
    }

    #[test]
    fn a_table_offers_the_full_read_write_action_set() {
        let actions = actions_for(ObjectKind::Table, caps());
        assert!(actions.contains(ActionSet::OPEN_CONSOLE));
        assert!(actions.contains(ActionSet::EDIT_DATA));
        assert!(actions.contains(ActionSet::GO_TO_DDL));
        assert!(actions.contains(ActionSet::RENAME));
        assert!(actions.contains(ActionSet::DROP));
        assert!(actions.contains(ActionSet::TRUNCATE));
        assert!(actions.contains(ActionSet::COMMENT));
        assert!(actions.contains(ActionSet::ER_DIAGRAM));
        assert!(actions.contains(ActionSet::EXPORT_DATA));
        assert!(actions.contains(ActionSet::COPY_TABLE));
        assert!(actions.contains(ActionSet::IMPORT_DATA));
    }

    #[test]
    fn a_read_only_source_still_offers_export_and_copy_but_not_import() {
        let read_only = SourceCapabilities {
            read_only: true,
            supports_comment: true,
        };
        let actions = actions_for(ObjectKind::Table, read_only);
        // Reading rows out, or copying them elsewhere, never writes this
        // source — only importing *into* it does.
        assert!(actions.contains(ActionSet::EXPORT_DATA));
        assert!(actions.contains(ActionSet::COPY_TABLE));
        assert!(!actions.contains(ActionSet::IMPORT_DATA));
    }

    #[test]
    fn a_view_offers_export_but_no_import_or_copy_table() {
        let actions = actions_for(ObjectKind::View, caps());
        assert!(actions.contains(ActionSet::EXPORT_DATA));
        assert!(!actions.contains(ActionSet::IMPORT_DATA));
        assert!(!actions.contains(ActionSet::COPY_TABLE));
    }

    #[test]
    fn a_read_only_source_offers_no_rename_drop_truncate_or_comment() {
        let read_only = SourceCapabilities {
            read_only: true,
            supports_comment: true,
        };
        let actions = actions_for(ObjectKind::Table, read_only);
        assert!(!actions.contains(ActionSet::RENAME));
        assert!(!actions.contains(ActionSet::DROP));
        assert!(!actions.contains(ActionSet::TRUNCATE));
        assert!(!actions.contains(ActionSet::COMMENT));
        // Read-only affordances stay available.
        assert!(actions.contains(ActionSet::OPEN_CONSOLE));
        assert!(actions.contains(ActionSet::GO_TO_DDL));
    }

    #[test]
    fn a_dialect_with_no_comment_support_never_offers_comment() {
        let no_comment = SourceCapabilities {
            read_only: false,
            supports_comment: false,
        };
        assert!(!actions_for(ObjectKind::Table, no_comment).contains(ActionSet::COMMENT));
    }

    #[test]
    fn a_view_offers_no_truncate_or_edit_data() {
        let actions = actions_for(ObjectKind::View, caps());
        assert!(!actions.contains(ActionSet::TRUNCATE));
        assert!(!actions.contains(ActionSet::EDIT_DATA));
        assert!(actions.contains(ActionSet::GO_TO_DDL));
    }

    #[test]
    fn a_column_offers_no_object_level_actions() {
        assert_eq!(actions_for(ObjectKind::Column, caps()), ActionSet::NONE);
    }

    #[test]
    fn a_schema_offers_refresh_and_copy_but_no_drop_or_rename() {
        let actions = actions_for(ObjectKind::Schema, caps());
        assert!(actions.contains(ActionSet::REFRESH));
        assert!(actions.contains(ActionSet::COPY_NAME));
        assert!(!actions.contains(ActionSet::DROP));
        assert!(!actions.contains(ActionSet::RENAME));
    }

    #[test]
    fn a_collection_offers_open_console_and_drop_but_no_edit_data_or_er_diagram() {
        let actions = actions_for(ObjectKind::Collection, caps());
        assert!(actions.contains(ActionSet::OPEN_CONSOLE));
        assert!(actions.contains(ActionSet::GO_TO_DDL));
        assert!(actions.contains(ActionSet::DROP));
        assert!(!actions.contains(ActionSet::RENAME));
        assert!(!actions.contains(ActionSet::EDIT_DATA));
        assert!(!actions.contains(ActionSet::ER_DIAGRAM));
    }

    #[test]
    fn a_read_only_collection_offers_no_drop() {
        let read_only = SourceCapabilities {
            read_only: true,
            supports_comment: true,
        };
        assert!(!actions_for(ObjectKind::Collection, read_only).contains(ActionSet::DROP));
    }

    #[test]
    fn a_keyspace_offers_refresh_and_copy_but_no_console_or_drop() {
        let actions = actions_for(ObjectKind::Keyspace, caps());
        assert!(actions.contains(ActionSet::REFRESH));
        assert!(actions.contains(ActionSet::COPY_NAME));
        assert!(!actions.contains(ActionSet::OPEN_CONSOLE));
        assert!(!actions.contains(ActionSet::DROP));
    }

    #[test]
    fn a_redis_key_offers_console_delete_and_ttl() {
        let actions = actions_for(ObjectKind::Key(RedisType::String), caps());
        assert!(actions.contains(ActionSet::OPEN_CONSOLE));
        assert!(actions.contains(ActionSet::DELETE_KEY));
        assert!(actions.contains(ActionSet::TTL_SET));
        assert!(actions.contains(ActionSet::COPY_NAME));
    }

    #[test]
    fn a_read_only_redis_key_offers_no_delete_or_ttl() {
        let read_only = SourceCapabilities {
            read_only: true,
            supports_comment: true,
        };
        let actions = actions_for(ObjectKind::Key(RedisType::String), read_only);
        assert!(!actions.contains(ActionSet::DELETE_KEY));
        assert!(!actions.contains(ActionSet::TTL_SET));
        // Reading a key's value is not a write.
        assert!(actions.contains(ActionSet::OPEN_CONSOLE));
    }

    #[test]
    fn a_key_namespace_offers_no_delete_ttl_or_console() {
        let actions = actions_for(ObjectKind::KeyNamespace, caps());
        assert!(actions.contains(ActionSet::REFRESH));
        assert!(!actions.contains(ActionSet::DELETE_KEY));
        assert!(!actions.contains(ActionSet::TTL_SET));
        assert!(!actions.contains(ActionSet::OPEN_CONSOLE));
    }

    #[test]
    fn action_set_union_and_contains() {
        let both = ActionSet::DROP | ActionSet::RENAME;
        assert!(both.contains(ActionSet::DROP));
        assert!(both.contains(ActionSet::RENAME));
        assert!(!both.contains(ActionSet::TRUNCATE));
    }

    #[test]
    fn object_path_skips_synthetic_folder_segments_but_node_id_keeps_them() {
        let roots = vec![table("users", vec!["id"])];
        let rows = flatten(&roots, &FlattenOptions::default());
        let users_row = rows.iter().find(|r| r.label == "users").unwrap();
        assert_eq!(users_row.node_id, "Tables/users");
        assert_eq!(users_row.object_path, vec!["users".to_string()]);
        let id_row = rows.iter().find(|r| r.label == "id").unwrap();
        assert_eq!(id_row.node_id, "Tables/users/id");
        assert_eq!(
            id_row.object_path,
            vec!["users".to_string(), "id".to_string()]
        );
    }

    #[test]
    fn regex_lite_supports_dot_and_star_over_a_full_string() {
        assert!(regex_lite_match("payment_.*", "payment_log"));
        assert!(!regex_lite_match("payment_.*", "user_log"));
        assert!(regex_lite_match(".*", "anything"));
        assert!(!regex_lite_match("abc", "abcd"));
    }

    #[test]
    fn a_schema_offers_create_table_and_create_user_when_writable() {
        let actions = actions_for(ObjectKind::Schema, caps());
        assert!(actions.contains(ActionSet::CREATE_TABLE));
        assert!(actions.contains(ActionSet::CREATE_USER));
    }

    #[test]
    fn a_read_only_source_offers_no_create_table_or_create_user() {
        let read_only = SourceCapabilities {
            read_only: true,
            supports_comment: true,
        };
        let actions = actions_for(ObjectKind::Schema, read_only);
        assert!(!actions.contains(ActionSet::CREATE_TABLE));
        assert!(!actions.contains(ActionSet::CREATE_USER));
    }

    #[test]
    fn a_table_offers_modify_add_column_and_create_index_when_writable() {
        let actions = actions_for(ObjectKind::Table, caps());
        assert!(actions.contains(ActionSet::MODIFY_TABLE));
        assert!(actions.contains(ActionSet::ADD_COLUMN));
        assert!(actions.contains(ActionSet::CREATE_INDEX));
    }

    #[test]
    fn a_read_only_table_offers_none_of_the_f4_4_write_actions() {
        let read_only = SourceCapabilities {
            read_only: true,
            supports_comment: true,
        };
        let actions = actions_for(ObjectKind::Table, read_only);
        assert!(!actions.contains(ActionSet::MODIFY_TABLE));
        assert!(!actions.contains(ActionSet::ADD_COLUMN));
        assert!(!actions.contains(ActionSet::CREATE_INDEX));
    }

    #[test]
    fn a_view_offers_no_f4_4_table_only_write_actions() {
        let actions = actions_for(ObjectKind::View, caps());
        assert!(!actions.contains(ActionSet::MODIFY_TABLE));
        assert!(!actions.contains(ActionSet::ADD_COLUMN));
        assert!(!actions.contains(ActionSet::CREATE_INDEX));
    }
}
