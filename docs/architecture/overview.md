# Architecture overview

Arc42-lite overview of the current system.
The binding rules live in [layering.md](layering.md) and the ADRs; this page orients a new contributor.

## 1. Context

The project is a cross-platform IDE with a JetBrains-like layout.
It is a Rust Cargo workspace with a Qt6 Widgets UI, bridged via `cxx-qt` per [ADR-0001](decisions/0001-core-tech-stack.md).

It has grown past the original MVP (open a folder, browse a tree, edit and save tabs — see the [MVP proposal](../product/mvp-proposal.md)).
Shipped since: a settings window and keymap, ADS-based docking, theming, tree-sitter syntax highlighting and folding, a Class View outline, an embedded terminal, a project-wide text and symbol index, find and replace, code navigation (Go to Declaration, Find Usages, Go to Implementation, jump history), a language platform with 30 bundled tree-sitter grammar crates covering roughly 36 languages (see `crates/syntax-core/Cargo.toml`) and an LSP client, refactoring (rename, Extract Method/Class through code actions), editor ergonomics — multi-caret and column selection, comment toggle, line operations (duplicate/move/delete/join), expand/shrink selection, auto-close/type-over/smart backspace, bracket match, reformat through the LSP client, and editing settings a project can override ([ADR-0023](decisions/0023-multi-caret.md)), with the settings dialog gaining a global/project scope selector, per-area origin badges, a `File > Project Settings...` entry, and project-scoped index excludes and language servers, all resolved by `settings_model::scope` ([ADR-0022](decisions/0022-per-project-settings.md)), and the rest of the LSP surface — Alt+Enter's grouped intentions (quick fixes and refactorings merged from the diagnostic- and range-scoped `codeAction` requests), organize imports, signature help, occurrence highlighting, inlay hints, and file-creating/renaming/deleting quick fixes applied through the same refactor preview a rename uses ([ADR-0029](decisions/0029-resource-operations.md)), and a Git-free `DiffView` component now backing that refactor preview's per-file diff and a real before/after preview for project-wide Replace in Files ([ADR-0030](decisions/0030-diff-view.md)), and Git v1 ([ADR-0031](decisions/0031-git-backend.md)) — gutter change markers against `HEAD` with a click-through popup to revert a hunk (spliced into the open buffer, one Ctrl+Z), show its diff, or stage the whole file, a Changes dock (staged/unstaged/untracked trees, per-file stage/unstage, commit/commit-and-push/amend), a status-bar branch widget and menu (checkout, create, delete with an unmerged-branch confirmation), a File History dock, an off-by-default blame gutter, and a VCS menu with fetch/pull/push, show-diff, hunk rollback and next/previous-change navigation, hidden for a project that is not a Git repository, and F4, run configurations and console ([ADR-0032](decisions/0032-run-configurations.md)) — a PTY-backed Run menu/toolbar/console dock with per-configuration tabs of ANSI-stripped output, Cargo/npm/pnpm/yarn/Makefile detection, a Run Configurations dialog persisted per project, Ctrl+Click file:line jumping from console output, and, riding the same PTY transport, a terminal dock that now holds N independent sessions as tabs instead of one, each opening in the project root and picking its shell — WSL distros, PowerShell and cmd on Windows, `$SHELL` and `/etc/shells` elsewhere — from the "+" button's dropdown or a project-scoped Settings > Terminal page ([ADR-0007](decisions/0007-embedded-terminal.md)). A Markdown and Mermaid preview dock ([ADR-0033](decisions/0033-markdown-preview.md)), the plugin host's third contribution point and its first content-returning sandboxed export — comrak/merman/resvg rendering off the Qt thread, scroll sync and click-to-jump, link resolution that never opens an external scheme, extended since by a standalone Mermaid file type — previewed and tree-sitter highlighted — and an in-tab edit/view toggle built as an overlay on the editor ([ADR-0043](decisions/0043-preview-mode-and-mermaid-documents.md)).
In progress: an in-IDE AI assistant — a docked chat panel with attachable context, four configurable providers, an Ask mode whose code blocks apply through the refactoring preview, and a policy-gated Agent mode that drives the editor and the index through the same tools the MCP server exposes ([ADR-0021](decisions/0021-ai-chat.md)).

## 2. Quality goals

1. **Testability without a display.**
   All business logic lives in Qt-free crates and runs under plain `cargo test`, with no Qt runtime or display server.
2. **View swappability.**
   The view is Qt Widgets today; QML is the planned future view (re-evaluated and deferred for the blend chrome in ADR-0038).
   Because the view holds zero rules ([ADR-0002](decisions/0002-application-layer-and-humble-view.md)), swapping it must not touch `app-core` or the domain crates.
3. **Performance** (from ADR-0001): typing latency and large-file handling drive the Rust-core decision.

## 3. Building-block view

Four layers plus the binary crate, per [layering.md](layering.md).
Only `ui-shell` and `app` touch Qt; every other crate is Qt-free and unit-tested with no display.

```mermaid
graph TB
    app["app (main)"] --> view
    subgraph uishell["ui-shell"]
        view["view: cpp/*.cpp<br/>widgets, layout, wiring"] --> adapter["adapter: src/bridge/*.rs<br/>thin QObject translation"]
    end
    adapter --> appcore["application: app-core<br/>AppSession, commands, AppError"]
    adapter --> support["support: app-config, syntax-core,<br/>index-core, lsp-core, settings-model,<br/>vcs-core, pty-core, terminal-core, run-core,<br/>mcp-server, ai-chat-core"]
    appcore --> editorcore["domain: editor-core"]
    appcore --> projectmodel["domain: project-model"]
    support --> editorcore
```

| Crate | Layer | Responsibility | Qt |
|-------|-------|----------------|----|
| `editor-core` | domain | Rope-backed `Document`, tab list, load/save/dirty state, find/replace matching | No |
| `project-model` | domain | `ProjectSession`, directory tree, `notify` watcher, last-project persistence | No |
| `app-core` | application | `AppSession`: orchestration, command methods, typed `AppError`, jump history; the icon-theme join ([ADR-0027](decisions/0027-icon-themes.md)) and the previews join ([ADR-0033](decisions/0033-markdown-preview.md)), each handing a row's language id or extension to the crate that renders it | No |
| `app-config` | support | `settings.toml` load/save, theme, editor font/colors, keymap | No |
| `syntax-core` | support | tree-sitter parsing: highlighting, folding, outline, occurrences, supertype edges | No |
| `index-core` | support | Project index: text search (tantivy + ripgrep crates) and symbols/references, plus declaration resolution ([ADR-0011](decisions/0011-code-navigation.md)) | No |
| `lsp-core` | support | LSP client: framing, supervised server processes, diagnostics, hover, navigation, completion, server catalog ([ADR-0016](decisions/0016-lsp-client.md)); code actions, rename, workspace edits and resource operations ([ADR-0019](decisions/0019-lsp-refactoring.md), [ADR-0029](decisions/0029-resource-operations.md)); intentions, organize imports, signature help, document highlights, inlay hints | No |
| `settings-model` | support | The settings pages' rules: syntax-colour draft and override origin, language load errors as sentences, language-server draft ([ADR-0017](decisions/0017-settings-model-crate.md)), and the Plugins page's rows including what a load error or a sandbox trap means in English | No |
| `edit-ops` | support | Language-aware editing operations that need the text *and* the grammar at once: comment toggle, expand/shrink selection over the tree-sitter node tree, indentation, auto-close/type-over/smart backspace, bracket match ([ADR-0023](decisions/0023-multi-caret.md)) | No |
| `plugin-api` | support | The plugin contract: `plugin.toml`, contribution points, typed load errors, the WebAssembly component world ([ADR-0026](decisions/0026-plugin-host.md)) | No |
| `plugin-host` | support | Which plugins exist and which may run: the `<config_dir>/plugins` scan, the built-ins embedded in the binary, the disabled list, quarantine and duplicate-id resolution ([ADR-0026](decisions/0026-plugin-host.md)); the sandboxed wasmtime tier with fuel, epoch and memory limits ([ADR-0028](decisions/0028-wasm-plugin-tier.md)) | No |
| `icon-theme` | support | Icon packs: the resolution order from a file name to an icon id, light/dark art, and `resvg` rasterisation to premultiplied RGBA8 ([ADR-0027](decisions/0027-icon-themes.md)) | No |
| `markdown-preview` | support | Markdown → the HTML subset `QTextDocument` understands (comrak, `render.r#unsafe = false`), Mermaid fences → SVG → premultiplied RGBA8 (merman, resvg, a bundled font), a diagram cache, link classification ([ADR-0033](decisions/0033-markdown-preview.md)) | No |
| `vcs-core` | support | Git v1: `gix` for pure reads (discovery, HEAD, status, branches, log), the `git` binary for anything touching credentials/hooks/config (staging, commit, branch create/checkout/delete, fetch/pull/push), gutter hunks and blame ([ADR-0031](decisions/0031-git-backend.md)) | No |
| `pty-core` | support | Cross-platform PTY transport, plus the catalogue of shells a machine offers | No |
| `terminal-core` | support | VT100/grid state over `alacritty_terminal` | No |
| `run-core` | support | Run configurations: the `LaunchSpec` seam, project-scoped detection (Cargo/npm/pnpm/yarn/Makefile), the PTY-backed supervisor, output batching, and the `file:line` link catalogue console output resolves through ([ADR-0032](decisions/0032-run-configurations.md)) | No |
| `mcp-server` | support | MCP server (protocol + transport) so an agent can read and drive the editor and query the project index ([ADR-0004](decisions/0004-mcp-transport.md), [ADR-0012](decisions/0012-mcp-protocol-index-and-lifecycle.md)) | No |
| `ai-chat-core` | support | AI assistant rules: provider dialects and capabilities, the conversation block model, token accounting, context assembly and its budget, the tool catalog and approval policy, the agent loop, conversation history, and turning a code block into an edit ([ADR-0021](decisions/0021-ai-chat.md)) | No |
| `ui-shell` | adapter + view | `src/bridge/*.rs`: cxx-qt QObject translation, one module per feature (ADR-0025); `cpp/`: Widgets, layout, menus, dialogs, `QApplication` | Yes |
| `app` | main | Thin binary; hands off to `ui-shell` | Yes |

The view never decides, it only displays and forwards intent.
Rules crossing the FFI seam (typed errors, `TabId`, Rust-owned dirty state) are fixed by [ADR-0003](decisions/0003-ffi-conventions.md).

## 4. Cross-boundary communication

- UI actions call invokable slots on the `cxx-qt` QObjects; the adapter translates and delegates to `AppSession`.
- Rust-side changes (dirty flags, watcher events) surface as Qt signals; tree data is exposed via a `QAbstractItemModel` backed by Rust data.
- Filesystem watcher events arrive on a background thread and are marshalled onto the Qt event loop via `CxxQtThread` queuing — no shared-mutex model.
- Long-running work (index build, search, symbol resolution, PTY reads) runs on a plain `std::thread` and streams results back through the same `CxxQtThread::queue()` hop, so the UI thread never blocks on it.
- The filesystem watcher also drives incremental re-indexing, which is what keeps navigation targets from drifting after an edit.

## 5. Build and deployment

`docker/Dockerfile` is a single multi-stage file: a `linux-builder` stage (apt Qt6) and a `windows-builder` stage cross-compiling with MXE's mingw-w64 + Qt6 toolchain.
The MXE toolchain is built by the `mxe-base` stage from a pinned upstream commit (`ARG MXE_COMMIT`), which compiles `qt6-qtbase` for `x86_64-w64-mingw32.shared` from source.
That first build takes hours and is then served from the Docker layer cache; it exists so the Windows toolchain is reproducible from the repo rather than depending on a hand-built local image.
Artifacts land in `dist/`.

## 6. Future scope (not implemented)

The following are documented direction per ADR-0001 but have no code and no crates today; add them when the work starts, not before:

- **Debugger adapter (DAP)** — a Qt-free core crate, same placement rationale as the shipped LSP client (`lsp-core`, [ADR-0016](decisions/0016-lsp-client.md)).
- **Plugin host** — *built.* [The plugin host and icon themes plan](plugin-host-and-icon-themes-plan.md) delivered the contract crate `plugin-api`, discovery and the registry in `plugin-host` ([ADR-0026](decisions/0026-plugin-host.md)), and the sandboxed wasmtime tier with fuel, epoch and memory limits ([ADR-0028](decisions/0028-wasm-plugin-tier.md)).
  A plugin declares contributions in `plugin.toml`; the Material icon pack ships as the first built-in and the Plugins settings page turns any of them off.
  ADR-0001's other half — a native dylib loader over a stable C ABI — remains unbuilt and unscheduled, and the sandbox is the reason it is not missed.
- **Markdown preview** — *built.* [The markdown preview plan](markdown-preview-plan.md) delivered the plugin host's third contribution point, `previews`, with a built-in native renderer (`markdown-preview`: comrak, merman, resvg) and a second, additive wasm world (`preview-plugin`) a sandboxed component may implement instead ([ADR-0033](decisions/0033-markdown-preview.md)).
  The Preview dock renders the active tab's Markdown, with inline Mermaid diagrams, off the Qt thread.
- **QML view** — the planned replacement for the Widgets view; the humble-view split exists so this swap stays cheap.

Each of these gets its own ADR under `decisions/` when it becomes real.

## Related

- [Layering rules](layering.md) — binding dependency and logic-placement rules.
- [ADR-0001](decisions/0001-core-tech-stack.md), [ADR-0002](decisions/0002-application-layer-and-humble-view.md), [ADR-0003](decisions/0003-ffi-conventions.md) — the binding stack, layering and FFI decisions.
- The remaining ADRs under `decisions/` cover MCP (transport, then protocol/index/lifecycle), docking, the terminal, the project index, find and replace, code navigation, the LSP client, and refactoring over LSP.
- [MVP implementation plan](mvp-implementation-plan.md) — historical.
