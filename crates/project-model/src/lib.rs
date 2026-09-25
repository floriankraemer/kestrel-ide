//! Single-root project state: a lazily-loaded directory tree snapshot,
//! "Open Folder" logic, and last-opened-project persistence.
//!
//! No Qt dependency — pure Rust,
//! unit-testable. `ui-shell` wraps [`DirectoryTree`] in a
//! `QAbstractItemModel` later; this crate only owns the tree data.
//!
//! The tree is lazy, the same model IntelliJ and VS Code use (plan:
//! `docs/architecture/fast-project-open-plan.md`, "Step 1"): opening a
//! project reads only the root's direct children ([`DirectoryTree::open_root`]),
//! and a directory's own children are read from disk only when something
//! asks for them ([`DirectoryTree::attach_children`]), typically the Qt
//! model's `fetchMore` on first expand. A directory's [`LoadState`] tracks
//! which state it's in.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod coalesce;
/// What is part of the project, independent of `.gitignore` (ADR-0064).
pub mod scope;
mod walk;
mod watcher;
pub use coalesce::RefreshCoalescer;
pub use scope::ProjectScope;
pub use walk::walk_project;
pub use watcher::{route_change, ChangeRouting, EventKind, ProjectWatcher};

/// File name used to persist the last-opened project path, per the plan's
/// "single plain-text line, no serde/toml/json" decision.
const LAST_PROJECT_FILE: &str = "last-project.txt";

/// The single open project's root folder (MVP is single-root only, per
/// mvp-proposal.md resolved question 6 — do not generalize to multi-root).
pub struct ProjectRoot {
    path: PathBuf,
}

impl ProjectRoot {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Why "Open Folder" failed. Carries enough detail for a clear user-facing
/// error message (US-1's error-handling acceptance criterion) without
/// mutating any existing project state.
#[derive(Debug)]
pub enum OpenFolderError {
    NotFound(PathBuf),
    NotADirectory(PathBuf),
    NotReadable(PathBuf, io::Error),
}

impl fmt::Display for OpenFolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenFolderError::NotFound(p) => {
                write!(f, "folder does not exist: {}", p.display())
            }
            OpenFolderError::NotADirectory(p) => {
                write!(f, "not a folder: {}", p.display())
            }
            OpenFolderError::NotReadable(p, err) => {
                write!(f, "folder is not readable: {} ({err})", p.display())
            }
        }
    }
}

impl std::error::Error for OpenFolderError {}

/// Directory the project search index is written into (owned by
/// `index-core`, mirrored here only so the sidebar tree can skip it).
const INDEX_DIR_NAME: &str = ".ide-index";

/// Direction the project tree's children are sorted in. Folders always sort
/// above files in either direction — only the name comparison within each
/// group flips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

/// Compare two directory entries the way every file manager and JetBrains'
/// own tree do: folders before files in both directions, then by name —
/// case-insensitively via `key`, with `name` (the raw, cased string) as a
/// tie-break so casing differences stay deterministic. `order` flips only
/// the name comparison; folders stay on top either way.
///
/// Free function, not a method, so both [`list_dir`] (reading fresh names
/// off disk) and [`DirectoryTree::resort_loaded`] (re-ordering an
/// already-built subtree in memory, no disk access) can share it without
/// either owning the other's data shape.
fn compare_entries(
    a_is_dir: bool,
    a_key: &str,
    a_name: &str,
    b_is_dir: bool,
    b_key: &str,
    b_name: &str,
    order: SortOrder,
) -> std::cmp::Ordering {
    b_is_dir.cmp(&a_is_dir).then_with(|| {
        let name_order = a_key.cmp(b_key).then_with(|| a_name.cmp(b_name));
        match order {
            SortOrder::Ascending => name_order,
            SortOrder::Descending => name_order.reverse(),
        }
    })
}

/// One entry a [`list_dir`] read off disk: not yet a tree node (no arena id,
/// no parent, no children) — [`DirectoryTree::attach_children`]/
/// [`DirectoryTree::refresh_dir`] turn a batch of these into one.
pub struct ListedEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// `name.to_lowercase()`, computed once here rather than per pairwise
    /// comparison later — the plan's explicit ask, since the old
    /// `compare_nodes` called `.to_lowercase()` inside the comparator itself.
    sort_key: String,
}

/// One `read_dir` of `path`, skipping the project's own search-index
/// directory, sorted `order`'s way (folders first, then name — see
/// [`compare_entries`]). The pure, disk-touching half of loading one
/// directory's worth of the lazy tree — safe to run on a worker thread,
/// same reasoning as the old `open_folder_sorted`/`rebuild_tree_sorted`
/// this replaces.
///
/// An entry whose `file_type()` can't be determined is skipped rather than
/// failing the whole listing — one bad entry must not hide the rest of an
/// otherwise-readable directory.
pub fn list_dir(path: &Path, order: SortOrder) -> io::Result<Vec<ListedEntry>> {
    let mut entries: Vec<ListedEntry> = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == INDEX_DIR_NAME {
                return None;
            }
            let is_dir = entry.file_type().ok()?.is_dir();
            let sort_key = name.to_lowercase();
            Some(ListedEntry {
                path: entry.path(),
                name,
                is_dir,
                sort_key,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        compare_entries(
            a.is_dir,
            &a.sort_key,
            &a.name,
            b.is_dir,
            &b.sort_key,
            &b.name,
            order,
        )
    });
    Ok(entries)
}

/// Whether a directory's children have been read from disk yet. Files carry
/// this too (always [`LoadState::Loaded`], since a file never has children
/// to fetch) purely so [`TreeNode`] needs one field, not an
/// `Option<LoadState>` that's meaningless for half of what it's on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadState {
    #[default]
    Unloaded,
    /// A worker is already listing this directory; guards against a second
    /// `fetchMore` (Qt's own view prefetching can fire twice in a row)
    /// spawning a second worker for the same directory.
    Loading,
    Loaded,
}

/// One entry in the project's directory tree — a plain Rust arena node, no
/// Qt awareness. `ui-shell` wraps this arena in a `QAbstractItemModel`.
pub struct TreeNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// This node's row within `parent`'s `children` — kept correct on every
    /// insert/remove so `QAbstractItemModel::parent()`'s "what row is my
    /// parent in *its* parent" question is an O(1) read, not a linear scan
    /// of the grandparent's children.
    pub index_in_parent: usize,
    /// Meaningless for a file (see [`LoadState`]'s doc comment) but always
    /// [`LoadState::Loaded`] for one, so callers never have to special-case
    /// "is this a file" before reading it.
    load_state: LoadState,
    sort_key: String,
}

/// A stable removed-slot id, and the diff a [`DirectoryTree::refresh_dir`]
/// hands back so a `QAbstractItemModel` can apply it as ranged
/// `beginRemoveRows`/`beginInsertRows` pairs rather than a full reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirDiffOp {
    /// Rows `first..=last` (inclusive, Qt's own convention) were removed
    /// from the directory's children, in the order they used to appear.
    Remove { first: usize, last: usize },
    /// Rows `first..=last` (inclusive) are new, already in their sorted
    /// position.
    Insert { first: usize, last: usize },
}

/// The ranged remove/insert operations [`DirectoryTree::refresh_dir`]
/// computed for one directory. Applied in order: every `Remove` describes a
/// position in the *old* row numbering, every `Insert` a position in the
/// row numbering that results after every `Remove` before it has already
/// been applied — exactly the sequencing `beginRemoveRows`/
/// `beginInsertRows` expect from repeated calls against the same parent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirDiff {
    pub ops: Vec<DirDiffOp>,
}

/// An in-memory snapshot of the project root's directory tree — an arena
/// loaded lazily, one directory at a time. Node 0 is always the root.
pub struct DirectoryTree {
    nodes: Vec<TreeNode>,
    /// Ids of tombstoned nodes, reusable for a genuinely new insert — never
    /// proactively; see [`DirectoryTree::alloc_node`]'s doc comment for why
    /// reuse is safe exactly when it happens.
    free: Vec<usize>,
    /// O(1) path → id lookup, maintained on every insert/tombstone, so a
    /// caller (the Qt model's `fetchMore`/watcher-refresh/`ensurePathLoaded`
    /// chain) never has to scan the arena to find "the node for this path".
    path_to_id: HashMap<PathBuf, usize>,
}

impl DirectoryTree {
    pub fn root_id(&self) -> usize {
        0
    }

    pub fn node(&self, id: usize) -> &TreeNode {
        &self.nodes[id]
    }

    pub fn children(&self, id: usize) -> &[usize] {
        &self.nodes[id].children
    }

    pub fn is_dir(&self, id: usize) -> bool {
        self.nodes[id].is_dir
    }

    pub fn load_state(&self, id: usize) -> LoadState {
        self.nodes[id].load_state
    }

    /// This node's row within its own parent — see the field's doc comment
    /// on [`TreeNode::index_in_parent`].
    pub fn index_in_parent(&self, id: usize) -> usize {
        self.nodes[id].index_in_parent
    }

    /// O(1) lookup of the arena id for an absolute path, or `None` if that
    /// path isn't currently in the (partially loaded) tree at all — either
    /// it doesn't exist, or an ancestor of it hasn't been expanded yet.
    pub fn node_by_path(&self, path: &Path) -> Option<usize> {
        self.path_to_id.get(path).copied()
    }

    /// Mark `id` as a worker having started listing it — a no-op unless it
    /// is currently [`LoadState::Unloaded`], so a caller can call this
    /// unconditionally right before spawning a worker without checking the
    /// state itself first.
    pub fn set_loading(&mut self, id: usize) {
        if self.nodes[id].load_state == LoadState::Unloaded {
            self.nodes[id].load_state = LoadState::Loading;
        }
    }

    /// The first ancestor of `target` (walking from the root down) that
    /// isn't [`LoadState::Loaded`] yet, or `None` if every ancestor down to
    /// (but not including) `target` itself is already loaded — meaning
    /// `target`'s immediate parent's children are all in the arena, so
    /// `target` itself is either present or genuinely doesn't exist.
    /// `target` need not itself resolve to a node (a file is never
    /// "loaded" in this sense).
    ///
    /// Used by `ui-shell`'s `ensurePathLoaded`: one call per still-unloaded
    /// level, walked from the top, rather than trying to resolve the whole
    /// chain in one shot.
    pub fn first_unloaded_ancestor(&self, target: &Path) -> Option<PathBuf> {
        let root_path = self.nodes[self.root_id()].path.clone();
        if target == root_path {
            return None;
        }
        if self.load_state(self.root_id()) != LoadState::Loaded {
            return Some(root_path);
        }
        let relative = target.strip_prefix(&root_path).ok()?;
        let mut current = root_path;
        for component in relative.components() {
            current = current.join(component.as_os_str());
            if current == target {
                return None;
            }
            let Some(id) = self.node_by_path(&current) else {
                // Nothing more can be loaded toward a path that doesn't
                // exist on disk (or under a directory we haven't listed) —
                // the caller stops the chain here.
                return None;
            };
            if self.load_state(id) != LoadState::Loaded {
                return Some(current);
            }
        }
        None
    }

    /// Build the arena with just the root node, its children read from one
    /// `read_dir` call. Never fails: an unreadable root simply becomes a
    /// Loaded-empty node (the "one unreadable directory must never fail the
    /// whole open" fix — the old recursive `walk` propagated this failure
    /// with `?`) — `open_folder_sorted`'s own existence/readability checks
    /// already cover the common "path is wrong" cases with a proper error
    /// before this ever runs.
    pub fn open_root(root: &Path, order: SortOrder) -> Self {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let sort_key = name.to_lowercase();
        let mut tree = Self {
            nodes: vec![TreeNode {
                path: root.to_path_buf(),
                name,
                sort_key,
                is_dir: true,
                parent: None,
                children: Vec::new(),
                index_in_parent: 0,
                load_state: LoadState::Unloaded,
            }],
            free: Vec::new(),
            path_to_id: HashMap::new(),
        };
        tree.path_to_id.insert(root.to_path_buf(), 0);
        let entries = list_dir(root, order).unwrap_or_default();
        tree.attach_children(tree.root_id(), entries);
        tree
    }

    /// Insert `entries` as `dir_id`'s children, mark it
    /// [`LoadState::Loaded`], and return the newly inserted ids in the same
    /// (already-sorted, per [`list_dir`]) order — the Qt model uses the
    /// count to call `beginInsertRows(parent, 0, count - 1)`/
    /// `endInsertRows` (a directory always has zero children before its
    /// first load, so the new rows always start at 0).
    pub fn attach_children(&mut self, dir_id: usize, entries: Vec<ListedEntry>) -> Vec<usize> {
        let ids: Vec<usize> = entries
            .into_iter()
            .map(|entry| self.alloc_node(entry, dir_id))
            .collect();
        for (row, &id) in ids.iter().enumerate() {
            self.nodes[id].index_in_parent = row;
        }
        self.nodes[dir_id].children = ids.clone();
        self.nodes[dir_id].load_state = LoadState::Loaded;
        ids
    }

    /// Diff a fresh listing of `dir_id` (already [`LoadState::Loaded`])
    /// against what it currently holds: a child matched by name+kind keeps
    /// its id and whatever subtree it already loaded; one no longer present
    /// is tombstoned (its own subtree recursively tombstoned with it, since
    /// it becomes unreachable either way); a genuinely new name gets a
    /// fresh (or reused-from-`free`) id. A rename is one remove and one
    /// insert — detecting "same inode, different name" is deliberately out
    /// of scope (the plan calls this out explicitly).
    ///
    /// Returns the ranged operations a `QAbstractItemModel` applies as
    /// `beginRemoveRows`/`beginInsertRows` pairs — see [`DirDiff`]'s own
    /// doc comment for the exact sequencing contract.
    pub fn refresh_dir(&mut self, dir_id: usize, entries: Vec<ListedEntry>) -> DirDiff {
        let old_children = self.nodes[dir_id].children.clone();
        let mut by_name: HashMap<(String, bool), usize> = old_children
            .iter()
            .map(|&id| {
                let node = &self.nodes[id];
                ((node.name.clone(), node.is_dir), id)
            })
            .collect();

        let mut new_children = Vec::with_capacity(entries.len());
        for entry in entries {
            match by_name.remove(&(entry.name.clone(), entry.is_dir)) {
                Some(id) => new_children.push(id),
                None => new_children.push(self.alloc_node(entry, dir_id)),
            }
        }

        // Whatever's left in `by_name` (everything matched above was
        // removed from it) is gone from disk.
        for &id in by_name.values() {
            self.tombstone_subtree(id);
        }

        for (row, &id) in new_children.iter().enumerate() {
            self.nodes[id].index_in_parent = row;
        }
        self.nodes[dir_id].children = new_children.clone();

        row_diff(&old_children, &new_children)
    }

    /// Re-order every currently-[`LoadState::Loaded`] directory's children
    /// in place for the sort-direction toggle — no filesystem access, since
    /// each node's [`TreeNode::sort_key`] was already computed when it was
    /// listed. An [`LoadState::Unloaded`] directory needs nothing done now:
    /// whenever it does get listed, [`list_dir`] is given the new `order`
    /// and sorts accordingly.
    pub fn resort_loaded(&mut self, order: SortOrder) {
        for id in 0..self.nodes.len() {
            if self.nodes[id].is_dir && self.nodes[id].load_state == LoadState::Loaded {
                let mut children = std::mem::take(&mut self.nodes[id].children);
                children.sort_by(|&a, &b| compare_ids(&self.nodes, a, b, order));
                for (row, &child_id) in children.iter().enumerate() {
                    self.nodes[child_id].index_in_parent = row;
                }
                self.nodes[id].children = children;
            }
        }
    }

    /// Allocate a node for `entry` under `parent_id`, reusing a tombstoned
    /// slot if one is free rather than always growing the arena. Safe to
    /// reuse a slot immediately: it is only ever pushed onto `free` by
    /// [`Self::tombstone_subtree`], which callers only reach *after* the
    /// corresponding `beginRemoveRows`/`endRemoveRows` pair has already run
    /// against the Qt model — by the time a new insert can observe the
    /// reused id, Qt itself has already been told the old row (and its
    /// persistent indexes) are gone.
    fn alloc_node(&mut self, entry: ListedEntry, parent_id: usize) -> usize {
        let node = TreeNode {
            path: entry.path.clone(),
            name: entry.name,
            sort_key: entry.sort_key,
            is_dir: entry.is_dir,
            parent: Some(parent_id),
            children: Vec::new(),
            index_in_parent: 0,
            load_state: LoadState::Unloaded,
        };
        let id = match self.free.pop() {
            Some(id) => {
                self.nodes[id] = node;
                id
            }
            None => {
                let id = self.nodes.len();
                self.nodes.push(node);
                id
            }
        };
        self.path_to_id.insert(entry.path, id);
        id
    }

    /// Recursively remove `id` and everything under it: drop its path from
    /// the lookup table and push its slot onto the free list. The subtree
    /// is tombstoned rather than merely detached because, once its parent
    /// no longer lists it, nothing else can reach it — leaving it "alive"
    /// would just be an unreachable-but-not-freed leak, and its ids would
    /// never become available for a genuinely new insert.
    fn tombstone_subtree(&mut self, id: usize) {
        let children = std::mem::take(&mut self.nodes[id].children);
        for child_id in children {
            self.tombstone_subtree(child_id);
        }
        self.path_to_id.remove(&self.nodes[id].path);
        self.free.push(id);
    }
}

/// [`DirectoryTree::resort_loaded`]'s comparator, as a free function over
/// the raw node slice rather than a `&self` method: it runs while a
/// `children` vector has already been taken out of the node it belongs to
/// (`std::mem::take`), so borrowing `self` immutably for the comparison
/// while a *different* field is mutated afterward needs no split-borrow
/// gymnastics this way.
fn compare_ids(nodes: &[TreeNode], a: usize, b: usize, order: SortOrder) -> std::cmp::Ordering {
    let (na, nb) = (&nodes[a], &nodes[b]);
    compare_entries(
        na.is_dir,
        &na.sort_key,
        &na.name,
        nb.is_dir,
        &nb.sort_key,
        &nb.name,
        order,
    )
}

/// Diff two ordered id lists (a directory's children before and after a
/// refresh) into the smallest prefix/suffix-trimmed remove+insert pair:
/// trim the common leading run and the common trailing run (ids are stable
/// across a refresh for anything that survived, so `==` is a legitimate
/// identity check here), and treat whatever's left in the middle as "the
/// old middle went away, the new middle arrived". This isn't a minimal
/// diff for a change scattered across many non-adjacent rows, but it is
/// always correct, and a plain directory listing changing by one or two
/// entries — by far the common case — collapses to exactly one remove and
/// one insert.
fn row_diff(old: &[usize], new: &[usize]) -> DirDiff {
    let mut start = 0;
    while start < old.len() && start < new.len() && old[start] == new[start] {
        start += 1;
    }
    let mut old_end = old.len();
    let mut new_end = new.len();
    while old_end > start && new_end > start && old[old_end - 1] == new[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    let mut ops = Vec::with_capacity(2);
    if old_end > start {
        ops.push(DirDiffOp::Remove {
            first: start,
            last: old_end - 1,
        });
    }
    if new_end > start {
        ops.push(DirDiffOp::Insert {
            first: start,
            last: new_end - 1,
        });
    }
    DirDiff { ops }
}

/// A successfully opened project: its root and the directory tree snapshot
/// taken at open time.
pub struct Project {
    pub root: ProjectRoot,
    pub tree: DirectoryTree,
}

/// Why a create/rename/delete filesystem-mutation operation (US-2b) failed.
/// Carries enough detail for a clear user-facing message.
#[derive(Debug)]
pub enum FileOpError {
    AlreadyExists(PathBuf),
    NotFound(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for FileOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileOpError::AlreadyExists(p) => write!(f, "already exists: {}", p.display()),
            FileOpError::NotFound(p) => write!(f, "no such file or folder: {}", p.display()),
            FileOpError::Io(p, err) => write!(f, "{}: {err}", p.display()),
        }
    }
}

impl std::error::Error for FileOpError {}

/// Create an empty file named `name` inside `parent_dir`. Errors if
/// something with that name already exists there.
pub fn create_file(parent_dir: &Path, name: &str) -> Result<PathBuf, FileOpError> {
    let path = parent_dir.join(name);
    if path.exists() {
        return Err(FileOpError::AlreadyExists(path));
    }
    fs::File::create(&path).map_err(|e| FileOpError::Io(path.clone(), e))?;
    Ok(path)
}

/// Create an empty folder named `name` inside `parent_dir`. Errors if
/// something with that name already exists there.
pub fn create_folder(parent_dir: &Path, name: &str) -> Result<PathBuf, FileOpError> {
    let path = parent_dir.join(name);
    if path.exists() {
        return Err(FileOpError::AlreadyExists(path));
    }
    fs::create_dir(&path).map_err(|e| FileOpError::Io(path.clone(), e))?;
    Ok(path)
}

/// Rename `path` (file or folder) to `new_name`, staying in the same parent
/// directory. Errors if `path` doesn't exist or `new_name` is already taken.
pub fn rename_path(path: &Path, new_name: &str) -> Result<PathBuf, FileOpError> {
    if !path.exists() {
        return Err(FileOpError::NotFound(path.to_path_buf()));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let new_path = parent.join(new_name);
    if new_path.exists() {
        return Err(FileOpError::AlreadyExists(new_path));
    }
    fs::rename(path, &new_path).map_err(|e| FileOpError::Io(path.to_path_buf(), e))?;
    Ok(new_path)
}

/// Delete `path` — recursively if it's a folder. Errors if it doesn't exist.
pub fn delete_path(path: &Path) -> Result<(), FileOpError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| FileOpError::NotFound(path.to_path_buf()))?;
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|e| FileOpError::Io(path.to_path_buf(), e))
}

/// Validate `path` exists, is a directory, and is readable, then build the
/// tree snapshot (root level only — see [`DirectoryTree::open_root`]).
/// Returns an error without touching any caller state — callers decide
/// whether/when to replace their current project.
pub fn open_folder_sorted(
    path: impl AsRef<Path>,
    order: SortOrder,
) -> Result<Project, OpenFolderError> {
    let path = path.as_ref();

    let metadata = fs::metadata(path).map_err(|_| OpenFolderError::NotFound(path.to_path_buf()))?;
    if !metadata.is_dir() {
        return Err(OpenFolderError::NotADirectory(path.to_path_buf()));
    }
    fs::read_dir(path).map_err(|e| OpenFolderError::NotReadable(path.to_path_buf(), e))?;

    let tree = DirectoryTree::open_root(path, order);

    Ok(Project {
        root: ProjectRoot {
            path: path.to_path_buf(),
        },
        tree,
    })
}

/// [`open_folder_sorted`] with the default (ascending) order — most tests'
/// entry point, and every call site that doesn't itself track a sort
/// direction.
pub fn open_folder(path: impl AsRef<Path>) -> Result<Project, OpenFolderError> {
    open_folder_sorted(path, SortOrder::Ascending)
}

/// Every file and folder currently reachable under `root`, gitignore-aware,
/// skipping the project's own search index — for a caller that needs the
/// *whole* project tree flattened (MCP's `list_project_tree`, the AI
/// agent's own tool) rather than whatever the lazy on-screen arena happens
/// to have loaded so far. Runs its own fresh walk rather than reading
/// [`DirectoryTree`] precisely because that arena is partial by design; safe
/// to call from any thread (a plain `ignore::WalkBuilder` walk, no shared
/// state), which is what lets both consumers run it on their own
/// already-off-the-Qt-thread worker.
pub fn walk_all_entries(root: &Path) -> Vec<(PathBuf, bool)> {
    let mut entries = Vec::new();
    walk_project(
        root,
        |builder| {
            builder
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                // A `.gitignore` should apply even in a project that isn't (yet) a
                // git repository itself — `require_git` defaults to `true`, which
                // would otherwise silently stop honoring it the moment there's no
                // `.git` directory to find.
                .require_git(false)
                .filter_entry(|entry| entry.file_name() != INDEX_DIR_NAME);
        },
        |entry| {
            if entry.path() != root {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                entries.push((entry.into_path(), is_dir));
            }
        },
    );
    entries
}

/// Persist `project_path` as the last-opened project: one plain-text line
/// in `config_dir` (per plan §3 — deliberately not serde/toml/json).
pub fn persist_last_project(config_dir: &Path, project_path: &Path) -> io::Result<()> {
    fs::create_dir_all(config_dir)?;
    fs::write(
        config_dir.join(LAST_PROJECT_FILE),
        project_path.to_string_lossy().as_bytes(),
    )
}

/// Read the last-opened project path, if any was persisted.
pub fn read_last_project(config_dir: &Path) -> io::Result<Option<PathBuf>> {
    match fs::read_to_string(config_dir.join(LAST_PROJECT_FILE)) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PathBuf::from(trimmed)))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The platform config dir the real app persists into (`dirs::config_dir()`
/// joined with `ide`). Tests should use their own temp dir instead of this,
/// to avoid touching the developer's real `~/.config`.
pub fn default_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ide"))
}

/// Session-scoped holder for "the one open project", matching US-1: opening
/// a new folder replaces the previous project and its tree; opening an
/// invalid folder leaves the current project untouched.
#[derive(Default)]
pub struct ProjectSession {
    current: Option<Project>,
    /// The single watcher for the current project root (plan §2: one
    /// `notify` instance, replaced — not added to — on a new project open).
    /// `None` until [`Self::install_watcher`] is called, or after a project
    /// with no watcher started yet. Registration itself
    /// (`ProjectWatcher::start`) now runs off this session entirely — see
    /// `ProjectTreeModel::start_watcher_async` (`ui-shell`) — so this field
    /// only ever receives an already-built watcher.
    watcher: Option<ProjectWatcher>,
    /// Persists across "Open Folder" and directory listings, so a mutation
    /// or a watcher-triggered refresh doesn't silently reset the user's
    /// chosen direction back to ascending.
    sort_order: SortOrder,
}

impl ProjectSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sort_order(&self) -> SortOrder {
        self.sort_order
    }

    /// Change the sort direction and re-order every already-loaded
    /// directory in place — no filesystem access; see
    /// [`DirectoryTree::resort_loaded`].
    pub fn set_sort_order(&mut self, order: SortOrder) {
        self.sort_order = order;
        if let Some(project) = self.current.as_mut() {
            project.tree.resort_loaded(order);
        }
    }

    pub fn current(&self) -> Option<&Project> {
        self.current.as_ref()
    }

    /// Open `path` as the active project and persist it as "last opened".
    /// On validation failure, the current project (if any) is left
    /// unchanged.
    pub fn open_folder(
        &mut self,
        path: impl AsRef<Path>,
        config_dir: &Path,
    ) -> Result<(), OpenFolderError> {
        let project = open_folder_sorted(path, self.sort_order)?;
        // Persistence failure shouldn't prevent the project from opening —
        // it only degrades "reopen last project" on next launch.
        let _ = persist_last_project(config_dir, project.root.path());
        self.current = Some(project);
        Ok(())
    }

    /// Install an already walked-and-sorted project as current, replacing
    /// any previous one — the swap-in half of "Open Folder" when the walk
    /// itself ran off the Qt thread (ADR-0037). Persisting "last opened" is
    /// the caller's job, same as it is off of [`open_folder`]'s pure half,
    /// `open_folder_sorted`.
    ///
    /// Returns whatever project and watcher were previously current, so the
    /// caller can drop them off the Qt thread: a huge previous tree's frees
    /// and its watcher's OS-level teardown must not block paint of the
    /// just-installed one any more than the walk that built it did (plan
    /// step 4). The old watcher goes out here rather than when the new
    /// one finishes registering: until then it would keep routing the old
    /// project's events into refreshes of the new one's directories.
    pub fn install_project(
        &mut self,
        project: Project,
    ) -> (Option<Project>, Option<ProjectWatcher>) {
        (self.current.replace(project), self.watcher.take())
    }

    /// Install a watcher that finished registering off the Qt thread (plan
    /// step 4): `ProjectWatcher::start`'s own `ignore` walk plus one
    /// blocking `watch()` per directory can take seconds on a large tree or
    /// a WSL/9P root, so registration itself now runs on a background
    /// thread and only the result is handed back here.
    ///
    /// `root` is the root the watcher was started for; it must still match
    /// the currently open project's root. A mismatch means a different
    /// project was opened while registration was still running, so
    /// `watcher` is handed back in `Err` for the caller to drop instead of
    /// installing a watcher for a project that is no longer open. On
    /// success, the watcher it replaces (if any) comes back in `Ok` for the
    /// same off-thread-drop treatment — this crate makes no assumption
    /// about which thread calls `install_watcher`, so it never drops
    /// anything itself.
    pub fn install_watcher(
        &mut self,
        root: &Path,
        watcher: ProjectWatcher,
    ) -> Result<Option<ProjectWatcher>, ProjectWatcher> {
        match &self.current {
            Some(project) if project.root.path() == root => Ok(self.watcher.replace(watcher)),
            _ => Err(watcher),
        }
    }

    /// Mark a directory as having a listing worker already in flight for
    /// it — a no-op if `root` no longer names the open project (the guard
    /// every other "apply an off-thread result" method here shares).
    pub fn mark_dir_loading(&mut self, root: &Path, dir_id: usize) {
        if let Some(project) = self.current.as_mut() {
            if project.root.path() == root {
                project.tree.set_loading(dir_id);
            }
        }
    }

    /// Insert an off-thread [`list_dir`] result as `dir_path`'s children —
    /// the swap-in half of the Qt model's `fetchMore`. `dir_path` (not a
    /// captured arena id) is resolved fresh here, since an id captured
    /// before the listing ran could, in principle, have been recycled by an
    /// unrelated tombstone/reuse while the worker was away; a path lookup
    /// can't be stale that way. Returns the newly inserted ids (for
    /// `beginInsertRows`), or `None` if `root` no longer names the open
    /// project or `dir_path` is no longer a node in it (e.g. removed by a
    /// watcher-driven refresh that landed first).
    pub fn attach_dir_children(
        &mut self,
        root: &Path,
        dir_path: &Path,
        entries: Vec<ListedEntry>,
    ) -> Option<Vec<usize>> {
        let project = self.current.as_mut()?;
        if project.root.path() != root {
            return None;
        }
        let dir_id = project.tree.node_by_path(dir_path)?;
        Some(project.tree.attach_children(dir_id, entries))
    }

    /// Diff an off-thread [`list_dir`] result against `dir_path`'s current
    /// children — the swap-in half of a watcher-driven or mutation-driven
    /// incremental refresh. Same stale-safety reasoning as
    /// [`Self::attach_dir_children`]: resolved fresh by path, not a
    /// captured id. `None` under the same conditions, plus when `dir_path`
    /// isn't currently [`LoadState::Loaded`] (nothing shown there to
    /// refresh).
    pub fn refresh_dir(
        &mut self,
        root: &Path,
        dir_path: &Path,
        entries: Vec<ListedEntry>,
    ) -> Option<DirDiff> {
        let project = self.current.as_mut()?;
        if project.root.path() != root {
            return None;
        }
        let dir_id = project.tree.node_by_path(dir_path)?;
        if project.tree.load_state(dir_id) != LoadState::Loaded {
            return None;
        }
        Some(project.tree.refresh_dir(dir_id, entries))
    }

    /// Reopen the last-persisted project, if any. Returns `Ok(true)` if a
    /// project was found and opened, `Ok(false)` if nothing was persisted.
    pub fn reopen_last(&mut self, config_dir: &Path) -> Result<bool, OpenFolderError> {
        let last = read_last_project(config_dir)
            .map_err(|e| OpenFolderError::NotReadable(config_dir.to_path_buf(), e))?;
        match last {
            Some(path) => {
                self.open_folder(path, config_dir)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests;
