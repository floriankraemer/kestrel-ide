use core::pin::Pin;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use app_core::{AppError, AppSession};
use cxx_qt::Threading;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};

use crate::bridge::convert::{push_recent_project, to_ffi_result};
use crate::bridge::ffi::{self, FfiResult, Roles};
use crate::bridge::registry::{shared_icons, shared_session, SharedIcons};

/// Rust side of the `ProjectTreeModel` QObject: handles on the shared
/// session and icon theme, nothing else — the tree data itself lives in
/// `app-core`.
pub struct ProjectTreeModelRust {
    session: Rc<RefCell<AppSession>>,
    icons: Rc<SharedIcons>,
    /// Whether a watcher event may start a tree rebuild, or belongs behind
    /// the one already walking (`project_model::RebuildCoalescer`). Touched
    /// only from the Qt thread — every `request`/`finished` call sits either
    /// in a slot or in a `qt_thread.queue`d closure — so a `RefCell` is the
    /// right cell here, as elsewhere in this adapter.
    rebuild: RefCell<project_model::RebuildCoalescer>,
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
            rebuild: RefCell::new(project_model::RebuildCoalescer::new()),
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
        let parent_node = tree.node(parent_id);
        // parent_id != root_id, so parent_node.parent is always Some.
        let grandparent_id = parent_node.parent.expect("non-root node has a parent");
        let row = tree
            .children(grandparent_id)
            .iter()
            .position(|&id| id == parent_id)
            .expect("parent_id must be one of its own parent's children") as i32;
        unsafe { self.create_index(row, 0, parent_id) }
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
                // The arena's root node is the model's invisible root, so no
                // row a view ever asks about is the project root itself.
                let key = |expanded| {
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
                        model.session.borrow_mut().install_opened_project(project);
                        // Borrow scoped tightly: `endResetModel` synchronously
                        // re-enters `rowCount`/`data`, which take their own
                        // borrow of the session.
                        unsafe {
                            model.as_mut().begin_reset_model();
                            model.as_mut().end_reset_model();
                        }
                        model.as_mut().start_watcher();
                        model.as_mut().emit_project_opened();
                        push_recent_project(root);
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

    /// (Re)start the filesystem watcher for whatever project is now
    /// current, replacing any previous watcher (single watcher). Each fs
    /// event queues a closure onto this `ProjectTreeModel`'s own Qt thread —
    /// the one cross-thread hop in the whole design — which reads
    /// `project_model::route_change`'s two-boolean answer for the event:
    /// `rebuild_tree` resets the model, `refresh_vcs` emits
    /// `filesChangedExternally(path)` for `main_window.cpp` to relay to
    /// `DocumentManager` via an ordinary (already-on-the-Qt-thread) signal
    /// connection, so US-3's reload/keep prompt for an open tab's content
    /// change keeps working. That relay is why `project-model`'s watcher
    /// only ever needs one `CxxQtThread` handle, not two.
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
    fn start_watcher(mut self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();
        // W6-1 (ADR-0052): the classification lives here, past the
        // domain/support boundary `project-model`/`app-core` stay below —
        // see `ProjectSession::start_watcher`'s doc comment.
        let root = crate::bridge::convert::current_project_root();
        let is_remote = root
            .as_ref()
            .map(|root| lsp_core::ExecHost::for_path(root).is_remote())
            .unwrap_or(false);
        let result =
            self.session
                .borrow_mut()
                .start_watcher(is_remote, move |kind, changed_path| {
                    // `project_model::route_change` (issue #285) is the
                    // whole decision: `bridge.rs` only reads its answer,
                    // never re-derives it — see CLAUDE.md's "translation
                    // only" rule for this file.
                    let Some(root) = root.as_deref() else {
                        // No project open — the watcher shouldn't be running
                        // at all in this state, so there is nothing to route.
                        return;
                    };
                    let routing = project_model::route_change(root, &kind, &changed_path);
                    // C5: the same event, mapped onto LSP's `FileChangeType` for
                    // `LanguageService::watchedFileChanged` — computed here,
                    // once, rather than in every listener.
                    let watched_kind = lsp_core::watched_files::FileChangeKind::from(kind) as i32;
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        if routing.rebuild_tree {
                            // Off the Qt thread (ADR-0037): a `git checkout` of
                            // a branch with many new files re-walks the whole
                            // tree here, and that walk must not block the UI
                            // any more than the initial "Open Folder" walk does.
                            model.as_mut().rebuild_tree_async();
                        }
                        let path = QString::from(changed_path.to_string_lossy().as_ref());
                        model
                            .as_mut()
                            .watched_file_changed(path.clone(), watched_kind);
                        if routing.refresh_vcs {
                            model.as_mut().files_changed_externally(path);
                        }
                    });
                });
        // The project itself is already open; a failed watch only means
        // external changes (a terminal `git pull`/`checkout`/commit, an
        // edit made outside the app) won't be noticed until it's reopened.
        // Surfaced rather than left silent (`.ok()` used to swallow this) —
        // see `AppError::WatcherFailed`.
        if let Err(err) = result {
            let result = to_ffi_result(Err(err));
            self.as_mut().watcher_failed(result);
        }
    }

    /// A structural filesystem-watcher event says the tree may have moved.
    ///
    /// Rebuilds are coalesced rather than spawned one per event: a rebuild
    /// re-walks the entire project, and the events arrive in bursts of
    /// thousands (a `git checkout`, a `cargo build`, the watcher's own
    /// registration sweep) that all ask the same question. Spawning a thread
    /// each — which this used to do — put hundreds of concurrent full walks
    /// on the machine, each holding its own copy of the tree: measured at 205
    /// threads and 30 GB of resident memory within fifty seconds of opening
    /// this repository, which the OOM killer then ended. See
    /// `project_model::RebuildCoalescer`.
    fn rebuild_tree_async(self: Pin<&mut Self>) {
        let start = self.rebuild.borrow_mut().request();
        if start {
            self.spawn_tree_rebuild();
        }
    }

    /// Re-walk the current project's tree off the Qt thread and reset the
    /// model once it lands — the rebuild half of `open_folder_async`'s
    /// worker-thread shape (ADR-0037). A no-op if no project is open by the
    /// time this runs (the watcher was about to be replaced or stopped
    /// anyway).
    ///
    /// Only ever called with the coalescer already holding the "running"
    /// slot, and every path out of here reports back to it — including the
    /// no-project and walk-failed paths, since a slot never given back would
    /// silently drop every later rebuild for the life of the process.
    fn spawn_tree_rebuild(mut self: Pin<&mut Self>) {
        let root = self.session.borrow().root_path().map(Path::to_path_buf);
        let Some(root) = root else {
            self.as_mut().finish_tree_rebuild();
            return;
        };
        let order = self.session.borrow().tree_sort_order();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let rebuilt = project_model::rebuild_tree_sorted(&root, order);
            // A failed `queue` means the Qt thread is gone (the app is
            // shutting down), so there is no later rebuild left to drop.
            let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                if let Ok(tree) = rebuilt {
                    // `false` means the open project changed while this
                    // rebuild was in flight (a fresh `openFolder` landed
                    // first) — the stale result is dropped rather than
                    // stomping the newer tree.
                    let applied = model.session.borrow_mut().install_rebuilt_tree(&root, tree);
                    if applied {
                        unsafe {
                            model.as_mut().begin_reset_model();
                            model.as_mut().end_reset_model();
                        }
                    }
                }
                model.as_mut().finish_tree_rebuild();
            });
        });
    }

    /// Hand the coalescer's "running" slot back, and start the one catch-up
    /// rebuild it asks for when events arrived while this walk was running.
    fn finish_tree_rebuild(self: Pin<&mut Self>) {
        // The borrow ends before the call: the catch-up re-enters
        // `spawn_tree_rebuild`, which reaches this same `RefCell` again.
        let again = self.rebuild.borrow_mut().finished();
        if again {
            self.spawn_tree_rebuild();
        }
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
        self.as_mut().finish_mutation(result.map(|()| None))
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
        self.as_mut().finish_mutation(result.map(|()| None))
    }

    pub fn rename_path(mut self: Pin<&mut Self>, path: &QString, new_name: &QString) -> FfiResult {
        let path = std::path::PathBuf::from(path.to_string());
        let result = self
            .session
            .borrow_mut()
            .rename_entry(&path, &new_name.to_string());
        self.as_mut().finish_mutation(result)
    }

    pub fn delete_path(mut self: Pin<&mut Self>, path: &QString) -> FfiResult {
        let path = std::path::PathBuf::from(path.to_string());
        let result = self.session.borrow_mut().delete_entry(&path);
        self.as_mut().finish_mutation(result)
    }

    /// Shared tail for the four tree-mutation slots above: reset the model
    /// so the view re-reads the rebuilt tree, and relay any retitled tab to
    /// the tab strip. The model is also reset when only the tree re-snapshot
    /// failed (`TreeRebuild`) — the disk mutation itself succeeded, so the
    /// stale rows must still be dropped (same behavior as before the
    /// refactoring). Full reset, no incremental diffing — consistent with
    /// the reset-based approach at MVP scope.
    fn finish_mutation(
        mut self: Pin<&mut Self>,
        result: Result<Option<app_core::RetitledTab>, AppError>,
    ) -> FfiResult {
        let mutated_disk = matches!(&result, Ok(_) | Err(AppError::TreeRebuild(_)));
        if mutated_disk {
            unsafe {
                self.as_mut().begin_reset_model();
                self.as_mut().end_reset_model();
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
