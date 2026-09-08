# Remote WSL support for the Windows build

## Context

On Windows, a project opened from `\\wsl$\<distro>\...` or `\\wsl.localhost\<distro>\...` runs every tool **Windows-side, over the 9P share**.
That is wrong in every direction it can be wrong.
`git` hits the dubious-ownership wall the repo already carries a workaround for (`crates/vcs-core/src/cli.rs:101`, `crates/vcs-core/src/error.rs`, the "mark safe" offer at `crates/ui-shell/src/bridge/ffi.rs:5926`).
`rust-analyzer` is installed in the distro, not on Windows.
`./gradlew` is an ELF file Windows cannot execute — and `analysis_core::detect::find_program`'s `is_file()` check happily says it exists, which is the trap.
A Cargo build over 9P is an order of magnitude slower than the same build inside the distro.

The decided answer is VS Code's Remote-WSL model, execution half only: **run the tooling inside the distro via `wsl.exe`, translate paths at every seam.**
File I/O — reading, writing, watching, indexing — stays Windows-side over the UNC share.
No file-transport protocol, no server component installed in the distro, no second process hosting the IDE core.

What already exists to build on:

- `crates/process-exec/src/lib.rs` — the mechanism crate (ADR-0047 §4): `run` (lib.rs:75), `spawn` (lib.rs:232), drain threads, wall-clock timeout, kill-on-timeout, `suppress_console_window` (lib.rs:27, the repo's only `creation_flags` call).
- `crates/pty-core/src/shells.rs:157-210` — the repo's only `wsl.exe` integration: `wsl.exe --list --quiet`, `decode_utf16le` (shells.rs:229), `wsl:<distro>` shell candidates with correct argv.
- `crates/index-core/src/lib.rs:158-220` — `index_dir_for`/`supports_file_locks` already fall back to `<cache_dir>/ide/index/<sanitised>` when the project directory cannot take a file lock, which is exactly what a 9P share cannot.
- `crates/diagnostics-core/src/lib.rs:238-282` — `path_from_uri`/`uri_from_path`, the two functions every LSP URI passes through.

What does not exist: any notion of *where* a process runs, any Windows↔Linux path translation, any watcher degradation for a share without `ReadDirectoryChangesW`.

## Design decisions

### The execution host lives in `process-exec`, and is a value, not a trait

New file `crates/process-exec/src/host.rs`:

```rust
pub enum ExecHost { Local, Wsl(WslHost) }

pub struct WslHost {
    pub distro: String,      // "Ubuntu", as it appeared in the path
    pub unc_prefix: String,  // exactly as it arrived: "//wsl.localhost/Ubuntu"
}

impl ExecHost {
    pub fn for_path(path: &Path) -> Self;
    pub fn is_remote(&self) -> bool;
    pub fn to_remote(&self, path: &Path) -> String;   // UNC -> "/home/f/proj"
    pub fn to_local(&self, remote: &str) -> PathBuf;  // "/home/f/proj" -> UNC
    pub fn argv(&self, program: &str, args: &[&str], cwd: &Path) -> (String, Vec<String>);
    pub fn command(&self, program: &str, args: &[&str], cwd: &Path, env: &[(&str, &str)])
        -> std::process::Command;
}
```

`process-exec` and not a new crate: it is already the declared mechanism crate for "start a program" (ADR-0047 §4, `layering.md` row 33 — *std only, a leaf on purpose*), and `ExecHost` is nothing but a decoration on the spawn it already performs.
A second leaf crate answering "how do I run a program" is what `layering.md` exists to prevent.
Path translation rides in the same crate because no consumer of the translation is not also a consumer of the spawn, and it is pure string work with no dependency cost.

An enum and not a trait: there is one alternative implementation and one for the foreseeable future, and a trait with one real implementation is banned by this repo's own standards.
When SSH remotes arrive the enum gains a variant and every `match` becomes a compile error naming the sites that need thinking about — the property a trait would take away.

Layering deltas: `pty-core`, `lsp-core`, `dap-core` gain `process-exec` in their rows.
All three are legal today (`process-exec` is std-only) and all three already hand-roll `Command::new`.

### `run` and `spawn` keep their signatures and classify their own `work_dir`

The load-bearing simplification.
`process_exec::run` gains, as its first act:

```rust
let host = ExecHost::for_path(work_dir);
let (program, args) = host.argv(program, args, work_dir);
```

`work_dir` is the project root or a directory inside it at every call site — `crates/vcs-core/src/cli.rs:61`, `crates/analysis-core/src/scheduler.rs:50`, `crates/test-core/src/runner.rs:76`.
So `vcs-core`, `analysis-core` and `test-core` get WSL execution with **zero lines changed in those crates**.

Threading an `ExecHost` down from `ui-shell` instead would be three signature changes, three new service fields, and a new way for two services to disagree about one project.
It buys only "Windows tool against a WSL directory", the configuration this plan exists to eliminate.
`run_on(&ExecHost, ...)` is reserved as the escape hatch and is not written until something needs it.

### Classification is a pure function, stored nowhere

`ExecHost::for_path` is pure, total and deterministic on the path string, so there is no state to keep in sync and no service can hold a stale answer.

That disposes of the Qt-versus-gix spelling divergence.
`QFileDialog::getExistingDirectory` (`crates/ui-shell/cpp/main_window.cpp:823`) yields `//wsl.localhost/Ubuntu/home/f/proj`; `gix::discover`'s `workdir()` (`crates/vcs-core/src/repo.rs:56-84`) may yield a backslashed or `\\wsl$\` spelling.
Both classify to the same distro and both `to_remote()` to `/home/f/proj`, which is the only value `wsl.exe` ever sees.

The rule, stated once:

> **Translation in is tolerant; translation out is canonical to the root's own spelling.**

`to_remote` accepts `\\wsl$\`, `\\wsl.localhost\`, `//wsl$/`, `//wsl.localhost/`, any mix of separators, any case.
`to_local` emits `unc_prefix` verbatim plus the Linux tail with `/` separators, reproducing the spelling the root arrived with.
That matters because Windows paths are compared by string equality throughout: recents dedupe by exact `PathBuf` equality (`crates/app-config/src/lib.rs:546`), `crates/run-core/src/context.rs:33` does `file.strip_prefix(project_root)`, the diagnostics store keys by URI (ADR-0046), the tab list keys by path.
`to_remote` → `to_local` must be the identity on any path under the root; that is the first test W1-2 writes.

### argv: `-e` with a resolved absolute program

```
wsl.exe -d <distro> --cd <linux-cwd> -e <resolved-program> <args...>
```

- `-d <distro>` always, never the default distro — a project names its distro in its own path, and depending on `wsl --set-default` would make behaviour depend on machine state the user did not set for this project.
- `--cd <linux-cwd>` rather than trusting `wsl.exe` to map the inherited Windows cwd. `process_exec::run` still sets `current_dir(windows_cwd)` as it always did, so both agree, but `--cd` is what decides.
- `-e` (exec, no shell) rather than `--`. See the forks section.
- Environment: `wsl.exe` passes a Windows variable in only if its name is in `WSLENV`. `ExecHost::command` sets each `(key, value)` on the Windows-side `Command` *and* appends `key/u` to a `WSLENV` merged onto the inherited one. `/p` and `/l` path-translation flags are deliberately unused — we do our own translation and do not want two systems doing it. This is what makes `vcs-core`'s `GIT_TERMINAL_PROMPT=0` survive the trip, and it gets its own unit test.
- Exit codes: `wsl.exe` returns the Linux command's code, so `Output::status` keeps its meaning. What breaks is `Failure::NotFound` — a missing Linux binary no longer surfaces as `io::ErrorKind::NotFound`, because `wsl.exe` itself spawned fine. W1-6 maps exit code 127 and `wsl.exe`'s own "no distribution with the supplied name" / "no installed distributions" stderr onto `Failure::NotFound` inside `process-exec`, so `VcsError::GitNotInstalled` and `AnalyzerStatus::NotDetected` keep working without either crate learning what WSL is.
- Stdio: `wsl.exe` relays the child's bytes unchanged, so stdout/stderr stay UTF-8 and every existing parser (TeamCity messages, checkstyle XML, `--message-format=json`) is unaffected. Only `wsl.exe`'s own *management* output is UTF-16LE, which is why `decode_utf16le` exists and why W1-4 moves it next to the only other code that will need it.
- `CREATE_NO_WINDOW` applies to the `wsl.exe` process, the only Windows-side process that could flash a console. Now inherited by `lsp-core` and `dap-core`, which never had it.
- Timeout and drain unchanged: the drain threads read `wsl.exe`'s pipes, `Child::kill()` kills `wsl.exe`. What that does not reach is stated under Accepted ceilings.

### Tool discovery inside the distro

`host::resolve_program(host, program, cwd) -> Option<String>`, memoised in a `Mutex<HashMap<(String, String), Option<String>>>` keyed by `(distro, program)`:

- `ExecHost::Local` — today's behaviour, no probe.
- A candidate containing a separator (`vendor/bin/phpstan`, `./gradlew`) — translate `cwd.join(candidate)` and confirm with one `test -x`. The probe must be about executability *in the distro*, not existence on the share.
- A bare name (`rust-analyzer`, `phpstan`, `git`) — one `wsl.exe -d D -e /bin/sh -lc 'command -v <name>'`; the `-lc` login shell is what sources the profile that puts `~/.cargo/bin` and `~/.local/bin` on `PATH`. Cache the absolute path and use it as the `-e` program from then on.

One login-shell probe per tool per distro per session is the entire cost, and it is what makes `-e`'s exact-argv guarantee affordable.
`crates/lsp-core/src/catalog.rs` needs no data change — `command` is still "executable, looked up on `PATH`", the `PATH` is just the distro's.

### The URI seam: translate inside `lsp-core`/`dap-core`/`build-core`, nowhere above

> **Everything above the protocol client speaks Windows paths. `lsp-core`, `dap-core` and `build-core` are the only crates that ever see a Linux path.**

`LspManager` holds an `ExecHost` and two wrappers:

```rust
fn uri_for(host: &ExecHost, path: &str) -> String         // Windows path -> Linux -> file:// URI
fn path_for(host: &ExecHost, uri: &str) -> Option<String> // file:// URI -> Linux -> Windows path
```

used at the existing call sites (`manager.rs` including `rootUri` at manager.rs:935, `diagnostics.rs`, `navigation.rs` ×4, `workspace_edit.rs` ×3).
`diagnostics-core`'s `uri_from_path`/`path_from_uri` stay exactly as they are — it is a leaf by design (ADR-0046) and must not learn about hosts.

This also fixes a latent bug: `uri_from_path` on `//wsl.localhost/Ubuntu/p` emits `file:////wsl.localhost/Ubuntu/p`, and `path_from_uri` refuses any non-empty authority, so the round trip is already broken for UNC paths today.
After translation the helpers only ever see `/home/f/proj/...`, the shape they were written for — the UNC case stops existing rather than being special-cased.

`ui-shell`'s URI users are untouched: translation happened at ingest, so the diagnostics store keys stay Windows-side.

For DAP the same rule applies in `crates/dap-core/src/session.rs`: `Source.path` in and out, `setBreakpoints` paths out, `stackTrace` frames in.
The existing remote-attach path mappings (`dap_core::launch::remote_attach_arguments`) are a different feature; WSL translation happens underneath them.

For builds, `build_core`'s parsers produce Linux paths because the compiler ran in the distro; `to_local` is applied where a `BuildDiagnostic` is constructed.

`/mnt/c/...` is **not** reverse-mapped to `C:/...` in v1 — it translates to `//wsl.localhost/<distro>/mnt/c/...`, a valid UNC path that reads correctly through 9P.

## Forks, and the recommendation

1. **Host derived from `work_dir` vs. threaded from `ui-shell`** — **derived**. Three crates change not at all, nothing to keep in sync.
2. **`-e` vs `--`** — **`-e`**. `--` silently mangles any argument containing a space, `$`, `*` or a quote: a commit message, a `git log --grep` pattern, `--message-format=json`. That bug class appears months later on one user's input. One cached probe is cheaper than "argv is sometimes reinterpreted".
3. **Translate at the helper sites vs. walk the JSON at the framing boundary** — **helper sites**. A JSON walk has to guess which strings are paths, which is how a `documentation` field containing `/usr/include/...` gets rewritten. A bounded greppable list beats a heuristic.
4. **Automatic vs. opt-in per project** — **automatic, with a global off switch, no per-project setting**. A user who opened `\\wsl.localhost\Ubuntu\...` has already said which machine the project belongs to. Per-project opt-in also has a bootstrap problem: `.ide/settings.toml` lives on the share and would have to be read over 9P before the host is known. The off switch is one `remote_wsl` key in `app-config`'s global settings, default on.
5. **Terminal default shell** — **the project's distro outranks the implicit platform default, not an explicit `shell_path`**. `crates/ui-shell/src/bridge/terminal.rs`'s resolution order gains one step ahead of `settings.shell_id`; the existing `wsl:<distro>` candidate already has correct argv, so this is a handful of lines.

## Accepted ceilings

- **Watcher**: `ReadDirectoryChangesW` does not work over 9P, so `crates/project-model/src/watcher.rs:113`'s per-directory `notify` watches deliver nothing on a WSL root, silently. W6-1 swaps in `notify::PollWatcher` (already in the `notify` dependency) at 4 s with `compare_contents: false`, over the same watch set `ProjectWatcher` computes today (ADR-0051). One branch on `host.is_remote()`. Upgrade path — a `wsl.exe`-hosted inotify relay — is file-transport-shaped and out of scope.
- **Index**: `index_dir_for` already handles the lock failure. The remaining cost is a slow first index reading every file over 9P. Stated, not fixed.
- **Process-tree kill**: `run_core::Supervisor::kill_tree` reaches `wsl.exe`, not the tree inside the distro. A daemonised Gradle survives. Upgrade path is `wsl.exe -e kill -TERM -<pgid>`; not written until a real orphan is observed.
- **Line endings**: `editor_core::save_rules` (save_rules.rs:62) defaults by `cfg!(windows)`; a WSL project's files are LF. One line in W6 if the existing "preserve the file's own endings" rule does not already cover it.

## Progress

Living status table — update the row in the same commit that finishes the task, per `CLAUDE.md`.

### W0 — the plan

| Task | Status | Commit |
|---|---|---|
| W0-1 — plan doc at `docs/architecture/remote-wsl-plan.md` + `docs/README.md` index line | done | c0958c3 |

### W1 — `process_exec::host`

| Task | Status | Commit |
|---|---|---|
| W1-1 — `ExecHost`/`WslHost`, `for_path`, tolerant classification of all four spellings | done | c6e4b0a |
| W1-2 — `to_remote`/`to_local`, spelling-preserving on the way out, round-trip identity | done | c6e4b0a |
| W1-3 — `argv` + `command`: `-d`, `--cd`, `-e`, `WSLENV`, `CREATE_NO_WINDOW` | done | c6e4b0a |
| W1-4 — `host::distros()` moved out of `pty_core::shells` (`decode_utf16le`, `wsl_distros`); `pty-core` calls it | done | 92398bc |
| W1-5 — `host::resolve_program`: login-shell `command -v`, memoised per `(distro, program)` | done | c6e4b0a |
| W1-6 — exit 127 and `wsl.exe`'s own failures become `Failure::NotFound` | done | c6e4b0a |
| W1-7 — `run`/`spawn` classify their own `work_dir`; no signature change, no caller change | done | c6e4b0a |

### W2 — git, analyzers, tests (callers that need no change)

| Task | Status | Commit |
|---|---|---|
| W2-1 — verify `vcs-core` runs `git` in the distro; only `gix::discover`'s cwd spelling may need work | done | 936e2fa |
| W2-2 — `analysis_core::detect::find_program` probes the distro, not the share | done | d541293 |
| W2-3 — `test-core`'s streamed `spawn` over `wsl.exe`, TeamCity output unaffected | done | 36e2d91 |
| W2-4 — dubious-ownership "mark safe" offer becomes unreachable for a WSL root; confirm and note | done | a9abf50 |

### W3 — `lsp-core`

| Task | Status | Commit |
|---|---|---|
| W3-1 — `LspManager` carries an `ExecHost`; `connect` spawns through `ExecHost::command` (gains `current_dir` and `CREATE_NO_WINDOW` it never had) | done | 5346659 |
| W3-2 — `rootUri` is the Linux path; `uri_for`/`path_for` at every URI site | done (translated at `LspManager`'s own methods rather than threaded through every `navigation.rs`/`workspace_edit.rs` parser — see commit message) | 5346659 |
| W3-3 — server discovery via `host::resolve_program`; a server missing in the distro says so | done | 5346659 |
| W3-4 — `watched_files` globs and `didChangeWatchedFiles` URIs translated the same way | done | 5346659 |

### W4 — run, build, debug, toolchains

| Task | Status | Commit |
|---|---|---|
| W4-1 — `run_core::supervisor::launch` translates `LaunchSpec` through `ExecHost::argv` before `ShellSpec` | done | 80b8f60 |
| W4-2 — `toolchain::python_program` and `wrapper_or` become host questions, not `cfg!(windows)` questions | done (`wrapper_or`'s existing `is_file()` checks needed no change — see commit message) | 80b8f60 |
| W4-3 — `run_core::macros` expand to Linux paths on a remote host | done (`$USER_HOME$` is a documented ceiling, stays Windows-side — see commit message) | 80b8f60 |
| W4-4 — `build_core` diagnostic paths translated back to UNC before the Problems dock | done | 80b8f60 |
| W4-5 — `dap_core::session` spawns through `ExecHost::command`; `Source.path` and breakpoints both ways | done | 80b8f60 |
| W4-6 — document the process-tree-kill ceiling | done (doc comment on `PtySession::kill_tree`, no code change — this is one of the plan's two "state, do not fix" items) | 80b8f60 |

### W5 — terminal

| Task | Status | Commit |
|---|---|---|
| W5-1 — a WSL project's default terminal is its distro at its Linux cwd | done | cf38b1b |
| W5-2 — `terminal-core` link resolution: a Linux `file:line` opens the UNC file | done (the shared resolver both the terminal and the run console use is `run_core::links::resolve_link`, not `terminal-core` itself — see commit message) | cf38b1b |

### W6 — watcher and index

| Task | Status | Commit |
|---|---|---|
| W6-1 — `ProjectWatcher::start` uses `notify::PollWatcher` on a remote root | done (takes `is_remote: bool` from `ui-shell` rather than depending on `process-exec` itself — a domain crate stays below the support layer, see commit message) | aed7d21 |
| W6-2 — state the index consequence; no code | done | aed7d21 |

### W7 — UX

| Task | Status | Commit |
|---|---|---|
| W7-1 — status-bar indicator `WSL: Ubuntu`, tooltip with distro and Linux root, on the existing `projectOpened` signal | done | 259fcbf |
| W7-2 — global `remote_wsl` off switch in `app-config`, default on | done (a process-wide `AtomicBool` in `process-exec`, set from `ui-shell`'s one project-open choke point; no settings-page checkbox yet — see commit message) | 259fcbf |

### W8 — docs and gates

| Task | Status | Commit |
|---|---|---|
| W8-1 — ADR-0052 + `layering.md` rows + `docs/README.md` lines | done (`layering.md` rows landed incrementally in the W1–W7 commits; this commit added the ADR and the `docs/README.md` index line) | 527472b |
| W8-2 — the manual Windows matrix below, walked once and recorded | **open — awaits a manual Windows/WSL session.** The implementing session ran in a Linux-only Docker container with no Windows machine or real `wsl.exe` available; every seam above was verified on Linux CI (unit tests, and a fake-`wsl.exe`-on-`PATH` trick proving the whole argv-wrapping path end to end) but the matrix below needs a real `\\wsl.localhost\...` project opened from real Windows. | |

## Verification

CI is Linux-only Docker, so nothing here spawns a real `wsl.exe`.
That is fine: every part with a decision in it is pure.

Unit-testable in CI (`make test`):

- `crates/process-exec/src/host.rs` — all four prefix spellings, mixed separators, mixed case, a distro with a hyphen or dot (`Ubuntu-22.04`), a path merely containing `wsl` (`C:/wsl/notaunc`) classifying `Local`; `to_remote`/`to_local` round-tripping to identity; `argv` as an exact `Vec<String>` comparison; `WSLENV` merged without dropping the inherited value; exit 127 and the two `wsl.exe` stderr strings mapping to `Failure::NotFound`.
- **End-to-end argv on Linux**: put a fake `wsl.exe` script on `PATH` in a tempdir that echoes its own argv as JSON, then run `process_exec::run` against a fabricated `//wsl.localhost/Ubuntu/tmp/x` `work_dir`. Proves `-d`, `--cd`, `-e` and env without Windows and without WSL — the same trick `analysis-core`'s scheduler tests use against `/bin/sh`.
- `lsp-core` — `uri_for`/`path_for` round trips and the `rootUri` sent in `initialize`, against the existing stub server (`crates/lsp-core/tests/stub_server_lifecycle.rs`).
- `build-core`/`dap-core` — a fixture of Linux-path compiler output producing UNC-path diagnostics.
- `run-core` — `python_program`, `wrapper_or` and macro expansion under a remote host.

Gates before each commit: `make test`, `make lint`, and the Qt-free checks in `CLAUDE.md`.

Manual Windows matrix (W8-2) — open `\\wsl.localhost\Ubuntu\home\<user>\projects\ide` from Windows and check in order:
status bar reads `WSL: Ubuntu`;
the Changes panel populates with no dubious-ownership error;
`rust-analyzer` starts and hover works;
Go to Definition into `~/.cargo/registry` opens a UNC path;
`cargo build` produces clickable diagnostics;
`cargo test` populates the test tree;
a breakpoint is hit under codelldb;
the terminal opens in the distro at the project root;
a file created inside the distro appears in the tree within the poll interval;
Stop terminates a running build.

## ADR

**ADR-0052 — Remote WSL: execute in the distro, keep file I/O on the share.**
One ADR, not three.
Records the scope boundary (process execution and path translation only); `ExecHost` as a value in `process-exec` rather than a new crate or a trait; classification as a pure function with no stored state; the tolerant-in/canonical-out rule; `-e` with resolved absolute programs over `--`; translation confined to `lsp-core`/`dap-core`/`build-core`; automatic activation with a global off switch; and the four accepted ceilings.
Related: ADR-0047 (`process-exec` as the mechanism crate), ADR-0046 (diagnostics URI keys stay Windows-side), ADR-0051 (the watcher this degrades), ADR-0016 (single root — why a per-project host is a single value), ADR-0040 and ADR-0041 (build and DAP paths gaining translation).

`layering.md` gains `process-exec` in the `pty-core`, `lsp-core` and `dap-core` rows with a one-line reason each.
