# Mermaid documents and preview mode

A standalone `.mermaid`/`.mmd` file becomes a first-class file type — previewed and highlighted — and one shortcut flips any previewable tab between its source and its rendered form.

Architecture decisions: [ADR-0043](decisions/0043-preview-mode-and-mermaid-documents.md) (both features) and [ADR-0042](decisions/0042-rust-toolchain-1-98.md) (the toolchain bump the grammar required).
Builds on [ADR-0033](decisions/0033-markdown-preview.md), whose [plan](markdown-preview-plan.md) delivered the renderer, the `previews` contribution point and the dock this reuses unchanged.

## Why

The preview shipped by ADR-0033 renders Mermaid, but only inside Markdown.

A file that is nothing but a diagram — this repository grew one while this plan was being written — matched no `previews` contribution, so it opened as unhighlighted plain text with no preview, while the identical diagram inside a fence rendered.
And the preview was a dock only, which answers "show me this document beside its source" but never "show me this file as a document", which is what reading an ADR or a diagram actually wants.

## Scope decisions

See [ADR-0043](decisions/0043-preview-mode-and-mermaid-documents.md) for the reasoning behind each.

- A second built-in `previews` contribution (`mermaid`, claiming `mermaid`/`mmd`), one dispatch arm in `app_core::preview`, and `Renderer::render_diagram` — the whole file as one diagram, never wrapped in a synthetic fence.
- `syntax-core` gains a `mermaid` row over `tree-sitter-mermaid`, queries ported from the crate's own with `@namespace` rewritten to `@module` and unmappable tag kinds dropped rather than mislabelled.
- View mode is a `MarkdownPreviewPanel` parented to the tab's `CodeEditor` and shown over it. The page widget stays the editor, because `saveAllModified` and `hasUnsavedChanges` skip any page that is not one — the alternative loses data silently.
- The editor is never made read-only to keep keystrokes out; `saveEditor` treats a read-only editor as having nothing to save. Focus is the only mechanism, asserted against the file on disk.
- While a tab renders itself, the dock stands down: one render per tab id, one width, one `preview_ready` per revision.
- The toolchain moves to 1.98.1 in both Dockerfile stages, as its own commit, lint fallout included.

## Progress

Living status table — update the relevant row **in the same commit** that finishes a task, so status and code never drift apart.

| Task | Status | Commit |
|---|---|---|
| T1 — rustc 1.90.0 → 1.98.1 in both Dockerfile stages, workspace lint fallout, ADR-0042 | done | 022e638 |
| T2 — the `mermaid` previews contribution, `Renderer::render_diagram`, the `app_core::preview` arm | done | 9cc877d |
| T3 — `syntax-core`: the `mermaid` language row, the ported query set, the catalog fixture | done | 2d1bd18 |
| T4 — view mode: the action, `editor_tabs_preview.cpp`, the dock stand-down, the E2E flows | done | 745fe9c |
| T5 — docs: ADR-0043, this plan, the index and overview lines | done | this commit |

T3 also moved the language table out of `registry.rs` into `catalog.rs`: the new row crossed the file-size gate's ceiling, and a baseline raise would have left the next language row hitting the same wall.

T1 is landed first and alone: it invalidates every cached build in the shared image, and the sessions sharing that image were told before it ran.
T2 and T3 are independent of each other and both independent of T4, which needs only that *something* previews a Mermaid file to be worth toggling.

## Verification

```sh
make test
make lint
make e2e                                                # crates/app/tests/e2e/preview.rs
cargo tree -p markdown-preview -e normal | grep -i qt    # must be empty
cargo tree -p app-core         -e normal | grep -i qt    # must be empty
cargo tree -p markdown-preview -e normal | grep -iE "memmap|fontconfig"   # must be empty
```

`e2e_preview.rs` carries two flows.
`e2e_preview_mode_toggle` opens a Markdown file, enters view mode, types a character that must *not* survive, leaves view mode, types one that must, saves, and reads the file back off disk — a positive assertion about content rather than an absence-of-marker check, which would pass equally against an app that ignored every keystroke.
`e2e_standalone_mermaid_file_previews` does the same round trip against a `.mermaid` fixture, which is what proves the second provider is reachable from the UI and not only from a unit test.

## Risks, and how each was retired

| Risk | Retired by |
|---|---|
| A tab in view mode escapes Save All / the quit-time dirty prompt | The page widget is never swapped; the overlay is a child of the editor. Asserted by the E2E flow reading the saved file. |
| Keystrokes leak into the buffer behind the preview | Focus, never read-only — read-only would make `saveEditor` a no-op. Same E2E assertion, from the other side. |
| Dock and in-tab view fight over one tab's render | The dock is fed the "no tab" sentinel while view mode is on; both surfaces share the one existing debounce. |
| A toolchain bump breaks other sessions' in-flight work | Announced to both active sessions before the image was rebuilt; both confirmed nothing was in flight. |
| `main_window.cpp` crosses its 1200-line ceiling | The menu wiring is a free function in `markdown_preview_panel.cpp`, the split `wireProjectTreeViewAction` already uses. `scripts/check-file-size.sh` is the gate. |
| A ported query paints nothing because a capture resolves to no scope | `every_highlights_capture_resolves_to_a_scope` and `every_language_highlights_its_sample` in `syntax-core`'s catalog tests, which is what caught `@namespace`. |
