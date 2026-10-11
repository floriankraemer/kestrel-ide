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
`app_config::php::PhpUnitRunSetting` holds `scope`, `path`, `class`, `method`, `data_set`, `filter`, `testsuite`, `configuration`, `groups`, `exclude_groups`, `runner_options`, `interpreter` and `working_directory`.
The environment is the configuration's own `env`.
- `scope` is a string, like `kind` (ADR-0039): `directory`, `file`, `class`, `method`, `data_set` or `config`.
  Blank and unknown read as `config`, so a newer file loads in an older build.
- `path`, `configuration`, `working_directory` and `runner_options` may use macros.
  A path is stored project-relative with `/`; an absolute path inside the project is made relative, one outside it is refused.
  The run's working directory is `working_directory` (PhpStorm's "Custom working directory"), the project root when it is blank.
  Paths are passed relative to the working directory (`../b/tests` from `packages/a`), so WSL and container hosts, which map the project root, see the right path.
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
Run All stays ad hoc.

**A Tests dock rerun runs the configuration that produced its tests (G2).**
As in PhpStorm, Rerun Test, Rerun Suite and Run Failed keep the working directory, configuration file, groups, runner options, interpreter and environment of the run that reported the tests, narrowed by the rerun's filter.
`test_core::TestTree` stamps every node with the run that last reported it, and `TestService` keeps the configuration of each run still in the tree.
`php_core::phpunit_run::rerun_config` picks the configuration: the one run's own, or, when the tests came from several runs that share all of the above, that context over the configuration file's whole scope.
Tests from runs in different contexts are refused with a sentence, since one process has one working directory.
`test_core::scope::rerun_args` keeps the configuration's test suite, options and path and puts the rerun's filter in place of the scope's own.
A test reported by Run All reruns ad hoc, as before.

**A run is prepared off the Qt thread.**
`TestService::start` reads only what needs the Qt thread (the tree, the runs that filled it), then a worker finds the editor run's monorepo package, the stored configuration, the settings and the framework's program, which under WSL or in a container is a process start.
The run then starts on the Qt thread; a refusal found on the worker arrives as `testRunRefused`, shown in the Tests dock and the status bar.

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

**A monorepo package runs in the package (#424).**
`php_core::tree_run::TestPackage::containing` finds the package of a directory: its nearest ancestor below the project root with a test framework (a PHPUnit configuration, or `phpunit/phpunit` or `pestphp/pest` in its `composer.json`), else the project.
A package that only autoloads (a `composer.json` without either) runs with the project's suite, as PhpStorm runs it.
`TestPackage::scope` gives a tree run, a gutter run, a caret run and a data-set rerun the package as `working_directory` and its PHPUnit configuration as `configuration`; the project's own package leaves the configuration unchanged.
The working directory is part of a temporary configuration's id only when it is set, so the ids earlier configurations were stored under hold.
`TestService` then treats the working directory as the run's root:
- the framework program is the package's (`vendor/bin/phpunit` there), else the project's, and the toolchains are detected in both;
- `test_core::run_on_env` starts in it, and its failure links and diagnostics resolve against it;
- each relative test location is made absolute against it as the event arrives (`TestLocation::anchored`), so a later run in another directory never re-points the nodes an earlier one left in the tree;
- a Pest class is located through the package's PSR-4 roots, then the project's (`php_core::tests::EventLocator`), so a package with only a `phpunit.xml`, or with its test autoload in the root `composer.json`, keeps its locations;
- a container engine still starts in the project root, where its relative compose files are; only `-w` carries the working directory into the container;
- a program probe on WSL or in a container is memoised by the candidate's absolute path, so the package's `vendor/bin/phpunit` and the project's never answer for each other;
- the coverage report is still written to `.ide/local/coverage/clover.xml` below the project root, named relative to the working directory (`test_core::coverage::args_in`).
The editor page shows the field as "Custom working directory", and without an explicit configuration file its test suite and group pickers read the working directory's (`phpunit_xml::model_in`); a package without one runs without one, as PHPUnit does.

**Go to Test lists the convention's tests and every covering test (#426).**
`php_core::test_navigation::CoversIndex` holds the project's `*Test.php` files below the test directories (every `autoload-dev` root, else `tests/`) with the classes each says it covers (`#[CoversClass]`, `#[CoversMethod]`, `@covers`, Pest's `covers()`), read within the PHPUnit model's budget (2000 files, 20000 entries, 512 KiB a file) and parsed only when a file mentions covering at all.
`CoversCache` keeps one per project on `TestService`: built on a thread of its own on the first Go to Test, rebuilt when the PSR-4 roots change or more than 1000 changes pile up, and refreshed for each PHP file or directory the project watcher reports (re-read, dropped, or a new directory scanned for the files it does not hold yet).
Roots that change while the first build runs mark it stale, so the old roots' index is never published, and the queued changes are kept.
A search waiting for one project's index gives up when another project is opened rather than answering with that project's index.
A refresh never grows the index past the scan budget's 2000 files, a renamed or deleted ancestor of a test directory takes its tests along, and a file deleted before its report arrives is not listed.
The watcher's report only queues the path, so the Qt thread never waits on a `\\wsl.localhost` share; the next search applies the queue on its worker thread.
A search answers the `<Name>Test` convention's matches, then the tests that cover the class, then (only when the convention found none) the test files named after it.
While the index is still being built a convention match answers at once with a status-bar note that the covering tests are still being indexed; with no convention match the search waits for the index, since it has nothing to show meanwhile.
`index-core` is not used for this: its text index is language-agnostic, and a covered name only means a class once the test's namespace and imports resolve it, so answering from it would still re-read and parse every candidate file on each request.

## Consequences

- Every way of running tests from the editor now has one code path and one persisted shape, which E5 (suites and groups from `phpunit.xml`) and E6 (project tree entries) add scopes to without a new launch path.
- A framework that cannot express a scope fails with a sentence instead of running the wrong tests.
- A group nobody declared in a scanned file can still be typed into the group picker; a suite name can be typed into the combo.
