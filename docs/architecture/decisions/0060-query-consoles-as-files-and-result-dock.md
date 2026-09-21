# 0060. Query consoles are files; results and data editing live in one windowed dock

## Status

Proposed.
Implemented by [the database-tools plan](../database-tools-plan.md)'s F3 (console + grid) and F4 (data editor) phases.
States that [ADR-0020](0020-tab-kinds-and-the-binary-viewer.md) and [ADR-0036](0036-virtual-documents.md) need no amendment; the [ADR-0033](0033-markdown-preview.md)/[ADR-0043](0043-preview-mode-and-mermaid-documents.md) preview dock is reused unchanged for ER diagrams (a later phase, F6).

## Context

A JetBrains query console is, structurally, two things at once: a place to type SQL with the editor's full feature set (completion, syntax highlighting, multi-caret, undo), and a place bound to one data source, one schema, one transaction mode, that remembers its own history and reopens where it was left.
A result grid is a third thing again: potentially a million rows behind a cursor that must not be pulled into memory at once, editable in place, with its own paging, filtering, and a DML preview before anything is written back.

Two seams already exist that a console or a result grid could try to reuse, and both were examined before deciding to add nothing new to either.

1. **`TabKind`** (ADR-0020): `Text`, `Binary`, `Diff`, `Image`. A console is textual SQL — it fits `Text` exactly as-is, with language `sql`, the same way any other `.sql` file would open. A result grid is not a document at all; it has no text representation a `CodeEditor` could show, and forcing one through `TabKind` would mean inventing a fifth kind whose "text" is actually a live, paged, mutable table — a shape none of the existing tab machinery (undo, dirty tracking, save) was built to represent.
2. **Virtual documents** (ADR-0036): `DocumentSource::{File, Virtual}` already gives this codebase a read-only, no-backing-file tab (`csharp:/` decompiled metadata). Go-to-DDL and a DML preview are exactly that shape — synthesized text with nowhere to save. But a query console is not read-only and is not synthesized; it is an ordinary file the user's own edits should persist to disk like any other, which virtual documents are not built to do (ADR-0036's read-only guard exists specifically so a synthesized document can never be typed into and silently lose the edit).

## Decision

### 1. A console is a real `.sql` file, opened as `TabKind::Text`

Every console is a file at `<config_dir>/consoles/<source-id>/console-<n>.sql`, created on "New console" and opened exactly the way any other file opens — `TabKind::Text`, language `sql` from the existing extension table (ADR-0018), full editing, undo, save, everything a `.sql` file already gets for free.
`db_core::console::source_of(path)` is a pure function from a console's path back to the source id it belongs to (the directory name), so nothing needs a side-table mapping open tabs to data sources; a `.sql` file living *outside* `consoles/` (a project file the user opened directly and wants to run against a source) is bound instead through the persisted `[database] file_sources` map (`path -> source-id`), read the same way and falling back to "no source bound yet, ask" when the map has no entry.
No new `TabKind` is added — ADR-0020's four kinds are unchanged; a console needed nothing that `Text` did not already give it.

### 2. Go-to-DDL and the DML preview are virtual documents, unchanged

Go-to-DDL opens a synthesized `db-ddl:/<source-id>/<object-path>` document — the DDL text `db_sql::ddl` reconstructs from the introspected schema (or the engine's own `SHOW CREATE`/`pg_get_tabledef`-equivalent where one exists) — through the exact `DocumentSource::Virtual` path `csharp:/` metadata already established: read-only, no save, the clean-refusal guard ADR-0036 built stops it from being typed into by accident.
The DML preview (F4: "here is the SQL your pending edits will run before you submit") is the same shape again — a synthesized, disposable text document, not a config or a draft the user could accidentally persist as their actual edit.
Neither needed ADR-0036 amended at all; both are exactly the case that ADR-0036 generalized `csharp:/`'s pattern for, restated here because Database Tools is the feature that actually needed a second consumer.

### 3. Results and the data editor are one dock, over a windowed table model — not a document

`databaseResults`, a bottom ADS dock (contributed via [ADR-0059](0059-tool-window-and-settings-page-contribution-points.md)'s generic `tool-windows` point), carries one tab per console — the same "N sessions, one dock, one tab each" shape `run_console_panel.cpp` already uses for run configurations.
Its content is `result_table_model.cpp`, the tree's first `QAbstractTableModel`: `rowCount()` answers from a count `db-core` already knows (or a lazily-growing estimate while the stream is still open), `data()` reads from an LRU of already-fetched pages, and Qt's own `fetchMore`/`canFetchMore` drive `ResultProvider::rowPage(resultId, first, count)` across the FFI seam — a genuinely paged model, not "all rows loaded, scrolled through," which is what makes the 1M-row NFR (§5 of the plan) achievable at all.
Editing a cell, adding/deleting/cloning a row, and submit/revert all mutate an `EditBuffer` in `db-core` first; nothing touches the database until "Submit" turns the buffer into a `DmlPlan` (bound parameters, quoted identifiers, one DML statement per pending change) and runs it — the DML preview from §2 is exactly a text rendering of that plan before it runs.
This is not a document with dirty-state tracking reused from `editor-core`: a result grid's "unsaved changes" are pending database writes, not an in-memory buffer diffed against a file, so it gets its own `EditBuffer`/`DmlPlan` model rather than stretching `Document`'s save/dirty machinery over a concept it was never built to represent.

## Alternatives considered

| Option | Why rejected |
|---|---|
| Writable virtual documents (extend ADR-0036 so `Virtual` documents can save) | The DML preview and Go-to-DDL both depend on a virtual document being *impossible* to accidentally edit and persist — that is the entire point of ADR-0036's clean-refusal guard. Making some virtual documents writable would mean every consumer of `DocumentSource::Virtual` has to re-check which kind it got, reopening exactly the ambiguity ADR-0036 closed. |
| A fifth `TabKind::DatabaseResult` | A result grid has no text, no save, no per-file identity the way `Text`/`Binary`/`Diff`/`Image` all do — it is scoped to a console/session, not a file. Forcing it into `TabKind` would mean either stubbing every tab-kind consumer (save, dirty-badge, file-watcher) for a kind that answers "not applicable" to all of them, or teaching those consumers a genuinely new axis (dock vs. tab) that a `TabKind` variant cannot express — a bottom dock already expresses it directly. |
| Result grid as a second kind of `Document` with its own dirty/save story | `editor_core::Document` owns dirty state for a file-backed text buffer (ADR-0003's FFI rule: Rust `Document` is the single source of truth for dirty state). A pending database edit is not "unsaved text," it is "a write not yet sent," and reusing `Document`'s save path would mean either a fake save target or a special-cased save that means something completely different from every other tab's — worse for a future reader than a purpose-built `EditBuffer`/`DmlPlan` pair that only this feature touches. |

## Consequences

- A console gets every editor feature (completion, syntax highlighting, multi-caret, undo, the file-watcher's external-change detection) for free, because it never stopped being an ordinary file — no seam had to learn a new kind of tab to make that true.
- Go-to-DDL and the DML preview are the second and third real consumers of ADR-0036's virtual-document machinery (the first being `csharp:/` decompiled metadata), proving the generalization ADR-0036 already claimed for itself rather than needing new code to restate it.
- The result grid's `EditBuffer`/`DmlPlan` pair is new, database-specific state that lives in `db-core`, not a stretch of `editor-core`'s document model — a future reader looking for "why does a grid edit not show a dirty-tab asterisk" finds the answer is "it was never a document," not a bug.
- `result_table_model.cpp` is the first `QAbstractTableModel` in this codebase's tree; its paging/LRU shape is the template a future large-dataset view (should one arise) would follow, rather than each inventing its own.
- `run-core` gains no new dependency and no new `LaunchSpec` variant: a `sql-script` run configuration is a `kind` tag `RunService` reads and routes to `ConsoleService` directly (the same kind-tag dispatch ADR-0056 already established for container run configurations), not a process `run-core` itself launches.

## Related

- [ADR-0020: tab kinds and the binary viewer](0020-tab-kinds-and-the-binary-viewer.md) — the four kinds a console fits into unchanged (`Text`), stated here as needing no fifth.
- [ADR-0036: read-only virtual documents](0036-virtual-documents.md) — the `DocumentSource::Virtual` shape Go-to-DDL and the DML preview reuse exactly, stated here as needing no amendment.
- [ADR-0033: Markdown and Mermaid preview](0033-markdown-preview.md) / [ADR-0043: preview mode and Mermaid documents](0043-preview-mode-and-mermaid-documents.md) — the preview dock ER diagrams ride unchanged in a later phase (F6), noted here for completeness since it was evaluated alongside the console/results question and found to need nothing new either.
- [ADR-0056: container run configurations](0056-container-run-configurations.md) — the kind-tagged run-configuration dispatch a `sql-script` config reuses rather than a new `run-core` edge.
- [ADR-0059: tool-window and settings-page contribution points](0059-tool-window-and-settings-page-contribution-points.md) — the generic point the `databaseResults` dock is contributed through.
- `docs/architecture/database-tools-plan.md` — the plan whose F3/F4 phases build the console, the result dock and the data editor this ADR shapes.
