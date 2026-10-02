# 0068. The php-core crate

## Status

Accepted.
Delivered by phases I and PS of the [PHP parity plan](../php-parity-plan.md).

## Context

PHP support needs rules that belong to no existing crate: what `composer.json` and `composer.lock` say, which language level the servers assume, what an interpreter reports about itself, what the Composer tool window lists and runs, and which JSON each language server wants for the `[php]` settings.
`jvm-build-core` is the precedent for a language's support crate (ADR-0057).
ADR-0039 keeps one toolchain table in `run-core`, so run-time pieces cannot move out of it, and `run-core` sits beneath this crate in the dependency order.
The Intelephense licence key is a secret and `settings.toml` may be committed.

## Decision

One support crate, `php-core`, Qt-free and tokio-free, depended on only by `ui-shell`.

- **Owns:**
  - the composer.json and composer.lock model (PSR-4 roots, scripts, the `require.php` constraint, packages with locked versions);
  - the language-level rule: an explicit `[php] language_level`, else the lower bound of `require.php`, else the probed interpreter version;
  - the interpreter probe, one `php -r` call that reports the version, the loaded ini, and whether Xdebug (with its `xdebug.mode`) and PCOV are loaded;
  - the Composer tool window's rows and the argv of every action, with arguments refused when they could be read as options;
  - the mapping from `[php]` to each language server's initialization options and settings (`lsp`), and `lsp::apply`, which lays it over the resolved `ServerConfig`s of Intelephense and Phpactor: the per-server toggles, the language level, include paths, stubs and the licence key.
    A hand-written `[[language_server]]` table stays on top of what is derived, and a user disable is not undone.
    A language-level or include-path change differs only in `settings` and is pushed with `didChangeConfiguration`; a licence-key change differs in the initialization options and restarts Intelephense (`catalog::reload_kind`).
- **Does not own** the toolchain table, Composer-script detection, `php -S` and console argv, or Run Current File for `.php`.
  They stay in `run-core` (`ToolchainId::Php`, `detect`, `context`, `php_run`), which must not depend on `php-core`.
  `run-core` receives the `[php]` interpreter through `MacroContext::php_interpreter`, set by the bridge when it launches.
- **Probing runs on an explicit host.**
  `process_exec::run_on` and `spawn_on` take an `ExecHost`; `run` and `spawn` delegate to them with the host the working directory implies.
- **The licence key lives in `secret-store`** under service `ide.php`, entry `intelephense-licence`.
  The Settings > PHP page holds an edit pending and writes it on OK; the key is passed to Intelephense as an initialization option, never stored in a settings file.
- **Composer actions run in the Run console** as temporary run configurations, so output, links, stop and rerun behave as for any other run.

## Alternatives considered

| Option | Why rejected |
|--------|--------------|
| Put the Composer model in `run-core` | Run configurations only need script names; the lock file, language level and server mappings are not run concerns, and `run-core` must stay lean. |
| Put the settings mapping in `settings-model` | It would make `settings-model` know two servers' option schemas; PHP-specific JSON belongs with PHP. |
| Let `lsp-core` know PHP | It must stay language-agnostic (ADR-0066); it receives plain JSON through `ServerConfig`. |
| Store the licence key in `[[language_server]] initialization_options` | That file may be committed. |
| Parse `composer outdated --format=json` into the window | Running `composer outdated` in the Run console needs no parser, and output is the same the user would see in a terminal. |

## Consequences

- The language level handed to the servers is the explicit setting, else `require.php`; the probed interpreter version is not used there, because probing runs a process and a server start must not wait for it.
- A global PsySH install is not detected for the PHP console; only `vendor/bin/psysh` is.
- The interpreter probe runs on the local or WSL host.
  A container interpreter is probed once the container exec host exists (ADR-0067).
- A new PHP-specific rule has an obvious home with unit tests beside it.
