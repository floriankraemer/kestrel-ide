# Layering rules

Binding target architecture per [ADR-0002](decisions/0002-application-layer-and-humble-view.md) and [ADR-0003](decisions/0003-ffi-conventions.md).
Hexagonal-lite with a humble Qt view: logic in Qt-free Rust, the view only displays and forwards intent.

## Layers

The layers are: domain (`editor-core`, `project-model`), application (`app-core`), support (`app-config`, `syntax-core`, `index-core`, `lsp-core`, `settings-model`, `edit-ops`, `vcs-core`, `pty-core`, `terminal-core`, `run-core`, `build-core`, `dap-core`, `stdio-framing`, `mcp-server`, `plugin-api`, `plugin-host`, `icon-theme`, `markdown-preview`), adapter + view (`ui-shell`), and the `app` binary.
The building-block diagram lives in [overview.md §3](overview.md#3-building-block-view) — one diagram, one place.

## Allowed imports

| Crate | May depend on | Qt/cxx-qt allowed |
|-------|---------------|-------------------|
| `editor-core` | (std, ropey, regex) | **No** |
| `project-model` | (std, notify, dirs) | **No** |
| `syntax-core` | (std, tree-sitter plus the bundled grammar crates — see `crates/syntax-core/Cargo.toml`, streaming-iterator, serde, toml, libloading, tree-sitter-language) | **No** |
| `app-config` | (std, dirs, serde, toml, nucleo-matcher) | **No** |
| `mcp-server` | `index-core`, `editor-core` (+ std, serde, serde_json, tokio, axum) | **No** |
| `pty-core` | (std, portable-pty) | **No** |
| `terminal-core` | (std, alacritty_terminal) | **No** |
| `lsp-core` | `editor-core`, `stdio-framing` (+ std, lsp-types, serde, serde_json, globset, notify); `syntax-core` as a normal dependency (ADR-0035, amending ADR-0018) for `semantic_tokens`'s LSP-token-to-`Scope` mapping and its tree-sitter-span overlay — ADR-0018's ban on `lsp-core` re-deciding file-to-language detection still holds, nothing here parses an extension or a language id. Stays free of `plugin-api`/`plugin-host`: `catalog::PluginServer` is a plain data type `lsp-core` defines for itself, and `ui-shell`/`settings-model` (which already depend on `plugin-host`) map `LanguageServerContribution` onto it at the call site. Also stays free of `project-model`: C5's `watched_files::FileChangeKind` converts from `notify::EventKind` directly rather than pulling in the domain crate for one enum. | **No** |
| `index-core` | `syntax-core`, `editor-core` (+ std, tantivy, grep-searcher, grep-regex, grep-matcher, ignore, rayon, nucleo-matcher, fs4, dirs) | **No** |
| `plugin-api` | (std, serde, toml) — a leaf on purpose, see [ADR-0026](decisions/0026-plugin-host.md) | **No** |
| `plugin-host` | `plugin-api` (+ std, wasmtime) — discovery, the registry and the built-ins ([ADR-0026](decisions/0026-plugin-host.md)), plus the sandboxed wasm tier ([ADR-0028](decisions/0028-wasm-plugin-tier.md)); `icon-theme` as a **dev**-dependency only, to check the vendored Material pack through the real load path | **No** |
| `icon-theme` | (std, serde, toml, resvg) — **not** `syntax-core` and **not** `plugin-host`, see [ADR-0027](decisions/0027-icon-themes.md) | **No** |
| `markdown-preview` | `syntax-core` (+ std, comrak, resvg, merman `=0.7.0-alpha.1` pinned exactly with its sibling crates, regex-lite) — **not** `plugin-api` and **not** `plugin-host`, the same isolation `icon-theme` keeps, see [ADR-0033](decisions/0033-markdown-preview.md) | **No** |
| `settings-model` | `app-config`, `syntax-core`, `lsp-core`, `edit-ops`, `editor-core`, `plugin-api`, `plugin-host` (+ std, serde, toml, tree-sitter) | **No** |
| `edit-ops` | `editor-core`, `syntax-core` (+ std, tree-sitter) | **No** |
| `vcs-core` | `editor-core` (+ std, gix, serde) | **No** |
| `run-core` | `pty-core`, `app-config`, `terminal-core` (+ std, serde, toml, serde_json, regex) | **No** |
| `build-core` | `run-core` (+ std, serde_json, regex) | **No** |
| `dap-core` | `run-core`, `app-config`, `stdio-framing` (+ std, serde, serde_json) | **No** |
| `stdio-framing` | (std only) | **No** |
| `app-core` | `editor-core`, `project-model`, `plugin-host`, `icon-theme`, `syntax-core`, `markdown-preview` — the last four only for the icon-theme and previews joins, see below | **No** |
| `ai-chat-core` | `lsp-core` (+ std, serde, serde_json, base64, tiktoken-rs, reqwest/rustls) | **No** |
| `ui-shell` | `app-core`, `editor-core`, `edit-ops`, `project-model`, `app-config`, `settings-model`, `syntax-core`, `mcp-server`, `index-core`, `lsp-core`, `ai-chat-core`, `pty-core`, `terminal-core`, `plugin-host`, `vcs-core`, `run-core`, `markdown-preview` (+ tokio, cxx, cxx-qt, cxx-qt-lib) | Yes (adapter + view live here) |
| `app` | `ui-shell` | Yes |
| `e2e` | (std, serde_json, tempfile) — **no workspace crate**; drives the built `app` binary over X11 and the filesystem, as a user does (ADR-0024) | **No** |

`e2e`'s *tests* live in `crates/app/tests/`, not in `crates/e2e/tests/`: `CARGO_BIN_EXE_app` is only defined for integration tests of the crate that declares the binary, and a harness that guesses at `target/<profile>/app` is wrong under every profile but one.
That test target is the one place `app-config` may be read from a test rather than from the application — the window-state flow must parse the persisted TOML with the app's own types, or it only asserts against its own re-implementation of them.

`editor-core`, `project-model`, and `app-core` MUST NOT depend on cxx-qt or Qt in any form — no direct dependency, no transitive dependency, no feature-gated dependency.

## Where logic may live

- **Business rules and orchestration** (open rules, path construction, delete/rename → tab policy, watcher policy, dirty tracking, jump history): only in the Qt-free crates, normally `app-core`.
- **Rules that need the project index** (which declaration a caret resolves to, ADR-0011's local-file-then-project ranking; expanding a replacement against a matched span) live in `index-core`, not `app-core`: `app-core` may not depend on `index-core`. They are still Qt-free and unit-tested like any other rule.
- **Rules a refactoring needs** (which documents of a workspace edit are spliced in a buffer and which are written to disk, whether an answer is still fresh enough to apply, whether an inbound `workspace/applyEdit` was asked for, and whether a name-based rename site can be vouched for) live in `lsp-core` and `index-core` (ADR-0019).
  The adapter routes and the view paints; neither decides. In particular `bridge.rs` never re-derives which pile an edit belongs to — it forwards the flag `lsp_core::plan_edit` set.
- **Diff computation** (line hunks and intra-line spans between two texts) lives in `editor_core::diff`, Git-free by design (ADR-0028), and stays the *only* place that runs the diff algorithm.
  `lsp-core` depends on `editor-core` for exactly this reason: `lsp_core::diff_preview::file_diff` turns a pending refactoring's before text and `DocumentEdits` into the after text and its hunks for `RefactorPreviewDialog`'s diff panel, reusing `apply_to_text` and `editor_core::diff::diff_lines` rather than a second implementation of either. `index-core` already depended on `editor-core`, so `index_core::TextIndex::preview_replacements` (the Replace-in-Files preview) needed no new edge.
- **Rules a settings page needs** (which override a colour row comes from, what a language load failure means in English, which server entries are worth persisting, which editing settings a language may override and what a nonsensical tab width is worth) live in `settings-model`, not in `app-config`: they join persisted settings to the vocabularies of `syntax-core` and `lsp-core`, which `app-config` deliberately knows nothing about (ADR-0017).
- **Which settings layer a value comes from** is `settings_model::scope`'s answer, not `app-config`'s and not the dialog's (ADR-0022).
  `app-config` reads and writes two files — the global `settings.toml` and `<project>/.ide/settings.toml` — and knows nothing about precedence; `scope::resolve` layers them and `scope::origin` says where an effective value came from, which is the badge the settings dialog shows and never re-derives.
  Which settings a project may override at all is `scope::ScopedField`, an enum rather than a convention, so widening that list is a deliberate edit rather than a side effect of adding a field.
  `settings_model::editing` resolves into the parameter objects the editing crates already take — `edit_ops::IndentStyle` and `editor_core::SaveRules` — which is why those two crates appear in its dependency row: a resolved `EditingRules` is handed straight through rather than unpacked into loose arguments at the seam, where a tab width could be paired with the wrong buffer's spaces flag.
- **What a plugin manifest means** — which ids and paths are well-formed, which contract revisions are compatible, how far a capability reaches — lives in `plugin-api`, next to the manifest it describes (ADR-0026).
  A `PluginManifest` that exists has been validated, so no consumer re-checks; `plugin-host` is left with the parts that genuinely need a disk.
  `plugin-api` names no consumer of a contribution — not the host, not `icon-theme` — because a contract that depends on one of its parties is not a contract.
- **Which plugins exist, and which of them may run** — the scan of `<config_dir>/plugins`, the built-ins embedded in the binary, the user's disabled list, quarantine markers and duplicate-id resolution — lives in `plugin-host` (ADR-0026).
  It stores contribution payloads and never interprets one: the registry has no idea what an icon theme is, which is why `icon-theme` reads `IconThemeContribution` without depending on the host and the two are joined in `app-core`.
  Reading a plugin's files goes through `LoadedPlugin::read_asset`, the single place that turns a manifest-supplied string into a filesystem read, so a built-in's embedded bytes and an installed plugin's directory look the same to every consumer.
- **What a plugin's own code may do** — the wasmtime component runtime, the fuel/epoch/memory limits, the capability-gated host functions, and running a contributed command — lives in `plugin_host::wasm` (ADR-0028), layered on top of discovery: a `WasmTier` is built from a finished `PluginRegistry` and can only start a plugin the registry already accepted.
  A trap disables that one plugin with a typed error and never the process, which is the property that made a sandbox worth choosing over ADR-0001's native dylib tier.
  That typed error is what Settings > Plugins renders as a sentence, which is where the property becomes visible to a user rather than only true.
- **What the Plugins page says** — which rows exist, which of them the user turned off, and what a `LoadErrorKind` or a `WasmError` means in English — lives in `settings_model::plugins`, which is why `plugin-api` and `plugin-host` appear in that crate's dependency row (P7).
  Exactly the arrangement `settings_model::languages` already has with `syntax-core`'s runtime, for exactly the reason ADR-0017 gives: the page's value is that it never prints a Rust error, and that mapping deserves unit tests neither `bridge.rs` nor `cpp/` can have.
  The page scans with nothing filtered rather than reading the live registry, because the live one has already dropped every disabled plugin and a page that cannot list one could never switch it back on.
- **Which icon a row gets** lives in `icon-theme` (ADR-0027), and it is handed the language id rather than deriving one: `IconPack::file_icon` takes `language_id: Option<&str>` and the crate does not depend on `syntax-core`, so ADR-0018's single detection table stays single.
  Its own extension table answers "which art", never "which language", and nothing may ask it the second question.
  Reading a pack's files is the caller's job through the `IconAssets` trait, because a built-in plugin's SVGs are embedded in the binary and an installed plugin's are on disk; `icon-theme` therefore depends on `plugin-host` no more than `plugin-api` does, and `app-core` joins the two.
- **Where those three meet** is `app_core::icons` (ADR-0026's amendment), and nowhere else.
  It owns the active theme — the registry snapshot, the resolved `IconPack`, the `IconAssets` implementation backed by `LoadedPlugin::read_asset`, and the `IconRenderer` — and answers exactly two questions: the `"<pack-id>/<icon-id>"` key for a row, and the premultiplied RGBA8 behind a key.
  It is also the one place that asks `syntax_core::language_for_path` on an icon's behalf, which is the ADR-0018 join `icon-theme` refuses to make itself.
  Mapping a colour theme name to a light or dark icon set is a rule and lives here too, not in `theme.cpp`.
  Which icon theme is active is the same kind of answer: `IconService` is handed the persisted id and falls back to the first theme there is when nothing offers it, so a setting that outlives its plugin costs the user no icons (P7).
  This is why `app-core`'s dependency row grew past the two domain crates: all three additions are Qt-free, so the hard rule below is untouched, and the `cargo tree` gate is what proves it rather than the claim.
- **Which preview a document gets, and who renders it** — the extension-to-provider table built from `previews` contributions (installed shadowing built-in, exactly `icon_themes`' direction and for the same reason), and the dispatch between the built-in native renderer and a running wasm plugin's `render` export — lives in `app_core::preview` (ADR-0033), the same shape as `app_core::icons` and joined for the same reason: `plugin-host` stores a contribution and never interprets it, `markdown-preview` renders and knows nothing about plugins.
  A wasm provider's SVG is rasterised with the host's own bundled font, in `markdown-preview`, never inside the sandbox — a fuel-metered 64 MiB store is not where a rasteriser belongs, and the host already owns one.
  A trap or a failed render from a wasm provider is never silently swapped for the native path: two different answers for one file would be worse than one honest `PreviewError`.
- **Which kind of page a tab needs** is `app-core`'s answer, carried across the seam as `TabKind` (ADR-0020). The view builds a `CodeEditor` or a `HexViewer` from it and never infers the kind from the path or the bytes. What a hex row *says* — offset format, byte grouping, printable-byte rule, short-row padding — belongs to `editor_core::hex`, next to the `binary_detect` rule that decides what counts as binary in the first place.
- **Rules the AI assistant needs** (how attachments are assembled into a prompt and in what order they are truncated, how many tokens that costs, which files are too secret to attach or read, whether a model-supplied path escapes the project, which tool an approval policy allows, how a code block or an approved tool call becomes an edit, and when an agent run must stop) live in `ai-chat-core` (ADR-0021).
  Which models a provider offers is fetched there too, by `ai_chat_core::models`, and so is the sentence describing that fetch — the pickers in the panel header and the settings page show it and never compose one (ADR-0034).
  It depends on `lsp-core` because a proposed edit is expressed as `Vec<lsp_core::DocumentEdits>` and applied through the same `plan_edit` path a refactoring uses — there is no second apply semantics and no second undo story.
  The agent's tool catalog is deliberately the work `mcp-server` already performs: `ai-chat-core` owns the schemas, the policy and the loop, while executing a tool is a callback `ui-shell` routes to the same `AppSession` and index paths MCP drives (ADR-0012).
  An assistant inside the IDE therefore cannot see a different project than an agent attached over MCP, and `bridge.rs` never decides whether a tool may run.
  `ai-chat-core` is the one Qt-free crate deliberately **not** covered by the `grep -i tokio` gate the other background-work crates take: `reqwest::blocking` builds a private current-thread runtime internally, so tokio appears in its tree even though no async code is written here and no runtime is managed by us.
  The gate that matters for it is the Qt one, and the rule the tokio gate was protecting — that long work runs on a `std::thread` and returns through `CxxQtThread::queue()`, not on an ambient runtime — is unchanged (ADR-0007, ADR-0021).

- **Language-aware editing operations** — commenting, expand/shrink selection, indentation, bracket pairing and bracket matching — live in `edit-ops`, because each needs the text (`editor-core`) *and* the grammar (`syntax-core`) at once.
  `editor-core` may not depend on `syntax-core` and must not start; passing "the comment token for this language" in from `bridge.rs` would put the join in the adapter, which is what these rules forbid. Same situation as `settings-model`, same answer.
  What a comment token, a bracket pair or a quote *is* stays in `syntax-core`'s one registry (ADR-0018) — `edit-ops` reads it and never keeps a second table.
  Every entry point takes `text: &str`, never a `Document`: the rope is only refreshed on save, so it is one save behind the live Qt buffer.

- **Which Git operations run in-process and which shell out** is `vcs-core`'s, and is stated in ADR-0031: pure reads of object/index state (discovery, HEAD, status, branch listing, log) go through `gix`; anything touching the user's configuration, credentials, hooks or signing (staging, commit, branch create/checkout/delete, fetch/pull/push) shells out to `git`.
  Two reads shell out as well, for the same measured reason in both cases — `git` has an implementation `gix` does not, and neither is on a hot path: blame (`git blame --porcelain`) and per-file history (`git log --follow`, 5.4× the in-process ancestry walk it replaced; see ADR-0031 §7).
  `ui-shell` never spawns `git` and never calls `gix` directly — both live behind `vcs-core`'s own API, wrapped in this crate's own types (`HeadInfo`, `RepoStatus`, `FileStatus`, `LogEntry`, `BlameLine`, `VcsError`) rather than `gix`'s, so a future backend swap stays inside this crate.
  Working-tree hunks (`hunks::HunkCache`) and a hunk revert (`revert::revert_hunk_edit`) reuse `editor_core::diff` rather than a second diff implementation, the same rule ADR-0028/0030 already established for the refactor preview and Replace in Files.

- **Where multi-caret state lives** (ADR-0023): the `SelectionSet`, the expand/shrink history and the auto-close pair tracker are kept in `ui-shell`'s `EditorOps`, keyed by `TabId` — not on `editor_core::Document` (stale, refreshed only on save) and not in `app_core::AppSession` (which has no reason to know about carets). `EditorOps` computes every operation as one `Transaction` over the live buffer text and hands the seam one `Vec<FfiTextEdit>`, spliced through the same `EditorTabs::applyBufferEdits` path a refactoring already uses — the rule that one keystroke is one undo step lives in `editor-core`, not in `cpp/`.
- **Which files a workspace edit creates, renames or deletes** is parsed and ordered by `lsp-core` (`ResourceOp`, `WorkspaceChanges`); *performing* one — and retargeting an open tab whose file moved — is `app-core`'s, as `FileOp` (ADR-0029). `ui-shell` maps one type to the other and decides nothing else; every resource operation runs, all-or-nothing, before any of the same edit's text edits are written.
- **Which build tool a project uses** is answered in exactly one place, `run_core::toolchain` (ADR-0039): the marker files that identify a toolchain, its build and clean invocations, the JS package manager or JVM wrapper to prefer, and the debug adapter it implies.
  `run_core::detect` is written on top of it, and `build-core` and `dap-core` read it rather than detecting again — the same single-table rule ADR-0018 established for languages and ADR-0027 for icon art.
  Detection is marker-file presence only; nothing here runs the build tool to find out.
  What a run configuration's macros mean (`run_core::macros`) and what running a file would launch (`run_core::context`) live beside it, and the view asks rather than deciding which files look runnable.

- **How a project is built, and what its output means** lives in `build-core` (ADR-0040): which steps a build request runs, and how a tool's output becomes a `BuildDiagnostic`.
  It delegates and never models — no compiler, no output folder, no artifact, no build-automatically-on-save — because every one of those is a second opinion about something the build tool already decides.
  It reads `run_core::toolchain` for the invocation and `run_core::LaunchSpec` for the launch, so there is no second detection table and no second way to start a process.
  Its diagnostics are deliberately the shape the Problems dock already renders for `lsp_core::DiagnosticStore`: one question, one place to look.
  Its text patterns overlap `run_core::links`' catalogue and stay separate on purpose — a link resolver wants a location, a build wants the severity and message too, and one table serving both would satisfy neither.

- **How a debugger is driven** lives in `dap-core` (ADR-0041): the Debug Adapter Protocol's envelope, the session handshake, the adapter catalog and, from D2, the breakpoint store.
  It reads `run_core::toolchain` for which adapter a project implies rather than keeping a second mapping, and it types only the protocol bodies something actually reads a field out of.
  A capability the adapter did not declare is unsupported: the view disables an action because the adapter said so, never because C++ guessed.
  It owns no editor buffer — a breakpoint's line is shifted from the existing buffer-edit seam in `ui-shell`, not from a new hook in the editor.
- **The `Content-Length` framing both protocols use** lives in `stdio-framing`, and in no other crate.
  `lsp-core` and `dap-core` frame their messages with the same bytes, so this is the one thing they share; anything that merely resembles the other stays separate, exactly as `build-core`'s diagnostic patterns do beside `run_core::links`.

- **Which language a file is** is answered in exactly one place, `syntax-core`'s registry (ADR-0018).
  `lsp-core` owns only what the protocol owns — the server command per language id, and the few ids LSP names differently from the grammar (`tsx` -> `typescriptreact`) — and `ui-shell` joins the two, which is translation and so allowed in the adapter.
  No crate may grow a second file-extension table.
- **The index instance** is built and updated by `ui-shell`'s `SearchModel` and shared with `mcp-server` as an `Arc<RwLock<IndexSlot>>` (ADR-0012). `mcp-server` only queries it; it never builds or owns one.
- **Window-frame rounding/shadow** stays inside ADR-0001's native-chrome constraint: `main_window.cpp`'s `applyNativeWindowChrome()` opts into Windows 11's own `DWMWA_WINDOW_CORNER_PREFERENCE`, a DWM setting rather than app-painted chrome, and is a no-op elsewhere. macOS's `NSWindow` already casts a native shadow with no code needed. On Linux this is entirely WM/compositor-controlled; getting the app to influence it would require going frameless (client-side decorations), which ADR-0001 rules out — so there is no Linux code path here, by design, not by omission.
- **`bridge.rs` (adapter)**: translation only — QString/QModelIndex ↔ Rust types, session call, emit signal, refresh model. No domain state, no rules, no branching beyond type mapping.
- **`cpp/` (view)**: widget construction, layout, menus, dialogs, signal wiring only. It may ask "what happened" and show the answer; it never decides "what should happen".

Rule of thumb: if it deserves a unit test, it cannot live in `bridge.rs` or `cpp/`.

## FFI seam rules

Summary of [ADR-0003](decisions/0003-ffi-conventions.md):

- Errors cross as a typed struct: stable `i32` code (0 = success) + display message. Never a `QString` sentinel.
- Tabs are identified by `TabId(u64)` issued by `app-core`; index mapping exists only at the Qt tab-strip/model edge.
- Rust `Document` is the single source of truth for dirty state; `QTextDocument` forwards edits, the view reads flags.

## UI framework

The view is Qt Widgets today.
QML is the planned future view.
View-swappability is therefore a hard requirement, not a nice-to-have — the humble-view split above is what guarantees it, because a view containing zero rules can be replaced without touching `app-core` or the domain crates.

## Verification

```sh
cargo test --workspace
cargo tree -p editor-core -e normal | grep -i qt    # must be empty
cargo tree -p project-model -e normal | grep -i qt  # must be empty
cargo tree -p app-core -e normal | grep -i qt       # must be empty
cargo tree -p pty-core -e normal | grep -i qt       # must be empty
cargo tree -p pty-core -e normal | grep -i tokio    # must be empty
cargo tree -p terminal-core -e normal | grep -i qt    # must be empty
cargo tree -p terminal-core -e normal | grep -i tokio # must be empty
cargo tree -p app-config -e normal | grep -i qt     # must be empty
cargo tree -p edit-ops -e normal | grep -i qt       # must be empty
cargo tree -p index-core -e normal | grep -i qt     # must be empty
cargo tree -p index-core -e normal | grep -i tokio  # must be empty
cargo tree -p mcp-server -e normal | grep -i qt     # must be empty
cargo tree -p lsp-core -e normal | grep -i qt       # must be empty
cargo tree -p lsp-core -e normal | grep -i tokio    # must be empty
cargo tree -p ai-chat-core -e normal | grep -i qt   # must be empty
cargo tree -p e2e -e normal | grep -i qt            # must be empty
cargo tree -p plugin-api -e normal | grep -i qt     # must be empty
cargo tree -p plugin-host -e normal | grep -i qt    # must be empty
cargo tree -p icon-theme -e normal | grep -i qt     # must be empty
cargo tree -p markdown-preview -e normal | grep -i qt  # must be empty
cargo tree -p run-core -e normal | grep -i qt        # must be empty
cargo tree -p build-core -e normal | grep -i qt      # must be empty
cargo tree -p build-core -e normal | grep -i tokio   # must be empty
cargo tree -p dap-core -e normal | grep -i qt        # must be empty
cargo tree -p dap-core -e normal | grep -i tokio     # must be empty
cargo tree -p stdio-framing -e normal | grep -i qt   # must be empty
```

## Known debt at time of writing

- (Both items previously listed here — stale `lib.rs` doc comments describing
  shipped work as future, and `settings-model` citing ADR-0016 instead of
  [ADR-0017](decisions/0017-settings-model-crate.md) — are fixed.)

No new code may reintroduce `QString` sentinel errors, int-index tab
identity, or business rules in `bridge.rs`/`cpp/` — see the FFI seam
rules and "Where logic may live" above.
