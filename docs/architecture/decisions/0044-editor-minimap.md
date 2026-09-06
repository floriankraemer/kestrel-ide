# 0044. Editor minimap (code map) with configurable overlays

## Status

Accepted.

## Context

The editor has a left gutter (run icon, breakpoints, VCS change markers, fold triangles, line numbers, blame) but nothing on the right except the plain `QScrollBar`.
Every mainstream editor puts an overview strip there — VS Code's minimap, Sublime's minimap, IntelliJ's error stripe — because it gives two things a scrollbar cannot: a scaled rendering of the whole file, so the reader recognises where they are by shape, and a density map of interesting rows across the whole file rather than only the visible part.

This adds that strip: a scaled, syntax-coloured rendering of the file on the right edge of every `CodeEditor`, with a draggable viewport slider and a set of individually switchable overlays, behind a global "Show minimap" setting that is on by default (issue #199).

Researched against VS Code's minimap and IntelliJ's error stripe so the overlay set is not invented: find matches, errors/warnings, VCS added/modified/removed, the caret line and breakpoints are each shipped as an independent checkbox.
The primary selection and section headers/bookmarks/TODOs are deferred — the primary selection is already on screen, and the IDE has neither bookmarks nor a TODO scanner.

## Decision

### 1. Colours come from the document, not from a new seam into the highlighter

`SyntaxHighlighter` already writes resolved `QTextCharFormat`s into each block via `setFormat`, which Qt stores on `QTextBlock::layout()->formats()`.
Reading those gives the minimap the exact colours on screen — tree-sitter spans and the LSP semantic-token overlay merged — with no accessor added to `SyntaxHighlighter`, no second parse, and no palette cache to invalidate on theme change.

Rejected: exposing the highlighter's internal span/format caches (a new public seam plus a UTF-8/UTF-16 offset mapping the view would have to redo), and a second tiny-font `QPlainTextEdit` (unreadable, and it would need its own highlighter).

### 2. Rows are visible blocks, so the scrollbar is the mapping

`CodeEditor` is `NoWrap`, so one block is one visual line, and folding hides blocks with `setVisible(false)`.
`QPlainTextEdit`'s vertical scrollbar therefore already counts *visible* lines: `value()` is the first visible row, and `maximum() + pageStep()` is the total.
The minimap iterates blocks skipping `!block.isVisible()` and uses the scrollbar directly for slider position and for click/drag targets — folding stays consistent for free, and there is no second coordinate system to keep in sync.

### 3. Paint only the visible window of the map, into a cached pixmap

When the file is taller than the strip, the strip scrolls proportionally, so at most `height() / kRowHeight` rows are ever drawn.
The rendered code is cached in a `QPixmap` keyed by (document revision, first row, size, base palette colour) — the same "recompute only when revision or visible range changed" idiom `CodeEditor`'s whitespace-glyph cache already uses.
Overlays and the slider paint on top of the cached pixmap every time, so a caret move or a scroll costs one blit plus a handful of rects.
No file-size ceiling is needed: cost is bounded by the widget, not by the document.

### 4. No new crate, no Qt-free logic

Everything the minimap decides is view geometry (widths, row heights, hit testing) — the same category as `lineNumberAreaWidth()`, which also lives in C++ and is untested by design (ADR-0002).
The only rule that leaves the view is *which* overlays are on, and that is data in `app-config` (`MinimapSettings`, mirroring the `EditingSettings` precedent), bridged across FFI the same way `FfiWhitespaceOptions` is.

### What was deferred

- **Selection overlay** — the primary selection is already visible on screen.
  A minimap tick for it is noise.
- **Section headers / bookmarks / TODOs** — the IDE has neither bookmarks nor a TODO scanner to source them from.
- **A block-number → visible-row index for overlay painting** — overlay rows (breakpoints, diagnostics, matches) are found by walking blocks from the top of the document per item, skipping hidden ones, rather than building a full index.
  This is `O(overlay items × document blocks)`, acceptable for the handful of items a normal file has; a file whose find-in-file lights up thousands of matches would pay for a walk per match.
  Upgrade path: a single per-paint block-number-to-visible-row array, built once per cache miss, if that shows up as a real stall.

## Consequences

- `app_config::Settings` gains a `minimap` field (`MinimapSettings`), on by default so a `settings.toml` written before this feature resolves to "on with every overlay" — the same backward-compatibility shape `mcp_enabled` uses, without the `Option<bool>` indirection, because every field here really does default to `true`.
- `crates/ui-shell/src/bridge/settings.rs` crosses its 1500-line ceiling by 5 lines for the new load/save pair; ratcheted in `scripts/check-file-size.sh`, no split planned.
- `crates/ui-shell/cpp/minimap.{h,cpp}` is a new, self-contained translation unit — `CodeEditor` owns one `Minimap*` child, the same relationship it has with `LineNumberArea`, and no `Q_OBJECT` is needed since it calls its `CodeEditor` directly and emits no signals.
- The Editor settings page gains a "Show minimap" master checkbox with five overlay sub-checkboxes, live-previewed and persisted the same way "show whitespace characters" already is.
