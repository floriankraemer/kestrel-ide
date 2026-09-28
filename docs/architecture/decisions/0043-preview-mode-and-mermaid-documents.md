# 0043. View mode in the tab, and Mermaid as a document in its own right

## Status

Accepted.
Amends [ADR-0033](0033-markdown-preview.md), which built the preview as a dock; this adds a second surface for the same renderer and a second built-in provider for it, and changes nothing about how a document is rendered.

## Context

ADR-0033 shipped a Markdown preview with inline Mermaid diagrams, rendered by the built-in `markdown-preview` plugin into an ADS dock beside the editor.

Two things were missing, and both are about *what* is previewed rather than *how*.

A standalone diagram file is not Markdown.
`erd.mermaid` is one diagram from its first line to its last, no prose around it, and the `previews` contribution claimed only `md`/`markdown`/`mdown`/`mkd`, so such a file opened as unhighlighted plain text with no preview at all — while the very same diagram, pasted into a fence in a `.md` file, rendered.

And a dock is not a view mode.
The dock answers "show me this document beside its source", which is the right shape while writing. It does not answer "show me this file as a document", which is the right shape while reading — and reading is most of what anyone does with an ADR or a diagram.

## Decision

### 1. A second built-in `previews` contribution, not a second renderer

`markdown-preview`'s manifest gains a second block, `id = "mermaid"`, claiming `mermaid` and `mmd`.
`app_core::preview::PreviewService::render` gains one arm beside the existing `"markdown"` one, dispatching to `markdown_preview::Renderer::render_diagram`.

`render_diagram` treats the whole file as one diagram: the same content-hash key, the same rasteriser, the same per-width cache and the same fallback block the fence path already uses, and no comrak at all.
It deliberately does **not** wrap the file in a synthetic ```mermaid fence and re-enter the Markdown parser. Such a fence would have to out-run whatever run of backticks the diagram's own labels contain, and a document with no prose in it has nothing for a Markdown parser to do.

The extension list now lives in two crates that cannot depend on each other — `plugin-host`'s built-in manifest and `syntax-core`'s language catalog — which is the arrangement `markdown` already had, kept as a comment on both sides rather than a new dependency edge.

### 2. Mermaid is a registered language, which is why the toolchain moved

`syntax-core` gains a `mermaid` row backed by `tree-sitter-mermaid`, whose queries are ported from the crate's own with one rewrite: `@namespace` is not a scope in this engine's taxonomy, so those four patterns say `@module`.
Its `tags.scm` keeps only the patterns whose kinds map onto a real `SymbolKind`; upstream's module- and variable-kinded ones are dropped rather than painted as `Class`, the same call `markdown` made for headings.

That crate requires `rustc 1.95`, which is what [ADR-0042](0042-rust-toolchain-1-98.md) is about.

### 3. View mode is an overlay on the editor, not a new page and not a new `TabKind`

`view.togglePreviewMode` (`Ctrl+Shift+M`) flips the current tab between its source and a `MarkdownPreviewPanel` **parented to that tab's `CodeEditor`** and shown over it.

ADR-0033's rejection of a new `TabKind` stands and is not revisited. What this adds is the surface that decision left unbuilt, and it is built so that the tab's page widget never stops being the editor.

That is a correctness requirement, not a preference.
`forEachEditor`, `editorForPath`, `openPaths`, `saveAllModified` and `hasUnsavedChanges` all reach a page through `qobject_cast<QPlainTextEdit *>` and silently skip anything that is not one.
A tab whose page had been swapped for a preview — the shape the editable diff window (F3-14) uses, by reparenting the editor into a floating window — would be invisible to Save All and to the quit-time unsaved-changes prompt.
That is silent data loss, and it is the whole reason for the overlay.

The same rule forbids the obvious way to keep typing out of the buffer: the editor is **never** made read-only while the preview is up, because `EditorTabs::saveEditor` returns "nothing to save" for a read-only editor and would lose the file's edits by the other route. Focus is the only mechanism, and the E2E flow asserts it against the file on disk rather than against the absence of a marker.

The overlay's existence and visibility *is* the mode state. There is no per-tab map, so a tab close, a drag between panes and a split need no bookkeeping at all — Qt's parenting frees the overlay with the editor it belongs to.

Mode is per-session. It is not written into the saved editor layout: restoring it would add an ordering dependency to `restoreLayout`, whose failure mode is "your tabs are gone", and a user who quit in view mode would reopen to a window that refuses to accept typing with no visible cause.

### 4. One tab, one renderer

`PreviewProvider` keys one render per tab id. While a tab renders itself in place, the dock is fed the "no tab" sentinel and stands down.

Without that, both surfaces would ask for the same tab at different widths: whichever request won the race would decide how wide the diagrams were rasterised, every settled keystroke would pay for two Mermaid layouts, and one revision would emit two `preview_ready` markers — which the existing dock E2E asserts on.

Both surfaces ride the one 300 ms content debounce that already existed, rather than a second timer.

## Consequences

- The renderer, the provider table and the FFI seam are unchanged. This ADR adds a manifest block, one dispatch arm, one language row and one C++ translation unit.
- `MarkdownPreviewPanel` is now instantiated more than once per window — once for the dock, once per tab that has entered view mode. The class was already self-contained per instance; the only change to it is a focus proxy onto its browser.
- `syncToEditorLine` and `nearestSourceLine` finally have a caller. They shipped with ADR-0033 and, despite the header claiming otherwise, nothing called them.
- The E2E flows live in a new `crates/app/tests/e2e_preview.rs` binary, because `e2e.rs` was at its ratcheted size ceiling — the same reason `e2e_run.rs` and `e2e_panes.rs` existed. (Later merged into one `app` E2E binary — see `crates/app/tests/e2e/preview.rs`.)

### Rejected alternatives

**Reuse the dock as the toggle** (show/hide, maximised).
Smallest possible change, and not a view mode: the editor stays on screen, so "show me this file as a document" is still unanswerable.

**A separate read-only preview tab beside the source tab.**
Needs the new `TabKind` ADR-0033 already rejected, plus a counterpart in every kind-blind loop, to deliver something the dock nearly does already.

**Swap the tab's page widget for the preview.**
The data-loss failure above. Rejected on that alone.

**Give the whole file to comrak as a fenced diagram.**
Cheaper to write, wrong on any diagram whose labels contain backticks, and it makes a Markdown parser responsible for a file that is not Markdown.
