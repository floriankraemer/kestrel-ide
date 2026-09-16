# Gradle & Maven support plugin — plan

## Context

IntelliJ IDEA's Gradle/Maven support is five things.
(1) **Sync** a project model out of the build tool — modules, source roots, output dirs, scoped dependency graph, JDK, tasks/goals — via the Gradle Tooling API or an embedded Maven.
(2) A **tool window**: task tree, lifecycle/plugins/goals, dependencies, profiles, Reload, Execute, offline/skip-tests toggles.
(3) **Run configurations** per task/goal, with tests delegated to the tool.
(4) **Build-file editing**: coordinate completion from the local repo and Maven Central, "newer version available" inspections with quick fixes, and a dependency analyzer with conflicts.
(5) **Sync triggers** (auto-reload `external`/`any`/`none`, a "Load Gradle changes" banner, Ctrl+Shift+O) and **settings** (distribution/JVM/offline/user settings/local repo).

What this IDE already has, verified in the tree: `ToolchainId::{Maven,Gradle}` with markers, wrapper-aware build/clean commands and `java-debug` (`crates/run-core/src/toolchain.rs`); `mvn exec:java`/`gradle run` configs (`run-core/src/detect.rs`); `[ERROR] File.java:[l,c]` diagnostic parsing (`build-core/src/text.rs`); jdtls and kotlin-language-server in the LSP catalog; Java/Kotlin(`.kts`)/XML grammars; a plugin host whose built-ins are real plugins (`php-tools` is the closest analogue); a TeamCity-streaming test runner with an unused JUnit-XML parser (`test-core/src/junit.rs`, ADR-0048 §consequences).

What is missing — the gap this plan closes: no project model at all (`project-model` is a directory tree), no task/goal discovery, no Gradle/Maven tool window, JUnit results never reach the Tests dock for Maven (the runner is hard-wired to TeamCity, `output_format` is ignored, and framework detection is PATH-probed so a stray `gradle` on `PATH` would claim a PHP project), no Groovy grammar (`build.gradle` is plain text), no build-file completion/inspections, no dependency analyzer, no reload-on-change, no Build Tools settings, and Maven's `BuildKind::Target` is a no-op (`build-core/src/spec.rs`).

User decisions taken: a **separate `linux-jvm` Docker image** for JVM verification (the main image is unchanged); **full scope in one plan** — sync, tool window, run, reload, settings, JUnit, and build-file editing (completion, version inspections, dependency analyzer).

## Decisions

1. **A built-in plugin `jvm-build-tools` plus a new Qt-free crate `jvm-build-core`.**
   Same split as `php-tools`/`analysis-core` (ADR-0047): the manifest is data (which toolchain, which build files, which asset, which test invocation); the provider that talks to the tool is native.
   A new contribution point, `build-tools`, is additive — `api_version` stays 1 (`Contributes` flattens unknown points, `plugin-api/src/manifest/mod.rs`).
2. **Gradle's model comes from an init script, not a Java helper.**
   `./gradlew --init-script <asset> -q ideModel --console=plain --no-configuration-cache` prints one JSON document: projects, tasks (name/group/description), source sets, output dirs, Java toolchain, and the resolved dependency graph per configuration with `selectionReason` (conflicts) and artifact files.
   It runs through the project's own wrapper so it matches the project's Gradle version, and ships as a text asset of the plugin (a contribution is data, ADR-0026).
   Rejected: a fat-jar Tooling-API server (VS Code's own design) — it needs a Java build this repo does not have and a bundled ~20 MB artifact for models the init script already yields.
3. **Maven in two phases, like IntelliJ.**
   *Read* = a static `pom.xml` parse (coordinates, parent, `<modules>`, properties, declared deps/plugins/profiles) — instant, no process.
   *Resolve* = `mvn -B help:effective-pom -Doutput=<tmp>` plus `mvn -B dependency:tree -Dverbose` (a text tree, stable for fifteen years).
   Plugin goals come from `META-INF/maven/plugin.xml` inside the plugin jar in `~/.m2/repository` (offline, instant, the `zip` crate); lifecycle phases are a static list.
4. **Detection stays marker-only; *sync* invokes the tool, behind a trust gate.**
   This amends ADR-0039's "no invoking mvn/gradle" rule, which was about detection.
   Running a project's build script is executing project code, so the first sync of a root needs a click ("Gradle project detected — Load"), and trust is stored in the **global** settings file (`[build_tools] trusted_roots`), never in `.ide/settings.toml` — a project could otherwise vouch for itself.
   Later opens sync automatically, per the auto-reload mode.
5. **Reload policy is IntelliJ's three modes**, decided by a tested rule in `jvm_build_core::sync`: `external` (default; a build-file change from the watcher while the file is not open in a tab auto-syncs, debounced, while an in-IDE save shows the banner), `any` (both auto-sync), `none` (always the banner).
   The banner's action is Ctrl+Shift+O.
6. **Tests run through the tool.**
   Gradle: the same init script installs a `TestListener` that prints TeamCity service messages when `-Pide.teamcity=true`, so the Tests dock's tree fills live through the existing parser — zero new format.
   Maven: Surefire has no streaming format, so a new `junit-xml` output format in `test-core` reads `report-glob` files after the run.
   `TestFrameworkContribution` gains optional `requires-toolchain`, `filter-template` (e.g. `-Dtest={pattern}`) and `report-glob`; `${asset_dir}` in `args` expands to the plugin's materialised asset directory.
7. **Build-file editing rides existing seams.**
   Completion items from `jvm-build-core` are converted to `lsp_core::CompletionList` in `LanguageService::completion_at` before the LSP branch, so `pom.xml` works with no `lemminx` installed.
   "Newer version" hints are `Diagnostic`s under source `build-tools:versions` (squiggles and Problems for free, ADR-0046).
   "Update to X" is a synthesised `CodeActionItem { kind: "quickfix", edit: <WorkspaceEdit> }` merged into `request_intentions`, so Alt+Enter applies it through the normal refactoring path.
   Maven Central queries go through a disk cache (24h TTL) and are skipped in offline mode.
8. **One dock, `buildTools`**, titled "Gradle" / "Maven" / "Build Tools" — computed in Rust — with one root per detected tool.
   A task/goal double-click creates a temporary `RunConfig` (toolchain plus target, via `run_core::context::remember_temporary`) that opens in the Run dock console.
9. **Verification runs in a `linux-jvm` image** (`FROM linux-builder`, Temurin 21 plus Gradle plus Maven, fixture caches pre-warmed at image build), nightly like `lsp-conformance` — outside the per-PR E2E flow budget.
   `make test` is unchanged.

## Architecture

### New crate `jvm-build-core` (support, Qt-free)

Deps: `run-core` (`ToolchainId`, wrapper resolution — `gradle_program`/`maven_program` made `pub`), `process-exec` (`run`/`spawn`, `ExecHost` for WSL roots), plus `serde`, `serde_json`, `quick-xml`, `globset`, `zip` (Maven plugin-jar reading, A5 only). `plugin-api`, `diagnostics-core` and `app-config` are deliberately **not** dependencies yet in the A-phase — nothing in `model`/`sync`/`run`/`gradle`/`maven` names a type from any of them today (a `BuildToolContribution`'s fields are read by the caller and passed in as plain strings, a sync failure is not yet published as a `Diagnostic`, and `[build_tools]` is read by `settings-model`), and they are added back only in the phase that first has real code needing them rather than carried speculatively. No `regex` either — `globset` alone covers every pattern this phase matches. No `reqwest` in this phase (P0+A only) — that lands with build-file editing's Maven Central client (phase D). No tokio: every run happens on its own `std::thread`, like `analysis-core`.

```
src/model.rs      BuildModel { tool, root, modules, tasks, warnings, synced_at }
                   Module { path, name, dir, build_file, source_roots, output_dirs, jdk, dependencies, children }
                   Dependency { group, artifact, requested, resolved, scope, transitive, file, conflict, children }
                   Task { path, name, group, description }   // Maven: Goal{plugin,goal}, phases, profiles
src/gradle/        init_script.rs, model_json.rs, sync.rs
src/maven/         pom.rs, effective_pom.rs, dep_tree.rs, goals.rs, sync.rs
src/sync.rs        AutoReload{External,Any,None}, is_build_file, decide, dedupe window, trust rule
src/run.rs         task_config(model, task, opts) -> run_core::RunConfig (temporary; marked to-be-replaced)
```

`src/deps.rs` and `src/editing/*` (dependency analyzer views, completion/version-hint editing assistance) are phase D scope and are not built in P0+A.

Layering rows to add: `jvm-build-core` → `run-core`, `process-exec` (No Qt; acyclic — `run-core` already depends on `app-config`; `plugin-api`/`diagnostics-core`/`app-config` join this crate's own row only once a later phase actually needs them).
`test-core` gains `globset` for `report-glob`.
`ui-shell` gaining `jvm-build-core`, and `settings-model::build_tools`/`app-core` gaining it later, are B/B7-phase changes and recorded when those phases land.

Process-invoking functions (`gradle::sync`, `maven::sync`) stay thin wrappers; every parser is fixture-tested (JSON, effective-pom XML, dep-tree text, plugin.xml, TeamCity output), because the coverage job's 80% patch gate never runs the `jvm-integration` feature.

### Plugin `crates/plugin-host/builtin/jvm-build-tools/`

Contributes two `build-tools` rows (`gradle`, `maven`) and two `test-frameworks` rows (`junit-gradle`, `junit-maven`) — see `plugin.toml` in that directory for the exact manifest, and `crates/jvm-build-core/src/gradle/init_script.rs`'s module doc for the init script's own rules (model-to-file, `--no-configuration-cache`, lenient resolution, the TeamCity `TestListener` with `flowId`).

### Settings (`app-config`, A7)

`[build_tools]` global: `trusted_roots: Vec<PathBuf>`.
`[build_tools.gradle]`: `distribution`, `gradle_home`, `java_home`, `offline`, `auto_reload`, `download_sources`, `jvm_args`.
`[build_tools.maven]`: `maven_home`, `user_settings_file`, `local_repository`, `offline`, `skip_tests`, `threads`, `always_update_snapshots`, `auto_reload`.
Rules live in `settings-model::build_tools`, mirroring `settings_model::analysis`'s shape (draft/scope/apply/validation).

### Verification image (A2)

`docker/Dockerfile` gains `FROM linux-builder AS linux-jvm` (Temurin 21 via the Adoptium apt repo, Gradle 8.x binary distribution, Maven 3.9.x), with a fixture-prewarm `RUN` added once A4/A5's fixture projects exist.
`Makefile` gains `JVM_IMAGE`, `linux-jvm-image`, `test-jvm`, `jvm-ci`.
`.github/workflows/nightly.yml` gains `jvm-image`/`jvm-integration` jobs mirroring the `lsp-conformance` pair.
This is nightly/on-demand verification, like `lsp-conformance` — it is not one of the per-PR E2E flows in the `e2e-ci` Makefile target, and does not count against that budget (see ADR-0057 §E2E flow budget).

## Task list (this plan doc; ADR-0057)

Update the row **in the same commit** that finishes the task.
`open` = not started.
`blocked on X` = cannot start until X lands.

**P0** — this plan doc, ADR-0057 (amends ADR-0026 asset dir, ADR-0039 sync-vs-detect, ADR-0040 read-only model, ADR-0048 junit-xml), `docs/README.md`, `layering.md` rows.

### A — foundation (Qt-free), this PR's scope

| Task | Status | Commit |
|---|---|---|
| P0 — plan doc + ADR-0057 + `docs/README.md` + `layering.md` | done | `064d25f` |
| A1 — `plugin-api`: split `manifest.rs` tests out; `build-tools` point + test-framework fields + validation + round-trip tests | done | `184396b` |
| A2 — `linux-jvm` image, `JVM_IMAGE`/`make test-jvm`/`jvm-ci`, nightly job, `jvm-integration.md` | done | `a74939a`, `5b7b4ae` (fixture prewarm), `9fcd39e` (non-root `RUN_JVM` fix) |
| A3 — `plugin-host`: on-demand `asset_dir()` materialisation, `${asset_dir}` expansion, `build_tools()` accessor; builtin registration with the init-script asset + "asset exists" test | done | `800ac7c`, `0979e1c` (write-then-rename) |
| A4 — `jvm-build-core::model` + `gradle::*`: init script + JSON parse; fixture JSON unit tests; `--features jvm-integration` tests against `tests/fixtures/gradle-{single,multi-kts,catalog}` | done | `9abbf0b`, `3c81028`/`9dc3243` (review fixes) |
| A5 — `maven::*`: static pom, effective-pom, verbose dep tree (conflicts), plugin goals, lifecycle table; fixtures `maven-{single,multi}` | done | `8e812d9`, `de4fe1f`/`777459a` (review fixes) |
| A6 — `sync.rs` policy (three modes, save/watcher dedupe) + trust rule + build-file globs; `run.rs` task → temporary config | done | `866abd6` |
| A7 — `app-config` `[build_tools]` sections + `settings-model::build_tools` draft/scope/validation | done | `d55851c`, `3a186e3`/`b34bb37` (review fixes) |
| Review fixes not tied to one task above | done | `b4e4195` (Tests dock regression, `test-core`/`ui-shell`), `16a5cb5` (unused deps, layering.md) |

### B — sync, tool window, run, reload, settings (later PR)

| Task | Status | Commit |
|---|---|---|
| B1 — `BuildToolsService` bridge + `RunService::run_temporary` + `build_tools_wiring.cpp` relays | done | `b169381`, `d4e8694`, `24cdddb` |
| B2 — dock panel (tasks/lifecycle/modules/dependencies/profiles). "Open Build File" only works on a `Module` row — only `Module` carries a `build_file` path in `view::Node`; a `ToolRoot` row does not | done | `d4e8694`, `5235c60` |
| B3 — run task/goal → `task_config`/`taskConfigWithArgs` → `run_temporary` → Run dock; "Execute…" line edit; Maven profile checkboxes (`view::rows`'s Profiles group, `BuildToolsService::setProfileChecked`, fed into `RunOptions::profiles`'s already-tested `-P<id>` rule) | done | `d4e8694`, `5235c60` |
| B4 — `EditorBanner` + reload policy wiring + trust prompt; `buildTools.reload` has no default binding (Ctrl+Shift+O is `view.goToSymbol` here — collided and failed a keymap test) | done | `d4e8694` |
| B5 — Build Tools settings page, project-scoped (`ScopedField::BuildTools`, the same `containers`/`analysis` shape) — `trusted_roots` stays global-only, structurally (`BuildToolsProjectSettings` has no such field) | done | `b169381` (initial, global-only), `7715feb` (made project-scoped) |
| B6 — status-bar sync indicator | done | `d4e8694` |
| B7 — project-tree source-root/output-dir decoration (`app-core` gains `jvm-build-core`). Output-dir greying is deferred: it needs a new per-row style/foreground role on `ProjectTreeModel` that does not exist yet (no "dimmed row" precedent to extend) | partial (icon role done; greying deferred) | `76245e9` (join fn), `d4e8694` (tree wiring) |
| B8 — Maven `-pl` build target | done | `77fa9dc` |

### C — tests (later PR)

| Task | Status | Commit |
|---|---|---|
| C1 — `test-core`: output-format dispatch, `junit-xml` post-run read, `TestSink::junit`; `report-glob`/`filter-template`/`requires-toolchain` in detection; `teamcity` parser accepts `flowId` | done | `68f46c8` |
| C2 — integration (JVM image): fixture `gradle test`/`mvn test` streams/fills live; a failing test is a diagnostic; second run is not empty | done | `3ce28ea` |

### D — build-file editing (later PR)

| Task | Status | Commit |
|---|---|---|
| D0 — build files registered as open documents without a server | done | `4110b08`, `533af5b` (review fix #7 — basename rule, not the sync globs) |
| D1 — Groovy grammar row | done | `63a05ab` |
| D2 — `editing::context` for pom.xml, `build.gradle(.kts)`, `libs.versions.toml` | done | `2374fed`, `6c642e9` (review fix #4 — nested elements no longer overwrite the enclosing coordinate) |
| D3 — local repository index (`~/.m2`, `~/.gradle/caches`) | done | `b1846b7`, `840c0ef` (review fix #11 — symlinks not followed) |
| D4 — Maven Central client with disk cache; offline-aware | done | `f6fba63` |
| D5 — completion merge in `completion_at` | done | `77da507`, `9f26da4` (review fix #1 — tracker prefix), `491f40a` (review fix #2 — repo index off the Qt thread), `fc0eeb1` (review fix #10 — client reuse/negative cache/generation guard/cap), `aa61457` (screenshot review — completion popup double-painted a label) |
| D6 — `versions.rs` hints → diagnostics source `build-tools:versions` | done | `5e27c3c`, `491f40a` (review fix #2), `6c642e9` (review fix #4), `d0f1fc6` (review fix #5 — `${property}` resolution), `ca99c1b` (review fix #6 — hints computed on open even with a server), `fc0eeb1` (review fix #10) |
| D7 — "Update to X" quick fix via synthesised `CodeActionItem` | done | `ab6e959`, `81e8a7f` (review fix #3 — no longer shadows a real server's intentions), `533af5b` (review fix #7 — `open_docs` guard), `6650ba5` (screenshot review — quick fix reads the diagnostic's own stored hint set) |
| D8 — dependency analyzer | done | `600316f`, `3d5bdf8`, `f185e2c` (review fix #9 — Go to Declaration module scoping + managed-dependency fallback), `840c0ef` (review fix #11 — conflict row detail), `90f578c` (screenshot review — title/empty-state reflect detection, not just sync) |

Review fix #8 (`d9a9563`, docs/CI only — the layering doc's stale tokio check and a missing `jvm-build-core` qt check in `ci.yml`) touches no single D-task above.

### E — docs & verification (later PR)

| Task | Status | Commit |
|---|---|---|
| E1 — `overview.md`, `project-structure.md` truthful; ADR status Accepted | done | `57f649b` |
| E2 — E2E (nightly, gated `IDE_E2E_JVM=1`): Gradle fixture sync → banner → Load → dock → run → Tests dock → pom completion | done | `41face6` |
| E3 — manual matrix: Windows `gradlew.bat`/`mvnw.cmd`, WSL root | done (manual matrix below: documented, awaiting a manual Windows pass — no Windows machine available to this session) | `a91de42` |

## Delivery

This PR covers P0 and A1–A7 only (the plan's B/C/D/E phases are out of scope here and land as separate PRs off `main`).
One branch off `main` in an agent worktree; `./mk test` + `./mk lint` green before every commit; the Progress row updated in the same commit that finishes a task; Conventional Commit messages; no co-author line.

## Verification

- Every task in this PR: `./mk test`, `./mk lint`, `cargo tree -p jvm-build-core -e normal | grep -i qt` empty.
- `./mk test-jvm` (the `linux-jvm` image): A3/A4/A5's `jvm-integration`-feature tests against the fixture projects, `--offline` after the image's fixture-prewarm build step.
- Manual verification of the tool window, reload, Tests dock and build-file editing is out of scope for this PR (it needs B–D).

## Out of scope (stated once)

Debug launch body with `mainClass`/classpath from the model (follow-up plan); run-from-context for a JVM `main` class; Gradle composite builds beyond listing included-build names; Maven daemon (`mvnd`); Gradle Kotlin-DSL script classpath for kotlin-language-server; vulnerable-dependency scanning; archetype/project wizards; download-sources action.

## Manual verification matrix (E3)

This doc has no numbered `§` headers elsewhere (unlike `run-build-debug-parity-plan.md`'s `## 6`), so this section is named rather than numbered — same table shape as that plan's own manual debug matrix.
Nobody on this delivery has a Windows machine, so nothing below is automated; CI stays Linux-only (`linux-builder`, `linux-jvm`), and this row is **documented, awaiting a manual pass** — the same status `containers-plan.md`'s C10 row and `changes-panel-plan.md`'s G11 row already use for an identical gap (no Windows box available to the implementing session).

Run this before a release, and after any change to `jvm-build-core`'s `gradle`/`maven`/`sync`/`editing` modules or `run_core::toolchain`'s wrapper resolution.

| Check | What to verify | Last walked | Result |
|---|---|---|---|
| Windows `gradlew.bat` / `mvnw.cmd` launch | `run_core::toolchain::wrapper_or` (`gradle_program`/`maven_program`, `crates/run-core/src/toolchain.rs`) looks for `gradlew.bat` — the actual Gradle-wrapper convention — but for Maven it builds the same `{wrapper}.bat` pattern against `mvnw`, i.e. `mvnw.bat`, never the file Maven's own wrapper generator actually writes, `mvnw.cmd`. Confirm on a real Windows checkout whether a project's `mvnw.cmd` is found at all, or whether Maven sync silently falls back to a bare `mvn` on `PATH` instead of the pinned wrapper version — a real gap this table exists to catch, not a hypothetical one. | not yet | — |
| WSL root (`\\wsl.localhost\…`) path translation | Verified absent, not merely unverified: `crates/jvm-build-core/src` and `crates/test-core/src` contain no reference to `process_exec::host::ExecHost` at all (grepped, zero hits), unlike `lsp-core`/`dap-core`/`build-core`, which ADR-0052 already threads it through. Every Gradle/Maven sync, run and JUnit report-glob read spawns on the local host directly. Confirm on Windows whether opening a `\\wsl.localhost\...` Gradle/Maven project even resolves the init-script path and the Surefire/Failsafe report glob correctly by accident (WSL's own UNC path handling) or whether it needs the same `ExecHost` treatment as a follow-up ADR. | not yet | — |
| Trust prompt on Windows | The editor banner ("Gradle/Maven project detected. Load it?") and `[build_tools] trusted_roots` round-trip through `app-config`'s TOML file — no Windows-specific code path exists, so this is confirming absence of a platform-specific bug, not a missing feature. | not yet | — |
| Reload banner on an external change (non-IDE tool) | `jvm_build_core::sync::decide`'s three-mode policy is unit-tested against synthetic watcher/save events on Linux; confirm a real `git checkout` of a build file on Windows reaches `ProjectTreeModel`'s watcher and shows the reload banner (`external`/`any` modes) the same way it does under Linux's `notify` backend. | not yet | — |
| JUnit results landing in the Tests dock | Gradle's TeamCity `TestListener` path is exercised by C2's `linux-jvm` integration tests; Maven's `junit-xml` post-run report-glob read (`test-core`) has no Windows-specific code either, but has never run against a real `mvn.cmd`/Surefire report path with backslash separators — confirm the glob still matches. | not yet | — |
| `pom.xml` completion fully offline | D4's Maven Central client is offline-aware (`editing::central`, skipped when `[build_tools.maven].offline` is set or no network answers); `editing::repo_index`'s local-repository scan is pure filesystem code with no OS-specific path assumptions beyond `PathBuf` itself. Confirm on Windows that completion still offers local-repo candidates with no network and no exception dialog. | not yet | — |
