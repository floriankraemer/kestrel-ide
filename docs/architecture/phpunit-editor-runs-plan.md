# PHPUnit editor runs plan

PhpStorm parity for running PHP tests from where the code is: the editor, the gutter, the project tree and saved run configurations.
The PHP parity plan (`php-parity-plan.md`, phase T) delivered the test runner, the Tests dock, gutter Run / Debug / Run with Coverage markers on test classes and methods, and coverage.
This plan closes the gaps between that and PhpStorm.

## Gaps this plan closes

- Run File (`Ctrl+Shift+F10`) ignores the caret: in a test file it runs `php <file>`, which declares the class and exits with no test results (issue #403).
- Gutter markers are always the same green triangle: they never show whether a test last passed or failed.
- Test runs are ad hoc: there is no PHPUnit run configuration, so a test run cannot be saved, edited, rerun from the toolbar or picked from the configuration list.
- Only class and method scopes exist: no file, directory, test suite or group scope, and `phpunit.xml` is never parsed.
- The project tree has no Run / Debug / Run with Coverage entries for a test directory or file.
- Test tree nodes carry no source location, so a passed test cannot be opened from the Tests dock and the gutter cannot be matched to a result by location.
- There is no Go to Test / Go to Test Subject.

## PhpStorm behaviour this plan matches

- Context run: `Ctrl+Shift+F10` (Run) and `Ctrl+Shift+F9` (Debug) in a test file run the method under the caret, else the class, else every test class in the file.
- A context run creates a temporary run configuration that shows in the configuration list, can be saved, and is pushed out by newer ones (IntelliJ keeps five).
- A PHPUnit run configuration has a test scope — directory, file, class, method, or "defined in the configuration file" with an optional test suite — plus an alternative configuration file, include/exclude groups, test runner options, environment variables, the interpreter and its options.
- Running a PHPUnit configuration fills the Tests dock, exactly like Run All; Run / Debug / Run with Coverage apply to every scope.
- Gutter icons on a test class and method reflect the last run: passed, failed (or errored), ignored, or never run; the tooltip of a failed one shows the failure message.
  As PhpStorm restores them from its test history, they survive a restart: `php_core::test_history` keeps each test's last result (status, short message, duration, file and symbol) in `.ide/local/test-history.toml`, at most 2000 of them, loaded on the gutter's first paint of the project; a live run's result for a marker supersedes the recorded one, and a recorded test that no longer exists has no marker to show on.
- The project tree offers Run / Debug / Run with Coverage "Tests in '<dir>'" on a directory and "'<file>'" on a test file.
- Double-clicking a test in the Tests dock navigates to its source, using PHPUnit's TeamCity `locationHint` (`php_qn://<file>::\Class::method`), Pest's `pest_qn://` alike.
- A single data set is rerun from the Tests dock (already supported) and from the gutter menu of a method that has data sets after a run.
- `Ctrl+Shift+T` navigates from a class to its test and back, by PSR-4 `autoload` / `autoload-dev` roots and the `<Name>Test` convention, and offers a chooser when there are several.

Out of scope here, each tracked by its own issue: Create New PHP Test dialog (#404), the Tests dock's auto-test toggle (#405), and run configuration templates (#406).

## Design

Rules stay in Qt-free crates; `ui-shell` translates; `cpp/` stays a humble view (CLAUDE.md, ADR-0003).

- `php-core::tests` owns everything PHP-specific: the marker at a caret, the file scope, `phpunit.xml` parsing (test suites, groups, the configured directories) and the test ⇄ subject mapping.
  `run-core` must not depend on `php-core` (ADR-0039), so `run-core` only learns a new configuration kind, not PHP.
- A PHPUnit run configuration is a `RunConfigSetting` with `kind = "phpunit"` and a `phpunit` table holding the scope and options, persisted in `.ide/settings.toml` beside the existing kinds.
  Launching one never goes through the PTY supervisor: `RunService` hands it to `TestService`, which already owns framework selection, the interpreter host (local, WSL, container), filters, coverage and the Tests dock.
  The kind names the scope, not the framework: the same table drives Pest when Pest is the selected framework, as PhpStorm's Pest configuration does.
- `test-core` parses `locationHint` into a node location (file + optional line), maps it through `ExecHost` like failure links, and keeps a status lookup by location and by `TestId` that the gutter reads.
- ADR-0073 records the configuration kind, why it is launched by `TestService` rather than the supervisor, and the scope → argv table.

## Scope → argv (PHPUnit; Pest uses its own dialect)

| Scope | Arguments after `--teamcity` |
|---|---|
| Directory | `<dir>` |
| File | `<file>` |
| Class | `--filter '^Ns\\Class::' <file>` |
| Method | `--filter '^Ns\\Class::method( with data set .+)?$' <file>` |
| Data set | `--filter '^Ns\\Class::method with data set "name"$' <file>` |
| Configuration file | nothing, or `--testsuite <name>` |
| Any scope | `--configuration <file>` when set, `--group a,b`, `--exclude-group c`, then the test runner options |

## Delivery

Sequential tasks, one PR each, in this order (each builds on the previous).

| Task | What |
|---|---|
| E1 | Caret-aware context run: `run.runContext` and a new `run.debugContext` (`Ctrl+Shift+F9`) pass the caret line; in a PHP test file they run the method under the caret, else the class, else the file, through `TestService`; non-test files keep `php <file>`. |
| E2 | Test node locations: parse `locationHint` (`php_qn://`, `pest_qn://`), map through `ExecHost`, double-click a Tests dock node opens its source. |
| E3 | Gutter test status: marker icons show passed / failed / ignored / not run from the last run, a failed marker's tooltip shows the failure, repaint on `testTreeChanged`, and the gutter menu offers single data sets after a run. |
| E4 | PHPUnit run configuration kind (ADR-0073): `kind = "phpunit"` with every scope and option, launched through `TestService`; E1's context runs become temporary configurations; Run / Debug / Coverage and toolbar rerun work for it. |
| E5 | `phpunit.xml` model: test suites, groups and directories parsed in `php-core`; the run configuration editor offers test suite and group pickers. |
| E6 | Project tree entries: Run / Debug / Run with Coverage "Tests in '<dir>'" and on a test file, creating temporary directory / file configurations. |
| E7 | Go to Test / Go to Test Subject (`Ctrl+Shift+T`) with a chooser when there are several. |
| E8 | Acceptance: testing-expert and product-owner decide the E2E coverage; run on Linux and on the real Windows build against a WSL project. |

## Progress

Every row starts as `open`.
A row's status and commit hash are updated in the commit that finishes it.

| Task | Status | Commit |
|---|---|---|
| E1 — caret-aware Run / Debug File for tests | done | bef8ab0 |
| E2 — test node locations | done | a59db68 |
| E3 — gutter test status | done | f4c42b8 |
| E4 — PHPUnit run configuration kind | done | 9ffe63b |
| E5 — `phpunit.xml` suites and groups | done | ad567a8 |
| E6 — project tree test entries | done | 0d7e478 |
| E7 — Go to Test / Test Subject | done | 4830654, 2aa85fe |
| E8 — acceptance (Linux + Windows/WSL) | open | |
