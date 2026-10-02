# 0066. Several language servers per language

## Status

Accepted.
Delivered by phase L of the [PHP parity plan](../php-parity-plan.md).
Amends the one-server-per-language rule of the language platform plan and the `ServerOverride` shape of [ADR-0016](0016-lsp-client.md).

## Context

`LspManager` kept one server per `language_id`, and every feature module asked it by language.
PHP wants two servers at once: Intelephense for most features, and Phpactor for navigation, completion and refactorings Intelephense lacks.
Running both means deciding who answers each request, how two answers combine, which server a completion item or command belongs to, and how two sets of diagnostics coexist.
`initialize` also sent no `initializationOptions`, so a server's licence key could not be set, and `update_settings` had no caller.
The forces are: existing single-server languages must behave exactly as before, `lsp-core` must stay language-agnostic, and the rules must be testable without a process.

## Decision

A language has an ordered list of servers, each running under a stable server `id`; requests are routed by a pure table and the answers merged by pure functions.

- **Identity and order.**
  `ServerDef`, `ServerConfig` and `[[language_server]]` gain `id`, `diagnostics`, `posix_only`, `exec`, `initialization_options` and `settings`.
  The `id` equals the language id for a language with one server, so existing entries and keys do not change.
  An entry without an `id` addresses the first server of its language.
  Answer order is the resolved list order (`priority`).
  A plugin entry still replaces every row of its language.
- **Routing** (`lsp_core::routing`).
  `First` goes to the first capable server whose answer is not empty: definition, hover, rename, formatting, implementation and every method not named.
  `Merge` goes to every capable server concurrently: completion, code actions, references and workspace symbols.
  `Origin` goes back to the producing server: `completionItem/resolve`, `codeAction/resolve`, `codeLens/resolve` and `workspace/executeCommand`.
  Capability comes from the stored `initialize` capabilities and from dynamic registrations; a method the table does not know is never filtered.
  With one server nothing is routed or filtered.
- **Merging** (`lsp_core::merge`).
  Completion items are deduplicated by label and kind, code actions by title and kind, references and workspace symbols by place; the first server wins.
  Items and actions carry an `x-ide-server` tag, removed before a payload is sent back to a server.
  A command goes to the server whose `executeCommandProvider.commands` lists it.
- **Notifications.**
  `didOpen`, `didChange`, `didSave`, `didClose` and watched-file changes go to every server.
  A server that starts late receives the open documents at their current version, and servers that already have them are not told twice.
- **Diagnostics.**
  The store source key is `lsp:<server-id>` (`lsp_core::diagnostics::source_key`), so ADR-0046's `(source, uri)` store merges two servers' rows and each clears only its own.
  A server with `diagnostics = false` publishes nothing; Phpactor defaults to that.
- **Settings.**
  `initialize` sends `initializationOptions`.
  `catalog::reload_plan` compares two resolved configurations and restarts a server whose command, arguments, initialization options, exec host, settings section or diagnostics flag changed, pushes `didChangeConfiguration` when only `settings` changed, and leaves the rest running.
- **Platform and host.**
  A `posix_only` server is skipped on native Windows with a local host and reported `Unavailable` with the reason (`catalog::launch_plan`, `is_windows` injected); it still runs in WSL and on an interpreter host.
  Each server may run on its own `ExecHost` (`start_on`).

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| One process per language, with Phpactor as a plugin of Intelephense's answers | Two language servers cannot share a process; the client has to speak to both. |
| Route in each feature module | Every module would repeat capability checks and merge rules, and a new method would have to be wired everywhere; routing in `request_with_timeout` covers every caller. |
| Pick one server per request by user setting | Loses the point: completion and code actions are better as a union, and the user would manage a per-feature table. |
| Tag origins in a side table keyed by item | Items cross the Qt seam as JSON and come back from the UI; an in-band tag survives the round trip with no shared state. |
| Typed `lsp_types::ServerCapabilities` | One field a server spells oddly would discard every other capability; raw JSON read by pointer is tolerant. |
| Retranslate request URIs per server host now | A host with a path map (ADR-0067) is not built yet, and Local and WSL never differ for one project; deferred to that ADR. |

## Consequences

- Positive: a second server is a catalog row; single-server languages are untouched; routing and merging are pure and unit-tested; a settings change restarts only what it must.
- Negative: a merged completion waits for the slowest capable server, up to the completion deadline; each extra server costs a process and memory per project.
- Negative: deduplication by label and kind can hide a distinct item with an identical label and kind from the later server.
- Negative: request params and results of a server on another host are not yet retranslated; only the process, `rootUri` and the document URIs of the `did*` notifications follow the server's host.
- Negative: `Server.diagnostics` and `priority` are fixed at launch, so changing them restarts the server.

## Related

- [PHP parity plan](../php-parity-plan.md), decisions D1 and D2.
- [ADR-0046](0046-one-diagnostics-model.md) — the `(source, uri)` store.
- [ADR-0052](0052-remote-wsl-execution.md) — `ExecHost` and URI translation.
