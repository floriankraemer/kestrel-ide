//! Single-root project state: directory tree snapshot, "Open Folder" logic,
//! and last-opened-project persistence.
//!
//! No Qt dependency — pure Rust,
//! unit-testable. `ui-shell` wraps [`DirectoryTree`] in a
//! `QAbstractItemModel` later; this crate only owns the tree data.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod rebuild;
mod watcher;
pub use rebuild::RebuildCoalescer;
pub use watcher::{is_structural_change, EventKind, ProjectWatcher};

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

/// One entry in the project's directory tree — a plain Rust arena node,
/// no Qt awareness. `ui-shell` wraps this arena in a `QAbstractItemModel`.
pub struct TreeNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// An in-memory snapshot of the project root's directory tree, built by
/// walking the root once. Node 0 is always the root.
/// Directory the project search index is written into (owned by
/// `index-core`, mirrored here only so the sidebar tree can skip it).
const INDEX_DIR_NAME: &str = ".ide-index";

pub struct DirectoryTree {
    nodes: Vec<TreeNode>,
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

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Walk `root` recursively and build the arena, with each directory's
    /// children sorted folders-first, then case-insensitively by name
    /// ascending — matching every file manager's default and JetBrains'
    /// default. `apply_sort_order` re-sorts an already-built tree without
    /// touching the filesystem, for the sort-direction toggle.
    fn build(root: &Path) -> io::Result<Self> {
        let mut nodes = vec![TreeNode {
            path: root.to_path_buf(),
            name: root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.to_string_lossy().into_owned()),
            is_dir: true,
            parent: None,
            children: Vec::new(),
        }];
        Self::walk(&mut nodes, 0, root)?;
        let mut tree = Self { nodes };
        tree.apply_sort_order(SortOrder::Ascending);
        Ok(tree)
    }

    fn walk(nodes: &mut Vec<TreeNode>, parent_id: usize, dir: &Path) -> io::Result<()> {
        let entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;

        for entry in entries {
            let path = entry.path();
            let is_dir = entry.file_type()?.is_dir();
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_dir && name == INDEX_DIR_NAME {
                // The search index the IDE writes into the project is an
                // implementation detail, not part of the user's project.
                continue;
            }

            let id = nodes.len();
            nodes.push(TreeNode {
                path: path.clone(),
                name,
                is_dir,
                parent: Some(parent_id),
                children: Vec::new(),
            });
            nodes[parent_id].children.push(id);

            if is_dir {
                Self::walk(nodes, id, &path)?;
            }
        }
        Ok(())
    }

    /// Re-order every node's children in place: folders before files in
    /// both directions, then by name — case-insensitively, with the raw
    /// name as a tie-break so casing differences stay deterministic.
    /// `order` flips the name comparison only; folders stay on top either
    /// way, matching a file manager's "Sort by name, descending".
    pub fn apply_sort_order(&mut self, order: SortOrder) {
        for id in 0..self.nodes.len() {
            let mut children = std::mem::take(&mut self.nodes[id].children);
            children.sort_by(|&a, &b| Self::compare_nodes(&self.nodes[a], &self.nodes[b], order));
            self.nodes[id].children = children;
        }
    }

    fn compare_nodes(a: &TreeNode, b: &TreeNode, order: SortOrder) -> std::cmp::Ordering {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            let name_order = a
                .name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name));
            match order {
                SortOrder::Ascending => name_order,
                SortOrder::Descending => name_order.reverse(),
            }
        })
    }
}

/// Direction the project tree's children are sorted in. Folders always sort
/// above files in either direction — only the name comparison within each
/// group flips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
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
/// tree snapshot. Returns an error without touching any caller state —
/// callers decide whether/when to replace their current project.
pub fn open_folder(path: impl AsRef<Path>) -> Result<Project, OpenFolderError> {
    let path = path.as_ref();

    let metadata = fs::metadata(path).map_err(|_| OpenFolderError::NotFound(path.to_path_buf()))?;
    if !metadata.is_dir() {
        return Err(OpenFolderError::NotADirectory(path.to_path_buf()));
    }
    fs::read_dir(path).map_err(|e| OpenFolderError::NotReadable(path.to_path_buf(), e))?;

    let tree = DirectoryTree::build(path)
        .map_err(|e| OpenFolderError::NotReadable(path.to_path_buf(), e))?;

    Ok(Project {
        root: ProjectRoot {
            path: path.to_path_buf(),
        },
        tree,
    })
}

/// [`open_folder`], with the tree sorted the caller's way. The pure,
/// `AppSession`-free piece of "open folder" — no thread-local, no Qt
/// dependency — so it is safe to run on a worker thread rather than the Qt
/// main thread (ADR-0037).
pub fn open_folder_sorted(
    path: impl AsRef<Path>,
    order: SortOrder,
) -> Result<Project, OpenFolderError> {
    let mut project = open_folder(path)?;
    project.tree.apply_sort_order(order);
    Ok(project)
}

/// Re-walk `root` from disk and sort — the pure piece of a tree rebuild
/// (e.g. after a filesystem-watcher event), safe to run off the Qt thread
/// for the same reason [`open_folder_sorted`] is. Skips the existence/
/// readability checks `open_folder` does, since `root` is by construction
/// an already-open project's root.
pub fn rebuild_tree_sorted(root: &Path, order: SortOrder) -> io::Result<DirectoryTree> {
    let mut tree = DirectoryTree::build(root)?;
    tree.apply_sort_order(order);
    Ok(tree)
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
    /// `None` until `start_watcher` is called, or after a project with no
    /// watcher started yet.
    watcher: Option<ProjectWatcher>,
    /// Persists across "Open Folder" and tree rebuilds, so a mutation or a
    /// watcher-triggered refresh doesn't silently reset the user's chosen
    /// direction back to ascending.
    sort_order: SortOrder,
}

impl ProjectSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sort_order(&self) -> SortOrder {
        self.sort_order
    }

    /// Change the sort direction and re-order the open tree in place, if
    /// any — no filesystem walk, since the arena already has everything
    /// `apply_sort_order` needs.
    pub fn set_sort_order(&mut self, order: SortOrder) {
        self.sort_order = order;
        if let Some(project) = self.current.as_mut() {
            project.tree.apply_sort_order(order);
        }
    }

    /// (Re)start the filesystem watcher for the current project root,
    /// replacing any previous watcher (single-watcher-per-session — the
    /// previous one is dropped, which stops it). No-op (returning `Ok`) if
    /// no project is open. `on_change` runs on `notify`'s background
    /// thread; the caller (`ui-shell`) is responsible for marshaling any
    /// Qt-object updates onto the Qt thread from within it. The
    /// `notify::EventKind` is passed through (not collapsed to just a path)
    /// so the caller can tell a structural change (create/remove/rename)
    /// apart from a content-only write to a file that already exists in
    /// the tree.
    ///
    /// Returns the `notify` error on failure rather than swallowing it —
    /// previously this discarded the error via `.ok()`, so a failed watch
    /// (e.g. the platform's watch-descriptor limit) left the Changes dock
    /// and the tree silently stale with no way to tell why.
    pub fn start_watcher(
        &mut self,
        on_change: impl Fn(notify::EventKind, PathBuf) + Send + 'static,
    ) -> notify::Result<()> {
        self.watcher = None;
        if let Some(project) = &self.current {
            self.watcher = Some(ProjectWatcher::start(project.root.path(), on_change)?);
        }
        Ok(())
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
    pub fn install_project(&mut self, project: Project) {
        self.current = Some(project);
    }

    /// Swap in an already re-walked tree for the still-current project, e.g.
    /// after a filesystem-watcher rebuild ran off the Qt thread (ADR-0037).
    /// `root` must match the currently open project's root; a mismatch means
    /// the project changed while the rebuild was in flight, so the stale
    /// result is dropped instead of applied. Returns whether it was applied.
    pub fn install_tree(&mut self, root: &Path, tree: DirectoryTree) -> bool {
        match self.current.as_mut() {
            Some(project) if project.root.path() == root => {
                project.tree = tree;
                true
            }
            _ => false,
        }
    }

    /// Re-snapshot the current project's tree from disk — a full rebuild,
    /// not incremental diffing, which is a legitimate MVP-scope choice since
    /// there's no filesystem watcher yet driving fine-grained updates
    /// Callers use this after a create/rename/delete mutation
    /// (US-2b). No-op if no project is open.
    pub fn rebuild_tree(&mut self) -> io::Result<()> {
        let Some(project) = self.current.as_ref() else {
            return Ok(());
        };
        let root_path = project.root.path().to_path_buf();
        let tree = rebuild_tree_sorted(&root_path, self.sort_order)?;
        // `current` cannot have changed between the two lines above (no
        // yield point in synchronous code), so this always applies.
        self.current.as_mut().expect("checked Some above").tree = tree;
        Ok(())
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
mod tests {
    use super::*;

    fn make_fixture_tree(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("empty_dir")).unwrap();
        fs::write(root.join("README.md"), "hello").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
    }

    #[test]
    fn tree_building_reflects_fixture_directory() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());

        let project = open_folder(dir.path()).unwrap();
        let tree = &project.tree;

        let root_id = tree.root_id();
        let root_children: Vec<&str> = tree
            .children(root_id)
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(root_children, vec!["empty_dir", "src", "README.md"]);

        let src_id = tree
            .children(root_id)
            .iter()
            .find(|&&id| tree.node(id).name == "src")
            .copied()
            .unwrap();
        assert!(tree.node(src_id).is_dir);
        let src_children: Vec<&str> = tree
            .children(src_id)
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(src_children, vec!["lib.rs", "main.rs"]);

        let empty_id = tree
            .children(root_id)
            .iter()
            .find(|&&id| tree.node(id).name == "empty_dir")
            .copied()
            .unwrap();
        assert!(tree.children(empty_id).is_empty());
    }

    #[test]
    fn folders_stay_above_files_and_names_sort_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("zebra_dir")).unwrap();
        fs::write(dir.path().join("apple.txt"), "").unwrap();
        fs::write(dir.path().join("Banana.txt"), "").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();

        let project = open_folder(dir.path()).unwrap();
        let tree = &project.tree;
        let names: Vec<&str> = tree
            .children(tree.root_id())
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["zebra_dir", "apple.txt", "Banana.txt", "Cargo.toml"]
        );
    }

    #[test]
    fn descending_reverses_names_within_each_group_but_keeps_folders_on_top() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());

        let mut project = open_folder(dir.path()).unwrap();
        project.tree.apply_sort_order(SortOrder::Descending);
        let names: Vec<&str> = project
            .tree
            .children(project.tree.root_id())
            .iter()
            .map(|&id| project.tree.node(id).name.as_str())
            .collect();
        assert_eq!(names, vec!["src", "empty_dir", "README.md"]);
    }

    #[test]
    fn set_sort_order_reorders_the_open_tree_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());
        let config_dir = tempfile::tempdir().unwrap();
        let mut session = ProjectSession::new();
        session.open_folder(dir.path(), config_dir.path()).unwrap();
        assert_eq!(session.sort_order(), SortOrder::Ascending);

        session.set_sort_order(SortOrder::Descending);
        assert_eq!(session.sort_order(), SortOrder::Descending);
        let tree = &session.current().unwrap().tree;
        let names: Vec<&str> = tree
            .children(tree.root_id())
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(names, vec!["src", "empty_dir", "README.md"]);

        // rebuild_tree() re-walks the filesystem (a create/rename/delete
        // mutation) and must keep the chosen direction, not silently reset
        // it back to ascending.
        fs::write(dir.path().join("zzz.txt"), "").unwrap();
        session.rebuild_tree().unwrap();
        let tree = &session.current().unwrap().tree;
        let names: Vec<&str> = tree
            .children(tree.root_id())
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(names, vec!["src", "empty_dir", "zzz.txt", "README.md"]);
    }

    #[test]
    fn open_folder_sorted_applies_the_requested_order() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());

        let project = open_folder_sorted(dir.path(), SortOrder::Descending).unwrap();
        let tree = &project.tree;
        let names: Vec<&str> = tree
            .children(tree.root_id())
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        // Folders still lead either way; only the name order within each
        // group flips.
        assert_eq!(names, vec!["src", "empty_dir", "README.md"]);
    }

    #[test]
    fn rebuild_tree_sorted_reflects_disk_changes() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());
        fs::write(dir.path().join("zzz.txt"), "").unwrap();

        let tree = rebuild_tree_sorted(dir.path(), SortOrder::Ascending).unwrap();
        let names: Vec<&str> = tree
            .children(tree.root_id())
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(names, vec!["empty_dir", "src", "README.md", "zzz.txt"]);
    }

    /// `install_project`/`install_tree` are the swap-in half of an
    /// off-thread open/rebuild (ADR-0037): the walk already happened
    /// elsewhere, only installing the result into the session is left.
    #[test]
    fn install_project_replaces_the_current_project() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());
        let project = open_folder_sorted(dir.path(), SortOrder::Ascending).unwrap();

        let mut session = ProjectSession::new();
        assert!(session.current().is_none());
        session.install_project(project);
        assert_eq!(session.current().unwrap().root.path(), dir.path());
    }

    #[test]
    fn install_tree_applies_only_when_the_root_still_matches() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());
        let config_dir = tempfile::tempdir().unwrap();
        let mut session = ProjectSession::new();
        session.open_folder(dir.path(), config_dir.path()).unwrap();

        fs::write(dir.path().join("new_file.txt"), "").unwrap();

        // A stale rebuild for a root that is no longer open is dropped.
        let other_dir = tempfile::tempdir().unwrap();
        let rebuilt = rebuild_tree_sorted(dir.path(), session.sort_order()).unwrap();
        assert!(!session.install_tree(other_dir.path(), rebuilt));
        let tree = &session.current().unwrap().tree;
        assert!(!tree
            .children(tree.root_id())
            .iter()
            .any(|&id| tree.node(id).name == "new_file.txt"));

        // A rebuild whose root still matches is applied.
        let rebuilt = rebuild_tree_sorted(dir.path(), session.sort_order()).unwrap();
        assert!(session.install_tree(dir.path(), rebuilt));
        let tree = &session.current().unwrap().tree;
        assert!(tree
            .children(tree.root_id())
            .iter()
            .any(|&id| tree.node(id).name == "new_file.txt"));
    }

    #[test]
    fn opening_nonexistent_path_errors_without_mutating_state() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());
        let config_dir = tempfile::tempdir().unwrap();

        let mut session = ProjectSession::new();
        session.open_folder(dir.path(), config_dir.path()).unwrap();
        assert_eq!(session.current().unwrap().root.path(), dir.path());

        let missing = dir.path().join("does-not-exist");
        let result = session.open_folder(&missing, config_dir.path());
        assert!(matches!(result, Err(OpenFolderError::NotFound(_))));

        // Current project must be unchanged after the failed open.
        assert_eq!(session.current().unwrap().root.path(), dir.path());
    }

    #[test]
    fn opening_unreadable_path_errors_without_mutating_state() {
        use std::os::unix::fs::PermissionsExt;

        // Permission bits don't block root, and our mandatory Docker build
        // runs tests as root — skip rather than assert a false positive.
        let is_root = std::process::Command::new("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false);
        if is_root {
            eprintln!(
                "skipping opening_unreadable_path_errors_without_mutating_state: running as root"
            );
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        make_fixture_tree(dir.path());
        let config_dir = tempfile::tempdir().unwrap();

        let mut session = ProjectSession::new();
        session.open_folder(dir.path(), config_dir.path()).unwrap();

        let unreadable = tempfile::tempdir().unwrap();
        let mut perms = fs::metadata(unreadable.path()).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(unreadable.path(), perms.clone()).unwrap();

        let result = session.open_folder(unreadable.path(), config_dir.path());
        assert!(matches!(result, Err(OpenFolderError::NotReadable(_, _))));
        assert_eq!(session.current().unwrap().root.path(), dir.path());

        // restore perms so tempdir cleanup can remove it
        perms.set_mode(0o755);
        fs::set_permissions(unreadable.path(), perms).unwrap();
    }

    #[test]
    fn last_opened_project_persists_and_reopens() {
        let project_dir = tempfile::tempdir().unwrap();
        make_fixture_tree(project_dir.path());
        let config_dir = tempfile::tempdir().unwrap();

        let mut session = ProjectSession::new();
        session
            .open_folder(project_dir.path(), config_dir.path())
            .unwrap();

        // Simulate a fresh app launch: a brand-new session, same config dir.
        let mut reopened_session = ProjectSession::new();
        let opened = reopened_session.reopen_last(config_dir.path()).unwrap();

        assert!(opened);
        assert_eq!(
            reopened_session.current().unwrap().root.path(),
            project_dir.path()
        );
    }

    #[test]
    fn reopen_last_with_nothing_persisted_is_a_noop() {
        let config_dir = tempfile::tempdir().unwrap();
        let mut session = ProjectSession::new();
        let opened = session.reopen_last(config_dir.path()).unwrap();
        assert!(!opened);
        assert!(session.current().is_none());
    }

    #[test]
    fn create_file_appears_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_file(dir.path(), "new.txt").unwrap();
        assert!(path.is_file());
        assert_eq!(path, dir.path().join("new.txt"));
    }

    #[test]
    fn create_file_errors_when_name_taken() {
        let dir = tempfile::tempdir().unwrap();
        create_file(dir.path(), "dup.txt").unwrap();
        let result = create_file(dir.path(), "dup.txt");
        assert!(matches!(result, Err(FileOpError::AlreadyExists(_))));
    }

    #[test]
    fn create_folder_appears_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_folder(dir.path(), "newdir").unwrap();
        assert!(path.is_dir());
    }

    #[test]
    fn create_folder_errors_when_name_taken() {
        let dir = tempfile::tempdir().unwrap();
        create_folder(dir.path(), "dup").unwrap();
        let result = create_folder(dir.path(), "dup");
        assert!(matches!(result, Err(FileOpError::AlreadyExists(_))));
    }

    #[test]
    fn rename_path_moves_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_file(dir.path(), "old.txt").unwrap();
        let new_path = rename_path(&path, "renamed.txt").unwrap();
        assert!(!path.exists());
        assert!(new_path.is_file());
        assert_eq!(new_path, dir.path().join("renamed.txt"));
    }

    #[test]
    fn rename_path_errors_when_target_name_taken() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_file(dir.path(), "a.txt").unwrap();
        create_file(dir.path(), "b.txt").unwrap();
        let result = rename_path(&path, "b.txt");
        assert!(matches!(result, Err(FileOpError::AlreadyExists(_))));
        assert!(path.exists(), "original must be untouched on error");
    }

    #[test]
    fn rename_path_errors_when_source_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("ghost.txt");
        let result = rename_path(&missing, "renamed.txt");
        assert!(matches!(result, Err(FileOpError::NotFound(_))));
    }

    #[test]
    fn delete_path_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_file(dir.path(), "gone.txt").unwrap();
        delete_path(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn delete_path_removes_nonempty_folder_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let folder = create_folder(dir.path(), "subdir").unwrap();
        create_file(&folder, "inside.txt").unwrap();
        fs::create_dir(folder.join("nested")).unwrap();
        fs::write(folder.join("nested/deep.txt"), "x").unwrap();

        delete_path(&folder).unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn delete_path_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let result = delete_path(&missing);
        assert!(matches!(result, Err(FileOpError::NotFound(_))));
    }

    #[test]
    fn rebuild_tree_reflects_mutations() {
        let project_dir = tempfile::tempdir().unwrap();
        make_fixture_tree(project_dir.path());
        let config_dir = tempfile::tempdir().unwrap();

        let mut session = ProjectSession::new();
        session
            .open_folder(project_dir.path(), config_dir.path())
            .unwrap();

        create_file(project_dir.path(), "brand_new.txt").unwrap();
        session.rebuild_tree().unwrap();

        let tree = &session.current().unwrap().tree;
        let root_children: Vec<&str> = tree
            .children(tree.root_id())
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert!(root_children.contains(&"brand_new.txt"));
    }

    #[test]
    fn the_search_index_directory_is_hidden_from_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".ide-index")).unwrap();
        fs::write(dir.path().join(".ide-index/meta.json"), "{}").unwrap();
        fs::write(dir.path().join("visible.txt"), "x").unwrap();

        let tree = DirectoryTree::build(dir.path()).unwrap();
        let names: Vec<&str> = (0..tree.len())
            .map(|i| tree.node(i).name.as_str())
            .collect();

        assert!(names.contains(&"visible.txt"));
        assert!(!names.contains(&".ide-index"));
        assert!(!names.contains(&"meta.json"));
    }
}
