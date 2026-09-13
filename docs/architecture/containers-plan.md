# Containers: Docker + Podman integration (JetBrains-parity plan)

## Context

JetBrains IDEA ships a Docker plugin with a wide, mature surface for both Docker and Podman.
Condensed:

| Area | JetBrains feature |
|---|---|
| Connections | Multiple named daemons; types Docker Desktop (Win/Mac), Unix socket, TCP (+cert dir), SSH, WSL, Minikube, Podman (machine); Colima/Rancher; connect/disconnect/test; "Connections from Docker contexts"; VM path mappings (Win/Mac) |
| Tool window | Services tree: connection → Containers / Images / Networks / Volumes / Compose projects; type-to-filter; filters (stopped containers, untagged images); Clean Up (prune) |
| Containers | start/stop/restart/remove; Terminal (user or root); Exec; Attach; Log tab; Inspect (JSON); Show Processes (`top`); Files tab (browse via `ls`, open read-only); Dashboard (name/id/image, env, ports, mounts — editable → recreate); Copy IDs; Open Project (remote dev) |
| Images | Images console: pull with completion (official images, then all); Build from Dockerfile (gutter → run config); Push to registry; Copy image to another daemon (`save`\|`load`); Create Container; Layers (+ Analyze image: per-layer fs changes, open/download file); Labels; Inspect; delete; hide untagged |
| Networks / Volumes | create, remove, prune, inspect, dashboard (connected containers, labels) |
| Compose | recognises compose files; gutter icons per service; run config; Services popup completion; inlay hints (service status → jump to logs; Open in browser / Connect to DB); tree nodes: Start All / Stop / Down / Scale / Jump to Source |
| Run configurations | Docker Image (~12 options), Dockerfile (~15), Docker Compose (~25); Server, Before-launch, "Show command preview"; usable as before-launch task of other configs |
| Run targets | run/debug an application inside a container (image / Dockerfile / compose service), wizard, JDK detection |
| Registries | Docker Hub, GitLab, Space, Docker V2, Generic; credentials; test connection; browse repos/tags and pull; push dialog |
| Settings | Docker (executables, connections, path mappings), Registry, Console (fold previous sessions), Advanced (SELinux `:z`) |
| Podman | treated as a Docker-compatible daemon: machine on Win/Mac, socket on Linux; Containerfile support |

Kestrel had none of it before this plan (only `docker/Dockerfile` for the build image).
Goal: the same surface for both Docker and Podman, native to this IDE's architecture, delivered as a phased plan with one pull request per task.

Decisions taken with the user before work started:

- CLI-driven transport (spawn `docker`/`podman`), not the Engine REST API and not `bollard` — recorded in [ADR-0055](decisions/0055-cli-driven-container-integration.md).
- Scope includes run configurations, editor assistance, registries and run targets.
- Registry credentials in the OS keychain (the `keyring` crate).
- Delivery: this plan doc with a Progress table, one pull request per task.

## Why CLI-driven

Recorded in full in [ADR-0055](decisions/0055-cli-driven-container-integration.md).
Short version:

- One mechanism for both engines: `podman` is a drop-in for `docker` on every subcommand this plan uses, so the engine is literally the executable.
- Every connection type collapses to argv prefix + environment: `-H unix://…`, `DOCKER_HOST=ssh://…` (the CLI runs `dial-stdio` itself), `--context`, `--connection <podman-machine>`, `wsl.exe -d <distro> -- docker …`, `minikube docker-env`.
- Credential helpers, buildx, compose v2, contexts, TLS: the CLI already does them; JetBrains' own troubleshooting page is mostly about re-implementing these.
- Structured data via `inspect`/`version --format json` (canonical JSON, same schema on Docker and Podman) rather than `ps`'s display strings.
- Streaming (`events`, `logs -f`, `build`, `compose up`, `exec -it`) is PTY/pipe children, the same shape `test-core`/`run-core` already use — no tokio.
- Cost: requires the CLI installed (JetBrains needs it too for remote hosts and buildx) and one process spawn per query (roughly 10-30 ms).

## Architecture

New Qt-free support crate `crates/container-core`:

```
container-core
├── connection.rs   ConnectionKind → Invocation { program, prefix_args, env, host: ExecHost }
│                   kinds: Auto | UnixSocket | Tcp{url, cert_dir} | NamedPipe | Context(name)
│                          | Ssh(url, identity) | Wsl(distro) | PodmanMachine(name) | Minikube
│                   Engine: Docker | Podman
├── discovery.rs    docker contexts, podman connections/machines, Colima/Rancher socket presets,
│                   default rootless podman socket path
├── probe.rs        `version --format json` → EngineInfo{engine, client_version, server_version,
│                   api_version} or a typed ConnectionError with a JetBrains-troubleshooting hint
├── model.rs        Container, Image, Volume, Network, ComposeProject/Service, Pod (podman) —
│                   serde over `inspect` JSON, lenient (#[serde(default)]), raw Value kept for Inspect tab
├── snapshot.rs     EngineSnapshot: ps -aq → inspect; image ls -q → inspect; volume/network; compose
│                   grouping by labels com.docker.compose.* / io.podman.compose.*; filters (stopped, untagged)
├── watcher.rs      per-connection thread: initial snapshot, `events --format json` child, 300 ms debounce
│                   → ContainerEvent over mpsc; manual refresh; reconnect with backoff
├── session.rs      pty sessions for log/attach/exec/terminal/build/compose (ShellSpec from Invocation)
├── ops.rs          start/stop/restart/rm/prune/create volume|network/pull/push/tag/login/save|load/
│                   history/top/cp/exec-ls (files parse)/inspect
├── recreate.rs     inspect JSON → RunSpec (image, name, env, ports, mounts, cmd, entrypoint, network,
│                   restart) for Dashboard edit → recreate; SELinux `:z` rule
├── run_config.rs   ImageRun / ContainerfileRun / ComposeRun option structs → argv (+ preview string)
├── compose_file.rs is_compose_file(path); services_with_lines(text) via tree-sitter-yaml (syntax-core);
│                   `compose config --services` for the ground-truth picker
├── target.rs       ContainerTarget (image | containerfile | compose service) → ExecHost::Container
├── registry.rs     RegistrySetting kinds Hub | GitLab | V2 | Generic; V2 API + Hub API (reqwest blocking);
│                   token dance; catalog/tags; credentials via keyring
├── image_ref.rs    parse/complete image references (FROM lines, `image:` keys); Hub search completion
└── bin/stub_engine.rs  fake `docker` for E2E (canned JSON, records argv) — precedent lsp-core/src/bin/stub_server.rs
```

`connection.rs`, `discovery.rs` and `probe.rs` landed in C1; `model.rs`, `snapshot.rs`, `watcher.rs` and `tree.rs` (the dock's flattened rows, kept in the Qt-free crate so their order and text are unit-tested) landed in C2; every other module lands with the task that needs it (see the task list below).

Layering (`docs/architecture/layering.md` carries the rows; greps for qt and tokio must stay empty):

- `container-core` → `process-exec`, `app-config` (persisted structs stay dumb there, ADR-0017/ADR-0039), and — from the task that needs each — `pty-core`, `syntax-core` (compose YAML walk), `reqwest` (blocking + rustls, already in the tree via `ai-chat-core`), `keyring` (new, C7).
- `run-core` → `container-core` from C5 (compiles container-kind run configs to a `LaunchSpec`, wraps launches for container targets).
- `ui-shell` → `container-core` directly (like `test-core`): a `ContainerService` QObject in `bridge/containers/` from C2, a worker thread owning the watchers, results queued via `qt_thread().queue(...)` exactly as `bridge/testing/mod.rs` does; C1 itself only needs `AppSettings`'s own accessors, added in `bridge/containers.rs`.
- `process-exec` unchanged: `ExecHost::for_path` is path-derived and every seam derives the host from a path, so a `Container` variant would be unreachable; a container run target instead rewrites the `LaunchSpec` in `container-core` before `Supervisor::launch` (see C8).

Persistence (`app-config`, new `[containers]` section, global with project override through `settings_model::scope`):
`connections: Vec<ContainerConnectionSetting>`, `registries: Vec<RegistrySetting>` (no secrets), `targets: Vec<ContainerTargetSetting>`, `show_stopped_containers`, `show_untagged_images`, `selinux_relabel`, `executable`/`compose_executable` overrides per connection.
`RunConfigSetting` gains `kind: Option<String>` (`"container-image" | "containerfile" | "compose"`), `run_on: Option<String>` (`"container:<target-id>"`), and one optional sub-table per kind — landing with C5.

## Tool window mockup ("Containers" dock, id `containers`, View menu + keymap `view.containers`)

```
┌ Containers ───────────────────────────────────────────────────────────────────────────────┐
│ [+ Add ▾] [⟳] [⏵ Connect] [⏹] [⤓ Pull Image…] [🧹 Clean Up ▾] [⚲ Filter ▾]   🔍 type to filter │
├────────────────────────────┬──────────────────────────────────────────────────────────────┤
│ ▾ 🐳 Docker (local)        │ Dashboard │ Log │ Terminal │ Inspect │ Files │ Processes      │
│   ▾ Containers (3)         ├──────────────────────────────────────────────────────────────┤
│     ● api        nginx:1.27│ Name      api                 ID  3f2a…  Image  nginx:1.27 → │
│     ○ db         postgres  │ Status    running (2 h)       Ports  0.0.0.0:8080 → 80/tcp    │
│     ● worker     app:dev   │ Env       POSTGRES_USER=app   [Add…] [Edit] [Remove]           │
│   ▸ Images (12)            │ Mounts    /home/f/proj → /workspace (rw)                       │
│   ▸ Networks (4)           │ Volumes   pgdata → /var/lib/postgresql/data                    │
│   ▾ Compose: shop (2 svc)  │                                    [Recreate with changes]     │
│     ● web  (1/1)           │                                                                │
│     ● db   (1/1)           │                                                                │
│ ▾ 🦭 Podman (machine)      │                                                                │
│   ▸ Pods (1)  …            │                                                                │
│ ▸ 📦 Registry: ghcr.io     │                                                                │
└────────────────────────────┴──────────────────────────────────────────────────────────────┘
```

Right pane tabs per node: Container → Dashboard/Log/Terminal(s)/Exec/Attach/Inspect/Files/Processes; Image → Dashboard/Layers/Labels/Inspect; Network/Volume → Dashboard/Inspect; Compose project → Dashboard(services)/Log; Registry repo → tags.
Log/Terminal/Exec/Attach are all `TerminalWidget`s (`cpp/terminal_widget.h:60`, parent-agnostic) over a PTY session running the CLI (`docker logs -f --timestamps --tail 1000`, `docker exec -it … sh` (or `-u root`), `docker attach`).
Seam change: `TerminalSupervisorRust::set_command(session_id, program, args)` stores a pending `ShellSpec` that `start` (`bridge/terminal.rs:425-445`) takes in preference to `shell_for(..)`; no widget change.

## Seam facts that shape the tasks (verified by reading the code)

- Before-launch tasks run sequentially on the run worker (`bridge/run/mod.rs:379-426` → `build_core::runner::run`, own Supervisor PTY, output to the Build dock via `run_menu.cpp:49`); variants `Build | RunConfiguration | ExternalTool{program,args}` (`run-core/src/before_launch.rs:15-23`).
  A Containerfile config is therefore `ExternalTool{docker build …}` + a `docker run …` launch — no new variant; build output lands in the Build dock like every other before-launch task in this IDE (a deliberate deviation from JetBrains' single Build Log tab).
- `process_exec::spawn` null-routes stdin (`lib.rs:265`): copy image is `save -o <tmp.tar>` then `load -i <tmp.tar>` with two `run` calls; no crate change.
- Completion/intentions/lenses all early-return for files without a language server (`bridge/language/mod.rs:459-461`, `completion_at` `:817`, `lsp_surface.rs:28,326`).
  Local items go in *before* the `open_docs` check: fill `self.completions` with `lsp_core::CompletionItem{label, insert, kind: None, raw: Null, range: None}` (`lsp-core/src/completion.rs:36-56`) and emit `completion_ready`; a local intention is a `CodeActionItem{kind: Some("container.pull"), raw: {"image": …}}` (`code_action.rs:37-50`) plus a `kind.starts_with("container.")` branch in `apply_intention` (`lsp_surface.rs:71`).
- Lenses: `CodeLensSpan{line,label,clickable}` + `setCodeLenses` + `codeLensClicked(int index)` (`code_editor.h:128-133, 316, 524`), fed by `EditorTabs::onCodeLensesReady` (`editor_tabs.cpp:739-751`), click routed at `editor_tabs_lsp.cpp:863-868`.
  Compose files have no LSP lenses, so `ContainerService::composeLensesReady(path)` collides with nothing; the click lambda gains an `ownsLenses(path)` branch.
- Gutter: `bool runnable_` painted at block 0 (`code_editor_gutter.cpp:205`, click `:257-273`).
  Becomes `QSet<int> runLines_` + `runRequested(int line)`; the one existing connect (`editor_tabs_lsp.cpp:907`) ignores args and still compiles.
- Virtual documents: `AppSession::open_virtual_document(scheme, key, text)` (`app-core/src/virtual_doc.rs:28`), read-only, dedup on `(scheme,key)`; title = last `/` segment of key and language detection runs on it, so `key = "container/<id>/inspect.json"` highlights as JSON.
  `ContainerService` needs the same `virtualDocumentOpened(tab_id, title, newly_opened)` signal `LanguageService` has (`ffi.rs:3933`, consumed `editor_tabs.cpp:228-234`).
- Detection: `dockerfile` entry (`syntax-core/src/catalog.rs:356-362`) already covers `Dockerfile`, `Containerfile`, `*.dockerfile`, `*.containerfile`, `Dockerfile.<stage>`; compose files are `yaml`.
  A compose flavor is not a language (ADR-0018): `container_core::is_compose_file(path)` (`^(docker-|container-|podman-)?compose(\..+)?\.ya?ml$`) is called from `ui-shell` where needed.
  The Material icon pack already maps `dockerfile` / `compose*.yml` → `docker`.
- Settings scope: `ScopedField::Containers` added to `settings-model/src/scope.rs` (C1); `app-config` gets `containers: ContainerSettings` on `Settings` and `Option<ContainerSettings>` on `ProjectSettings` (C1).
- `build.rs`'s `.cpp_file("cpp/tests_panel.cpp")` neighbourhood is where new panel/page files are registered.

## Tasks (one pull request each)

**C1 — Foundation: crate, connections, settings, ADR.**
`crates/container-core` skeleton; `connection.rs` (all kinds → `Invocation`; discovery of docker contexts / podman connections+machines / Colima+Rancher sockets; `version` probe → `EngineInfo{engine, client_version, server_version, api_version}`); `[containers]` section in `app-config` + `settings_model::scope` wiring; Settings page Containers (`cpp/containers_page.cpp`: connection list, kind combo, fields per kind, "Test connection" with JetBrains-troubleshooting-style hints as error text, "Add from contexts"); this plan doc; ADR-0055; layering rows; README index.
Tests: invocation argv/env per kind (table-driven, both engines), discovery parsers on captured/hand-authored JSON, version-probe parser for docker + podman output.

**C2 — Snapshot, watcher, dock tree.**
`model.rs`/`snapshot.rs` (fixture `testdata/inspect/{docker,podman}/*.json` captured from real engines), compose grouping, podman pods, filters, name search; `watcher.rs` events → debounce → snapshot; `bridge/containers/mod.rs` `ContainerService` (`FfiContainerNode{id,parentId,kind,name,status,detail,connectionId}` flat rows + `treeChanged` signal, `connect/disconnect/refresh`); `cpp/containers_panel.{h,cpp}` (dock via `DockRegistry::registerDock`, toolbar, tree, type-to-filter, context menus skeleton, splitter + detail `QTabWidget`); `containers_menu.cpp` View entry; `build.rs` registration; icons (`.a8` masks: docker, podman, container-running/stopped, image, network, volume, compose, pod, registry).
Tests: grouping, filters, search, event debounce, status derivation from `State`.

**C3 — Container actions and tabs.**
`ops.rs` start/stop/restart/remove/prune; Dashboard (read-only first: name/id/image link, status, env, ports, mounts, labels); `TerminalSupervisor::set_command` seam + `session.rs` + Log tab (`TerminalWidget` over `logs -f`); Terminal (user / root), Exec (command dialog + history), Attach; Inspect → `open_virtual_document("container", "<id>/inspect.json", …)` + `virtualDocumentOpened` signal on `ContainerService`; Processes (`top` → table); Files tab (`exec ls -la --time-style=+%s` parser, lazy expand, open read-only via `cp <c>:<path> -` tar → text, Download via `cp`); Copy Container/Image ID; error surfacing as `FfiResult` codes.
Tests: `ls -la` parser (GNU + busybox note), `top` parser, argv per op, root-vs-user terminal spec.

**C4 — Images, networks, volumes.**
Images console row ("Image to pull" + Pull; completion of local names now, Hub in C6); pull/push-tag/remove/prune, Layers (`history` JSON), Labels, Inspect, Dashboard (tags, size, created, containers using it); Create Container → run-config dialog prefilled (C5 dependency: lands as "create + run with defaults" until C5, then reuses the dialog); Copy image to another daemon (`save -o <tmp.tar>` then `load -i` on the target invocation, two `process_exec::run` calls, progress dialog); Networks/Volumes: create dialogs, remove, prune, inspect, dashboards; Clean Up menu (all / stopped containers / unused networks / unused volumes / dangling images / build cache); Filter menu persisted.
Tests: history parser, size/age formatting, prune argv matrix, save|load piping (against `stub_engine`).

**C5 — Run configurations (Image / Containerfile / Compose) and Compose nodes.**
`RunConfigSetting.kind` + `ContainerImageRunSetting`/`ContainerfileRunSetting`/`ComposeRunSetting` (every JetBrains option from the three tables, same names, in a new `app_config::container_run`); `container_core::run_config` argv compilers (every option → its documented flag, both engines) + `preview()` (shell-quoted) + `validate_{image,containerfile,compose}` + `compose_services` (`compose config --services`); `run-core::RunConfigExt::to_launch_spec_in` dispatches by kind through a new `container_run` module (`program`/`args` derived from the sub-table, never stored) — connection resolution threads `app_config::ContainerSettings` through a new `MacroContext::containers` field (the smallest change that reaches every `to_launch_spec_in` call site without a second method); Containerfile with `run_built_image` set gets an auto `BeforeLaunchTask::ExternalTool{docker build …}` via `before_launch::tasks_of_with_containers` (build output in the Build dock, the IDE's before-launch convention) — with it unset, the configuration's own launch *is* the build, usable as another configuration's before-launch `RunConfiguration` task; `stop_command`/`down_command`/`compose_project_{up_spec,stop_command,down_command,scale_command}` back the console's and the Compose tree's Stop/Down/Start All/Scale actions, all launched as tracked consoles (`RunServiceRust::spawn_ad_hoc_console`); `detect.rs` and `run_core::context::{containerfile_config, compose_config}` suggest/derive a `containerfile`/`compose` configuration from a project-root or gutter-clicked file.
`run_config_container_pages.{h,cpp}` gives each kind a real page: a Server combo (`ContainerService::connections()`), `QTableWidget`s (Add/Remove) for ports/mounts/env/build args/scale, a "Modify options ▾" disclosure menu, an ordered compose-files list, a Services picker fed by `RunConfigEditor::requestComposeServices`/`composeServicesReady` (worker thread), and a live "Command preview". `run_config_dialog.cpp` gains the Add ▸ Containers submenu and a `selectConfigId` parameter so a caller can open it pointed at a specific entry. The Compose tree (`containers_compose.cpp`) gets Start All/Stop/Down/Scale.../Jump to Source and a project-node Dashboard (services + running/total counts, read straight off the tree's own child rows). The Dockerfile/Containerfile gutter (`syntax_core`'s `dockerfile` language id) and the compose-file gutter (`run_core::is_compose_file_name`) both show a Build/Run/New-configuration popup. C4's `createContainerQuick` is replaced by "Create Container..." opening this dialog on a prefilled Image configuration (`ContainerService::imageRunDefaults`). ADR-0056 (extends ADR-0032/ADR-0039).
Tests: table-driven argv for every option × both engines, preview string quoting, the SELinux `:z` rule, the shell-word splitter, the three `validate_*` functions, Containerfile build vs. run-built-image dispatch, `tasks_of_with_containers`' auto build task, `detect`'s Dockerfile/compose suggestions and `is_compose_file_name`, `context::{containerfile_config, compose_config}`, `compose_project_*`'s argv, TOML round-trips for all three sub-tables, `FfiContainerOptions`'s structured round-trip through `container_form.rs`.

**C6 — Editor assistance.**
`compose_file.rs` `is_compose_file(path)` (file-name rule, called from `ui-shell` — not a new language) + `services_with_lines(text)` over tree-sitter-yaml; per-service gutter run markers (`runnable_` → `runLines_` + `runRequested(int line)`); compose code lenses "● running · Open localhost:8080" via `setCodeLenses` from `ContainerService::composeLensesReady(path)` (status from the snapshot, Open → `QDesktopServices`); image-reference completion (local images + Hub search API + configured registries) injected in `completion_at` before the `open_docs` gate for Dockerfile `FROM` / compose `image:`; "Pull image" intention (`kind = "container.pull"`) with the same pre-gate injection; Dockerfile/Containerfile/compose icons already come from the Material pack.
Tests: detection table, service/line extraction on tree-sitter-yaml, image-ref parsing, lens text derivation, completion ranking (official first).

**C7 — Registries.**
`registry.rs` (V2 token dance, catalog, tags; Hub API repos/tags/search; GitLab as V2 + project registry listing; Generic = push-only), `keyring` secret store (`service = "ide.containers"`, `user = <registry-id>`; graceful "no keychain available — use `docker login`" fallback); Settings page Registries (+ Test connection); tree Registry nodes (repositories → tags → Pull Image…); Push Image dialog (registry, repository, tag → `tag` + `login --password-stdin` + `push`); Add-service menu entry.
Tests: auth header/token parsing, tag listing pagination, push argv, keyring fallback path.

**C8 — Run targets.**
`ContainerTargetSetting` (image | containerfile | compose service; workdir mount `/workspace`; extra run options; env); New Target wizard (pull/use existing image, build Dockerfile, pick compose service); `RunConfigSetting.run_on`; `target.rs` rewrites the `LaunchSpec` before `Supervisor::launch` (`docker run --rm -i -v <root>:/workspace -w /workspace <ports> <env> <image> <program> <args>` / `compose run --rm --service-ports <svc> <program> <args>`; project-relative `cwd` re-based under `/workspace`); console link/diagnostic paths get a `/workspace` → project-root prefix map in `run-core/src/links.rs` (no `ExecHost` variant — it is path-derived and would be unreachable, see the seam facts above).
Debugging inside the target is deferred (adapter must run in-container; noted as a gap, remote-attach configs still work).
Tests: wrapping argv, cwd re-basing, link prefix map, compose-service target argv.

**C9 — Dashboard editing, Podman extras, polish.**
Dashboard Add/Edit/Remove env/port/mount → `recreate.rs` (`rm -f` + `run` from inspect; confirm dialog); SELinux `:z` advanced setting + rule; Podman: Pods node (ls/start/stop/rm/inspect), machine Start/Stop from the connection node, `podman-remote` resolution, "Containerfile" everywhere the UI says "Dockerfile"; Layers → "Analyze image" (`save` → per-layer tar listing, open/download file) — optional, last.
Tests: recreate spec from fixture inspect (docker + podman), `:z` rule (skips top-level dirs), pod parsing.

**C10 — E2E, docs, manual pass.**
`stub_engine` on PATH via fixture (`crates/app/tests/fixtures/containers/`), `e2e_containers.rs`: dock shows tree from canned data, Start records argv, run-config dialog creates a compose config and previews the command, Dockerfile gutter; `overview.md`/`project-structure.md` truthful; manual matrix recorded in this doc's Progress table: Linux docker + rootless podman, Windows Docker Desktop + podman machine, WSL distro connection.

## Parity gaps recorded deliberately (not planned)

Open Project in a container (needs a remote-dev backend Kestrel does not have), JetBrains Space registry, a "Connect to database" port action (no DB tool in this IDE), a VM path-mappings table (Docker Desktop/Podman machine already mount home directories themselves; a bind-mount error surfaces verbatim instead), Console "fold previous sessions" (the Log tab re-runs `logs --tail`; the engine keeps the history), compose Services-popup template completion, debug inside a run target.

## Critical existing code reused

- `crates/process-exec/src/lib.rs` `run` (buffered, takes stdin bytes)/`spawn` (streamed); `host.rs` `ExecHost::Wsl` for WSL-distro connections; `resolve_program`, `is_missing_program`.
- `crates/pty-core/src/lib.rs` `ShellSpec`, `PtySession` — every streaming tab.
- `crates/ui-shell/src/bridge/testing/mod.rs` — worker thread + `qt_thread().queue` + flat-row FFI pattern; `cpp/tests_panel.{h,cpp}` `buildTestsDock` — dock template; `cpp/dock_layout.h` `DockRegistry::registerDock`.
- `crates/ui-shell/src/bridge/terminal.rs` `TerminalSupervisor`, `cpp/terminal_widget.cpp` — PTY rendering for Log/Exec/Attach.
- `crates/run-core/src/{config,before_launch,context,supervisor}.rs`, `crates/app-config/src/launch_settings.rs` `RunConfigSetting`; `cpp/run_config_dialog.cpp`, `bridge/run/editor.rs` draft pattern.
- `crates/settings-model/src/scope.rs`; `bridge/settings.rs` `terminal_settings()` accessor shape; `cpp/settings_dialog.cpp` `categoryList->addItem` + `pages->addWidget(scopedPage(...))`.
- `crates/lsp-core/src/{completion,intentions}.rs`, `bridge/language/lsp_surface.rs` — injection point for local completion/intentions; `cpp/code_editor.h` lenses/inlay hints; `cpp/editor_tabs_run.cpp` gutter wiring.
- `crates/syntax-core` detection table (tree-sitter-containerfile and tree-sitter-yaml already bundled).
- `crates/lsp-core/src/bin/stub_server.rs` — stub-binary precedent; `crates/e2e/src/lib.rs` harness; `cpp/e2e_mark.h`.
- `crates/ui-shell/cpp/theme_icons.cpp` `maskIcon` + `resources/ui_icons.qrc` for new icons; `tr()` everywhere (ADR-0049); `build.rs` `.cpp_file(...)` registration.

## Verification

- Unit: `cargo test -p container-core` with captured docker+podman fixture JSON; `cargo test -p run-core` argv tables; `make test` + `make lint` before every commit.
- Layering: `cargo tree -p container-core -e normal | grep -iE 'qt|tokio'` empty; same for `run-core` (the edge C5 added).
- E2E: `make e2e` with `stub_engine` (Xvfb + xdotool); markers `containers_tree_changed`, `containers_action`, `run_config_preview` (from C10).
- Manual (recorded in the Progress table below, W8-2 style): real Docker on Linux, rootless Podman on Linux, Docker Desktop + `podman machine` on Windows via WSL interop, a WSL-distro connection, one private registry (ghcr.io) pull + push.

## Progress

| # | Task | Status | Commit |
|---|------|--------|--------|
| C1 | Foundation: crate, connections, settings, ADR | done | `2d50701` |
| C2 | Snapshot, watcher, dock tree | done | `b0ee05f`, `cdf81cd`, `08fcd1f` |
| C3 | Container actions and tabs | done | `a268df3` |
| C4 | Images, networks, volumes | done | `83f77fd` |
| C5 | Run configurations and Compose nodes | done | `1ba53a2` |
| C6 | Editor assistance | not started | |
| C7 | Registries | not started | |
| C8 | Run targets | not started | |
| C9 | Dashboard editing, Podman extras, polish | not started | |
| C10 | E2E, docs, manual pass | not started | |
