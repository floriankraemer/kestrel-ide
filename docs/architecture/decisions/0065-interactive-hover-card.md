# 0065. Interactive hover card

## Status

Accepted.
Supersedes the hover part of R3 in the [IntelliJ parity refinement plan](../intellij-parity-refinement-plan.md).
Extends [ADR-0021](0021-ai-chat.md)'s rule that server-provided text is untrusted to the hover card.
Delivered by the [hover card plan](../hover-card-plan.md).

## Context

R3 gave hover, quick documentation and signature help one shared `EditorPopup`, but the popup stayed a tooltip in behaviour.
The first mouse move closed it, so its links and scrollbar worked only after pinning it with Ctrl+Alt+Q.
It offered no quick fixes: those were reachable only from the bulb or Alt+Enter at the caret.
The content was one flat run of documentation followed by one `Warning (source): message` line per diagnostic, with no diagnostic code and no source location.
The composition rule lived in the bridge, which the layering rules reserve for translation.
The popup was not themed, the bulb colour was hard-coded, and the hover delay was Qt's tooltip wake-up rather than a setting.

JetBrains IDEs show problems first with their fixes, then the signature and documentation, then where the symbol is declared, and let the pointer travel into the card.
The forces are: server text is untrusted, rules must stay out of C++, fixes arrive asynchronously and must never be applied against a card the user has left, and the card has to be usable with the keyboard as well as the mouse.

## Decision

The card is rendered to HTML by Qt-free Rust, carries its actions as `ide:` anchors, and is shown by a thin C++ popup that only handles geometry and timers.

- **Model and rendering** (`lsp_core::hover_card`).
  `HoverCard` holds the problems (severity, message, source, code, fix state), the signature, the documentation HTML and the source location.
  `render` emits the `QTextDocument` HTML subset, escaping every server-provided string.
  It chooses no colour: elements carry semantic classes (`problem`, `fixrow`, `dim`, `signature`, `doc`, `source`, `path`) that the popup maps onto the active theme through a default stylesheet, refreshed on every show so a live theme switch is honoured.
  The signature is the first fenced block of a Markdown hover, lifted out by `split_signature`, and is coloured by the same tree-sitter highlighter and theme palette the editor and the Markdown preview use.
- **Actions are anchors.**
  `ide:fix/<n>` applies problem *n*'s primary fix, `ide:more/<n>` opens its intentions under the card, and `ide:source` jumps to the declaration.
  The popup turns an `ide:` click into `actionRequested`, and `EditorTabs` decides what it means.
  Every other URL keeps going through the desktop's opener (ADR-0021).
- **Translation.**
  Rust cannot translate, so the view hands `CardLabels` (the fixed words and the user's shortcut hints) to the bridge once, and `render` takes them as a parameter.
  English defaults keep the crate usable without a view.
- **Fixes are asynchronous and token-bound.**
  The card shows at once with each problem in the `Loading` state.
  A worker asks the existing intentions path for that problem's fixes, and the answer is dropped unless its hover token is still current.
  `HoverFixes` holds the fetched intentions only while their card is on screen: it is emptied when the popup closes or shows anything else, and a late answer for another token is refused.
  Alt+Shift+Return (`code.applyPreferredFix`) and the fix link therefore can never apply a fix the user is no longer looking at.
  With no card, the shortcut applies the caret's primary fix.
- **Primary fix.**
  `primary_fix` is the first preferred enabled quick fix, else the first enabled quick fix.
  Disabled actions are never primary.
- **Pointer behaviour.**
  The popup gives the pointer 300 ms to travel from the hovered word into the card.
  Entering the card stops the close timer, and leaving it starts the timer again.
  Escape, typing in the editor, a click outside and scrolling the editor still close it at once.
  Ctrl+Alt+Q pins it, and a click inside activates it so the keyboard can scroll it.
  `CodeEditor` owns a single-shot dwell timer whose interval is the user's delay.
- **Bulb.**
  `bulb_kind` decides red (any quick fix) or yellow (refactorings only), and the gutter paints it from the theme's semantic error and warning colours.
- **Settings.**
  `HoverSettings` (`[hover]` in the global settings) holds `docs_on_hover`, `problems_on_hover` and `delay_ms`, clamped to 100 to 3000 ms on read.
  `HoverSettings::scope` decides what a request fetches; an explicit Ctrl+Alt+Q request ignores the switches.
  The Editor settings page edits them live, and the card's ⋮ menu toggles `docs_on_hover`.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| Compose the card in C++ from structured data | It would put ordering, escaping and primary-fix rules in the view, which cannot be unit tested. |
| Keep HTML composition in the bridge | The bridge is translation only; the rule had already grown a second caller. |
| One `QTextBrowser` per section or a custom widget tree | The pointer, scroll and selection behaviour of one document is what the user expects, and tables and classes already cover the layout. |
| Load the fixes before showing the card | A slow server would delay every hover; showing at once with a loading line keeps the card responsive. |
| Keep fixes in the view and apply by title | Titles are not unique, and it would let a stale card apply a fix. Indices into a token-bound list cannot. |
| Close on the first mouse move but keep Ctrl+Q pinning | The card is unusable with the mouse, which was the complaint. |
| Translate inside Rust | The Qt translation catalogues live in the view. |

## Consequences

- Positive: what the card contains, its order, its escaping and its primary fix are unit tested in a Qt-free crate, and the C++ stays a view.
- Positive: a fix can only be applied from the card that offered it, and only while it is showing.
- Positive: colours, delay and content are user settings, and the card follows the theme without a restart.
- Negative: the popup depends on the Qt rich-text subset, so tables and a per-class stylesheet stand in for layout that CSS would do better.
- Negative: fixes are fetched per problem, so a position with many diagnostics makes several `codeAction` requests.
- Negative: the declaration footer comes from the project index and can be absent or slower than the LSP part of the card.

## Related

- [ADR-0021: AI chat](0021-ai-chat.md), for the untrusted-text rule.
- [ADR-0003: FFI conventions](0003-ffi-conventions.md), for errors and identifiers across the bridge.
- [ADR-0049: UI internationalization](0049-ui-internationalization.md), for `tr()` on every user-visible string.
