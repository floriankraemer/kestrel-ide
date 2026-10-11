//! J7 (project tree entries) and J8 (monorepo packages).

use super::*;

// --- J7 + J8: project tree entries, monorepo packages -------------------------

/// The latest project-tree row whose path ends with `suffix`.
fn tree_row(ide: &Ide, suffix: &str) -> Value {
    let suffix = format!("/{suffix}");
    e2e::wait_for(&format!("the project tree row {suffix}"), || {
        ide.events().into_iter().rev().find(|e| {
            e["ev"] == "project_tree_row"
                && e["path"].as_str().is_some_and(|p| p.ends_with(&suffix))
        })
    })
}

/// Expand the folder row `suffix` until its child `child` is listed.
fn expand(ide: &Ide, suffix: &str, child: &str) {
    let child = format!("/{suffix}/{child}");
    let listed = ide.events().into_iter().any(|e| {
        e["ev"] == "project_tree_row" && e["path"].as_str().is_some_and(|p| p.ends_with(&child))
    });
    if listed {
        return; // expanded already
    }
    let (x, y) = rect_centre(&tree_row(ide, suffix)["rect"]);
    let mark = ide.mark();
    ide.click_at(x, y, 1);
    ide.key("Right");
    ide.wait_for_event(mark, &format!("{child} in the tree"), |e| {
        e["ev"] == "project_tree_row" && e["path"].as_str().is_some_and(|p| p.ends_with(&child))
    });
}

/// Right-click the tree at `(x, y)` and return the menu's entries.
fn tree_menu_at(ide: &Ide, x: i32, y: i32) -> Vec<Value> {
    let mark = ide.mark();
    ide.click_at(x, y, 3);
    ide.wait_for_event(mark, "the project tree's context menu", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "project_tree_context_menu"
    });
    ide.wait_for_ev(mark, "project_tree_menu_action");
    ide.events_since_of(mark, "project_tree_menu_action")
}

fn tree_menu(ide: &Ide, suffix: &str) -> Vec<Value> {
    let (x, y) = rect_centre(&tree_row(ide, suffix)["rect"]);
    tree_menu_at(ide, x, y)
}

/// The labels of the test entries (Run/Debug/Coverage of a test target).
fn test_entries(menu: &[Value]) -> Vec<String> {
    menu.iter()
        .filter_map(|e| e["label"].as_str())
        .filter(|l| l.starts_with("Run '") || l.starts_with("Debug '"))
        .map(str::to_string)
        .collect()
}

/// Click the open menu's entry `label`, or close the menu when it has none.
fn choose(ide: &Ide, menu: &[Value], label: &str) -> bool {
    match menu.iter().find(|e| e["label"] == label) {
        Some(entry) => {
            let (x, y) = rect_centre(&entry["rect"]);
            ide.click_at(x, y, 1);
            ide.focus_main();
            true
        }
        None => {
            ide.key("Escape");
            ide.focus_main();
            false
        }
    }
}

/// J7 (Run / Debug / Coverage entries in the project tree) and J8 (monorepo
/// packages run in their own directory with their own PHPUnit).
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_phpunit_project_tree_and_monorepo_packages() {
    if !require_php_e2e() {
        return;
    }
    let mut ide = launch_suites("e2e_phpunit_project_tree_and_monorepo_packages", None);
    let mut findings = Findings::default();
    ide.wait_for_ev(Mark::start(), "project_tree_rows");

    // AC7.1: a test directory offers "Tests in 'Unit'" and runs below it.
    expand(&ide, "tests", "Unit");
    let menu = tree_menu(&ide, "tests/Unit");
    let entries = test_entries(&menu);
    findings.check(
        [
            "Run 'Tests in 'Unit''",
            "Debug 'Tests in 'Unit''",
            "Run 'Tests in 'Unit'' with Coverage",
        ]
        .iter()
        .all(|l| entries.iter().any(|e| e == l)),
        format!("AC7.1: the entries on tests/Unit: {entries:?}"),
    );
    let mark = ide.mark();
    if choose(&ide, &menu, "Run 'Tests in 'Unit''") {
        let (_, tests) = run_and_collect(&ide, mark);
        findings.check(
            tests.contains_key("testSend")
                && tests.contains_key("testPasses")
                && !tests.contains_key("testDb"),
            format!("AC7.1: Tests in 'Unit' ran {tests:?}"),
        );
        // AC7.5: the temporary configuration is selected.
        findings.check(
            answer(&ide, mark, |e| {
                e["ev"] == "run_config_selected" && e["name"] == "Tests in 'Unit'"
            })
            .is_some(),
            format!(
                "AC7.5: the combo shows {:?}",
                selected_config(&ide, mark).map(|e| e["name"].clone())
            ),
        );
    }

    // AC7.2: a test file's entries name the class and run the file.
    expand(&ide, "tests/Unit", "MailerTest.php");
    let menu = tree_menu(&ide, "tests/Unit/MailerTest.php");
    let mark = ide.mark();
    if choose(&ide, &menu, "Run 'MailerTest'") {
        let (_, tests) = run_and_collect(&ide, mark);
        findings.check(
            names(&tests) == ["testSend"],
            format!("AC7.2: MailerTest ran {tests:?}"),
        );
    } else {
        findings.check(
            false,
            format!(
                "AC7.2: the entries on MailerTest.php: {:?}",
                test_entries(&menu)
            ),
        );
    }

    // AC7.4: a source file and vendor/ have no test entries.
    expand(&ide, "src", "Mailer.php");
    let menu = tree_menu(&ide, "src/Mailer.php");
    findings.check(
        test_entries(&menu).is_empty(),
        format!("AC7.4: src/Mailer.php offers {:?}", test_entries(&menu)),
    );
    choose(&ide, &menu, "");
    let menu = tree_menu(&ide, "vendor");
    findings.check(
        test_entries(&menu).is_empty(),
        format!("AC7.4: vendor/ offers {:?}", test_entries(&menu)),
    );
    choose(&ide, &menu, "");

    // AC8.1: a package's tests run with its own PHPUnit, in the package.
    expand(&ide, "packages", "billing");
    expand(&ide, "packages/billing", "tests");
    let menu = tree_menu(&ide, "packages/billing/tests");
    let mark = ide.mark();
    if choose(&ide, &menu, "Run 'Tests in 'tests''") {
        let (finished, tests) = run_and_collect(&ide, mark);
        findings.check(
            tests.get("testFailsInPackage").map(String::as_str) == Some("failed")
                && tests.keys().filter(|n| n.starts_with("testTotal")).count() == 2,
            format!("AC8.1: the billing package ran {tests:?}: {finished}"),
        );
        let selected = selected_config(&ide, mark).map(|e| e["id"].clone());
        findings.check(
            selected
                .as_ref()
                .and_then(Value::as_str)
                .is_some_and(|id| id.contains("packages/billing/phpunit.xml")),
            format!("AC8.1: the package run's configuration: {selected:?}"),
        );
    } else {
        findings.check(
            false,
            format!(
                "AC8.1: the entries on packages/billing/tests: {:?}",
                test_entries(&menu)
            ),
        );
    }

    // AC8.7: Run Failed keeps the package's directory and configuration.
    let mark = ide.mark();
    let (finished, tests) = press_dock_button(&ide, "run_failed");
    findings.check(
        names(&tests) == ["testFailsInPackage"] && tests["testFailsInPackage"] == "failed",
        format!("AC8.7: Run Failed after the package run ran {tests:?}: {finished}"),
    );
    let output: String = ide
        .events_since_of(mark, "test_output")
        .iter()
        .filter_map(|e| e["text"].as_str())
        .collect();
    eprintln!("AC8.7 Run Failed output: {output}");

    // AC8.2/AC8.3/AC8.4: gutter, caret, data set and coverage in a package.
    let invoice_src = open(&ide, "Invoice.php");
    let _ = invoice_src;
    let invoice = open(&ide, "InvoiceTest.php");
    let mark = ide.mark();
    gutter(&ide, invoice, 26, "Run 'testFailsInPackage'");
    let (_, tests) = run_and_collect(&ide, mark);
    findings.check(
        tests.get("testFailsInPackage").map(String::as_str) == Some("failed"),
        format!("AC8.2: the gutter run in the package: {tests:?}"),
    );
    caret_to(&ide, 23);
    let mark = ide.mark();
    ide.key("ctrl+shift+F10");
    let (_, tests) = run_and_collect(&ide, mark);
    findings.check(
        tests.keys().filter(|n| n.starts_with("testTotal")).count() == 2 && tests.len() == 2,
        format!("AC8.2: the caret run of testTotal: {tests:?}"),
    );
    let sets = gutter_menu_labels(&ide, invoice, 21);
    match sets
        .iter()
        .find(|l| l.starts_with("Run") && l.contains("standard"))
    {
        Some(standard) => {
            let mark = ide.mark();
            gutter(&ide, invoice, 21, &standard.clone());
            let (finished, tests) = run_and_collect(&ide, mark);
            findings.check(
                tests.len() == 1 && tests.keys().all(|n| n.contains("standard")),
                format!("AC8.2: the data set run in the package: {tests:?}: {finished}"),
            );
        }
        None => findings.check(false, format!("AC8.2: no data set entries in {sets:?}")),
    }
    let mark = ide.mark();
    gutter(&ide, invoice, 21, "Run 'testTotal' with Coverage");
    wait_for_test_run(&ide, mark);
    let clover = ide.project_root().join(".ide/local/coverage/clover.xml");
    findings.check(clover.exists(), format!("AC8.4: no {}", clover.display()));
    let coloured = answer(&ide, mark, |e| {
        e["ev"] == "coverage_lines"
            && e["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("packages/billing/src/Invoice.php"))
    });
    findings.check(
        coloured.is_some(),
        "AC8.4: the package's subject was not coloured",
    );

    // AC8.3: jump to source and the failure link resolve in the package.
    let mark = ide.mark();
    gutter(&ide, invoice, 26, "Run 'testFailsInPackage'");
    wait_for_test_run(&ide, mark);
    let (x, y) = tests_row(&ide, mark, "testFailsInPackage", |n| {
        n == "testFailsInPackage"
    });
    ide.click_at(x, y, 1);
    // Since the run: reaching the row by keyboard may have selected it already.
    let shown = ide.wait_for_event(mark, "the package failure's details", |e| {
        e["ev"] == "test_failure_details"
            && e["id"]
                .as_str()
                .is_some_and(|id| id.ends_with("::testFailsInPackage"))
    });
    findings.check(
        shown["message"]
            .as_str()
            .is_some_and(|m| m.contains("Failed asserting")),
        format!("AC8.1/AC8.3: the package failure is not an assertion: {shown}"),
    );
    let jump = ide.mark();
    ide.key("Return");
    let line = landing(&ide, jump, "packages/billing/tests/InvoiceTest.php");
    findings.check(
        line == Some(26),
        format!("AC8.3: jump to source landed on {line:?}"),
    );
    match shown["link_rect"].as_array().filter(|r| !r.is_empty()) {
        Some(_) => {
            let (x, y) = rect_centre(&shown["link_rect"]);
            let jump = ide.mark();
            e2e::xdotool::run(&["keydown", "ctrl"]);
            ide.mouse_move(x, y);
            e2e::xdotool::run(&["click", "1"]);
            e2e::xdotool::run(&["keyup", "ctrl"]);
            let line = landing(&ide, jump, "packages/billing/tests/InvoiceTest.php");
            findings.check(
                line == Some(28),
                format!("AC8.3: the failure link landed on {line:?}"),
            );
        }
        None => findings.check(false, format!("AC8.3: no failure link in {shown}")),
    }

    // AC8.5: an autoload-only package runs with the project's suite.
    expand(&ide, "packages", "util");
    expand(&ide, "packages/util", "tests");
    let menu = tree_menu(&ide, "packages/util/tests");
    let mark = ide.mark();
    if choose(&ide, &menu, "Run 'Tests in 'tests''") {
        let (finished, tests) = run_and_collect(&ide, mark);
        findings.check(
            tests.get("testSlugs").map(String::as_str) == Some("passed"),
            format!("AC8.5: packages/util ran {tests:?}: {finished}"),
        );
    } else {
        findings.check(
            false,
            format!(
                "AC8.5: the entries on packages/util/tests: {:?}",
                test_entries(&menu)
            ),
        );
    }

    // AC7.3: the tree's empty area runs all the configured suites.
    let last = ide
        .events()
        .into_iter()
        .filter(|e| e["ev"] == "project_tree_row")
        .max_by_key(|e| e["rect"][1].as_i64().unwrap_or(0))
        .expect("a tree row");
    let (x, y) = rect_centre(&last["rect"]);
    let menu = tree_menu_at(&ide, x, y + 60);
    let mark = ide.mark();
    if choose(&ide, &menu, "Run 'All tests'") {
        let (_, tests) = run_and_collect(&ide, mark);
        findings.check(
            tests.contains_key("testSend")
                && tests.contains_key("testDb")
                && tests.contains_key("testSlugs")
                && !tests.contains_key("testSlow")
                && !tests.contains_key("testWaits"),
            format!("AC7.3: All tests ran {tests:?}"),
        );
    } else {
        findings.check(
            false,
            format!("AC7.3: the empty area offers {:?}", test_entries(&menu)),
        );
    }

    ide.focus_main();
    assert_eq!(ide.quit(), 0);
    findings.finish();
}
