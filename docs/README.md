# Docs index

## Architecture

- [Architecture overview](architecture/overview.md) — arc42-lite orientation: context, quality goals, building-block view, crate table, communication, build, future scope.
- [Layering rules](architecture/layering.md) — binding dependency table, logic-placement rules, FFI seam rules, verification commands.
- [Project structure](architecture/project-structure.md) — repository layout, layer summary, where tests live, dev workflow pointers.

### Decisions

ADR numbers 0006 and 0013–0015 were never used; the gaps are historical and intentional.

- [ADR-0001: core tech stack](architecture/decisions/0001-core-tech-stack.md) — Rust core + Qt6 UI via cxx-qt; hybrid plugin system direction.
- [ADR-0002: application layer and humble view](architecture/decisions/0002-application-layer-and-humble-view.md) — `app-core` application layer; the Qt view is humble and holds zero rules.
- [ADR-0003: FFI conventions](architecture/decisions/0003-ffi-conventions.md) — FFI seam: typed errors, stable `TabId`, Rust-owned dirty state.
- [ADR-0004: MCP transport](architecture/decisions/0004-mcp-transport.md) — hand-rolled axum+tokio transport over the `rmcp` SDK.
- [ADR-0005: ADS build integration](architecture/decisions/0005-ads-build-integration.md) — docking via vendored Qt Advanced Docking System built through `cxx-qt-build`.
- [ADR-0007: embedded terminal](architecture/decisions/0007-embedded-terminal.md) — `portable-pty` + `alacritty_terminal` + custom QPainter grid widget.
- [ADR-0008: project index](architecture/decisions/0008-project-index.md) — hybrid tantivy(ngram) + ripgrep-crates text index; name-based symbol schema.
- [ADR-0009: find and replace](architecture/decisions/0009-find-and-replace.md) — matching in `editor-core`; project-wide replace through `index-core`.
- [ADR-0010: search everywhere](architecture/decisions/0010-search-everywhere.md) — one popup over ranked tiers, persistent index, batched results.
- [ADR-0011: code navigation](architecture/decisions/0011-code-navigation.md) — local-file-first declaration resolution; supertype edges as third index schema.
- [ADR-0012: MCP protocol, index and lifecycle](architecture/decisions/0012-mcp-protocol-index-and-lifecycle.md) — real MCP protocol surface, index tools, user-controlled lifecycle; index shared as `Arc<RwLock<IndexSlot>>`.
- [ADR-0016: LSP client](architecture/decisions/0016-lsp-client.md) — Qt-free `lsp-core`: blocking threads, supervised child servers, catalog + user-override config.
- [ADR-0017: settings-model crate](architecture/decisions/0017-settings-model-crate.md) — `settings-model`: Qt-free home for the settings pages' rules.
- [ADR-0018: single-source language detection](architecture/decisions/0018-single-source-language-detection.md) — one source of truth for file→language detection: `syntax-core`'s registry.
- [ADR-0019: LSP refactoring](architecture/decisions/0019-lsp-refactoring.md) — refactoring over LSP: code actions, rename, applying workspace edits.
- [ADR-0020: tab kinds and the binary viewer](architecture/decisions/0020-tab-kinds-and-the-binary-viewer.md) — a tab has an explicit kind; binary files open a read-only hex view instead of erroring.
- [ADR-0021: AI chat](architecture/decisions/0021-ai-chat.md) — a docked assistant with four providers, environment-only keys, and a policy-gated agent whose edits go through the refactoring path.
- [ADR-0022: per-project settings](architecture/decisions/0022-per-project-settings.md) — a sparse `.ide/settings.toml` layered over the global file; precedence resolved by `settings-model`.
- [ADR-0023: multi-caret](architecture/decisions/0023-multi-caret.md) — `SelectionSet`/`Transaction` in `editor-core`, spliced through the existing `applyBufferEdits` seam; no second undo stack.
- [ADR-0024: verification foundation](architecture/decisions/0024-verification-foundation.md) — a headless E2E harness driving the real binary, and a pinned real-server LSP conformance gate that runs nightly.
- [ADR-0025: seam split and file-size ceiling](architecture/decisions/0025-seam-split-and-file-size-ceiling.md) — `bridge.rs` and `main_window.cpp` split per feature; a ratcheted size gate; the split proven by byte-identical FFI headers.
- [ADR-0026: plugin host](architecture/decisions/0026-plugin-host.md) — `plugin-api` as the contract and `plugin-host` as the machinery; declarative contributions, built-ins loaded as plugins, `api_version` the one compatibility lever.
- [ADR-0027: icon themes](architecture/decisions/0027-icon-themes.md) — a Qt-free `icon-theme` crate: our own `pack.toml`, `resvg` rasterisation to premultiplied RGBA8, and a resolver handed a language id rather than detecting one.
- [ADR-0028: wasm plugin tier](architecture/decisions/0028-wasm-plugin-tier.md) — wasmtime components under fuel, an epoch deadline and a memory cap; capabilities that deny rather than omit; a trap disables one plugin and never the process.
- [ADR-0029: resource operations](architecture/decisions/0029-resource-operations.md) — a `WorkspaceEdit`'s file create/rename/delete steps, parsed by `lsp-core` and performed by `app-core` as `FileOp`, all-or-nothing before any text edit is written.
- [ADR-0030: `DiffView`](architecture/decisions/0030-diff-view.md) — one Git-free diff component over `editor_core::diff`; the refactor preview and Replace in Files retrofit onto it; `TabKind::Diff` deferred until the Git backend exists.
- [ADR-0031: Git backend](architecture/decisions/0031-git-backend.md) — `gix` for reads (discovery, status, HEAD, history), the `git` binary for anything touching credentials, hooks or signing (staging, commit, branch, remote); hunks computed in-process, blame shelled out.
- [ADR-0032: run configurations](architecture/decisions/0032-run-configurations.md) — a PTY-backed console over the debugger-agnostic `LaunchSpec`; ANSI-stripped output in v1; one `TerminalSupervisorRust` QObject for N terminal sessions, not N QObject instances; the AI agent gains no run tool from this.
- [ADR-0033: Markdown and Mermaid preview](architecture/decisions/0033-markdown-preview.md) — the `previews` contribution point (no `api_version` bump), a second wasm world (`preview-plugin`) for a sandboxed renderer, comrak + merman + resvg native rendering joined in `app_core::preview`, an ADS dock rather than a new `TabKind`.
- [ADR-0042: Rust 1.98](architecture/decisions/0042-rust-toolchain-1-98.md) — the builder image's pinned toolchain moves to current stable in both stages at once, because a first-class dependency required it; `merman` stays pinned on its own merits.
- [ADR-0043: preview mode and Mermaid documents](architecture/decisions/0043-preview-mode-and-mermaid-documents.md) — a second built-in `previews` contribution for standalone `.mermaid`/`.mmd` files, and an in-tab view mode built as an overlay on the editor so the tab's page never stops being a `QPlainTextEdit`.
- [ADR-0034: model selection](architecture/decisions/0034-model-selection.md) — the model catalogue is fetched from the provider and never compiled in; the picker stays typeable; the chosen model belongs to the conversation, not the provider row.
- [ADR-0035: semantic-tokens overlay](architecture/decisions/0035-semantic-tokens-overlay.md) — `lsp-core` takes a normal (not dev-only) dependency on `syntax-core` so C9's semantic-token mapping and overlay can reuse `Scope::resolve` and `HighlightSpan`, amending ADR-0018 without reopening the language-detection duplication it fixed.
- [ADR-0036: read-only virtual documents](architecture/decisions/0036-virtual-documents.md) — `editor_core::DocumentSource::{File, Virtual}` for a document with no backing file (C12's decompiled `csharp:/` metadata), the systematic audit of "every tab is a file" call sites it amends ADR-0003 to close, and the clean-refusal guard that replaces a confusing generic I/O error.
- [ADR-0037: async project open](architecture/decisions/0037-async-project-open.md) — `openFolder`/`reopenLastProject` become fire-and-forget, walking the directory tree on a worker thread and reporting through `projectOpened`/a new `projectOpenFailed` signal instead of blocking the Qt thread; the filesystem watcher's structural rebuild gets the same treatment.
- [ADR-0038: stay on Widgets for the blend chrome](architecture/decisions/0038-stay-on-widgets-for-the-blend-chrome.md) — re-evaluates Qt Quick / QML for the blend design and keeps the chrome on Qt Widgets: one `ChromePalette` per theme feeding generated stylesheets, `ui_tokens.h` for shape, an overlay-painted panel card instead of a mask, Inter bundled as the interface font.
- [ADR-0039: typed run configurations](architecture/decisions/0039-typed-run-configurations.md) — one toolchain table in `run-core` feeding run, build and debug alike; toolchain and target persisted as strings so `app-config` keeps depending on nothing; macros expanded in arguments too, and run-from-context's capped temporary configurations.
- [ADR-0040: `build-core`](architecture/decisions/0040-build-core.md) — build is delegated to the project's own tool and never modelled by the IDE: no output folders, artifacts or auto-build; Cargo parsed from JSON and everything else from a small pattern table; build diagnostics join the existing Problems dock rather than getting a second one.
- [ADR-0041: `dap-core`](architecture/decisions/0041-dap-core.md) — a Debug Adapter Protocol client shaped like the LSP one, sharing its `Content-Length` framing through `stdio-framing`; the protocol typed only where it is read; an undeclared capability is unsupported; adapters are installed, not bundled, so every catalog entry carries an install hint.

## Plans

All plan documents are complete except the plugin-host-and-icon-themes plan and the run-build-debug parity plan, which are the two currently being delivered; the rest remain as historical records of how each feature phase was delivered.
(An earlier version of this line called the index-performance and large-files plans incomplete. Both of their Progress tables are fully `done`; the claim was stale.)

- [MVP implementation plan](architecture/mvp-implementation-plan.md) — MVP editor shell; marked historical.
- [Settings, docking, theming, MCP plan](architecture/settings-docking-theming-mcp-plan.md) — settings, docking, theming, MCP foundation, line numbers, tab reorder, syntax foundation.
- [Language, folding, Class View, terminal, search plan](architecture/language-folding-classview-terminal-search-plan.md) — language expansion, folding, Class View, terminal, project index and search.
- [Terminal shells plan](architecture/terminal-shells-plan.md) — a shell catalogue in `pty-core`, a project-scoped `[terminal]` section, the "+" dropdown and the project root as the start directory.
- [Find & Replace plan](architecture/find-replace-plan.md) — find and replace, in-editor and project-wide.
- [Search Everywhere plan](architecture/search-everywhere-plan.md) — Search Everywhere popup and Search Results dock.
- [Code navigation plan](architecture/code-navigation-plan.md) — Go to Declaration, Find Usages, Go to Implementation, jump history.
- [Language platform plan](architecture/language-platform-plan.md) — extensible tree-sitter languages, per-language theming, runtime grammars, LSP.
- [Refactoring plan](architecture/refactoring-plan.md) — rename, extract via code actions, signature on hover.
- [Index performance plan](architecture/index-performance-plan.md) — faster project index build and a status-bar indexing indicator.
- [Large files and the binary viewer plan](architecture/large-files-and-binary-viewer-plan.md) — no-wrap default, highlighting size ceilings, O(1) fold lookup, read-only hex view for binary files.
- [Next five features plan](architecture/next-five-features-plan.md) — the current roadmap: verification foundation, editor ergonomics, Alt+Enter intentions, Git v1, run configurations. Carries the living Progress table.
- [Plugin host and icon themes plan](architecture/plugin-host-and-icon-themes-plan.md) — a two-tier plugin host and the Material icon themes built on it; in delivery, carries its own Progress table.
- [Run, build and debug parity plan](architecture/run-build-debug-parity-plan.md) — IntelliJ-parity roadmap for running, compiling and debugging: typed run configurations, a delegated build with parsed diagnostics, and a DAP debugger; in delivery, carries its own Progress table.
- [Markdown and Mermaid preview plan](architecture/markdown-preview-plan.md) — the plugin host's third contribution point and its first content-returning wasm export; comrak + merman + resvg rendering.
- [Mermaid documents and preview mode plan](architecture/mermaid-documents-and-preview-mode-plan.md) — standalone Mermaid files as a previewed, highlighted file type, and the in-tab edit/view toggle; carries its own Progress table.

- [LSP conformance](architecture/lsp-conformance.md) — checking the LSP client against a real rust-analyzer; the executable expectations file and why it is not a per-PR gate.

## Design

- [Language platform UI](design/language-platform-ui.md) — UX spec for the three language-platform settings pages and the Problems dock.

## Product

- [MVP proposal](product/mvp-proposal.md) — original MVP product proposal; draft, superseded by shipped scope.
