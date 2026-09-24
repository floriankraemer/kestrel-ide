# 0062. Lazy project tree and off-UI-thread project open

## Status

Accepted for the open-sequence reorder (this document's "Decision 1"), delivered in PR1 of [the fast project open plan](../fast-project-open-plan.md).
Accepted, not yet delivered, for the lazy tree (this document's "Decision 2") — it lands in PR2.
Supersedes [ADR-0037](0037-async-project-open.md) §3 ("Startup sequencing waits for the outcome instead of blocking on it").
Amends [ADR-0051](0051-ignore-aware-project-watcher.md) (watcher registration moves off the Qt thread).

## Context

Opening a large project showed a blank tree for several seconds, even though ADR-0037 already moved the directory walk itself off the Qt thread.
Two separate problems produce that gap, and this ADR addresses them as two decisions because they ship in separate PRs and have separate scopes.

**Decision 1's problem**: even with the walk off-thread, everything *after* the walk landed still ran synchronously on the Qt thread, in front of the tree's first paint — watcher registration (`ProjectWatcher::start`'s own `ignore::WalkBuilder` walk plus one blocking `notify::Watcher::watch()` call per directory, worse on a WSL root where `watch()` stat-scans over a 9P share) and the entire `projectOpened` slot chain (roughly seven to nine `settings.toml` parses, a WSL analysis-label probe).
The old project's tree and watcher were also torn down inline, on the Qt thread, ahead of installing the new one.

**Decision 2's problem**: the walk itself is eager and recursive (`DirectoryTree::build`/`walk`), reading every directory under the root with no gitignore skip before the tree can show anything at all.
On this repository that is roughly 424k nodes; moving that walk off-thread (ADR-0037) hides it from the Qt event loop, but the tree still shows nothing until the entire walk finishes.

See [the fast project open plan](../fast-project-open-plan.md) for the full root-cause trace and the delivery order.

## Decision

### 1. Open-sequence reorder: paint first, then watcher, then the `projectOpened` chain (PR1)

`ProjectTreeModel::open_folder_async`'s queued closure (`crates/ui-shell/src/bridge/tree.rs`) splits into two `qt_thread.queue` hops instead of one:

1. The first hop installs the walked project (`AppSession::install_opened_project`, now returning the *previous* project instead of discarding it) and resets the model — nothing else.
   The previous project is dropped on a throwaway `std::thread` rather than inline, since freeing a few hundred thousand tree nodes is not free.
2. The first hop then queues a second closure via another `qt_thread.queue` call, rather than calling straight through.
   A directly chained call would run before Qt's event loop gets a chance to process the paint the reset model just scheduled; queuing again lets that paint actually happen first.
   The second hop starts watcher registration (below), emits `projectOpened`, and calls `push_recent_project`.

Watcher registration itself (`ProjectWatcher::start`) moves to a plain `std::thread::spawn`, matching the shape ADR-0037 already established for the directory walk.
The finished watcher is handed back via `qt_thread.queue` and installed through a new Qt-free method, `ProjectSession::install_watcher(root, watcher) -> Result<Option<ProjectWatcher>, ProjectWatcher>`: `Ok(previous)` when `root` still names the currently open project (the normal case — `previous` is whatever watcher it replaces, `None` on a fresh open), `Err(watcher)` when a different project was opened while registration was still running, in which case the caller drops `watcher` instead of installing it.
This is the same stale-result guard `ProjectSession::install_tree` already applies to an off-thread tree rebuild — one rule, tested once, in the Qt-free crate, per CLAUDE.md's "business rules in Qt-free crates" rule.
Either the previous watcher or a stale one is dropped on a throwaway thread, same as the previous project.

A watcher event routed before the new watcher has finished installing is harmless: it can only rebuild a tree that was just loaded fresh by the open this watcher belongs to, so a redundant rebuild costs nothing a real one wouldn't have anyway.

`push_recent_project` (a `settings.toml` read-modify-write) stays on the Qt thread rather than moving to the worker thread alongside `persist_last_project`: the Qt thread's event loop already serializes it against a second rapid open the way it always has, while a fresh worker thread per open would let two such opens race the same load-then-save and silently drop one project from the recent list.
It is not the bottleneck — one small settings file, not a directory walk or a per-directory `watch()` call — so nothing is gained by risking that race for it.

### 2. Lazy tree, `IntelliJ`/VS Code model (PR2)

`DirectoryTree` gains an `Unloaded`/`Loaded` state per node; opening a project reads only the root's direct children (one `read_dir`), and the Qt model's `fetchMore`/`canFetchMore` load a directory's children on first expand, on a worker thread, queued back the same way a rebuild already is.
A filesystem-watcher event refreshes only the one directory it names (a diff against a fresh listing, not a full rebuild and model reset), which also fixes the existing "saving a file collapses the sidebar"-adjacent problem of losing expand state on unrelated changes elsewhere in the tree.
The two full-tree consumers that cannot read a partially loaded lazy arena (`AppSession::project_tree_entries`, used by the MCP `list_project_tree` tool and the AI agent) switch to their own gitignore-aware `ignore::WalkBuilder` walk on the caller's own worker thread instead of reading the arena at all.

Full detail (the arena's tombstone/free-list scheme for stable ids, the incremental-refresh diff shape, the settings/analysis work PR3 trims) lives in the plan doc, not duplicated here — this ADR records the two decisions and their status, not the implementation.

## Consequences

- The tree paints as soon as the walk (PR1) or the root listing (PR2) is ready, never blocked by watcher registration or the `projectOpened` slot chain.
- `ProjectSession::install_project` and `AppSession::install_opened_project` now return the project they replaced (`Option<Project>`) instead of discarding it, so every caller can choose where to drop it; the one existing caller (`tree.rs`) drops it off-thread.
- `ProjectSession` gains `install_watcher`, mirroring `install_tree`'s stale-root guard for the same reason: a result that finished computing off-thread must not silently apply itself to a project that is no longer open.
- Watcher registration failing (e.g. the platform's watch-descriptor limit) is now surfaced from a background thread instead of the Qt thread that started it; the existing `watcherFailed(FfiResult)` signal and status-bar treatment (ADR-0051 §4) are unchanged, just reached from one hop further away.
- Once PR2 lands, opening a project no longer costs an eager walk of the whole tree at all — PR1 alone still pays that cost, just no longer in front of the watcher and the slot chain.

## Alternatives rejected

**Also moving `push_recent_project` to the worker thread that persists the last-opened project.**
Rejected for PR1: it is a shared, non-atomic read-modify-write of `settings.toml`, and the worker thread has none of the Qt event loop's implicit serialization against a second rapid open.
The Qt thread was never the bottleneck for this one small file; there was nothing to gain by risking the race.

**A generation counter instead of a root-path comparison for the watcher's stale-result guard.**
`install_tree` already uses a root-path comparison for the equivalent tree-rebuild case, and the watcher case has the same shape (one long-running off-thread operation, one "is this still current" check on completion) — reusing the same rule keeps one pattern instead of two for what is the same decision.

## Related

- [ADR-0037](0037-async-project-open.md) — the original off-thread open/rebuild shape this ADR extends past the walk itself.
- [ADR-0051](0051-ignore-aware-project-watcher.md) — the watcher's own directory-by-directory registration and failure reporting, unchanged in shape, now started off the Qt thread.
- [ADR-0003](0003-ffi-conventions.md) — `FfiResult`'s typed-code-plus-message convention, used unchanged by `watcherFailed`.
- [The fast project open plan](../fast-project-open-plan.md) — root-cause trace, delivery order, and the living Progress table.
