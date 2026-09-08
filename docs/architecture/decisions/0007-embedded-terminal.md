# 0007. Embedded terminal: `portable-pty` + `alacritty_terminal` + custom `QPainter` grid widget

## Status

Accepted.
The `windows-artifact` Docker target (MXE cross-build to `x86_64-pc-windows-gnu`) built
`app.exe` clean with `pty-core`/`terminal-core` in the dependency tree. Verified by
inspecting the resulting binary's PE import table (`objdump -p`): `portable-pty` and
`alacritty_terminal` are fully statically linked into `app.exe` — neither adds any new
runtime DLL dependency beyond the Qt6/mingw runtime set the bundle already ships (only
`KERNEL32`/`advapi32`/`ws2_32`/`userenv`/`combase`/`bcryptprimitives`/standard
`api-ms-win-core-*` system DLLs appear alongside `Qt6Core/Gui/Widgets`,
`libstdc++-6`, `libwinpthread-1` — no ConPTY-related DLL surprises). The gate this ADR
was pending on is closed.

## Context

The language-folding/Class-View/terminal/search plan (task F, tracks F1-F3) calls for an
embedded, dockable, cross-platform shell: PowerShell, WSL2, or a regular Unix shell, running
inside the IDE rather than in an external terminal window.
This needs two things a Qt-only approach can't give cleanly: a PTY transport that behaves the
same on Windows (ConPTY) and Unix (a real PTY), and VT100/escape-sequence interpretation that
can be unit tested without a running Qt event loop.

## Decision

Split the terminal into two new Qt-free crates plus a humble view, mirroring the
`syntax-core`/`SyntaxHighlighter` split already in this codebase:

- `pty-core` (this task, F1): spawns a shell attached to a pseudo-terminal via the
  `portable-pty` crate, which abstracts Windows ConPTY and Unix PTYs behind one API.
  Exposes blocking read/write, resize, and kill/wait on the child process.
  Shell resolution is caller-driven, not OS-probed: Unix reads `$SHELL` with a `/bin/bash`/
  `/bin/sh` fallback; Windows takes an explicit `WindowsShellKind` (`PowerShellCore` →
  `pwsh.exe`, `WindowsPowerShell` → `powershell.exe`, `Wsl` → `wsl.exe`) so no constructor
  probes the host OS, keeping shell selection testable on any single platform.
- `terminal-core` (task F2, not yet built): consumes `pty-core`'s byte stream through
  `alacritty_terminal`'s VT100/grid-state engine, producing cursor/selection/cell-grid state
  that can be asserted against in unit tests using known escape-sequence fixtures.
- A `QPainter`-based grid widget in `ui-shell`'s `cpp/` (task F3, not yet built): paints the
  cell grid `terminal-core` produces and forwards keystrokes back through the bridge.
  Same "humble view" shape as `SyntaxHighlighter`'s Qt-side consumer — no VT100 logic in C++.

Background PTY reads run on a plain `std::thread` doing blocking reads, marshaled to Qt via
`CxxQtThread::queue()` — the same pattern `start_mcp_server` already uses for its listener
thread (`ui-shell/src/bridge.rs:1160-1182`).
No `tokio` or other async runtime in `pty-core` or `terminal-core`; `tokio` stays scoped to
`mcp-server`.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| `QTermWidget` (Qt-based terminal emulator widget) | Puts VT100/escape-sequence interpretation and PTY handling behind Qt, making it untestable without a running Qt event loop and violating the "business rules live in Qt-free crates" layering rule this codebase enforces everywhere else. |
| Hand-rolled VT100 parser | A correctness minefield — full-featured terminal emulation (cursor movement, scrollback, color modes, alternate screen buffer, etc.) is exactly the kind of well-trodden, easy-to-get-subtly-wrong problem a maintained crate (`alacritty_terminal`, extracted from a real terminal emulator and already battle-tested) is worth depending on for. |
| Hand-rolled per-platform PTY code | Windows ConPTY and Unix PTYs have meaningfully different APIs; `portable-pty` already abstracts this split behind one interface and is maintained by the same project (wezterm) that also produces `alacritty_terminal`'s closest competitor, so its Windows path is exercised in practice, not theoretical. |

## Consequences

- Positive: PTY transport and VT100 state are both unit-testable in CI without Qt, matching
  every other domain crate in this codebase.
- Positive: the view stays humble — `cpp/`'s terminal widget only paints cells and forwards
  input, with zero VT100 or PTY logic to maintain in C++.
- Negative / accepted risk: two new native-dependency crates (`portable-pty`, later
  `alacritty_terminal`) whose Windows/ConPTY and MXE cross-build behavior has not yet been
  verified end to end — this was the open item this ADR was Proposed on, closed by the MXE
  `windows-artifact` cross-build recorded in the Status section above.
  `pty-core`'s Linux build and unit tests (spawn/read, resize, kill/wait) are verified clean
  under the Docker `linux-builder` stage as of this task; the MXE cross-build path is not yet
  checked.
- `terminal-core` (task F2) landed on `alacritty_terminal` 0.26.0, not 0.24: 0.24.2's `tty`
  module failed to build on this toolchain (a `rustix`/`rustix-openpty` version-unification
  conflict — two incompatible major versions of `rustix` in the dependency graph produced a
  trait-not-implemented error unrelated to any code in this repo). 0.26.0 resolves clean.
  `terminal-core` doesn't use `alacritty_terminal`'s `tty` module at all (it consumes bytes
  from `pty-core` instead), so this was purely a "does the crate compile" gate, not a feature
  gap. Its Linux build and unit tests (known VT100/SGR/CUP escape-sequence fixtures) are
  verified clean under `linux-builder`; the MXE cross-build path was checked afterwards, in
  the `windows-artifact` build the Status section records.
- Selection state, `http(s)` link detection, and paste sanitizing/bracketing (task F4) also
  live in `terminal-core` and are unit tested there.
  The widget contributes only pixel-to-cell arithmetic, clipboard access, and
  `QDesktopServices::openUrl` — the same humble-view split as F3's painting.
  Selection itself delegates to `alacritty_terminal`'s own `Selection`/`selection_to_string`
  rather than a hand-rolled model, so word and line expansion come from the emulator's
  semantic rules instead of a second implementation of them.
- Accepted limitation: `GridSize::total_lines() == screen_lines()`, so there is no scrollback
  and `display_offset` is always 0.
  Both the viewport-row-to-`Line` mapping F4's selection and link lookup depend on and the
  "a resize clears the selection" rule rest on that invariant, which is asserted in debug
  builds — adding scrollback means revisiting both.
  This limitation is closed — see "Rendering pipeline, palette, keys, scrollback" below.
- The `WindowsShellKind` enum only names the shell; nothing in `pty-core` verifies the named
  executable actually exists on the target machine before spawning, that's a caller-side
  concern for whichever task builds the terminal dock widget's shell-picker (F3 or later).
  That task has since landed — see the section below.

## Shell selection and the start directory (later addition)

The deferral above is closed.
Which shells a machine offers is now `pty_core::shells`, a catalogue beside `ShellSpec`; which one the user wants is `app_config::TerminalSettings`, project-scoped like `[editing]` (ADR-0022); and which of the two wins for a given tab is `shell_for` in `ui-shell`'s `bridge/terminal.rs`.

The rule this ADR set for `ShellSpec` still holds and is what shaped the catalogue: constructors do not probe the OS.
Probing is a function a caller asks for by name, and each platform's list is built by a pure function taking the machine's answers — `$SHELL`, `/etc/shells`, `wsl.exe --list --quiet`, and a "can this be launched?" predicate — as arguments.
That is what keeps the Windows catalogue, WSL distros included, covered by tests on Linux CI, which is the same property `WindowsShellKind` was introduced for and the reason there is still no Windows runner to need.

Two consequences worth writing down:

- A shell named in a settings file but since uninstalled falls back to the platform default rather than failing to spawn.
  A machine that had `fish` yesterday is a normal thing to find, and a terminal that refuses to open because of it would be worse than one that opens `bash`.
- A new terminal starts in the open project's root, not the IDE process's working directory.
  The old behaviour also quietly contradicted `TerminalSupervisor::linkAt`, which resolves a relative `file:line` against the project root on the stated grounds that this is where a terminal starts.

## Rendering pipeline, palette, keys, scrollback (2026-09-08, later addition)

`docs/architecture/terminal-experience-plan.md` closed four remaining gaps against a JetBrains-quality terminal, without moving anything across the humble-view line this ADR drew.

- **Rendering.** The seam carried five full-grid accessors per paint (`gridRows`/`gridCols`/`gridCells`/`cursorRow`/`cursorCol`); it now carries one, `snapshot(session_id) -> FfiTerminalSnapshot`, with `cells: Vec<FfiTerminalCell>` nested directly in the cxx shared struct.
  The reader thread's `feed` no longer emits one `grid_updated` per read: an `Arc<AtomicBool>` coalesces bursts into at most one signal in flight per session (swap-before-queue, store-before-emit), and its read buffer grew from 4 KiB to 64 KiB.
  `TerminalWidget` caches the last snapshot behind a `snapshotStale_` flag and repaints by run — grouping consecutive cells sharing `(fg, bg, bold, italic, underline, selected, is_cursor)` into one `fillRect`/`drawText` pair — instead of one of each per cell.
- **Palette.** `terminal_core::Palette` (foreground, background, cursor, selection, 16 ANSI colours) replaces the hardcoded ANSI 0-15-only table; `Palette::xterm()` is `Default` and is pinned byte-identical to the old hardcoded values by a dedicated test, so `sgr.rs` and the ANSI-stripped run console (ADR-0032) are unaffected.
  Indexed colours 16-255 now resolve: the 6×6×6 cube and the greyscale ramp.
  `theme.cpp::terminalPaletteForTheme` maps the active colour theme to a JetBrains Darcula/light ANSI set, falling back to `ChromePalette` when no editor colours are configured — the same theme-resolution helper the editor already uses, not a second one.
- **Keys.** `terminal_core::keys::encode(Key, Modifiers, app_cursor) -> Option<String>` is the one place xterm's key encodings are decided — arrow/Home/End's `CSI`-vs-`SS3` split on app-cursor mode, modified-key `CSI 1;{m}` parameters, F-key `SS3`/`CSI ~` sequences, Ctrl/Alt encoding.
  `TerminalWidget::keyPressEvent` is a `Qt::Key -> FfiTerminalKey` switch and nothing more; a `ShortcutOverride` handler keeps arrows/Tab/Home/End/PageUp/PageDown reaching the terminal instead of Qt's focus chain, while leaving Ctrl+` (dock toggle) and Ctrl+Shift+C/V (copy/paste) alone.
- **Scrollback.** `alacritty_terminal`'s 10,000-line history is reachable: `scroll`/`scroll_to`/`scroll_to_bottom`/`scroll_state` wrap `Term::scroll_display`, and the two `debug_assert_eq!(display_offset, 0)` tripwires this ADR's Consequences section named are gone — `point_at`/`link_at` map a viewport row through `display_offset` instead of assuming it is zero.
  The widget adds a `QScrollBar`, wheel-to-scroll (redirected to Up/Down key presses while `alt_screen()` is true, so `vim`/`less` still get arrow keys instead of a view scroll), and Shift+PgUp/PgDn/Home/End.
  Any key input scrolls back to the bottom, matching every other terminal's behaviour.

None of this moved logic across the seam: encoding, palette resolution, and scroll-offset math are still Qt-free and unit-tested in `terminal-core`; `bridge.rs` still only translates; `cpp/` still only paints and forwards input.

## Related

- [ADR-0003: FFI seam conventions](0003-ffi-conventions.md) — typed-error convention `pty-core`
  follows internally now, ahead of crossing the FFI seam in task F3.
- [ADR-0004: MCP transport](0004-mcp-transport.md) — the `std::thread` + `CxxQtThread::queue()`
  background-work pattern this ADR reuses for PTY reads.
- `crates/pty-core/src/lib.rs` — the crate this ADR documents.
- `docs/architecture/language-folding-classview-terminal-search-plan.md` — decision 3 and the
  F1-F3 task breakdown this ADR covers.
