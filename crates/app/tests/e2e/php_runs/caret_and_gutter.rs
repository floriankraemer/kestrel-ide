//! J1 (Run File / Debug File at the caret, AC5.1–AC5.3) and J2 (gutter
//! status, tooltips, data sets, coverage, G4).

use super::*;

// --- J1 + AC5.1–5.3: Run File / Debug File at the caret, toolbar reruns --------

/// J1 (Run File / Debug File at the caret) and AC5.1–AC5.3 (the toolbar
/// selection, rerun and temporaries an editor run leaves).
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_phpunit_run_file_at_the_caret() {
    if !require_php_e2e() {
        return;
    }
    let mut ide = launch_suites("e2e_phpunit_run_file_at_the_caret", None);
    let outcomes = open(&ide, "OutcomesTest.php");

    // AC1.1: the caret in `testFails` runs only that method.
    caret_to(&ide, 19);
    let mark = ide.mark();
    ide.key("ctrl+shift+F10");
    let (_, tests) = run_and_collect(&ide, mark);
    assert_eq!(names(&tests), ["testFails"], "AC1.1: {tests:?}");
    assert_eq!(tests["testFails"], "failed", "AC1.1");
    wait_for_selected(&ide, mark, "OutcomesTest::testFails");

    // AC1.2: on the class line, the whole class.
    caret_to(&ide, 9);
    let mark = ide.mark();
    ide.key("ctrl+shift+F10");
    let (_, tests) = run_and_collect(&ide, mark);
    assert_eq!(
        names(&tests),
        ["testErrors", "testFails", "testPasses", "testSkipped"],
        "AC1.2"
    );
    wait_for_selected(&ide, mark, "OutcomesTest");

    // AC1.3: above the class, the whole file.
    caret_to(&ide, 7);
    let mark = ide.mark();
    ide.key("ctrl+shift+F10");
    let (_, tests) = run_and_collect(&ide, mark);
    assert_eq!(tests.len(), 4, "AC1.3: {tests:?}");
    wait_for_selected(&ide, mark, "OutcomesTest.php");

    // AC5.1: a gutter run selects its configuration; Rerun reruns it.
    let mark = ide.mark();
    gutter(&ide, outcomes, TEST_PASSES, "Run 'testPasses'");
    let (_, tests) = run_and_collect(&ide, mark);
    assert_eq!(names(&tests), ["testPasses"], "AC5.1");
    wait_for_selected(&ide, mark, "OutcomesTest::testPasses");
    let mark = ide.mark();
    ide.focus_main();
    ide.key("ctrl+F5");
    let (_, tests) = run_and_collect(&ide, mark);
    assert_eq!(names(&tests), ["testPasses"], "AC5.1: Rerun");

    // AC5.3: the same method twice is one entry.
    let configs = selected_config(&ide, Mark::start()).expect("a selection")["configs"].clone();
    let same: Vec<&Value> = configs
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] == "OutcomesTest::testPasses")
        .collect();
    assert_eq!(same.len(), 1, "AC5.3: {configs}");

    // AC1.4: a script runs with `php` in the Run console, not as a test.
    open(&ide, "console.php");
    let mark = ide.mark();
    ide.focus_main();
    ide.key("ctrl+shift+F10");
    ide.wait_for_event_within(mark, REAL_TOOLCHAIN_TIMEOUT, "the script's output", |e| {
        e["ev"] == "run_console_output"
            && e["text"]
                .as_str()
                .is_some_and(|t| t.contains("console ran"))
    });
    assert!(
        ide.events_since_of(mark, "test_run_started").is_empty(),
        "AC1.4: the script started a test run"
    );

    // AC1.6: Debug File in a script says why and starts nothing.
    let mark = ide.mark();
    click_run_menu_item(&ide, "Debug File");
    wait_for_status(&ide, mark, "no test in this file to debug");
    assert!(
        ide.events_since_of(mark, "debug_started").is_empty(),
        "AC1.6: a script debug session started"
    );

    // AC1.7: Pest, the caret in an `it()` inside nested `describe()`s.
    open(&ide, "NestedTest.php");
    caret_to(&ide, PEST_IT_ADDS as usize + 2);
    let mark = ide.mark();
    ide.key("ctrl+shift+F10");
    let (finished, tests) = run_and_collect(&ide, mark);
    assert_eq!(tests.len(), 1, "AC1.7: {tests:?} {finished}");
    assert!(tests.keys().all(|n| n.contains("adds")), "AC1.7: {tests:?}");

    // AC5.2: six and more editor runs keep the five newest temporaries.
    let configs = selected_config(&ide, Mark::start()).expect("a selection")["configs"].clone();
    let temporaries = configs
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["temporary"] == true)
        .count();
    assert_eq!(temporaries, 5, "AC5.2: {configs}");

    // AC1.5: Debug File with a breakpoint in `testPasses` stops there.
    reopen(&ide, "OutcomesTest.php");
    toggle_breakpoint_at(&ide, 13);
    caret_to(&ide, 13);
    let mark = ide.mark();
    click_run_menu_item(&ide, "Debug File");
    wait_for_stop_at(&ide, mark, 13);
    ide.key("F9");
    let (finished, tests) = run_and_collect(&ide, mark);
    assert_eq!(finished["ok"], true, "AC1.5: {finished}");
    assert_eq!(
        tests.get("testPasses").map(String::as_str),
        Some("passed"),
        "AC1.5"
    );

    assert_eq!(ide.quit(), 0);
}

/// J2 (gutter run/debug/coverage, status, tooltips, data sets) and G4
/// (the statuses survive a restart), with dark and light screenshots of the
/// gutter for AC2.11.
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_phpunit_gutter_status_data_sets_and_coverage() {
    if !require_php_e2e() {
        return;
    }
    let mut ide = launch_suites("e2e_phpunit_gutter_status_data_sets_and_coverage", None);
    let mut findings = Findings::default();
    let start = ide.mark();
    let outcomes = open(&ide, "OutcomesTest.php");

    // AC2.1: a never-run file shows the run triangle on every marker.
    let lines = wait_for_gutter_lines(&ide, start, "tests/Unit/OutcomesTest.php");
    for line in [
        OUTCOMES_CLASS,
        TEST_PASSES,
        TEST_FAILS,
        TEST_SKIPPED,
        TEST_ERRORS,
    ] {
        assert!(
            lines.contains(&line),
            "AC2.1: no marker on {line}: {lines:?}"
        );
    }
    assert!(
        marker_statuses(&ide, start, outcomes).is_none_or(|m| m.is_empty()),
        "AC2.1: a never-run file shows statuses"
    );

    // AC2.2: after the class ran, each marker shows its test's result.
    let mark = ide.mark();
    gutter(&ide, outcomes, OUTCOMES_CLASS, "Run 'OutcomesTest'");
    wait_for_test_run(&ide, mark);
    wait_for_marker(&ide, mark, outcomes, TEST_ERRORS, "failed");
    let statuses = marker_statuses(&ide, mark, outcomes).expect("statuses");
    let status = |line| statuses.get(&line).map(|(s, _)| s.as_str());
    assert_eq!(status(TEST_PASSES), Some("passed"), "AC2.2: {statuses:?}");
    assert_eq!(status(TEST_FAILS), Some("failed"), "AC2.2: {statuses:?}");
    assert_eq!(status(TEST_SKIPPED), Some("ignored"), "AC2.2: {statuses:?}");
    assert_eq!(status(TEST_ERRORS), Some("failed"), "AC2.2: {statuses:?}");
    assert_eq!(
        status(OUTCOMES_CLASS),
        Some("failed"),
        "AC2.2: {statuses:?}"
    );

    // AC2.3: a failed marker's tooltip carries the failure, on the icon.
    let tip = hover_marker(&ide, outcomes, TEST_FAILS);
    let text = tip["text"].as_str().unwrap();
    findings.check(
        text.contains("expected text") && text.contains("actual text"),
        format!("AC2.3: the failed marker's tooltip lacks expected/actual: {text:?}"),
    );
    shot("e8_gutter_failed_tooltip_dark");
    // AC2.4: passed shows the duration, skipped the reason.
    let passed = hover_marker(&ide, outcomes, TEST_PASSES);
    findings.check(
        passed["text"].as_str().unwrap().contains("ms"),
        format!("AC2.4: the passed tooltip has no duration: {passed}"),
    );
    let skipped = hover_marker(&ide, outcomes, TEST_SKIPPED);
    findings.check(
        skipped["text"]
            .as_str()
            .unwrap()
            .contains("not on this platform"),
        format!("AC2.4: the skipped tooltip has no reason: {skipped}"),
    );
    ide.mouse_move(800, 600);

    // AC2.7: a data-provider marker lists each set; one runs alone.
    let data_sets = open(&ide, "DataSetsTest.php");
    let mark = ide.mark();
    gutter(&ide, data_sets, 9, "Run 'DataSetsTest'");
    wait_for_test_run(&ide, mark);
    wait_for_marker(&ide, mark, data_sets, TEST_UNNAMED, "passed");
    let named = gutter_menu_labels(&ide, data_sets, TEST_NAMED);
    for set in ["a/b", "x@y", "plain"] {
        assert!(
            named
                .iter()
                .any(|l| l.starts_with("Run") && l.contains(set)),
            "AC2.7: no Run entry for {set:?} in {named:?}"
        );
    }
    let unnamed = gutter_menu_labels(&ide, data_sets, TEST_UNNAMED);
    for set in ["#0", "#1"] {
        assert!(
            unnamed
                .iter()
                .any(|l| l.starts_with("Run") && l.contains(set)),
            "AC2.7: no Run entry for {set:?} in {unnamed:?}"
        );
    }
    let x_at_y = named
        .iter()
        .find(|l| l.starts_with("Run") && l.contains("x@y"))
        .unwrap()
        .clone();
    let mark = ide.mark();
    gutter(&ide, data_sets, TEST_NAMED, &x_at_y);
    let (finished, tests) = run_and_collect(&ide, mark);
    findings.check(
        names(&tests) == ["testNamed with data set \"x@y\""],
        format!("AC2.7: choosing x@y ran {tests:?}: {finished}"),
    );

    // AC2.8: debugging a data set stops in the method.
    toggle_breakpoint_at(&ide, 29);
    let debug_x_at_y = x_at_y.replacen("Run", "Debug", 1);
    let mark = ide.mark();
    gutter(&ide, data_sets, TEST_NAMED, &debug_x_at_y);
    let first = ide.wait_for_event_within(mark, REAL_TOOLCHAIN_TIMEOUT, "a stop or the end", |e| {
        e["ev"] == "debug_stopped" || e["ev"] == "test_run_finished"
    });
    if first["ev"] == "debug_stopped" {
        findings.check(
            first["line"] == 29,
            format!("AC2.8: stopped elsewhere: {first}"),
        );
        ide.key("F9");
    } else {
        findings.check(
            false,
            format!("AC2.8: the data set's debug run ended unstopped: {first}"),
        );
    }
    let (_, tests) = run_and_collect(&ide, mark);
    findings.check(
        names(&tests) == ["testNamed with data set \"x@y\""],
        format!("AC2.8: debugging x@y ran {tests:?}"),
    );

    // AC2.9: Run with Coverage from a marker colours the subject.
    let mark = ide.mark();
    open(&ide, "Mailer.php");
    let mailer_test = open(&ide, "MailerTest.php");
    gutter(&ide, mailer_test, 11, "Run 'testSend' with Coverage");
    wait_for_test_run(&ide, mark);
    let coverage = ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        "coverage for src/Mailer.php",
        |e| {
            e["ev"] == "coverage_lines"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("src/Mailer.php"))
        },
    );
    let lines = |key: &str| -> Vec<u64> {
        coverage[key]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default()
    };
    assert!(lines("covered").contains(&10), "AC2.9: {coverage}");
    assert!(lines("uncovered").contains(&15), "AC2.9: {coverage}");

    // AC2.10: Pest `describe()`, `it()` and dataset rows behave alike.
    let start = ide.mark();
    let pest = open(&ide, "NestedTest.php");
    let pest_lines =
        wait_for_gutter_lines(&ide, start, "packages/pest/tests/Feature/NestedTest.php");
    for line in [4, 5, PEST_IT_ADDS, 10, 15] {
        assert!(
            pest_lines.contains(&line),
            "AC2.10: no marker on {line}: {pest_lines:?}"
        );
    }
    let mark = ide.mark();
    gutter(&ide, pest, 4, "Run '");
    let (_, tests) = run_and_collect(&ide, mark);
    assert_eq!(tests.len(), 4, "AC2.10: the outer describe ran {tests:?}");
    wait_for_marker(&ide, mark, pest, PEST_IT_ADDS, "passed");
    wait_for_marker(&ide, mark, pest, 4, "passed");
    let squares = gutter_menu_labels(&ide, pest, 15);
    // A finding, not a panic: the ACs after this one still run.
    match squares
        .iter()
        .find(|l| l.starts_with("Run") && l.contains("three"))
    {
        Some(three) => {
            let mark = ide.mark();
            gutter(&ide, pest, 15, &three.clone());
            let (finished, tests) = run_and_collect(&ide, mark);
            findings.check(
                tests.len() == 1 && tests.keys().all(|n| n.contains("three")),
                format!("AC2.10: the Pest dataset row `three` ran {tests:?}: {finished}"),
            );
        }
        None => findings.check(false, format!("AC2.10: no data set entry in {squares:?}")),
    }

    // AC2.5: a running test shows the running state.
    let slow = open(&ide, "SlowTest.php");
    let mark = ide.mark();
    gutter(&ide, slow, 11, "Run 'testWaits'");
    wait_for_marker(&ide, mark, slow, 11, "running");
    shot("e8_gutter_running_dark");
    ide.focus_main();
    ide.key("ctrl+F2");
    wait_for_test_run(&ide, mark);

    // G4: the statuses survive a restart; AC2.11 in the light theme.
    reopen(&ide, "OutcomesTest.php");
    assert_eq!(ide.quit(), 0);
    let mut settings = app_config::load(&ide.config_dir()).expect("settings");
    settings.theme = "light".to_string();
    app_config::save(&ide.config_dir(), &settings).expect("the light theme");
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
    // The session restores the open tabs; bring this one to the front.
    let outcomes = ide.wait_for_event(Mark::start(), "the restored OutcomesTest.php tab", |e| {
        e["ev"] == "tab_added" && e["title"] == "OutcomesTest.php"
    })["tab_id"]
        .as_u64()
        .unwrap();
    reopen(&ide, "OutcomesTest.php");
    wait_for_marker(&ide, Mark::start(), outcomes, TEST_FAILS, "failed");
    let restored = marker_statuses(&ide, Mark::start(), outcomes).expect("restored");
    assert_eq!(
        restored.get(&TEST_PASSES).map(|(s, _)| s.as_str()),
        Some("passed"),
        "G4: {restored:?}"
    );
    hover_marker(&ide, outcomes, TEST_FAILS);
    shot("e8_gutter_failed_tooltip_light");
    ide.mouse_move(800, 600);

    // AC2.6: a method moved by an edit keeps its icon.
    caret_to(&ide, TEST_PASSES as usize + 1);
    ide.key("Home");
    let mark = ide.mark();
    for _ in 0..10 {
        ide.key("Return");
    }
    wait_for_marker(&ide, mark, outcomes, TEST_PASSES + 10, "passed");
    wait_for_marker(&ide, mark, outcomes, TEST_FAILS + 10, "failed");
    ide.key("ctrl+s");
    assert_eq!(ide.quit(), 0);
    findings.finish();
}

/// The file-level Run icon on a test file's `<?php` line (R1-7) runs the
/// file's tests, never `php <file>`, which only declares the class and dies
/// on the missing `TestCase` (#403's shape, from the gutter).
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_phpunit_file_run_icon_runs_the_tests() {
    if !require_php_e2e() {
        return;
    }
    let mut ide = launch_suites("e2e_phpunit_file_run_icon_runs_the_tests", None);
    let start = ide.mark();
    let outcomes = open(&ide, "OutcomesTest.php");
    let lines = wait_for_gutter_lines(&ide, start, "tests/Unit/OutcomesTest.php");
    if lines.contains(&0) {
        let (x, y) = rect_centre(&latest_run_icon(&ide, outcomes, 0));
        let mark = ide.mark();
        ide.click_at(x, y, 1);
        let what = ide.wait_for_event(mark, "the line-1 Run icon to do something", |e| {
            e["ev"] == "run_console_tab_added"
                || e["ev"] == "test_run_started"
                || e["ev"] == "run_gutter_menu_action"
        });
        assert_ne!(
            what["ev"], "run_console_tab_added",
            "the `<?php` Run icon of a test file runs it as a script"
        );
        if what["ev"] == "run_gutter_menu_action" {
            ide.key("Escape");
        } else {
            wait_for_test_run(&ide, mark);
        }
    }
    ide.focus_main();
    assert_eq!(ide.quit(), 0);
}
