# 0067. Container exec host: `ExecHost::Container`

## Status

Accepted.
Delivered by phase X of the [PHP parity plan](../php-parity-plan.md).
Amends [ADR-0052](0052-remote-wsl-execution.md) and supersedes the "wrap the `LaunchSpec`, not a new `ExecHost` variant" paragraph of [ADR-0056](0056-container-run-configurations.md) §7.

## Context

ADR-0056 rejected an `ExecHost::Container` variant because `ExecHost::for_path` classifies a *path*, and a container is not a property of a path.
That reasoning holds for `for_path`, and it is why run targets wrap a `LaunchSpec` instead.
It does not cover tools the IDE runs itself: PHPStan, PHPCS, PHPUnit, the interpreter probe and Phpactor must run in the container that holds the project's PHP, while git, cargo and npm must stay on the host.
For those callers the choice comes from configuration (`[php]` `container_target`), not from where the project lives.
`process_exec::run_on` and `spawn_on` already take an explicit host (phase I3), and `Server.host` plus `LspManager::start_on` do the same for language servers (phase L8).
Two more facts forced the shape:
- `ExecHost::is_remote()` meant two things at once: the tool runs elsewhere, and the files are reached over a remote filesystem.
  A container satisfies the first and not the second, because the project is bind-mounted and stays on the local disk.
- A container's paths differ from the local ones, so every path a tool is given or prints must be translated, and so must every URI a language server sees.

## Decision

`ExecHost` gains a third value, `Container(ContainerHost)`, which is never produced by `ExecHost::for_path`.

- **Host value.**
  `ContainerHost` holds the engine program and prefix arguments, the engine environment, the verb arguments (`exec -T`, `run --rm -i -v ...`), the container reference and a `PathMap`.
  `argv` builds `<engine> <prefix> <verb> -w <cwd> [-e K=V] <target> <program> <args>`.
  Project paths in the program and arguments (and the value of a `--flag=<path>`) are rebased onto the mount; any other argument passes through.
  `to_remote`, `to_local` and `path_from_tool` use the `PathMap`, so a path a tool prints under the mount opens the local file and a path outside it is left as printed.
- **`PathMap` moves to `process-exec`.**
  It was in `container-core`, which depends on `process-exec`; `container_core::target` re-exports it, so `wrap_launch` and the host share one mapping.
- **`is_remote()` is split.**
  `runs_remotely()` (WSL or container) decides spawn mechanics, exit-code mapping and path translation.
  `filesystem_is_remote()` (WSL only) decides local file I/O, the native watcher and `cwd.is_dir()`.
  Every former `is_remote()` site was walked: sites that classify a path with `for_path` use `filesystem_is_remote()`; sites that take any host use `runs_remotely()`.
- **Building the host.**
  `container_core::target::exec_host(target, invocation, root, mode, selinux_relabel)` builds it from a `ContainerTargetSetting` and the resolved connection.
  A compose service defaults to `exec` (`compose exec -T <svc>`) and may be `run --rm -T`; an image or Containerfile target is always `run --rm -i` with the project bind-mounted at the target's workdir.
  Docker and Podman differ only in the engine program; a connection reached through `wsl.exe` marks the host `via_wsl`, so its environment goes through `WSLENV` and the mount source is the distro's path.
  `exec_host_for` resolves a target id against `ContainerSettings`; `php_core::host::interpreter_host` joins that to the `[php]` settings and falls back to the project's own host when no target is set or it no longer exists.
- **Program lookup.**
  `resolve_program` on a container host runs `sh -c 'command -v "$1"'` (a name) or `test -x` (a path) inside it.
  Only a hit is memoised, because a miss may mean the container is not up yet.
  Exit 127 and the engine's own messages (`No such container`, `no container with name or ID`, `service "x" is not running`, `no such service`, `executable file not found`, an unreachable daemon) map to `Failure::NotFound`, and `container_unavailable_reason` gives the sentence to show.
  `spawn_on` returns `NotFound` up front when the lookup fails, since a spawned child's exit is never inspected.
- **Who opts in.**
  Analyzers, test frameworks and (later) formatters whose contribution says `requires-interpreter = "php"`, and language servers with `exec = "interpreter"`, run on the interpreter's host through `run_on`/`spawn_on`.
  `analysis_core::Scheduler`, `test_core::run_on`, `find_program_on`, `status_on`, `locate_file_on` and `diagnostics_by_file_on` take the host.
  There is no formatter path yet; the formatter phase takes `php_core::host::interpreter_host` the same way.
- **Language servers.**
  For a server whose host differs from the project's, `lsp-core` translates every URI at the wire, in one place: `Server::send` rewrites outgoing messages and the reader rewrites incoming ones.
  The fields rewritten are `uri`, `targetUri`, `rootUri`, `oldUri`, `newUri`, `scopeUri` and the keys of a `changes` map.
  Requests, results, diagnostics and workspace edits therefore reach callers in project paths.
  `initialize` sends `processId: null` to a container server, which would otherwise exit when it cannot find the IDE's pid.
  `launch_plan` takes the interpreter host as well as the project host, so a `posix_only` server whose interpreter is native Windows is still skipped.
- **Run configurations.**
  A PHP configuration (PHP toolchain, `php-builtin-server`, `php-console`) with no `run_on` inherits `container:<[php] container_target>` and goes through the existing C8 wrap.
  An explicit `run_on`, including `local`, wins.
  In a container `php -S` listens on `0.0.0.0` and its port is published.
  `wrap_launch` now rebases project paths in the program and arguments, and a compose target publishes the bindings it is given (`-p`).
  Debugging inside a container stays refused until the Xdebug phase.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| Keep wrapping the `LaunchSpec` only (ADR-0056 §7) | It rewrites a run configuration before launch. Analyzers, test runs and language servers call `run_on`/`spawn_on` and have no `LaunchSpec`, so each would need its own wrapper. |
| Have `for_path` return `Container` for a configured project | The host would silently apply to git, cargo and npm, which must stay on the host. A container is chosen per tool, not per path. |
| One `is_remote()` that is true for a container | A container would be given UNC-path handling, no watcher and no local file I/O, all wrong for a bind mount. |
| Translate URIs in each feature module | About twenty request modules and every server-initiated message would each need the rule, and a missed one opens the wrong file. The wire is the one point they all share. |
| Cache a failed lookup like WSL does | A container started after the first failed lookup would keep reporting the tool missing. |
| Translate only paths under `PathMap` and fail on others | A container-only path (an image's vendor stubs) is still useful to show; it is returned as printed and cannot be opened locally. |

## Consequences

- Positive: PHPStan, PHPCS, PHPUnit, Phpactor, Run File and `php -S` run in the container with local diagnostics, test locations and navigation, with one host value and one path mapping.
- Positive: `process-exec` stays a leaf; the engine specifics live in `container-core`.
- Positive: git, cargo and npm are unaffected, and a project without a container target behaves as before.
- Negative: every `ExecHost` match now has a third arm, which is the point of using an enum.
- Negative: a file only the container has (for example a server's built-in stubs) opens at its container path and cannot be loaded locally.
- Negative: argument rebasing is a prefix match on the project root; an argument that is a project path by coincidence is rewritten.
- Negative: a compose target with a `compose_executable` override (`docker-compose`) is run through the engine's `compose` subcommand; the override is ignored.
- Negative: environment variables of a per-call `env` appear as `-e K=V` in the engine's argv.

## Related

- [ADR-0052: remote WSL execution](0052-remote-wsl-execution.md), amended.
- [ADR-0056: container run configurations](0056-container-run-configurations.md), §7 superseded.
- [ADR-0066: several language servers per language](0066-several-language-servers-per-language.md), the `exec = "interpreter"` field.
- [ADR-0068: the php-core crate](0068-php-core-crate.md).
