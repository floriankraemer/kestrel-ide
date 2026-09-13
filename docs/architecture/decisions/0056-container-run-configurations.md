# 0056. Container run configurations: `kind` + sub-tables, build as a before-launch task, stop/down as console actions

## Status

Accepted.
C5 (`RunConfigSetting.kind`, the three container-kind sub-tables, `container_core::run_config`'s argv compilers, `run-core`'s dispatch, and the run-config dialog's Containers submenu with a live command preview) is implemented.
The Compose tree's own Start All/Stop/Down/Scale/Jump-to-Source actions, the Dockerfile gutter's Build/Run popup, per-field table widgets in the dialog, and replacing C4's quick "Create Container" dialog are deferred — see this ADR's Consequences and `docs/architecture/containers-plan.md`'s C5 paragraph.

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

### 6. Scope actually shipped in this task vs. deferred

The dialog gained an Add ▸ Containers submenu and a live command preview (`RunConfigEditor::commandPreview`), but each kind's options are edited as one JSON blob (`FfiRunConfig::container_json`) rather than per-field port/mount/env/build-arg/compose-file table widgets or a "Modify options" disclosure menu — `container_json`'s own doc comment in `ffi.rs` explains why (no bare `Vec<T>` of a non-trivial shape crosses this seam, and a dozen more shared-struct fields for tables nobody asked to see all at once was judged not worth it yet).
`RunConfigEditor::composeServices` (the Services picker's data source) and `run_core::{stop_command, down_command}`/`RunService::composeDown` all exist and are tested, but nothing in the Compose tree (`ContainerService`, unchanged by this task) calls them yet — Start All/Stop/Down/Scale/Jump to Source tree actions, the Dockerfile gutter's Build/Run/New-configuration popup, and replacing C4's `createContainerQuick` dialog with this one are deferred to a follow-up task.
None of this narrows what `container-core`/`run-core` can do — every compiler, dispatch path and console action is implemented and tested; what is deferred is entirely `ui-shell`'s C++ view surface.

## Consequences

- `run-core` depends on `container-core` (new edge; `docs/architecture/layering.md` updated, `cargo tree -p run-core -e normal | grep -iE 'qt|tokio'` still empty).
- `MacroContext` grows a fourth field; every existing constructor (`for_project`, `for_file`) had to be touched to initialize it, but no existing call site's behaviour changed (`containers` defaults to `None`).
- A containerfile configuration's `run_built_image` flag changes what "launch" means (run vs. build-only) rather than adding a distinct configuration kind for "build only" — a JetBrains-matching choice, but a reader of `to_launch_spec_in` has to know this to understand why a containerfile launch sometimes never runs a container.
- The compose console's Down action has no tracked output — a real gap for a `compose down` that fails or hangs, the honest cost of not building it a console in this pass.
- The dialog's JSON editor is a genuine capability regression against JetBrains' per-field forms for anyone who has not read this ADR; it is also strictly more capable than no editor at all, and every field it edits is validated Rust-side (`app_config::container_run`'s structs, `serde`'s own type checking) before it ever reaches an argv compiler.

## Alternatives rejected

**A `ContainerRunConfig` type parallel to `RunConfig`.** Rejected: forks the toolbar picker, before-launch references, detection's merge rule and the dialog's list for a distinction (`kind`) a tagged field already carries.

**`to_launch_spec_with(&self, context, containers)` as a second method.** Rejected: every pre-C5 call site would have to pick the right method by kind; a `MacroContext` field reaches all of them through the one method that already existed.

**A fourth `BeforeLaunchTask` variant for "build a container image".** Rejected: `ExternalTool{program, args}` already expresses "run this program before launching", and a Containerfile's build is exactly that — a dedicated variant would only save recomputing the argv, which `container_core::run_config::containerfile_build_argv` already does cheaply.

**A tracked console (full `Supervisor::launch`) for compose Stop/Down.** Deferred, not rejected: real value (visible output, a Stop button of its own), but no compose-teardown failure has yet been reported as needing it, and it is additive over the fire-and-forget version.

## Related

- [ADR-0032: run configurations](0032-run-configurations.md) — `LaunchSpec`, the debugger-agnostic seam every kind here still produces.
- [ADR-0039: typed run configurations](0039-typed-run-configurations.md) — the `toolchain`/`target` string-plus-data pattern this ADR extends to `kind`.
- [ADR-0055: CLI-driven container integration](0055-cli-driven-container-integration.md) — `Invocation`, `Engine`, `ConnectionConfig`, and the "one argv, two engines" rule this task's compilers depend on.
- `docs/architecture/containers-plan.md`'s C5 entry — the task this ADR documents, including the deferred UI scope.
- `crates/app-config/src/container_run.rs`, `crates/container-core/src/run_config.rs`, `crates/run-core/src/container_run.rs`, `crates/ui-shell/src/bridge/run/{mod,editor}.rs`, `crates/ui-shell/cpp/run_config_dialog.cpp` — the code.
