# 0046. One diagnostics model, keyed by `(source, uri)`

## Status

Accepted

Amends [ADR-0040](0040-build-core.md) §4 ("Build diagnostics join the existing Problems dock").
That section's claim — that a build diagnostic is shaped like an `lsp_core::DiagnosticStore` row and therefore belongs in the same dock — was true of the *shape* and false of the *store*: `BuildServiceRust` kept its own flat `Vec`, never the LSP store, so the two only ever met at the FFI seam (`ffi::FfiDiagnostic`), and only inside `ProblemsPanel`.
This ADR gives them the one store §4 assumed they already shared.

## Context

The PHP tooling plan (`docs/architecture/php-tooling-plan.md`) needs a third diagnostic source — PHPStan and PHPCS findings, and later PHPUnit failures — to reach the editor as squiggles and the Problems dock as rows, the same as a language server's or a build's.
Reading the actual code surfaced two problems that already existed before any PHP work started:

1. **Inline squiggles have exactly one source.**
   `EditorTabs::applyDiagnostics()` (`ui-shell/cpp/editor_tabs_lsp.cpp`) read only `LanguageService::diagnosticsForFile`.
   `BuildService`'s diagnostics reached the Problems dock — `ProblemsPanel::refresh()` merged the two at read time — but never the editor.
   A failing build has never underlined the failing line; it has only ever listed it in a panel.
2. **There were two diagnostic stores, not one model.**
   `lsp_core::DiagnosticStore` was keyed by URI alone, with replace-by-URI semantics, four severities and a real end range.
   `BuildServiceRust::diagnostics` was a flat `Vec<BuildDiagnostic>`, three severities, no end range, cleared and rebuilt per build.
   A third publisher writing into the LSP store as it stood would have clobbered a language server's rows for the same file, because `replace(uri, …)` owned the whole file regardless of who published it.

Adding an `analyzers` contribution point (a later phase) on top of that shape would have meant either a fourth flat `Vec` bolted onto a fourth adapter, or the same URI-clobbering bug a build diagnostic was one edit away from tripping today.

## Decision

### 1. A new Qt-free crate, `diagnostics-core`, not a growth of `lsp-core`

`lsp-core` cannot own the shared model: `build-core` and a future `analysis-core`/`test-core` would then depend on an LSP client to report a linter warning or a build error, which is exactly backwards (a build or a linter knows nothing about the Language Server Protocol).
Nor can the model live in `build-core`, which is about invoking build tools, not about being the one place every tool's findings converge.
`diagnostics-core` owns `Diagnostic`, `Severity` (four variants — the richer LSP enum, since a three-value build-tool enum was always a lossy subset), `DiagnosticRow`, `DiagnosticCounts`, and the `DiagnosticStore` itself, with dependencies on nothing but `std` and `serde_json`.

### 2. The store is keyed by `(source, uri)`, not by `uri` alone

`source` identifies the *publisher* — `"lsp:<language_id>"` for a language server, `"build:<toolchain>"` for a build — not the per-diagnostic display label (`Diagnostic::source`, e.g. `"rustc"`), which stays a plain field on the row.
`replace(source, uri, diagnostics)` only ever touches that one publisher's rows for that one file: a build's diagnostics for `main.rs` and rust-analyzer's diagnostics for the same file coexist, and a new build clearing its own rows (`clear_source`) never touches the language server's.
This is the one piece of new behaviour the whole ADR exists to buy — everything else is moving code that already existed to a shared home.

### 3. `lsp-core` converts at ingest; the raw payload rides along

`lsp_core::diagnostics::to_diagnostics` turns a `publishDiagnostics` batch into `Vec<diagnostics_core::Diagnostic>`, mapping LSP's `Option<DiagnosticSeverity>` onto the shared four-value enum and carrying the diagnostic's own JSON verbatim on `Diagnostic::raw`.
That field exists for exactly one reader, `LspManager::intentions`: several servers compute a code action from a diagnostic's own opaque `data` field and refuse to offer one for a diagnostic not handed back exactly as they sent it, so `DiagnosticStore::diagnostics_at` returns each covering row's raw JSON rather than a re-serialised approximation of it.
A build diagnostic's `raw` is always `None` — there is no protocol to hand it back to — and `diagnostics_at` filters those out for free.

`lsp_core::DiagnosticStore`/`DiagnosticRow`/`Severity` are gone; `path_from_uri`/`uri_from_path` moved to `diagnostics-core` (the one place a `DiagnosticRow`'s `path` is derived from its `uri`) and are re-exported from `lsp_core` unchanged, so the many existing `lsp_core::uri_from_path` call sites in `ui-shell` needed no edits.

### 4. `build-core`'s severity is the shared enum, not a translated one

`BuildDiagnostic::severity` is `diagnostics_core::Severity` directly.
The three-to-four mapping ADR-0040 implicitly left for the seam to do (`Severity::Note` → `FfiSeverity::Information`) no longer exists anywhere, because there is only one `Severity` to begin with; an unrecognised tool word (`Severity::from_word`, now the free function `severity_from_word` since an inherent method on a re-exported foreign type isn't possible) maps onto `Information` directly.

### 5. `ui-shell` owns the one store instance; `DiagnosticsService` is the only reader

`registry::SharedDiagnostics` — previously a `thread_local` `Rc<RefCell<lsp_core::DiagnosticStore>>` — now wraps `diagnostics_core::DiagnosticStore`, shared by `LanguageService` (writer), `BuildService` (writer), the new `DiagnosticsService` QObject (reader), and `AiChat` (reader, for `attachDiagnostics`).
`LanguageService::diagnostics`/`diagnosticsForFile`/`diagnosticCounts` and `BuildService::diagnostics` are removed rather than kept as a second way to ask the same question; `LanguageService`/`BuildService` keep their `diagnosticsChanged` signals, now meaning only "my part of the store changed", not "here is what changed".
`ProblemsPanel::refresh()` and `EditorTabs::applyDiagnostics()` read `DiagnosticsService` exclusively.

This is what makes finding 1 disappear as a side effect rather than a separate fix: `EditorTabs::wireDiagnosticsService` wires `applyDiagnostics()` to *both* `LanguageService::diagnosticsChanged` and `BuildService::diagnosticsChanged`, so a build's rows reach the editor for the first time.
`build/mod.rs`'s `to_diagnostic_core` also corrects an off-by-one that finding 1 had hidden: a build tool's line/column are 1-based, `diagnostics-core`'s `Position` is 0-based like LSP's, and the old code passed a build diagnostic's raw 1-based column into a struct whose column the editor already treated as 0-based — invisible while build diagnostics never reached anything position-sensitive, wrong the moment they did.

## Alternatives considered

| Option | Why rejected |
|---|---|
| Grow `lsp_core::DiagnosticStore` to accept a `source` parameter, keep it in `lsp-core` | Makes every non-LSP publisher (`build-core`, and later `analysis-core`/`test-core`) depend on an LSP client for a shared model that has nothing to do with the protocol. |
| Keep two stores, merge only at the FFI seam (what `ProblemsPanel::refresh()` already did) | Reproduces finding 1 for every future source: each new publisher needs its own bespoke merge into every consumer, and the editor's squiggles — which never merged at all — stay one-source forever. |
| A three-value `Severity` shared type, LSP's `Information`/`Hint` collapsed onto it | Throws away information a real language server sends; the four-value LSP enum is the richer one, and a build tool's coarser vocabulary maps onto it losslessly in the other direction. |

## Consequences

- Positive: adding a fourth diagnostic source (an analyzer, a test run) is a `(source, uri)` key and a converter function, not a new store, a new merge, or a new squiggle path.
- Positive: a build failure is now visible without opening the Problems dock — the regression this ADR's own unit and integration tests, plus the manual code-reading verification in the PHP tooling plan's phase A, exist to catch.
- Positive: `path_from_uri`/`uri_from_path` have exactly one implementation instead of a second one waiting to be written for `diagnostics-core`.
- Negative / accepted: `DiagnosticsService` adds a fourth QObject to a window that already has `LanguageService` and `BuildService`; it is intentionally a thin, thread-free reader (no `cxx_qt::Threading` impl) rather than folded into either producer, so neither producer QObject has to answer for a question it no longer owns.
- Negative / accepted: the store's `source` key (a publisher id like `"lsp:rust"`) and a `Diagnostic`'s own `source` field (a display label like `"rustc"`) are two different strings that happen to often look similar; a reader of the code has to keep that distinction straight, documented on both fields rather than solved by naming one of them something more different, since both names are already the honest description of what they hold.

## The E2E budget

`docs/architecture/run-build-debug-parity-plan.md` and `docs/architecture/terminal-shells-plan.md` both record the flow budget's ceiling as 15, reached in the run/build/debug parity work — "Once the budget is full, adding a flow means deleting one" (ADR-0024).
This phase (A) adds no E2E flow; the phase-A regression is verified by `diagnostics-core`'s and `lsp-core`'s unit tests plus a manual/static read of `EditorTabs::applyDiagnostics`'s wiring, per the plan's own phase-A verification section.
A later phase (E2 in the PHP tooling plan) adds `e2e_analyzer_findings_appear_inline_and_in_problems`, moving the ceiling from 15 to 16.
The justification recorded there, restated here since this ADR is what makes that flow meaningful to write: a diagnostic source whose rows never reach the editor is exactly the bug class finding 1 above describes, and it shipped once already (build diagnostics, from ADR-0040 until this ADR) — the new flow exists to make sure a fourth source cannot repeat it silently.

## Related

- [ADR-0040: `build-core`](0040-build-core.md) — the ADR this amends.
- [ADR-0024: verification foundation](0024-verification-foundation.md) — the E2E flow budget this ADR's last section discusses.
- `docs/architecture/php-tooling-plan.md` — the plan this ADR's phase A belongs to; phases B-E build the analyzer/test-runner surfaces this ADR's store is the foundation for.
