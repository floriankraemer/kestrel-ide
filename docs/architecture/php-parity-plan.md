# PHP parity plan — closing the gap to PhpStorm

This plan extends `docs/architecture/php-tooling-plan.md` (PHPStan/PHPCS/PHPUnit, done through E2); it does not replace it.

## Context

Reference: PhpStorm's PHP help (https://www.jetbrains.com/help/phpstorm/php.html and its child pages).
A product-owner gap analysis compared it against the IDE, and an inventory of the code confirmed it.

What PHP has today:
- Tree-sitter highlighting, with HTML, JS and CSS injected.
- Folding, outline, and index-based symbols with Go to Implementation.
- Intelephense, with every LSP feature the client already consumes.
- PHPStan and PHPCS, but only through *Manual* runs.
- PHPUnit in the Tests dock.
- WSL as a remote host.

Biggest gaps, with evidence:
- **Analyzers never run live.**
  `Scheduler::schedule_file_run` (`analysis-core/src/scheduler.rs:107`) has no caller.
  `analysis_core::php::invocation` (`php.rs:97`) also has no caller, and no `php_binary` setting exists.
- **One server per language.**
  `LspManager.servers` is keyed by `language_id` (`lsp-core/src/manager.rs:446,844`).
  `initialize` sends no `initializationOptions` (`manager.rs:1049-1059`), so the Intelephense licence key cannot be set.
  `update_settings` (`manager.rs:797`) has no caller.
- **Missing LSP requests:** `implementation`, `typeDefinition`, `declaration`, `workspace/symbol`, `onTypeFormatting`.
  `rangeFormatting` is implemented but not wired (`ui-shell/src/bridge/language/refactor.rs:188`).
- **No PHP debugging.**
  `dap-core/src/catalog.rs:29-55` lists only codelldb, debugpy and java-debug.
  `ToolchainId` has no PHP entry, there is no listen mode, and debugging inside a container is refused.
- **No interpreter model.**
  There is no language level, no Composer scripts, no Run PHP file, no `php -S` and no PHP console.
  `ExecHost` is only `Local | Wsl` (`process-exec/src/host.rs:44`).
- **Testing:** only PHPUnit, with no single-test gutter run and no coverage.
- **Quality tools:** no Psalm, PHPMD or formatter/fixer point (php-cs-fixer, Pint, phpcbf), no format on save, and no quick fixes for findings.
- **Templates:** no Twig or Blade grammar, and `.phtml`/`.inc` are not mapped to PHP.
  Ctrl+/ in the HTML part of a `.php` file inserts `//`.
- **Code generation:** no Generate menu, no live or postfix templates, and no file templates (New PHP Class).

User decisions:
- Run **Intelephense and Phpactor at the same time**.
- Support **Docker/Compose interpreters**.
- Include Twig/Blade, the Composer tool window, coverage, the Generate menu, live templates, the PHP console and `php -S`.

Intended outcome: a Composer project gets PhpStorm-class editing, navigation, running, testing, debugging and quality tooling, locally, on WSL and in a container.

## Decisions

**D1: several language servers per language (ADR-0066).**
- Servers are keyed by a stable server `id`, with an ordered list per language.
- A pure `lsp_core::routing` table decides who answers each method:
  - `First(capable)` for definition, hover, rename, formatting and implementation.
  - `Merge(all capable)` for completion, code actions, references and workspace symbols.
- Completion items and code actions are tagged with the server they came from, so `resolve` and `executeCommand` go back to that server.
- didOpen, didChange, didSave and didClose fan out to every server.
- Diagnostics use the source key `lsp:<server-id>`, so the `(source, uri)` store (ADR-0046) merges them for free.
- A per-server `diagnostics=false` flag exists; Phpactor defaults to off.
- Each server gets its own `ExecHost`.
- `posix_only` servers (Phpactor) are skipped on native Windows and the reason is shown; they still run on WSL and in containers.

**D2: settings that drive the servers.**
- `ServerConfig` and `[[language_server]]` gain `id`, `settings` and `initialization_options`.
- `initialize` sends `initializationOptions`.
- A change to pulled settings sends `didChangeConfiguration`; a change to initialization options restarts the server (`reload_kind`).
- The mapping from PHP settings to each server's JSON lives in `php-core`, so `lsp-core` stays language-agnostic.
- The licence key lives in `secret-store`, never in a `settings.toml` that may be committed.

**D3: new support crate `php-core` (ADR-0068), on the `jvm-build-core` precedent.**
- It owns:
  - the composer.json/composer.lock model (PSR-4, scripts, the `require.php` constraint);
  - the language-level rule;
  - the interpreter probe (version, ini, xdebug/pcov);
  - the LSP settings mapping;
  - test-marker discovery;
  - the tree-sitter Generate helpers;
  - the Composer tool window model.
- It never depends on Qt or tokio.
- `ToolchainId::Php`, Composer-script detection and the `php -S`/console argv stay in `run-core` (ADR-0039 keeps one toolchain table).

**D4: `ExecHost::Container` as an explicit value (ADR-0067).**
- It amends ADR-0052 and supersedes ADR-0056's "no Container variant" paragraph.
- `ExecHost::for_path` never returns `Container`, so git, cargo and npm stay on the host.
- Consumers opt in through `process_exec::run_on`/`spawn_on`:
  - analyzers, test frameworks and formatters whose contribution says `requires-interpreter = "php"`;
  - servers configured with `exec = "interpreter"`.
- `container_core::target::exec_host` builds the host from `ContainerTargetSetting`, sharing `PathMap`.
  - Compose services default to `exec`, images to `run --rm -i`.
- `is_remote()` splits into "runs remotely" and "filesystem is remote".

**D5: Xdebug through vscode-php-debug (ADR-0069).**
- vscode-php-debug is a stdio DAP adapter that itself listens on port 9003.
- The `php-debug` row in `dap-core` finds `phpDebug.js` on its own; `[[debug_adapter]]` can override it.
- `ToolchainId::Php.debug_adapter() = "php-debug"`.
- Every PHP debug case is the same three steps:
  1. A listen session (with a `pathMappings` object).
  2. The program launched through `RunService` with the Xdebug environment.
  3. Each connection appears as a DAP thread.
- That one path covers CLI scripts, `php -S`, tests, WSL and containers (`client_host=host.docker.internal`).

**D6: new contribution points, additive, `api_version` stays 1 (the ADR-0033 reasoning).**
- **`formatters`** (ADR-0070).
- **`live-templates`**, covering abbreviation, surround and postfix templates (ADR-0072).
- **`file-templates`** (ADR-0072).
- `analyzers` gains `languages`, `file-args`, `buffer`, `composer-package`, `requires-interpreter` and `suppress-comment`.
- `test-frameworks` gains `composer-package`, `requires-interpreter`, `coverage-args` and a `junit-xml-stdout` format.
- The hardcoded Composer package table in `analysis-core/src/php.rs` is deleted.

**D7: Twig and Blade grammars vendored into `syntax-core` (ADR-0071).**
- They are built by a new `build.rs` with `cc`, because neither grammar is on crates.io.
- Blade injects `php_only` and `html`.
- Comment toggle becomes injection-aware through `syntax_core::language_at`.

**D8: deliberately skipped.**
- **LSP `documentSymbol`, `foldingRange` and `selectionRange`:** tree-sitter equivalents exist.
- **`documentLink`, pull diagnostics:** both servers push diagnostics.
- **Namespace-aware indexing:** `workspace/symbol` covers it.
- **Also not built:**
  - Rector;
  - installers for language servers or adapters (install hints only);
  - our own inspection engine;
  - change signature, pull up and push down beyond what the servers offer;
  - the Zend Debugger;
  - SFTP and Vagrant interpreters;
  - deep Laravel, Symfony and WordPress awareness (stubs settings only).

## Progress

Every row starts as `open`.
A row's status and commit hash are updated in the commit that finishes it.

### P0 — foundations
| Task | Status | Commit |
|---|---|---|
| P0-1 — this plan doc + `docs/README.md` index line | done | dfb8a2b |
| P0-2 — `AnalyzerContribution` gains `languages`/`file-args`/`buffer`/`composer-package`/`requires-interpreter`; `TestFrameworkContribution` gains `composer-package`/`requires-interpreter`; drop the `php.rs` package table; update the php-tools manifest | done | 4979052 |
| P0-3 — `settings_model::analysis::file_jobs(event, path, …)`: which analyzers fire for a file on type or save, with the SavedOnly→OnSave downgrade | done | e178fbc |
| P0-4 — `AnalysisService` calls `Scheduler::schedule_file_run` on debounced didChange and on didSave | done | 7ef569b |
| P0-5 — `[php]` settings section (interpreter, language_level, include_paths, stubs, container target/mode, xdebug_port, formatter, per-server toggles), `ScopedField::Php`, `settings_model::php::resolve` | done | 980226c |
| P0-6 — `analysis_core::php::invocation` wired with the resolved interpreter for `requires-interpreter="php"` | done | bdfc119 |
| P0-7 — checkstyle `file=` and TeamCity `php_qn://` locations mapped through `ExecHost::to_local` (also fixes a latent WSL bug) | done | a82342d |

### L — several servers per language (ADR-0066)
| Task | Status | Commit |
|---|---|---|
| L1 — `ServerDef`/`ServerConfig`: `id`, `initialization_options`, `diagnostics`, `posix_only`, `exec`; `resolve_servers` keyed by id; `intelephense` + `phpactor` rows | done | 2662562 |
| L2 — `LanguageServerSetting` gains `id`, `settings`, `initialization_options`; legacy entries still load | done | edbf427 |
| L3 — `initialize` sends `initializationOptions`; the full `ServerCapabilities` is stored per server (as raw JSON) | done | b6c7a10 |
| L4 — servers keyed by id with a per-language order; did* fans out; `LspEvent.server_id`; stop/restart per server (the bridge's diagnostics key is already `lsp:<server-id>` from here) | done | 5207174 |
| L5 — `routing` module: method → `First`/`Merge`; `request()` / `request_all()` | done | fe1c1eb |
| L6 — merge rules (completion dedupe, code actions, references/symbols); origin tagging so resolve/executeCommand reach the right server | done | 66ffeaf |
| L7 — diagnostics source `lsp:<server-id>`; `diagnostics=false`; trigger characters are the union | done | f6ba0fb |
| L8 — `ExecHost` and URI translation per server (process, `rootUri` and `did*` URIs; request params wait for the container path map, ADR-0067) | done | 835b382 |
| L9 — `posix_only` skipped on native Windows, with status text | done | 63faf37 |
| L10 — settings save → `update_settings` or restart via `catalog::reload_kind` | done | 2a538cd |
| L11 — Language Servers page shows N servers per language | done | 4ae6c97 |
| L12 — ADR-0066 + `layering.md` note | done | e8eed54 |

### I — `php-core`, interpreter, Composer, running PHP (ADR-0068)
| Task | Status | Commit |
|---|---|---|
| I1 — crate `php-core`: composer.json/composer.lock model | done | a6f489a |
| I2 — language level: explicit → `require.php` lower bound → probed version | done | eb7af7c |
| I3 — `process_exec::run_on`/`spawn_on`; interpreter probe (version, ini, xdebug/pcov, `xdebug.mode`) | done | 87be72c |
| I4 — `ToolchainId::Php`; Composer scripts detected as run configs; Run Current File for `.php` | done | 7ac1fcf |
| I5 — run kinds `php-builtin-server` (`php -S`) and `php-console` (PsySH, otherwise `php -a`) | done | 8d61c39 |
| I6 — run-configuration dialog pages for I5 | done | b5de8ed |
| I7 — Settings > PHP page: interpreter (local/container), probed version and Xdebug, language level, include paths, stubs, licence key (secret-store), server toggles | done | cf63262 |
| I8 — Composer tool window: scripts, packages, install/update/require/remove/dump-autoload/outdated | done | eaf9a0f |
| I9 — ADR-0068 + `layering.md` row + Qt/tokio gate | done | 2877b7d |

### PS — PHP language-server configuration
| Task | Status | Commit |
|---|---|---|
| PS1 — `php_core::lsp::{intelephense, phpactor}` → (initialization options, settings), golden JSON | done | 0c4f605 |
| PS2 — ui-shell joins PS1 into `ServerConfig` on start and on settings save | done | 01441fe |
| PS3 — install hints (npm / phar) in the server start error | done | 65b95d1 |

### N — missing LSP requests
| Task | Status | Commit |
|---|---|---|
| N1 — `implementation`/`typeDefinition`/`declaration`; Go to Implementation tries LSP first and falls back to the index | done | 497cad8 |
| N2 — Go to Type Declaration action; Go to Declaration uses `declaration` when the server supports it | done | bf163dd |
| N3 — `workspace/symbol` merged with index hits; Go to Class | done | 8b487fb |
| N4 — Reformat Selection → `format_range` | done | 1ac97c6 |
| N5 — `onTypeFormatting`, applied as one undo step | done | f58eec3 |

### X — container exec host (ADR-0067)
| Task | Status | Commit |
|---|---|---|
| X1 — `ExecHost::Container` (argv/to_remote/to_local); `is_remote` split; walk the match sites | open | |
| X2 — `resolve_program` inside the container; NotFound mapping | open | |
| X3 — `container_core::target::exec_host` (exec/run modes, Docker/Podman/Compose) | open | |
| X4 — scheduler, test runner, formatter and `exec="interpreter"` servers run on the interpreter host | open | |
| X5 — PHP run configs inherit `run_on` = the interpreter target (including `php -S` ports) | open | |
| X6 — ADR-0067 + `layering.md` rows | open | |

### D — Xdebug (ADR-0069)
| Task | Status | Commit |
|---|---|---|
| D0 — `stub_adapter` bin in `dap-core` | open | |
| D1 — `php-debug` catalog row, automatic location of `phpDebug.js`, `ToolchainId::Php.debug_adapter()` | open | |
| D2 — `launch.rs` `php-debug` arm (`pathMappings` as an object) | open | |
| D3 — "Start Listening for PHP Debug Connections" toggle; connections arrive as threads | open | |
| D4 — `dap_core::xdebug::env` | open | |
| D5 — debug PHP run configs and tests in a container (lift the refusal for `php-debug` only) | open | |
| D6 — Xdebug check (extension missing, mode without `debug`) on the PHP page and in the debug start error | open | |
| D7 — ADR-0069 | open | |

### T — testing
| Task | Status | Commit |
|---|---|---|
| T1 — Pest, Codeception, Behat and PHPSpec rows (`junit-xml-stdout` format) | open | |
| T2 — `php_core::tests::markers` (PHPUnit `test*`/`#[Test]`/`@test`, Pest `test`/`it`/`describe`) | open | |
| T3 — gutter Run / Debug / Run with Coverage per test | open | |
| T4 — `test_core::coverage` Clover parser + `coverage-args`, paths mapped to local | open | |
| T5 — coverage gutter stripes + Coverage dock | open | |
| T6 — ADR-0048 amendment | open | |

### Q — quality tools (ADR-0070)
| Task | Status | Commit |
|---|---|---|
| Q1 — Psalm and PHPMD analyzer rows | open | |
| Q2 — `formatters` contribution point; php-cs-fixer, Pint and phpcbf rows | open | |
| Q3 — `analysis_core::format` through the buffer strategies; formatter mode in the stub analyzer | open | |
| Q4 — Reformat Code uses the configured formatter, otherwise LSP; result applied as diff edits in one undo step | open | |
| Q5 — `format_on_save` per language | open | |
| Q6 — suppress quick fixes (`@phpstan-ignore`, `@psalm-suppress`, `phpcs:ignore`) in Alt+Enter | open | |
| Q7 — "Fix with phpcbf" intention | open | |
| Q8 — ADR-0070 | open | |

### Y — templating languages (ADR-0071)
| Task | Status | Commit |
|---|---|---|
| Y1 — vendor the Twig/Blade sources, `build.rs` + `cc` | done | — |
| Y2 — `twig` and `blade` rows (`.blade.php` beats `.php`), hidden `php_only` row, queries | open | |
| Y3 — `.phtml` and `.inc` → PHP | open | |
| Y4 — `syntax_core::language_at`; injection-aware comment toggle | open | |
| Y5 — richer `php/folds.scm` (arrays, doc comments, use groups, match, heredoc, attributes) | open | |
| Y6 — ADR-0071 | open | |

### G — code generation (ADR-0072)
| Task | Status | Commit |
|---|---|---|
| G1 — `live-templates` point + user `[[live_template]]` | open | |
| G2 — `edit_ops::templates`: expand, surround, postfix → Transaction + snippet stops | open | |
| G3 — PHP live and postfix templates in php-tools | open | |
| G4 — view: Tab expansion, Ctrl+J, Ctrl+Alt+T, postfix in completion | open | |
| G5 — `file-templates` point; `php_core::psr4::namespace_for`; Class/Interface/Trait/Enum/Test templates | open | |
| G6 — New > File/Directory/from template in the project tree and the File menu | open | |
| G7 — Generate menu (Alt+Insert): constructor/getters/setters from `php_core::generate`, plus server `source.*` code actions | open | |
| G8 — ADR-0072 | open | |

### E — verification and docs
| Task | Status | Commit |
|---|---|---|
| E1 — `stub_server` capability-profile flag (`STUB_LSP_TAG`/`STUB_LSP_CAPS`, done with L3) | done | b6c7a10 |
| E2 — E2E `e2e_php_two_servers_and_on_save_analysis` | open | |
| E3 — nightly E2E behind `IDE_E2E_PHP=1`: real PHP, Xdebug breakpoint, gutter test, container interpreter | open | |
| E4 — manual matrix (old E3 plus licence key, Phpactor/WSL, container, Xdebug, coverage, Twig/Blade, templates), recorded here | open | |
| Z1 — `overview.md`, `layering.md`, README index, keymap defaults | open | |

## Acceptance per phase (as the user sees it)

- **P0:** editing or saving a PHP file updates PHPStan and PHPCS squiggles without Inspect Project, using the configured `php`.
- **L/PS:**
  - Intelephense and Phpactor both run; completion merges without duplicates; Problems shows both sources.
  - A licence key unlocks Intelephense premium.
  - A language-level change applies without restarting the IDE.
  - On Windows without WSL, only Intelephense runs, with a reason shown.
- **I:** Run Current File, Composer scripts, `php -S` and the PHP console work; the Composer dock lists packages and runs actions.
- **N:** Go to Implementation, Go to Type Declaration, Go to Class and Reformat Selection work against Intelephense.
- **X:** with a Compose-service interpreter, analyzers, tests, Phpactor, Run and `php -S` run inside the container, and locations open the local files.
- **D:** breakpoints hit for a CLI script, a `php -S` request and a PHPUnit test, locally, on WSL and in a container.
- **T:**
  - A single PHPUnit or Pest test runs or debugs from the gutter.
  - Codeception, Behat and PHPSpec fill the Tests dock.
  - Coverage paints the editor.
- **Q:**
  - Psalm and PHPMD findings appear inline.
  - Reformat and format on save use php-cs-fixer, Pint or phpcbf.
  - Alt+Enter offers suppress and fix actions.
- **Y:** Twig and Blade highlight and fold; Ctrl+/ in the HTML part of a `.php` file inserts `<!-- -->`.
- **G:** these work, including on Windows:
  - `fore`+Tab;
  - `$x.foreach`;
  - New > PHP Class with the correct namespace;
  - Alt+Insert → Getters and Setters.

## Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | Merged results show duplicates or conflict | One primary server per method; L6 dedupe; Phpactor diagnostics off by default; the order is configurable |
| 2 | resolve or executeCommand reaches the wrong server | Origin tag at ingest, covered by a stub test |
| 3 | Phpactor has no Windows build | `posix_only` + status; Generate also has tree-sitter generators |
| 4 | Container host leaks into git, cargo or npm | `for_path` never returns Container; only `requires-interpreter` callers opt in |
| 5 | OnType analysis through `docker run` is slow | `exec` mode by default for Compose; debounce |
| 6 | Xdebug cannot reach the adapter from the container on Linux | D6 check + `extra_hosts: host-gateway` hint |
| 7 | Twig/Blade ABI drift | Regenerate when vendoring; runtime-grammar fallback |
| 8 | Licence key committed by accident | Stored in secret-store only |
| 9 | No PHP, Node or Docker in `linux-builder` | Stubs for per-PR tests; real toolchain nightly and in the manual matrix |
| 10 | One large PR | One conventional commit per task, each with its Progress row, gated |

## Delivery

**Everything in this plan ships in exactly one PR**: every phase (P0 through Z1) lands on one branch, and no intermediate PRs are opened.

- **Workflow:**
  - One branch `feat/php-parity` in a dedicated worktree with its own `target/`. Check `df -h /` first.
  - Tasks run sequentially in table order.
    Each task goes to an implementer subagent on `model: "sonnet"` (`senior-software-engineer`), working in that worktree.
    The main session orchestrates and reviews; it does not write the code itself.
  - Escalation: if a Sonnet agent fails a task twice, or it is a hard problem by nature, the task is re-dispatched to the same agent type on `model: "opus"`.
    Hard problems include LSP routing/merge across concurrent servers, `ExecHost::Container` match-site walk, Xdebug listen sessions, Twig/Blade ABI regeneration, and cxx-qt seam breakage.
  - One conventional commit per task updates its Progress row.
- **Testing:**
  - Tests run locally only, in Docker via the Makefile.
  - While implementing a task, run `cargo test -p <crate>`/`cargo check -p <crate>` from `make shell`.
  - Before every commit, run `make test` + `make lint`.
  - E2E runs only at the end (E2/E3), once testing-expert and product-owner have chosen the flows.
- **PR and follow-ups:**
  - Only when every row is done: the single PR to `main`, self-merged once green, then the worktree, branches and Docker leftovers are cleaned up.
  - Follow-ups found but not fixed become gh issues, linked in the final report.

## Verification

- **Gates:**
  - `make test`, `make lint`, `scripts/check-test-layout.sh`.
  - `cargo tree -p php-core -e normal | grep -iE 'qt|tokio'` must be empty, as must the existing Qt gates.
- **Unit tests:**
  - routing and merge;
  - settings mapping (golden JSON);
  - composer model and language level;
  - container argv golden tests and path round-trips;
  - xdebug env/plan;
  - Clover, markers and templates;
  - namespace_for;
  - the generators.
- **Stub-driven tests:**
  - two `stub_server` profiles;
  - `stub_analyzer` in formatter mode;
  - `stub_adapter` for listen sessions.
- **E2E:**
  - E2 runs per-PR flows under Xvfb.
  - E3 runs nightly against real PHP 8.3, Composer, Intelephense, Phpactor, vscode-php-debug and Docker.
- **Manual (E4):** a real Laravel or Symfony Composer project, walked by hand on Linux, WSL and Windows, with screenshots checked pixel by pixel.

## Critical files
- `crates/lsp-core/src/{manager.rs,catalog.rs}` (+ new `routing.rs`)
- `crates/process-exec/src/host.rs`
- `crates/container-core/src/target.rs`
- `crates/dap-core/src/{catalog.rs,launch.rs}`, `crates/ui-shell/src/bridge/debug/mod.rs`
- `crates/plugin-api/src/manifest/mod.rs`, `crates/plugin-host/builtin/php-tools/plugin.toml`
- `crates/analysis-core/src/{scheduler.rs,php.rs}`, `crates/ui-shell/src/bridge/analysis/mod.rs`
- `crates/run-core/src/{toolchain.rs,detect.rs,context.rs}`
- `crates/syntax-core/src/catalog.rs`, `crates/edit-ops/src/comment.rs`
- new `crates/php-core/`
