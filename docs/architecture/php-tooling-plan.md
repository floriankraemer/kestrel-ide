# PHP tooling plan — PHPStan, PHP_CodeSniffer, PHPUnit

## Context

The IDE has PHP syntax highlighting (`syntax-core/src/catalog.rs:118`) and an Intelephense row in the LSP catalog (`lsp-core/src/catalog.rs:102`), and nothing else.
There is no way to see a PHPStan level-9 error, a PHPCS sniff violation, or a failing PHPUnit assertion without leaving the editor for a terminal.

The request is: run those three tools against a Composer project, autodetect their binaries and configuration files, and show what they report *inline in the editor*.
The secondary question — "do we have a generic API for this already?" — has a clear answer after reading the code: **no, and the closest thing has a hole in it.**

Three findings drive the whole design:

1. **Inline squiggles have exactly one source.**
   `EditorTabs::applyDiagnostics()` (`ui-shell/cpp/editor_tabs_lsp.cpp:446-485`) reads `languageService_->diagnosticsForFile(path)` and nothing else.
   `build-core`'s diagnostics reach the Problems dock but **never reach the editor** — `ProblemsPanel::refresh()` (`ui-shell/cpp/problems_panel.cpp:187-201`) is the single place the two sources are merged, and it is a panel, not the editor.
   That is a latent bug today and a blocker for a third source.

2. **There are two diagnostic stores, not one model.**
   `lsp_core::DiagnosticStore` (`lsp-core/src/diagnostics.rs:69`) is keyed by URI with replace-by-URI semantics, four severities, and a real end range.
   `BuildServiceRust.diagnostics` (`ui-shell/src/bridge/build/mod.rs:31`) is a flat `Vec` cleared per build, three severities, no end range.
   They meet only as `FfiDiagnostic` (`ui-shell/src/bridge/ffi.rs:474`) at the seam.
   A third publisher writing into the LSP store as it stands would clobber Intelephense's rows for the same file, because `replace(uri, …)` owns the whole file.

3. **The plugin host has no analysis surface.**
   `ContributionPoint` is `IconThemes | Commands | Previews | LanguageServers` (`plugin-api/src/manifest.rs:35`).
   `language-servers` is the only point that launches an external process, and diagnostics arrive only as a side effect of LSP.
   A wasm plugin cannot spawn a process at all (`wit/plugin.wit:36-48`), so an analyzer plugin must be **declarative data**, exactly as `language-servers` already is.

Intended outcome: opening a Composer project makes PHPStan and PHPCS findings appear as squiggles and Problems rows without configuration, and PHPUnit gets a real test tool window whose failures are also inline.
Adding ESLint, Ruff or Clippy afterwards is a manifest, not a code change.

## Decisions taken before work starts

**One diagnostics model, source-keyed.**
A new Qt-free `diagnostics-core` owns `Diagnostic`, `Severity` and a store keyed by `(source, uri)`.
`lsp-core` and `build-core` publish into it; so do analyzers and the test runner.
The editor underlines everything in it, which fixes finding 1 as a side effect rather than as a separate task.
Precedent: `stdio-framing` was *extracted* when `dap-core` needed `lsp-core`'s framing (D1-1) rather than copied — same move here.

**Analyzers are a declarative contribution point, and their output formats are a native table.**
`plugin-api` gains `analyzers` and `test-frameworks`.
Both are additive, so `api_version` stays 1 — the same reasoning ADR-0033 recorded for `previews` (`plugin-api/src/manifest.rs:144`'s flattened `unknown` map already makes an unknown point inert on an older host).
A manifest names the program candidates, argv, output *format id* and severity mapping; the format id resolves to a parser in `analysis-core`, because a wasm guest can neither spawn the tool nor be trusted with its bytes.

**Analyzers run over pipes, not a PTY.**
This is the one place the repo's PTY-for-everything default is wrong: `run_core::Supervisor` spawns at `PtySize { rows: 24, cols: 120 }` (`run-core/src/supervisor.rs:27`), and a tty hard-wraps output at 120 columns.
A wrapped line is survivable for `cargo`'s prose and fatal for PHPStan's JSON, PHPCS's XML and PHPUnit's TeamCity service messages.
PHPCS also wants content on stdin, which a PTY cannot give cleanly.

**One shared piped-exec helper, extracted rather than copied.**
`vcs-core/src/cli.rs:59-120` is already the only process path in the repo with a wall-clock timeout and per-pipe drain threads — precisely what an analyzer needs.
It becomes `process-exec`, and `vcs-core` moves onto it in the same commit.

**Trigger is configurable; the default is on-the-fly.**
Three triggers — `OnType` (debounced), `OnSave`, `Manual` — settable globally and per analyzer.
`OnType` and `OnSave` analyze **one file**; `Manual` analyzes the project.
An analyzer that cannot read an unsaved buffer degrades to `OnSave` and *says so* in the settings page rather than silently doing nothing.

**PHPUnit gets a real test tool window.**
A Tests dock with a suite/class/method tree, live status, durations, a failure pane and rerun — fed by PHPUnit's `--teamcity` service messages so the tree fills while the run is in flight, with `--log-junit` as the fallback format for frameworks that speak no TeamCity.
Failures publish diagnostics like any other source, so a failing assertion is underlined in the editor.

**Out of scope, stated once:** code coverage, PHP interpreter management/remote interpreters (Docker, SSH, WSL), Xdebug configuration, PHP-CS-Fixer/Rector/Psalm rows (they are manifest rows once the point exists, not work here), running a single test from a gutter icon (deferred to a follow-up, the tree's context menu covers rerun), and quick fixes for analyzer findings (needs per-tool rule identifiers, which `checkstyle-xml` carries but nothing consumes yet).

## Architecture

### New crates

| Crate | Layer | Why not an existing crate |
|---|---|---|
| `diagnostics-core` | support | The one Problems model. It cannot live in `lsp-core` (`build-core` and `analysis-core` would then depend on an LSP client to report a linter warning) and it cannot live in `build-core` (which is about invoking build tools). |
| `process-exec` | support | Extracted from `vcs-core/src/cli.rs`. Pipes, drain threads, optional stdin, wall-clock timeout, tree kill. |
| `analysis-core` | support | Analyzer definitions, detection, scheduling, output parsing. Not `build-core`: a build addresses a *project* on demand, an analyzer addresses a *file* on a keystroke, and the two have opposite invocation and cancellation shapes. |
| `test-core` | support | A test run is a tree with live per-node state and rerun selectors — a different domain from both. |

### Layering rows (to add to `docs/architecture/layering.md`)

| Crate | Allowed imports | Qt/cxx-qt |
|---|---|---|
| `diagnostics-core` | std, serde, serde_json | **No** |
| `process-exec` | std | **No** |
| `analysis-core` | `diagnostics-core`, `process-exec`, `plugin-api`, `syntax-core`, `app-config` (+ serde, serde_json, quick-xml) | **No** |
| `test-core` | `diagnostics-core`, `process-exec`, `plugin-api`, `app-config` (+ serde, quick-xml) | **No** |

`lsp-core` and `build-core` each gain a `diagnostics-core` row.
`vcs-core` gains `process-exec` and keeps everything else.
Neither new crate may take tokio: long work runs on a `std::thread` and returns through `CxxQtThread::queue()`, per the `build-core`/`dap-core` precedent.

### QObjects

`AnalysisServiceRust` and `TestServiceRust` — one registered `#[qobject]` each owning a `HashMap` of in-flight runs, per the ADR-0032 precedent (cxx-qt registers a type's `QMetaObject` once at build time).

`DiagnosticsServiceRust` becomes the single QObject the Problems dock and the editor read.
`LanguageService::diagnostics*` and `BuildService::diagnostics` are removed rather than left as a second way to ask the same question.

## Progress

Update the row **in the same commit** that finishes the task.
`open` = not started. `blocked on X` = cannot start until X lands.

### P0 — the plan itself

| Task | Status | Commit |
|---|---|---|
| P0-1 — this plan doc + `docs/README.md` index line | open | |

### A — one diagnostics model, and squiggles for every source

| Task | Status | Commit |
|---|---|---|
| A1 — `diagnostics-core`: `Diagnostic`, `Severity`, `(source, uri)`-keyed `DiagnosticStore`, `rows`/`rows_for_uri`/`counts`/`at` | done | `77c0dc8` |
| A2 — `lsp-core` converts at ingest; the raw protocol value is carried on the row so `LspManager::intentions` still round-trips it verbatim | done | `1d07d5c` |
| A3 — `build-core` publishes into the store; build diagnostics gain inline squiggles, which they have never had | done | `1d07d5c`, `7505df0` |
| A4 — `DiagnosticsServiceRust`; `ProblemsPanel` and `EditorTabs::applyDiagnostics` read it alone; `LanguageService`/`BuildService` lose their diagnostic surfaces | done | `7505df0` |
| A5 — ADR-0046 + `layering.md` rows + CI layering gate | done | `3c438a1` |

### B — `process-exec` and the `analyzers` contribution point

| Task | Status | Commit |
|---|---|---|
| B1 — extract `process-exec` from `vcs-core/src/cli.rs`; `vcs-core` migrated onto it in the same commit | done | `TBD` |
| B2 — `plugin-api`: `AnalyzerContribution` + validation + manifest round-trip tests; `api_version` unchanged | done | `80ece93` |
| B3 — `analysis-core`: `AnalyzerDef` built from a contribution, detection (program candidates, config files, `composer.json` `require-dev`), status reporting for "declared but not installed" | done | `37054a5` |
| B4 — `checkstyle-xml` parser and its severity mapping, fixture-driven | done | `de4c858` |
| B5 — the scheduler: `OnType` (debounced) / `OnSave` / `Manual`, one in-flight run per (analyzer, file), a new keystroke cancels the previous, project-wide runs serialized | done | `5017b50` |
| B6 — unsaved-buffer strategies: `stdin`, `temp-copy` beside the original, `saved-only`; `saved-only` degrades `OnType` to `OnSave` visibly | done | `309a02d` |
| B7 — `[analysis]` settings section, `ScopedField::Analysis`, `settings-model::analysis` rows | done | `2c3e6a5` |
| B8 — `AnalysisServiceRust`: runs on worker threads, publishes into the store | done | `1dbab9c` |
| B9 — view: the Analysis settings page, an "Inspect Project" action, per-analyzer status in the status bar | done | `a6afd56` |
| B10 — ADR-0047 + `layering.md` rows + gates | done | `92780dd` |

### C — the `php-tools` built-in plugin

| Task | Status | Commit |
|---|---|---|
| C1 — built-in manifest contributing `phpstan` and `phpcs` analyzers | done | `TBD` |
| C2 — Composer detection specifics: `vendor/bin/*`, `.phar`, the Windows `.bat` shims, an optional `php_binary` prefix, config discovery | done | `TBD` |
| C3 — `settings_model::plugins::contributes()` learns the two new points (it already omits `language-servers` — `settings-model/src/plugins.rs:183`) | done | `TBD` |

### D — the PHPUnit test tool window

| Task | Status | Commit |
|---|---|---|
| D1 — `plugin-api`: `TestFrameworkContribution` (program candidates, run argv, `--filter` spelling, output format, config files) | done | `36572fc` |
| D2 — `test-core`: `TestTree`, `TestId`, `TestStatus`, the TeamCity service-message parser, the JUnit-XML fallback | done | `431a15e` |
| D3 — failures published as diagnostics through `diagnostics-core` | done | `c67c2e6` |
| D4 — `TestServiceRust`, one QObject for N runs | done | `c67c2e6` |
| D5 — view: the Tests dock — toolbar, tree with status icons and durations, failure pane with clickable `file:line` via `run_core::links` | done | `fb1340b` |
| D6 — rerun: all, failed only, one node, from the tree's context menu | done | `fb1340b` |
| D7 — `phpunit` rows added to the `php-tools` manifest | done | `33bbf34` |
| D8 — ADR-0048 | done | `5477b50` |

### E — verification

| Task | Status | Commit |
|---|---|---|
| E1 — a stub analyzer binary (a script that prints fixture XML), on the `lsp-core::bin::stub_server` precedent, so CI needs no PHP | done | `86511eb` |
| E2 — E2E: `e2e_analyzer_findings_appear_inline_and_in_problems` | done | `2c45884` |
| E3 — the manual PHP matrix: a real Composer project walked by hand, recorded in the plan | open | |

## Critical files

**Read before starting A:**
`crates/lsp-core/src/diagnostics.rs` (the store being generalised), `crates/ui-shell/src/bridge/registry.rs:185-202` (the thread-local `Rc<RefCell<DiagnosticStore>>` that is today's de-facto singleton), `crates/ui-shell/cpp/editor_tabs_lsp.cpp:446-485` and `crates/ui-shell/cpp/problems_panel.cpp:187-263` (the two consumers), `crates/ui-shell/src/bridge/build/mod.rs:61-84` (the 3→4 severity map that disappears when both sides share one enum).

**Read before starting B:**
`crates/plugin-api/src/manifest.rs:35-155` and `:244-460` (the point enum, `Contributes`, and validation), `crates/plugin-host/src/builtins.rs:43` + `crates/plugin-host/src/lib.rs:69` (how a built-in is registered), `crates/plugin-host/builtin/csharp/plugin.toml` (the nearest existing manifest — a declarative external process), `crates/ui-shell/src/bridge/language/mod.rs:33` and `crates/lsp-core/src/catalog.rs:252-310` (how a contributed external tool is joined onto a native catalog — the pattern `analysis-core` should copy), `crates/vcs-core/src/cli.rs:59-120` (the code being extracted).

**Read before starting B7:**
`crates/app-config/src/project_settings.rs:77-197` and `:255-305`, `crates/settings-model/src/scope.rs:73-125`, and the Language Servers page end to end (`settings-model/src/servers.rs` → `ui-shell/src/bridge/settings.rs:799-930` → `ui-shell/cpp/language_servers_page.h` → `settings_dialog.cpp:57-99`) — it is the closest existing section and its category-list/stack ordering trap is documented in place.

## Reuse, not reinvention

- `run_core::links::resolve_link` for clickable `file:line` in the test failure pane — the terminal and the run console already share it (R2-6).
- `run_core::macros::expand` for `$PROJECT_DIR$`/`$FILE_PATH$` in analyzer argv, so analyzer configuration spells paths the same way run configurations do.
- `syntax_core::language_for_path` for "does this analyzer apply to this file" — ADR-0018 forbids a second extension table.
- `app_config::project_settings::update` for the `[analysis]` block, atomic write and `.ide/.gitignore` seeding included.
- `editor_core::diff`-adjacent nothing: no diffing here.
- `run_core::AnsiStripper` for the raw output panes, since PHPUnit colours its output.

## Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | PHPStan on a single file loses whole-project inference and reports differently from a project run. | Documented in the settings page. `Manual` project runs replace the whole source's rows; per-file runs replace only that file's, so a project run's richer findings survive until that file is edited. |
| 2 | `OnType` spawns a PHP process per pause. | Debounce, one in-flight run per (analyzer, file), cancellation on the next keystroke, and a concurrency cap. `OnType` is the default only for analyzers whose manifest says they can read a buffer. |
| 3 | Squiggles drift while typing, until the next run lands. | Accepted for v1 and stated: the drift window is one debounce interval. Shifting rows on edit (the `dap_core::BreakpointStore::shift_lines` precedent) is the upgrade path if it grates. |
| 4 | A `temp-copy` beside the original can be committed or picked up by the tool's own file walker. | The copy is a dotfile with a per-run suffix, removed on completion *and* on cancellation, and a gitignore covers the pattern. Landed (B6) against the project's **root** `.gitignore` rather than `.ide/.gitignore`: a nested gitignore only ever matches paths inside its own directory, and the temp copy sits beside a source file that can be anywhere in the tree, so `.ide/.gitignore` could never have covered it. The seed-if-absent *mechanism* is reused (`app_config::project_settings::ensure_root_gitignore_pattern`), just aimed at the file that can actually see every location, and it also appends the line to an already-existing root `.gitignore` (most real projects have one) rather than skipping seeding entirely the way `.ide/.gitignore`'s does. |
| 5 | The A-phase refactor touches every diagnostics consumer at once. | A is one phase, landed before any PHP code exists, with the existing LSP diagnostics tests as the regression gate. Nothing in B–D depends on the old shape. |
| 6 | The E2E budget is at its stated ceiling of 15. | E2 raises it to 16, recorded in ADR-0046. The justification is B1-9's: a diagnostic source whose rows never reach the editor is exactly the bug class this budget exists to catch, and it shipped once already. |
| 7 | Neither PHP nor Composer is in `linux-builder`. | CI never runs a real analyzer: unit tests parse fixture output files, and E2 drives a stub analyzer script. Real-tool verification is the manual matrix (E3), mirroring D4-6. |

## Verification

Per task, before every commit:

```sh
make lint
make test
cargo tree -p diagnostics-core -e normal | grep -i qt      # must be empty
cargo tree -p analysis-core    -e normal | grep -i qt      # must be empty
cargo tree -p analysis-core    -e normal | grep -i tokio   # must be empty
cargo tree -p test-core        -e normal | grep -i qt      # must be empty
cargo tree -p process-exec     -e normal | grep -i qt      # must be empty
```

End to end, in the headless harness under Xvfb (see the E2E harness notes in `crates/e2e`):

- **A** — break a Rust file, press Build, confirm the error is now *underlined in the editor* as well as listed in the Problems dock. This is the regression that proves A did its job.
- **B/C** — open a fixture project whose `vendor/bin/phpstan` is the stub analyzer; confirm findings appear as squiggles within the debounce interval, that the Problems dock shows them with source `phpstan`, and that double-clicking navigates.
- **D** — run the fixture test framework, confirm the tree fills while the run is in flight, that a failure is both a red node and an editor squiggle, and that Rerun Failed re-runs exactly the failed node.

Manual, against a real Composer project (E3), recorded in this document:

1. `composer create-project laravel/laravel` (or any project with `phpstan/phpstan`, `squizlabs/php_codesniffer` and `phpunit/phpunit` in `require-dev`), open it, and confirm all three are detected with no configuration.
2. Introduce a type error, an unused-variable sniff violation and a failing assertion; confirm each is underlined, each has a Problems row naming its source, and the failing test is red in the Tests dock.
3. Remove `vendor/`, confirm the settings page says "declared in composer.json, not installed" rather than reporting nothing.
4. Repeat once on Windows, where the `vendor/bin` shims are `.bat` files rather than shebang scripts.

## Docs to write

- **ADR-0046** — one diagnostics model: why `diagnostics-core` rather than growing `lsp-core`, what `(source, uri)` keying buys, why build diagnostics reach the editor now, and the E2E ceiling moving to 16. Amends ADR-0040 §4.
- **ADR-0047** — the `analyzers` contribution point and `analysis-core`: why the point is declarative and the format table native, why analyzers run over pipes while everything else uses a PTY, why `process-exec` was extracted rather than copied, and the trigger model.
- **ADR-0048** — the test runner: why TeamCity service messages rather than JUnit XML alone, why `test-core` is its own crate, and why test failures are diagnostics.
- `docs/README.md` index lines for this plan and all three ADRs.
- `docs/architecture/layering.md` rows for the four new crates and the changed rows for `lsp-core`, `build-core` and `vcs-core`.
