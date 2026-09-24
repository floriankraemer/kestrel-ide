# 0037. Opening a project walks the directory tree off the Qt thread

## Status

Accepted.
§3 ("Startup sequencing waits for the outcome instead of blocking on it") is superseded by [ADR-0062](0062-lazy-project-tree-and-off-ui-thread-project-open.md): the open sequence itself was still blocking the tree's first paint on watcher registration and the `projectOpened` slot chain even after this ADR moved the walk off-thread; ADR-0062 reorders it so the tree paints first. The rest of this ADR (the worker-thread shape, the stale-root guard pattern, the watcher's own off-thread rebuild) is unchanged and still in force.

## Context

`ProjectTreeModel::openFolder` was a blocking `#[qinvokable]` returning `FfiResult`.
On the Qt main thread it ran the full recursive directory walk (`project_model::DirectoryTree::walk`, plain `fs::read_dir` recursion over every entry), `apply_sort_order`, two config writes (`persist_last_project`, `push_recent_project`), and `beginResetModel`/`endResetModel`.
`reopenLastProject` (used at every startup) called the same blocking path.
The filesystem watcher's structural-change handler re-ran the identical walk synchronously inside a `qt_thread.queue`d closure — itself already a hop onto the Qt thread — so a `git checkout` of a branch with many new files froze the UI exactly as badly as the initial open did.
None of this showed any busy indication: the window simply stopped repainting for however long the walk took, worse the larger the project or the slower the disk.

This repo already has an established shape for exactly this problem: `VcsService`/`LanguageService`/`RunService` each own a worker thread (or a job queue backed by one), do blocking work there, and marshal results back to the Qt object via `cxx_qt::Threading::qt_thread().queue(...)`.
`openFolder` predates that pattern and was never migrated to it.

## Decision

### 1. `openFolder`/`reopenLastProject` become fire-and-forget, backed by a fresh worker thread per open

`ProjectTreeModel::open_folder` no longer returns `FfiResult`; it spawns a plain `std::thread` running `project_model::open_folder_sorted` — a new pure function (no `AppSession`, no Qt, safe off the Qt thread) that walks and sorts in one call — and queues the result back via `qt_thread.queue`.
A queued success closure installs the walked `Project` into the shared `AppSession` (`AppSession::install_opened_project`, a thin new swap-in method — the walk already happened, only the replace is left), resets the model, restarts the watcher, emits `projectOpened`, and calls `push_recent_project`.
A queued failure closure emits a new signal, `projectOpenFailed(FfiResult)`, carrying the same typed-code-plus-message `FfiResult` every other fallible slot already uses (ADR-0003) — a signal rather than a return value because the walk no longer has a synchronous return to carry it on.

A **fresh thread per open**, not a persistent job queue like `VcsService`'s, because there is never more than one project-open in flight that matters: a second `openFolder` before the first lands simply replaces whatever the first would have installed once both land — no ordering guarantee is needed the way `VcsService`'s stage/commit sequence needs one.

`reopenLastProject` reads the persisted last-project path synchronously first (`project_model::read_last_project`, one small text file) and only spawns the worker if a path was found, reusing the same worker function as `openFolder`.
It keeps its `bool` return, but its meaning changes from "was a project opened" to "was a reopen kicked off" — `false` only when nothing was ever persisted, so the caller knows no `projectOpened`/`projectOpenFailed` is coming.

### 2. The watcher's structural rebuild gets the same treatment

`start_watcher`'s queued closure no longer calls `ProjectSession::rebuild_tree()` (a blocking walk) inline.
It now calls `rebuild_tree_async`, which reads the current root and sort order (cheap, in-memory), spawns a worker running the new `project_model::rebuild_tree_sorted` (the same split `open_folder_sorted` uses: a pure walk-and-sort function), and queues the result back.
The queued closure installs the tree via `AppSession::install_rebuilt_tree(root, tree)`, which only applies when `root` still matches the currently open project — a stale rebuild for a project that changed mid-walk is dropped rather than clobbering the newer tree, the same staleness guard `VcsService`'s generation counters exist for elsewhere in this codebase.

### 3. Startup sequencing waits for the outcome instead of blocking on it

Before this change, `buildMainWindow()` ran `reopenLastProject()` synchronously and returned; `run_app()` then showed the window and closed the splash screen immediately after.
Making the reopen fire-and-forget without changing that sequencing would show an empty tree for however long the walk takes — arguably better than a frozen window, but the wrong tradeoff for the one guaranteed project-open on every launch.

`buildMainWindow` instead takes a second callback, `whenReady`, called exactly once: immediately if `reopenLastProject()` returns `false` (nothing to reopen), or — if it returns `true` — once a one-shot connection on `projectOpened`/`projectOpenFailed` fires.
`run_app()`'s window-show/`applyNativeWindowChrome`/`splash.finish` logic moves into that callback.
This relies on `QApplication::exec()` starting shortly after `buildMainWindow()` returns: a `CxxQtThread::queue`d closure is a Qt posted event, and Qt holds a posted event queued regardless of whether the event loop is running yet, delivering it once `exec()` starts — so queuing the outcome before `exec()` begins is sound, not a race.

### 4. Cheap wins bundled into the same change

- `ProjectTreeModelRust::sort_descending()` read `settings.toml` from disk on every call; it now reads the shared session's in-memory sort order (`AppSession::tree_sort_order()`), which `Default` already seeds from disk once and `set_sort_descending` is the only writer of — no new cached field, no invalidation to get wrong.
- `SearchModel::open_index`'s settings-layer read moved inside its own existing `std::thread::spawn`, using a new `load_resolved_settings_for(root)` (explicit root, not `shared_session`'s current project) rather than the existing `load_resolved_settings()` — `shared_session` is a `thread_local`, sound only on the Qt thread, so the existing helper could not simply be called from a worker thread without silently reading a second, empty `AppSession`.
- `LanguageService::open_project`'s equivalent settings read was **not** moved (see Alternatives rejected).

## Consequences

- Opening or switching a project no longer blocks the Qt event loop for the directory walk, on any of its three trigger paths (menu action, Recent Projects, startup reopen) or the watcher's structural rebuild.
- `openFolder`'s FFI contract changes from a blocking `FfiResult`-returning call to fire-and-forget plus two signals — a breaking change for any C++ call site checking the old return value; the one existing call site (`recent_projects_menu.cpp`) is updated in the same change.
- A status-bar message plus `Qt::WaitCursor` (`status_bar.cpp`'s `showProjectOpening`/paired clear on `projectOpened`/`projectOpenFailed`) gives the explicit "Open Folder..."/Recent Projects paths busy feedback they never had before, matching the existing indexing/LSP-busy status-bar pattern.
- `project_model::ProjectSession` gains two new swap-in methods (`install_project`, `install_tree`) alongside its existing walk-and-install methods (`open_folder`, `rebuild_tree`), which now delegate to the same pure `open_folder_sorted`/`rebuild_tree_sorted` functions the worker threads call — one walk implementation, two call shapes (do-it-yourself and hand-me-the-result), rather than parallel logic.
- A second `openFolder`/Recent Projects click before the first settles is not queued or rejected; whichever walk lands last wins, and `recent_projects_menu.cpp`'s per-click outcome listener is scoped (a throwaway `QObject` parented to `treeModel`, deleted the moment its own signal arrives) so a stacked request's dialog/refresh does not linger after the newest one's does.

## Alternatives rejected

**Moving `LanguageService::open_project`'s settings read into its worker thread too, for consistency with `SearchModel`'s.**
`self.configs`, computed from that read, is consulted synchronously by `config_for_path` the moment a tab opens (`document_opened`) — including every tab the editor layout restores immediately after `open_project` returns, now that startup waits for `projectOpened` before restoring the layout (§3).
Moving the read off-thread would leave `configs` empty for however long it takes, silently skipping LSP startup for any tab opened in that window, for no measurable win: the read is two small settings files, not a directory walk.

**A persistent job queue for project-open, matching `VcsService`'s shape exactly.**
`VcsService` needs one because staging/commit/branch operations must run in the order the user issued them against one repository handle.
Project-open has no such ordering requirement — only the latest attempt's outcome matters — so a queue would add a `Sender`/worker-loop lifecycle for no behavior a fresh `std::thread::spawn` per call doesn't already provide.

## Related

- ADR-0003 (FFI seam conventions: typed errors, stable `TabId`, Rust-owned dirty state) — `projectOpenFailed`'s `FfiResult` follows its typed-code-plus-message convention.
- `crates/ui-shell/src/bridge/vcs/mod.rs` — the worker-thread template this change follows the shape of.
