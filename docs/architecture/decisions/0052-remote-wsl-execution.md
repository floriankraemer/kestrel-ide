# 0052. Remote WSL: execute in the distro, keep file I/O on the share

## Status

Accepted

## Context

On Windows, a project opened from `\\wsl$\<distro>\...` or `\\wsl.localhost\<distro>\...` ran every tool Windows-side, over the 9P share.
`git` hit the dubious-ownership wall the repo already carried a workaround for.
`rust-analyzer` is installed in the distro, not on Windows.
`./gradlew` is an ELF file Windows cannot execute — and `analysis_core::detect::find_program`'s `is_file()` check happily said it exists, which was the trap.
A Cargo build over 9P is an order of magnitude slower than the same build inside the distro.

The full design rationale, the forks considered, and the accepted ceilings are recorded in `docs/architecture/remote-wsl-plan.md`, which this ADR summarizes and makes authoritative.

## Decision

### Scope: process execution and path translation only

VS Code's Remote-WSL model, execution half only: run the tooling inside the distro via `wsl.exe`, translate paths at every seam.
File I/O — reading, writing, watching, indexing — stays Windows-side over the UNC share.
No file-transport protocol, no server component installed in the distro, no second process hosting the IDE core.

### `ExecHost` is a value in `process-exec`, not a new crate or a trait

New module `crates/process-exec/src/host.rs`:

```rust
pub enum ExecHost { Local, Wsl(WslHost) }
pub struct WslHost { pub distro: String, pub unc_prefix: String }
```

`process-exec`, not a new crate: it is already the declared mechanism crate for "start a program" (ADR-0047 §4, a leaf on purpose), and `ExecHost` is nothing but a decoration on the spawn it already performs.
A second leaf crate answering "how do I run a program" is what `layering.md` exists to prevent.

An enum, not a trait: there is one remote kind today, and a `match` on this enum makes the next kind (SSH, say) a compile error at every site that needs to think about it — the property a trait with one implementation would trade away.

### Classification is a pure function, stored nowhere

`ExecHost::for_path` is pure, total and deterministic on the path string, so there is no state to keep in sync and no service can hold a stale answer.
That disposes of the Qt-versus-`gix` spelling divergence: `QFileDialog::getExistingDirectory` and `gix::discover`'s `workdir()` can disagree on separator style or the `wsl$`/`wsl.localhost` spelling, and both still classify to the same distro.

The rule, stated once: **translation in is tolerant; translation out is canonical to the root's own spelling.**
`to_remote` accepts any of the four UNC spellings, mixed separators, any case.
`to_local` emits `unc_prefix` verbatim plus the Linux tail, reproducing the spelling (including separator style) the root arrived with — load-bearing because Windows paths are compared by string equality throughout this codebase (recents dedupe, `strip_prefix`, diagnostics-store keys, the tab list).

### `run`/`spawn` classify their own `work_dir` — the load-bearing simplification

`process_exec::run` and `process_exec::spawn` call `ExecHost::for_path(work_dir)` as their first act and route through it automatically.
`work_dir` is the project root or a directory inside it at every call site in `vcs-core`, `analysis-core` and `test-core`, so those three crates get WSL execution with **zero lines changed**.
Threading an `ExecHost` down from `ui-shell` instead would have been three signature changes, three new service fields, and a new way for two services to disagree about one project, for the configuration this design exists to eliminate ("Windows tool against a WSL directory").

A PTY spawn (`pty-core`, used by `run-core::Supervisor` and the terminal) does **not** go through `process_exec::run`/`spawn`, so it gets none of this automatically — `Supervisor::launch` and the terminal's shell resolution translate by hand (W4-1, W5-1).

### argv: `-e` with a resolved absolute program, never `--`

```
wsl.exe -d <distro> --cd <linux-cwd> -e <resolved-program> <args...>
```

- `-d <distro>` always, never the default distro.
- `--cd <linux-cwd>` decides the remote cwd; `current_dir(windows_cwd)` is still set too, so both agree, but `--cd` is authoritative.
- `-e`, not `--`: `--` silently mangles any argument containing a space, `$`, `*` or a quote — a commit message, a `git log --grep` pattern, `--message-format=json`. One cached probe (`host::resolve_program`, one login-shell `command -v` per `(distro, program)`, memoised) is cheaper than "argv is sometimes reinterpreted."
- `WSLENV`: `wsl.exe` passes a Windows variable through only if its name is listed there with a translation flag; `ExecHost::command` merges `<key>/u` onto the inherited value for every `env` pair, never dropping what was already there.
- Exit codes: `wsl.exe` returns the Linux command's own code, so a missing Linux binary does *not* surface as `io::ErrorKind::NotFound` (`wsl.exe` itself spawned fine). `host::is_missing_program` remaps exit 127 and `wsl.exe`'s own "no such distro" stderr onto `Failure::NotFound`, so `VcsError::GitNotInstalled` and friends keep working without either crate learning what WSL is.
- `CREATE_NO_WINDOW` applies to whichever process is spawned directly — `wsl.exe` itself for a remote host — and `lsp-core`/`dap-core` gain it as a side effect of going through `ExecHost::command`, which they never had before.

### Translation confined to `lsp-core`, `dap-core`, `build-core`, `run-core`

**Everything above the protocol/process client speaks Windows paths.**
`diagnostics-core`'s `uri_from_path`/`path_from_uri` stay exactly as they are (ADR-0046) — a leaf, host-unaware by design.
`lsp-core` adds `uri_for`/`path_for` wrappers around them and a `LspManager::normalize_uri` that recovers the Windows path a caller's pre-existing `uri_from_path(path)` call encoded and retranslates it through the manager's own `ExecHost` — callers needed **no signature change** for the common ingest path (`did_open`/`did_change`/every feature request). Egress (`definition`, `rename`, `LspManager::parse_workspace_changes`) retranslates a server's response paths the same way. `dap-core` mirrors this with `source_path`/`local_path` against `DapSession::host()`. `build-core::diagnostics::resolve_path` applies the same rule to a compiler's own file references, and `run_core::links::resolve_link` to a `file:line` pasted into a WSL run's output.

Deliberate deviation from a literal reading of the plan doc's "`uri_for`/`path_for` at every URI site" (which named specific line counts in `navigation.rs`/`workspace_edit.rs`): translation happens once, at each crate's own public boundary (`LspManager`'s methods, `DapSession`'s helpers), rather than threading an `ExecHost` parameter through every low-level protocol parser. The parsers stay pure and host-unaware; the boundary is still exactly one layer, and `ui-shell` needed only two call-site changes (`manager.parse_workspace_changes(&edit)` instead of the free function) rather than dozens. One narrow ceiling accepted and documented at its call site: `refactor.rs`'s `current_path_of`, a preview-confinement check rather than the actual apply path, still reads an untranslated path on a WSL root.

`/mnt/c/...` is **not** reverse-mapped to `C:/...` in v1 — it translates to `//wsl.localhost/<distro>/mnt/c/...`, a valid UNC path that reads correctly through the share.

### Automatic activation, one global off switch, no per-project setting

A user who opened `\\wsl.localhost\Ubuntu\...` has already said which machine the project belongs to; per-project opt-in has a bootstrap problem besides (`.ide/settings.toml` lives on the share and would have to be read over 9P before the host is even known).

`app_config::Settings::remote_wsl: Option<bool>` (default on, via `remote_wsl_or_default`, the same "never chosen" shape `mcp_enabled` already established) backs a **process-wide** `AtomicBool` in `process_exec::host` — `process-exec` cannot depend on `app-config` (a leaf crate, ADR-0047), so the switch cannot be read from the setting directly at the point of use. `ExecHost::for_path` checks the flag first and returns `Local` unconditionally when it is off. `ui-shell` reads the persisted setting and calls `set_remote_wsl_enabled` at the one place every project open passes through (`ProjectTreeModel::open_folder_async`), which covers `openFolder`, `reopenLastProject`, and by extension every seam that classifies a path afterward. No settings-page checkbox was added in this round — editing `remote_wsl` in `settings.toml` and reopening the project is sufficient to prove the switch end to end; a UI row is a purely additive follow-up.

### Domain crates stay below the support layer

`process-exec` is a support-layer crate (`docs/architecture/layering.md`'s stated layer order: domain, application, support, adapter+view, app).
Two seams needed `ExecHost`'s classification from inside a domain or application crate — `project-model`'s `ProjectWatcher` (W6-1) and, transitively, `app-core::AppSession::start_watcher` — and neither was allowed to gain `process-exec` as a dependency for it. Both take a plain `is_remote: bool` parameter instead; `ui-shell` (already past that boundary) classifies the root once and hands the bool down. `run-core`, `build-core` and `dap-core` *do* gain `process-exec` directly — all three are support-layer crates themselves, so the dependency direction is legal; this is a deviation from the plan doc's "Layering deltas" paragraph, which named only `pty-core`/`lsp-core`/`dap-core` — the argv/diagnostic-path translation those three crates need (W4-1, W4-3, W4-4) reaches call sites `pty-core`'s own dependency graph does not.

### Server/tool discovery: `host::resolve_program`

`ExecHost::Local` never probes. A remote host resolves a candidate containing a path separator with one `test -x` against the distro; a bare name with one login-shell `command -v` (the login shell is what sources the profile putting `~/.cargo/bin` on `PATH`). Both are memoised per `(distro, program)` for the process's lifetime. `lsp-core`'s `connect`, `dap-core`'s `DapSession::start`, `analysis-core`'s `find_program`, and `run-core`'s `Supervisor::launch` all resolve through this before spawning, so a missing server/adapter/analyzer/tool reports plainly rather than as a generic spawn failure or (worse, `analysis-core`'s pre-existing trap) a false "detected."

## Accepted ceilings

- **Watcher**: `ReadDirectoryChangesW`/`inotify`/`FSEvents` do not fire over the 9P share. `ProjectWatcher::start` switches to `notify::PollWatcher` (4 s, `compare_contents: false`) on a remote root, over the same watch set computed today (ADR-0051).
- **Index**: `index_dir_for` already handles the lock-incompatible-filesystem case (a WSL share is exactly that case). The remaining cost — a slow first index reading every file once over the share — is stated, not fixed; fixing it would mean file I/O moving off the share, which this design deliberately does not do.
- **Process-tree kill**: `PtySession::kill_tree` reaches `wsl.exe`, not the process tree inside the distro's own PID namespace. A daemonised Gradle build survives a Stop. Upgrade path: `wsl.exe -e kill -TERM -<pgid>` against the distro's own process group; not written until a real orphan is observed.
- **Line endings**: a WSL project's files are LF. `editor_core::save_rules`'s `LineEnding::platform()` fallback (`cfg!(windows)`) only ever fires for a brand-new file with no existing line ending to preserve — every other file already keeps its own regardless of host. `ui-shell`'s `save_rules` overrides that one fallback to `Lf` when the project root is remote.
- **`$USER_HOME$` macro**: stays the Windows-side `$HOME`/`$USERPROFILE` on a remote host — resolving the distro's own home needs a login-shell probe, and no run configuration in this codebase reads it as a path handed to the spawned process.
- **W8-2, the manual Windows verification matrix**: not walked in this change — it requires a real Windows machine with a real WSL install, which the implementing session did not have. Left open pending a manual session; see `docs/architecture/remote-wsl-plan.md`'s Progress table.

## Alternatives considered

See `docs/architecture/remote-wsl-plan.md`'s "Forks, and the recommendation" section for the full reasoning behind each of the following:

| Fork | Decision | Why |
|---|---|---|
| Host derived from `work_dir` vs. threaded from `ui-shell` | Derived | Three crates change not at all; nothing to keep in sync. |
| `-e` vs `--` | `-e` | `--` silently mangles arguments containing shell metacharacters; the bug appears months later on one user's input. |
| Translate at helper sites vs. walk the JSON at the framing boundary | Helper sites | A JSON walk has to guess which strings are paths; a bounded, greppable list beats a heuristic. |
| Automatic vs. opt-in per project | Automatic, with a global off switch | The user already said which machine the project belongs to by opening a WSL path; per-project opt-in has a 9P bootstrap problem. |
| Terminal default shell | The project's distro outranks the implicit platform default, not an explicit `shell_path` | A deliberate per-machine override (`shell_path`) still wins; a global default the user did not set with this project in mind does not. |

## Consequences

- Positive: `git`, `rust-analyzer`, `cargo build`/`test`, and debugging all run inside the distro for a WSL project, at native speed, without the dubious-ownership wall or the "ELF file over the share" trap.
- Positive: `vcs-core`, `analysis-core` and `test-core` needed zero lines changed — the load-bearing simplification held for all three.
- Positive: every translation site is unit-testable on Linux CI, including the whole `process_exec::run` → `ExecHost` → `Command` path, via a fake `wsl.exe` script on `PATH` — no Windows or WSL install needed to verify the argv-wrapping contract.
- Negative / accepted: the four ceilings above (watcher polling, slow first index, process-tree-kill, line endings) are real, documented limitations rather than fixed.
- Negative / accepted: `process-exec` gains a process-wide mutable flag (the off switch) — the one piece of state in an otherwise fully stateless design, justified by the leaf-crate dependency constraint.
- Negative / accepted: W8-2's manual Windows matrix is outstanding, tracked in the plan doc's Progress table, not this ADR.

## Related

- ADR-0047 (`process-exec` as the mechanism crate)
- ADR-0046 (diagnostics URI keys stay Windows-side)
- ADR-0051 (the watcher this degrades)
- ADR-0016 (single root — why a per-project host is a single value)
- ADR-0040 and ADR-0041 (build and DAP paths gaining translation)
