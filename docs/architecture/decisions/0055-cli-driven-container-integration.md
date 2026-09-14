# 0055. CLI-driven container integration for Docker and Podman

## Status

Accepted.
C1 (foundation: `container-core`'s connection/discovery/probe, `[containers]` settings, the Settings > Containers page) is implemented.
The remaining tasks (C2-C10) are tracked in `docs/architecture/containers-plan.md`'s Progress table.

## Context

Kestrel has no container integration today — only `docker/Dockerfile` for the build image itself.
JetBrains IDEA ships a full Docker/Podman surface: connections, a Services tree for containers/images/networks/volumes/compose, run configurations, run targets, and registries.
The goal is the same surface for both Docker and Podman, native to this IDE's layering (Qt-free domain crates, a humble adapter and view).

Three transports were considered for talking to the engine:

- **The Docker Engine REST API directly** (`hyper`/`reqwest` against the daemon socket).
  Requires hand-rolling the API surface twice — once for Docker's API, once for Podman's libpod-compatible-but-not-identical API — and reimplementing everything the CLI already does for free: credential helpers, buildx, compose v2, SSH tunnelling (`ssh://` hosts), TLS cert handling, and context resolution.
- **`bollard`** (the most complete Rust Docker Engine API client).
  Solves the Docker half only; Podman's REST API diverges enough (different pagination, different libpod-only endpoints for machines/pods) that a second client, or extensive `bollard` patching, would still be needed.
  Neither transport gives compose v2, SSH `dial-stdio`, or `docker context`/`podman system connection` resolution — all CLI-only concepts.
- **CLI-driven**: spawn `docker`/`podman` and their subcommands, parsing structured (`--format json`) output.

## Decision

Container operations are CLI-driven.
`container-core` (Qt-free, no tokio) builds argv/env for the connection in play and parses the CLI's own JSON output; it never talks to a daemon socket directly.

**One mechanism, two engines.**
Podman is a drop-in replacement for Docker on every subcommand this crate uses, so "which engine" is a value (`container_core::connection::Engine`), not a second code path: the same `ConnectionKind` enum, the same `Invocation` shape, and the same probe/discovery parsers (each lenient to the other engine's JSON, `#[serde(default)]` throughout) serve both.
`connection::ConnectionConfig::invocation` is the one place a connection kind becomes an actual argv/env difference between the two — `-H`/`--context`/`DOCKER_HOST`/`DOCKER_CERT_PATH`/`DOCKER_TLS_VERIFY` for Docker, `--url`/`--connection`/`--identity` for Podman — everything downstream of it (ops, run configs, registries) is engine-agnostic.

**Every connection kind collapses to argv prefix + environment.**
A Unix socket, a TCP daemon, a named pipe, a Docker context or Podman connection/machine, an SSH host, a WSL distro, or a Minikube cluster are all, structurally, "prepend these arguments and set these variables before the caller's own argv" — captured once as `connection::Invocation { program, prefix_args, env, host }`.
A WSL connection reuses the wrapping *shape* `process_exec::host::ExecHost::Wsl` already established for remote execution (ADR-0052) — `wsl.exe -d <distro> -- ...` — built directly rather than through `ExecHost::command`, because that helper's `--cd <path>` has nothing to translate for a connection with no project-relative working directory.

**Structured data, not display strings.**
Every read goes through `inspect`/`version --format json` rather than `ps`/`images`'s human-formatted columns, because the JSON shape is the same schema on both engines and is meant to be parsed, where the table output is not and differs release to release.

**Registry credentials live in the OS keychain** (the `keyring` crate, landing with C7), keyed by a registry id — never in `settings.toml`.
`[[containers.registry]]` rows carry only address/username/kind (ADR-0017/ADR-0039: persistence stays dumb), the same separation the AI chat providers already draw between a provider's settings row and its API key.

**Cost accepted:** this requires the `docker`/`podman` CLI installed (JetBrains needs it too, for remote hosts and buildx), and one process spawn per query — roughly 10-30 ms, acceptable for a settings-page probe or an on-demand snapshot refresh, and the same cost every other CLI-driven crate in this codebase (`vcs-core`, `analysis-core`, `test-core`) already pays.

## Registries and secrets (C7)

The Docker Registry HTTP API V2 client, Docker Hub's own repository-listing API, and GitLab's project-registry API all live in `container-registry::registry`, alongside the C6 Hub search/tags client — the same crate for the same reason: every one of them is a blocking `reqwest` call, and `reqwest::blocking` carries a private tokio runtime that must never enter `container-core`'s tree.
`container_core::registry_ref` stays the one place a registry's *kind* has meaning (`RegistryKind`, reference formatting, login/push argv) — `container-registry::registry` re-exports it rather than defining a second copy, the same "one enum, shared through the lower layer" shape `Engine`/`ConnectionKind` already establish for connections.

**Credentials live in the OS keychain**, via the `keyring` crate's `linux-native` backend on Linux (kernel keyutils) — deliberately not `sync-secret-service` or `linux-native-sync-persistent`, both of which pull in `dbus-secret-service` and therefore need `libdbus-1-dev` + `pkg-config` just to *compile*.
The linux-builder image has neither, and even a build that added them would still be exercising a D-Bus Secret Service daemon that CI, a headless server, and plenty of minimal desktops do not run — the exact reasoning ADR-0021 already gives for rejecting `keyring` outright for `ai-chat-core`'s provider API keys.
`linux-native` needs no system package at all, so the build itself never depends on one being present.
On Windows and macOS the native OS credential stores (`windows-native`/`apple-native`) are used instead, no such gap there.

That still leaves a real gap: a container running with the kernel's keyutils syscalls themselves blocked (a locked-down sandbox, some minimal container base images) has no working backend at runtime even though the build succeeded.
`container_registry::secrets::SecretStore` classifies exactly that failure — `keyring::Error::NoStorageAccess`/`PlatformFailure` — into `SecretError::Unavailable`, carrying one fixed, actionable message: *"No OS keychain available — run `docker login <address>` and leave the password empty"*.
This is not a dead end: every registry action here (`pullFromRegistryCommand`/`pushImageCommand`) already skips its login line entirely when no secret is stored, and lets the engine CLI's own credential store (whatever `docker login`/`podman login` already wrote to `~/.docker/config.json` or the platform equivalent) supply the credentials instead.
Push and pull keep working either way, just without this IDE's own "store a password" convenience.
A `RegistrySetting` row itself never carries a secret field, matching `ContainerConnectionSetting`/the AI chat providers' own settings-vs-key split (ADR-0017/ADR-0039): `id`, `name`, `kind`, `address`, `username`, `gitlab_project`, `token_auth` are all TOML ever sees.

## Recreate (C9)

Neither engine has a "change this container's configuration in place" command — editing a running container's env/ports/mounts through the Dashboard is, structurally, `rm -f <id>` followed by a fresh `run`, the same tradeoff JetBrains' own Docker plugin makes.
`container_core::recreate::RunSpec::from_inspect` reads everything `recreate_argv` needs off a container's own `inspect` JSON: [`model::Container`] already carries image/name/env/cmd/entrypoint/mounts/ports/labels, and the rest (`HostConfig`, `Config.WorkingDir`/`User`/`Hostname`, `NetworkSettings.Networks`) is read straight off `Container::raw` — the field kept on every model exactly for a later task like this one, rather than growing `Container` itself for fields only `recreate` needs.
A compose-managed container (`com.docker.compose.*`/`io.podman.compose.*` labels present) refuses recreate: `is_compose_managed` gates `nodeActions.canRecreate`, and `RunSpec::from_inspect` also drops every compose/`ide.*` label from a recreated container's own labels, so recreate never produces a container carrying markers for a management path it no longer belongs to.
`recreate()` is `rm -f` then `run`; when the `run` half fails, [`ops::OpErrorCode::OldContainerRemoved`] reports both the run's own error and the fact that the old container is already gone, matching the confirm dialog's own warning rather than leaving that half of the story to the user's memory.

The SELinux `:z` relabel rule (`[containers].selinux_relabel`, landed in C1 as a setting with nowhere yet to apply it beyond `run_config::image_run_argv`) is now genuinely shared: `crate::selinux::relabel_suffix(host_path)` is the one function `run_config::mount_arg`, `target::wrap_launch` (which never actually called the rule before C9 — a real gap, not a refactor for its own sake) and `recreate::recreate_argv` all go through, so a bind mount's `:z` suffix cannot drift between a run configuration, a run target and a recreated container.

## Podman pods and machines (C9)

Pods (`podman pod ls`) already land in `snapshot::EngineSnapshot`/the dock tree as of C2; C9 adds the lifecycle argv (`pods::start_args`/`stop_args`/`restart_args`/`remove_args`/`inspect_args`) and the tree's own pod-status-to-actions matrix (`tree::pod_actions_for`) the pod node's context menu needed and did not have — narrower than a container's (no pause/unpause; neither engine documents one for `pod`).
Machines are new: `machine::Machine`/`machine::parse_list` over `podman machine list --format json`, and `start_args`/`stop_args` for the connection node's own "Start machine"/"Stop machine", offered only when that connection's kind is `PodmanMachine`.
`podman` is not installed on this repo's build host, so `testdata/machine/list.json` is hand-authored from the documented `podman-machine-list(1)` schema — flagged the same way the other hand-authored Podman fixtures already are (`testdata/inspect/README.md`).

`podman-remote` resolution: `ConnectionConfig::program` now tries `podman` on `PATH`, then `podman-remote`, before falling back to `podman` unchanged (an actually-missing CLI is still reported by the op that tries to run it, via `process_exec::Failure::NotFound` — this never guesses at that).
The search itself (`connection::program_on_path`) is a pure function of a `PATH` string, not `std::env::var` read directly, so it is testable against a fake `PATH` rather than mutating the process environment.

A pod node's own "Inspect" reuses the container Inspect tab's exact virtual-document path: `session::inspect_json` is now `session::inspect_json_with_args`, taking the argv to run (`inspect <id>` for a container, `pods::inspect_args` — `pod inspect <id>` — for a pod) rather than always the former, so one function still backs both rather than a second copy for the pod case.
A Podman machine's `running`/stopped state reaches the connection row the same way a snapshot does: `ContainerServiceRust::Connection` gained `machine_running: Option<bool>`, refreshed on a worker thread alongside `connect_engine`/`refresh` (never polled on its own timer — a machine's state changes no more often than one of those already-existing triggers), threaded through `tree::ConnectionRow`/`TreeNode::machine_running` as discrete data (`Some(bool)`, never a pre-worded string) so the view still owns the "running"/"stopped" word.

## Layers: Analyze image (C9, optional-but-landed)

`layer_fs` is a sequential, headers-only tar reader over `save -o` output: every entry's data is seeked past rather than read, so this works on an image of any real-world size without buffering its content.
It supports ustar name/prefix splitting, GNU `@LongLink` and PAX extended-header long names (both folded into the *next* real entry rather than returned themselves), and whiteout detection (`.wh.<name>` -> deleted, `.wh..wh..opq` -> the containing directory treated as deleted for this layer's own report — a per-layer view, not a merged one, so an "opaque directory" and "this directory was removed and recreated" read the same way here).
`manifest.json`'s `Layers` list maps each layer id onto its own byte region inside the outer tar (the classic `docker save`/`podman save` layout both engines still write); added/modified/deleted classification tracks a running set of paths seen so far, bottom layer to top, the same order `manifest.json` already gives them.
Every test builds its tar bytes by hand (`Cursor<Vec<u8>>` implements `Read + Seek`) — no fixture binaries needed for a from-scratch binary format reader.

`analyze` keeps its temp `save` tar on success rather than deleting it immediately (`AnalyzeResult::tar_path`) so a double-clicked entry or "Download..." can reopen it (`layer_fs::read_entry`, re-walking the manifest and the one layer's own tar region) without a second, full `save`.
`read_entry` takes an optional byte cap — `Some(8 MiB)` for the read-only preview (the same ceiling `files::read_single_tar_entry` already uses for a `cp` stream), `None` for Download, matching JetBrains' own no-cap download.
`ContainerServiceRust` keeps at most one analyzed image's tar path at a time (`analyzed_image: Option<(node_id, PathBuf)>`), replaced — and the old file removed — by the next `analyzeImage` call, and removed on `Drop` as a last resort if the session ends first.

## Session exit (C9 polish)

`TerminalSupervisor::sessionExited(session_id, exit_code)` closes the gap between "the PTY reader thread saw EOF" and "the view can act on the child's own exit code": the reader thread's loop already breaks on `Ok(0)`/an `Err`, and now queues one more closure onto the Qt thread that calls `PtySession::try_wait` — queued rather than called directly on the reader thread because `TerminalEntry::pty_session` is an `Rc<RefCell<..>>`, not `Send`, and the Qt thread is where every other access to it already happens.
The Containers dock's Pull/Push consoles (`ContainerDetailArea::addTerminalTab`, an `autoCloseOnExitZero` marker set only for those two) close themselves on `exitCode == 0`; Log/Terminal/Exec/Attach tabs carry no marker and are untouched by the signal, so an interactive shell or a container's own log stream never disappears out from under the user.

## Consequences

- New Qt-free crate `container-core`: `connection` (`Engine`, `ConnectionKind`, `Invocation`), `discovery` (contexts/connections/machines/socket presets), `probe` (`version --format json` → `EngineInfo`/`ConnectionError`) in C1; `model`/`snapshot`/`watcher`/`session`/`ops`/`recreate`/`run_config`/`compose_file`/`target`/`registry`/`image_ref` land in C2-C9 per the plan.
  No tokio, matching `analysis-core`/`test-core`: every operation is a blocking `process_exec` call or a `pty_core` session, reported back to `ui-shell` through a `Send` closure forwarded onto `CxxQtThread::queue()`.
- `app-config` gains a `[containers]` section (`ContainerSettings`: connections, registries, targets, the two dock filters, the SELinux relabel toggle) as a project-scoped field, following the same sparse-`Option` override rule as `[terminal]`.
  `ContainerConnectionSetting`'s kind-specific fields are plain strings, never an enum — persistence stays dumb (ADR-0017/ADR-0039); `container_core::connection` is the only place that gives them meaning.
- `settings-model::scope` gains `ScopedField::Containers`.
- `ui-shell` depends on `container-core` directly, the same way it already depends on `test-core`/`analysis-core` for their respective services; a new Settings > Containers page (connection list, per-kind fields, Test connection, Add from contexts) lands in C1, and a `run-core` → `container-core` edge is planned for C5 (container-kind run configurations).
- **Parity gaps, deliberately not planned:** Open Project in a container (needs a remote-dev backend Kestrel does not have), JetBrains Space registry, a "Connect to database" port action (no DB tool in this IDE), a VM path-mappings table (Docker Desktop and Podman machine already mount the user's home directory themselves; a bind-mount error surfaces verbatim instead of a mapping UI), the Docker CLI's SSH connections carry no identity-file override (the CLI itself has none; Podman's `--identity` flag does, and is honoured), and debugging inside a run target (C8) is deferred — the adapter would have to run in-container, and remote-attach configurations still work without it.
