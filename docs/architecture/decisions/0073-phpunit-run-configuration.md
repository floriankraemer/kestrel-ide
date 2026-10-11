# 0073. PHPUnit run configuration

## Status

Accepted.
Delivered by task E4 of the [PHPUnit editor runs plan](../phpunit-editor-runs-plan.md).
Builds on [ADR-0048](0048-test-runner.md) (the Tests dock), [ADR-0056](0056-container-run-configurations.md) (kind-tagged run configurations) and [ADR-0068](0068-php-core-crate.md).

## Context

A test run was always ad hoc: Run All, a gutter marker or a Tests dock rerun.
It could not be saved, edited, picked from the configuration list or rerun from the toolbar.
PhpStorm has a PHPUnit run configuration with a test scope and options, and its context runs create temporary configurations of that kind.

## Decision

**`kind = "phpunit"` with a `[run_configs.phpunit]` table.**
`app_config::php::PhpUnitRunSetting` holds `scope`, `path`, `class`, `method`, `data_set`, `filter`, `testsuite`, `configuration`, `groups`, `exclude_groups`, `runner_options` and `interpreter`.
The environment is the configuration's own `env`.
- `scope` is a string, like `kind` (ADR-0039): `directory`, `file`, `class`, `method`, `data_set` or `config`.
  Blank and unknown read as `config`, so a newer file loads in an older build.
- `path`, `configuration` and `runner_options` may use macros.
  A path is stored project-relative with `/`; an absolute path inside the project is made relative, one outside it is refused.
  The run's working directory is the project root on every host, so WSL and container hosts see the right path.
- `interpreter` replaces `[php].interpreter` for this configuration.
  The host (local, WSL, container) is still `[php]`'s, so it is not a second target mechanism.
- `filter` narrows a `file` scope by a PCRE.
  A Pest `it()` has no class to name, so its configuration is the file plus the marker's filter.
- `data_set` is the text after `with data set `: `"name"` with its quotes, or `#0`.

**The rules live in Qt-free crates.**
- `php_core::phpunit_run` validates the table (a file scope needs a path, a method scope a class and a method), resolves it into a `test_core::scope::TestScope` plus `ScopeOptions`, splits the runner options like a shell line, and names and identifies the temporary configurations editor runs create.
  `run-core` cannot depend on `php-core` (ADR-0039), so it only learns the kind: `validate::problem` and `to_launch_spec_in` treat it as a kind that is not a process, and the editor asks `php-core` for the real check.
- `test_core::scope::scope_args` is the scope to argv table, below.
  It is dialect aware: PHPUnit and Pest share one dialect, Codeception selects by `file` or `file:^method$` and has no flag for groups or a configuration file, and every other framework refuses with a typed `ScopeError`.

| Scope | Arguments after the framework's own (`--teamcity`) |
|---|---|
| Directory | `<dir>` |
| File | `<file>`, or `--filter <filter> <file>` |
| Class | `--filter '^Ns\\Class::' [<file>]` |
| Method | `--filter '^Ns\\Class::method( with data set .+)?$' [<file>]` |
| Data set | `--filter '^Ns\\Class::method with data set "name"$' [<file>]` |
| Configuration file | nothing, or `--testsuite <name>` |
| Any scope | `--configuration <file>`, `--group a,b`, `--exclude-group c`, the runner options, then the path last |

**`TestService` launches it, never the PTY supervisor.**
`RunService::launch` recognises the kind, runs the configuration's own before-launch tasks as for any kind, and then emits `testConfigurationRequested`, as it already special-cases `sql-script`; `DebugService::debug` emits `testConfigurationDebugRequested`; the view connects both to `TestService`.
Unless the configuration allows parallel runs, a run in progress is stopped and the new one starts once it has finished; the Tests dock shows one run, so one that allows them is refused while another runs.
The Run menu gains `run.runWithCoverage`.
So the toolbar, the configuration list, Rerun (`run.rerun`) and Debug fill the Tests dock and use the framework selection, interpreter host, filters, coverage and the Xdebug staging that Run All already has.
Run All and the Tests dock's reruns stay ad hoc.

**Editor runs create temporary configurations.**
A caret run, a gutter run and a data-set run resolve to a temporary `phpunit` configuration when the run starts, persisted with `remember_temporary`.
Its id derives from the scope, so repeated runs reuse one entry and the cap evicts the oldest.
Names follow PhpStorm: `GreeterTest`, `GreeterTest::testGreets`, `tests in 'Unit'`, `Unit (phpunit.xml)`.
Editing a temporary configuration in the dialog saves it, as for every kind, and the next editor run of the same test runs the edited one: the stored configuration with the derived id wins over the freshly derived one.

**A `phpunit` configuration as a before-launch task** runs on `TestService`, so its results fill the Tests dock and its Stop cancels it, and a non-zero exit fails the launch.
Before-launch tasks run on a thread of their own, never on the worker that owns the PTY supervisor, so a long suite freezes no console's output; that thread hands the run to `TestService` on the Qt thread and waits on a channel for its end.

**The configuration editor offers what `phpunit.xml` declares (E5).**
`php_core::phpunit_xml::parse` reads the test suites (`<directory suffix>`, `<file>`, `<exclude>`), `<groups>` include/exclude and `bootstrap` of a PHPUnit 9 to 12 configuration over `quick-xml`.
`discover` finds the file the way PHPUnit does (`phpunit.xml`, `phpunit.dist.xml`, `phpunit.xml.dist` from `app-config::php`'s single list, or the configuration field's path) and `model` joins it with a bounded scan of the suites' test files for the groups PHPUnit knows: `#[Group]`, `#[Ticket]` and the `#[Small]`/`#[Medium]`/`#[Large]` sizes, and below PHPUnit 12 (by `composer.lock`) their `@group`, `@ticket`, `@author` and size annotations.
The scan shares one budget across suites (2000 test files, 20000 directory entries, 512 KiB a file), walks breadth first in name order, never enters `vendor`, `node_modules` or `.git` below a suite's own directory, does not follow symlinked directories, and reads a file two suites share once.
`RunConfigEditor::requestPhpUnitModel` runs that on a worker thread and answers through `phpUnitModelReady`; the page shows the test suite as an editable combo and the groups as checkable lists, and asks again when the configuration file field changes.
A glob in a suite's `<directory>` or `<file>` is expanded as PHPUnit does (`*` and `?` within a segment, `**` across segments).
The "defined in the configuration file" scope with a suite is `--testsuite <name>`, unit-tested end to end through `plan` and `scope_args`.

**An editor run selects its temporary configuration (#415).**
`TestService::configurationRemembered(configId)` makes the view refresh the configuration list and select that entry in the toolbar combo, so the combo, Rerun and `run.rerun` name what just ran.

## Consequences

- Every way of running tests from the editor now has one code path and one persisted shape, which E5 (suites and groups from `phpunit.xml`) and E6 (project tree entries) add scopes to without a new launch path.
- A framework that cannot express a scope fails with a sentence instead of running the wrong tests.
- A group nobody declared in a scanned file can still be typed into the group picker; a suite name can be typed into the combo.
