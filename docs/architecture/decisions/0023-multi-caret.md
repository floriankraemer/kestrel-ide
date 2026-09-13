# 0023. Multi-caret: a selection set and edit transactions in `editor-core`, and no second undo stack

## Status

Accepted

## Context

The editor has one caret.
There is no `Ctrl+D`, no column selection, no way to make the same edit at several places in a file at once — a gap the plan behind this ADR called out as one of the three things separating this editor from an IDE.

Multi-caret is not one feature; it is one rule applied everywhere a keystroke touches the buffer.
Get the rule wrong and every editing feature built afterward — comment toggle, line operations, auto-close — inherits the mistake.
So the rule needed settling once, before F1's other tasks, rather than once per feature.

Two questions had to be answered together: where do carets live, and what does one keystroke at N carets mean for undo.

## Decision

### 1. `SelectionSet` and `Transaction` live in `editor-core`, over byte offsets

A `SelectionSet` is one or more `Caret`s, always sorted, non-overlapping and holding a primary.
Every multi-caret operation — including a single keystroke — is computed as one `Transaction`: a set of `TextEdit`s applied descending and all-or-nothing, so a transaction that cannot be applied changes nothing at all.

Offsets are bytes throughout `editor-core` and `edit-ops`, matching the rope's own indexing and tree-sitter's byte-addressed nodes.
Byte ↔ UTF-16 conversion happens at exactly one place, `editor_core::offsets` (`Utf16Cursor`, promoted from a private helper in `search.rs` for this purpose), and nowhere else — five reimplementations was the alternative.

### 2. The transaction crosses the seam as `Vec<FfiTextEdit>` and is spliced inside one `beginEditBlock`

This is not new machinery: `EditorTabs::applyBufferEdits` already splices a refactoring's edits this way.
`EditorOps` (the bridge QObject this ADR's work introduces) reuses it for every keystroke, which is what makes a 200-caret edit one `Ctrl+Z` — the rule that constitutes "one user-visible change" lives in `editor-core`, unit-tested there, and C++ contributes one wiring fact: the edit list is spliced inside one edit block.

### 3. Undo stays `QTextDocument`'s

A second undo stack in `editor-core`, mirroring Qt's, was considered and rejected.
Two stacks that must agree forever is a worse trade than the wiring fact above, and the existing refactoring path already proves the fact holds.

### 4. Carets are computed against the live buffer text, never the rope

`editor_core::Document`'s rope is populated on open and refreshed only on save (`replace_content`'s own doc comment says so) — it is one save behind the widget at all times.
So `EditorOps`, `edit-ops` and every new `editor-core` entry point take `text: &str`, the same stateless shape `find_matches` and `replacements_for` already use, rather than reading `Document`.

### 5. Caret state lives in the adapter, keyed by `TabId`, not in `Document` or `AppSession`

`Document` is stale (see above); `AppSession` has no reason to know about carets at all.
`EditorOps` keeps one entry per open tab — the selection set, the expand/shrink history, the auto-close pair tracker — because it is the one object that reads and writes it on every gesture, and the entry is dropped when the tab closes.

### 6. The primary caret stays the widget's own `QTextCursor`

`CodeEditor` does not represent every caret as a list it owns and paints from scratch.
The primary caret and its selection ride on the real `QTextCursor`, so scrolling, Find, the status bar and the completion popup keep working exactly as they did with one caret.
Only the *other* carets are a view-local list (`SecondaryCaret`), painted as solid bars and extra selections alongside the diagnostic and match highlights already there.
They do not blink — one timer driving two blink phases is how a secondary caret ends up invisible while the primary is lit, and a caret you cannot see is worse than one that sits still.

### 7. A ceiling, stated rather than discovered

`SelectionSet` refuses past `MAX_CARETS`, returning a typed refusal (ADR-0003) rather than silently truncating or degrading.
Originally, only printable keys, backspace, delete and newline routed through the multi-caret path; arrows, Home, End and other navigation dropped the secondary carets and did exactly what they always did — moving N carets together was left as its own rule, to be built in `editor_core::selection` rather than improvised in the view.
R1 (`docs/architecture/intellij-parity-refinement-plan.md`) built that rule: `SelectionSet::move_carets` takes a `CaretMotion` and moves every caret at once, and `CodeEditor::keyPressEvent` routes Left/Right/Up/Down/Home/End and the Ctrl+word-move combos through it whenever more than one caret is active.
Every *other* key still drops the secondary carets and falls through to the single-cursor default — Tab/Shift+Tab go through `edit_ops::indent` instead (their own per-caret transaction, not a caret motion), and a shortcut, a fold toggle or anything not in `CaretMotion` still collapses to the primary caret, which remains the stated ceiling for everything this ADR did not name above.

## Consequences

- `edit-ops` (comment toggle, expand/shrink, indent, pairs, brackets) is multi-caret-aware by construction: every operation takes a `SelectionSet` and returns a `Transaction`, so a feature built on top never has to special-case "more than one caret."
- A bare modifier key press (Shift held ahead of a Shift+digit combo, Control held ahead of a chord's second key) must not be treated as "some other operation" by the view — it carries no meaning of its own, and treating it as one drops the multi-caret selection before the character it is part of ever arrives. This surfaced as a real bug during the E2E flow for this ADR (`e2e_multi_caret_edit_is_one_undo`) and is why `CodeEditor::keyPressEvent` special-cases `Qt::Key_Shift`/`Control`/`Alt`/`AltGr`/`Meta` ahead of its general fallback.
- Save rules (F1-11) reuse the same seam: `EditorOps::saveRuleEdits` computes `editor_core::save_rules::on_save` as a `Transaction` and splices it through the identical `applyEditsTo` path, so a save's tidying is one undo entry for the same reason a keystroke is.
- The E2E suite gained one flow (`e2e_multi_caret_edit_is_one_undo`) proving the ADR's central claim end to end: two carets, one keystroke, one `Ctrl+Z`.

## Alternatives rejected

**A Rust-side undo stack.** Two stacks that must agree forever, against a wiring fact (`applyBufferEdits`'s single `beginEditBlock`) that already gives refactorings their one-`Ctrl+Z` property.

**Carets owned by the view as a list of `QTextCursor`s.** Ctrl+D's next-occurrence search and column selection are rules — what counts as "the next occurrence," whether a column selection pads or clips a ragged line — and rules do not belong in `cpp/`.

**Per-caret sequential edits, applied one caret at a time.** Offsets shift under each other as earlier edits land, and undo becomes N steps instead of one. The transaction *is* the feature.

**A byte-addressed seam.** The rope is char-indexed internally, but the FFI seam already speaks UTF-16 (`FfiTextEdit`'s line/character pairs, `editor_core::search::TextMatch`) — an earlier draft of this decision claimed the seam should be bytes and was wrong about it.
