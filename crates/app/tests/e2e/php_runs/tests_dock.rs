//! J3 (the Tests dock's navigation) and AC10.1–AC10.3 (shortcut contexts).

use super::*;

/// J3 (Tests dock jump to source, F4, failure links) and AC10.1–AC10.3
/// (Ctrl+Shift+T and F4 in their focus contexts).
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_phpunit_tests_dock_navigation_and_shortcut_contexts() {
    if !require_php_e2e() {
        return;
    }
    let mut ide = launch_suites(
        "e2e_phpunit_tests_dock_navigation_and_shortcut_contexts",
        None,
    );
    let mut findings = Findings::default();
    let outcomes = open(&ide, "OutcomesTest.php");
    let run = gutter_run_then_dock(&ide, outcomes, OUTCOMES_CLASS, "Run 'OutcomesTest'");
    // An editor run does not show the Tests dock, and the first failure's
    // diagnostic opens Problems over it: let that happen first.
    findings.check(
        !ide.events_since_of(run, "tests_toolbar_rects").is_empty(),
        "AC1.1/J3: a test run from the gutter did not show the Tests dock",
    );
    ide.wait_for_event(run, "the failures to reach Problems", |e| {
        e["ev"] == "problems_refreshed" && e["total"].as_u64().unwrap_or(0) > 0
    });
    // Shown once; every later run keeps it up and re-reports its rows.
    let mark = open_view_item(&ide, "Tests");
    ide.wait_for_ev(mark, "test_tree_rects");
    shot("e8_tests_dock_shown_dark");

    // AC3.1: double-click a passed method row.
    let (x, y) = tests_row(&ide, mark, "testPasses", |n| n == "testPasses");
    let jump = ide.mark();
    ide.double_click_at(x, y, 1);
    let line = landing(&ide, jump, "tests/Unit/OutcomesTest.php");
    findings.check(
        line == Some(TEST_PASSES),
        format!("AC3.1: the double-click landed on {line:?}, not {TEST_PASSES}"),
    );
    if line.is_none() {
        // Tells a double-click that never activates from a row with no source.
        let jump = ide.mark();
        ide.key("Return");
        let line = landing(&ide, jump, "tests/Unit/OutcomesTest.php");
        findings.check(
            false,
            format!("AC3.1: Enter on the same row landed on {line:?}"),
        );
    }

    // AC3.2: F4 on the class row.
    let (x, y) = tests_row(&ide, mark, "the class", |n| {
        n == "Tests\\Unit\\OutcomesTest"
    });
    ide.click_at(x, y, 1);
    let jump = ide.mark();
    ide.key("F4");
    let line = landing(&ide, jump, "tests/Unit/OutcomesTest.php");
    findings.check(
        line == Some(OUTCOMES_CLASS),
        format!("AC3.2: F4 landed on {line:?}, not {OUTCOMES_CLASS}"),
    );

    // AC3.4: Jump to Source is in the row's context menu, enabled.
    let (x, y) = tests_row(&ide, mark, "testPasses", |n| n == "testPasses");
    let menu = ide.mark();
    ide.click_at(x, y, 3);
    let entry = ide.wait_for_event(menu, "the Jump to Source entry", |e| {
        e["ev"] == "tests_menu_action" && e["label"] == "Jump to Source"
    });
    findings.check(entry["enabled"] == true, format!("AC3.4: {entry}"));
    ide.key("Escape");

    // AC3.6: the failure link in the details pane opens the assertion line.
    let details = ide.mark();
    let (x, y) = tests_row(&ide, mark, "testFails", |n| n == "testFails");
    ide.click_at(x, y, 1);
    let shown = ide.wait_for_event(details, "testFails' details", |e| {
        e["ev"] == "test_failure_details"
            && e["id"]
                .as_str()
                .is_some_and(|id| id.ends_with("::testFails"))
    });
    shot("e8_tests_dock_failure_dark");
    let details_text = shown["details"].as_str().unwrap_or_default().to_string();
    findings.check(
        details_text.contains("expected text") && details_text.contains("actual text"),
        format!("AC3.6: the details lack the comparison: {details_text:?}"),
    );
    match shown["link_rect"].as_array().filter(|r| !r.is_empty()) {
        Some(_) => {
            let (x, y) = rect_centre(&shown["link_rect"]);
            let jump = ide.mark();
            e2e::xdotool::run(&["keydown", "ctrl"]);
            ide.mouse_move(x, y);
            e2e::xdotool::run(&["click", "1"]);
            e2e::xdotool::run(&["keyup", "ctrl"]);
            let line = landing(&ide, jump, "tests/Unit/OutcomesTest.php");
            findings.check(
                line == Some(18),
                format!("AC3.6: the link landed on {line:?}, not 18"),
            );
        }
        None => findings.check(false, format!("AC3.6: no failure link in {details_text:?}")),
    }

    // AC3.5: unsaved edits moved the method; the jump finds it in the buffer.
    reopen(&ide, "OutcomesTest.php");
    caret_to(&ide, TEST_PASSES as usize + 1);
    ide.key("Home");
    for _ in 0..3 {
        ide.key("Return");
    }
    let (x, y) = tests_row(&ide, mark, "testFails", |n| n == "testFails");
    let jump = ide.mark();
    ide.double_click_at(x, y, 1);
    let line = landing(&ide, jump, "tests/Unit/OutcomesTest.php");
    findings.check(
        line == Some(TEST_FAILS + 3),
        format!(
            "AC3.5: landed on {line:?}, not the moved {}",
            TEST_FAILS + 3
        ),
    );
    reopen(&ide, "OutcomesTest.php");
    ide.key("ctrl+s");

    // Run Failed at the project root (AC8.7's counterpart outside a package).
    let (finished, tests) = press_dock_button(&ide, "run_failed");
    findings.check(
        names(&tests) == ["testErrors", "testFails"],
        format!("Run Failed at the project root ran {tests:?}: {finished}"),
    );

    // AC3.3: Enter on a data-set row opens the method.
    fold_row(&ide, mark, "Tests\\Unit\\OutcomesTest");
    let data_sets = open(&ide, "DataSetsTest.php");
    let mark = gutter_run_then_dock(&ide, data_sets, 9, "Run 'DataSetsTest'");
    let (x, y) = tests_row(&ide, mark, "the x@y data set", |n| n.ends_with("\"x@y\""));
    shot("e8_tests_dock_data_sets_dark");
    ide.click_at(x, y, 1);
    let jump = ide.mark();
    ide.key("Return");
    let line = landing(&ide, jump, "tests/Unit/DataSetsTest.php");
    findings.check(
        line == Some(TEST_NAMED),
        format!("AC3.3: Enter landed on {line:?}, not {TEST_NAMED}"),
    );

    // AC3.4: … disabled once the file is gone.
    fold_row(&ide, mark, "Tests\\Unit\\DataSetsTest");
    let tab = open_file(&ide, "MailerTest.php");
    let mailer_test = tab["tab_id"].as_u64().unwrap();
    let mark = gutter_run_then_dock(&ide, mailer_test, 11, "Run 'testSend'");
    // The tab is closed first, as a user deleting the file would have it:
    // deleting an open file raises a "modified outside the editor" modal.
    let (x, y) = rect_centre(&tab["rect"]);
    let menu = ide.mark();
    ide.click_at(x, y, 3);
    let close = ide.wait_for_event(menu, "the tab's Close entry", |e| {
        e["ev"] == "tab_context_menu_action" && e["label"] == "Close"
    });
    let (x, y) = rect_centre(&close["rect"]);
    ide.click_at(x, y, 1);
    ide.wait_for_ev(menu, "tab_closed");
    ide.focus_main();
    std::fs::remove_file(ide.project_root().join("tests/Unit/MailerTest.php")).expect("rm");
    let (x, y) = tests_row(&ide, mark, "testSend", |n| n == "testSend");
    let menu = ide.mark();
    ide.click_at(x, y, 3);
    let entry = ide.wait_for_event(menu, "the Jump to Source entry", |e| {
        e["ev"] == "tests_menu_action" && e["label"] == "Jump to Source"
    });
    findings.check(
        entry["enabled"] == false,
        format!("AC3.4: a deleted file's entry: {entry}"),
    );
    ide.key("Escape");

    // AC3.7: a Pest row opens its `it()`, nested `describe()` included.
    fold_row(&ide, mark, "Tests\\Unit\\MailerTest");
    let pest = open(&ide, "NestedTest.php");
    let mark = gutter_run_then_dock(&ide, pest, 4, "Run '");
    let (x, y) = tests_row(&ide, mark, "it adds", |n| n.ends_with("it adds"));
    let jump = ide.mark();
    ide.double_click_at(x, y, 1);
    let line = landing(&ide, jump, "packages/pest/tests/Feature/NestedTest.php");
    findings.check(
        line == Some(PEST_IT_ADDS),
        format!("AC3.7: landed on {line:?}, not {PEST_IT_ADDS}"),
    );
    let pest_run = mark;

    // AC10.1: Ctrl+Shift+T is Go to Test in the editor …
    open(&ide, "Orphan.php");
    let keyed = ide.mark();
    ide.focus_main();
    ide.key("ctrl+shift+T");
    wait_for_status(&ide, keyed, "No test found for");
    assert!(
        ide.events_since_of(keyed, "terminal_session_added")
            .is_empty(),
        "AC10.1: the editor's Ctrl+Shift+T opened a terminal"
    );
    // … and New Terminal Tab in the terminal.
    open_view_item(&ide, "Terminal");
    let keyed = ide.mark();
    ide.key("ctrl+shift+T");
    ide.wait_for_ev(keyed, "terminal_session_added");
    let mcp = ide.mcp();
    ide.sync(&mcp);
    findings.check(
        !ide.events_since_of(keyed, "status_message")
            .iter()
            .any(|e| e["text"].as_str().is_some_and(|t| t.contains("test"))),
        "AC10.1: the terminal's Ctrl+Shift+T ran Go to Test",
    );
    findings.check(
        !ide.stderr().contains("Ambiguous shortcut"),
        "AC10.1: Qt reported an ambiguous shortcut on stderr",
    );

    // AC10.3: in the project tree Ctrl+Shift+T does nothing …
    let row = ide
        .events()
        .into_iter()
        .rev()
        .find(|e| {
            e["ev"] == "project_tree_row" && e["path"].as_str().is_some_and(|p| p.ends_with("/src"))
        })
        .expect("a src row");
    let (x, y) = rect_centre(&row["rect"]);
    ide.click_at(x, y, 1);
    let keyed = ide.mark();
    ide.key("ctrl+shift+T");
    ide.sync(&mcp);
    findings.check(
        ide.events_since_of(keyed, "status_message").is_empty()
            && ide.events_since_of(keyed, "opened_at").is_empty()
            && ide
                .events_since_of(keyed, "terminal_session_added")
                .is_empty(),
        format!(
            "AC10.3: Ctrl+Shift+T in the project tree did something: {:?}",
            ide.events_since(keyed)
        ),
    );
    // … while Navigate > Test still works from there.
    let menu = ide.mark();
    ide.key("alt+n");
    let entry = ide.wait_for_event(menu, "Navigate > Test", |e| {
        e["ev"] == "navigate_menu_action" && (e["label"] == "Test" || e["label"] == "Test Subject")
    });
    let (x, y) = rect_centre(&entry["rect"]);
    let chosen = ide.mark();
    ide.click_at(x, y, 1);
    wait_for_status(&ide, chosen, "No test found for Orphan");

    // The Pest run reported no failure of OutcomesTest: Problems must not
    // list that file's PHPUnit failures again under Pest.
    let misattributed: Vec<Value> = ide
        .events_since_of(pest_run, "problem_row")
        .into_iter()
        .filter(|e| {
            e["source"] == "Pest"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("OutcomesTest.php"))
        })
        .collect();
    findings.check(
        misattributed.is_empty(),
        format!(
            "Problems lists OutcomesTest's PHPUnit failures again as Pest's: {:?}",
            misattributed.first()
        ),
    );

    ide.focus_main();
    assert_eq!(ide.quit(), 0);
    findings.finish();
}
