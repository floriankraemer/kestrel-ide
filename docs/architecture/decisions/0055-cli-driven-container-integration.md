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

## Consequences

- New Qt-free crate `container-core`: `connection` (`Engine`, `ConnectionKind`, `Invocation`), `discovery` (contexts/connections/machines/socket presets), `probe` (`version --format json` → `EngineInfo`/`ConnectionError`) in C1; `model`/`snapshot`/`watcher`/`session`/`ops`/`recreate`/`run_config`/`compose_file`/`target`/`registry`/`image_ref` land in C2-C9 per the plan.
  No tokio, matching `analysis-core`/`test-core`: every operation is a blocking `process_exec` call or a `pty_core` session, reported back to `ui-shell` through a `Send` closure forwarded onto `CxxQtThread::queue()`.
- `app-config` gains a `[containers]` section (`ContainerSettings`: connections, registries, targets, the two dock filters, the SELinux relabel toggle) as a project-scoped field, following the same sparse-`Option` override rule as `[terminal]`.
  `ContainerConnectionSetting`'s kind-specific fields are plain strings, never an enum — persistence stays dumb (ADR-0017/ADR-0039); `container_core::connection` is the only place that gives them meaning.
- `settings-model::scope` gains `ScopedField::Containers`.
- `ui-shell` depends on `container-core` directly, the same way it already depends on `test-core`/`analysis-core` for their respective services; a new Settings > Containers page (connection list, per-kind fields, Test connection, Add from contexts) lands in C1, and a `run-core` → `container-core` edge is planned for C5 (container-kind run configurations).
- **Parity gaps, deliberately not planned:** Open Project in a container (needs a remote-dev backend Kestrel does not have), JetBrains Space registry, a "Connect to database" port action (no DB tool in this IDE), a VM path-mappings table (Docker Desktop and Podman machine already mount the user's home directory themselves; a bind-mount error surfaces verbatim instead of a mapping UI), the Docker CLI's SSH connections carry no identity-file override (the CLI itself has none; Podman's `--identity` flag does, and is honoured), and debugging inside a run target (C8) is deferred — the adapter would have to run in-container, and remote-attach configurations still work without it.
