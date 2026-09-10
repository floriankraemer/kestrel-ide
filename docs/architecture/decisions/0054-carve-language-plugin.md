# 0054. Carve language support spans the registry and the plugin contribution seams

## Status

Accepted and implemented.

## Context

Carve documents use `.crv` or `.carve` and have first-party Tree-sitter, language-server, and Rust rendering implementations.
Kestrel already has one authoritative file-to-language registry in `syntax-core`, while optional language servers and previews are declarative plugin contributions joined to their consumers in `app-core`.
Treating every part as plugin-manifest data would create a second extension table for syntax detection and violate ADR-0018; hardcoding every part would bypass the plugin lifecycle for features users may disable.

## Decision

Carve syntax is a bundled `syntax-core` language backed by `tree-sitter-carve`.
The catalog owns `.crv` and `.carve` detection, comment and pairing metadata, highlighting, language injection, locals, and folds exactly as it does for every other compiled grammar.
The upstream highlight query is vendored with editor-only paint-suppression, conceal, and spell-exclusion directives removed because they are not Kestrel colour scopes; the grammar and injection query remain pinned through the crate dependency.

A built-in `carve` plugin contributes the `carve-lsp` process definition and a preview provider for both extensions.
The executable is discovered on `PATH` and is not bundled, matching every other language-server contribution.

`carve-lang` is a rendering engine, so it is a dependency of `markdown-preview` — next to comrak and merman — behind `Renderer::render_carve`, and `app_core::preview` only dispatches to it, exactly as it already dispatches Markdown and standalone Mermaid.
The renderer's default rejects raw HTML and unsafe URL behavior, so its output is suitable for Kestrel's non-navigating `QTextBrowser` preview.
Carve needs none of `markdown-preview`'s stateful machinery — no diagram cache, image rasterizer, theme integration, or link resolver — but that argues against a *new* crate, not for pulling a rendering engine up into the application layer: `app-core` decides which provider serves a document, and never how HTML is produced.

Carve heading links render normally, but editor-to-preview scroll sync has no source-line map because the convenience renderer exposes no positions.
Kestrel leaves `anchors` empty instead of maintaining an approximate second heading parser; adding position-aware rendering upstream is the clean upgrade path.

## Consequences

- `.crv` and `.carve` files receive incremental syntax highlighting, folding, fenced-language injection, reference locals, preview, and the standard LSP surface when `carve-lsp` is installed.
- Disabling the built-in plugin removes its preview and language-server offers, while syntax recognition remains part of the compiled language catalog, the same separation used by C#.
- `syntax-core` gains `tree-sitter-carve`, and `markdown-preview` gains `carve-lang`; both dependencies are Qt-free and preserve the hard layering rule, and `app-core`'s dependency row is unchanged.
- Carve preview currently has no source-line scroll synchronization.
