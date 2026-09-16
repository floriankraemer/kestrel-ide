# 0057. Gradle & Maven support: an init-script sync, a trust gate, and a read-only model

## Status

Accepted.
Implemented by [the jvm-build-tools plan](../jvm-build-tools-plan.md); this ADR covers the plan's P0 and A-phase (`jvm-build-core`, the `build-tools` contribution point, the `jvm-build-tools` built-in plugin, and the `[build_tools]` settings sections).
The plan's later B–E phases (tool window, run/reload/settings, Tests dock integration, build-file editing, and this record's own verification) landed as follow-up PRs on the same crate and contribution point; see the amendment below for what they delivered against what this ADR and the plan originally described.
Amends [ADR-0026](0026-plugin-host.md) (asset materialisation for a built-in plugin's text assets), [ADR-0039](0039-typed-run-configurations.md) (detection-only vs. invoking a build tool), [ADR-0040](0040-build-core.md) (delegate-and-never-model, extended from building to sync), and [ADR-0048](0048-test-runner.md) (a second `junit-xml` output-format consumer, the one the format was generalised for but never had until now).

## Context

The IDE detects Gradle and Maven projects (`run_core::toolchain`, ADR-0039) and can run their build/clean tasks (`build-core`, ADR-0040), but nothing knows the shape of the project itself: no module list, no source roots, no dependency graph, no task/goal tree.
That is what IntelliJ's Gradle/Maven tool windows are built on, and it does not exist here.

Three questions had to be answered before any JVM-specific code could be written.

1. **How is a Gradle project's model obtained without a bundled Java server?**
   The Gradle Tooling API is a Java library; embedding it means shipping a JVM helper this repo has no build for.
   A `gradlew --init-script <script> ideModel` run, printing one JSON document to a file, needs nothing but the project's own wrapper and a text asset — the same "a contribution is data" rule ADR-0026 already established for `language-servers` and `analyzers`.
2. **Does detection get to invoke the build tool now?**
   ADR-0039 says no — detection is marker-file presence only, because opening a project must not run its build script.
   That rule still holds for detection; *sync* is a different, user-initiated action (a click on a "Gradle project detected — Load" banner, or an auto-reload the user opted into), so this ADR narrows ADR-0039's rule to detection specifically and adds a trust gate for the action ADR-0039 never had to consider.
3. **Where does JUnit result data live when the tool speaks no streaming format?**
   ADR-0048 built `test_core::junit::parse` and `TestTree::apply_junit` as "the generalised primitive... for a future test framework whose manifest cannot name a streaming format," with no caller.
   Maven's Surefire/Failsafe reports are exactly that caller: XML written once, at the end, with no streaming hook. Gradle needs no equivalent, because the same init script installs a TeamCity `TestListener`.

## Decision

### 1. `build-tools` is a declarative contribution point; the model schema is native code

`plugin-api` gains `ContributionPoint::BuildTools` and `BuildToolContribution { id, name, toolchain, build_files, init_script }` — the same shape `AnalyzerContribution` and `TestFrameworkContribution` already have: a native process, no `[wasm]` component, `api_version` unchanged.
`toolchain` is a free-form string joined to `run_core::ToolchainId::from_id` at the seam in `jvm-build-core`, not in `plugin-api` — this crate stays a leaf and does not depend on `run-core`, exactly the split `AnalyzerContribution::output_format` already has with `analysis-core`'s parser table.
`TestFrameworkContribution` gains `requires_toolchain`, `filter_template` and `report_glob`, and `filter_flag` becomes `Option<String>`: exactly one of `filter_flag`/`filter_template` must be set, and a template must contain the literal `{pattern}` placeholder — validated in `plugin-api`, the same "decidable without a filesystem, so validate and unit-test it here" rule every other manifest field follows.

### 2. A built-in plugin's *materialised* assets, not only its embedded bytes (amends ADR-0026)

ADR-0026 gave a built-in plugin `LoadedPlugin::read_asset` for reading an embedded byte slice, which is enough for a component or an icon pack read once at startup.
An init script is different: `gradlew --init-script` needs a real file *path* on disk, not bytes in memory, and re-writing it into a temp file on every sync would leave stale copies and race a second concurrent sync.
`LoadedPlugin::asset_dir(&self, config_dir: &Path) -> io::Result<PathBuf>` is the amendment: for an installed plugin it is the plugin's own directory (already on disk); for a builtin it materialises `files` under `<config_dir>/plugin-assets/<id>/`, rewriting a file only when its content differs from what is already there (a byte compare, no new hashing dependency).
This runs on demand — the first sync that needs it — not on every `load()` scan, which runs on every settings-page open; `expand_asset_dir` replaces `${asset_dir}` tokens in a manifest's `args`, the same substitution shape `${plugin_dir}` already has for capability paths.

### 3. Sync is invoked, but only from a trust gate; detection is unchanged (amends ADR-0039)

Detection still never runs `gradle`/`mvn` — ADR-0039's rule is untouched for that question.
*Sync* runs `gradlew --init-script ... ideModel`, which executes the project's own build logic (arbitrary code the project author wrote, not the IDE's), so the first sync of a root requires an explicit user action, and a project is never trusted by anything the project itself can write: trust lives in the **global** settings file only (`[build_tools] trusted_roots`, `app-config`'s user-scoped `settings.toml`), never in a project's `.ide/settings.toml`.
A project vouching for its own trustworthiness in a file it controls is not a trust gate at all — it is the project author granting itself permission, which is the attack this gate exists to prevent.
Once a root is trusted, later opens sync automatically per the project's auto-reload mode (`external`/`any`/`none`, `jvm_build_core::sync::decide`), unit-tested against every combination of watcher-event vs. in-tab-open vs. save, with a save/watcher-event dedupe window so the IDE's own Ctrl+S does not trigger its own reload banner.

### 4. The model is read-only, same as `build-core`'s delegate-and-never-model rule (extends ADR-0040)

`jvm-build-core::model::BuildModel` is exactly what the init script or Maven's `dependency:tree`/effective-pom reported: no IDE-owned notion of an output path, an artifact, or a "correct" dependency version that could drift from what the tool itself would say.
ADR-0040 stated this rule for *building*; this ADR states it again for *sync*, because a project model is exactly the kind of thing a second opinion accretes around if the rule is not restated at the point a new read path is added.
Parsing stays split by trust boundary the same way `build-core` splits Cargo (structured JSON) from every other toolchain (pattern-matched text): Gradle's init script emits structured JSON `jvm-build-core` deserializes directly, and Maven's `dependency:tree -Dverbose` is parsed from text with a fixture per shape (`omitted for conflict with X`, `omitted for duplicate`, `version managed from`) — a tool that changes its output format is a fixture and a row, not a rewrite.

### 5. `junit-xml` gets its first real caller (extends ADR-0048)

`test-core::junit`'s parser and `TestTree::apply_junit` were built generalised and unused (ADR-0048 §consequences, "the very next test-frameworks contribution that cannot stream is a manifest change away from using it").
The `junit-maven` test-frameworks row (`output-format = "junit-xml"`, `report-glob = "**/target/{surefire,failsafe}-reports/TEST-*.xml"`) is that contribution: `test_core::runner::run` reads the glob's matching files once the process exits, skips report files older than the run's own start time (a stale report from a previous run left on disk), and delivers them through a new `TestSink::junit` the same way TeamCity events already deliver through `TestSink::event`.

### 6a. `app-core` gains `jvm-build-core`, for the project-tree decoration join only (phase B7)

`app_core::build_tools_tree::folder_role` joins a synced `BuildModel`'s source roots and output directories onto a project-tree path, the same shape `app_core::icons`/`app_core::preview` already use for the icon-theme and previews joins: `jvm-build-core` never learns an icon pack's folder-name keys, and `icon-theme` never learns what a Gradle source set or a Maven module is, so the two meet in `app-core`.
This is the one B-phase change to the A-phase's dependency row (`docs/architecture/layering.md`'s `jvm-build-core` row itself is unchanged — only `app-core`'s own row gains the edge) and stays within the no-Qt rule: `jvm-build-core` carries no cxx-qt/Qt dependency of its own, so `cargo tree -p app-core -e normal | grep -i qt` stays empty.

### 6. Verification: a separate `linux-jvm` image, nightly, outside the per-PR E2E budget

Neither a JDK, Gradle, nor Maven exists in `linux-builder`, and installing all three there would slow every PR's `make test`/`make lint` for work most PRs never touch.
A `linux-jvm` stage (`FROM linux-builder`, Temurin 21, Gradle, Maven, fixture projects pre-warmed at image build so `--offline` resolution works) is verified nightly and on demand — `make test-jvm` — mirroring `lsp-conformance`'s own "needs its own image, takes minutes, can go red because upstream changed rather than because we did" reasoning (`Makefile`, the `lsp-image`/`lsp-conformance` pair).

**E2E flow budget note.** The `e2e-ci` Makefile target currently lists 12 per-PR flows (`e2e`, `e2e_run`, `e2e_panes`, `e2e_preview`, `e2e_vcs`, `e2e_minimap`, `e2e_about`, `e2e_diff`, `e2e_analysis`, `e2e_edit`, `e2e_editor_popups`, `e2e_containers`), counted directly from that target rather than trusted from an earlier plan draft.
This PR (P0+A) adds no E2E flow at all — there is no UI yet to drive.
The plan's later E2 flow (`e2e_build_tools`, phase E) is gated `IDE_E2E_JVM=1` and runs only inside the nightly `linux-jvm` image alongside `test-jvm`, the same way `lsp-conformance`'s flows never appear in `e2e-ci`: it does not count against the per-PR budget above, and does not move it.

## Alternatives considered

| Option | Why rejected |
|---|---|
| A bundled Gradle Tooling API Java helper (VS Code's own design) | Needs a JVM build this repo does not have, plus a ~20 MB bundled artifact, for a model the project's own wrapper can already produce as JSON. |
| Static parsing of `build.gradle`/`build.gradle.kts` | Groovy and Kotlin DSL scripts are arbitrary code — `apply from:`, `subprojects {}`, a version catalog, a convention plugin — that only Gradle itself can evaluate correctly; a static parser would be permanently behind and silently wrong on anything but the simplest script. |
| Trust recorded in `.ide/settings.toml` | The project directory the trust gate exists to guard against is exactly the file that would be granting the permission — a project can commit its own "trusted" flag and vouch for itself. Global `settings.toml`, which only the user who clicked "Load" can write, is the only place a trust decision can live and still mean anything. |

## Consequences

- `plugin-api`'s manifest gains one contribution point and extends `TestFrameworkContribution`, both additive; `api_version` stays 1.
- `plugin-host` gains asset materialisation for any future built-in plugin that needs a real on-disk file rather than embedded bytes — not JVM-specific, though this is its first user.
- A sync is the one place in the codebase (besides an explicit Run) that executes project-authored code, and it is gated exactly like that fact deserves: an explicit first click, trust recorded where only the user can write it, and never inferred from anything the project itself ships.
- `jvm-build-core`'s dependency-graph and task-tree types have no IDE-owned interpretation layered on top of what the tool reported, so a "why does the IDE disagree with `gradle dependencies`" support question cannot arise by construction.
- CI's per-PR gate stays exactly as fast as it was: nothing in this PR touches `linux-builder`, `make test`, or `make lint`'s toolset, and the new `linux-jvm` image is built and run only nightly or on demand.

## Amendment: what was delivered vs. planned (phase E)

The B–D phases (tool window, run/reload/settings, Tests dock integration, build-file editing) and this E phase's own docs/verification pass landed after this ADR's original text above, which is left unedited per this repo's ADR-immutability rule.
The deviations below were verified against the shipped code, not transcribed from the plan's own wording, which had drifted from the code in a few places.

- `buildTools.reload` ships with no default keybinding: it is reachable only from the Build menu's "Reload Build Tool Project" action (`crates/ui-shell/cpp/build_menu.cpp`), because `Ctrl+Shift+O` — IntelliJ's own binding for this action — collides in this IDE with the pre-existing `view.goToSymbol` action and failed a keymap uniqueness test (recorded already in the plan's B4 row).
- Output-directory greying in the project tree was deferred, not shipped: `app_core::build_tools_tree::folder_role` (B7) joins a synced `BuildModel`'s source roots and output directories onto tree paths and drives the folder *icon*, but nothing paints an output directory in a dimmed foreground — `ProjectTreeModel` has no per-row style/foreground role to extend for it, and none was added in this delivery (recorded already in the plan's B7 row).
- The Build Tools settings page (B5) is project-scoped like `analysis`/`containers` (`ScopedField::BuildTools`, `settings_model::scope`), with one deliberate exception: `trusted_roots` is not a `ScopedField` at all and is read only from the global settings file, exactly as this ADR's §3 requires — a project has no field to carry its own trust flag in, structurally, not just by convention.
- The Groovy grammar (D1) shipped exactly as planned: `tree-sitter-groovy` is pinned from crates.io (`0.1.2`, `crates/syntax-core/Cargo.toml`), not vendored from git; its highlight query is hand-written off `build.gradle`'s node shape, the same way `kotlin`'s already was.
- `${property}` resolution (D6) is real but narrower than a naive reading of "version hints" would suggest, and the limitation differs by build tool.
  For Maven, `resolve_pom_property` resolves a `${x}` reference against the *same file's own* `<properties>` table only — never a parent POM's, which `maven::pom`'s own reader already documents as a limitation for the identical reason (it would need opening a second file).
  For Gradle, `is_resolvable_version` treats any version string containing `$`, `{` or `}` as unresolvable and silently skips it — a Gradle/Kotlin-DSL `${ext.foo}` or `$foo` interpolation never produces a version hint at all, not even a same-file-only one, a wider gap than Maven's that was not closed in this delivery.
- `filter-dialect` shipped as designed, not as a gap: `TestFrameworkContribution` carries `filter_flag`/`filter_template` exactly as this ADR's §1 describes, *and* the `filter-dialect` field the plan's D-phase notes mention (`gradle`/`surefire`, validated against an unknown-dialect load error) is present in `plugin-api`'s manifest and the shipped `jvm-build-tools/plugin.toml` rows — there is no shortfall here to record.
- The E2E flow (E2, `crates/app/tests/e2e_build_tools.rs`) being nightly-only, gated `IDE_E2E_JVM=1` and run inside `linux-jvm` alongside `test-jvm` rather than in the per-PR `e2e-ci` budget, is not a deviation: it is exactly what this ADR's §6 already decided, restated here because E2 is the phase that actually built the flow the decision described.

## Related

- [ADR-0026: plugin host](0026-plugin-host.md) — the contribution-point and built-in-plugin shape this ADR's `build-tools` point and asset-materialisation amendment extend.
- [ADR-0039: typed run configurations](0039-typed-run-configurations.md) — the toolchain table `jvm-build-core` joins into, and the detection-only rule this ADR narrows to detection specifically while adding a trust gate for sync.
- [ADR-0040: `build-core`](0040-build-core.md) — the delegate-and-never-model rule this ADR restates for a project's sync model.
- [ADR-0047: the `analyzers` contribution point](0047-analyzers-contribution-point.md) — the "native process, no wasm component, format id resolved by native code" shape `build-tools` and its test-framework extensions follow.
- [ADR-0048: the test runner](0048-test-runner.md) — the `junit-xml` format and `TestTree::apply_junit` this ADR gives their first real caller.
- `docs/architecture/jvm-build-tools-plan.md` — the plan this ADR's P0/A phase belongs to; phases B–E build the tool window, run/reload wiring, the Tests dock integration and build-file editing on the crate and contribution point this ADR establishes.
