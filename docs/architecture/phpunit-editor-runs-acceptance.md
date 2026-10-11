# PHPUnit editor runs — acceptance specification (E8)

Written by the product owner from the [plan](phpunit-editor-runs-plan.md), [ADR-0073](decisions/0073-phpunit-run-configuration.md), [ADR-0074](decisions/0074-keyboard-shortcut-contexts.md), PRs #408–#427 and issues #404–#428.
PhpStorm reference behaviour from product knowledge.

## 1. Environments

| Env | Where | Project |
|---|---|---|
| L (Linux) | `linux-php` image, Xvfb harness (`crates/app/tests/e2e/php_real.rs` style, `IDE_E2E_PHP=1`) | new fixture `php_suites` beside `php_app` (§2) |
| W (Windows + WSL) | real Windows build, guided pass with screenshots | `\\wsl.localhost\Ubuntu\home\florian\projects\symfony-aop` (Symfony, PHPUnit 12, `phpunit.dist.xml`, suites core-unit / bundle-integration / app-e2e, 641 tests) |

Every journey runs on L; journeys tagged W must also pass on W.
Must blocks E8 sign-off; Should is verified and a failure becomes an issue.
Pixel bar: on W at 100 % and 150 % scaling, light and dark theme; gutter icons crisp, centred, aligned with breakpoint/folding markers; no truncated popup text or stray scrollbars.
Visual defects are fixed or filed.

## 2. Fixture `php_suites` (L)

1. `phpunit.xml` with suites `unit` → `tests/Unit`, `feature` → `tests/Feature`, and `<groups><exclude>slow</exclude></groups>`.
2. A test class with a passing, a failing (`assertSame` mismatch), a skipped (`markTestSkipped`) and an erroring method.
3. A `#[DataProvider]` method with named sets including `"a/b"` and `"x@y"`, and one with unnamed sets (`#0`, `#1`).
4. Methods with `#[Group('slow')]` and `#[Group('db')]`.
5. `src/Mailer.php`, `tests/Unit/MailerTest.php` (convention), `tests/Unit/NotifierTest.php` with `#[CoversClass(Mailer::class)]`.
6. `src/Orphan.php` with no test.
7. Monorepo package `packages/billing/` with its own `composer.json` (phpunit in require-dev), `phpunit.xml`, `vendor/bin/phpunit`, `src/`, `tests/`.
8. Package `packages/util/` whose `composer.json` only autoloads.
9. A Pest file with nested `describe()` and an `it()` with a dataset.
10. A non-test script `bin/console.php`.

## 3. Journeys

### J1 Run File / Debug File at caret — Must — W
- AC1.1 Caret in `testFails`, Ctrl+Shift+F10 → only that method runs; dock shows one failed test; combo shows `<Class>::testFails`.
- AC1.2 Caret on class line or between methods → whole class; combo `<Class>`.
- AC1.3 Caret above the class (use statements) → whole file (`--teamcity <file>`).
- AC1.4 Non-test PHP file → `php <file>` in the Run console, not the Tests dock.
- AC1.5 Breakpoint in `testPasses`, Debug File (Run menu) → Xdebug stops; resume finishes with the result in the dock.
- AC1.6 Debug File in a non-test file → status message, nothing hangs (#407 tracks script debugging).
- AC1.7 Pest: caret in an `it()` in nested `describe()` → exactly that `it()`.
- AC1.8 (W) symfony-aop core-unit test → runs through wsl.exe; time to first result within 2 s of the same run in a WSL shell.

### J2 Gutter run/debug/coverage, status, tooltips, data sets — Must — W
- AC2.1 Never-run test file → every marker shows the run triangle.
- AC2.2 After pass/fail/skip/error → passed/failed/ignored/failed; class marker failed.
- AC2.3 Hover a failed marker → tooltip with the first failure lines (expected/actual), anchored to the icon.
- AC2.4 Passed → duration; skipped → skip reason.
- AC2.5 Running state shown; repaint within 200 ms of a method finishing.
- AC2.6 Method moved 10 lines by an edit after a run → its icon moves with it.
- AC2.7 Data-provider marker menu lists each set (`"a/b"`, `"x@y"`, `#0` …, cap 15); choosing `"x@y"` runs only that set.
- AC2.8 Debug a data set → Xdebug stops in the method for that set.
- AC2.9 Run with Coverage from the marker → subject shows covered/uncovered lines.
- AC2.10 Pest `it()` and `describe()` markers behave as AC2.1–2.7; a dataset call runs all its rows, and no single-row entry is offered because Pest's `--filter` cannot select one row (#451).
- AC2.11 Pixel: glyphs legible in dark and light themes, same size/baseline as the run triangle (theme-aware colours, gap G8).
- AC2.12 (W) Full symfony-aop run (641 tests): typing never freezes > 100 ms; markers correct at the end.

### J3 Tests dock jump to source, F4, failure links — Must — W
- AC3.1 Double-click a passed method row → file opens with caret on the method.
- AC3.2 F4 on a class row → class declaration.
- AC3.3 Enter on a data-set row → the method.
- AC3.4 Context menu "Jump to Source" present; disabled only when the file no longer exists.
- AC3.5 Unsaved edits moved the method → caret lands on it in the buffer.
- AC3.6 Failure link in the details pane → assertion line.
- AC3.7 Pest `pest_qn://` row → the `it()` call, nested `describe()` included.
- AC3.8 (W) locationHint `/home/florian/...` → opens `\\wsl.localhost\Ubuntu\...`, reusing an open tab (no duplicate from a different spelling).

### J4 PHPUnit run configuration: scopes and options — Must (scopes), Should (option combos) — W
- AC4.1 Add > PHP > PHPUnit page: scope selector; switching scope shows only that scope's fields.
- AC4.2 Each scope (directory, file, class, method, data set, configuration file) runs exactly the expected tests; argv preview matches the ADR-0073 table.
- AC4.3 Configuration-file scope + suite `unit` → only `tests/Unit`.
- AC4.4 Suite combo lists phpunit.xml suites and accepts typed names.
- AC4.5 Groups list offers `slow`, `db`; checking `db` runs only db tests; excluding `slow` removes them.
- AC4.6 Alternative configuration file → pickers reload from it.
- AC4.7 Runner options `--stop-on-failure` and env `APP_ENV=test` both honoured.
- AC4.8 Picker path outside the project → refused with a message; inside → stored project-relative with `/`.
- AC4.9 Missing required field → editor shows the problem; Run refuses with that sentence.
- AC4.10 Toolbar Run / Debug / Run with Coverage on a saved configuration all work.
- AC4.11 Enter in the group picker does not close the dialog.
- AC4.12 (W) symfony-aop suite combo lists core-unit, bundle-integration, app-e2e within 2 s, no freeze; `--testsuite core-unit` runs only that suite.
- AC4.13 (W) Runner-option macro expanding to a Windows path with spaces stays one argument.
- AC4.14 Pixel: label column aligned, picker buttons same height as line edits, group lists scroll inside their frame, argv preview wraps.

### J5 Toolbar selection, rerun, temporaries — Must — W
- AC5.1 Gutter run of `testPasses` → combo selects `<Class>::testPasses`; Ctrl+F5 (rerun) reruns exactly it.
- AC5.2 Six editor runs → five newest temporaries kept; permanent configurations untouched.
- AC5.3 Same method twice → one entry.
- AC5.4 Edit and save a temporary (add `--stop-on-failure`) → next gutter run of that method uses the edited one.
- AC5.5 Run in progress, configuration without allow-parallel → first stops, second runs after; dock shows the second.
- AC5.6 With allow-parallel and a run in progress → refused with a message.
- AC5.7 Stop (Ctrl+F2) → PHP process ends (W: Linux-side process gone, checked with ps in WSL); unfinished tests marked not finished.

### J6 Before-launch tasks — Should — W
- AC6.1 Shell before-launch task runs first, for Run and Debug.
- AC6.2 Task exits non-zero → tests do not run, failure reported.
- AC6.3 Script configuration with a PHPUnit before-launch task that fails → script does not run; dock shows the failure; dock Stop cancels it.
- AC6.4 Long before-launch suite → other consoles keep printing.
- AC6.5 Launch while the same configuration's before-launch runs → refused.

### J7 Project tree entries — Must — W
- AC7.1 Right-click `tests/Unit` → Run/Debug/Coverage "Tests in 'Unit'"; Run runs everything below it.
- AC7.2 Right-click `MailerTest.php` → entries name `MailerTest`; Run runs that file.
- AC7.3 Empty area / project root → "All tests", runs the configured suites.
- AC7.4 `src/Mailer.php`, `vendor/`, or a project without PHPUnit/Pest → no test entries.
- AC7.5 Tree run → temporary configuration selected in the combo.
- AC7.6 (W) symfony-aop right-click on `tests/` or a test file → menu within 300 ms (entries may be absent on a cold slow share); second right-click shows them.

### J8 Monorepo packages — Must — L only (no WSL monorepo)
- AC8.1 Right-click `packages/billing/tests` → `packages/billing/vendor/bin/phpunit`, cwd `packages/billing`, its `--configuration`; tests pass.
- AC8.2 Gutter, caret and data-set runs in a billing test run in the package.
- AC8.3 Failure link and jump to source resolve relative to the package.
- AC8.4 Coverage in the package → `.ide/local/coverage/clover.xml` under the project root; package subject coloured.
- AC8.5 `packages/util` tests run with the project suite from the project root.
- AC8.6 Package configuration page shows Custom working directory filled; pickers read the package phpunit.xml.
- AC8.7 Dock Rerun Test / Run Failed after a package run keep the package cwd and configuration (gap G2 — implemented in E8).

### J9 Go to Test / Test Subject — Must — W
- AC9.1 `src/Mailer.php`, Ctrl+Shift+T → chooser: `MailerTest` first, `NotifierTest` (covering) after; Enter opens.
- AC9.2 Exactly one test → opens directly.
- AC9.3 `MailerTest.php` → `src/Mailer.php`.
- AC9.4 `NotifierTest.php` → `Mailer` (+ `Notifier` if it exists) in a chooser.
- AC9.5 `src/Orphan.php` → status message, nothing else.
- AC9.6 Navigate menu reads "Test" in a subject, "Test Subject" in a test; editor context menu Go To > Test.
- AC9.7 New covering test created outside the IDE → listed on the next Go to Test (watcher refresh).
- AC9.8 First Go to Test → convention answer at once, "Still indexing…" note.
- AC9.9 Two rapid presses in different files → only the latest answer; tab switch drops a pending answer.
- AC9.10 Pixel: chooser matches the Go to Declaration popup; `\\wsl.localhost` paths not truncated; keyboard navigable.
- AC9.11 (W) symfony-aop: convention answer within 1 s; covering list within 10 s cold; no UI freeze.
- AC9.12 (W) Test file created from a WSL shell → AC9.7 holds over `\\wsl.localhost` (file an issue if the watcher gets no events).
- AC9.13 (Should) Laravel flat `tests/Feature/MailerTest.php` found by file name.

### J10 Shortcut contexts — Must — W
- AC10.1 Editor focus: Ctrl+Shift+T = Go to Test; terminal focus: new terminal tab; neither fires the other; no Qt ambiguity warning on stderr.
- AC10.2 Tests tree: F4 jumps to source; result grid on a foreign-key cell: F4 goes to the referenced row.
- AC10.3 Project tree or Run console focus: Ctrl+Shift+T does nothing; Navigate > Test still works.
- AC10.4 Keymap page: terminal.newSession → F4 gives no conflict warning; navigate.goToTest → Ctrl+Shift+F10 warns (Run File, Global).
- AC10.5 Old keymap (`tests.jumpToSource = "F4"`, `database.goToReferencedRow = ""`) → F4 restored once; a later explicit unbind sticks.
- AC10.6 (W) AC10.1 and AC10.2 hold on Windows.

### J11 Coverage — Should — W
- AC11.1 run.runWithCoverage on a saved configuration updates coverage view and editor colouring.
- AC11.2 No coverage driver → clear message, not an empty report.
- AC11.3 (W) Clover paths `/home/...` map to `\\wsl.localhost` files and colour them.

### J12 Errors and edges — Should
- AC12.1 Missing `vendor/bin/phpunit` → sentence naming the missing runner.
- AC12.2 Codeception with a group scope → typed ScopeError sentence.
- AC12.3 Test calling `exit()` → run ends, started test marked not finished.
- AC12.4 Invalid data provider (#412) → record what the user sees.
- AC12.5 New UI strings appear translated in German (tree entries, tooltip chrome, chooser, run config page, status notes).

## 4. Suggested L launch grouping
1. Context and gutter: J1 + J2 + AC5.1–5.3.
2. Dock navigation: J3 + AC10.1–10.3.
3. Configurations: J4 + AC5.4–5.6 + J6.
4. Tree and monorepo: J7 + J8.
5. Go to Test: J9.
AC10.4–10.5 are unit-tested in app-config already.

## 5. Gap triage (decided by the coordinator)
In scope for E8 (implement): G2 dock reruns keep the producing configuration (cwd, config file, groups, env, interpreter); G3 "Interpreter options" field (`php -d …`); G4 gutter status persistence across restarts (verify first; implement if missing — PhpStorm restores from test history); G8 theme-aware gutter status colours; G9 explicit "Save '<name>' Configuration" for a temporary from the combo; G11 Go to Test Subject from a test method lands on the tested method when resolvable (#[CoversMethod], name match); #428 package PSR-4 roots for Go to Test.
Open decision (user): G1 Debug File default key — PhpStorm's Ctrl+Shift+F9 is Rebuild here.
Filed as issues: G5 #429, G6 #430, G7 #431, G10 #432.

## 6. Automation on Linux

The L journeys run as `crates/app/tests/e2e/php_runs/` against the `php_suites` fixture (`crates/app/tests/fixtures/php_suites`), in the `linux-php` image through `make test-php`.
The fixture's three Composer locks (the root, `packages/billing`, `packages/pest`) are prewarmed in that image, as `php_app`'s is.
Pest lives in its own package, `packages/pest`, so the root project runs real PHPUnit rather than Pest's PHPUnit dialect.
`tests/Extra` holds the tests outside every suite: a 30-second test for the running state, Stop and parallel launches, and one that passes only with the configuration's environment.

| Flow | Journeys |
|---|---|
| `e2e_phpunit_run_file_at_the_caret` | J1, AC5.1–AC5.3 |
| `e2e_phpunit_gutter_status_data_sets_and_coverage` | J2, G4 |
| `e2e_phpunit_file_run_icon_runs_the_tests` | the file-level Run icon of a test file |
| `e2e_phpunit_tests_dock_navigation_and_shortcut_contexts` | J3, AC10.1–AC10.3 |
| `e2e_phpunit_saved_configurations` | J4, AC5.4–AC5.7, J6 |
| `e2e_phpunit_project_tree_and_monorepo_packages` | J7, J8 |
| `e2e_phpunit_go_to_test_and_subject` | J9, G11 |

The configurations flow also types into the PHPUnit page (AC4.6, AC4.11) and checks its label column (AC4.14) through the dialog's `run_config_form` marker; the tree flow reads a package configuration's page (AC8.6).
It also runs a `$FILE_DIR$` runner option (AC4.13, #458), a configuration whose interpreter options make php exit before the runner starts (#456), and, as its last step, saves the dialog and waits for the toolbar combo to refresh (#457).
The Go to Test flow opens the chooser's first entry with Enter alone, so the entry is preselected (AC9.10, #459).
The Tests dock marks a row only when it changes (#462), so a flow takes a test as run when its row was marked after the run started.
AC4.8 stays manual: its pickers open the platform's file dialog.
A Pest dataset row cannot run on its own (#451): AC2.10 checks that no single-row entry is offered and that the call runs every row.
A flow records each failed AC and keeps going, so one defect does not hide the ACs after it; it fails at its end, listing every failed AC.
Screenshots for the pixel ACs go to `target/shots` (or `$IDE_SHOT_DIR`) as a diagnostic, never an assertion (ADR-0024).

### Results

Linux (`make test-php`): all thirteen flows pass, the seven `php_runs` flows above and six `php_real` flows; the container interpreter flow runs only under `make test-php-container`.
Pest dataset rows (AC2.10) are not offered on their own (#451), which the gutter flow checks.

Windows/WSL: the pass ran the real Windows build against a `\\wsl.localhost` clone of a large Symfony project (514 tests in its `core-unit` suite).
Its first findings blocked everything else: the Windows build wrote no E2E markers (#453, fixed by #454), and php received the UNC path of `vendor/bin/phpunit`, so every run failed silently (#455, fixed by #464).
With both fixed, Run File reached its first result 0.73–0.79 s after the click (the WSL shell itself takes 0.14 s), and every W check of this spec ran.
The defects it found are fixed by the PR that closed this plan:
a run that never started read as "Run finished." and hid php's stderr (#456);
a configuration saved in the dialog reached the toolbar combo only on the next refresh (#457);
file macros stayed literal in runner options (#458);
the Go to Test chooser's title took the keyboard and nothing was preselected (#459);
the run configuration dialog's labels sat above their fields, its list rows clipped, it showed stray scrollbars and outgrew a 150 % work area (#460);
German strings were missing or inconsistent (#461);
the Tests dock rebuilt its whole tree on every event, 42 s for 514 tests with markers on (#462);
and the gutter's status discs were larger than the run triangle at 150 % (#463).
The command preview of a WSL project now shows the Linux paths that run.
