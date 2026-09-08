# Terminal experience: fast, smooth, JetBrains-comparable

A six-task plan to close the gap between this IDE's embedded terminal and a JetBrains-quality one: instant shell selection, smooth output, a proper font, and a theme-following ANSI palette.

## Context

The terminal worked, but four things about it fell short of what a user expects from a terminal that has already shipped.

| Symptom | Root cause |
|---|---|
| The "+" shell list takes seconds to open, and picking a shell takes seconds to start. | `pty_core::shells::detect()` ran on every menu `aboutToShow` (`crates/ui-shell/cpp/terminal_sessions_panel.cpp`) and again inside `start()` via `shells::find()` for both the requested id and the configured id (`crates/ui-shell/src/bridge/terminal.rs`). On Windows each call spawns `wsl.exe --list --quiet` (~1-3s), so one pick cost 2-3 WSL round-trips on the Qt thread. |
| Output is not smooth. | Each paint redrew five full-grid snapshots, per cell, with a `QString` allocation, a `fillRect` and a `drawText` per cell — and the reader thread queued output with no bound, so a fast-printing process could pile up paints faster than the UI thread could keep up. |
| The font looks generic. | The terminal used a 10pt `QFont::systemFont()` with the default family `"Monospace"`, never anything chosen for code. |
| Colours look flat and don't follow the theme. | Only ANSI 0-15 resolved, to hardcoded defaults that ignored the active colour theme; there was no 256-colour support, and selection was a plain fg/bg swap rather than a themed highlight. |

## Progress

Living status table — update the relevant row **in the same commit** that finishes a task, so status and code never drift apart.

| Task | Status | Commit |
|---|---|---|
| T1 — Shell catalogue off the critical path | done | f6a6ff2 |
| T2 — One snapshot, coalesced repaints, run-based painting | done | c24bae6 |
| T3 — Font, padding, palette, 256 colours | done | 2e9386a |
| T4 — Full keyboard translation, in terminal-core | done | this commit |
| T5 — Scrollback | open | |
| T6 — Docs and plan bookkeeping | open | |

## Decisions worth keeping

- Bundle JetBrains Mono as the default editor font.
- Settings > Terminal gets its own font family/size, which follows the editor font when left unset rather than duplicating a second hardcoded default.
- Scrollback is reachable by mouse wheel, the scrollbar, and Shift+PgUp/PgDn — the three ways a JetBrains terminal offers it.
- The ANSI palette is per-theme, JetBrains-like, with background/foreground following the editor's colours and full 256-colour support; selection is a themed highlight, not a bare fg/bg swap.

## Known ceilings

- The Terminal settings page's shell combo (`crates/ui-shell/cpp/terminal_page.cpp`, via `AppSettings::availableShells()`) still detects the shell catalogue synchronously, once per dialog open. T1 left this in place deliberately: a settings dialog opens rarely enough that one blocking detect is a non-issue, unlike the "+" dropdown this task fixed, which opens on every new terminal tab.
