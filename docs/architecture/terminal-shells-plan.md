# Terminal shells and the project start directory

## Why

The embedded terminal spawned one hard-coded shell and started in the wrong directory.
`resolve_shell()` in `ui-shell`'s `bridge/terminal.rs` was a two-branch `cfg` — PowerShell on Windows, `$SHELL` everywhere else — and it never set a working directory, so every tab inherited the IDE process's own.

Two consequences a user hit daily: on Windows there was no way to reach WSL, `powershell.exe` or `cmd.exe`, and on every platform a fresh terminal needed a manual `cd` before it was useful.
The old behaviour also quietly contradicted `TerminalSupervisor::linkAt`, which resolves a relative `file:line` against the project root on the stated grounds that this is where a terminal starts.

The shape is IntelliJ's: a project-scoped **Settings > Terminal** page (shell, start directory, environment) and a `"+"` split button whose dropdown lists the shells the machine actually offers.

## Progress

Living status table — update the relevant row(s) **in the same commit** that finishes a task, so status and code never drift apart.

| Task | Status | Commit |
|---|---|---|
| T1 — `pty_core::shells`: the catalogue, per-platform lists built from injected inputs | done | this branch |
| T2 — `app_config::TerminalSettings`, its project layer, and `ScopedField::Terminal` | done | this branch |
| T3 — `shell_for` in the adapter: precedence, project-root cwd, environment; `availableShells()` across the seam | done | this branch |
| T4 — the `"+"` dropdown, `terminal.selectShell`, shell-named tabs | done | this branch |
| T5 — Settings > Terminal, project-scoped like Editing | done | this branch |
| T6 — ADR-0007's shell-selection section, ADR-0022's fifth scoped area, `overview.md`, this doc | done | this branch |

## Decisions worth keeping

**The catalogue lives in `pty-core`.**
Which shells exist is a process fact, and the crate already owned `ShellSpec` and `WindowsShellKind`.
`ShellSpec`'s promise that constructors never probe the OS is intact: probing is a function a caller asks for by name.

**Each platform's list is a pure function.**
`unix_candidates` and `windows_candidates` take the machine's answers — `$SHELL`, `/etc/shells`, `wsl.exe --list --quiet`, and a "can this be launched?" predicate — as arguments, and `detect()` is the shim that reads them.
`detect()` branches on `cfg!(windows)` rather than `#[cfg]`, so both platforms' code compiles and is checked everywhere.
That is what keeps the WSL and PowerShell paths covered by tests on Linux CI, which is the property ADR-0007 bought with `WindowsShellKind`.

**Shells are stored by id, not by path.**
`system`, `zsh`, `pwsh`, `wsl:Ubuntu`.
A committed project file then works on two machines that install `zsh` in different places, and a machine that no longer has the named shell falls back to the platform default instead of failing to spawn.

**The terminal is project-scoped (ADR-0022's fifth area).**
A repository whose tooling only runs under WSL, or under `bash` on a machine whose owner uses `fish`, is describing the checkout rather than the person.
`settings_model::scope`'s own test asserts the number of scoped areas precisely so that widening the list cannot happen without an ADR change; that test and ADR-0022 moved together.

**No new draft QObject for the settings page.**
Editing and Language Servers each have one because they hold a list the user manipulates before OK.
The Terminal page is a form: its widgets are the draft, `AppSettings::terminalSettings()` reads the layer the scope selector names, and `saveTerminalSettings()` writes it back on OK.

## Known ceilings

- `detect()` spawns `wsl.exe --list` on Windows. It runs when the dropdown opens and when the settings page is built, not per keystroke.
- Candidates carry no `--login`/`-i` argument, which matters most on macOS. The page's arguments field is the escape hatch; auto-adding login flags is a separate decision.
- Nothing consumes OSC title sequences, so a shell never renames its own tab (`terminal-core` discards those events). A tab opened from the dropdown is named after the shell that was picked.

## Verification

`cargo test --workspace` and `make lint`, plus the Xvfb pass ADR-0032 chose for the terminal rather than a new automated E2E flow — the flow budget stands at its stated ceiling of 15 (`run-build-debug-parity-plan.md`), and adding one would mean deleting another.
Windows (WSL, PowerShell, cmd) has no CI runner and goes on the manual release checklist, as `pty-core`'s Windows paths already do.
