# Implementation plan: interactive hover card

## Status

In progress.

## Progress

| Task | Status | PR / commit |
|---|---|---|
| H1 | done | (this PR) |
| H2 | open | |
| H3 | open | |
| H4 | open | |
| H5 | open | |
| H6 | open | |
| H7 | open | |

## Context

Hovering a symbol or squiggle shows `EditorPopup`, filled by a composition rule that lived in the bridge (`compose_hover_html`).
The popup closes on the first mouse move, so links and scrolling only work when it is pinned with Ctrl+Alt+Q.
It offers no quick fixes; fixes are reachable only from the bulb or Alt+Enter at the caret.
The content is flat: documentation, then one `Warning (source): message` line per diagnostic, with no diagnostic code and no source location.
The popup is unthemed and the hover delay is Qt's default tooltip wake-up, not configurable.
The goal is a JetBrains-style card: problems first with fix links, then signature and docs, then a "Source:" footer, with a pointer that can travel into it, a menu, and settings.

## Design summary

Rust owns what the card contains and which fix is primary.
C++ only renders HTML, routes `ide:` anchors to signals, and handles mouse and timer geometry.

- `crates/lsp-core/src/hover_card.rs` holds `HoverCard`, `CardProblem`, `FixState`, `CardLocation`, `split_signature`, `primary_fix` and `render`.
- `render` emits `QTextDocument`-compatible HTML with anchors `ide:fix/<problem>`, `ide:more/<problem>` and `ide:source`, and escapes every server-provided string (ADR-0021).
- It chooses no colours: elements carry semantic classes (`problem`, `sev-error`, `dim`, `signature`, `doc`, `source`, `fix`) that the popup maps to theme colours through a default stylesheet.
- Fixes load asynchronously per problem through the existing intentions path, and a hover token drops stale answers.
- The Source footer comes from the index's declaration resolution, with no extra LSP round-trip.
- The popup gets a close grace period, an overflow menu, a themed QSS rule and a configurable dwell timer.

## Task breakdown

| Task | Scope |
|---|---|
| H1 | `hover_card` model, `split_signature`, `render`, `primary_fix`; the bridge uses it (same content, new structure, no fixes yet); `DiagnosticRow.code` |
| H2 | Themed popup, close grace, `ide:` anchor routing, overflow menu, custom dwell timer |
| H3 | Per-problem fixes (Loading to fix row), `applyHoverFix`, More actions, `code.applyPreferredFix` |
| H4 | Source footer via index resolution, Go to Declaration |
| H5 | Red and yellow themed bulb (`bulb_kind`) |
| H6 | `HoverSettings` and Editor page rows, overflow-menu toggle wiring |
| H7 | ADR-0065, README index, `overview.md` touch-up, E2E pass |
