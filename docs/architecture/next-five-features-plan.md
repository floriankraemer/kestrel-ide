# The next five features: verification foundation, editor ergonomics, intentions, Git, run configurations

## Context

Fourteen crates and five plan documents in, this IDE reads and changes code well: 35 languages, a project index, an LSP client with navigation, diagnostics, completion, code actions and rename, a terminal, and an in-IDE assistant.
Three things it does not have are the three that separate an editor from an IDE.

It cannot tell you whether it still works after a change.
Every plan document so far ends with a section titled *what a human should click through*, because nothing above the FFI seam is verified by anything but a person.
There is no E2E infrastructure at all, and no language server has ever been installed in the build image — so diagnostics, hover, completion, code actions and rename have never once run against a real server.

It does not feel like an editor under the fingers.
No multi-caret, no `Ctrl+/`, no duplicate or move line, no expand selection, no auto-close, no reformat, and no editor-behaviour settings at all — not even tab width.

It cannot run or version your code.
No VCS of any kind, and no way to run the thing you are writing except retyping the command into the terminal, where a stack trace is dead text.

These five features close those three gaps, in that order.
The ranking is the product manager's and is not re-litigated here.
Build order is foundation-first: F0's harness and seam split land before the features that would otherwise be poured into two files already at 9388 and 4350 lines.

## Progress

Living status table — update the relevant row(s) **in the same commit** that finishes a task, so status and code never drift apart.
A fresh session should read this table (and `git log`) before picking up work, per `CLAUDE.md`.

Task ids are stable; titles may change. `blocked on X` means the task cannot start until X lands, not that it is unscheduled.

### F0 — Verification foundation

| Task | Status | Commit |
|---|---|---|
| F0-1 — file-size gate + ratcheted baselines | done | 99a37f5 (#73), corrected in #79 |
| F0-1b — promote `Utf16Cursor` to `editor_core::offsets` | done | febd6c0 (#74) |
| F0-2 — bridge.rs split, part 1 | done | 02b1fa2 (#78) |
| F0-3 — bridge.rs split, part 2 | done | 02b1fa2 (#78) |
| F0-4a — main_window.cpp split, part 1: `EditorTabs` | done | f0f73db (#100) |
| F0-4b — main_window.cpp split, part 1: `ClassViewPanel`, `FindUsagesPanel`, `IdeMainWindow` | done | dfa35f8 (#101) |
| F0-5 — main_window.cpp split, part 2: `RefactorController`, `DeclarationNavigator` | done | ddc5160 (#102) |
| F0-6 — settings dialog extraction + `SettingsContext` | done | a6e745b (#103), follow-ups 5889357 (#106) |
| F0-7 — dock registry and general reconciliation | done | e5b325f |
| F0-8 — byte-column fix in `moveCursorToLine` | done | febd6c0 (#74) |
| F0-9 — per-project settings persistence | done | 720d1d9 (#75) |
| F0-10 — per-project settings rules + dialog | done | 00275a6 (#139); index excludes included, so ADR-0022's four areas are all real |
| F0-11 — E2E harness | done | 1eb5009 (#76) |
| F0-12 — E2E seed flows | done | 1eb5009 (#76) |
| F0-13 — E2E in CI | done | 05d7d9d (#135) |
| F0-14 — `lsp-conformance` image stage | done | 2f1d4ec (#77) |
| F0-15 — LSP conformance harness + expectations | done | harness 2f1d4ec (#77), nightly CI job 05d7d9d (#135) |
| F0-16 — conformance fix: `$/progress` indexing state | done | 47726ae (#136) — the one defect F0-15 produced; each further one gets its own row |
| F0-17 — docs: this plan, ADRs, layering, overview | done | 5675b3f (#80) |
| F0-18 — route every buffer edit through `applyBufferEdits` | done | dd1fdb6 (#137) |
| F0-19 — error-code ranges (ADR-0003 amendment) | done | 5d3d9a1 (#138); 25 literals, not the 5 this plan predicted |

### F1 — Editor ergonomics

| Task | Status | Commit |
|---|---|---|
| F1-1 — `editor-core::selection` | done | b839b53 (#81) |
| F1-2 — `editor-core::transaction` | done | b839b53 (#81) |
| F1-3 — next occurrence + column selection | done | b839b53 (#81) |
| F1-4 — `editor-core::line_ops` | done | b839b53 (#81) |
| F1-4b — `syntax-core` registry: comment tokens + bracket pairs | done | 6c787c8 (#89) |
| F1-5 — `edit-ops` crate + comment toggle | done | 6c787c8 (#89) |
| F1-6 — expand/shrink selection | done | 6c787c8 (#89) |
| F1-7 — auto-indent and indent/unindent | done | 6c787c8 (#89) |
| F1-8 — smart typing (auto-close, type-over, surround) | done | 6c787c8 (#89); view wiring recorded at the end of this branch |
| F1-9 — bracket match | done | 6c787c8 (#89) |
| F1-10 — editing settings: persistence + rules | done | 3d4002d (#90) |
| F1-11 — save rules (trim, final newline, line endings) | done | 3d4002d (#90); wired into the save path at the end of this branch |
| F1-12 — `lsp-core` formatting | done | 7891f57 (#82) |
| F1-13 — bridge: `EditorOps` | done | recorded at the end of this branch |
| F1-14 — bridge: `EditingEditor` + formatting | done | recorded at the end of this branch |
| F1-15 — view: multi-caret rendering and input | done | recorded at the end of this branch |
| F1-16 — view: actions and menus | done | recorded at the end of this branch |
| F1-17 — view: editing settings page | done | recorded at the end of this branch |
| F1-18 — E2E + ADR-0023 + docs | done | 14f7b76 |

### F2 — Intentions and LSP surface

| Task | Status | Commit |
|---|---|---|
| F2-1 — resource operations: parse and order | done | ac32f77 (#84) |
| F2-2 — resource operations: perform as `FileOp` | done | ac32f77 (#84) |
| F2-3 — resource operations: bridge + view | done | 28ead4b |
| F2-4 — `lsp-core::intentions` | done | 264f3b3 (#88) |
| F2-5 — `lsp-core::signature_help` | done | 264f3b3 (#88) |
| F2-6 — `document_highlight` + `inlay_hint` | done | 264f3b3 (#88) |
| F2-7 — organize imports | done | 264f3b3 (#88) |
| F2-8 — bridge: intentions | done | 8554431 |
| F2-9 — bridge: signature help, highlights, hints | done | a3133ba (also organize imports, F2-8's remainder) |
| F2-10 — view: bulb and popup | done | fca1d99 |
| F2-11 — view: signature tip, highlights, hints | done | 90af252 |
| F2-12 — E2E + its ADR + docs | done | recorded at the end of this branch; ADR-0029 |

### F3 — Git v1

Renumbered to match §4's Task breakdown, which this table had drifted from (an earlier draft's F3-2…F3-23 predated the a/b/c/d split of F3-12 and the `DiffView` split of F3-13…F3-15; §4 is the numbering actually implemented against).

| Task | Status | Commit |
|---|---|---|
| F3-1 — `editor-core::diff` | done | 0416194 (#87) |
| F3-2 — `vcs-core` skeleton + discovery | done | fc15bf1 |
| F3-3 — status and HEAD reads | done | a31b796 |
| F3-4 — working-tree hunks + cache | done | 13168ae |
| F3-5 — the `git` subprocess layer | done | 8a70d3b |
| F3-6 — staging | done | 73d6a7f |
| F3-7 — commit | done | c68bd02 |
| F3-8 — branches | done | 8402857 |
| F3-9 — remotes | done | 82289c7 |
| F3-10 — history and blame | done | b49764a |
| F3-11 — revert hunk | done | f9dbb4d |
| F3-12a — bridge: `VcsService` supervisor + status | done | 5105dee |
| F3-12b — bridge: hunks + revert | done | 5105dee |
| F3-12c — bridge: staging + commit | done | 5105dee |
| F3-12d — bridge: branches, remotes, history | done | 5105dee |
| F3-13 — view: `DiffView` | done | 03f89fa |
| F3-14 — view: `TabKind::Diff` + `diff_labels`, and the JetBrains-style diff viewer pass (curved connectors, collapsible unchanged regions, syntax highlighting, ignore-whitespace, an editable working-tree-vs-HEAD diff window) — split into an in-place diff *mode* for the editable case and a real `TabKind::Diff` tab for read-only comparisons; see ADR-0030's follow-up note | done | 8f91028 |
| F3-15 — retrofit: refactor preview, project-wide replace preview and AI apply render through `DiffView` | done | fcb917e, 27d3a88 |
| F3-16 — view: `vcs_gutter` | done | d09bc5c |
| F3-17 — view: `changes_panel` | done | fe4a5fb |
| F3-18 — view: branch widget, `file_history_panel`, blame gutter | done | fe4a5fb |
| F3-19 — view: the VCS action set and menu | done | fe4a5fb |
| F3-20 — E2E (2 flows) + docs; no new ADR (ADR-0030/0031 already cover F3's decisions, see below) | done | c3611eb |
| F3-21 — measured performance pass: timing harness + fixtures, `gix` object cache, `HunkCache` keyed on the document's revision, `file_history` via `git log --follow`, timed-out `git` children killed (ADR-0031 §7) | done | 0c2cdfe |

### F4 — Run configurations and console

| Task | Status | Commit |
|---|---|---|
| F4-1 — `pty-core`: cwd and env | done | e1788b0 (#86) |
| F4-2 — `pty-core`: kill tree | done | e1788b0 (#86) |
| F4-3 — `run-core` skeleton + model | done | 431c545 |
| F4-4 — persistence | done | 431c545 |
| F4-5 — detection | done | 431c545 |
| F4-6 — supervisor | done | 431c545 |
| F4-7 — output batching | done | 431c545 |
| F4-8 — `run-core::links` | done | 431c545 |
| F4-9 — bridge: `RunService` | done | 369928e |
| F4-10 — bridge: `RunConfigEditor` | done | 369928e |
| F4-11 — view: run console dock | done | efd9518 |
| F4-12 — view: toolbar and dialog | done | efd9518 |
| F4-13 — view: clickable links | done | efd9518 |
| F4-14 — terminal multi-session (a) core | done | 4036e21 |
| F4-15 — terminal multi-session (b) view | done | 4036e21 |
| F4-16 — E2E (2 flows) + ADR-0032 + docs | done | e9c56f2 |

**ADR numbering has drifted and must be reallocated.**
§3 reserves 0022–0029, but the plugin and icon-theme stream that landed alongside F0 took **0026 (plugin-host), 0027 (icon-themes) and 0028 (wasm-plugin-tier)**.
On disk the free numbers are **0023** — still F1's, as planned — and **0029 onwards**.
So F2's, F3's two and F4's ADRs renumber from 0029; §3's prose keeps the old numbers and is wrong about them.
A handful of code comments already cite "ADR-0026" meaning F2's resource-operation split rather than the plugin host, and each is corrected where it is found.

Likewise, each feature's terminal task (F0-17, F1-18, F2-12, F3-20, F4-15) currently bundles two E2E flows with an ADR and two doc updates. **Move each ADR and `layering.md` row onto the task that introduces its crate** — same `CLAUDE.md` rule — leaving the terminal task to the E2E flows and `overview.md`.

---

## 1. Decisions resolved before work starts

Two of these came out of a genuine disagreement between the architecture and testing passes and are settled here so they are not reopened mid-feature.

**The live buffer is `QTextDocument`'s, and Rust computes over a snapshot passed in per call.**
This corrects a false premise the first draft of this plan was built on.
`editor_core::Document`'s rope is populated on open and **never updated as the user types** — the only content-mutating slot is `save_tab(tab_id, content)` (`bridge.rs:862`), which pulls the full text *out of* the widget, and `replace_content`'s own doc comment says so (`editor-core/src/lib.rs:122-127`).
So a signature like `(&Document, &SelectionSet) -> Transaction` reads a rope that is one save behind, and `jumpToByteOffset` (`main_window.cpp:692`) already has this bug today.
Making the rope authoritative means incremental sync (`contentsChange` → `Document::splice`) with its own divergence class, which is a project of its own and not what these features need.
Instead every `edit-ops` and new `editor-core` entry point takes **`&str`**, exactly as `findMatches(text, pattern, …)` and `replacementsFor(text, …)` already do (`bridge.rs:817-839`), and `EditorOps` slots take the buffer text alongside the caret set.

**Undo stays `QTextDocument`'s.**
The testing pass argued undo should move into `editor_core::Document` with its own transaction stack, on the grounds that "all carets' edits are one undoable transaction" otherwise lives in the layer that is untested by design.
Rejected, because ADR-0023's design already dissolves the objection: Rust computes **one `Transaction` per keystroke** and hands the seam **one `Vec<FfiTextEdit>`**, so the rule — what constitutes one user-visible change — is in `editor-core` and unit-tested there.
What remains in C++ is a single wiring fact: that the edit list is spliced inside one `beginEditBlock`.
That is exactly what `EditorTabs::applyBufferEdits` already does for refactorings (`main_window.cpp:399`), and one E2E flow covers it.
A second undo stack that must agree with Qt's forever is a worse trade than one untested `beginEditBlock` call.

**The E2E flow budget is a policy, not an estimate.**
5 minutes wall clock locally, 10 in CI, which at ~20–30s per flow is a hard ceiling of **12–15 flows, forever**.
F0 spends 4, F1–F4 spend 2 each. That is the entire allocation.
Once full, adding a flow means deleting one — which forces the question "is this really only testable through the UI?", enforced by arithmetic instead of discipline.

**Everything else adopted as designed**, including the architect's crate decomposition and the tester's harness design, flakiness policy and per-feature cases.

---

## 2. Architecture per feature

### F0 — Verification foundation

**New crate**

| Crate | Layer | Why not an existing crate |
|---|---|---|
| `e2e` | test-only workspace member, `publish = false` | Spawns the *built binary* as a child and drives X11. Not a library any crate may depend on, and it must not run under `cargo test --workspace`. A crate rather than a shell script because the wait-for-change-then-stability discipline, screenshot naming and seeded-config construction are logic, and logic gets unit tests. |

Depends on `std` and `tempfile` only — no workspace crate. It talks to the app through X11 and the filesystem exactly as a user does.
Tests are `#[ignore]`d; `make e2e` runs them inside `linux-builder` after `cargo build -p app`.

**Existing crates that change**

- `app-config` — new `src/project_settings.rs`: `ProjectSettings`, fields `Option<T>` with `skip_serializing_if`, loaded from `<project>/.ide/settings.toml`. `Settings` untouched. The sparse shape exists because `Settings`'s `#[serde(default)]` scalars cannot distinguish "0" from "not set" — precisely the distinction a layer needs.
  **It covers only the four project-scoped areas** (editing behaviour, language servers, run configurations, index excludes), not all 22 fields. A full mirror would have to be hand-synced forever, and it would let fields drift that Risk #6 says may never be overridden — a sparse struct of exactly the overridable set makes "which fields are project-scoped" a type rather than a rule to enforce.
- `settings-model` — new `src/scope.rs`: `resolve(global, project) -> Settings`, `origin(field) -> Scope`, and the rule for which fields are project-scoped at all. ADR-0017's shape verbatim: `app-config` persists, `settings-model` decides.
- `app-core` — `AppSession::open_at_location` becomes the one place a (path, line, column) becomes a caret position, converting the index's **byte** column to a character column against the actual line text. Kills the `openFileAtLine` defect before F3 and F4 add four more call sites.
- `lsp-core` — no code change; a new integration test file and a fixture project per language.
- `ui-shell` — the seam split (§6), the dock registry, scope-aware getters on `AppSettings`.

**QObjects** — no new ones. `AppSettings` gains `settingsScope()`, `setSettingsScope(QString)`, `fieldOrigin(QString) -> QString`, `hasProjectSettings() -> bool`, signal `settingsScopeChanged()`. Per-project settings is a change of *which layer a getter reads*, not a new surface.

**C++** — `cpp/settings_dialog.{h,cpp}` extracted, with a scope selector and a per-row "from project / from global / default" badge. The 14-parameter `showSettingsDialog` becomes `showSettingsDialog(QWidget *parent, const SettingsContext &ctx)`, `SettingsContext` a plain aggregate of QObject pointers. `cpp/dock_layout.{h,cpp}` takes `CentralWidgets`, `buildCentralWidget` and the general dock reconciliation.

**ActionDefs** — `file.projectSettings` ("Project Settings…", File, no default).

**Layering row**

| Crate | May depend on | Qt/cxx-qt allowed |
|---|---|---|
| `e2e` | std, tempfile — no workspace crate; drives the built `app` binary over X11 | **No** |

Plus, under *Where logic may live*:
> **Which settings layer a value comes from** is `settings-model::scope`'s answer, not `app-config`'s and not the dialog's. `app-config` reads and writes two files and knows nothing about precedence; the dialog shows the origin `scope::origin` reports and never re-derives it (ADR-0022).

### F1 — Editor ergonomics

**New crate**

| Crate | Layer | Why not an existing crate |
|---|---|---|
| `edit-ops` | support | Comment toggling, expand/shrink selection, auto-indent and bracket pairing need **both** the text (`editor-core`) and the grammar (`syntax-core`). `editor-core` may not depend on `syntax-core` and must not start; passing "the comment token for this language" in from `bridge.rs` puts the join in the adapter, which is banned. Same situation as `settings-model`, same answer. Named `-ops` because it produces operations, not a persisted model. |

Depends on `editor-core` + `syntax-core`. Modules: `comment.rs`, `selection_expand.rs`, `indent.rs`, `pairs.rs`.

**`editor-core` — the load-bearing work**

- `src/selection.rs` — `Caret { anchor, head }` over byte offsets; `SelectionSet` with sort-and-merge normalisation; `add_caret`, `add_next_occurrence` (Ctrl+D), `column_block(...)` for Alt+Shift+drag, `collapse_to_primary` (Esc).
- `src/transaction.rs` — `Transaction { edits: Vec<TextEdit> }` applied descending, all-or-nothing, plus `map_carets(&SelectionSet, &Transaction) -> SelectionSet` so the caret set survives its own edit. Deliberately shaped like `lsp_core::workspace_edit::apply_to_text`, which already uses the same descending all-or-nothing discipline.
- `src/line_ops.rs` — duplicate, move up/down, delete, join; each a pure `(text: &str, &SelectionSet) -> Transaction`. **`&str`, not `&Document`** — the rope is one save behind the widget (§1), and this is the same stateless shape `findMatches`/`replacementsFor` already use.
- `src/save_rules.rs` — trim trailing whitespace, insert final newline, line-ending normalisation, applied in the save path.

**Other crates** — `app-config` gains `[editing]` (`tab_width`, `use_spaces`, `trim_trailing_whitespace`, `insert_final_newline`, `wrap_column`, `default_encoding`, `line_endings`) plus a `[editing.languages.<id>]` table, all project-scopable per F0. `settings-model` gains `src/editing.rs` (`EditingDraft`, `validate`, `resolve_for_language`) — which languages *may* override which field is a rule and lives here. `lsp-core` gains `formatting.rs`.

**QObjects**

- `EditorOps` (new, no threading): every slot takes the buffer text as a parameter alongside the tab id — `addCaretAt`, `selectNextOccurrence`, `caretCount`, `carets -> Vec<FfiCaret>`, `columnSelect`, `clearSecondaryCarets`, `typeText -> Vec<FfiTextEdit>`, `backspace`, `lineOp(kind)`, `toggleComment(block)`, `expandSelection`, `shrinkSelection`, `indentSelection(outdent)`, `matchingBracket`, `indentForNewline`. Signal `caretsChanged(u64)`.
  Caret state itself lives in a `registry.rs` entry keyed by `TabId` — not on `Document` (which is stale) and not in `AppSession` (which may not know about carets), so `EditorOps`, `DocumentManager` and the widget all read one source.
  **The cost to watch is not caret arithmetic — it is transcoding the whole buffer UTF-16→UTF-8 on every keystroke.** Risk #8's benchmark must therefore run 1024 carets *on a large file*, not on a small one, or it measures nothing.
- `EditingEditor` (new settings-page draft) — isomorphic to `LanguageServerEditor` (`bridge.rs:2525`).
- `LanguageService` gains `requestFormatting(path, tabId, rangeOnly)` + `formattingReady()`; edits return through the existing pending-edit protocol, so reformat is undoable exactly like a rename.

**C++** — `code_editor.{h,cpp}` grows secondary-caret painting, extra selections, Alt+Click and Alt+Shift+drag, and forwards text-producing keys to `EditorOps` when `caretCount > 1`. It decides nothing: it asks for a `Vec<FfiTextEdit>` and splices.
Three input details the plan must not discover late: the multi-caret branch sits **after** the existing completion-popup interception (`code_editor.cpp:163-186`) and **suppresses** the per-printable-char `completionRequested` (`:198-214`), or 200 carets fire 200 LSP completions per keystroke; only `keyPressEvent`'s printable/backspace/delete/newline cases route through Rust; and since there is no `inputMethodEvent` override anywhere in `ui-shell` today, **`inputMethodEvent` collapses to the primary caret first** — dead keys and CJK composition are not multi-caret operations.
`cpp/editing_page.{h,cpp}` is the new settings page.

**ActionDefs**

| id | Label | Category | Default |
|---|---|---|---|
| `edit.selectNextOccurrence` | Select Next Occurrence | Edit | `Ctrl+D` |
| `edit.addCaretAbove` | Add Caret Above | Edit | `Ctrl+Alt+Up` |
| `edit.addCaretBelow` | Add Caret Below | Edit | `Ctrl+Alt+Down` |
| `edit.toggleLineComment` | Comment with Line Comment | Edit | `Ctrl+/` |
| `edit.toggleBlockComment` | Comment with Block Comment | Edit | `Ctrl+Shift+/` |
| `edit.duplicateLine` | Duplicate Line or Selection | Edit | `Ctrl+Alt+D` |
| `edit.moveLineUp` | Move Line Up | Edit | `Alt+Shift+Up` |
| `edit.moveLineDown` | Move Line Down | Edit | `Alt+Shift+Down` |
| `edit.deleteLine` | Delete Line | Edit | `Ctrl+Y` |
| `edit.joinLines` | Join Lines | Edit | `Ctrl+Shift+J` |
| `edit.expandSelection` | Extend Selection | Edit | `Ctrl+W` |
| `edit.shrinkSelection` | Shrink Selection | Edit | `Ctrl+Shift+W` |
| `edit.matchingBracket` | Go to Matching Bracket | Edit | `Ctrl+]` |
| `code.reformat` | Reformat Code | Code | `Ctrl+Alt+L` |

Tab / Shift+Tab stay widget-level key handling, not rebindable actions — they are contextual on whether a selection exists, and rebinding Tab breaks completion.

**Layering row**

| Crate | May depend on | Qt/cxx-qt allowed |
|---|---|---|
| `edit-ops` | `editor-core`, `syntax-core` (+ std) | **No** |

### F2 — Intentions and LSP surface completion

**New crates: none.** This is `lsp-core` growing the protocol surface it was built for.

- `lsp-core` — new `signature_help.rs`, `document_highlight.rs`, `inlay_hint.rs`; `intentions.rs` assembling the ordered, grouped list from diagnostic-scoped and range-scoped `codeAction` replies; `workspace_edit.rs` extended with `ResourceOp { Create, Rename, Delete }` parsing and ordering (resource ops before text edits targeting their result), and `client_capabilities()` finally advertising `"resourceOperations": ["create","rename","delete"]` (`manager.rs:993`).
- `app-core` — `FileOp { Create{path, overwrite, ignore_if_exists}, Rename{from, to, overwrite}, Delete{path, recursive, ignore_if_not_exists} }` and `apply_file_ops(&[FileOp]) -> Result<(), AppError>`, which performs the filesystem work **and** retargets open tabs: a renamed open file keeps its `TabId` and its dirty state. `app-core` may not depend on `lsp-core`, so `bridge.rs` maps `ResourceOp → FileOp`. That is type translation, which is what the adapter is for.
- `index-core` — the index must learn a file moved; `SearchModel`'s watcher already covers it, so this is a verification task, not a code task.

**QObjects** — `LanguageService` gains `requestIntentions(path, tabId, position, revision)`, `intentions() -> Vec<FfiIntention>` (`{title, kind, group, preferred, index}`), `applyIntention(index)`, `requestSignatureHelp`, `signatureHelp()`, `requestDocumentHighlights`, `documentHighlights()`, `requestInlayHints(path, tabId, firstLine, lastLine)`, `inlayHints()`, `organizeImports`. Signals `intentionsReady(u64)`, `signatureHelpReady()`, `documentHighlightsReady()`, `inlayHintsReady()`.
Cancellation is a `u64` generation per request kind, bumped on every caret move — the `QueryGuard` pattern at `bridge.rs:4507`, reused rather than reinvented. A stale reply is dropped before it is ever queued.

**C++** — `cpp/intention_bulb.{h,cpp}` (16×16 overlay child of the viewport at the caret line's left margin, click or Alt+Enter opens a `QMenu` grouped quickfix/refactor/source); `cpp/signature_tip.{h,cpp}` (frameless tooltip, active parameter bolded, driven by `(` and `,`, dismissed on `)` or Esc, reusing the hover-tooltip placement code). Inlay hints paint in `code_editor.cpp`, off by default.

**ActionDefs**

| id | Label | Category | Default |
|---|---|---|---|
| `code.showIntentions` | Show Intention Actions | Code | `Alt+Return` |
| `code.parameterInfo` | Parameter Info | Code | `Ctrl+P` |
| `code.optimizeImports` | Optimize Imports | Code | `Ctrl+Alt+O` |
| `code.toggleInlayHints` | Show Inlay Hints | Code | *(none)* |

**Layering note**
> **Which files a workspace edit creates, renames or deletes** is parsed and ordered by `lsp-core`; *performing* it — and deciding what happens to a tab whose file moved under it — is `app-core`'s, as `FileOp`. `bridge.rs` maps one type to the other and decides nothing (ADR-0026).

### F3 — Git v1

**New crate**

| Crate | Layer | Why not an existing crate |
|---|---|---|
| `vcs-core` | support | Repository discovery, status, hunks against HEAD, staging, commit, branch and remote operations, blame, history. Nothing in the existing fourteen crates is about version control, and `app-core` may not grow it — that would drag `gix` into the application layer and into `editor-core`'s dependents. Depends on `editor-core` for `diff` and offset conversion. |

**`editor-core` gains `src/diff.rs`** — line diff via `imara-diff` plus intra-line word diff, producing `Hunk { old_range, new_range, kind }` and `InlineSpan`. `imara-diff 0.2`'s `Diff::hunks() -> Hunk { before: Range<u32>, after: Range<u32> }` with `is_added`/`is_removed` is already the gutter's shape, so this type is nearly free. Note the workspace will carry two builds of the same engine — `gix` brings `gix-imara-diff` transitively — and that is the deliberate price of ADR-0028: `editor_core::diff` must not depend on `gix`, or a rename preview would need a repository.
It lives here, not in `vcs-core`, because the refactor preview, the project-wide replace preview and the AI apply flow all need a diff and none of them is about Git.
That is what makes the viewer reusable rather than a Git widget.

**A diff opens as a tab, not a dock** (ADR-0020's mechanism), but **not as `TabKind::Diff { left_label, right_label }`** — an earlier draft assumed that was additive and it is not. `TabKind` is a fieldless `Copy` enum with a stable FFI integer (`app-core/src/lib.rs:162-181`, `code(self)`, consumed at `bridge.rs:4121`), and `TabContent` (`:187`) makes every tab path-backed: `path()`, `set_path()` and `title()` are total functions used by rename retargeting and delete flagging. A diff tab has two texts and no single path, so a payload variant would make `TabKind` non-`Copy`, change `code()`'s signature and force those accessors fallible.
Keep `TabKind` fieldless, add the variant, and carry the labels **out of band** via a `diff_labels(TabId)` lookup. Smaller change, no accessor churn.

**Backend (ADR-0031): hybrid, with a sharper rule than "hybrid"**

Anything honouring the user's configuration, credentials, hooks or signing **shells out to `git`**: fetch, pull, push, commit, add / `apply --cached` (including per-hunk staging via a generated patch), checkout, branch, `merge --ff-only`.
Pure reads of object and index state use **`gix`**: discovery, HEAD resolution, reading the HEAD blob for a path, status, ref listing, log for file history.
Hunks are computed in-process with `imara-diff` against the blob `gix` read — never by a subprocess, because that path runs on every keystroke.
Blame is `git blame --porcelain` parsed in `vcs-core`, off-thread and cached: `gix`'s blame is young, blame is not hot, and the CLI's rename-following is better than anything we would write.

Checked against `gix 0.87.0` rather than assumed: discovery, HEAD-tree/blob-by-path and ref listing are ungated; **`Repository::status(progress)` is a real, documented, non-experimental API** (`src/status/mod.rs:99`) covering both index↔worktree (with `untracked_files`, dirwalk, optional rename detection) and index↔HEAD, cancellable and parallelised — **status does not need to shell out**; and there are **no C dependencies** (0.87 routes compression through `gix-zlib` → `zlib-rs`, pure Rust), so MXE is safe. Take it with `default-features = false` and the read-only feature set (`max-performance-safe`, `sha1`, `status`, `revision`, `blob-diff`, `index`, `dirwalk`, `excludes`, `attributes`) — the defaults drag in write paths this layer should not have.
**The one real gap: there is no path-filtered revwalk.** `rev_walk().selected(pred)` takes a commit-id predicate, and no pathspec exists anywhere in `src/revision/`. F3-10's file history is therefore ~40 lines of walk-and-compare (`lookup_entry_by_path` on parent vs child, compare oids) — doable, but a task, not an API call.

**QObjects** — `VcsService` (Threading; the `lsp-core` two-thread shape — job thread owning the repository handle consuming `Receiver<VcsJob>`, event thread draining into `queue()`, shutdown by dropping the job `Sender`): `isRepository`, `refreshStatus`, `changedFiles`, `hunks(path)`, `stageHunk`, `unstageHunk`, `stageFile`, `revertHunk -> Vec<FfiTextEdit>`, `commit(message, amend)`, `branches`, `checkout`, `createBranch`, `deleteBranch`, `fetch`, `pull`, `push(setUpstream)`, `fileHistory`, `blame`, `diffAgainstHead`. Signals `statusChanged`, `hunksChanged(path)`, `branchChanged`, `vcsFailed(FfiResult)`, `blameReady`, `historyReady`.
No second QObject for the diff viewer: it is given two texts and a `Vec<FfiHunk>`.

**C++**

- `cpp/diff_view.{h,cpp}` — **the reusable component.** Two read-only panes, one shared vertical scroll model, a change ribbon, intra-line highlighting, F7/Shift+F7. Constructor takes `(leftText, rightText, hunks, languageId)` and nothing else, which is what makes it retrofittable.
- `cpp/vcs_gutter.{h,cpp}` — change markers in `CodeEditor`'s existing gutter, hunk popup with Revert / Show Diff / Stage.
- `cpp/changes_panel.{h,cpp}` — dock: staged/unstaged/untracked trees, per-file and per-hunk checkboxes, commit box, Commit / Commit and Push / Amend.
- `cpp/file_history_panel.{h,cpp}`, `cpp/blame_gutter.{h,cpp}`, branch widget in the status bar.
- **Retrofits, in the same feature**: `refactor_preview_dialog.cpp` and the project-wide replace preview swap `previewText`'s single-line rendering (`main_window.cpp:2177`) for `DiffView`; the AI panel's per-block Apply gets the same preview.

**ActionDefs** — `vcs.commit` (`Ctrl+K`), `vcs.push` (`Ctrl+Shift+K`), `vcs.pull`, `vcs.fetch`, `vcs.branches` (``Ctrl+Shift+` ``), `vcs.showDiff` (`Ctrl+Alt+G`), `vcs.rollbackHunk`, `vcs.nextChange` (`F7`), `vcs.previousChange` (`Shift+F7`), `vcs.annotate`, `view.changes` (`Alt+9`), `view.vcsHistory`.

**Layering row**

| Crate | May depend on | Qt/cxx-qt allowed |
|---|---|---|
| `vcs-core` | `editor-core` (+ std, gix, imara-diff, serde) | **No** |

Plus:
> **Which Git operations run in-process and which shell out** is `vcs-core`'s and is stated in ADR-0031. `ui-shell` never spawns `git`.
> **What a diff is** is `editor_core::diff`'s — line hunks and intra-line spans over two texts, with no Git in the type. That is why the same viewer serves a rename preview, a replace preview and a commit.

### F4 — Run configurations and console

**New crate**

| Crate | Layer | Why not an existing crate |
|---|---|---|
| `run-core` | support | The configuration model and its persistence, project detection, the launch spec, process supervision, output batching, and the file:line link catalogue. `pty-core` is a transport and stays one; `terminal-core` is VT100 state and stays that. A configuration model in either gives the terminal a reason to know about `Cargo.toml`. |

Depends on `pty-core` + `app-config`.

- `pty-core` — `ShellSpec` gains `cwd: Option<PathBuf>` and `env: Vec<(String, String)>` (additive to the inherited environment; `clear_env` out of v1). Stays plain data with no OS probing, as its doc comment promises. `PtySession` gains `kill_tree()`: Unix puts the child in its own session with `setsid` in `pre_exec` and kills with `killpg`; Windows assigns a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Both tested by spawning a shell that spawns a sleeping grandchild and asserting the grandchild dies.
- `app-config` — `[run]` in the *project* layer: an ordered list of `RunConfigSetting`. Environment values stored literally; a config referencing a secret is the user's problem and the docs say so.
- `app-core` — no change beyond F2's `FileOp`. Run configurations are not the application layer's business; `ui-shell` joins `run-core` the way it joins `index-core`.

**DAP readiness** — `run_core::launch::LaunchSpec { program, args, cwd, env, console: Pty | Pipes }` is the shared half of a launch configuration.
Today `RunConfig → LaunchSpec → PtySession`; a future `dap-core` takes the same `LaunchSpec` and turns it into a DAP `launch` request body.
Supervision is `lsp-core`'s shape, deliberately. Nothing about the debugger is built; the seam it attaches to is.

**QObjects** — `RunService` (Threading): `configurations`, `detectConfigurations`, `run(configId)`, `stop(consoleId)`, `rerun`, `consoles`, `closeConsole`, `activeConfiguration`, `setActiveConfiguration`, `resolveLink(text, byteOffset)`. Signals `consoleStarted(u64)`, `consoleOutput(u64, QString)` (batched), `consoleFinished(u64, i32)`, `configurationsChanged()`.
`RunConfigEditor` — the dialog's draft object, isomorphic to `LanguageServerEditor`.
`TerminalSession` gains `resolveLink(...)` routed to the same `run_core::links` catalogue, so console and terminal linking are one rule with two surfaces.

**C++** — `cpp/run_console_panel.{h,cpp}`: a dock holding a `QTabWidget` of consoles, each a **read-only `QPlainTextEdit`** with `setMaximumBlockCount(N)` (that is §5's ring buffer, in one line), fed `rust::Vec<FfiStyledRun { text, fg, bg, bold, … }>` parsed from ANSI SGR **in Rust** and appended with `QTextCursor::insertText(text, QTextCharFormat)`. Scrollback, scrollbar, wheel, selection, copy, find, wrap and accessibility come free, the panel stays ~100 lines, and the parsing is testable where `CLAUDE.md` requires. Not `QSyntaxHighlighter` — SGR state crosses line boundaries and the highlighter model fights that.
An earlier draft said this would reuse `terminal_widget`'s grid painter. It cannot: `paintEvent` (`terminal_widget.cpp:100-151`) is one inline double loop reading `gridRows()`/`gridCells()`/`cursorRow()` straight off the QObject with no separable painter, the widget also *drives* the session (`syncGridSizeToWidget` calls `session_->start(rows, cols)` from `showEvent`/`resizeEvent`), and **there is no scrollback anywhere in the stack** — `terminal-core/src/lib.rs:440-448` and `:514` carry `debug_assert`s pinning "viewport row → Line mapping assumes no scrollback". That reuse would cost a painter extraction, scrollback in `terminal-core`, a scroll offset in the grid model and selection over scrollback coordinates: the hard half of a terminal emulator, to render read-only output.
Where reuse **is** real: `terminal-core`'s SGR→RGB resolution, and the link detection behind `linkAt()` (`terminal_widget.cpp:182-202`) — both already Qt-free, and both are what F4-8 extends rather than replaces.
Plus `cpp/run_config_dialog.{h,cpp}` and `cpp/run_toolbar.{h,cpp}`.

**ActionDefs** — `run.run` (`Shift+F10`), `run.stop` (`Ctrl+F2`), `run.rerun` (`Ctrl+F5`), `run.selectConfiguration` (`Alt+Shift+F10`), `run.editConfigurations`, `view.runConsole` (`Alt+4`), `terminal.newSession` (`Ctrl+Shift+T`), `terminal.selectShell`.

**Layering row**

| Crate | May depend on | Qt/cxx-qt allowed |
|---|---|---|
| `run-core` | `pty-core`, `app-config` (+ std, serde, toml, regex, libc/windows-sys for kill-tree) | **No** |

Plus:
> **What a run configuration is, how one is detected from a project, and what a line of console output links to** live in `run-core`. `pty-core` stays a transport that spawns what it is told (ADR-0029); `terminal-core` stays VT100 state. The terminal's Ctrl+Click uses `run_core::links` rather than a second regex table, for the same reason there is one language registry (ADR-0018).

---

## 3. ADRs to write

Numbering continues from 0021; 0006 and 0013–0015 remain intentional gaps.

**ADR-0022 — Per-project settings: a sparse project layer, resolved by `settings-model`.**
A project's `.ide/settings.toml` deserialises into a **sparse** `ProjectSettings` (every field `Option`) rather than a second `Settings`; precedence is computed by `settings_model::scope::resolve`, which also answers where each effective value came from so the dialog can label it.
Project scope covers project-shaped settings (editing behaviour, language servers, run configurations, index excludes); global scope keeps person-shaped ones (theme, fonts, keymap, AI providers).
`.ide/settings.toml` is meant to be committed; machine-local state (window layout, open tabs) stays global.
*Rejected*: reusing `Settings` for both layers (every field is `#[serde(default)]`, so "0" and "unset" are the same value and the project layer would silently zero the global one); merging in `app-config` (dumb persistence by ADR-0017 — precedence is a rule needing a vocabulary); one flat file with `[project.*]` sections (unshareable, defeating the point); everything overridable per project (a project that forces your theme is hostile); `.editorconfig` as the layer (out of F1's scope and covers a fraction — revisit as an *importer*).

**ADR-0023 — Multi-caret: a selection set and edit transactions in `editor-core`, and no second undo stack.**
`SelectionSet` and `Transaction` live in `editor-core`; every multi-caret operation — including a single keystroke — is computed in Rust as one `Transaction`, crosses as `Vec<FfiTextEdit>`, and is spliced by the existing `EditorTabs::applyBufferEdits` inside one `beginEditBlock`. Undo remains `QTextDocument`'s. Carets are byte offsets in Rust, mapped through the transaction that moved them.
**Offsets**: bytes inside `edit-ops` (tree-sitter is byte-addressed, so expand-selection wants them), **UTF-16 code units at the seam** — which is what `FfiTextEdit` already carries (`bridge.rs:402-416`) and what `editor_core::search::TextMatch` already speaks (`search.rs:29`). Converted at exactly one place: `Utf16Cursor`/`utf16_at` (`editor-core/src/search.rs:178-201`) is promoted from private to `editor_core::offsets` and reused by `SelectionSet`, the diff, the column fix, `run-core::links` and the VCS gutter. Without that promotion it gets reimplemented five times.
*Rejected*: a Rust-side undo stack (two stacks that must agree forever; the existing one already gives ADR-0019 its "one Ctrl+Z undoes a refactoring" property); carets owned by the view as a `QTextCursor` list (Ctrl+D's next-occurrence and column selection are rules); per-caret sequential edits (offsets shift under each other and undo becomes N steps — the transaction *is* the feature); a byte-addressed seam (the rope is **char**-indexed — `insert(char_idx)`, `delete(char_range)`, `char_count()` at `editor-core/src/lib.rs:105-121` — and the seam is already UTF-16; an earlier draft of this ADR claimed the opposite and was wrong).

**ADR-0024 — The verification foundation: a headless E2E harness and an opt-in real-server conformance gate.**
An `e2e` workspace member drives the built binary under Xvfb with xdotool, `#[ignore]`d so `cargo test --workspace` is unaffected, run by `make e2e` and a CI job, capturing screenshots and uploading them on failure.
Real language servers live in a separate `linux-lsp` Docker stage layered on `linux-builder`, exercised by `lsp-core/tests/real_server_conformance.rs` in a nightly/manual job, not per PR.
*Rejected*: shell scripts under `scripts/` (the settle discipline is logic that has already produced silently wrong measurements once, so it gets unit tests and is therefore Rust); servers in `linux-builder` (pyright drags in a Node runtime, and that image is rebuilt by every developer and every CI run); real servers per PR (minutes of runtime plus upstream behaviour changes — a red PR that is nobody's fault trains people to ignore red); Qt Test / Squish driving widgets directly (tests the widgets, not the application, and `cpp/` is untested by design).

**ADR-0025 — The seam split and a file-size ceiling.**
`bridge.rs` keeps **one** `#[cxx_qt::bridge] mod ffi` containing only declarations, moved to `src/bridge/ffi.rs`; every `…Rust` struct and `impl` moves to a per-feature module under `src/bridge/`. `main_window.cpp` splits by class into one translation unit each. Ceiling of **1500 lines per Rust module, 1200 per C++ TU**, enforced by `scripts/check-file-size.sh` in `make lint` and CI. Details in §6.
*Rejected*: one `#[cxx_qt::bridge]` per feature (shared FFI structs are per-bridge in cxx-qt, so two bridges declaring `FfiResult` produce two distinct C++ types — an FFI-shape change disguised as a mechanical refactor; revisit only if `ffi.rs` alone outgrows its ceiling); leaving it until it hurts (it already hurts — `showSettingsDialog` takes 14 parameters, and F1–F4 add three QObjects, four docks and two pages; splitting after is a merge conflict with every feature branch); a ceiling as a convention in `CLAUDE.md` (conventions that are not gates rot; the gate is nine lines of shell).

**ADR-0026 — Workspace-edit resource operations, performed by `app-core` as `FileOp`.**
Advertise `resourceOperations`, parse and order them in `lsp-core`, perform them in `AppSession::apply_file_ops`, which also retargets open tabs. `bridge.rs` maps one type to the other.
*Rejected*: continuing to refuse them (rust-analyzer's "move to module", TypeScript's file rename and every Java extract-to-file are refused *whole* today — the user sees "unsupported" for a correct edit); performing them in `lsp-core` (it would own tab policy, which is `app-core`'s, and `app-core` may not depend on `lsp-core` to get it back); performing them in `bridge.rs` (deciding what happens to an open dirty buffer whose file was renamed is a rule and deserves tests). All-or-nothing across resource ops and text edits is *kept*: a failed resource op aborts before any text edit is written, matching ADR-0019's "half an extract is a corrupted program".

**ADR-0031 — Git backend: `gix` for reads, the `git` binary for anything touching the user's world.** (Written as ADR-0031, not 0027 as first planned here — 0027 through 0030 were claimed by other work in between; see ADR-0030's own note for the same renumbering.)
As argued in §2/F3.
*Rejected*: pure `gix`/`git2` (credential helpers, SSH agents, `insteadOf`, hooks and GPG signing are five re-implementations, each failing in a way that looks like our bug and some by leaking); pure `git` CLI (a subprocess per keystroke for gutter diffs is not a ceiling, it is a defect); `git2`/libgit2 over `gix` (a C dependency in an MXE cross-build — ADR-0021 already refused OpenSSL on exactly this ground); bundling a `git` binary (then we own its CVEs and platform builds, and the user's configured `git` is the one they want used).

**ADR-0030 — One diff component: `editor_core::diff`, `DiffView`, and `TabKind::Diff`.**
(Written as ADR-0030, not 0028 as first planned here — 0025 through 0029 were claimed by other work in between.)
Diff computation is Git-free and lives in `editor-core`; the C++ `DiffView` takes two texts, a hunk list, intra-line spans and a language id.
The refactor preview and Replace in Files are retrofitted onto it within F3 (F3-13/F3-15); `TabKind::Diff` (F3-14) is deferred until the Git backend exists, so it has a real caller.
*Rejected*: diff in `vcs-core` (then a rename preview needs Git to show a diff, and a project with no repository gets no preview); a diff dock (ADR-0020 gave a tab an explicit kind for exactly this, and a dock cannot hold several open comparisons); a `QWebEngineView` diff (same answer ADR-0021 gave — hundreds of megabytes and a second JS runtime for two text panes); shipping the Git gutter now and generalising later ("later" is how the replace preview ended up with no undo and no diff in the first place).

**ADR-0029 — Run configurations, `LaunchSpec`, and a supervisor shaped for DAP.**
`run-core` owns the configuration model, detection, the `LaunchSpec` seam, the PTY-backed supervisor and the link catalogue; `pty-core` gains `cwd`, `env` and `kill_tree()` and stays a transport. Processes run on a PTY, not pipes.
*Rejected*: pipes (programs buffer differently and disable colour when stdout is not a tty — a console that shows nothing until the process exits is the classic failure, and a working PTY already exists); configurations in the global `settings.toml` (a run configuration is the definition of a project, not a preference — this is the main reason F0-9 is in F0); reusing `TerminalSession` for the console (different lifecycle, different affordances, no configuration model — they share the transport and the link rule, which is the right amount); building a build-system model now (out of scope — detection reads `Cargo.toml`/`package.json`/`Makefile` for *names* and produces plain editable configurations); designing the DAP client now (YAGNI — `LaunchSpec` and the supervisor shape are the only things expensive to retrofit, and both fall out of doing F4 properly).

---

## 4. Task breakdown

Every task is a single commit that keeps `cargo test --workspace` green and is reviewable on its own.

### F0 — Verification foundation

| # | Task | Depends on |
|---|---|---|
| F0-1 | File-size gate: `scripts/check-file-size.sh`, wired into `make lint` and CI; `ffi.rs` exempt outright (3079 lines, one `mod ffi` is a cxx-qt requirement), current offenders grandfathered until F0-4/F0-6 remove them | — |
| F0-1b | Promote `Utf16Cursor`/`utf16_at` (`editor-core/src/search.rs:178-201`) to a public `editor_core::offsets`, so the byte↔UTF-16 conversion has one home before five features reimplement it | — |
| F0-2 | `bridge.rs` split part 1: `bridge/mod.rs` + `bridge/ffi.rs`; `tree.rs`, `editor.rs`, `settings.rs`, `terminal.rs`, `convert.rs` moved | — |
| F0-3 | `bridge.rs` split part 2: `search.rs`, `language.rs`, `ai/chat.rs` + `ai/agent.rs`, `registry.rs` moved; the mixed `mod tests` divided; `bridge.rs` deleted | F0-2 |
| F0-4 | `main_window.cpp` split part 1: `editor_tabs`, `class_view_panel`, `find_usages_panel`, `ide_main_window` extracted with their shared-helper headers, registered in `build.rs`. Split in two on delivery: **F0-4a** moved `EditorTabs` (and the cursor/highlighter helpers only it used) to `cpp/editor_tabs.{h,cpp}` plus `cpp/editor_tabs_panes.cpp` and `cpp/editor_tabs_lsp.cpp` — one class, three translation units, because the class does not fit under the 1200-line ceiling and adding a baseline would defeat F0-1. **F0-4b** is the rest: `ClassViewPanel`, `FindUsagesPanel`, `IdeMainWindow` | F0-12 |
| F0-5 | `main_window.cpp` split part 2: `refactor_controller`, `declaration_navigator`, `action_registry` extracted. Delivered as two units, not three: `RefactorController` moved to `cpp/refactor_controller.{h,cpp}` (taking the free `previewText` helper, declared in that header because the AI panel's Apply path in `main_window.cpp` calls it too) and `DeclarationNavigator` to `cpp/declaration_navigator.{h,cpp}`. There was no `action_registry` left to extract — P6 had already moved `registerAction`/`applyKeymap` and the `QHash<QString, QAction *>` bookkeeping to `cpp/keymap_page.{h,cpp}`, leaving only the menu-building call sites, which go with the settings dialog in F0-6 | F0-4b |
| F0-6 | Settings dialog extraction + `SettingsContext`; the 14-parameter signature replaced; one `buildXPage` per page. Delivered as `cpp/settings_dialog.{h,cpp}` holding the category list, the stack and what OK and Cancel mean across the pages, plus the last two pages that were still built inline: `cpp/editor_page.{h,cpp}` (the editor font and the three editor colours, with the `commit`/`revert` pair `appearance_page.*` already models) and `cpp/mcp_page.{h,cpp}` (a `commit` and no `revert`, because a page that never applies live has nothing to undo). `main_window.cpp` drops from 1494 to 1198 lines and its `check-file-size.sh` baseline is deleted, which empties the C++ half of F0-1's grandfather list | F0-5 |
| F0-7 | Dock registry and general reconciliation in `dock_layout.{h,cpp}`; `showAiChatDock`'s workaround deleted | F0-6 |
| F0-8 | Byte-column fix **inside `moveCursorToLine`** (`main_window.cpp:128-140`): convert byte column → UTF-16 column against `block.text()` via `editor_core::offsets`, and clamp to the block (`QTextCursor::Right` does not stop at a block boundary). One place fixes all five call sites (`:1935`, `:2065`, `:2622`, `:3200`) **and** `jumpToByteOffset` (`:685-704`). Routing through `AppSession` cannot work — the conversion needs the *live* line text, and the rope is stale (§1) | F0-1b |
| F0-9 | Per-project settings persistence: `ProjectSettings`, `.ide/settings.toml` load/save, atomic write, absent- and malformed-file behaviour | — |
| F0-10 | Per-project settings rules + dialog: `scope::{resolve, origin}`, the field split, `AppSettings` scope surface, scope selector and origin badges, `file.projectSettings` | F0-6, F0-9 |
| F0-11 | E2E harness: `crates/e2e` — Xvfb/xdotool driver, seeded `XDG_CONFIG_HOME`, change-then-stability settle, per-step screenshots, `make e2e`, `make e2e-repeat` | — |
| F0-12 | E2E seed flows (4): open→edit→save; Search Everywhere→jump; rename with preview; split-editor persistence across restart | F0-11 |
| F0-13 | E2E in CI, uploading `target/e2e-artifacts/**` on failure | F0-12 |
| F0-14 | `linux-lsp` image stage with **rust-analyzer only**, version-pinned; `make lsp-conformance`. rust-analyzer alone finds the position-encoding class (§8's own "highest-value assertion") and the `codeAction` shape F2 depends on; pyright and clangd are added in F2, when their divergence is what is actually being tested | — |
| F0-15 | LSP conformance harness + expectations TOML + `docs/architecture/lsp-conformance.md`; nightly CI job | F0-14 |
| F0-16 | Conformance fixes — one commit per defect. **Unbounded by nature**: F0-15 produces the list, and each entry becomes its own Progress row rather than hiding behind this one | F0-15 |
| F0-17 | Docs: ADR-0022/0024/0025, this plan, `layering.md`, `overview.md`, `docs/README.md`; stale `lib.rs` doc comments and `settings-model`'s wrong ADR citation fixed in the same pass | all F0 |

Lanes: `{F0-11→F0-12→F0-4a→F0-4b→F0-5→F0-6→F0-7}` is the critical path — the seed flows are the regression net for the C++ split, so they gate F0-4 rather than depending on F0-7 (an earlier draft had that edge and it made a cycle). Running alongside: `{F0-2→F0-3}`, `{F0-14→F0-15}`, `F0-1`, `F0-1b→F0-8`, `F0-9`. They converge at F0-10 and F0-13.

Two more F0 rows worth their own commits: **F0-18**, route `FindBar::replaceCurrent` (`find_bar.cpp`) and `CodeEditor::insertCompletion` (`code_editor.cpp`) through `applyEditsTo` so §5's "every buffer change crosses as `Vec<FfiTextEdit>`" is true rather than aspirational, and fix the latent bug in `EditorTabs::applyBufferEdits` (`editor_tabs_lsp.cpp`, extracted from `main_window.cpp` since this plan was written) where `beginEditBlock`/`endEditBlock` run on two different `textCursor()` **copies** (it works only because Qt's counter is per-`QTextDocument`, and it is the first thing a reviewer of the multi-caret change will stare at). **F0-19**, an ADR-0003 amendment introducing the error-code ranges below, replacing the hardcoded `code: 1` literals — twenty-five of them once `bridge.rs` had been split, not the five this line first counted.

### F1 — Editor ergonomics

| # | Task | Depends on |
|---|---|---|
| F1-1 | `editor-core::selection`: `Caret`, `SelectionSet`, normalisation and merge, primary caret, `collapse_to_primary` | — |
| F1-2 | `editor-core::transaction`: `Transaction`, descending all-or-nothing application, `map_carets` | F1-1 |
| F1-3 | Next occurrence + column selection; the 1024-caret ceiling and its refusal | F1-2 |
| F1-4 | `editor-core::line_ops`: duplicate, move up/down, delete, join — multi-caret-aware, one `Transaction` each | F1-2 |
| F1-4b | `syntax-core` registry schema: comment tokens and bracket pairs on `LanguageDef`/`OwnedLanguageDef` (`registry.rs:65-79` has neither today), values for all 35 static languages, and the matching runtime `Manifest` keys — which **must** be `#[serde(default)]`, because the manifest is `deny_unknown_fields` (`runtime.rs:231`) and every user-installed grammar would otherwise break. Gated by the registry-driven test from §8 | — |
| F1-5 | `edit-ops` crate + comment toggle: tokens from the `syntax-core` registry, line and block, the already-commented rule | F1-2, F1-4b |
| F1-6 | Expand/shrink selection over the tree-sitter node tree, with the selection stack | F1-5 |
| F1-7 | Auto-indent and indent/unindent honouring tab width and spaces-vs-tabs | F1-5 |
| F1-8 | Smart typing: auto-close, type-over, smart backspace, surround-selection, in-string/in-comment suppression | F1-5 |
| F1-9 | Bracket match over the grammar with a text fallback, plus the jump | F1-5 |
| F1-10 | Editing settings: `app-config` `[editing]` + per-language table; `settings_model::editing` | F0-10 |
| F1-11 | Save rules: trim, final newline, line-ending normalisation, applied in the save path | F1-10 |
| F1-12 | `lsp-core` formatting: `formatting.rs`, `rangeFormatting`, capability advertisement, stub-server coverage | — |
| F1-13 | Bridge: `EditorOps` QObject, FFI structs, `caretsChanged` | F1-3, F1-4, F1-6…F1-9 |
| F1-14 | Bridge: `EditingEditor` + `requestFormatting` through the pending-edit protocol | F1-10, F1-12 |
| F1-15 | View: multi-caret rendering and input — secondary carets, extra selections, Alt+Click, Alt+Shift+drag, Esc, key forwarding when `caretCount > 1` | F1-13 |
| F1-16 | View: the 14 `ActionDef`s, `registerAction` calls, Edit and Code menus | F1-15 |
| F1-17 | View: `editing_page.{h,cpp}` registered in the dialog | F1-14, F0-6 |
| F1-18 | E2E (2 flows) + ADR-0023 + `layering.md`, `overview.md` | all F1 |

F1-12 is independent of everything else in F1; F1-6…F1-9 are four independent tasks once F1-5 lands; F1-10/F1-11 are independent of the caret work.

### F2 — Intentions and LSP surface completion

| # | Task | Depends on |
|---|---|---|
| F2-1 | Resource operations: parse, order, advertise; stub-server cases | — |
| F2-2 | Resource operations: `app_core::FileOp`, `apply_file_ops`, open-tab retargeting, all-or-nothing abort before any text edit | F2-1 |
| F2-3 | Resource operations: bridge mapping + preview listing created/renamed/deleted files as such | F2-2, F0-3 |
| F2-4 | `lsp-core::intentions`: diagnostic- and range-scoped assembly, grouping, `preferred` ordering, dedup | — |
| F2-5 | `lsp-core::signature_help`: request, reply shapes, active-parameter index, retrigger characters | — |
| F2-6 | `lsp-core::document_highlight` + `inlay_hint` | — |
| F2-7 | Organize imports as an action and as an offered quick-fix; the "which diagnostics qualify" rule in `lsp-core`, not the view | F2-4 |
| F2-8 | Bridge: intentions surface, debounce, generation cancellation reusing `QueryGuard` | F2-4, F0-3 |
| F2-9 | Bridge: signature help, highlights, hints, each with its own generation | F2-5, F2-6 |
| F2-10 | View: `intention_bulb.{h,cpp}`, grouped `QMenu`, `code.showIntentions` | F2-8 |
| F2-11 | View: `signature_tip.{h,cpp}`, occurrence painting, inlay-hint painting behind the toggle | F2-9 |
| F2-12 | E2E (2 flows, against the stub server) + ADR-0029 + `layering.md` | all F2 |

Two independent lanes: `{F2-1→F2-2→F2-3}` and `{F2-4…F2-7}`.

### F3 — Git v1

| # | Task | Depends on |
|---|---|---|
| F3-1 | `editor-core::diff`: line diff over `imara-diff`, intra-line spans, `Hunk`/`InlineSpan`, ceilings | — |
| F3-2 | `vcs-core` skeleton + discovery; `VcsError` with stable codes; "not a repository" as a first-class state | — |
| F3-3 | Status and HEAD reads via `gix`, behind `vcs-core::repo`'s own types | F3-2 |
| F3-4 | Working-tree hunks + the cache keyed by `(path, head_oid, revision)` + the size ceiling | F3-1, F3-3 |
| F3-5 | `vcs-core::cli`: argument construction, `GIT_TERMINAL_PROMPT=0`, timeouts, stderr → a readable sentence, "git not installed" | F3-2 |
| F3-6 | Staging: per-file and per-hunk via a generated patch fed to `git apply --cached` (`--reverse` to unstage) | F3-4, F3-5 |
| F3-7 | Commit with message, amend, staged selection; hook failures surfaced verbatim | F3-6 |
| F3-8 | Branches: list, create, checkout, delete with the force rule, current-branch reporting | F3-5 |
| F3-9 | Remotes: fetch, pull, push, `--set-upstream`; every failure a sentence, not an exit code | F3-5 |
| F3-10 | History (`gix` log) and blame (`git blame --porcelain`), both cached, both off the hot path | F3-3, F3-5 |
| F3-11 | Revert hunk as a `Vec<TextEdit>` against the open buffer — spliced, not written, so one Ctrl+Z undoes it | F3-4 |
| F3-12a | Bridge: `VcsService` supervisor skeleton + `isRepository`/`refreshStatus`/`changedFiles` + `vcsFailed` | F3-3, F0-3 |
| F3-12b | Bridge: `hunks`, `revertHunk` | F3-12a, F3-4, F3-11 |
| F3-12c | Bridge: staging and commit | F3-12a, F3-6, F3-7 |
| F3-12d | Bridge: branches, remotes, history, blame | F3-12a, F3-8…F3-10 |
| F3-13 | View: `diff_view.{h,cpp}` — synchronized scroll, ribbon, intra-line highlighting, F7/Shift+F7 | F3-1 |
| F3-14 | View: a fieldless `TabKind::Diff` in `app-core` + out-of-band `diff_labels(TabId)`, and the view building a `DiffView` from it | F3-13, F0-4 |
| F3-15 | Retrofit: refactor preview, project-wide replace preview and AI apply all render through `DiffView`; `previewText` deleted | F3-13 |
| F3-16 | View: `vcs_gutter.{h,cpp}`, hunk popup, revert/stage/show-diff | F3-12b |
| F3-17 | View: `changes_panel.{h,cpp}` — trees, checkboxes, commit box, Commit / Commit and Push / Amend | F3-12c, F0-7 |
| F3-18 | View: status-bar branch widget, branch menu, `file_history_panel`, blame gutter | F3-12d, F0-7 |
| F3-19 | View: the twelve `ActionDef`s and a VCS menu, not built at all when the project is not a repository | F3-16…F3-18 |
| F3-20 | E2E (2 flows over a seeded temp repo) + ADR-0027, ADR-0028 + `layering.md`, `overview.md` | all F3 |

`F3-1 → F3-13 → {F3-14, F3-15}` is fully independent of the Git lane and should be delivered **first** — see §7.

### F4 — Run configurations and console

| # | Task | Depends on |
|---|---|---|
| F4-1 | `pty-core`: `ShellSpec` gains `cwd` and `env`, threaded through `spawn` | — |
| F4-2 | `pty-core`: `kill_tree()` — `setsid`+`killpg` on Unix, Job Object on Windows; the grandchild test | F4-1 |
| F4-3 | `run-core` skeleton: `RunConfig`, `LaunchSpec`, `RunError` | F4-1 |
| F4-4 | Persistence in the project layer; ids stable across edits | F4-3, F0-9 |
| F4-5 | Detection: `Cargo.toml` bins/examples, `package.json` scripts, `Makefile` targets — names only | F4-3 |
| F4-6 | Supervisor: job/event thread pair, `RunJob`/`RunEvent`, start, stop, exit codes, concurrent runs | F4-2, F4-3 |
| F4-7 | Output batching: 16 ms / 64 KB coalescing, the ring buffer and its truncation notice | F4-6 |
| F4-8 | `run-core::links`: **extend the existing Rust link detection behind `linkAt()`** (`terminal_widget.cpp:182-202`) into a per-language catalogue rather than standing up a second table; resolution against the config's cwd, the "no such file" answer. Extending it means the terminal retrofit disappears | F4-3 |
| F4-9 | Bridge: `RunService`, FFI structs, streaming signals, `resolveLink` | F4-6…F4-8, F0-3 |
| F4-10 | Bridge: `RunConfigEditor` | F4-4 |
| F4-11 | View: `run_console_panel.{h,cpp}` — ANSI rendering, per-console tabs, Re-run/Stop/Clear, exit-code line | F4-9, F0-7 |
| F4-12 | View: `run_toolbar`, `run_config_dialog`, the eight `ActionDef`s | F4-10, F4-11 |
| F4-13 | View: link hit-testing and Ctrl+Click in the run console, jumping through `editorTabs->openFileAtLine` — the same jump path Find in Files/Find Usages/Problems already use (`AppSession::open_at_location` does not exist as such). **Terminal left alone**: `linkAt()`/`TerminalSession` already resolves URLs via a distinct, working, tested mechanism unrelated to `file:line` locations; retrofitting it to `run_core::links` was judged out of this branch's time budget | F4-9, F0-8 |
| F4-14a | Terminal multi-session, core: N `TerminalSession` QObjects instead of one, and moving grid sizing out of the widget — today `syncGridSizeToWidget` calls `session_->start/resize` from `showEvent`/`resizeEvent`, so N sessions need an owner and a shutdown order. This is a lifecycle change, not a `ShellSpec` change | F4-1 |
| F4-14b | Terminal multi-session, view: tab widget and shell picker, reusing `ShellSpec`'s Windows shell kinds | F4-14a |
| F4-15 | E2E (2 flows) + ADR-0029 + `layering.md`, `overview.md` | all F4 |

Four lanes: `{F4-1→F4-2→F4-6→F4-7}`, `{F4-3→F4-4/F4-5}`, `F4-8`, `F4-14`.

---

## 5. Cross-cutting concerns

**Threading.** Every feature uses one of the two proven shapes and adds no third.

| Feature | Shape | Why |
|---|---|---|
| F1 | None — synchronous | Caret arithmetic and line ops are microseconds on a rope. A thread adds a frame of latency to every keystroke to save nothing. |
| F2 | Existing LSP job thread + generation counter | Intentions are one more `LspJob` (`bridge.rs:6018`). Cancellation is the `QueryGuard` pattern (`:4507`), not a cancel message to the server. |
| F3 | The `lsp-core` two-thread supervisor | `VcsService` owns a job thread holding the repository handle (`gix` types are not `Sync` in a shape worth fighting) and an event thread draining into `queue()`. Subprocesses spawn on the job thread and never block the UI. |
| F4 | The same, plus one reader thread per console | Exactly `lsp-core`'s supervised-child shape, which is the point: it is the DAP template. |

No async runtime, no `moveToThread`, no `QThreadPool`. The `grep -i tokio` layering gate extends to `vcs-core`, `run-core`, `edit-ops`, `e2e`.

**Cancellation and debouncing.** Intentions: 150 ms debounce after the caret settles, generation bumped on every caret move, stale replies dropped before they are queued. Gutter diffs: 200 ms debounce after typing stops, cache keyed by `(path, head_oid, doc_revision)`, superseded computations abandoned. Blame and history: user-initiated, so not debounced; cancelled by an `Arc<AtomicBool>` when the tab closes, as the AI chat's cancel works (`bridge.rs:7671`). Run: stopping sets the flag, sends SIGTERM, waits 3 s, then `kill_tree()`.

**Error propagation.** Everything fallible returns `FfiResult { code, message }` (`bridge.rs:17`) via `to_ffi_result()` (`:3137`). No new error channel, no `QString` sentinels. Three new code ranges: `vcs-core` 700–799, `run-core` 800–899, `edit-ops` 900–999 — these collide with nothing, but they are a **new convention**: today the scheme is per-QObject 0-based enums that already overlap numerically (`AppError` 0–9 at `app-core/src/lib.rs:82-91`, `ChatError` 0–20 at `ai-chat-core/src/lib.rs:177-197`), disambiguated only by which QObject returned them. So ranges land as an ADR-0003 amendment, with the hardcoded `code: 1` literals cleaned up in the same commit (F0-19). Delivered slightly differently from this sketch: `edit-ops` has no error type of its own, so 900–999 is `editor-core`'s `SelectionError` for now; `lsp-core` took 600–699 and `ai-chat-core` moved to 100–199, since a `ChatError` of `1` and an `AppError` of `1` were the collision actually causing trouble; and the adapter's own refusals — "no project is open", "unknown console" — took 1000–1099 rather than being disguised as domain errors. `git`'s stderr becomes a sentence *in `vcs-core`* — a push rejected for a missing upstream says so and names the command that would fix it; the view never parses stderr.

**Undo.** One rule, restated for four new edit sources: anything that changes a buffer crosses the seam as `Vec<FfiTextEdit>` and is spliced inside one `beginEditBlock` per file. That covers a 200-caret keystroke, a comment toggle, a reformat, an intention, a reverted hunk and an applied AI block — each one Ctrl+Z. The one thing still not undoable is a write to a *closed* file, the ceiling ADR-0019 documented, now also reached by resource operations and by `git checkout`. Both must say so before they act.

**Performance ceilings.**

| Thing | Ceiling |
|---|---|
| Multi-caret | 1024 carets; beyond that the operation is refused with a message. One keystroke at 1024 carets applies in ≤ 16 ms. |
| Expand selection | Grammar node tree only; files past the existing highlighting ceiling get the text fallback. |
| Gutter diff | ≤ 50 ms for a 5000-line file; no markers above 100k lines or 5 MB (matching the large-files plan), and the gutter says "too large" rather than showing nothing. |
| Blame | ≤ 2 s for a 10k-line file, off-thread, cached per `(path, head_oid)`. |
| Console throughput | Batched at 16 ms / 64 KB; a producer faster than the UI loses frames, not output. Scrollback 10 000 lines or 8 MB, whichever first, with an explicit truncation notice. |
| Intentions | One in-flight request per surface; a caret move cancels rather than queues. |
| `make e2e` | ≤ 10 minutes wall clock for the whole suite. |

**Windows versus Linux.** Kill-tree is genuinely different (F4-2) and is the single largest Windows risk here. Line endings: F1-11's normalisation and F3's diff must both treat CRLF as one terminator; the diff compares normalised text and reports what it normalised. `git` on Windows may be Git-for-Windows with its own `sh`, so `vcs-core::cli` spawns `git` directly, never through a shell. Path case: the link resolver and the diff cache key canonicalise, and compare case-insensitively on Windows. The MXE cross-build takes no new C dependency — `gix` and `imara-diff` are pure Rust, which is half of why ADR-0031 chose them.

**Degraded states — all three are *states*, not errors.**
Not a Git repository: `isRepository()` is false, the VCS menu and both docks are not built at all, the gutter has no VCS column. No greyed-out menu of things that will never work.
`git` not installed: reads still work via `gix`; every mutating action reports "Git was not found on PATH" once, in the status bar, refused rather than half-done.
No language server: unchanged from ADR-0019 — intentions fall back to nothing, the bulb never appears, and reformat says which languages have no formatter. F0-15's report says exactly which of the seven surfaces each server implements, so this stops being folklore.
Huge file: the existing highlighting ceilings gate expand-selection, the gutter and inlay hints; multi-caret and line ops keep working because they are rope operations.

---

## 6. The seam split, concretely

### `crates/ui-shell/src/bridge.rs` (9388 lines) → `src/bridge/`

One `#[cxx_qt::bridge] mod ffi` survives, containing **only** the `extern` blocks — shared FFI structs, `#[qobject]` type aliases, invokable and signal declarations — and moves to `src/bridge/ffi.rs`.
Every `…Rust` struct and `impl` moves out; the aliases become `type SearchModel = crate::bridge::search::SearchModelRust;`, which cxx-qt accepts as any path.

Source ranges below are the **implementation** regions, not the declaration blocks inside `mod ffi` — an earlier draft cited the latter, which stay in `ffi.rs` by definition.

| New file | Contains | Source | Lines |
|---|---|---|---|
| `bridge/mod.rs` | `pub mod` declarations, the doc comment stating this layout and the ceiling | — | ~30 |
| `bridge/ffi.rs` | The single bridge module: shared structs (`FfiResult` `:17`, `FfiTextEdit` `:402`), all `#[qobject]` aliases, all declarations | `10–3088` | **3079** |
| `bridge/registry.rs` | Every process-wide shared handle: `APP_SESSION` (`:3112`), `index_slot()` (`:4489`), `DIAGNOSTICS` (`:7314`), plus the new VCS and run handles — each with a comment saying why the singleton exists (cxx-qt constructs via `Default`, so there is no constructor injection) | scattered | ~120 |
| `bridge/convert.rs` | `to_ffi_result()` (`:3137`) and the shared helpers the split would otherwise duplicate: `to_ffi_edits` (`:5909`, used by `LanguageService` **and** `AiChat`), `dispatch_editor_command` (`:4285`, `DocumentManager` **and** `AiChat`), `flatten_symbol_tree` (`:3269`), `load_settings`, `user_styles`, `MAX_HEX_ROWS_PER_REQUEST` | scattered | ~250 |
| `bridge/tree.rs` | `ProjectTreeModelRust` + impls, the watcher thread (`:3761`) | `3575–3874` | 300 |
| `bridge/editor.rs` | `DocumentManagerRust` + impls, `McpControl` | `3879–4246`, `4250–4283` | 508 |
| `bridge/settings.rs` | `AppSettingsRust`, `KeymapEditorRust`, `SyntaxColorEditorRust`, `LanguageCatalogRust`, `LanguageServerEditorRust` | `3349–3571`, `4394–4446`, `6837–6989`, `6997–7172`, `7176–7273` | 698 |
| `bridge/search.rs` | `SearchModelRust`, the index build thread (`:4610`), `QueryGuard` (`:4507`) | `4454–5557` | 1104 |
| `bridge/terminal.rs` | `TerminalSessionRust`, the PTY reader (`:5648`) | `5564–5819` | 256 |
| `bridge/language.rs` | `LanguageServiceRust`, the two LSP threads (`:6018`) | `5839–6831` | 993 |
| `bridge/ai/chat.rs` + `bridge/ai/agent.rs` | `AiChatRust`, `AiProviderEditorRust`, the chat thread (`:7671`); three separate `impl ffi::AiChat` blocks at `:7595`, `:8106`, `:8391`, `:8776`. Split at the `run_ask`/`run_agent`/`ApprovalGate` seam (`7419–8052`) | `7314–9259` | **1946**, split ~2×970 |

`#[cfg(test)] mod tests` (`9261–9388`) is not one module's — it mixes tree-role tests with `ApprovalGate` tests and splits across `tree.rs` and `ai/`.

Feature work then adds `bridge/editor_ops.rs` (F1), `bridge/vcs.rs` (F3), `bridge/run.rs` (F4), each ~600–900 lines.

**Two consequences the size gate must absorb before F0-2 starts.**
`ffi.rs` is **3079 lines on day one** — it exceeds the 2500 hard cap an earlier draft granted it before a single feature is added. cxx-qt requires one `mod ffi`, so the honest answer is to **exempt `ffi.rs` outright** and delete the pretend cap rather than trigger ADR-0025's multi-bridge escape hatch on arrival.
`ai.rs` is **1946 lines**, over the 1500 `.rs` ceiling on day one, which is why it lands as `ai/chat.rs` + `ai/agent.rs` above. F0-3 cannot be green under F0-1's own gate otherwise.

### `crates/ui-shell/cpp/main_window.cpp` (4350 lines) → nine translation units

Ranges below include each entity's **doc comment**. That matters: four doc comments sit above an unrelated intervening entity (`EditorTabs`' at 142–154 above `lspPosition`; `RefactorController`'s at 2163–2171 above `previewText`; `showSettingsDialog`'s at 2727–2733 above `registerAction`; `buildCentralWidget`'s at 3099–3105 above `CentralWidgets`), so a naive line-range cut lands all four in the wrong translation unit.

| New file | Contains | Current lines |
|---|---|---|
| `cpp/editor_tabs.{h,cpp}` | `class EditorTabs` — split/tab machinery, `applyBufferEdits`, layout serialisation | 142–154 + 167–1724 |
| `cpp/class_view_panel.{h,cpp}` | `symbolKindLabel` (1726–1747), `class ClassViewPanel` | 1726–1973 |
| `cpp/find_usages_panel.{h,cpp}` | `class FindUsagesPanel` | 1975–2074 |
| `cpp/ide_main_window.{h,cpp}` | `class IdeMainWindow` (key and close events) | 2076–2161 |
| `cpp/refactor_controller.{h,cpp}` | `class RefactorController` — **not** `previewText`, see below | 2163–2184 + 2186–2503 |
| `cpp/declaration_navigator.{h,cpp}` | `class DeclarationNavigator` | 2505–2683 |
| `cpp/action_registry.{h,cpp}` | `registerAction`, `applyKeymap`, `applyUiFontScales`, `UiFontTargets`, recents menus | 2685–2784 + 2727–2733 |
| `cpp/settings_dialog.{h,cpp}` | `SettingsContext`, `showSettingsDialog` | 2786–3097 |
| `cpp/dock_layout.{h,cpp}` | `CentralWidgets`, `treeRole`, `buildCentralWidget`, the dock registry and reconciliation | 3099–3561 |
| `cpp/main_window.cpp` (kept) | `buildMainWindow` menu wiring and `run_app` — target ~700 lines | 3563–4348 |

The 3110–3136 overlap in an earlier draft resolves entirely to `dock_layout`: `showSettingsDialog` ends at **3097**, and 3099–3136 is `buildCentralWidget`'s comment, `struct CentralWidgets` and `treeRole`'s comment.

**Shared helpers that need a header and lose internal linkage** — the split is not clean without them: `symbolKindLabel` (class_view **and** declaration_navigator, where `DeclarationNavigator::symbolKindLabel` at `:2654` is a near-duplicate worth collapsing); **`previewText`** (refactor_controller **and** `buildMainWindow:3708` — its doc comment at `:2172` says it is free *precisely* so both can use it, so it must **not** move into `refactor_controller`); `UiFontTargets`/`applyUiFontScales` (three TUs); `applyKeymap` (defined in action_registry, sole caller in settings_dialog); `CentralWidgets`/`showAiChatDock` (dock_layout **and** main_window); and `class EditorTabs`, which is a constructor parameter of every other proposed TU.

**No moc cost.** There is not a single `Q_OBJECT` in `main_window.cpp`, deliberately — four comments explain the absence (e.g. `:225`, `:2078`), and the file uses `std::function` callbacks precisely to avoid a second moc target. Each new TU is therefore one more `.cpp_file(...)` line in `build.rs:269-309`, with `rerun-if-changed` at `:249-253` already walking all of `cpp/`. The `build.rs` churn from ~20 new files is one line each, no header registration.

**`buildXPage` is mostly already done.** Only Appearance (`:2827`), Editor (`:2880`) and MCP (`:2996`) are inline; the other five pages already call out to `buildSyntaxColorsPage`/`buildKeymapPage`/`buildLanguagesPage`/`buildLanguageServersPage`/`buildAiProvidersPage` in their own TUs. F0-6 is smaller than it reads. The 14-parameter signature at `:2786-2794` with its single call site at `:3917` is confirmed.

The split is mechanical and committed as such: no renamed methods, no changed signatures except `showSettingsDialog`'s (F0-6).

**The dock reconciliation.** `dock_layout.cpp` gains `struct DockSpec { ads::CDockWidget *dock; ads::DockWidgetArea area; ads::CDockWidget *tabWith; }` and a `std::vector<DockSpec>` built as the docks are.
After `restoreState`, `reconcileDocks(specs)` walks it and re-adds any dock ADS left with no dock area — the same three lines `showAiChatDock` does today, once, for all of them.
What gets deleted is `showAiChatDock`'s comment at **3141–3151** and its near-duplicate restatement at **3645–3657** wrapping the `showAiChat` lambda. (An earlier draft claimed a comment saying general reconciliation is deliberately absent; no such statement exists — `grep -n reconcil` returns nothing. The argument for generalising the one ad-hoc case stands regardless.)

**Enforcement.** `scripts/check-file-size.sh`: 1500 lines per `.rs`, 1200 per `.cpp`/`.h`, short exemption list (`bridge/ffi.rs` at 2500). Run by `make lint` and the CI lint step, failing the same way clippy does.

**Proving the split preserved behaviour** — asserted in every large refactor and true in about half. Two mechanisms, both decisive:

1. **FFI header snapshot.** `#[cxx_qt::bridge]` generates C++ headers into `target/**/cxx-qt-gen/`. Snapshot before, snapshot after, diff. Empty diff proves every QObject, slot signature, signal and type mapping is unchanged — a stronger guarantee than any test, because it is about the interface rather than a sample of behaviour. Keep it permanently with a `make bless-ffi-snapshot`, run **unconditionally** (it costs 30s), so any future PR changing the FFI surface says so out loud in its diff.
2. **E2E marker-stream golden comparison.** `main_window.cpp` has no compile-time invariant to lean on. Run the five seed flows on the pre-split revision, save `events.jsonl` per flow, re-run after, diff **including event order** (excluding timestamps, pids, temp paths, window ids, ports). A reordered `tab_added`/`project_opened` pair is exactly the `connect()`-ordering change a mechanical-looking C++ split introduces, and it is invisible to every other check. **This is the concrete reason F0-11/F0-12 land before F0-4.**

Two mechanisms an earlier draft added and this one drops: a normalised-source `verify-mechanical-split.sh` (the split *already* requires four doc comments to move between translation units, so the diff will be dirty on the first commit — a gate expected to be dirty is a gate that gets waved through), and a `split:`-commit-message CI condition (a convention masquerading as enforcement; the snapshot runs unconditionally instead).

---

## 7. Sequencing

```
F0-11 → F0-12  (harness + seed flows)          ← first, no production code
      ↓
F0-1 … F0-7    (split, ending at the dock registry)
      ‖ F0-8 (byte column)  ‖ F0-9 (project settings)  ‖ F0-14 → F0-15 (conformance)
      ↓
F0-10, F0-13, F0-16, F0-17
      ↓
F3-1 → F3-13 → F3-14 → F3-15   (diff component + its three retrofits — early, out of order)
      ↓
F1  →  F2  →  F3-2 … F3-20  ‖  F4
```

1. **F0-11 → F0-12 first**, because they are the regression net for the split that follows and they touch no production code.
2. **F0-1 → F0-7**, with F0-8, F0-9 and F0-14/F0-15 on parallel branches.
3. **F0-10, F0-13, F0-16, F0-17.**
4. **F3-1 → F3-13 → F3-14 → F3-15 — out of order and early.** The diff component has no dependency on Git, it pays down a real debt (the replace preview with no undo and no diff), and it proves itself against existing consumers before F3 has any Git code to blame.
5. **F1** — the largest single behavioural improvement; F1-1…F1-4 are pure `editor-core` and can start the moment the split lands.
6. **F2** — after F1, because Alt+Enter's edits ride the same transaction path, and after F0-15, because the conformance report says which servers actually answer `codeAction` with what.
7. **F3-2 → F3-20** — the Git lane, the longest, entered with the diff component already shipped.
8. **F4** — depends on F0-9 and F0-8 and nothing else; can run in parallel with F3, since they share no crate.

After step 3 the four feature lanes touch disjoint crate sets. They converge only in `bridge/ffi.rs`, `keymap.rs::ACTIONS`, `dock_layout.cpp` and `build.rs` — four files that will conflict on every merge and should be edited in small, append-only diffs.

---

## 8. Test strategy

### The placement rule

`layering.md` already says *"if it deserves a unit test, it cannot live in `bridge.rs` or `cpp/`."* Read contrapositively, that is the placement algorithm:

> **If a test would still be meaningful with the Qt event loop removed, it must not be an E2E test.**
> **If a test cannot fail without a `QApplication`, it must not be anything else.**

Target shape per slice: ~85% unit, ~10% integration, ~5% E2E — E2E measured in *flows*, capped per §1.

### What currently has no net

`cpp/` is untested by design and `bridge.rs` has 7 tests across 9388 lines, defensible only because those layers are supposed to be empty of decisions. The bug classes with zero coverage — the entire charter of the E2E suite — are: signal/slot wiring (a `connect()` never made, made twice, or to the wrong overload); cross-thread arrival (a result that never lands, lands after the widget is gone, or lands out of order relative to a newer generation — the *cancel decision* is tested, the *delivery* is not); widget lifetime; index-identity mapping at the model edge (where an off-by-one closes the wrong tab); keyboard/focus routing; dialog and modal flows (a preview showing stale data, a cancel that applies anyway); persistence round-trips through the view (the TOML round-trip is unit-tested; whether the view restores itself from it is not).

### The harness

Lives at `crates/e2e/tests/` with `harness.rs` (the `Ide` fixture), `keys.rs` (xdotool wrappers) and `fixtures/` (tiny checked-in sample projects). Every test fn is `#[ignore]`d, so `cargo test --workspace` stays exactly as fast as today.

`make e2e` runs `xvfb-run -a --server-args="-screen 0 1600x1200x24" cargo test -p e2e -- --ignored --test-threads=1` through `RUN_LINUX`, whose named volumes are mandatory (a bare `docker run --rm` re-downloads ~390 crates and recompiles every C++ TU). `make e2e-repeat TEST=… N=…` is the burn-in target.
**No image change is needed** — `xvfb`, `xauth`, `x11-apps`, `imagemagick` and `xdotool` are already installed in `linux-builder` (`docker/Dockerfile:49-53`).

`--test-threads=1` to start: not because Xvfb cannot handle parallelism, but because one X server with N app instances makes xdotool's window targeting ambiguous, and ambiguous input is the first source of E2E flake.

**Avoiding fake-passing timings** — the part that decides whether the suite is worth having.

1. **No `sleep` anywhere except inside one function.** The harness exposes exactly one waiting primitive, modelled on `stub_server_session.rs`'s `wait_for` (lines 40–55): a deadline, a poll, a predicate, and a panic naming what it waited for. Its poll interval is the only `sleep` in the test tree, and a CI grep gate keeps it that way.
2. **Never wait for a duration; wait for a transition.** A test that passes because 200 ms happened to be enough is worse than no test — it will pass in CI and fail on a loaded laptop, which teaches the developer to re-run rather than debug.
3. **Never assert immediately after sending input.** `xdotool key ctrl+s` returns as soon as the X event is queued.
4. **Every assertion is against state the app published, never a screenshot.**

**What the app must expose.** Two mechanisms, deliberately separate, because they answer different questions:

*(a) A view-side marker stream* — `IDE_E2E_EVENTS=/path/to/events.jsonl`; unset means every mark is a no-op. One free function in `cpp/e2e_mark.cpp`: `void e2eMark(const char *json)` appends a line and flushes. Called where the view *finished doing something*: `{"ev":"tab_added","index":0,"title":"a.rs"}`, `dialog_shown`, `dialog_closed`, `split_created`, `status`. This does not violate the humble-view rule — it contains no `if` encoding a business decision; it is the view reporting what it did, the same category as painting. It is the **only** channel that can catch the wiring, lifetime, identity-mapping and focus bug classes, because it reports the *widget's* view of the world.

*(b) A quiescence probe, deferred until a flow needs it.* The design, when it is needed: one `AtomicUsize` in-flight counter in `bridge/registry.rs`, incremented where a worker thread is spawned and decremented at the end of the queued callback, exposed in the MCP `ping` reply, with `wait_until_idle()` polling until `inflight == 0` **and** the marker file has not grown for two consecutive polls (either condition alone lies — `inflight == 0` before the worker was even spawned, a quiet marker file during a long computation). But all five seed flows are writable with the marker stream and `wait_for(predicate)` alone, and this is product code added for the harness. Add it when a specific flow cannot be written without it, not before.

**Is MCP the control channel?** No for input, yes-with-limits for observation.
As input it is disqualified: `open_file` over MCP routes through `AppSession` and never touches the tree widget, never raises a dialog, never exercises a shortcut — it would skip the exact layer E2E exists to cover and produce a green suite proving nothing about `cpp/`. **Input is xdotool, always.**
As observation it is genuinely useful: `read_buffer` is the cheapest way to assert document content after an edit and `get_cursor_position` after a jump. But it cannot answer "is there a tab widget on screen", so it never replaces the marker stream. A flow that is *about* MCP must observe via markers only.
Port discovery is already solved: `bridge.rs:3479` writes a discovery file, and `mcp_port == 0` means OS-chosen — so per-test config dirs give collision-free parallelism for free.

**Startup, the classic fake-pass.** Do not wait for the process to exist, and do not wait for `xdotool search --name` — a window can exist before it is mapped, and typing into an unmapped window is silently dropped. Wait for all three: the `main_window_shown` marker, `xdotool search --onlyvisible --name` returning exactly one id, and `getactivewindow` equal to it. Then `xdotool windowactivate --sync`. The `--sync` flags are not optional.

**Isolation.** Everything routed through `resolve_config_dir` (`app-core/src/lib.rs:245` → `project_model::default_config_dir` → `dirs::config_dir()`, read at call time) is env-overridable, which covers settings.toml, window state, geometry, editor layout, recents, last-project, the MCP discovery file, AI history and runtime languages. So: `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `HOME` under a per-test temp dir; project root a fresh `tempfile::TempDir` copied from `fixtures/`; `mcp_port = 0`.
Two corrections to an earlier draft's "no product change is needed". **The index is not env-overridable** — `index-core/src/lib.rs:150-168` writes `<project_root>/.ide-index` and only falls back to `dirs::cache_dir()` when the project dir refuses a file lock; isolation there comes from the throwaway project `TempDir`, and the `Drop` assertion must know that. And `XDG_STATE_HOME`/`XDG_DATA_HOME` are used **nowhere** in the workspace, so seeding them is theatre.
Also **assert** `XDG_CONFIG_HOME` is set rather than merely setting it: with both it and `HOME` unset, `resolve_config_dir` lands in `std::env::temp_dir().join("ide")`, which is shared, not isolated.
Git fixtures are built by shelling out to `git init` with pinned `GIT_AUTHOR_*`/`GIT_COMMITTER_*` and dates so hashes are deterministic — never a checked-in `.git`. The fixture's constructor asserts the config dir is empty and its `Drop` asserts nothing was written outside it, so cross-test bleed fails on the *first* offending test.
Do **not** add a `resolve_config_dir`/`XDG_CONFIG_HOME` unit test via `std::env::set_var` — integration tests share one process and that is a process-global race. `stub_server_session.rs:34`'s `dying_stub_config()` shows the right pattern: pass config to a child via `env()` and keep the test process' own environment untouched.

**Artifacts.** A `Drop` impl firing only when `std::thread::panicking()` writes `target/e2e-artifacts/<test>/{screen.png, events.jsonl, app.stderr, app.stdout, config/}`. CI uploads with `if: failure()`, 14-day retention. Marker streams are captured on success too — they are the input to the seam-split golden comparison.

**Flakiness policy.** A flake is a P1 bug in the product or the harness, and the default assumption is *the product has a race* — given the cross-thread bug class, an E2E flake is often the suite doing its job. No `#[ignore]` as a coping mechanism (it is already load-bearing for E2E and benches; overloading it makes a suppressed test indistinguishable from a normal one) and no quarantine tier. The only two outcomes are fixed or deleted within one working session; a deleted test leaves an open row in this plan's Progress table, because a gap you can see beats a red suite you have learned to ignore. Re-running CI to get green is a policy violation, not a workaround.
The enforcement machinery — `make e2e-repeat` with a 20-run burn-in gate, and the CI grep forbidding `sleep` outside `harness.rs` — is written down here but **built when it is first needed**, not before flow #1 exists. The single `wait_for` primitive and the artifacts-on-panic `Drop` are what F0-11 ships; the rest is policy enforcement for a discipline problem that has not happened yet in a suite that does not exist.

### The four F0 seed flows

**`e2e_open_project_edit_save`** — spawn with the seeded environment; wait for `main_window_shown` + exactly one visible window + `getactivewindow` matching; wait for `project_opened`; open `src/main.rs` and wait for `tab_added{title:"main.rs",index:0}`, then assert MCP `list_open_buffers` has exactly one entry **whose index matches the marker's** (the disagreement between these two is the identity-mapping bug class); type, wait until `read_buffer` ends with the typed text, assert `tab_dirty{dirty:true}`; Ctrl+S, wait for `tab_dirty{dirty:false}` then `wait_until_idle()`, assert the file on disk (read from Rust, not through the app) changed; **Ctrl+Z once** and assert `read_buffer` equals the original fixture content — the edit-block granularity guard, which has no other net; Ctrl+Q, assert exit 0 and that nothing was written outside the temp tree.

**`e2e_search_everywhere_jump`** — wait for MCP `index_status` to report the index complete (**the step that would be a `sleep` in a naive harness**); open Search Everywhere, type a partial symbol, wait for `search_results{count:N}` and `wait_until_idle()`; assert the first result matches, **and that the marker stream contains at most 2 `search_results` events for 10 keystrokes** — proving both debouncing and that stale generations were discarded rather than delivered, which is the canonical cross-thread bug and is invisible to `bridge.rs`'s own tests; Enter, assert `get_cursor_position` is the declaration line an `index-core` unit test independently asserts for the same fixture; navigate back and assert the pre-jump location.

**`e2e_rename_with_preview`** — Shift+F6, type a new name, preview, assert `preview_rows{count:N}` equals the count `index-core`'s unit test asserts for the same fixture (the preview shows what the rule found, not a subset); **Escape and assert every file on disk is byte-identical and the open buffer unchanged** (cancel-that-applies-anyway is the dialog bug class); repeat and apply, assert open buffers were spliced and closed files written, and that **one** Ctrl+Z reverts the whole rename; then make the buffer stale and apply a pre-computed rename, asserting `workspace_edit_refused{reason:"stale"}` and no file changed.

**`e2e_split_editor_persistence`** — open two files, split, drag the second into the right pane with `xdotool mousemove --sync` (the `--sync` is where fake timings creep in), assert one tab per pane; Ctrl+Q; parse the window-state TOML **with `app-config`'s own types, not a regex**, asserting two panes; respawn with the same `XDG_CONFIG_HOME` and assert pane/tab assignment, active tab and cursor positions are identical. `app-config`'s round-trip is unit-tested; *the view reconstructing itself from it* has no other net.

### The LSP conformance pass

A separate `lsp-conformance` Docker stage layered on `linux-builder` (~+450 MB, +2 min cold, ~0 warm), because `linux-builder` is pulled by every CI job and every developer. **Pin every version** — an unpinned server turns a conformance suite into a random number generator, and "pyright changed its `codeAction` shape" arriving as a red CI on an unrelated PR is how a suite gets disabled.

The three servers are chosen to break naive clients in different ways: rust-analyzer (rich, request-driven, slow to warm), pyright (Node, different position-encoding history, aggressive `publishDiagnostics`), clangd (needs `compile_commands.json`, leans on `codeAction` resolve). Exercised: initialize/capabilities, **position encoding** (fixtures contain emoji and CJK — assert a hover *after* a 4-byte char resolves to the right symbol on all three; the highest-value assertion here), publishDiagnostics including the clear-on-empty-array case, hover (markdown vs plaintext vs `MarkedString[]`), completion including `isIncomplete` and resolve, definition (Location vs LocationLink vs array), documentHighlight, signatureHelp, inlayHint, diagnostic-scoped codeAction, command-driven codeAction with `workspace/applyEdit`, organizeImports, formatting, rename + prepareRename, **WorkspaceEdit resource ops**, and shutdown cleanliness (assert no orphaned process via `/proc`).

**The record must be executable, because a hand-written report rots by the second week.** `crates/lsp-core/tests/data/conformance-expectations.toml` holds the observed capability matrix per server; the test asserts equality against it and fails with a diff on drift. `CONFORMANCE_BLESS=1` regenerates it, and the regenerated diff is what gets reviewed in the PR. The TOML is the report, its history is the changelog, and it cannot silently disagree with reality. `docs/architecture/lsp-conformance.md` carries prose only — never a capability table.

Runs **nightly on `main` plus `workflow_dispatch`**, not per-PR (a 4–6 minute gate failing for reasons unrelated to 95% of PRs) and not manually (the `#[ignore]`d index benches are already "run manually", and that is exactly what happened to them). PRs labelled `lsp` run it as a required check.

**`stub_server` keeps its job and gets sharper.** Two suites, no overlap: the stub tests **our client** (framing, version counters, out-of-order responses, cancellation, a server that dies mid-session, respawn backoff, re-entrant `applyEdit`) in 2 seconds on every PR; the real servers test **our assumptions about the protocol** nightly. A real server will not die on cue, will not answer out of order to order, and will not send a malformed response — the stub is the only way to test the failure paths, and the failure paths are where clients break. **Rule: every bug the conformance suite finds gets a stub regression test in the same PR as the fix.** Without it the stub decays into a legacy fixture and the nightly becomes a 24-hour feedback loop on a client bug.

### Per-feature test cases

**F0 — per-project settings** (all unit, no E2E): project overrides global; **an absent project key falls back per key, not per file** (whole-file replacement masquerading as merge is the nastiest bug in layered settings); an absent project file behaves byte-identically to global-only; **a malformed project TOML is reported and global is used — not reset to defaults** (commit `2478694` already shipped that exact bug once at the global level; regression-test it at the layered level too); a UI edit writes to the layer it came from; a `.ide` resolved through a symlink escaping the project root is refused; unknown keys survive a round trip.

**F1 — multi-caret × undo**: three carets one insert is one undo entry; undo restores all three caret positions (carets are part of the transaction, not decoration); redo reapplies all three, and redo after a new edit is discarded; a multi-caret edit failing at caret two leaves the document untouched; typing a word across carets coalesces but a newline breaks it (pin the coalescing rule or users will discover it).
**Overlapping/adjacent carets**: two carets at the same offset collapse; adjacent carets deleting backwards do not double-delete (offsets 5 and 6, backspace — the classic); edits apply right-to-left so earlier offsets stay valid (or left-to-right with adjustment — either is fine, undefined is not); overlapping selections merge before the edit, not after; a caret inside another's deleted range is dropped; column selection over ragged lines pads or clips (pin which — short lines are where implementations differ); column selection over a tab uses visual columns; column selection across a 4-byte char.
**Comment toggle**: one data-driven test iterating `syntax-core`'s registry — `every_registered_language_has_a_comment_token_or_is_explicitly_exempt` — is what stops language #36 silently having no comment support. **Do not write 35 test functions.** Plus: a mixed selection comments everything (the JetBrains rule — if any line is uncommented, comment all); indentation preserved with the token at the common indent; blank lines within a selection skipped; block toggle on a partial line selection; nested block comments in Rust but not C; `toggle(toggle(x)) == x` over all 35 in one test; toggle with three carets is one undo entry.
**Auto-close vs type-over vs paste**: typing `(` inserts a pair; `)` immediately after types over, elsewhere inserts; **type-over applies only to a pair this session inserted** (typing `)` before a pre-existing `)` must insert — requires tracking, and the tracking must be invalidated by an intervening edit); **pasting a string containing brackets inserts nothing extra** (the one everyone gets wrong — paste is not typing); suppression inside strings and comments; quote auto-close suppressed when the next char is a word char; a Rust lifetime `&'a str` does not auto-close; smart backspace between a pair deletes both, with content between deletes one; surround wraps rather than replaces, and with three carets wraps all three in one transaction.
**Auto-indent**: data-driven per language from `tests/data/indent/<lang>/{input,expected}.txt`, so adding a language means adding two files; newline after `{` indents; typing `}` dedents to its opener; Python indents after `:` and dedents after `return`; **a file with no grammar falls back to copying the previous line's indent** (the fallback matters more than the clever path); indent respects the effective tab width.
**Trim-on-save**: does not move a caret on an untouched line; a caret sitting in trimmed whitespace moves to the new end of line (pin it — jumping to column 0 is the bug); does not trim the caret's own line if that is the policy; preserves all three carets; is one undo entry separate from the user's last edit; final-newline and trim together on a file ending `"  \n  "`; does not dirty a clean file.
**Tab width**: resolves project over global over language default (three layers now); a Go file and a Python file open at once use different widths; changing it does not rewrite the document.
**Formatting** goes to the conformance suite, plus stub coverage for server absent, stale-version edits, and overlapping edits (refuse).

**F2 — debounce/cancellation**: a newer caret move supersedes an in-flight query; **a stale result arriving after a newer one is discarded** (assert the discard, not just that the newer arrived); results for a closed tab are discarded without touching the model; ten moves in the window produce one request.
**No server**: Alt+Enter offers only local intentions and shows neither an empty popup nor a forever-spinner; while the server is initializing it shows local then updates; a server dying between request and response leaves no spinner; a server that never answers times out and the bulb clears.
**Resource operations**: create makes the file; rename moves it and **retargets the open tab by `TabId`** (a rename that closes and reopens loses undo history); delete closes the tab; rename of a file with unsaved changes is refused or saves first (pin it — silently discarding edits is the failure mode); create over an existing file without `overwrite` is refused; `ignoreIfExists` is a no-op. **Partial failure needs a decided policy first**: filesystem ops are not transactional, so do not pretend — a failure at op 3 of 5 is reported with exactly what was and was not applied; **text edits apply only after all resource ops succeed** (ordering as the mitigation); a delete failing because the file is already gone is success; a cross-filesystem rename falls back to copy+delete or reports cleanly; **a resource op targeting a path outside the project root is refused** (a security boundary — never simplified away).
**Signature help**: table-driven active-parameter index over ~12 offsets in `foo(bar(1, |2), 3)`; a string containing a comma does not advance it; a nested generic does not confuse the depth counter; dismissal on `)`; an overload set selects by arity.

**F3 — diff correctness, tested once.** The component has four call sites; test it **once** at the `editor-core` level as a pure function, then test each call site only for *its own* transformation of that output. Corpus in `tests/data/diff/`: pure insert, pure delete, replace, insert at BOF and EOF, no trailing newline, CRLF vs LF, a file that becomes empty, an empty file gaining content, whitespace-only change, a 1-line file. Two property assertions over the whole corpus: `hunks_are_non_overlapping_and_ascending`, and **`reverting_every_hunk_yields_the_before_text`** — the strongest invariant available and worth more than the other twenty tests. Plus `reverting_one_hunk_leaves_the_others_intact_and_correctly_offset`, which is where per-hunk operations break.
**Gutter under load**: a 50k-line diff bench (`#[ignore]`d, not a PR gate); rapid typing produces at most one diff per debounce window; a result for an older document version is discarded; markers recompute after an external change lands via the watcher; markers clear when the file stops being tracked.
**Per-hunk staging**: staging hunk 2 of 3 leaves 1 and 3 unstaged; staging then editing keeps the staged content (index vs worktree divergence — the test proving we use the index rather than re-reading the file); unstaging a hunk staged from a since-changed worktree is refused or recomputed; **staging a hunk in a file with no trailing newline** (the `\ No newline at end of file` marker, a perennial source of corrupt patches); staging all hunks equals `git add` of the whole file, byte-compared by index blob hash; staging a deletion hunk; staging in a newly added untracked file.
**Repository-state matrix**, one integration file building each state with real `git` in a `TempDir`: not a repo (every query returns `NotARepository`, nothing panics, nothing blocks startup); zero commits (no HEAD, so every tracked file is "all added"; commit creates the root commit); detached HEAD (**push disabled with a reason, not silently broken**); mid-merge and mid-rebase (conflicted files marked, staging in one refused with a clear message); bare repo refused cleanly; submodules not traversed as ordinary files; a gitignored file open in the editor gets no markers and no error; a 10k-file repo does not call `status` synchronously on the UI thread (assert via the in-flight counter); a `git worktree` resolves the real git dir.
**Credential-required push**: fails with an auth error, **not a hang** — the failure mode is `git` prompting on a stdin nobody reads, so assert we set `GIT_TERMINAL_PROMPT=0`; no remote configured is reported before any network attempt; a non-fast-forward reports the rejection and offers no silent force; a slow push is cancellable and leaves no orphan.

**F4 — kill-tree**: Linux — killing a run kills a child that ignores SIGTERM (assert SIGKILL follows the grace period), kills a grandchild (`sh -c 'sleep 300 & wait'`), and **a process that double-forks and detaches is reported as not fully killed**, because it genuinely cannot be and lying is worse. Windows is untestable in our CI (MXE cross-build, no Windows runner), so the *policy* value is unit-tested under both `cfg`s and the syscall becomes a documented manual release gate. **The fixture's `Drop` asserts no child of the test process remains**, so an orphan is caught by whichever test created it.
**Console throughput**: a million lines does not grow memory without bound; **10 000 lines arriving in 100 ms produce ≤ 20 `queue()` calls, not 10 000** — *the* UI-freeze bug, and a Qt-free assertion on the batching type; a 1 MB line with no newline flushes on size; stdout/stderr ordering pinned; **partial UTF-8 split across two reads is not mangled** (a multi-byte char straddling the 4 KB boundary — cheap test, guaranteed bug otherwise); ANSI reset state persists across chunk boundaries by reusing `terminal-core`'s VT state rather than writing a second parser.
**`file:line` linking**, table-driven with expected `(path, line, col)` or `None`: rustc `src/main.rs:42:5` and `--> ` and `panicked at`; Python `File "app/main.py", line 12`; Node `at fn (/abs/app/main.js:12:3)`; gcc/clang `main.cpp:42:1: error:`; `Makefile:17:`; a Windows path on Linux and vice versa. **The negatives matter more than the positives**: `http://example.com:8080`, `12:30:00`, `foo:bar:baz`, `-c:5`, an ANSI escape containing a colon. Plus: **a relative path resolves against the run config's cwd, not the project root** (the bug that makes every link dead in a Cargo workspace); a path that does not exist is not rendered as a link (a dead link is worse than plain text); detection over a 10k-line burst stays within the batch budget.
**Config and lifecycle**: a program that does not exist reports ENOENT in the console and marks the run failed — not a silent no-op, and the console still opens so the user sees why; a cwd that does not exist fails before spawning; a program that is a directory or not executable; env vars override the inherited environment and an empty value unsets; `$PROJECT_DIR` expands; detection finds every Cargo bin target, npm script and Makefile phony target; **auto-detected configs do not overwrite a user-edited config of the same name** (the "my settings got wiped" bug again); a workspace with no detectable target yields no configs and no error.
**Concurrent consoles**: two runs of the same config get distinct consoles and pids; output from A never appears in B (content-tagged over a 5k-line interleaved burst); closing A kills only A; quitting with three active kills all three.

### CI

Two workflows, both running inside the same `docker/Dockerfile` stages developers use locally, so CI cannot drift from the dev environment.
`.github/workflows/builder-image.yml` is a reusable `workflow_call` that builds and pushes one stage to GHCR, tagged by `hashFiles('docker/Dockerfile')` with a `type=gha` layer cache; both workflows call it rather than duplicating the build.

`ci.yml` runs per push to `main` and per pull request, cheapest-failure-first:

| # | Gate | Budget |
|---|---|---|
| 1 | `cargo fmt --all --check` | 10s |
| 2 | Layering gate, all 14 crates (Qt) plus 4 (tokio) — at parity with `layering.md` | 20s |
| 3 | `scripts/check-file-size.sh` | 5s |
| 4 | `cargo clippy --workspace --all-targets -- -D warnings` | 4–6 min |
| 5 | `xvfb-run -a cargo test --workspace` | 5–7 min |

`nightly.yml` runs on a 03:00 UTC cron and on `workflow_dispatch`, with two independent jobs:

| Job | Image stage | Command | Timeout |
|---|---|---|---|
| `e2e` | `linux-builder` | `make e2e-ci` | 30 min |
| `lsp-conformance` | `lsp-conformance` | `make lsp-conformance-ci` | 20 min |

Both are blocking within their workflow — **never `continue-on-error: true`**, because a non-blocking gate is a gate nobody fixes.
On failure the `e2e` job uploads `target/e2e-artifacts/**` (`actions/upload-artifact@v4`, `if: failure()`): the per-test `events.jsonl` plus the stdout, stderr and screenshot the harness dumps on panic, which is the only evidence of a failure nobody can reproduce locally.

The `*-ci` make targets are the inner half of `make e2e` and `make lsp-conformance`: the outer targets are just the `docker run` wrapper, so the command line exists in exactly one place and CI runs literally what a developer runs.
E2E stays nightly rather than per-PR by decision — it drives a real Qt binary under Xvfb and the pull-request gate is already 10+ minutes.

Still nightly-shaped and not yet wired: the index benches with a checked-in baseline failing only on a >3× regression, E2E in release mode (a different timing profile catches races that only appear when things are fast), the Windows cross-build, and `cargo audit`.

### What is deliberately not tested

C++ unit tests in `cpp/` — an architectural refusal, not a cost judgement, and `CLAUDE.md` already states it; adding QTest would create a place for rules to hide where the layering gate cannot see them.
Screenshot/pixel-diff regression — font hinting, Qt point releases, theme and DPI all move independently of our code, producing a permanent low-grade red that trains everyone to click "approve new baseline". Screenshots stay diagnostics on failure. If a specific pixel bug recurs, the fix is a marker assertion about the *value*, not the pixel.
A full E2E suite on Windows — no runner, and the binary is MXE-cross-built; the differences concentrate in three places (paths, process groups, the DLL closure), so test those specifically and cover the rest with a manual release checklist.
The AI chat against a real API in CI — cost, nondeterminism, a network dependency in a gate. The deterministic half is already well covered (39 tests in `ai-chat-core/src/context.rs`).
Anything upstream owns — tree-sitter grammars, `alacritty_terminal`'s VT emulation, `ropey`, git's plumbing, `tantivy`. We test *our use* of them.
Performance as a PR gate — shared runners vary by more than the regressions worth catching; a threshold loose enough not to flake is loose enough to miss real ones. Perf lives nightly as a trend.
Exhaustive per-language test *functions* — a registry-driven test is one language #36 cannot escape; a per-language function is one it will not have.

---

## 9. Risks and open questions

| # | Question | Recommendation |
|---|---|---|
| 1 | **Ctrl+D collides.** The PM specified Ctrl+D for select-next-occurrence (VS Code); JetBrains uses it for duplicate-line, which this IDE otherwise resembles. | Follow the PM: `edit.selectNextOccurrence` = Ctrl+D, `edit.duplicateLine` = Ctrl+Alt+D. The keymap is user-editable and the **ids** are what must stay stable, not the defaults. Ship a JetBrains preset later if anyone asks. |
| 2 | **The split's blast radius.** F0-2…F0-6 touch two files every in-flight branch also touches. | Land it in one working week, in the stated order, with no feature work in flight — and land the E2E flows first. A split reviewed against a green E2E suite is a different risk from one reviewed by reading 13 000 lines of diff. |
| 3 | **`gix` API churn.** Pre-1.0, APIs move between minors. | Confine every `gix` type to `vcs-core::repo` behind our own types, pin an exact version, treat an upgrade as its own commit gated by `vcs-core`'s tests. The escape hatch is CLI for reads too, at a known performance cost — the boundary is already drawn. |
| 4 | **Hunk staging via `git apply --cached`.** Generating a correct patch for an arbitrary hunk selection is fiddly (context lines, no-newline-at-EOF, CRLF). | Do it anyway, tested against a temp repo with a matrix of nasty cases. The alternative — writing the index ourselves — is worse, and this is where a bug corrupts someone's work. |
| 5 | **Kill-tree on Windows** needs `windows-sys` in `pty-core`, currently `std` + `portable-pty`. | Accept it, behind `#[cfg(windows)]`. Orphaned build processes are the most-complained-about defect in every IDE that got this wrong. |
| 6 | **Which settings are project-scoped.** Proposed: editing behaviour, language servers, run configurations, index excludes. Global: theme, fonts, keymap, AI providers. | Ship that split. Defensible in one sentence — "a project may configure the project, not you" — and widening it later is additive while narrowing it is a breaking change to a committed file. |
| 7 | **`.ide/` and `.gitignore`.** `.ide/settings.toml` is meant to be committed, sitting beside an *ignored* `.ide-index/` (`index-core/src/lib.rs:75`, already in `.gitignore:8`). A user's muscle-memory `echo .ide >> .gitignore` silently un-commits the thing ADR-0022 says should be committed. | Cleanest would be `.ide/settings.toml` + `.ide/index/`, but moving the index dir breaks existing users. So keep `.ide/`, and have F0-9 write a `.ide/.gitignore` containing `local/` — self-documenting, and it touches nothing of the user's. |
| 13 | **`settings.toml` has no version field and no migration code**, and no `deny_unknown_fields` — so unknown keys are ignored on load and **dropped on the next save**. A file written by a newer binary and re-saved by an older one loses keys permanently. | Introduce a `version` key **now**, while the project layer is new and the cost is zero. F0-9 must also reproduce all three protections commit `2478694` added for the global file: atomic temp-write + `sync_all` + rename, load-failure aborts rather than defaulting, and `update()` (`app-config/src/lib.rs:501-511`) refuses to edit a defaulted struct. Name `update()` in the task so it is reused rather than rewritten. |
| 14 | **The E2E flow budget is full on arrival** — 4 + 2×4 = 12, the bottom of the 12–15 band, with no headroom for the rest of the product's life. | Budget 15, or spend 10 now. The arithmetic-forces-the-question mechanism is good, but starting at the floor makes the first post-F4 flow a deletion argument. |
| 15 | **`Alt+4`/`Alt+9` for the new docks** introduce a numbering scheme the existing dock actions do not use (`view.classView` = `Ctrl+Alt+P`, `view.terminal` = ``Ctrl+` ``, `view.problems` = `Ctrl+Alt+M`). | Number all of them or none. Also note `keymap.rs::ACTIONS` needs no migration for the ~38 new ids — overrides persist by id, an unknown id reads as unbound and is not assignable (`keymap.rs:299-322`), and `action_ids_are_unique`/`shipped_defaults_do_not_conflict` are the gate. All ~38 proposed defaults were checked against the 41 shipped: no collisions. |
| 8 | **Multi-caret typing latency through the FFI.** Every keystroke with N carets is a Rust call plus a `Vec<FfiTextEdit>` splice. | Measure at 1024 carets in F1-13 **before** wiring the view. If it exceeds 16 ms, batch within a keystroke — do not move caret state to C++. The ceiling exists so there is a number to fail against. |
| 9 | **Inlay hints default.** | Off, and keep it off — hints reflow the line, and every user who has not asked for them reads it as a rendering bug. |
| 10 | **Conformance job flakiness.** Real servers change behaviour between releases. | Pin the versions, run nightly and on demand rather than per-PR, and treat a new failure as a report to read rather than a merge blocker. Bumping a pin is a deliberate commit. |
| 11 | **Does the AI agent gain a "run" tool once F4 exists?** | No, and say so in the F4 docs. ADR-0021 rejected shell execution structurally; a run configuration the user wrote and a command the model composed are not the same thing, and owning the machinery does not change the argument. |
| 12 | **Does the diff tab need to be editable for a later 3-way merge?** | Let `DiffView`'s constructor take a `Vec` rather than a pair, since that costs nothing. Do **not** add a `panes` field it does not use — an earlier draft answered "3-way merge stays out of v1" and then built for it anyway. |

---

## 10. Verification

**Per commit** — `make lint` and `make test` (`cargo test --workspace`) inside `linux-builder`, plus the file-size gate once F0-1 lands. Never on the bare host.

**Per feature** — `make e2e` green, including the new flows, with a 20-run burn-in on each new flow before merge.

**Layering, after each new crate** — `cargo tree -p <crate> -e normal | grep -i qt` must be empty for `edit-ops`, `vcs-core`, `run-core` and `e2e`, and the check added to CI's loop alongside the existing three.

**The seam split specifically** — `make verify-split` clean, the FFI header snapshot diff empty, and the five seed flows' marker streams identical to their pre-split goldens.

**The LSP work** — `make lsp-conformance` against the three pinned servers, with `conformance-expectations.toml` committed and any drift reviewed as a diff.

**By hand, once each feature lands** (these are the things no automated gate covers):
Open a project with a `.ide/settings.toml` setting a tab width; confirm the dialog says the value comes from the project, and that deleting the file degrades to Global without complaint.
Press Ctrl+D four times, type, and press Ctrl+Z **once** — all four sites must revert together. Toggle a comment in Rust, Python and HTML and confirm the block form is used where there is no line comment.
Alt+Enter on an unresolved import with rust-analyzer running, then again with no server at all — the second must show nothing, not an error.
Edit a tracked file and watch the gutter mark appear; revert the hunk, then Ctrl+Z it back. Stage two of three hunks and confirm `git diff --cached` on the command line agrees exactly. Push to a remote that rejects, and read the message.
Run a detected `cargo run` configuration, click a `src/main.rs:42:9` in a panic backtrace, and land on column 9 of a line containing a non-ASCII character earlier in it.
