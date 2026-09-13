# IntelliJ-parity refinement plan

## Context

Measured against the IntelliJ IDEA feature list, this IDE's *breadth* is already close: completion, intentions, refactoring, navigation, Search Everywhere, Git, run/build/debug, a test runner, a terminal, plugins, colour themes and UI languages all exist.
What still separates it from IDEA is *depth*: many features stopped at their first working version.
This plan deliberately refines what exists rather than adding features; live templates, bookmarks, a TODO window, local history, a spell checker and breadcrumbs are new features and stay out of scope.

The audit behind it swept every plan document's Progress table and "Known gaps" section, every ADR's accepted ceilings, the thirty `ponytail:` ceiling markers in `crates/`, the open issue list, and a per-capability read of `crates/ui-shell/cpp`, `lsp-core`, `edit-ops`, `vcs-core`, `dap-core` and `index-core`.
One pattern recurs across it: **the Rust core is ahead of the Qt view**.
A rule is implemented and tested in a Qt-free crate, exposed over cxx-qt, and then never wired to a widget — `edit_ops::indent_selection`, `stage_hunk`/`unstage_hunk`, `configureBreakpoint`, `runToCursor`, `setVariable`, `removeWatch`, `format_range` and the debugger's cached thread list all have zero C++ callers today.
Closing such gaps is cheap and visible because the rule already exists; only the humble view is missing.

Items are ranked by daily user-visible pain multiplied by how much of the rule already lives in a Qt-free crate.
Each item is delivered as its own branch and pull request; none depends on another.
Every item follows the standing rules: rules and tests in Qt-free crates, `bridge.rs` translation only, `cpp/` a humble view with `tr()`-wrapped strings, `make test` and `make lint` green before each commit, and one headless E2E flow per item inside the existing flow budget.

## R1 — Indentation, Tab and bracket basics

### Status quo

- `Tab` in the editor is handled nowhere in `cpp/` except inside the completion popup (`crates/ui-shell/cpp/code_editor.cpp:224`).
  It falls through to `QPlainTextEdit`, which inserts a literal `\t` regardless of the "use spaces" setting and *replaces* a selection instead of indenting it.
  `Shift+Tab` does nothing.
- `edit_ops::indent_selection` / `unindent_selection` (`crates/edit-ops/src/indent.rs`) are tested and exposed through `crates/ui-shell/src/bridge/editor_ops.rs` with zero C++ callers.
- Enter copies the base indent and adds one unit after an unmatched opener or a trailing `:`.
  There is no "Enter between `{` and `}` opens a three-line block" and no dedent when a closing bracket is typed on a whitespace-only line.
- Bracket matching is jump-only (`edit.matchingBracket`); the pair under the caret is never highlighted.
- `wrap_column` is persisted by `settings-model` and shown on the Editing page, but nothing reads it: the editor is hard `NoWrap` and paints no guide line.
- With more than one caret, arrows, Home and End collapse to the primary caret (ADR-0023 ceiling).

### Target

- Tab and Shift+Tab indent and unindent the selected lines for every caret, and insert the configured unit at a bare caret.
- Enter between a bracket pair opens a three-line block; a closer typed on a whitespace-only line dedents that line.
- The matching bracket pair is highlighted while the caret sits on either side; an unmatched closer is highlighted in the error colour.
- `wrap_column` paints a vertical guide line, and a new soft-wrap setting plus View-menu toggle turns on `QPlainTextEdit::WidgetWidth` wrapping.
- Arrow, Home, End and word-move keys keep every caret.

### Plan

1. `edit-ops`: add `enter_between_pair` and `dedent_closer` returning a `Transaction`, and `brackets::pair_at(text, offset)` returning both ranges plus a matched flag, each with unit tests beside the existing ones.
2. `editor-core::selection`: add caret-motion operations on `SelectionSet` (left, right, up, down, home, end, word) so multi-caret movement is a tested rule rather than view code.
3. Bridge (`editor_ops.rs`): expose `moveCarets`, `enterBetweenPair`, `dedentCloser` and `bracketPairAt`.
4. View: route Tab/Backtab to `indentSelection`/`unindentSelection`, route Enter and closer keys through the new operations before the default path, route arrow keys through `moveCarets` when more than one caret exists, paint the pair through `extraSelections` with the theme's existing colours, and apply `wrap_column` and the new soft-wrap flag on the editor.
5. Keymap: `edit.indent`, `edit.unindent`, `view.toggleSoftWrap`.
6. E2E: select three lines, Tab, Shift+Tab, Enter between braces, assert the buffer text.

Files: `crates/edit-ops/src/{indent,brackets}.rs`, `crates/editor-core/src/selection.rs`, `crates/settings-model/src/editing.rs`, `crates/ui-shell/src/bridge/editor_ops.rs`, `crates/ui-shell/cpp/{code_editor,editing_actions,editing_page}.cpp`, `crates/app-config/src/keymap.rs`.

## R2 — Completion popup depth

### Status quo

- Filtering is a case-insensitive prefix match on `filterText` (`crates/lsp-core/src/completion.rs`); there is no CamelHump or subsequence matching, no exact-match-first and no recency.
- Rows are plain `QStandardItem` labels with no kind icon, no detail column and no strike-through for deprecated items.
- Documentation is the row's Qt tooltip: plain text, not scrollable.
- Snippets are flattened to plain text — placeholders become their defaults and `$0` is dropped — and the client advertises `snippetSupport: false` (ADR-0016 ceiling).
- The popup hides on any caret move, including Left and Right inside the typed word, there is no debounce, and without a language server Ctrl+Space does nothing at all.

### Target

- Ranking is exact, then prefix, then CamelHump, then subsequence, with `sortText` breaking ties, and the matched characters are highlighted in the row.
- Rows carry a kind icon, the label, a right-aligned detail column, and a strike-through for deprecated items.
- Documentation renders as Markdown in a scrollable side panel next to the popup, resolved lazily as today.
- Snippet tab stops work: accepting inserts the snippet, the caret lands on `$1`, Tab and Shift+Tab cycle the stops, Escape or `$0` ends the session, and the client advertises `snippetSupport: true`.
- Keyword and document-word completion fill in when no server answers, so Ctrl+Space always offers something.
- The popup survives Left and Right within the word, and auto-popup is debounced by 50 ms.

### Plan

1. `lsp-core::completion`: a `MatchKind` plus a scorer returning the kind and matched positions, replacing the prefix filter, with tests for CamelHump, subsequence and tie-breaks.
2. New `edit-ops::snippet`: parse the LSP snippet grammar into text plus tab-stop ranges, with tests for `${1:name}`, `$0`, nesting and escapes.
3. `editor-core`: a `SnippetSession` (active stop, mirrored ranges) on the document; multi-caret already provides mirrored placeholders.
4. `syntax-core`: `keywords(language_id)` derived from the grammar's node kinds; `editor-core::words_in(text)` for document words.
5. Bridge: completion rows gain kind, detail, deprecated and match positions; new `snippetNext`, `snippetPrev`, `snippetEnd` slots.
6. View: a `QStyledItemDelegate` for icon, label, detail and highlight; a `CompletionDocsPanel` (`QTextBrowser`) beside the popup rendering through `markdown-preview`'s HTML; Tab and Shift+Tab routed to the snippet session while one is active.
7. E2E: type `fBr`, assert `fooBar` is the first row; accept a snippet, press Tab, assert the caret position.

Files: `crates/lsp-core/src/completion.rs`, `crates/lsp-core/src/client_capabilities.rs`, `crates/edit-ops/src/snippet.rs` (new), `crates/editor-core/src/lib.rs`, `crates/syntax-core/src/lib.rs`, `crates/ui-shell/src/bridge/language/mod.rs`, `crates/ui-shell/cpp/code_editor.{h,cpp}`, new `crates/ui-shell/cpp/completion_delegate.cpp` and `completion_docs_panel.cpp`.

## R3 — Quick documentation, signature help and hover as real popups

### Status quo

- Hover, signature help and completion documentation are all `QToolTip` (`crates/ui-shell/cpp/editor_tabs.cpp`, `signature_tip.cpp`): transient, non-interactive, not scrollable, no links.
- Hover Markdown goes through a hand-rolled mini-renderer that handles code fences, bold and rules only; lists, links and tables render raw.
- There is no Ctrl+Q quick-documentation action, and hover fires only on identifier-shaped words, so hovering a squiggle never shows the diagnostic message.
- Signature help offers no overload cycling and drops per-parameter documentation.

### Target

- One `EditorPopup` widget: a frameless frame with a `QTextBrowser` body, a size cap, Escape or click-outside to close, and Ctrl+Q to pin.
- The hover popup shows the LSP hover plus every diagnostic at that offset (message, source, code) and its quick-fix count, with Markdown rendered by `markdown-preview`.
- The signature popup cycles overloads with Up and Down and shows the active parameter's own documentation.
- Ctrl+Q opens documentation at the caret without moving the mouse, and documentation links open in the browser.

### Plan

1. `lsp-core::hover` returns structured `HoverContent { markdown, range }` and the mini-renderer is replaced by `markdown_preview::render_html`; `signature_help` surfaces the per-parameter docs it already parses.
2. `diagnostics-core`: a `DiagnosticStore::at(uri, offset)` query, reusing the existing range query if one fits.
3. Bridge: `hoverAt(tab, offset)` composes hover and diagnostics into one HTML string through a tested Qt-free helper.
4. View: `editor_popup.{h,cpp}` replaces the three `QToolTip::showText` call sites; keymap gains `code.quickDocumentation` (Ctrl+Q).
5. The hover trigger widens from "identifier word" to "identifier word or inside a diagnostic range".
6. E2E: open a file with a build diagnostic, hover it, assert the popup text.

Files: `crates/lsp-core/src/{hover,signature_help}.rs`, `crates/diagnostics-core/src/lib.rs`, `crates/ui-shell/src/bridge/language/{mod,lsp_surface}.rs`, `crates/ui-shell/cpp/{editor_tabs,editor_tabs_lsp,signature_tip,code_editor}.cpp`, new `crates/ui-shell/cpp/editor_popup.cpp`.

## R4 — Diagnostics navigation and the error stripe

### Status quo

- Squiggles and the Problems dock work, but there is no way to walk errors: the keymap has no next/previous-diagnostic action.
- Error marks exist only in the minimap; with the minimap off there is no stripe, and the marks are not clickable.
- No gutter icon marks a diagnostic line, a Problems row offers no quick fix, and the dock has no current-file scope.
- The status-bar problems button counts globally, never per file.

### Target

- F2 and Shift+F2 move to the next and previous diagnostic in the file, wrapping, errors before warnings.
- A thin error stripe beside the vertical scrollbar, independent of the minimap: severity-coloured ticks, a hover tooltip with the message, click to jump, and a file-level summary in the top corner.
- A gutter icon on every diagnostic line that opens the intentions popup on click.
- The Problems dock offers quick fixes from a row's context menu and a current-file scope toggle.

### Plan

1. `diagnostics-core`: `next_after`, `prev_before` and `summary(uri)` with tests for wrapping and severity filtering.
2. Bridge: `nextDiagnostic`, `diagnosticSummary`, and the stripe rows shared with the minimap's existing FFI list rather than a second one.
3. View: `error_stripe.{h,cpp}` mapped through the vertical scrollbar the way ADR-0044's minimap is; the gutter icon in `code_editor_gutter.cpp`; the Problems context menu calling `showIntentions`.
4. Keymap: `code.nextDiagnostic`, `code.previousDiagnostic`, with Navigate-menu entries.
5. E2E: a file with two build diagnostics, F2 twice, assert the caret lines.

Files: `crates/diagnostics-core/src/lib.rs`, `crates/ui-shell/src/bridge/language/lsp_surface.rs`, `crates/ui-shell/cpp/{problems_panel,code_editor_gutter,status_bar,minimap}.cpp`, new `crates/ui-shell/cpp/error_stripe.cpp`, `crates/app-config/src/keymap.rs`.

## R5 — Debugger UI: wire the core that already exists

### Status quo

- The breakpoint model supports condition, hit count, log message, temporary and depends-on (`crates/dap-core/src/breakpoints.rs`), and the bridge exposes `configureBreakpoint`, but no widget calls it and the gutter marker has no context menu.
- `runToCursor`, `setVariable`/`canSetVariable` and `removeWatch` are exposed and unused.
- Threads are cached in the bridge but the panel has no thread selector.
- The variables tree cannot edit, copy or filter; watches are a plain `QListWidget` with no remove, edit or expansion; the call stack is a flat list; evaluate is single-line text into the console with no history.
- Function and data breakpoints exist in the core with no UI.

### Target

- Right-click on a gutter marker opens "Edit Breakpoint…" (condition, hit count, log message, temporary, enabled) and "Remove"; Ctrl+Shift+F8 opens a Breakpoints window listing every breakpoint with those fields; Alt+click sets a temporary breakpoint.
- Run to Cursor (Alt+F9) in the menu, the gutter context menu and the keymap.
- A threads combo above the frames list; double-click on a frame jumps to source.
- Variables support inline edit where `canSetVariable`, Copy Value, Copy Path and a filter box; watches become a tree with add, edit, remove and expansion; evaluate renders its result as a tree and keeps an expression history.

### Plan

1. `dap-core`: feed `evaluate`'s `variablesReference` into the same variable-node tree the Variables view uses; a tested history ring buffer.
2. Bridge: `threads`, `selectThread`, `evaluateToTree`, `watchChildren`; `configureBreakpoint` unchanged.
3. View: `breakpoint_dialog.cpp`, `breakpoints_window.cpp`, the gutter context menu, and a `debug_panel.cpp` with a thread combo, tree views for watches and evaluate, and an edit delegate for values.
4. Keymap: `debug.runToCursor`, `debug.viewBreakpoints`, `debug.editBreakpoint`.
5. Verification: extend the debugpy row of the run-build-debug parity plan's manual matrix with conditional-breakpoint and set-variable steps; unit tests cover the rules.

Files: `crates/dap-core/src/{breakpoints,session}.rs`, `crates/ui-shell/src/bridge/debug/mod.rs`, `crates/ui-shell/cpp/{debug_panel,debug_menu,editor_tabs_debug,code_editor_gutter}.cpp`, new `crates/ui-shell/cpp/breakpoint_dialog.cpp` and `breakpoints_window.cpp`.

## R6 — Staging and commit workflow

### Status quo

- `stage_hunk`/`unstage_hunk` exist in `vcs-core` and the bridge, but no widget calls them: the gutter popup's "Stage File" stages the whole file because the core's hunks diff against `HEAD` rather than the index, which `changes_panel.h` documents as "increasingly wrong the more of the file is already staged".
- `hunk_patch` emits no `\ No newline at end of file` marker, so staging a hunk that touches a newline-less last line breaks.
- Commit has no message history, Amend always demands a fresh message, and there is no author override or sign-off.
- Nothing is coloured by VCS status: the project tree and the editor tabs ignore `FfiChangeKind`, ignored files are not dimmed, and there is no "Add to .gitignore".
- The hunk popup shows no removed text, and the only comparison is "Compare with HEAD".

### Target

- Correct per-hunk stage and unstage: staging hunks are computed index-blob versus worktree while gutter markers stay `HEAD` versus worktree, painted in three states (unstaged, staged, both) as IDEA does; the gutter popup gains "Stage Hunk" and Changes-dock rows expand to hunks with their own checkboxes.
- A commit-message history dropdown (last 25, per project), Amend pre-filled with `HEAD`'s message, optional author override and `--signoff`.
- Project tree and editor tabs coloured by status (modified, added, untracked, ignored, conflicted) with the theme's VCS colours; "Add to .gitignore" in the tree's context menu.
- The hunk popup shows the removed lines inline; "Compare with Branch, Tag or Revision…" on a file and on the project root.

### Plan

1. `vcs-core`: `hunks_against_index(path)` reading the index blob through `gix`; the no-newline marker in `hunk_patch`; commit-message history persisted in `.ide/vcs-state.toml` through `app-config`; `head_message`; `ignored_paths` from `git status --porcelain=v2 --ignored`; tests for the marker and the three-state colouring.
2. `app-core`: a per-path `ChangeKind` role on the Rust-backed tree model.
3. Bridge: `stageHunk` reworked onto the index diff; `changeKind(path)`; `commitHistory`.
4. View: the gutter popup and Changes-dock hunk rows, the commit combo, a coloured delegate in `project_tree_dock.cpp`, tab text colour in `editor_tabs.cpp`, and the new tree-menu entries opening the existing `DiffView`.
5. E2E: modify two hunks, stage one, assert `git diff --cached` contains only it.

Files: `crates/vcs-core/src/{staging,hunks,status,lib}.rs`, `crates/app-core/src/lib.rs`, `crates/ui-shell/src/bridge/vcs/{staging,mod}.rs`, `crates/ui-shell/cpp/{changes_panel,editor_tabs_vcs,vcs_gutter,project_tree_dock,project_tree_git_menu,editor_tabs}.cpp`.

## R7 — Git log, blame and branch operations

### Status quo

- The log is a paged flat list with no graph, no filters (author, date, path, text), no ref decorations, no branch selector and no row actions.
- Blame is a passive 220-pixel text column with no hover, no click-through to the commit and no age colouring.
- The branch popup is a flat `QMenu` offering checkout, create and delete; there is no merge, rebase, rename, remote branch list or search, `vcs-core` has no merge, rebase or stash at all, and the remote is hard-coded to `origin`.

### Target

- Log: a filter bar (text, author, date range, path), branch and tag chips on rows, a branch-scope selector, a simple lane graph, and a row context menu with Checkout Revision, New Branch Here, Cherry-pick, Revert Commit, Reset Current Branch Here (soft, mixed, hard with confirmation) and Copy Hash.
- Blame: a hover tooltip with the full message and date, click to select the commit in the log, age-shaded backgrounds, and the toggle remembered per file.
- Branch popup: Local and Remote sections with search, per-branch Checkout, Merge into Current, Rebase Current onto, Rename, Delete, Push and Compare with Current; Stash and Unstash under the VCS menu; a remote picker wherever `origin` is hard-coded.

### Plan

1. `vcs-core`: `log(LogFilter)` over a `gix` revwalk with path, author, date and message filters; `refs_by_commit`; a small tested `lanes` graph layout; CLI-backed `merge`, `rebase`, `cherry_pick`, `revert`, `reset`, `stash_push`/`pop`/`list`/`drop`, `rename_branch` and `remotes`, each with typed errors including the "conflict" and "no upstream" variants ADR-0031 names as missing.
2. `app-config`: per-project blame toggle memory.
3. Bridge: `commitLog(filter, page)`, `commitRefs`, `branchActions`, `stashList`.
4. View: the log filter toolbar, a delegate for chips and lanes, the row context menu, blame hover and click in `code_editor_gutter.cpp`, and `branch_popup.cpp` replacing the flat menu; a conflict after merge or rebase lands in the existing Merge Conflicts group with "Resolve…" opening the two-pane `DiffView`.
   A full three-way merge editor stays out of scope and is recorded as such in the ADR amendment.
5. Amend ADR-0031 for the new write operations.
6. E2E: create a branch, commit, merge through the popup, assert `git log --oneline`'s head.

Files: `crates/vcs-core/src/{log,branch,remote,lib}.rs`, new `crates/vcs-core/src/{stash,graph}.rs`, `crates/ui-shell/src/bridge/vcs/*.rs`, `crates/ui-shell/cpp/{commit_log_panel,history_list_view,commit_detail_view,code_editor_gutter,vcs_menu}.cpp`, new `crates/ui-shell/cpp/branch_popup.cpp`, `docs/architecture/decisions/0031-git-backend.md`.

## R8 — Find in Files and Find Usages depth

### Status quo

- Find in Files offers Regex, Match Case and Replace All only: no file mask, no scope, no whole word, no preview pane, a modal replace preview, and a 10 000-match cap with no "showing N of M".
- Regex and case-insensitive queries skip ngram narrowing and scan every indexed file.
- Find Usages is name-based through the index only — no LSP `textDocument/references` — shown as a flat list with no grouping and no scope.
- Search Everywhere has no preview and no per-tab filters, and symbol rows carry no match positions.

### Target

- Find in Files: a file mask (`*.rs, !*_test.rs`), a scope combo (Project, Directory…, Open Files, Current File), Whole Word, results grouped by directory then file with a read-only preview pane, "N of M — refine" when capped, and per-match exclusion before Replace.
- Find Usages: LSP references when a server is up with the index as fallback, results grouped by file with read/write kind where the server reports it, a scope, and the same preview pane.
- Regex queries extract a literal prefix for ngram narrowing.

### Plan

1. `index-core`: a `SearchScope` (paths, `globset` mask, open-only) applied at the candidate stage; literal-prefix extraction for regex through `regex-syntax`, with tests; a `whole_word` flag; a `total_hint` for the cap.
2. `lsp-core::references`: the request plus a tested group-by-uri helper.
3. `app-core::find_usages` chooses LSP or index, as a tested rule.
4. Bridge: extended `searchProject` parameters; `usages(tab, offset, scope)`.
5. View: the toolbar (mask, scope, whole word), a grouped `QTreeView` model, `search_preview_pane.cpp` reusing the read-only `CodeEditor` pattern from the diff pane, and `find_usages_panel.cpp` migrated onto the same tree and preview.
6. E2E: search with a mask, assert only matching files are listed.

Files: `crates/index-core/src/lib.rs`, new `crates/lsp-core/src/references.rs`, `crates/app-core/src/lib.rs`, `crates/ui-shell/src/bridge/search.rs`, `crates/ui-shell/cpp/{search_results_panel,find_usages_panel,search_everywhere_dialog}.cpp`, new `crates/ui-shell/cpp/search_preview_pane.cpp`.

## Considered and deferred

- LSP incremental document sync and a captured server `stderr` log view (ADR-0016 ceilings): infrastructure with low daily visibility; pick up when a server is slow enough to notice.
- Terminal search, split and OSC title: real gaps, folded into the terminal experience plan still in delivery.
- Reformat selection (`format_range` exists, unwired) and format-on-save: small enough to ride on R1 or R3 as a follow-up.
- Rename in comments and strings, and cross-file undo: refactoring-plan known gaps that need an undo-journal design first.
- Live templates, bookmarks, a TODO window, local history, spell check, breadcrumbs: new features, outside this plan's brief.

## Progress

| # | Item | Status | Commit |
|---|---|---|---|
| R1 | Indentation, Tab and bracket basics | done | 3061c31 |
| R2 | Completion popup depth | done | 88af3d4 |
| R3 | Quick documentation, signature help and hover as real popups | open | |
| R4 | Diagnostics navigation and the error stripe | open | |
| R5 | Debugger UI: wire the core that already exists | open | |
| R6 | Staging and commit workflow | open | |
| R7 | Git log, blame and branch operations | open | |
| R8 | Find in Files and Find Usages depth | open | |
