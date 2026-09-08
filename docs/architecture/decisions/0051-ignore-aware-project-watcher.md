# 0051. The project watcher skips gitignored directories

## Status

Accepted

## Context

The Changes dock (VCS) went stale under real use: a `git pull`, `checkout`, or commit made from a terminal stopped showing up, with the tree sidebar sometimes also failing to notice new files.

`ProjectWatcher::start` (`crates/project-model/src/watcher.rs`) registered one blanket `notify::RecursiveMode::Recursive` watch on the whole project root, with no exclusions — every directory below it got an OS-level watch, `target/` included.
This repository's own `target/` alone is 10,000+ directories.
Recursively watching that exhausts the platform's watch budget — inotify's `max_user_watches` on Linux, `ReadDirectoryChangesW`'s fixed event buffer on Windows — after which *every* watch on the project, `.git` included, silently stops delivering events.
`ProjectSession::start_watcher` then compounded this by discarding a failed watch with `.ok()`, so the failure was invisible: no log (this codebase has no logging framework), no error dialog, nothing — the dock and the tree just quietly stopped updating.

## Decision

### 1. Watch directory-by-directory, skipping what `.gitignore` already excludes

`ProjectWatcher::start` now walks the project root once with `ignore::WalkBuilder` — the same gitignore-aware traversal `index-core` already uses for its own search index (`crates/index-core/src/lib.rs`) — and registers one `RecursiveMode::NonRecursive` watch per directory the walk yields.
`WalkBuilder` itself skips descending into an ignored directory, so `target/`, `node_modules/`, and anything else a project's own `.gitignore` excludes never gets a watch at all.
Kept `hidden(false)`: the crate's default hidden-file skip would also have excluded `.git` and other dotdirs (`.github/`, `.vscode/`) the watcher must keep seeing — `.git` is never matched by a project's own `.gitignore` (nothing excludes ignoring it there), so no special-casing is needed once hidden directories aren't blanket-skipped.

### 2. A directory created after the watcher starts gets its own watch, added dynamically

The initial walk only covers directories that exist at `start()` time; a `cargo build` that creates `target/` for the first time, or a `git checkout` that creates new source directories, needs the watch set kept current.
On a `Create` event for a directory, a lightweight matcher built once from the project root's own `.gitignore` (`root_gitignore_matcher`) decides whether to add a watch for it.
This matcher is deliberately narrower than `WalkBuilder`'s full semantics (it does not see nested `.gitignore` files elsewhere in the tree, the global git-excludes file, or `.git/info/exclude`) — missing an edge case here means briefly over-watching one new directory until the project is reopened, not the exhaustion this ADR exists to prevent, so the simpler matcher is the right trade for the incremental case.

### 3. The "add a watch" call cannot run inside `notify`'s own event callback

The first implementation called `Watcher::watch` directly from inside the `Create`-event closure, since that closure already had the watcher in scope.
This deadlocks the `inotify` backend permanently after the very first directory it tries to add: the closure runs *on* the backend's own event-processing thread, and `watch` blocks sending a command back to that same thread.
Caught by the test written for point 2 (`a_newly_created_non_ignored_directory_is_watched_dynamically`), which hung until the deadlock was found — a worse regression than the bug this ADR fixes, since it stops the watcher from ever recovering, not just under a large ignored directory.
The fix decouples "notice a new directory" from "register a watch for it" with a plain `mpsc::channel`: the event callback only sends the path; a separate thread owns the `Arc<Mutex<Option<RecommendedWatcher>>>` and calls `watch` from outside the backend's event thread.
That thread ends on its own once `ProjectWatcher` is dropped (dropping the watcher drops the callback closure, which drops the channel's sending half, ending the receiving thread's loop).

### 4. A failed watch is now reported, not swallowed

`ProjectSession::start_watcher` and `AppSession::start_watcher` (`crates/project-model/src/lib.rs`, `crates/app-core/src/lib.rs`) return `Result` instead of discarding the error with `.ok()`.
`ProjectTreeModel::start_watcher` (`crates/ui-shell/src/bridge/tree.rs`) surfaces a failure through a new `watcherFailed(FfiResult)` signal — a new `AppError::WatcherFailed` variant, its own stable FFI code (`CODE_WATCHER_FAILED = 11`, append-only per ADR-0003) — which `status_bar.cpp` shows as a transient status-bar message rather than a blocking dialog: the project itself is already open, only live external-change detection is degraded.

## Alternatives considered

| Option | Why rejected |
|---|---|
| Filter events after the fact (let `notify` watch everything, drop events under `target/` in the callback) | Doesn't reduce OS-level watch count at all — the resource that actually runs out. The OS still creates a watch per subdirectory whether or not the callback later discards its events. |
| One blanket recursive watch, plus a periodic re-check that it's still alive | Papers over the exhaustion instead of preventing it; still watches everything a project ever produces under `target/`, and a "still alive" poll can't tell "silently dropping events" apart from "healthy" on every backend. |
| Reuse `index_core::excludes::exclude_overrides` directly | It's `pub(crate)` to `index-core`, and `index-core` isn't a dependency `project-model` should take on for one helper — same `ignore` crate, used the same way, is the right amount of sharing. |

## Consequences

- Positive: this repository's own `target/` (10k+ directories) no longer costs a single watch; only directories `.gitignore` doesn't exclude do.
- Positive: a watch failure is visible (status-bar message) instead of leaving the Changes dock silently stale with no diagnostic trail.
- Positive: `crates/project-model/src/watcher.rs` gained regression tests for the ignored-directory exclusion, `.git` staying watched, and the dynamic-add path — the last of which is what caught the reentrant-deadlock bug during this change.
- Negative / accepted: a directory created after `start()` is checked against a root-`.gitignore`-only matcher, not the fully correct nested-`.gitignore`/global-excludes semantics the initial walk gets — documented as a deliberate, narrow gap (point 2 above).
- Negative / accepted: `project-model` gains a dependency on `ignore` (already a workspace dependency via `index-core`, so no new crate enters the dependency graph).
