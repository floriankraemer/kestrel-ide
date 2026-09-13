# Run, build and debug parity plan

## Context

The IDE ships a working but minimal run story, and has no build or debug story at all.

What exists at the time this plan was written:

- `run-core` — `RunConfig` (id, name, program, args, cwd, env), `LaunchSpec`, `ConsoleKind::{Pty, Pipes}` with `Pipes` declared but unused, a PTY `Supervisor` carrying N consoles, `detect` for Cargo binaries, `package.json` scripts and Makefile phony targets, output batching over a bounded ring, and `resolve_link` for Ctrl+Click on `file:line[:col]`.
- `ui-shell` — `RunServiceRust` with an ANSI stripper, `RunConfigEditorRust`, and the C++ `RunToolbar`, `RunConsolePanel`, `run_menu` and `run_config_dialog`.
- Per-project persistence of `[[run_config]]` blocks in `<project>/.ide/settings.toml`, scoped by `settings-model` as `runConfigs`.

ADR-0032 recorded those decisions and left three openings on purpose: SGR-coloured console output, a `Pipes` console for a future DAP client, and `LaunchSpec` as the debugger-agnostic launch seam.

What does not exist: typed run configurations, run from context, before-launch tasks, any build-tool invocation, any compiler-diagnostic parsing, and any debugger at all.
`RunToolbar` deliberately omits the mockup's Debug and Build buttons because neither has a backing command anywhere in the codebase.

This plan closes that gap against three IntelliJ IDEA help pages — running applications, compiling applications, and debugging code — across four toolchains: Cargo, CMake/C++, Python, and Maven/Gradle on the JVM.

## Progress

Living status table — update the relevant row(s) **in the same commit** that finishes a task, so status and code never drift apart.
A fresh session should read this table (and `git log`) before picking up work, per `CLAUDE.md`.

Task ids are stable; titles may change.
`blocked on X` means the task cannot start until X lands, not that it is unscheduled.
`open` means not started; this plan is the first in the repo to need that status, because it is written before the work rather than alongside it.

### R0 — the plan itself

| Task | Status | Commit |
|---|---|---|
| R0-1 — this plan doc + `docs/README.md` index line | done | (#179) |

### R1 — typed run configurations, macros, run from context

| Task | Status | Commit |
|---|---|---|
| R1-1 — `run_core::toolchain`: the toolchain table, `detect` rewritten on top of it | done | this branch |
| R1-2 — typed configurations: `toolchain` + `target` on `RunConfig`, back-compatible serde in `app-config` | done | this branch; a pair of strings rather than the enum this plan predicted, so `app-config` keeps depending on nothing |
| R1-3 — `run_core::macros`: `$PROJECT_DIR$`, `$FILE_PATH$`, `$FILE_DIR$`, `$FILE_NAME$`, `$USER_HOME$` in cwd, args and env | done | this branch |
| R1-4 — `run_core::context`: the configuration a file implies, temporary configurations with a cap | done | this branch |
| R1-5 — `allow_parallel` policy on top of the existing N-session supervisor | done | this branch; enforced in `RunService::launch`, exposed as a checkbox in the run-configuration dialog |
| R1-6 — adapter: `canRunFile`, `runContext`, toolchain/target/temporary/parallel across the FFI | done | this branch; one `runContext(path)` rather than a separate `runCurrentFile` — the menu action and the gutter icon are the same call |
| R1-7 — view: gutter Run icon, `run.runContext` (Ctrl+Shift+F10), allow-parallel checkbox | done | this branch; the icon sits on the first line (naming the entry point needs the symbol index) and the recent-first combo ordering is deferred — it needs a recency the settings file does not record |
| R1-8 — ADR-0039 + `layering.md` bullet + `docs/README.md` index line | done | this branch |
| R1-9 — E2E: `e2e_run_from_context_creates_a_temporary_configuration` | done | this branch; the run flows moved to their own `e2e_run.rs` binary because `e2e.rs` is at its size ceiling, and the flow drives `run.runContext` rather than a pixel-located gutter click |

### B1 — `build-core` and the Build dock

| Task | Status | Commit |
|---|---|---|
| B1-1 — new crate `build-core`: `BuildSpec` → the steps a request runs | done | this branch; over a PTY rather than `ConsoleKind::Pipes` — only the PTY transport can kill a build's process tree, so `Pipes` stays reserved for `dap-core` |
| B1-2 — Cargo diagnostics via `--message-format=json` | done | this branch |
| B1-3 — text diagnostic parsers (javac/Maven/Gradle, CMake/gcc/clang) + the streaming `DiagnosticParser` | done | this branch; its own table rather than `run_core::links`' — the two want different fields out of the same text |
| B1-4 — `BuildDiagnostic` in the shape `problems_panel` already renders | done | this branch |
| B1-5 — Build, Rebuild, build one target | done | this branch; "build file" dropped — no toolchain here addresses a single source file, they address targets |
| B1-6 — adapter: `BuildServiceRust`, one QObject for N builds, a thread per build | done | this branch |
| B1-7 — view: Build dock, the "&Build" menu, the Build toolbar button, build rows in the Problems dock | done | this branch; the Problems dock already had a Source column, so build rows only had to be a second source |
| B1-8 — ADR-0040 + `layering.md` row, verification gates, CI layering gate | done | this branch; the gate also gained the `run-core` rows it never had |
| B1-9 — E2E: `e2e_build_failure_populates_problems_dock` | done | this branch; found the carriage-return progress-redraw bug that made a real Cargo build produce no diagnostics at all |

### B2 — before-launch tasks

| Task | Status | Commit |
|---|---|---|
| B2-1 — `BeforeLaunchTask` in `run-core`; persistence; a Build task by default only where the run command does not already compile | done | this branch |
| B2-2 — sequential fail-fast execution; a failed task cancels the launch | done | this branch; the shared runner moved into `build_core::runner` so the Build task and the Build dock cannot drift, and `run_core::ansi` now holds the one ANSI stripper both need |
| B2-3 — cycle detection, validated before the first task runs | done | this branch |
| B2-4 — view: the Before launch list in the dialog, and a task's output in the Build dock | done | this branch; a one-task-per-line text field rather than an add/remove/reorder list — the same shape the environment field already uses, and a `Vec` is not a cxx-shareable field |

### D1 — `dap-core` foundation

| Task | Status | Commit |
|---|---|---|
| D1-1 — the framing, extracted into `stdio-framing` and shared with `lsp-core` | done | this branch; extracted rather than copied — the two protocols frame with the same bytes, which is what the plan's risk 5 said to check |
| D1-2 — `protocol`: the envelope, capabilities, and the five bodies something reads | done | this branch; the rest stays `serde_json::Value` on purpose |
| D1-3 — `session`: initialize → launch/attach → configurationDone, capability flags | done | this branch |
| D1-4 — `catalog`: codelldb, debugpy, java-debug, `[[debug_adapter]]` overrides, install hints | done | this branch |
| D1-5 — adapter lifecycle: spawn, reader thread, shutdown, every in-flight request failed on death | done | this branch; no separate `supervisor` module — the lifetime is the session's, and a second type for it would own nothing. Automatic respawn is deliberately not offered: a debug session that died has lost its debuggee, so restarting the adapter would attach to nothing |
| D1-6 — `runInTerminal` handed back to `run-core`'s PTY supervisor | done | this branch; the capability is claimed and answered in the same commit, which is the only safe way to move those two |
| D1-7 — ADR-0041 + `layering.md` rows for `dap-core` and `stdio-framing` + CI gate | done | this branch |

### D2 — breakpoints

| Task | Status | Commit |
|---|---|---|
| D2-1 — `BreakpointStore`: line, enabled, condition, hit condition, log message, temporary, suspend policy, dependent | done | this branch |
| D2-2 — function breakpoints, data breakpoints, exception filters, Mute Breakpoints | done | this branch |
| D2-3 — `shift_lines`, with a deleted line taking its breakpoint with it | done | this branch; the seam that drives it is wired in D3 |
| D2-4 — persistence under `.ide/local/breakpoints.toml` | done | this branch; temporary breakpoints are deliberately not persisted |
| D2-5 — view: gutter breakpoint column, `debug.toggleBreakpoint`, Mute Breakpoints | done | this branch; the conditions/log-message editor is `configureBreakpoint` on the seam with no dialog in front of it yet — D4 |

### D3 — the debug session and its tool window

| Task | Status | Commit |
|---|---|---|
| D3-1 — adapter: `DebugServiceRust`, one QObject for N sessions | done | this branch |
| D3-2 — stepping: over, into, out, run to cursor, resume, pause, stop | done | this branch; force step into and smart step into need `stepInTargets`, which is D4 rather than buttons that do nothing |
| D3-3 — Threads and Frames | done | this branch |
| D3-4 — Variables with lazy expansion, Set Value gated on the adapter's capability | done | this branch |
| D3-5 — Watches, re-evaluated on every stop, and Evaluate in the console | done | this branch |
| D3-6 — the debugger console, fed by DAP `output` events | done | this branch |
| D3-7 — inline values from the current frame's scopes | done | this branch; which value belongs on which line turned out to be a rule, not a paint decision — no shipped adapter implements DAP's optional `inlineValues`, so `dap_core::inline_values` associates a variable with the last line at or above the stopped one that mentions it as a whole word. The buffer text comes from the view, because a file being debugged may have unsaved edits |
| D3-8 — view: the Debug toolbar button, the "&Debug" menu, capability-gated enablement | done | this branch; the toolbar now has the whole cluster the mockup shows |
| D3-9 — E2E: `e2e_debug_stops_at_a_breakpoint`, plus a real-adapter conformance test | done | this branch; found that debugpy holds the `launch` response until `configurationDone`, so a client that waits for it deadlocks |

### D4 — the remaining debugger surface

| Task | Status | Commit |
|---|---|---|
| D4-1 — attach to a local process | done | this branch; the user gives the pid — enumerating processes portably is three implementations and a permissions story for a number they already have |
| D4-2 — remote `attach` configurations | done | this branch; there is still no common schema, so `dap_core::launch::remote_attach_arguments` is a table of what each of the three adapters documents — debugpy connects to a socket and maps paths, codelldb drives an LLDB `gdb-remote` command and maps sources the other way round, java-debug names a host and a port. The persisted half is one `[remote_attach]` block per project, path mappings included, because they are the same for everyone working on it |
| D4-3 — exception breakpoints, from the adapter's own filters | done | this branch; per adapter rather than per language — which exceptions can be broken on is something only the adapter knows, and it says so in `initialize` |
| D4-4 — reload changed classes where the adapter exposes it | done, unverified against a JVM | this branch; DAP has no capability flag for it, so `dap_core::catalog::supports_class_reload` is where the asymmetry is written down and the action is greyed out everywhere else. The request itself is java-debug's custom `redefineClasses`. **Not exercised**: java-debug is not in the builder image (see D4-6), so this ships tested only for the branch that disables it — it belongs on the manual matrix below |
| D4-5 — multiple simultaneous sessions, with a session picker | done | this branch; a picker rather than tabs — the dock already has four panes, and a second row of tabs above them buys nothing |
| D4-6 — the four-toolchain debug matrix | done as a documented manual matrix | decided: no adapters are added to the builder image. Python stays covered automatically (`dap-core/tests/debugpy.rs` + `e2e_debug_stops_at_a_breakpoint`); Cargo, CMake and the JVM are walked by hand against the script in §6 below and recorded there. The alternative — codelldb plus a JDK, Maven and Gradle in `linux-builder` — is several hundred megabytes on every image pull to automate a walk that takes minutes and changes rarely |

### R2 — console and run-widget ergonomics

An independent second lane; it may run at any point after R1.

| Task | Status | Commit |
|---|---|---|
| R2-1 — SGR colour as `FfiStyledRun` beside the unchanged plain text, per ADR-0032's named path | done | this branch; runs are offsets into the one cached string rather than a second copy of it, so `resolveLink` is untouched — and they cross the seam in UTF-16 units because that is what `QTextCursor` counts |
| R2-2 — `AnsiStripper` becomes `AnsiResolver` reusing `terminal-core`'s SGR state machine | done | this branch; the machine is reused by adding a second sink to it (`terminal_core::SgrResolver`) rather than by moving it — `AnsiStripper` stays as the wrapper that discards styling, which is all `build-core` wants |
| R2-3 — console find, pin tab, scroll lock, clear | done | this branch; the find bar asks `editor_core::search`, the matcher the editor and Find in Files already use. Doing it exposed an older bug: the cache and the widget trimmed their copies of the same text at different limits, so after either fired a Ctrl+Click opened whatever had moved into that offset — the view now trims only when `consoleTrimmed` says the cache did, and the truncation notice moved into the cached text so nothing on screen is unknown to the offsets |
| R2-4 — soft terminate before `kill_tree` | done | this branch; `kill_tree` had always *sent* TERM and then killed the child in the next line, so nothing was ever given the moment its own doc comment promised. Stop now sends TERM and escalates after `TERMINATION_GRACE`, on a thread of its own so a two-second wait does not freeze every other console's output, and Kill is the action that skips it |
| R2-5 — Show Running List over `Supervisor::active_ids` | done | this branch; over the adapter's own console map rather than the supervisor's ids — a popup must fill on the click, and the two cannot disagree because the same code sets both |
| R2-6 — the terminal's `linkAt()` unified with `run_core::links` | done | this branch; unified at the adapter rather than in either crate — `terminal-core` may not depend on `run-core`, so `TerminalSupervisor::linkAt` asks the grid for a URL and `run_core::links` for a `file:line`, and `ResolvedLink` gained the span the terminal needs to underline what it offers |

## 1. Decisions resolved before work starts

**Build is delegated, never owned.**
We invoke `cargo`, `cmake`, `gradle` and `maven`, parse their diagnostics, and navigate them.
We do not model compilation output folders, module output paths, artifacts, or build-automatically-on-save.
LSP already gives live errors, so an auto-build would duplicate a signal the user already has, and an output-path model is meaningful almost only on the JVM.
IntelliJ's compilation-output-folders page is therefore satisfied by reading the tool's own layout rather than by configuring ours.

**The debugger is a DAP client.**
A Qt-free `dap-core` shaped like `lsp-core`: blocking threads, `Content-Length` framing, a supervised child process, a catalog plus user overrides.
One client, N adapters — codelldb for Rust and C/C++, debugpy for Python, java-debug for the JVM.
*Rejected*: driving gdb/lldb machine interface directly (a bespoke protocol per debugger, no adapter ecosystem, and nothing to reuse for the JVM).

**One toolchain table, in `run-core`.**
`run_core::toolchain` is the single source of truth for which build tool a project uses and what its run argv, build argv and default debug adapter are.
`build-core` and `dap-core` consume it rather than each detecting again, per the layering rule against a second detection table.
File-to-language detection still comes from `syntax-core`'s registry (ADR-0018).

**`LaunchSpec` stays the seam.**
Run uses it with `ConsoleKind::Pty`, build with `Pipes`, and debug turns the same struct into a DAP `launch` body.
Nothing re-derives how a process is started.

**Three new E2E flows, and only three.**
The budget is 12–15 flows forever, and stands at 12.
R1, B1 and D3 take one each; no phase gets a fourth.

Out of scope, stated once: code coverage, CPU and memory live charts, run targets (Docker, SSH, WSL), artifact packaging, the run dashboard and Services tool window, stream and async debugging, and hot swap beyond whatever an adapter offers for free.

## 2. Architecture

### New crates

| Crate | Layer | Why not an existing crate |
|---|---|---|
| `build-core` | support | `run-core` launches a *user program* for a console; build invokes a *tool* and parses structured diagnostics out of it. Folding the parsers into `run-core` would make every run console carry a compiler-diagnostic dependency it never uses. |
| `dap-core` | support | The debug adapter protocol is a client with its own framing, session state machine and catalog, exactly parallel to `lsp-core`. It is not an adapter concern and must be unit-testable without Qt. |

### Layering rows

| Crate | Allowed imports | Qt/cxx-qt allowed |
|---|---|---|
| `run-core` | `pty-core`, `app-config`, `terminal-core` (+ std, serde, toml, serde_json, regex) — unchanged | **No** |
| `build-core` | `run-core`, `app-config`, `syntax-core` (+ std, serde, serde_json, regex) | **No** |
| `dap-core` | `run-core`, `app-config`, `syntax-core` (+ std, serde, serde_json) | **No** |

`dap-core` takes a normal dependency on `syntax-core` for the same reason ADR-0035 gave `lsp-core` one: the adapter for a session is chosen by language id, and that id must not be re-derived from a second extension table.

Both new crates take the tokio gate as well as the Qt gate: long work runs on a `std::thread` and returns through `CxxQtThread::queue()`, never on an ambient runtime.

### Where logic may live

- `run-core` owns the toolchain table, macro expansion, before-launch task ordering, and the run console's link table. It must not grow a diagnostic parser.
- `build-core` owns build invocation and diagnostic parsing. It must not re-derive file-to-language detection and must not open a second Problems model — its diagnostics reach the view through the shape `lsp_core::DiagnosticStore` already uses.
- `dap-core` owns the protocol, the session state machine, the adapter catalog and the breakpoint store. It must not own an editor buffer; line shifting is driven from the existing buffer-edit seam in `ui-shell`.

### QObjects

`BuildServiceRust` and `DebugServiceRust` each follow the ADR-0032 precedent: one registered `#[qobject]` type owning a `HashMap<u64, Session>`, not one QObject per session, because cxx-qt registers a type's `QMetaObject` once at build time.

## 3. ADRs to write

**ADR-0039: typed run configurations, macros and before-launch tasks.**
Amends ADR-0032.
Records why the toolchain table lives in `run-core` rather than in a new crate, why `RunConfigKind` defaults to `Custom` so existing `.ide/settings.toml` files keep loading, and why macro expansion covers args as well as cwd and env.

**ADR-0040: `build-core` — delegate to the build tool and parse its diagnostics.**
Records why there is no IDE-owned output-folder, artifact or auto-build model, why Cargo is parsed from JSON while the other toolchains are parsed from text, and why build diagnostics join the existing Problems dock instead of getting a second panel.

**ADR-0041: `dap-core` — a DAP client, its adapter catalog, and who owns breakpoints.**
Records DAP over gdb/lldb MI, the `lsp-core`-shaped structure, the `syntax-core` dependency on the ADR-0035 precedent, and why the breakpoint store is Qt-free and shifted through the existing buffer-edit seam rather than through a new editor hook.

## 4. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | Debug adapters are external binaries the user may not have. | The catalog reports a missing adapter as a typed error with the install hint, exactly as `lsp-core`'s server catalog already does for language servers. |
| 2 | Four toolchains multiply the test matrix. | Only Cargo is exercised by an E2E flow and by CI; the other three are a manual matrix recorded in D4-6, mirroring how the LSP conformance suite is kept out of the per-PR gate. |
| 3 | Text diagnostic parsing is brittle across tool versions. | Cargo, the toolchain we dogfood, is parsed from JSON. The text parsers are table-driven with fixture files, so a broken format is a fixture change, not a rewrite. |
| 4 | The E2E budget is full on arrival. | Exactly three flows are added, taking the budget from 12 to 15, its stated ceiling. Any further flow means deleting one. |
| 5 | `dap-core` could drift into a second `lsp-core`. | Framing is the same wire shape; if the second implementation is byte-identical in behaviour it is extracted into a shared module rather than duplicated, and that extraction is the decision ADR-0041 must state either way. |

## 5. Verification

Per task, before every commit:

```sh
make lint
make test
cargo tree -p build-core -e normal | grep -i qt      # must be empty
cargo tree -p build-core -e normal | grep -i tokio   # must be empty
cargo tree -p dap-core   -e normal | grep -i qt      # must be empty
cargo tree -p dap-core   -e normal | grep -i tokio   # must be empty
```

End to end, per phase, in the headless harness under Xvfb:

- R1 — click the gutter run icon on a `fn main`, see a temporary configuration appear in the toolbar combo and its output in the console.
- B1 — introduce a deliberate compile error, press Build, confirm the Problems dock lists it at the right file and line and that double-clicking navigates there.
- B2 — attach a Build before-launch task to a run configuration, break the build, confirm the run never starts and the failure is visible.
- D3 — set a breakpoint in a Rust binary, press Debug, confirm the session suspends on that line, that Variables shows locals, that step over advances one line, and that resume runs to exit.
- D4 — repeat the D3 walk once per toolchain, by hand, against §6.

## 6. The manual debug matrix (D4-6)

Only Python is verified automatically. The other three toolchains need a debug adapter that is not in `linux-builder` and, for the JVM, a whole JDK and build tool with it — several hundred megabytes on every image pull to automate a walk that takes a few minutes and changes rarely.
So they are walked by hand, and the result is written down here rather than assumed.

Run this before a release, and after any change to `dap-core`'s session, launch or catalog modules.

**The walk, identical for every toolchain**: open the project, set a breakpoint on a line inside the entry point, press Debug, confirm the session suspends on that line and the gutter marks it; confirm Variables lists the locals with plausible values and that the inline values appear at the end of their lines (D3-7); step over once and confirm the execution point advances exactly one line; resume and confirm the program runs to exit and the session ends.

| Toolchain | Adapter | How to get it | Last walked | Result |
|---|---|---|---|---|
| Python (`debugpy`) | debugpy | `python3-debugpy`, already in `linux-builder` | automated | `cargo test -p dap-core --test debugpy` and `e2e_debug_stops_at_a_breakpoint` on every CI run, which since R5 also opens Edit Breakpoint on the breakpoint's line, gives it a condition, confirms the session only suspends once that condition is true rather than on the first hit, then edits a local's value through the Variables tree and confirms the debuggee's own output reflects the new value after Resume |
| Cargo | codelldb | [vadimcn/codelldb releases](https://github.com/vadimcn/codelldb/releases), then a `[[debug_adapter]]` override pointing at it | not yet | — |
| CMake / C++ | codelldb | the same install; the toolchain table already maps both to it | not yet | — |
| Maven / Gradle | java-debug | a JDK plus [microsoft/java-debug](https://github.com/microsoft/java-debug), launched as a `[[debug_adapter]]` command | not yet | — |

The JVM row carries one extra step, because it is the only toolchain where anything depends on it: with the session suspended, edit a method body, rebuild, and use Debug > Reload Changed Classes (D4-4). That action is disabled for every other adapter, so this row is the only place it can be exercised at all.

Rows read "not yet" rather than being left blank on purpose: an empty cell looks like an oversight, and this one is a decision.
