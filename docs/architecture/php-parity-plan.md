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
| X1 — `ExecHost::Container` (argv/to_remote/to_local); `is_remote` split; walk the match sites | done | 1d1806e |
| X2 — `resolve_program` inside the container; NotFound mapping | done | 463118a |
| X3 — `container_core::target::exec_host` (exec/run modes, Docker/Podman/Compose) | done | 85b2938 |
| X4 — scheduler, test runner, formatter and `exec="interpreter"` servers run on the interpreter host | done | 6600ce2 |
| X5 — PHP run configs inherit `run_on` = the interpreter target (including `php -S` ports) | done | 4169dbe |
| X6 — ADR-0067 + `layering.md` rows | done | 0c30f43 |

### D — Xdebug (ADR-0069)
| Task | Status | Commit |
|---|---|---|
| D0 — `stub_adapter` bin in `dap-core` | done | 3320bee |
| D1 — `php-debug` catalog row, automatic location of `phpDebug.js`, `ToolchainId::Php.debug_adapter()` | done | 210eefe |
| D2 — `launch.rs` `php-debug` arm (`pathMappings` as an object) | done | 5995082 |
| D3 — "Start Listening for PHP Debug Connections" toggle; connections arrive as threads | done | 88d670f |
| D4 — `dap_core::xdebug::env` | done | cfb5cb9 |
| D5 — debug PHP run configs and tests in a container (lift the refusal for `php-debug` only) | done | c0c6d6d |
| D6 — Xdebug check (extension missing, mode without `debug`) on the PHP page and in the debug start error | done | 03cb350 |
| D7 — ADR-0069 | done | 1c70a70 |

### T — testing
| Task | Status | Commit |
|---|---|---|
| T1 — Pest, Codeception, Behat and PHPSpec rows (`junit-xml-stdout` format) | done | 6cc7057 |
| T2 — `php_core::tests::markers` (PHPUnit `test*`/`#[Test]`/`@test`, Pest `test`/`it`/`describe`) | done | 0a84f0d |
| T3 — gutter Run / Debug / Run with Coverage per test | done | 4092770 |
| T4 — `test_core::coverage` Clover parser + `coverage-args`, paths mapped to local | done | 414f9fc |
| T5 — coverage gutter stripes + Coverage dock | done | 6920853 |
| T6 — ADR-0048 amendment | done | 5237a52 |

### Q — quality tools (ADR-0070)
| Task | Status | Commit |
|---|---|---|
| Q1 — Psalm and PHPMD analyzer rows | done | 290b5f1 |
| Q2 — `formatters` contribution point; php-cs-fixer, Pint and phpcbf rows | done | 1ca74be |
| Q3 — `analysis_core::format` through the buffer strategies; formatter mode in the stub analyzer | done | d8e9042 |
| Q4 — Reformat Code uses the configured formatter, otherwise LSP; result applied as diff edits in one undo step | done | c21a39d |
| Q5 — `format_on_save` per language | done | aca9bd6 |
| Q6 — suppress quick fixes (`@phpstan-ignore`, `@psalm-suppress`, `phpcs:ignore`) in Alt+Enter | done | 86451ff |
| Q7 — "Fix with phpcbf" intention | done | ba3ad0e |
| Q8 — ADR-0070 | done | 9c4bcaf |

### Y — templating languages (ADR-0071)
| Task | Status | Commit |
|---|---|---|
| Y1 — vendor the Twig/Blade sources, `build.rs` + `cc` | done | 16b6fd5 |
| Y2 — `twig` and `blade` rows (`.blade.php` beats `.php`), hidden `php_only` row, queries | done | 5e2da04 |
| Y3 — `.phtml` and `.inc` → PHP | done | a787251 |
| Y4 — `syntax_core::language_at`; injection-aware comment toggle | done | faac937 |
| Y5 — richer `php/folds.scm` (arrays, doc comments, use groups, match, heredoc, attributes) | done | d54d9a5 |
| Y6 — ADR-0071 | done | 143e250 |

### G — code generation (ADR-0072)
| Task | Status | Commit |
|---|---|---|
| G1 — `live-templates` point + user `[[live_template]]` | done | dd628ec |
| G2 — `edit_ops::templates`: expand, surround, postfix → Transaction + snippet stops | done | d4edec9 |
| G3 — PHP live and postfix templates in php-tools | done | f381669 |
| G4 — view: Tab expansion, Ctrl+J, Ctrl+Alt+T, postfix in completion | done | d84c601 |
| G5 — `file-templates` point; `php_core::psr4::namespace_for`; Class/Interface/Trait/Enum/Test templates | done | 2ccbcf5 |
| G6 — New > File/Directory/from template in the project tree and the File menu | done | cb2e560 |
| G7 — Generate menu (Alt+Insert): constructor/getters/setters from `php_core::generate`, plus server `source.*` code actions | done | 4b1c1c4 |
| G8 — ADR-0072 | done | 1791717 |

### E — verification and docs
| Task | Status | Commit |
|---|---|---|
| E1 — `stub_server` capability-profile flag (`STUB_LSP_TAG`/`STUB_LSP_CAPS`, done with L3) | done | b6c7a10 |
| E2 — per-PR E2E: `e2e_php_two_servers_and_on_save_analysis`, `e2e_php_listen_session_stops_for_two_connections`, `e2e_php_generate_templates_and_new_class` | done | 40db0bb |
| E3 — nightly E2E behind `IDE_E2E_PHP=1`: real PHP, Xdebug breakpoint, gutter test, container interpreter | done | 6e0bd8e |
| E4 — manual matrix (old E3 plus licence key, Phpactor/WSL, container, Xdebug, coverage, Twig/Blade, templates), recorded here | done | — |
| Z1 — `overview.md`, `layering.md`, README index, keymap defaults | done | d442627 |

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

## Phase E test plan

Agreed by the testing expert and the product owner on 2026-10-02.

### E2 — per-PR flows (stubs only, under Xvfb)

- Prerequisites:
  - `stub_analyzer` reads a file (last argument an existing file) or stdin (`-` with `--stdin-path=`) and reports one finding per line containing `STUB_FINDING`.
  - Its `source` is `PHPStan.stubFinding` when the binary is named `phpstan`, else `Stub.Sniff.Finding`; the project-root mode stays.
  - `stub_server` publishes `source: "stub_<tag>"` when `STUB_LSP_TAG` is set.
  - `make e2e-ci` builds `stub_adapter`.
  - `route_language_at_stubs` routes a language at tagged stub servers with diagnostics on.
- New marks: the Generate menu and member dialog, the Ctrl+J and Ctrl+Alt+T menus, the New submenu and its name prompt, and `checked` on menu actions.
- Flow `analysis::e2e_php_two_servers_and_on_save_analysis`:
  - Problems shows a row per server (`stub_intelephense`, `stub_phpactor`).
  - Completion merges both servers and lists `shared` once.
  - Typing `// STUB_FINDING` adds a PHP_CodeSniffer row and no PHPStan row.
  - Saving adds the PHPStan row.
  - Alt+Enter offers each server's fix and a suppress action per analyzer.
  - The PHPStan suppress inserts `// @phpstan-ignore PHPStan.stubFinding` above the line, and one Ctrl+Z removes it.
  - `fore` + Tab expands `foreach (`.
- Flow `run::e2e_php_listen_session_stops_for_two_connections`:
  - The Run menu toggle starts one listen session and shows as checked.
  - The first connection stops at the breakpoint with variables.
  - F9 brings the second stop on the same session, with no second `debug_started`.
  - Toggling off ends the session.
  - The container target's `0.0.0.0` listener and `pathMappings` are asserted in a `dap-core` integration test, since they need no UI.
- Flow `edit::e2e_php_generate_templates_and_new_class`:
  - Alt+Insert > Getters and Setters generates no setter for a readonly property.
  - File > New > PHP Class in `src/Sub` creates `App\Sub\Thing` under the composer PSR-4 map.
  - Ctrl+Alt+T wraps a line in `if`.
  - `$xs.foreach` + Tab expands.
  - Ctrl+/ on an HTML line of a `.php` file writes `<!-- … -->`.
- Alt+1 shows the Project dock but does not toggle it (see the follow-ups in the delivery report), so no flow asserts a toggle.

### E3 — nightly flows (`IDE_E2E_PHP=1`)

- Image `linux-php`, built `FROM linux-builder` like `linux-jvm`:
  - PHP 8.3 (sury) with cli, xdebug, pcov, mbstring, xml, curl, zip and intl, and `xdebug.mode=off`, `pcov.enabled=0`;
  - Composer pinned with a checksum;
  - Node and Intelephense pinned;
  - the Phpactor phar pinned;
  - the vscode-php-debug VSIX from open-vsx unzipped into `/opt/vscode-home/.vscode/extensions` (`HOME=/opt/vscode-home`, so auto-location is tested);
  - the Docker CLI and Compose for the container flow;
  - a prewarmed `COMPOSER_CACHE_DIR` and a `composer install` per test, with no vendor symlink.
- Fixture `crates/app/tests/fixtures/php_app/`:
  - PSR-4 `App\` to `src`, `Tests\` to `tests`;
  - PHPUnit ^11, Pest ^3, PHPStan, PHPCS and php-cs-fixer, with a committed `composer.lock`;
  - `Greeter` and `GreeterInterface` with a planted PHPStan error and a PSR-12 violation;
  - `GreeterTest` and `GreeterPestTest`;
  - `phpstan.neon`, `phpcs.xml`, `.php-cs-fixer.dist.php`, `bin/console.php` and `public/index.php`.
- Makefile: `linux-php-image`, `test-php` (outer) and `php-ci` (inner), and `test-php-container` (Compose like `test-db`, with `docker.sock` and an identical bind path).
- Flows in `php_real.rs`:
  - a) servers, analysis, navigation and format, plus the Composer dock;
  - b) Xdebug from the CLI, a listen session, and `php -S` with `?XDEBUG_TRIGGER=1`;
  - c) a gutter test, debug from the gutter, coverage, the Pest marker and a namespaced rerun;
  - d) the container interpreter, in `test-php-container` only.

#### E3 results (run locally, 2026-10-02)

`make test-php` ran flows a to c green (six tests with the skipped container flow) and `make test-php-container` ran flow d green against `php:8.3-cli` through the host's Docker socket.
Running real tools found and fixed three bugs:
- A missing `phpDebug.js` started `node phpDebug.js`, which spawns and then dies with an opaque "adapter disconnected"; the start is now refused with the install hint (`dap_core::catalog::not_located`).
- A rerun of a PHPUnit class's test in a Pest project filtered on Pest's prettified name (`Greets by name` for `testGreetsByName`), matched nothing and still reported success; the Pest framework now uses the `pest-regex` filter dialect, which accepts the reported name or its words.
- The harness copied `vendor/bin` symlinks as files, which cannot find the autoloader; `copy_tree` keeps symlinks.

Verdicts for the "Verify" lines of `followups.md`:
- **vscode-php-debug in listen mode** sends no `terminated` when a connection ends: one session served two `php -S` requests in turn, and only the toggle ended it.
- **PHPStan relative paths:** its checkstyle names files relative to its working directory (`src/Greeter.php`); the IDE runs it from the project root and the Problems row carries the absolute path.
- **Pest filter shape:** `Suite::<reported name>` matches Pest's own tests, including a `describe` block's `` `Group` → it x `` name; a PHPUnit class's tests only match by method name, which the gutter marker uses and the tree rerun now reconstructs.
- **php-cs-fixer and Pint on the dotfile temp copy** both rewrite it (php-cs-fixer in the editor flow, Pint on the command line with the same file name shape).
- **phpcbf `--sniffs`** accepts `Standard.Category.Sniff` and refuses the four-part message code, so `fix-args = ["--sniffs={sniff}"]` is right.
- **PHPStan stacked ignores:** two stacked `// @phpstan-ignore` lines suppress the finding without an unused-ignore report.
- **Intelephense `didChangeConfiguration` with `settings: null`** makes it send `workspace/configuration` again and re-index, so the null push is enough.

The container flow proves PHP runs in the service by a test that asserts the service's hostname.
It does not debug in the container: the official image has no Xdebug, so that was walked by hand in E4 item 6 with an image that adds it.

### E4 — manual matrix (Linux, WSL, Windows; light and dark; 100% and 150% DPI)

1. Settings > PHP, pixel by pixel.
2. The licence key lives only in the secret store.
3. Windows without WSL shows the Phpactor reason.
4. A WSL interpreter.
5. Container `exec` against `run` latency.
6. Xdebug from the CLI, `php -S` and PHPUnit, locally, on WSL and in a container, with the advice shown.
7. The listen toggle's glyph and sync.
8. The Composer dock.
9. Coverage colours.
10. Generate.
11. New menu namespaces.
12. Ctrl+J and Ctrl+Alt+T (German layout, AltGr).
13. Twig and Blade.
14. The format-on-save failure notice.
15. Search Everywhere with six tabs and Ctrl+N.
16. A real Laravel or Symfony project: open time, memory, Psalm, PHPMD, Pint, phpcbf, Codeception, Behat and PHPSpec.
17. `composer require` mid-session is picked up.
18. The "Verify …" lines in `followups.md`.

#### E4 — isolating the Windows and WSL walks

The Windows build resolves its config dir through the Known Folder API, which ignores `APPDATA`, so a launch would use the real `%APPDATA%\ide`.
`IDE_CONFIG_DIR` (read by `project_model::default_config_dir`) points the app at a throwaway directory instead.
The walk launches with `IDE_CONFIG_DIR=C:\Users\flori\ide-phptest\config` and pre-seeds `last-project.txt` there with the fixture path, since the app takes no project argument.
The OS keychain is written only by the Settings fields that store a secret, so a walk that enters no key and no password never touches it.

#### E4 results — Linux column (2026-10-02)

Driven in the real app under Xvfb in the `linux-php` image (real PHP 8.3, Xdebug, PCOV, Intelephense, Phpactor, vscode-php-debug), over the `php_app` fixture, in the dark and light themes at 100% and at 150% (`QT_SCALE_FACTOR=1.5`).
The WSL and Windows columns follow in the next section.
Rows 5, 6 (container) and 16 were walked afterwards, see the container section below; the Windows half of row 2 is recorded as not checked.

| Row | Linux result | Notes |
|---|---|---|
| 1. Settings > PHP | pass after fixes | Probe line, advice, language level, formatter combo and masked licence field render in both themes and at 150%; Detect took the dialog's default button (fixed); the stock OK/Cancel icons come from the container's missing icon theme. |
| 2. Licence key only in the secret store | pass | A typed key with no OS keychain gives the "No OS keychain available" warning; the key is in no file under the config dir, the project or `HOME`. The keychain-present path needs a desktop session. |
| 3. Windows without WSL shows the Phpactor reason | n/a | Windows only; see the Windows column below. |
| 4. WSL interpreter | n/a | WSL only; see the WSL column below. |
| 5. Container `exec` against `run` latency | measured | See the container section below. |
| 6. Xdebug from the CLI, `php -S`, PHPUnit | CLI pass after fixes | Run configuration stops at the breakpoint under real Xdebug, three scope fetches per stop; `php -S` and the gutter test are the green E3 flows. The Debug dock's inputs, frame path and variable names were cut off at the default height (fixed), and the `xdebug.mode` advice no longer prints on IDE-started runs, only when the listen toggle starts a listener. Container: see the container section below. WSL: not checkable (see the WSL column). |
| 7. Listen toggle glyph and sync | pass after fix | Run menu check and toolbar highlight agree in both states; the toolbar clipped the checked button and the config combo (fixed). |
| 8. Composer dock | pass after fixes | Lists the locked packages; stayed stale after a change on disk (fixed). At the default width its button row forced a horizontal scroll bar and clipped the version column (fixed: Install and Update stay, the rest sit behind More; the version always shows, names elide in the middle). |
| 9. Coverage colours | pass after fixes | Covered lines green, uncovered red in the gutter, per-file and per-directory percentages in the dock; a file with no executable lines shows a dash instead of `0.0% (0/0)` (fixed). Captured at 150% once the harness scaled marker coordinates (fixed). |
| 10. Generate | pass after fix | Menu with the reason on disabled entries; the member picker had no heading or Select All and None (fixed). |
| 11. New menu namespaces | partial | The submenu lists the PHP kinds; the name prompt is E2's flow. |
| 12. Ctrl+J and Ctrl+Alt+T | partial | Both menus open, Ctrl+Alt+T included; the German layout and AltGr are pending. |
| 13. Twig and Blade | pass | Both highlighted in both themes. |
| 14. Format-on-save failure notice | pass after fixes | A project's `[editing] format_on_save` was never applied (fixed); the notice then clipped under the status bar's widgets (fixed). |
| 15. Search Everywhere with six tabs, Ctrl+N | pass after fixes | Classes tab lists the project's and vendor's classes, headed Classes, each with its kind and file greyed after the name; exact, prefix and substring matches now rank before loose subsequences. The first results take 1 to 3 s on a cold index. The dock tab bar seen under the popup is the popup overlapping it, not a rendering fault. |
| 16. A real Laravel or Symfony project | pass after fixes | Laravel 12.69.3, see the Laravel section below. |
| 17. `composer require` mid-session | pass after fix | `composer require --dev` was picked up by the dock only after fixing its reload. |
| 18. The "Verify ..." lines | done in E3 | See the E3 results. |

#### E4 results — WSL and Windows columns (2026-10-03)

Driven in the real Windows build (`dist/php-windows`, mingw cross-build) on the Windows 11 desktop, German layout, 2560x1440 at 100% scale (the display's real scale), light and dark, and once at `QT_SCALE_FACTOR=1.5`.
Config isolated with `IDE_CONFIG_DIR` (see above); the fixture lives under `C:\Users\flori\ide-phptest\proj`, and for WSL under `\\wsl.localhost\Ubuntu\home\florian\ide-phptest-wsl`.
The distro has PHP 8.4.6 with Xdebug, Composer and Node, but no Intelephense and no Phpactor.
Windows itself has no PHP.
Screenshots: `screens/win-*.png`, cropped to the IDE window.

| Row | Windows (native) | WSL project | Notes |
|---|---|---|---|
| 1. Settings > PHP | pass | pass | Detect inside the WSL project reports `PHP 8.4.6 · Xdebug (develop)` with the `xdebug.mode` advice. The search box placeholder elided to "Search settin..." with Windows' wider UI font (fixed). |
| 3. Phpactor reason | pass after fix | n/a | Status `Unavailable` with "Phpactor needs a POSIX system and does not run on native Windows. Open the project in WSL or run the server in a container."; Intelephense shows `Stopped` and "program not found. Install it with: npm i -g intelephense". The Status column and detail strip were blank because the page only listened to state changes while open (fixed; Linux showed the same blank rows). |
| 4. WSL interpreter | n/a | pass after fix | The status bar shows `WSL: Ubuntu`; Phpactor is not skipped: it is started in the distro and reports `Stopped`, "phpactor not found inside the WSL distro. Install it with: composer global require phpactor/phpactor, or download phpactor.phar and put it on PATH as phpactor". The status read `Stopped` rather than `Command not found`; fixed, the start failure is now classified in `lsp-core` (`StartFailure::NotFound`). |
| 7. Listen toggle | pass | not run | Run menu check and toolbar highlight agree on and off. |
| 8. Composer dock | pass after fix | not run | The dock opened as its own right column and squeezed the editor; it now tabs beside Structure and AI Chat (fixed, seen on Linux and Windows). |
| 9. Coverage dock | empty state only | not run | No PHP on Windows, so no run; the dock, its toolbar and the Tests dock render in both themes. |
| 10. Generate | pass | not run | Alt+Insert menu with the disabled reasons, and the Getters picker with Select All and None. |
| 11. New menu | pass | not run | File > New lists the PHP kinds. |
| 12. Ctrl+J and Ctrl+Alt+T | pass | not run | The active layout is de-DE. Ctrl+J, Ctrl+Alt+T with the left Alt and with the right Alt (AltGr) all open their menus. `fore` plus Tab expands to `foreach ($array as $item)` inside a method body (at file level it is not a template context and Tab indents). |
| 13. Twig and Blade | pass | not run | Both highlighted in both themes (Twig delimiters, keywords, strings, comments; Blade directives). |
| 15. Search Everywhere | pass | not run | Ctrl+N opens the Classes tab with kind and path (backslashes on Windows). |
| 6. Xdebug at a breakpoint | not checkable | not checkable | Windows has no PHP, and the user's WSL distro has no vscode-php-debug; installing it would modify the distro's home, which the walk must not do. The listen toggle is row 7. |

Pixel findings fixed from this walk: the Language Servers status above; the project tree's sort arrow was the platform's `SP_ArrowUp` (a black triangle on Windows, a green disc on Linux), now drawn in the theme colour; checkable list and table rows (the Getters picker, the Language Servers On column) kept the platform's box while every `QCheckBox` had the themed tick, now the same box; the settings search placeholder (above).
At 150% the Settings dialog is taller than the 1600x1000 main window, which is how the walk window was sized, not a defect.

Not checked in these columns: row 2 on Windows (the walk enters no licence key and writes nothing to the keychain; recorded here as not checkable without touching the user's keychain), rows 5 and 16 (walked on Linux, see the container section), row 6 on Windows and WSL (no `vscode-php-debug` adapter, so no stop at a breakpoint; the listen toggle's glyph and sync are row 7), the Variables name tooltip (set on every variable row, checked by reading the code only because the walk cannot stop at a breakpoint).
Follow-ups, all fixed afterwards: the live status said `Stopped` for a server missing inside WSL (the C++ page matched only `No such file`; the classification now lives in `lsp-core`); the `Unavailable` detail strip was red like an error although it is a platform rule (now muted); `index-core`'s cache directory followed the real `%LOCALAPPDATA%` (it now lives under `IDE_CONFIG_DIR/cache` when that is set, resolved in `project_model::default_cache_dir`); the Analysis page's Status column elided at the default width (it takes the free width now, checked at 100% and 150%).

#### E4 results — container, Xdebug and Laravel (2026-10-03)

Driven in the real app under Xvfb in the `linux-php` image, with the host Docker engine reached through its socket (the engine is Docker Desktop style: `host-gateway` resolves to `192.168.65.254`).
Screenshots are in `screens/row5-*.png`, `screens/row6-*.png` and `screens/row16-*.png` of the walk kit.

**Row 5, container `exec` against `run`.**
The interpreter is the compose service `php` (`php:8.3-cli`) from `docker/php-compose.yml`, `container_mode` `exec` or `run`.
The baseline is PHP in the test container itself.
Every number is wall time from the key press, read from the app's marker stream (20 ms poll), on the `php_app` fixture.

| | local | exec | run |
|---|---|---|---|
| One `php -v` through the engine | 0.02 s | 0.13 to 0.16 s | 0.9 to 1.3 s |
| Open `Greeter.php` to the first PHPCS result | 6.3 to 6.6 s | 6.4 s | 8.6 to 9.3 s |
| Ctrl+S with format-on-save (php-cs-fixer): time until the UI answers | 0.20 to 0.24 s | 1.49 s | 8.2 to 8.6 s |
| On-type, first squiggle refresh after the keystroke (3 samples) | 0.48 to 0.51 s | 0.61 to 0.64 s | none in 90 s |
| On-type, second refresh | 6.26 to 6.28 s | 1.40 to 1.43 s | none |

- The second refresh is the slower of the sources that answer after PHPCS; the marker does not say which, so it is not attributed. In the local case the image loads Xdebug into PHP, which PHPStan restarts without.
- Format-on-save blocks the UI thread: the UI answers a request only when the formatter has finished. In `exec` mode that is 1.5 s, above the 500 ms line, and 8.3 s in `run` mode. It was recorded for an issue, then fixed (`0789a70`, numbers below).
- In `run` mode every program lookup and every tool run starts a container (about 23 in the first 30 s). The lookups happened on the Qt thread (`resolve_launch`), which is where most of the 8 s came from (fixed in `5d96286`).
- After the 8 s save in `run` mode a modal "modified outside the editor" prompt appeared (seen once), and the keystrokes that followed went to it, so no on-type result could be measured in that mode. In `exec` mode the prompt did not appear (fixed in `401a94e`).

**Row 6, Xdebug in a container.**
The service is built `FROM php:8.3-cli` with `pecl install xdebug` and `extra_hosts: ["host.docker.internal:host-gateway"]`, and mounts the project at `/app`, so the path mapping is not the identity (`/app/<dir>` to the local checkout).
The test container publishes the listen port (`-p 9003:9003`) because the engine here is not the test container's own network.

| Case | Result |
|---|---|
| CLI script (`php bin/console.php Xdebug`, run configuration through `compose run`) | The breakpoint at line 9 stops at the local line 9 with the local frame path and variables; resume finishes the script with exit 0 and output. |
| `php -S` request (built-in server run configuration, port 8123 published, listen toggle started first) | Two `?XDEBUG_TRIGGER=1` requests in turn each stop at `public/index.php:9` in one session, with no second `debug_started`; both bodies come back after resume. |
| Listen toggle | The Run menu check and the toolbar highlight agree while the session listens. |
| PHPUnit | Not repeated in the container: the gutter debug path is the E3 flow, and it goes through the same listen session. |
| WSL | Not checkable here: vscode-php-debug is not installed in the user's distro, and the walk must not modify the distro's home. |

Found and fixed: a `php -S` run configuration in a compose service exited at once with "--service-ports and --publish are incompatible" (`wrap_launch` added both); the flag is now added only when the target publishes nothing itself.

**Row 16, a real Laravel project.**
`composer create-project laravel/laravel:^12.0` (Laravel Framework 12.69.3, 73 MB with `vendor/`), plus `phpstan/phpstan` and `squizlabs/php_codesniffer` with a `phpstan.neon` at level 5 over `app`, `routes` and `tests` and a PSR-12 `phpcs.xml`.
Laravel 11 could not be installed: every release matching `^11.31` is blocked by Composer's security advisories.

| Measurement | Value |
|---|---|
| Launch to `project_opened` | 1.6 to 2.1 s |
| Launch to index complete | 10.9 to 11.7 s |
| Peak RSS (`VmHWM`) after the index | 477 to 527 MB |
| After opening and saving four files (PHPStan, PHPCS on type) | 509 to 597 MB |
| At the end of the walk (Inspect Project, `composer require`, Settings, PHPUnit) | 660 to 744 MB |
| Problems after opening four files | 6 |
| Problems after Inspect Project (PHPStan, PHPCS, PHPMD; after the fix below) | 10: 5 errors, 5 PHPMD warnings |
| Psalm | Detected after `composer require --dev vimeo/psalm phpmd/phpmd` in the running session (Settings > Analysis lists four analyzers); with no `psalm.xml` it produced no rows and no message. |
| Pint | Present in Laravel's `require-dev`; Reformat Code with `[php] formatter = "pint"` rewrote an unsaved messy line in 0.4 s. |
| phpcbf | "Fix with phpcbf" on a real finding offers the three PHPCS sniffs of the line; it did nothing until the fix below, then rewrote `$walk=1;` to `$walk = 1;` as one edit. |
| PHPUnit | The gutter run of `Tests\Unit\ExampleTest::test_that_true_is_true` passes in the Tests dock (1 test, 1 assertion). |

Defects found and fixed, one commit each:
- "Fix with phpcbf" and Reformat Code through a tool formatter read the Rust rope, which keystrokes bypass (ADR-0003), so on an unsaved buffer the tool saw stale text, changed nothing and said nothing. The view now forwards the live text first (`EditorTabs::syncLiveText`); the new `php_real` flow types an unsaved finding and fixes it.
- Inspect Project appended the project root to `phpstan analyse` and to `phpcs`, which replaces the config's `paths` and `<file>` elements, so PHPStan analysed `vendor/`: 2500+ findings, the app at 100% CPU for minutes with the search popup not opening, and "Analysis: running..." that never ended. An analyzer may now list `project-paths-config` files, and a project run passes no path when the first that exists names paths.
- The status bar's "Analysis: N detected" was computed once at project open and said 2 after `composer require` of two more analyzers; it is refreshed when a project run ends.

**Fixed after the walk (2026-10-03), one commit each:**
- `401a94e` The modal "modified outside the editor" prompt after a slow save in `run` mode was the watcher's echo of our own write, handled 6.3 s after it: the Qt thread was busy with program lookups and the 1.5 s suppression window had expired.
  The session now also remembers a digest of what it wrote, so a file that still holds exactly that never raises the prompt, however late its event arrives; a burst of events for one tab asks once.
- `5d96286` Program lookups run off the Qt thread, once per (analyzer, host) until the launch cache invalidates them, and a container answers a whole candidate list in one probe (`resolve_first`).
- `0789a70` Format-on-save runs on a worker (`app_core::pending_save`); the answer is applied as one undo step only to the text it was computed from, otherwise the buffer is saved as it is with "Saved without formatting: the file changed while it was being formatted".
  Close, quit and Save All do not wait for a formatter.
- `27b2f88` The Problems dock read one row rect per visible row between visibility changes, a full tree layout each: 92.3 s for 4999 rows.
  Rects are read once visibility is settled and only for E2E marks; refreshes are coalesced.
- `fa0a06e` A PHP analyzer's project run with no config that names paths covers the Composer autoload paths (PSR-4, PSR-0, classmap; never `vendor/`) instead of the root.
- `ce038ab` Psalm without a `psalm.xml` is not run and says "Psalm needs a psalm.xml — run `vendor/bin/psalm --init`" in Settings > Analysis and the status bar's tooltip.
- `4d2643b` Wording seen on screen: "1 without a config file", and the unsaved-changes prompt names the file without the tab's modified dot.

| Measurement, after the fixes | local | exec | run |
|---|---|---|---|
| Ctrl+S with format-on-save: time until the UI answers | — | 1 ms (was 1.49 s) | 2 ms (was 8.2 to 8.7 s) |
| Ctrl+S to the formatted file on disk | — | 0.54 s | 2.0 to 2.1 s |
| On-type, first squiggle refresh after the keystroke (3 samples) | — | 0.59 to 0.63 s | 1.36 to 1.42 s (was none) |
| On-type, second refresh | — | 1.40 to 1.43 s | 1.40 to 1.43 s (was none) |
| Containers started in the first 30 s (`docker events`) | — | — | 13 (was 22 in 20 s, then the prompt) |
| "Modified outside the editor" prompt | — | none | none (was every `run` walk) |

- The 13 containers are one combined lookup each for PHPStan, PHPCS, Psalm, PHPMD, `php`, Phpactor and php-cs-fixer, then one per tool run; later saves and keystrokes start only the tool runs.
- A synthetic publish of 4999 findings for one file (the stub analyzer): rows in the dock 1.0 s after the save (was 96 s), worst UI answer 0.43 s (was 95.7 s).
- `make test-php` (6 flows) and `make test-php-container` (1 flow) pass after the fixes.
- Laravel (row 16) again: four analyzers detected after `composer require`, Psalm reported as needing a `psalm.xml`, Inspect Project gives the same 10 problems (PHPStan, PHPCS, PHPMD), Pint and phpcbf as before.

**Not fixed, for issues:**
- Typing in a 5000-line file takes about 0.23 s per keystroke in the debug build before the UI answers again, and about 0.45 s with 5000 squiggles in it (the synthetic publish above); not profiled further.
- Starting a test run still looks the framework's program up on the Qt thread (`TestService::start`); with the combined probe and its memo that is one container on the first run only.
- The Windows half of row 2 and the WSL half of row 6 are not checked (see above).

Batch 3 settings fixes: opening a project or confirming the Settings dialog no longer rewrites `.ide/settings.toml`.
Detected run configurations are shown and never saved, an update that changes nothing writes nothing, and unset editing fields are not serialised.
Checkboxes carry a tick and a visible border in both themes, and the PHP page collapses its empty probe line.

Fixes made while walking: the listed `fix(...)` commits after `d442627`, one per finding, each with its regression test where the rule lives or a real-toolchain flow assertion.

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
