# 0056. Container run configurations: `kind` + sub-tables, build as a before-launch task, stop/down as console actions

## Status

Accepted and fully implemented: `RunConfigSetting.kind` + the three container-kind sub-tables, `container_core::run_config`'s argv compilers and validators, `run-core`'s dispatch (`to_launch_spec_in`, the containerfile auto-build task, compose stop/down/scale), the run-config dialog's structured per-kind pages (Server combo, tables, disclosure menu, Services picker, live command preview), the Compose tree's Start All/Stop/Down/Scale/Jump-to-Source actions and project dashboard, the Dockerfile/compose gutter popups, and "Create Container..." replacing C4's `createContainerQuick`.

## Context

ADR-0032/ADR-0039 gave this IDE one run-configuration shape: a program, arguments, a working directory, an environment, and (ADR-0039) a `toolchain`/`target` pair that says which build tool produced it.
A container-kind configuration does not fit that shape at all — there is no single `program`/`args` a Docker Image, a Dockerfile build-then-run, and a `docker compose up` all reduce to; each has its own JetBrains-documented option table (Image ~12 options, Dockerfile ~15, Compose ~25), and Compose additionally needs Stop and Down as actions on an already-running console, not just a launch.

Three questions had to be answered without breaking ADR-0039's central rule: `app_config::RunConfigSetting` depends on nothing, so persistence stays dumb and `run-core` maps meaning back onto it.

## Decision

### 1. `kind: Option<String>` plus one optional sub-table per kind, not a second run-configuration type

`RunConfigSetting` gains `kind` (`None`/`"process"` = today's behaviour, or `"container-image"`/`"containerfile"`/`"compose"`) and three `Option<...>` fields — `container_image`, `containerfile`, `compose` — each a plain struct in a new `app_config::container_run`, field-for-field the JetBrains option tables, `#[serde(default, skip_serializing_if = ...)]` throughout so an existing `.ide/settings.toml` with no container configuration round-trips byte-for-byte.
This is the same shape ADR-0039 chose for `toolchain`/`target`: a string plus data `run-core` interprets, rather than a `RunConfigKind` enum on a struct `app-config` owns, which would invert the "persistence depends on nothing" row.
A second top-level configuration type (`ContainerRunConfig` alongside `RunConfig`) was rejected too: it would fork the toolbar's picker, the before-launch `RunConfiguration` reference, detection's merge rule, and the dialog's list — all of which already work in terms of one list of one type.

`Containerfile`'s sub-table repeats every field `ContainerImageRunSetting` has (rather than nesting one inside the other) because a settings file is meant to be read in a diff: a flat table beats `containerfile.run.port_bindings` for the same reason `ContainerConnectionSetting` is flat instead of a tagged union (see its own doc comment).

### 2. `container_core::run_config` compiles options to argv; `run-core` never builds a `docker`/`podman` command line itself

Every option maps to exactly the flag its JetBrains table names — `-P`, `-p host:container/proto`, `--entrypoint`, `-v h:c[:ro][,z]`, `-e`, `--pull`, `-a`/`-d`, `--build-arg`, `-f` per compose file, `--profile`, `--env-file`, `--compatibility`, `--rmi all|local`, `--exit-code-from` + `--abort-on-container-exit`, `--scale svc=n`, `--always-recreate-deps`, `-V`, `--no-log-prefix`, `--no-start`/`--no-deps`, `--attach-dependencies`, `--force-recreate`/`--no-recreate`, `--build`/`--no-build` — table-tested against both engines, since ADR-0055 already established that `docker` and `podman` (and their `compose` subcommands) share this surface byte for byte.
`run_options`/`build_options` are free-form extra flags, split with a small POSIX-ish word splitter (quotes and backslash escapes, no globbing or `$VAR` — this is a text field, not a shell) rather than pulling in a crate for what a for-display, never-executed splitter needs.
`preview(argv) -> String` shell-quotes the result for the dialog's live "Command preview" — also never executed, so it only has to look right.

This lives in `container-core`, not `run-core`, for the same reason `ops.rs`/`images.rs` do: it is engine-agnostic argv compilation over `app-config` structs, and `run-core` depending on it (a new edge, `docs/architecture/layering.md` updated) is a support crate reusing a support crate, the same shape `run-core` already has with `pty-core`/`terminal-core`.

### 3. Connection resolution threads through `MacroContext`, not a second `to_launch_spec_with` method

A container-kind configuration's `connection_id` has to resolve to an `Invocation` (program, prefix args, environment), which needs `app_config::ContainerSettings` — something `RunConfigExt::to_launch_spec_in(&self, context: &MacroContext)` never needed before.
Two shapes were weighed: a second method, `to_launch_spec_with(&self, context, containers)`, or a new `containers: Option<ContainerSettings>` field on `MacroContext` itself, set via a `with_containers` builder.
The field was chosen: `to_launch_spec_in` already has every call site that matters (`RunService::run`/`run_context`, every existing test), and a second method would mean every one of those call sites picking the right one by kind, or always calling the container-aware one and threading settings through call sites that today have never heard of `ContainerSettings`.
Extending the context costs one `Option` field, defaults to `None` (read as "no connection resolved, fall back to a bare local `docker`" — the same "unknown reads as the least surprising default" rule `ToolchainId::from_id` follows), and every pre-C5 call site is unaffected because it never sets it.

The one caller this does not reach: a `BeforeLaunchTask::RunConfiguration` task resolved through the plain `to_launch_spec(root)` convenience (no `MacroContext` at all) still gets `containers: None` if the referenced configuration is itself container-kind.
Recorded as a gap rather than fixed by threading settings through `before_launch::tasks_of`/`resolve_task` too — no configuration in this codebase's own test fixtures nests a container-kind configuration inside another's before-launch list, and doing so today is rare enough that the honest gap is cheaper than the churn.

### 4. A Containerfile's build is a before-launch `ExternalTool`, not a new `BeforeLaunchTask` variant

`before_launch::tasks_of_with_containers(config, containers)` — a sibling of `tasks_of`, not a replacement for it — prepends `docker build …` as a `BeforeLaunchTask::ExternalTool` whenever `ContainerfileRunSetting::run_built_image` is set, computed fresh from the connection and Dockerfile path on every call rather than persisted, so editing either never leaves a stale build command behind.
Build output lands in the Build dock exactly like every other before-launch task (`run_menu.cpp`'s existing convention) — a deliberate divergence from JetBrains' single Build Log tab, already the plan's stated approach for this task.

`tasks_of` itself (used by cycle validation and everything pre-C5) is untouched: it does not know about `[containers]`, so a config with no container settings handy still validates correctly.
Only the actual launch path (`RunService::launch`) calls the `_with_containers` variant.

When `run_built_image` is unset, launching the configuration *is* the build — `containerfile_launch_spec` compiles to `docker build …` instead of `docker run …`, and no before-launch task is added.
This is what makes such a configuration usable as another configuration's before-launch `RunConfiguration` reference with nothing started afterward, matching JetBrains' own "Run built image" checkbox semantics rather than inventing a fourth `BeforeLaunchTask` variant for "build only".

### 5. Compose Stop/Down are console actions, not before-launch tasks or a new console kind

`compose up -d` is a fire-and-forget launch (the console just shows the CLI's own status output and exits once the containers are up); killing that process, IntelliJ observed, leaves the containers running.
`container_core::run_config::compose_stop_argv`/`compose_down_argv` compile the JetBrains-parity teardown commands; `run_core::{stop_command, down_command}` resolve them against a configuration's connection.
`RunService::composeDown` runs `down_command` fire-and-forget (`std::process::Command::spawn`, no output captured) rather than a full `Supervisor::launch` — a deliberate v1 simplification (see Consequences) since giving it a tracked console is more machinery than a teardown command's own (usually silent) output justifies today.

### 6. The dialog edits structured fields, not JSON; every planned surface is wired

Each kind's options cross the seam as a flat `FfiContainerOptions` struct — port bindings, bind mounts, env, build args and scale as `Vec<FfiPortBinding>`/`Vec<FfiBindMount>`/`Vec<FfiKeyValue>`/`Vec<FfiScaleEntry>` (nested shared structs, the same shape `FfiRgb` already nests in the palette structs — cxx generates `Vec<T>` bindings per shared struct, not per field type, so this sidesteps the "no bare `Vec<QString>`" rule cleanly), compose files/services/profiles/env files as `\n`-separated strings (plain lists, the existing `before_launch` convention). `run_config_container_pages.{h,cpp}` builds one page per kind: a Server combo from `ContainerService::connections()`, `QTableWidget`s with Add/Remove for every list field, a "Modify options ▾" menu with one checkable action per optional group (a group starts open when loading a row that already has content in it), an ordered compose-files list (Add.../Remove/Up/Down), and a Services picker fed by `RunConfigEditor::requestComposeServices`/`composeServicesReady` (a worker-thread call, matching `ContainerService::testConnection`'s own shape — not synchronous, unlike this ADR's first draft). Validation (`container_core::run_config::validate_{image,containerfile,compose}`) stays in Rust and surfaces through the dialog's existing `FfiResult` error path.

The Compose tree gained real actions: `ContainersPanel::showComposeProjectContextMenu`/`showComposeServiceContextMenu` (Start All/Stop/Down/Scale.../Jump to Source), reading a project's connection id, name and compose files straight off the tree item (`FfiContainerNode::tooltip`/`resourceId`/`connectionId`, already set by `onTreeChanged` — no new query needed) and handing them to new `RunService` methods (`runComposeProject`, `stopComposeProject`, `downComposeProject`, `scaleComposeService`) that compile through `run_core::container_run::compose_project_*` and launch as tracked consoles. The project node's Dashboard lists its services and their running/total counts by reading its own child rows — display, not a new query.

The Dockerfile/Containerfile gutter (`syntax_core::language_for_path(..).id() == "dockerfile"`, ADR-0018's one detection table, not a second file-name rule) and the compose-file gutter (`run_core::is_compose_file_name`) both show a popup — Build Image/Run Container/New Configuration... for a Dockerfile, Run/New Configuration... for a compose file — backed by new `RunService` methods (`buildContainerfile`, `runContainerfile`, `runComposeFile`, `new{Containerfile,ComposeFile}Configuration`) built on `run_core::context::{containerfile_config, compose_config}`, siblings of the existing `config_for_file`/`remember_temporary` shape. "New Configuration..." opens the run-config dialog already pointed at the new entry (`showRunConfigDialog` gained a `selectConfigId` parameter).

C4's `createContainerQuick` (a name + "publish all ports" checkbox, `docker run -d` with nothing else configurable) is gone. "Create Container..." now calls `ContainerService::imageRunDefaults` (the image node's own connection + reference) and `RunConfigEditor::addContainerConfiguration`, then opens the real dialog on the new entry — every Image option is available immediately, not just the two `createContainerQuick` exposed.

Compose Down (both the run console's button and the tree's) and Start All/Stop/Scale all launch through `RunServiceRust::spawn_ad_hoc_console` — the `launch()` tail (worker-thread `Supervisor::launch` + reader thread + `consoleStarted`) factored out and reused with no before-launch tasks — so every one of them is a real, visible console in the run dock, not a fire-and-forget `std::process::Command::spawn`. This ADR's own §5 originally reserved a `TerminalWidget`-session fallback for the case where `Supervisor` "cannot host a second command for the same config"; it turned out that it always can (a console's tracking key is an arbitrary string label, not a uniqueness constraint), so the fallback was never needed and is not built.

## Consequences

- `run-core` depends on `container-core` (new edge; `docs/architecture/layering.md` updated, `cargo tree -p run-core -e normal | grep -iE 'qt|tokio'` still empty).
- `MacroContext` grows a fourth field; every existing constructor (`for_project`, `for_file`) had to be touched to initialize it, but no existing call site's behaviour changed (`containers` defaults to `None`).
- A containerfile configuration's `run_built_image` flag changes what "launch" means (run vs. build-only) rather than adding a distinct configuration kind for "build only" — a JetBrains-matching choice, but a reader of `to_launch_spec_in` has to know this to understand why a containerfile launch sometimes never runs a container.
- `FfiContainerOptions` is one large flattened struct (every kind's fields at once, ~40 of them) rather than three smaller ones — the same tradeoff `FfiContainerConnection` already made for connection kinds, for the same reason (no clean tagged-union shape crosses this seam).
- `ContainersPanel` and `EditorTabs` each gained a `RunService`/`RunConfigEditor` pointer set after construction (`setRunContext`/`setContainerRunContext`), because those two QObjects are constructed later in `main_window.cpp` than the panels that need them — the same retrofit shape `setPreviewProvider`/`setDiagnosticsService` already use there, not a new pattern.

## Alternatives rejected

**A `ContainerRunConfig` type parallel to `RunConfig`.** Rejected: forks the toolbar picker, before-launch references, detection's merge rule and the dialog's list for a distinction (`kind`) a tagged field already carries.

**`to_launch_spec_with(&self, context, containers)` as a second method.** Rejected: every pre-C5 call site would have to pick the right method by kind; a `MacroContext` field reaches all of them through the one method that already existed.

**A fourth `BeforeLaunchTask` variant for "build a container image".** Rejected: `ExternalTool{program, args}` already expresses "run this program before launching", and a Containerfile's build is exactly that — a dedicated variant would only save recomputing the argv, which `container_core::run_config::containerfile_build_argv` already does cheaply.

**A `TerminalWidget` session in the Containers dock as compose Down's console, via the C3 `setCommand` seam.** Considered as the fallback for "the run `Supervisor` cannot host a second command for the same configuration" — rejected once it became clear that concern does not apply: a console's tracking key (`ConsoleId`, keyed off an arbitrary string label) is not a uniqueness constraint on `config_id`, so `spawn_ad_hoc_console` already gives Down (and Start All/Stop/Scale) a real `Supervisor`-tracked console with no new mechanism.

## Related

- [ADR-0032: run configurations](0032-run-configurations.md) — `LaunchSpec`, the debugger-agnostic seam every kind here still produces.
- [ADR-0039: typed run configurations](0039-typed-run-configurations.md) — the `toolchain`/`target` string-plus-data pattern this ADR extends to `kind`.
- [ADR-0055: CLI-driven container integration](0055-cli-driven-container-integration.md) — `Invocation`, `Engine`, `ConnectionConfig`, and the "one argv, two engines" rule this task's compilers depend on.
- `docs/architecture/containers-plan.md`'s C5 entry — the task this ADR documents, including the deferred UI scope.
- `crates/app-config/src/container_run.rs`, `crates/container-core/src/run_config.rs`, `crates/run-core/src/container_run.rs`, `crates/ui-shell/src/bridge/run/{mod,editor}.rs`, `crates/ui-shell/cpp/run_config_dialog.cpp` — the code.
