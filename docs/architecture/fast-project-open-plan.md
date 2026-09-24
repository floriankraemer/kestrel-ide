# Instant project open: lazy tree + background everything

## Context

Opening a large folder shows nothing for seconds.
The goal: the tree is visible and files are openable immediately, with all other project-open work running in the background.

### Root causes (traced, file:line as of the start of this work)

1. **Full eager recursive walk before first paint.**
   `DirectoryTree::build`/`walk` (`crates/project-model/src/lib.rs:108-153`) reads every directory, with no gitignore skip — `target/`, `node_modules/`, `.git/` are all walked.
   This repo alone is roughly 424k nodes and 130 MB (`crates/project-model/src/rebuild.rs:13-16`).
   The walk already runs on a worker thread (ADR-0037), but the tree installs only once the walk finishes, so the tree sits blank for seconds regardless.
   A second, smaller defect rides along: one unreadable subdirectory fails the whole open (the `?` at `lib.rs:126,130`).
2. **Watcher registration ran on the Qt thread, in front of first paint.**
   The `open_folder_async` queue closure (`crates/ui-shell/src/bridge/tree.rs:319-331`, before this PR) called `start_watcher()` synchronously, which ran `ProjectWatcher::start` (`crates/project-model/src/watcher.rs:282-295`) inline: a second full `ignore::WalkBuilder` walk plus one blocking `watch()` call per directory.
   WSL roots use `PollWatcher`, whose `watch()` stat-scans each directory over a 9P share — many seconds of a frozen UI, every time.
3. **The `projectOpened` slot chain ran synchronously inside the same closure** (`tree.rs:329`, before this PR).
   Its listeners do roughly seven to nine `settings.toml` parses, and the analysis status label spawns `wsl.exe` probes on WSL roots (`bridge/analysis/mod.rs:90` calling into `process-exec/src/host.rs:316`).
4. **The old tree and the old watcher were dropped on the Qt thread** (`app-core/src/project_open.rs:18`, `project-model/src/lib.rs:409`, before this PR) — hundreds of thousands of frees plus `inotify_rm_watch` calls, all inline with the next project's open.
5. **Every change triggers a full re-walk and a full model reset.**
   Watcher events (`tree.rs:445-495`) and create/rename/delete operations (`app-core/src/lib.rs:738,745,760,778`, `file_ops.rs:155` — the latter synchronous on the Qt thread) all re-walk the entire tree.
   A model reset also collapses the user's expand state.

Index, LSP, VCS, run configurations and JVM sync already run on workers; they are not the bottleneck.

## Strategy

The project tree is loaded lazily, per directory, the same model IntelliJ and VS Code use: only what the user expands is ever read from disk.
The root listing is one `read_dir` call, so the tree paints in milliseconds regardless of project size.
Everything else — watcher registration, the `projectOpened` slot chain, recent-project bookkeeping, old-project teardown — moves off the Qt thread and after first paint.

The work ships as three PRs:

- **PR1** reorders the existing (eager) open sequence so the tree paints before the watcher registers and before the `projectOpened` slot chain runs, and moves watcher registration and old-project teardown off the Qt thread.
  This is a real, measurable win on its own even before the tree becomes lazy, and it ships first because it is small and self-contained.
- **PR2** replaces the eager walk with the lazy tree (`project-model`'s `DirectoryTree`, the Qt model's `fetchMore`/`hasChildren`), switches the watcher to incremental per-directory refresh instead of a full rebuild, and moves the MCP/AI-agent full-tree consumers onto their own gitignore-aware walk since they can no longer read the (now-partial) lazy arena.
  This is the PR that actually fixes first paint on a huge project — PR1 alone still pays the eager walk's cost, just no longer in front of the watcher and the slot chain.
- **PR3** trims the remaining UI-thread work in the `projectOpened` slot chain itself: sharing one resolved-settings read across LSP/build-tools/analysis/search instead of each re-parsing, and resolving the WSL analysis-label program on a worker instead of spawning `wsl.exe` on the Qt thread.

See the ADR below for the two decisions this plan makes: the open-sequence reorder (PR1, accepted and delivered) and the lazy tree (PR2, accepted, not yet delivered).

## Progress

| Task | Status | Commit |
|---|---|---|
| PR1 — Step 4 open reorder + watcher off UI thread | done | this commit |
| PR2 — Steps 1+2+3+6 lazy tree, incremental refresh, full-tree consumers | not started | |
| PR3 — Step 5 trim `projectOpened` UI-thread work | not started | |
