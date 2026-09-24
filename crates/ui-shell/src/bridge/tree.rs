use core::pin::Pin;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use app_core::{AppError, AppSession};
use cxx_qt::Threading;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use project_model::{DirDiffOp, ListedEntry, LoadState};

use crate::bridge::convert::{push_recent_project, to_ffi_result};
use crate::bridge::ffi::{self, FfiResult, Roles};
use crate::bridge::registry::{shared_icons, shared_session, SharedIcons};

/// Rust side of the `ProjectTreeModel` QObject: handles on the shared
/// session and icon theme, nothing else — the tree data itself lives in
/// `app-core`.
pub struct ProjectTreeModelRust {
    session: Rc<RefCell<AppSession>>,
    icons: Rc<SharedIcons>,
    /// One [`project_model::RefreshCoalescer`] per directory currently
    /// being refreshed by a watcher-driven event burst — see
    /// `request_watcher_refresh`'s doc comment. An entry is removed once its
    /// coalescer goes idle (no refresh running, none queued), so this stays
    /// bounded by "directories with recent activity", not every directory
    /// ever loaded. Touched only from the Qt thread — every read/write sits
    /// either in a slot or in a `qt_thread.queue`d closure — so a `RefCell`
    /// is the right cell here, as elsewhere in this adapter.
    dir_refresh: RefCell<HashMap<PathBuf, project_model::RefreshCoalescer>>,
}

impl Default for ProjectTreeModelRust {
    fn default() -> Self {
        let session = shared_session();
        // Seed the shared session's sort order from the persisted setting
        // before any tree gets built — `reopenLastProject`'s startup call
        // must land in the right order the first time, not flip after.
        let descending = app_config::load(&app_core::resolve_config_dir())
            .unwrap_or_default()
            .project_tree_sort_descending;
        if descending {
            session
                .borrow_mut()
                .set_tree_sort_order(project_model::SortOrder::Descending);
        }
        Self {
            session,
            icons: shared_icons(),
            dir_refresh: RefCell::new(HashMap::new()),
        }
    }
}

/// Separates the collapsed key from the expanded one in the `IconKey` role.
///
/// The role carries *both* states of a row's icon, because that is what Qt
/// already knows how to use: a `QIcon` holds a `QIcon::Off` and a
/// `QIcon::On` pixmap, `QTreeView` paints an expanded row with
/// `QStyle::State_Open`, and `QStyledItemDelegate` turns that into
/// `QIcon::On`. Handing the view one icon that knows both states means an
/// expand or collapse repaints the open/closed folder art on its own — no
/// `dataChanged` plumbing on the expansion signals, and no C++ asking the
/// view what state a row is in.
///
/// A newline because an icon key is `<pack-id>/<icon-id>` and neither part
/// can contain one: `plugin-api` restricts the id charset and an icon id is
/// a file stem.
const ICON_KEY_STATE_SEPARATOR: char = '\n';

/// `Qt::UserRole` — the first role number Qt promises never to use itself.
const QT_USER_ROLE: i32 = 0x0100;

/// The role number a `Roles` variant actually travels as. See the `Roles`
/// doc comment: the variants are offsets, because cxx-qt cannot give them
/// discriminants of their own.
const fn user_role(role: Roles) -> i32 {
    QT_USER_ROLE + role.repr
}

impl ffi::ProjectTreeModel {
    /// Row count for `parent` — the number of children the arena node has.
    /// Files (and empty directories) simply have no children, so this
    /// naturally yields 0 without any separate "is leaf" tracking; Qt's
    /// tree view relies on that to skip drawing an expand affordance.
    pub fn row_count(&self, parent: &QModelIndex) -> i32 {
        let session = self.session.borrow();
        let Some(project) = session.project() else {
            return 0;
        };
        let tree = &project.tree;
        let node_id = if parent.is_valid() {
            parent.internal_id()
        } else {
            tree.root_id()
        };
        tree.children(node_id).len() as i32
    }

    pub fn column_count(&self, _parent: &QModelIndex) -> i32 {
        1
    }

    /// Map (row, column, parent) to a `QModelIndex` carrying the child
    /// arena node's id as `internalId` — the id is the only piece of
    /// arena-mapping state a `QModelIndex` needs to carry, since `parent()`
    /// can always re-derive a node's row by searching its own parent's
    /// children.
    pub fn index(&self, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex {
        let session = self.session.borrow();
        let Some(project) = session.project() else {
            return QModelIndex::default();
        };
        let tree = &project.tree;
        let parent_id = if parent.is_valid() {
            parent.internal_id()
        } else {
            tree.root_id()
        };
        let children = tree.children(parent_id);
        match children.get(row as usize) {
            Some(&child_id) => unsafe { self.create_index(row, column, child_id) },
            None => QModelIndex::default(),
        }
    }

    /// Map a child index back to its parent's `QModelIndex`. The arena's
    /// root node is never itself wrapped in a `QModelIndex` — it is the
    /// model's invisible root — so a child whose arena parent is the root
    /// correctly yields an invalid (root) `QModelIndex`.
    pub fn parent(&self, child: &QModelIndex) -> QModelIndex {
        let session = self.session.borrow();
        let Some(project) = session.project() else {
            return QModelIndex::default();
        };
        let tree = &project.tree;
        if !child.is_valid() {
            return QModelIndex::default();
        }
        let node = tree.node(child.internal_id());
        let Some(parent_id) = node.parent else {
            return QModelIndex::default();
        };
        if parent_id == tree.root_id() {
            return QModelIndex::default();
        }
        // O(1): `index_in_parent` is kept correct on every insert/remove,
        // rather than searching the grandparent's children for `parent_id`.
        let row = tree.index_in_parent(parent_id) as i32;
        unsafe { self.create_index(row, 0, parent_id) }
    }

    /// A directory reports it may have children until it's actually been
    /// loaded and found empty (see [`LoadState`]'s doc comment) — a file
    /// never has children, so it always reports `false` regardless of load
    /// state (files don't carry a meaningful one, see `TreeNode`'s own
    /// field doc).
    pub fn has_children(&self, parent: &QModelIndex) -> bool {
        let session = self.session.borrow();
        let Some(project) = session.project() else {
            return false;
        };
        let tree = &project.tree;
        let id = if parent.is_valid() {
            parent.internal_id()
        } else {
            tree.root_id()
        };
        if !tree.is_dir(id) {
            return false;
        }
        match tree.load_state(id) {
            LoadState::Loaded => !tree.children(id).is_empty(),
            LoadState::Unloaded | LoadState::Loading => true,
        }
    }

    /// `true` iff `parent` is a directory whose children have never been
    /// read from disk.
    pub fn can_fetch_more(&self, parent: &QModelIndex) -> bool {
        let session = self.session.borrow();
        let Some(project) = session.project() else {
            return false;
        };
        let tree = &project.tree;
        let id = if parent.is_valid() {
            parent.internal_id()
        } else {
            tree.root_id()
        };
        tree.is_dir(id) && tree.load_state(id) == LoadState::Unloaded
    }

    /// Load `parent`'s children off the Qt thread — the lazy tree's actual
    /// "expand this folder" path (plan "Step 2"). Marks `parent` as
    /// [`LoadState::Loading`] synchronously before spawning, so a second
    /// `fetchMore` call for the same row while the first is still in flight
    /// (Qt's own view prefetching can do this) sees `canFetchMore() ==
    /// false` and never spawns a second worker for the same directory.
    pub fn fetch_more(mut self: Pin<&mut Self>, parent: &QModelIndex) {
        let found = {
            let session = self.session.borrow();
            let Some(project) = session.project() else {
                return;
            };
            let tree = &project.tree;
            let id = if parent.is_valid() {
                parent.internal_id()
            } else {
                tree.root_id()
            };
            if tree.load_state(id) != LoadState::Unloaded {
                return;
            }
            (
                project.root.path().to_path_buf(),
                id,
                tree.node(id).path.clone(),
            )
        };
        let (root, dir_id, dir_path) = found;
        self.session
            .borrow_mut()
            .mark_tree_dir_loading(&root, dir_id);
        self.as_mut().spawn_attach(dir_path);
    }

    /// Load whichever ancestors of `path` are still unloaded, then emit
    /// `pathReady(path)` once every ancestor down to (not including) `path`
    /// itself is loaded — see `ffi.rs`'s doc comment on the invokable.
    pub fn ensure_path_loaded(mut self: Pin<&mut Self>, path: &QString) {
        let target = std::path::PathBuf::from(path.to_string());
        self.as_mut().load_next_ancestor(target);
    }

    fn load_next_ancestor(mut self: Pin<&mut Self>, target: std::path::PathBuf) {
        let next_unloaded = {
            let session = self.session.borrow();
            let Some(project) = session.project() else {
                return;
            };
            if !target.starts_with(project.root.path()) {
                return;
            }
            project.tree.first_unloaded_ancestor(&target)
        };
        match next_unloaded {
            Some(dir) => self.as_mut().spawn_attach_then(dir, target),
            None => {
                self.as_mut()
                    .path_ready(QString::from(target.to_string_lossy().as_ref()));
            }
        }
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let session = self.session.borrow();
        let Some(project) = session.project() else {
            return QVariant::default();
        };
        if !index.is_valid() {
            return QVariant::default();
        }
        let node = project.tree.node(index.internal_id());
        match role {
            // Qt::DisplayRole
            0 => QVariant::from(&QString::from(node.name.as_str())),
            r if r == user_role(Roles::Path) => {
                QVariant::from(&QString::from(node.path.to_string_lossy().as_ref()))
            }
            r if r == user_role(Roles::IsDir) => QVariant::from(&node.is_dir),
            r if r == user_role(Roles::IconKey) => {
                // B7 (ADR-0057): a synced Gradle/Maven source root paints
                // as the icon pack's own src/test/resources art, by role
                // rather than by this directory's real name (Maven's
                // `src/main/java` is a directory named `main`) —
                // `folder_role`'s whole point.
                let role = node
                    .is_dir
                    .then(|| {
                        crate::bridge::registry::shared_build_models()
                            .borrow()
                            .iter()
                            .find_map(|model| {
                                app_core::build_tools_tree::folder_role(&node.path, model)
                            })
                    })
                    .flatten();
                // The arena's root node is the model's invisible root, so no
                // row a view ever asks about is the project root itself.
                let key = |expanded| {
                    if let Some(role) = role {
                        return self.icons.service.borrow().folder_icon_key(
                            role.canonical_name(),
                            expanded,
                            self.icons.appearance.get(),
                        );
                    }
                    self.icons.service.borrow().icon_key(
                        &node.path,
                        node.is_dir,
                        expanded,
                        false,
                        self.icons.appearance.get(),
                    )
                };
                match (key(false), key(true)) {
                    (Some(closed), Some(open)) => QVariant::from(&QString::from(
                        format!("{closed}{ICON_KEY_STATE_SEPARATOR}{open}").as_str(),
                    )),
                    // No icon theme active: an empty key, which the
                    // decoration proxy turns into an invalid QVariant so the
                    // row reserves no icon width.
                    _ => QVariant::from(&QString::default()),
                }
            }
            // Every role Qt itself defines (decoration, edit, tooltip, size
            // hint, ...) lands here and gets an invalid QVariant, which is
            // what tells the view "this item has no icon, no tooltip, no
            // size of its own" and keeps the label flush against its own
            // branch indicator.
            _ => QVariant::default(),
        }
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        roles.insert(0, QByteArray::from("display"));
        roles.insert(user_role(Roles::Path), QByteArray::from("path"));
        roles.insert(user_role(Roles::IsDir), QByteArray::from("isDir"));
        roles.insert(user_role(Roles::IconKey), QByteArray::from("iconKey"));
        roles
    }

    /// Persisted sort direction. Reads the shared session's in-memory copy
    /// rather than `settings.toml` itself — `Default` seeds it from disk
    /// once at construction, and `set_sort_descending` below is the only
    /// writer of the setting, keeping both in sync — so this no longer
    /// costs a file read at all.
    pub fn sort_descending(&self) -> bool {
        self.session.borrow().tree_sort_order() == project_model::SortOrder::Descending
    }

    /// Flip the sort direction: persist it, then re-order the open tree
    /// in place and reset the model. No filesystem walk — the arena
    /// already has everything `apply_sort_order` needs.
    pub fn set_sort_descending(mut self: Pin<&mut Self>, descending: bool) {
        let _ = app_config::update(&app_core::resolve_config_dir(), |settings| {
            settings.project_tree_sort_descending = descending;
        });
        let order = if descending {
            project_model::SortOrder::Descending
        } else {
            project_model::SortOrder::Ascending
        };
        self.session.borrow_mut().set_tree_sort_order(order);
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().end_reset_model();
        }
    }

    /// Open `path` as the active project. Fire-and-forget (ADR-0037): the
    /// walk runs on a worker thread, and the outcome arrives later as
    /// `projectOpened`/`projectOpenFailed`.
    pub fn open_folder(self: Pin<&mut Self>, path: &QString) {
        let path = std::path::PathBuf::from(path.to_string());
        self.open_folder_async(path);
    }

    /// W7-2 (ADR-0052): read the global off switch and apply it before
    /// anything classifies this project's root — every `ExecHost::for_path`
    /// call site in the plan funnels through the same process-wide flag
    /// (`process_exec::host`'s doc comment on why), so this one read at the
    /// one place every project open passes through keeps it current.
    fn apply_remote_wsl_setting() {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        lsp_core::set_remote_wsl_enabled(settings.remote_wsl_or_default());
    }

    /// Reopen the last-persisted project (US-1), the same way as
    /// `openFolder` — fire-and-forget, walking on a worker thread. Returns
    /// whether a reopen was kicked off at all: `false` means nothing was
    /// ever persisted, so the caller (the splash screen) knows no
    /// `projectOpened`/`projectOpenFailed` is coming and startup should
    /// proceed without waiting. Reading the persisted path itself is cheap
    /// (one small text file) and stays synchronous — only the directory
    /// walk that follows is worth moving off the Qt thread.
    pub fn reopen_last_project(mut self: Pin<&mut Self>) -> bool {
        let config_dir = app_core::resolve_config_dir();
        match project_model::read_last_project(&config_dir) {
            Ok(Some(path)) => {
                self.as_mut().open_folder_async(path);
                true
            }
            _ => false,
        }
    }

    /// Shared worker for `openFolder`/`reopenLastProject`: walks `path` off
    /// the Qt thread — `project_model::open_folder_sorted` is pure, no
    /// `AppSession`, no Qt, so it is safe to run on a plain `std::thread` —
    /// and marshals the outcome back via `qt_thread.queue`, the same
    /// worker-thread shape `VcsService::open_project` uses (ADR-0037). A
    /// fresh thread per open rather than a persistent job queue: unlike
    /// VCS, there is never more than one project-open in flight that
    /// matters — a second one simply replaces whatever the first would have
    /// installed once both land.
    ///
    /// The queued install itself is split into two hops (plan step 4,
    /// "open sequence reorder"): everything the tree needs to paint runs in
    /// the first — install, `beginResetModel`/`endResetModel` — and nothing
    /// else. A second `qt_thread.queue` then carries the rest (watcher
    /// registration kickoff, `projectOpened` and everything its slots do:
    /// several `settings.toml` parses, a WSL analysis probe). Queuing again
    /// rather than calling straight through lets the event loop actually
    /// paint the reset tree first — a directly-chained call runs before Qt
    /// gets to process a paint event, so the tree would still sit blank
    /// behind however long that second half takes.
    fn open_folder_async(mut self: Pin<&mut Self>, path: std::path::PathBuf) {
        Self::apply_remote_wsl_setting();
        let order = self.session.borrow().tree_sort_order();
        let config_dir = app_core::resolve_config_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(
            move || match project_model::open_folder_sorted(&path, order) {
                Ok(project) => {
                    // Persistence failure shouldn't prevent the project from
                    // opening — it only degrades "reopen last project" next
                    // launch, same tolerance `ProjectSession::open_folder` has.
                    let _ = project_model::persist_last_project(&config_dir, project.root.path());
                    let root = project.root.path().to_path_buf();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        let previous = model.session.borrow_mut().install_opened_project(project);
                        // Borrow scoped tightly: `endResetModel` synchronously
                        // re-enters `rowCount`/`data`, which take their own
                        // borrow of the session.
                        unsafe {
                            model.as_mut().begin_reset_model();
                            model.as_mut().end_reset_model();
                        }
                        // Tree paints here. Freeing the previous project's
                        // tree (hundreds of thousands of nodes, for a big
                        // one) and tearing down its watcher must not delay
                        // that paint, so both go on a throwaway thread.
                        std::thread::spawn(move || drop(previous));

                        let qt_thread = model.qt_thread();
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model.as_mut().start_watcher_async(root.clone());
                            model.as_mut().emit_project_opened();
                            // Settings I/O, not moved to the worker thread
                            // above alongside `persist_last_project`: unlike
                            // that write (a different file, one open at a
                            // time in practice), this reads-modifies-writes
                            // the *shared* `settings.toml`, and running it
                            // here keeps it serialized on the single Qt
                            // thread — moving it to a fresh worker thread
                            // per open would let two rapid opens race the
                            // same load-then-save and drop an update.
                            push_recent_project(root);
                        });
                    });
                }
                Err(err) => {
                    let result = to_ffi_result(Err(AppError::OpenFolder(err)));
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().project_open_failed(result);
                    });
                }
            },
        );
    }

    /// Shared tail for a successful open: re-reads the now-current root
    /// path from the session (rather than trusting the caller-supplied
    /// path verbatim) and emits `projectOpened`.
    fn emit_project_opened(mut self: Pin<&mut Self>) {
        let root = self
            .session
            .borrow()
            .root_path()
            .map(|p| p.to_string_lossy().into_owned());
        if let Some(root) = root {
            self.as_mut().project_opened(QString::from(root.as_str()));
        }
    }

    /// (Re)start the filesystem watcher for `root` — the project that was
    /// just installed — replacing any previous watcher (single watcher).
    ///
    /// Registration itself (`ProjectWatcher::start`'s own `ignore` walk plus
    /// one blocking `notify::Watcher::watch()` per directory) runs on a
    /// background thread rather than here (plan step 4): on a large tree,
    /// or a WSL root where `watch()` stat-scans each directory over 9P, that
    /// walk alone used to freeze the UI for seconds — in front of the paint
    /// this method now runs well after. The finished watcher is installed
    /// back via `qt_thread.queue`, guarded by
    /// `AppSession::install_watcher`'s stale-root check: if a different
    /// project was opened while registration was still running, the
    /// watcher this call started is simply dropped instead of replacing the
    /// newer project's own.
    ///
    /// Each fs event queues a closure onto this `ProjectTreeModel`'s own Qt
    /// thread — the one cross-thread hop in the whole design — which reads
    /// `project_model::route_change`'s two-boolean answer for the event:
    /// `rebuild_tree` resets the model, `refresh_vcs` emits
    /// `filesChangedExternally(path)` for `main_window.cpp` to relay to
    /// `DocumentManager` via an ordinary (already-on-the-Qt-thread) signal
    /// connection, so US-3's reload/keep prompt for an open tab's content
    /// change keeps working. That relay is why `project-model`'s watcher
    /// only ever needs one `CxxQtThread` handle, not two. An event routed
    /// here before the watcher itself finishes installing is harmless: the
    /// tree it would rebuild was just loaded fresh by the open this watcher
    /// belongs to.
    ///
    /// Root cause of the "saving a file collapses the sidebar" bug: this
    /// used to reset the model on *every* fs event unconditionally,
    /// including the app's own `Ctrl+S` write of a file that was already in
    /// the tree — a content-only change that doesn't move a single row.
    /// `beginResetModel`/`endResetModel` throws away Qt's per-item expand
    /// state for the whole tree, so every save re-collapsed it. Filtering
    /// on the event kind here fixes both the app's own saves and genuinely
    /// external content-only edits (no reason to reset for either), while
    /// still fully rebuilding for real structural changes (US-2).
    fn start_watcher_async(mut self: Pin<&mut Self>, root: std::path::PathBuf) {
        let qt_thread = self.qt_thread();
        let install_thread = self.as_mut().qt_thread();
        // W6-1 (ADR-0052): the classification lives here, past the
        // domain/support boundary `project-model`/`app-core` stay below —
        // see `ProjectWatcher::start`'s doc comment.
        let is_remote = lsp_core::ExecHost::for_path(&root).is_remote();
        std::thread::spawn(move || {
            let event_root = root.clone();
            let result = project_model::ProjectWatcher::start(
                &root,
                is_remote,
                move |kind, changed_path| {
                    // `project_model::route_change` (issue #285) is the
                    // whole decision: `bridge.rs` only reads its answer,
                    // never re-derives it — see CLAUDE.md's "translation
                    // only" rule for this file.
                    let routing = project_model::route_change(&event_root, &kind, &changed_path);
                    // C5: the same event, mapped onto LSP's `FileChangeType` for
                    // `LanguageService::watchedFileChanged` — computed here,
                    // once, rather than in every listener.
                    let watched_kind = lsp_core::watched_files::FileChangeKind::from(kind) as i32;
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        if routing.refresh_tree {
                            // The directory the changed path lives in, not
                            // the whole tree (plan "Step 3", ADR-0062
                            // "Decision 2"): a `git checkout` of a branch
                            // with many new files now costs one `list_dir`
                            // per *currently expanded* directory it touched,
                            // not a re-walk of the entire project repeated
                            // for every event in the burst.
                            if let Some(dir) = changed_path.parent() {
                                model.as_mut().request_watcher_refresh(dir.to_path_buf());
                            }
                        }
                        let path = QString::from(changed_path.to_string_lossy().as_ref());
                        model
                            .as_mut()
                            .watched_file_changed(path.clone(), watched_kind);
                        if routing.refresh_vcs {
                            model.as_mut().files_changed_externally(path);
                        }
                    });
                },
            );
            match result {
                Ok(watcher) => {
                    let _ = install_thread.queue(move |model: Pin<&mut Self>| {
                        let outcome = model.session.borrow_mut().install_watcher(&root, watcher);
                        // Either the previous watcher this one replaces, or
                        // (a newer project having been opened meanwhile)
                        // this now-stale one itself: dropped off the Qt
                        // thread either way, since tearing down hundreds of
                        // OS-level watches synchronously is exactly what
                        // registering them off-thread exists to avoid.
                        let to_drop = outcome.unwrap_or_else(Some);
                        std::thread::spawn(move || drop(to_drop));
                    });
                }
                Err(err) => {
                    // The project itself is already open; a failed watch
                    // only means external changes (a terminal `git
                    // pull`/`checkout`/commit, an edit made outside the
                    // app) won't be noticed until it's reopened. Surfaced
                    // rather than left silent — see `AppError::WatcherFailed`.
                    let result = to_ffi_result(Err(AppError::WatcherFailed(err.to_string())));
                    let _ = install_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().watcher_failed(result);
                    });
                }
            }
        });
    }

    /// The `QModelIndex` for arena node `id` — the root's own invisible
    /// index (`QModelIndex::default()`) if `id` is the tree's root,
    /// otherwise built from `id`'s row within its own parent.
    fn model_index_for(&self, id: usize) -> QModelIndex {
        let session = self.session.borrow();
        let Some(project) = session.project() else {
            return QModelIndex::default();
        };
        if id == project.tree.root_id() {
            return QModelIndex::default();
        }
        let row = project.tree.index_in_parent(id) as i32;
        unsafe { self.create_index(row, 0, id) }
    }

    /// List `dir` off the Qt thread and insert the result as its children —
    /// the worker half of `fetchMore`, and of `ensurePathLoaded`'s per-level
    /// chain (`spawn_attach_then`, below, is the version that continues the
    /// chain once this lands).
    fn spawn_attach(mut self: Pin<&mut Self>, dir: std::path::PathBuf) {
        let Some((root, order)) = self.root_and_order() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let entries = project_model::list_dir(&dir, order).unwrap_or_default();
            let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                model.as_mut().apply_attach_children(root, dir, entries);
            });
        });
    }

    /// Same worker as [`Self::spawn_attach`], but continues
    /// `ensurePathLoaded`'s chain (`load_next_ancestor`) once this level
    /// lands, instead of stopping here.
    fn spawn_attach_then(
        mut self: Pin<&mut Self>,
        dir: std::path::PathBuf,
        target: std::path::PathBuf,
    ) {
        let Some((root, order)) = self.root_and_order() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let entries = project_model::list_dir(&dir, order).unwrap_or_default();
            let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                model.as_mut().apply_attach_children(root, dir, entries);
                model.as_mut().load_next_ancestor(target);
            });
        });
    }

    /// Insert an off-thread `list_dir` result as `dir`'s children and, if
    /// any rows were inserted, bracket it with `beginInsertRows`/
    /// `endInsertRows`. A no-op (correctly: nothing to show) if `dir` no
    /// longer resolves in the tree — see
    /// `AppSession::attach_tree_children`'s doc comment on why it's
    /// resolved fresh by path rather than trusting an id captured before
    /// the listing ran.
    fn apply_attach_children(
        mut self: Pin<&mut Self>,
        root: std::path::PathBuf,
        dir: std::path::PathBuf,
        entries: Vec<ListedEntry>,
    ) {
        let dir_id = {
            let session = self.session.borrow();
            session
                .project()
                .filter(|p| p.root.path() == root)
                .and_then(|p| p.tree.node_by_path(&dir))
        };
        let Some(dir_id) = dir_id else {
            return;
        };
        let count = entries.len();
        self.session
            .borrow_mut()
            .attach_tree_children(&root, &dir, entries);
        if count == 0 {
            return;
        }
        let parent_index = self.model_index_for(dir_id);
        unsafe {
            self.as_mut()
                .begin_insert_rows(&parent_index, 0, (count - 1) as i32);
            self.as_mut().end_insert_rows();
        }
    }

    /// A structural filesystem-watcher event says `dir`'s contents may have
    /// changed. Refreshes are coalesced per directory rather than spawned
    /// one per event: the events arrive in bursts of thousands (a `git
    /// checkout`, a `cargo build`, the watcher's own registration sweep)
    /// that can all name the same directory, and each only needs answering
    /// once. See `project_model::RefreshCoalescer`'s doc comment for the
    /// history of what unbounded spawning here used to cost.
    ///
    /// A no-op if `dir` isn't currently loaded in the tree at all — nothing
    /// is shown there, so there's nothing to refresh (and no `list_dir` call
    /// worth spending on a directory the user hasn't even opened).
    fn request_watcher_refresh(mut self: Pin<&mut Self>, dir: std::path::PathBuf) {
        let is_loaded = {
            let session = self.session.borrow();
            session
                .project()
                .and_then(|p| p.tree.node_by_path(&dir).map(|id| p.tree.load_state(id)))
                == Some(LoadState::Loaded)
        };
        if !is_loaded {
            return;
        }
        let start = self
            .dir_refresh
            .borrow_mut()
            .entry(dir.clone())
            .or_default()
            .request();
        if start {
            self.as_mut().spawn_coalesced_refresh(dir);
        }
    }

    fn spawn_coalesced_refresh(mut self: Pin<&mut Self>, dir: std::path::PathBuf) {
        let Some((root, order)) = self.root_and_order() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        let dir_for_worker = dir.clone();
        std::thread::spawn(move || {
            let entries = project_model::list_dir(&dir_for_worker, order).unwrap_or_default();
            let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                model
                    .as_mut()
                    .apply_dir_refresh(root, dir_for_worker.clone(), entries);
                model.as_mut().finish_coalesced_refresh(dir_for_worker);
            });
        });
    }

    /// Hand the directory's coalescer its "running" slot back, and start the
    /// one catch-up refresh it asks for when events arrived while this
    /// listing was in flight. Removes the coalescer entirely once it goes
    /// idle, so `dir_refresh` stays bounded by directories with recent
    /// activity rather than growing for the life of the process.
    fn finish_coalesced_refresh(self: Pin<&mut Self>, dir: std::path::PathBuf) {
        let again = {
            let mut map = self.dir_refresh.borrow_mut();
            let Some(coalescer) = map.get_mut(&dir) else {
                return;
            };
            let again = coalescer.finished();
            if !again {
                map.remove(&dir);
            }
            again
        };
        if again {
            self.spawn_coalesced_refresh(dir);
        }
    }

    /// The uncoalesced sibling of the watcher path — a single refresh of
    /// `dir`, used right after a tree-context-menu create/rename/delete
    /// mutation, so the user sees the result immediately rather than
    /// waiting on watcher latency (plan "Step 3"). A later watcher echo of
    /// the same change is a no-op diff, since `refresh_dir` diffs against
    /// what's actually on disk. A no-op if `dir` isn't currently loaded, same
    /// as [`Self::request_watcher_refresh`].
    fn refresh_dir_async(mut self: Pin<&mut Self>, dir: std::path::PathBuf) {
        let is_loaded = {
            let session = self.session.borrow();
            session
                .project()
                .and_then(|p| p.tree.node_by_path(&dir).map(|id| p.tree.load_state(id)))
                == Some(LoadState::Loaded)
        };
        if !is_loaded {
            return;
        }
        let Some((root, order)) = self.root_and_order() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let entries = project_model::list_dir(&dir, order).unwrap_or_default();
            let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                model.as_mut().apply_dir_refresh(root, dir, entries);
            });
        });
    }

    /// Diff an off-thread `list_dir` result against `dir`'s current children
    /// and apply the result as ranged `beginRemoveRows`/`beginInsertRows`
    /// pairs — never a model reset, so the view's expansion state and
    /// selection elsewhere in the tree survive. Shared by the watcher path
    /// and the mutation path (`refresh_dir_async`) so there is exactly one
    /// code path for "a loaded directory's contents changed on disk".
    fn apply_dir_refresh(
        mut self: Pin<&mut Self>,
        root: std::path::PathBuf,
        dir: std::path::PathBuf,
        entries: Vec<ListedEntry>,
    ) {
        let dir_id = {
            let session = self.session.borrow();
            session
                .project()
                .filter(|p| p.root.path() == root)
                .and_then(|p| p.tree.node_by_path(&dir))
        };
        let Some(dir_id) = dir_id else {
            return;
        };
        let Some(diff) = self
            .session
            .borrow_mut()
            .refresh_tree_dir(&root, &dir, entries)
        else {
            return;
        };
        if diff.ops.is_empty() {
            return;
        }
        let parent_index = self.model_index_for(dir_id);
        for op in diff.ops {
            match op {
                DirDiffOp::Remove { first, last } => unsafe {
                    self.as_mut()
                        .begin_remove_rows(&parent_index, first as i32, last as i32);
                    self.as_mut().end_remove_rows();
                },
                DirDiffOp::Insert { first, last } => unsafe {
                    self.as_mut()
                        .begin_insert_rows(&parent_index, first as i32, last as i32);
                    self.as_mut().end_insert_rows();
                },
            }
        }
    }

    /// The open project's root path and the tree's current sort order, or
    /// `None` if no project is open — the pair every off-thread `list_dir`
    /// worker above needs before it can spawn.
    fn root_and_order(&self) -> Option<(std::path::PathBuf, project_model::SortOrder)> {
        let session = self.session.borrow();
        let project = session.project()?;
        Some((project.root.path().to_path_buf(), session.tree_sort_order()))
    }

    pub fn root_path(&self) -> QString {
        match self.session.borrow().root_path() {
            Some(path) => QString::from(path.to_string_lossy().as_ref()),
            None => QString::default(),
        }
    }

    /// W7-1: the distro name if the open root is a WSL UNC path, empty
    /// otherwise — `cpp/`'s status-bar indicator branches on emptiness
    /// only, never on the string's shape.
    pub fn remote_wsl_distro(&self) -> QString {
        match self.session.borrow().root_path() {
            Some(path) => match lsp_core::ExecHost::for_path(path) {
                lsp_core::ExecHost::Wsl(wsl) => QString::from(wsl.distro.as_str()),
                lsp_core::ExecHost::Local => QString::default(),
            },
            None => QString::default(),
        }
    }

    /// W7-1: the Linux path `remote_wsl_distro`'s root translates to, for
    /// the status-bar tooltip.
    pub fn remote_wsl_linux_root(&self) -> QString {
        match self.session.borrow().root_path() {
            Some(path) => {
                let host = lsp_core::ExecHost::for_path(path);
                if host.is_remote() {
                    QString::from(host.to_remote(path).as_str())
                } else {
                    QString::default()
                }
            }
            None => QString::default(),
        }
    }

    pub fn create_file(
        mut self: Pin<&mut Self>,
        parent_dir: &QString,
        name: &QString,
    ) -> FfiResult {
        let parent = std::path::PathBuf::from(parent_dir.to_string());
        let result = self
            .session
            .borrow_mut()
            .create_file(&parent, &name.to_string());
        self.as_mut()
            .finish_mutation(result.map(|()| None), vec![parent])
    }

    pub fn create_folder(
        mut self: Pin<&mut Self>,
        parent_dir: &QString,
        name: &QString,
    ) -> FfiResult {
        let parent = std::path::PathBuf::from(parent_dir.to_string());
        let result = self
            .session
            .borrow_mut()
            .create_folder(&parent, &name.to_string());
        self.as_mut()
            .finish_mutation(result.map(|()| None), vec![parent])
    }

    pub fn rename_path(mut self: Pin<&mut Self>, path: &QString, new_name: &QString) -> FfiResult {
        let path = std::path::PathBuf::from(path.to_string());
        // `project_model::rename_path` always joins `new_name` onto `path`'s
        // own parent, so the affected directory is the same before and
        // after the rename — never two different ones to refresh.
        let parent = path.parent().map(Path::to_path_buf);
        let result = self
            .session
            .borrow_mut()
            .rename_entry(&path, &new_name.to_string());
        self.as_mut()
            .finish_mutation(result, parent.into_iter().collect())
    }

    pub fn delete_path(mut self: Pin<&mut Self>, path: &QString) -> FfiResult {
        let path = std::path::PathBuf::from(path.to_string());
        let parent = path.parent().map(Path::to_path_buf);
        let result = self.session.borrow_mut().delete_entry(&path);
        self.as_mut()
            .finish_mutation(result, parent.into_iter().collect())
    }

    /// Shared tail for the four tree-mutation slots above: on success,
    /// trigger an immediate incremental refresh of every directory the
    /// mutation touched (through the exact same `list_dir` → diff → ranged
    /// model update path a watcher event uses — see `refresh_dir_async`),
    /// and relay any retitled tab to the tab strip. No model reset: the
    /// whole point of the lazy tree is that a "New File" in one expanded
    /// folder doesn't collapse every other one.
    fn finish_mutation(
        mut self: Pin<&mut Self>,
        result: Result<Option<app_core::RetitledTab>, AppError>,
        affected_dirs: Vec<std::path::PathBuf>,
    ) -> FfiResult {
        if result.is_ok() {
            for dir in affected_dirs {
                self.as_mut().refresh_dir_async(dir);
            }
        }
        match result {
            Ok(retitled) => {
                if let Some(tab) = retitled {
                    self.as_mut()
                        .tab_title_changed(tab.id.raw(), QString::from(tab.title.as_str()));
                }
                FfiResult::default()
            }
            Err(err) => FfiResult {
                code: err.code(),
                message: QString::from(err.to_string().as_str()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Qt asks a model for `Qt::DecorationRole` (1), `Qt::EditRole` (2) and a
    /// dozen more on every paint. A custom role sharing one of those numbers
    /// answers a question Qt asked itself — a path `QString` handed back as
    /// the decoration made the tree reserve icon width it never drew, so
    /// every label sat well right of its own branch indicator.
    #[test]
    fn tree_roles_stay_out_of_the_range_qt_reserves() {
        // `IconKey` is in here rather than answering Qt::DecorationRole
        // directly: the decoration is `IconDecorationProxy`'s job, and the
        // Rust model stays free of every role Qt defines.
        let roles = [
            ("Path", Roles::Path),
            ("IsDir", Roles::IsDir),
            ("IconKey", Roles::IconKey),
        ];
        for (name, role) in roles {
            assert!(
                user_role(role) >= QT_USER_ROLE,
                "role {name} collides with a Qt-defined role"
            );
        }
        let numbers: Vec<i32> = roles.iter().map(|&(_, role)| user_role(role)).collect();
        let mut distinct = numbers.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), numbers.len(), "two roles share a number");
    }
}
