# 0069. Xdebug through vscode-php-debug

## Status

Accepted.
Delivered by phase D of the [PHP parity plan](../php-parity-plan.md).

## Context

Xdebug does not get launched by a debugger: the PHP process connects back to an IDE that is already listening (DBGp).
Every PHP debug case therefore has the same shape (a CLI script, a `php -S` request, a test, WSL, a container), and differs only in where PHP runs.
`dap-core` speaks DAP over a child process's stdio (ADR-0041) and starts its adapter through `ExecHost`.
vscode-php-debug is a Node stdio DAP adapter that listens for DBGp itself and reports each Xdebug connection as a DAP thread.
Writing a DBGp client in this IDE would duplicate that and add a second debugger front end.

## Decision

Drive vscode-php-debug through the existing `DapSession`, in its listen mode.

- **Adapter.**
  The `php-debug` row of the `dap-core` catalog runs `node` on `phpDebug.js`.
  The newest `xdebug.php-debug-*` extension under `~/.vscode`, `~/.vscode-server`, `~/.vscode-insiders`, `~/.vscode-oss`, `~/.cursor` or `~/.windsurf` is used.
  A `[[debug_adapter]]` with id `php-debug` overrides the command and arguments.
  A missing adapter shows an install hint, and when no `phpDebug.js` was found the start is refused with it (`dap_core::catalog::not_located`) rather than starting `node phpDebug.js`, which spawns fine and then dies with an opaque "adapter disconnected".
  `ToolchainId::Php.debug_adapter()` is `php-debug`.
- **Listen session.**
  The `launch` body has no `program`, a `port` (`[php].xdebug_port`, default 9003), a `hostname` and `pathMappings` as an object from server path to local path.
  The session stays up across runs, and ends only when the toggle is switched off or the session is stopped: the adapter does not report a termination when one connection ends, so a script that finishes leaves the session listening for the next one.
  The Run menu and toolbar toggle "Start Listening for PHP Debug Connections" starts and ends it.
  Connections are threads of that one session in the existing debugger UI.
- **One path for every debug launch.**
  Debug on a PHP run configuration starts the listen session (or reuses one that listens the same way), then runs the configuration through `RunService::runWithEnv` with the Xdebug environment.
  `dap_core::xdebug::env` builds it: `XDEBUG_MODE=debug`, `XDEBUG_SESSION` (or `XDEBUG_TRIGGER`) and `XDEBUG_CONFIG="client_host=… client_port=…"`.
  The program starts only after the adapter finished its handshake, because Xdebug does not retry.
- **One lifecycle type.**
  `dap_core::xdebug::ListenSession` owns what is running, what waits behind a shutting-down adapter and which runs wait for the handshake.
  `request` answers with a `ListenAction` (`Start`, `Launch`, `Replace` or `Nothing`), `started` and `handshake_done` advance it, and `stop` drops the queue.
  A run requested while the running listener is still handshaking is held until `handshake_done` hands it back, so a connection is never attempted before the adapter listens.
  `bridge/debug/php.rs` only performs the actions; it holds no lifecycle rule.
- **Where PHP runs** is decided by `dap_core::xdebug::plan`.

  | PHP runs | Adapter listens on | PHP dials | pathMappings |
  |----------|--------------------|-----------|--------------|
  | locally, or in WSL beside the adapter | `127.0.0.1` | `127.0.0.1` | none |
  | in a container (ADR-0067) | `0.0.0.0` | `host.docker.internal` | container mount to checkout |

  The environment is added to the configuration before the C8 container wrap, which turns it into `-e` arguments.
  The refusal to debug inside a container target is lifted for `php-debug` only.
- **Xdebug check.**
  The interpreter probe (ADR-0068) says whether Xdebug is loaded and what `xdebug.mode` holds.
  A missing extension fails the debug start with advice; a mode without `debug` is only a warning, because a run started by the IDE sets `XDEBUG_MODE` itself.
  Both show on the Settings > PHP page.
  When PHP runs in a container the advice adds the Linux Docker hint `extra_hosts: ["host.docker.internal:host-gateway"]`.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| A DBGp client in `dap-core` | A second protocol and a second debugger model for something the adapter already does. |
| A TCP DAP transport to an adapter in the container | The adapter is not in the container; Xdebug dials out of it, which needs no extra transport. |
| Run the adapter inside the container (as ADR-0067 does for tools) | Needs Node and the extension in every image; dialing out needs only Xdebug. |
| Have the adapter start PHP (`program` set) | Run configuration features (before-launch tasks, container wrap, console, `php -S`) would be duplicated in the adapter. |
| Refuse to start when `xdebug.mode` lacks `debug` | The IDE sets `XDEBUG_MODE=debug` for its own runs, so the ini value does not matter there. |

## Consequences

- Debugging a PHP configuration also leaves a listener up until the toggle is switched off or the session is stopped.
- Replacing the listener for a different listen address (a container after a local run) shuts the old adapter down on the Qt thread, which takes a moment.
- The adapter location is found on the machine the IDE runs on; for a WSL project the override is the way to point at the distro's copy.
- A container needs a route to the host: Docker Desktop and Podman resolve `host.docker.internal`, Linux Docker needs `host-gateway`.
- PHPUnit and Pest debugging from the gutter reuses this path (phase T).
- Checked against the real adapter and Xdebug 3 (the nightly `php_real` flows): a CLI run stops at its breakpoint with variables, and one listen session serves two `php -S` requests in turn with no second session.
