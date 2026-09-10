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

`app_core::preview` dispatches that provider to `carve-lang::to_html`.
The renderer's default rejects raw HTML and unsafe URL behavior, so its output is suitable for Kestrel's non-navigating `QTextBrowser` preview.
Unlike `markdown-preview`, Carve needs no stateful diagram cache, image rasterizer, theme integration, or link resolver; adding a new crate whose only API forwarded one function would add indirection without isolating any policy.
The direct `app-core` dependency is therefore deliberate, and this ADR records the widening required by the repository's layering rule.

Carve heading links render normally, but editor-to-preview scroll sync has no source-line map because the convenience renderer exposes no positions.
Kestrel leaves `anchors` empty instead of maintaining an approximate second heading parser; adding position-aware rendering upstream is the clean upgrade path.

## Consequences

- `.crv` and `.carve` files receive incremental syntax highlighting, folding, fenced-language injection, reference locals, preview, and the standard LSP surface when `carve-lsp` is installed.
- Disabling the built-in plugin removes its preview and language-server offers, while syntax recognition remains part of the compiled language catalog, the same separation used by C#.
- `syntax-core` gains `tree-sitter-carve`, and `app-core` gains `carve-lang`; both dependencies are Qt-free and preserve the hard layering rule.
- Carve preview currently has no source-line scroll synchronization.
