//! J4 (saved PHPUnit configurations), AC5.4–AC5.7 and J6 (before-launch tasks).

use super::*;

// --- J4 + AC5.4–5.7 + J6: saved configurations, before-launch tasks -----------

/// The saved configurations flow 3 runs, one per scope and option.
const CONFIGURATIONS: &str = r#"
[[run_config]]
id = "e8-dir"
name = "E8 directory"
kind = "phpunit"
[run_config.phpunit]
scope = "directory"
path = "tests/Unit"

[[run_config]]
id = "e8-file"
name = "E8 file"
kind = "phpunit"
[run_config.phpunit]
scope = "file"
path = "tests/Unit/MailerTest.php"

[[run_config]]
id = "e8-class"
name = "E8 class"
kind = "phpunit"
[run_config.phpunit]
scope = "class"
path = "tests/Unit/OutcomesTest.php"
class = 'Tests\Unit\OutcomesTest'

[[run_config]]
id = "e8-method"
name = "E8 method"
kind = "phpunit"
[run_config.phpunit]
scope = "method"
path = "tests/Unit/OutcomesTest.php"
class = 'Tests\Unit\OutcomesTest'
method = "testPasses"

[[run_config]]
id = "e8-data-set"
name = "E8 data set"
kind = "phpunit"
[run_config.phpunit]
scope = "data_set"
path = "tests/Unit/DataSetsTest.php"
class = 'Tests\Unit\DataSetsTest'
method = "testNamed"
data_set = '"x@y"'

[[run_config]]
id = "e8-suite"
name = "E8 suite unit"
kind = "phpunit"
[run_config.phpunit]
scope = "config"
testsuite = "unit"

[[run_config]]
id = "e8-group"
name = "E8 group db"
kind = "phpunit"
[run_config.phpunit]
scope = "config"
groups = ["db"]

[[run_config]]
id = "e8-exclude"
name = "E8 feature without slow"
kind = "phpunit"
[run_config.phpunit]
scope = "config"
testsuite = "feature"
exclude_groups = ["slow"]

[[run_config]]
id = "e8-stop"
name = "E8 stop on failure"
kind = "phpunit"
[run_config.phpunit]
scope = "class"
path = "tests/Unit/OutcomesTest.php"
class = 'Tests\Unit\OutcomesTest'
runner_options = "--stop-on-failure"

[[run_config]]
id = "e8-env"
name = "E8 env"
kind = "phpunit"
env = [["APP_ENV", "test"]]
[run_config.phpunit]
scope = "file"
path = "tests/Extra/EnvTest.php"

[[run_config]]
id = "e8-broken"
name = "E8 broken"
kind = "phpunit"
[run_config.phpunit]
scope = "method"
path = "tests/Unit/OutcomesTest.php"
class = 'Tests\Unit\OutcomesTest'

[[run_config]]
id = "e8-slow"
name = "E8 slow"
kind = "phpunit"
[run_config.phpunit]
scope = "file"
path = "tests/Extra/SlowTest.php"

[[run_config]]
id = "e8-slow-parallel"
name = "E8 slow parallel"
kind = "phpunit"
allow_parallel = true
[run_config.phpunit]
scope = "file"
path = "tests/Extra/SlowTest.php"

[[run_config]]
id = "e8-before"
name = "E8 before launch"
kind = "phpunit"
[[run_config.before_launch]]
kind = "external_tool"
program = "sh"
args = ["-c", "date +%s%N > before-launch.txt"]
[run_config.phpunit]
scope = "method"
path = "tests/Unit/OutcomesTest.php"
class = 'Tests\Unit\OutcomesTest'
method = "testPasses"

[[run_config]]
id = "e8-before-fails"
name = "E8 before launch fails"
kind = "phpunit"
[[run_config.before_launch]]
kind = "external_tool"
program = "false"
[run_config.phpunit]
scope = "method"
path = "tests/Unit/OutcomesTest.php"
class = 'Tests\Unit\OutcomesTest'
method = "testPasses"

[[run_config]]
id = "e8-file-macro"
name = "E8 file macro"
kind = "phpunit"
[run_config.phpunit]
scope = "file"
path = "tests/Unit/MailerTest.php"
runner_options = "--log-junit $FILE_DIR$/e8-junit.xml"

[[run_config]]
id = "e8-cannot-start"
name = "E8 cannot start"
kind = "phpunit"
[run_config.phpunit]
scope = "file"
path = "tests/Unit/MailerTest.php"
interpreter_options = """-r "fwrite(STDERR, 'e8 cannot start'); exit(3);""""

[[run_config]]
id = "e8-script-after-tests"
name = "E8 script after failing tests"
program = "php"
args = ["bin/console.php"]
toolchain = "php"
[[run_config.before_launch]]
kind = "run_configuration"
config_id = "e8-class"
"#;

/// Select the configuration called `name` in the toolbar combo, through
/// Run > Select Run Configuration… and the keyboard.
fn select_config(ide: &Ide, name: &str) {
    let selection = selected_config(ide, Mark::start()).expect("a selection");
    if selection["name"] == name {
        return;
    }
    let configs = selection["configs"].clone();
    let index = configs
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no configuration {name:?} in {configs}"));
    let mark = ide.mark();
    ide.focus_main();
    ide.key("alt+r");
    let item = ide.wait_for_event(mark, "Select Run Configuration…", |e| {
        e["ev"] == "run_menu_action" && e["label"] == "Select Run Configuration..."
    });
    let (x, y) = rect_centre(&item["rect"]);
    ide.click_at(x, y, 1);
    // The combo's popup holds the keyboard now.
    ide.key("Home");
    for _ in 0..index {
        ide.key("Down");
    }
    ide.key("Return");
    wait_for_selected(ide, mark, name);
    ide.focus_main();
}

/// Run the selected configuration through the Run menu's `entry` and return
/// the tests it ran.
fn run_selected(ide: &Ide, entry: &str) -> (Value, BTreeMap<String, String>) {
    let mark = ide.mark();
    click_run_menu_item(ide, entry);
    run_and_collect(ide, mark)
}

/// The Edit Configurations dialog's command preview of each configuration,
/// by name: the list walked top to bottom with Down.
fn previews(ide: &Ide, count: usize) -> BTreeMap<String, String> {
    let mark = open_config_dialog(ide);
    // The first row's page asks for the pickers' model before we move on.
    answer(ide, mark, |e| e["ev"] == "phpunit_page_model");
    for _ in 0..count {
        let moved = ide.mark();
        ide.key("Down");
        if answer(ide, moved, |e| e["ev"] == "run_config_preview").is_none() {
            break; // the last row
        }
    }
    shot("e8_run_config_dialog_dark");
    let mut out = BTreeMap::new();
    for e in ide.events_since_of(mark, "run_config_preview") {
        out.insert(
            e["name"].as_str().unwrap_or_default().to_string(),
            e["preview"].as_str().unwrap_or_default().to_string(),
        );
    }
    close_config_dialog(ide, mark);
    out
}

/// AC4.14: on every PHPUnit page the dialog showed, the fields start at one
/// x and the rows that do not apply to the kind are gone (#448).
fn check_page_layout(ide: &Ide, opened: Mark, findings: &mut Findings) {
    let forms: Vec<Value> = ide
        .events_since_of(opened, "run_config_form")
        .into_iter()
        .filter(|e| e["kind"] == "phpunit")
        .collect();
    findings.check(!forms.is_empty(), "AC4.14: no PHPUnit page was laid out");
    for form in forms {
        let rows = form["rows"].as_array().cloned().unwrap_or_default();
        let xs: std::collections::BTreeSet<i64> =
            rows.iter().filter_map(|r| r["field_x"].as_i64()).collect();
        let labels: Vec<&str> = rows.iter().filter_map(|r| r["label"].as_str()).collect();
        findings.check(
            xs.len() == 1,
            format!("AC4.14: {:?} has field columns at {xs:?}", form["name"]),
        );
        findings.check(
            !labels
                .iter()
                .any(|l| ["Program:", "Arguments:", "Working dir:", "Run on:"].contains(l)),
            format!(
                "AC4.14: {:?} shows rows of another kind: {labels:?}",
                form["name"]
            ),
        );
    }
}

/// AC4.11 (Enter in the group picker adds a group, the dialog stays) and
/// AC4.6 (an alternative configuration file reloads the pickers).
fn check_page_pickers(ide: &Ide, findings: &mut Findings) {
    let opened = open_config_dialog(ide);
    let Some(form) = find_config_row(ide, opened, 20, |f| f["kind"] == "phpunit") else {
        findings.check(
            false,
            "AC4.6/AC4.11: the dialog lists no PHPUnit configuration",
        );
        close_config_dialog(ide, opened);
        return;
    };
    let page = &form["phpunit"];
    let (x, y) = rect_centre(&page["group_add_rect"]);
    ide.click_at(x, y, 1);
    let typed = ide.mark();
    ide.type_text("extra");
    ide.key("Return");
    let preview = answer(ide, typed, |e| {
        e["ev"] == "run_config_preview"
            && e["preview"]
                .as_str()
                .is_some_and(|p| p.contains("--group extra"))
    });
    findings.check(
        preview.is_some(),
        "AC4.11: Enter in the group picker added no group to the preview",
    );
    findings.check(
        ide.events_since(typed)
            .iter()
            .all(|e| !(e["ev"] == "dialog_closed" && e["name"] == "run_config_dialog")),
        "AC4.11: Enter in the group picker closed the dialog",
    );

    let (x, y) = rect_centre(&page["configuration_rect"]);
    ide.click_at(x, y, 1);
    ide.key("ctrl+a");
    let typed = ide.mark();
    ide.type_text("packages/billing/phpunit.xml");
    let model = answer(ide, typed, |e| {
        e["ev"] == "phpunit_page_model"
            && e["suites"]
                .as_array()
                .is_some_and(|s| s.iter().any(|n| n == "billing"))
    });
    findings.check(
        model.is_some(),
        "AC4.6: the alternative configuration file did not reload the pickers",
    );
    close_config_dialog(ide, opened);
}

/// #457: a configuration edited and saved in the dialog reaches the toolbar
/// combo at once. The flow's last step: it removes "E8 broken", which the
/// dialog would refuse to save.
fn check_save_refreshes_combo(ide: &Ide, findings: &mut Findings) {
    let opened = open_config_dialog(ide);
    let shown = ide.wait_for_event(opened, "the run configuration dialog", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "run_config_dialog"
    });
    if find_config_row(ide, opened, 30, |f| f["name"] == "E8 broken").is_none() {
        findings.check(false, "#457: the dialog does not list \"E8 broken\"");
        close_config_dialog(ide, opened);
        return;
    }
    let (x, y) = rect_centre(&shown["remove_rect"]);
    ide.click_at(x, y, 1);
    // Back to the list's first row, so the keyboard walks the list again.
    let removed = ide.mark();
    let list = &shown["list_rect"];
    let (x, _) = rect_centre(list);
    ide.click_at(x, list[1].as_i64().unwrap() as i32 + 8, 1);
    let Some(form) = find_config_row(ide, removed, 30, |f| f["name"] == "E8 file macro") else {
        findings.check(false, "#457: the dialog does not list \"E8 file macro\"");
        close_config_dialog(ide, opened);
        return;
    };
    let (x, y) = rect_centre(&form["phpunit"]["group_add_rect"]);
    ide.click_at(x, y, 1);
    let typed = ide.mark();
    ide.type_text("extra");
    ide.key("Return");
    answer(ide, typed, |e| {
        e["ev"] == "run_config_preview"
            && e["preview"]
                .as_str()
                .is_some_and(|p| p.contains("--group extra"))
    });
    let saving = ide.mark();
    let (x, y) = rect_centre(&shown["save_rect"]);
    ide.click_at(x, y, 1);
    ide.wait_for_event(saving, "the dialog to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "run_config_dialog"
    });
    ide.focus_main();
    findings.check(
        answer(ide, saving, |e| e["ev"] == "run_configurations_changed").is_some(),
        "#457: saving the dialog did not refresh the toolbar's configurations",
    );
}

/// J4 (scopes and options of saved PHPUnit configurations), AC5.4–AC5.7
/// (edited temporaries, parallel runs, Stop) and J6 (before-launch tasks).
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_phpunit_saved_configurations() {
    if !require_php_e2e() {
        return;
    }
    let mut ide = launch_suites("e2e_phpunit_saved_configurations", Some(CONFIGURATIONS));
    let mut findings = Findings::default();
    ide.wait_for_event(Mark::start(), "the configurations in the combo", |e| {
        e["ev"] == "run_config_selected" && e["configs"].as_array().is_some_and(|c| c.len() >= 16)
    });

    // AC4.2: the argv preview of each scope follows ADR-0073's table.
    let opened = ide.mark();
    let shown = previews(&ide, 24);
    check_page_layout(&ide, opened, &mut findings);
    check_page_pickers(&ide, &mut findings);
    // AC4.4/AC4.5: the pickers offer what phpunit.xml declares.
    let model = answer(&ide, opened, |e| e["ev"] == "phpunit_page_model");
    let listed = |key: &str| -> Vec<String> {
        model
            .as_ref()
            .and_then(|m| m[key].as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    findings.check(
        ["unit", "feature", "util"]
            .iter()
            .all(|s| listed("suites").iter().any(|l| l == s)),
        format!("AC4.4: the suite combo lists {:?}", listed("suites")),
    );
    findings.check(
        ["slow", "db"]
            .iter()
            .all(|g| listed("groups").iter().any(|l| l == g)),
        format!("AC4.5: the group pickers offer {:?}", listed("groups")),
    );
    eprintln!("previews: {shown:#?}");
    let preview = |name: &str| shown.get(name).cloned().unwrap_or_default();
    for (name, needles) in [
        ("E8 directory", vec!["--teamcity", "tests/Unit"]),
        ("E8 file", vec!["tests/Unit/MailerTest.php"]),
        (
            "E8 class",
            vec!["--filter", "OutcomesTest::", "tests/Unit/OutcomesTest.php"],
        ),
        (
            "E8 method",
            vec!["--filter", "OutcomesTest::testPasses( with data set .+)?$"],
        ),
        // `@` is a PCRE delimiter-safe `\x40` in the filter (test_core::scope).
        (
            "E8 data set",
            vec!["--filter", "testNamed with data set \"x\\x40y\"$"],
        ),
        ("E8 suite unit", vec!["--testsuite", "unit"]),
        ("E8 group db", vec!["--group", "db"]),
        (
            "E8 feature without slow",
            vec!["--testsuite", "feature", "--exclude-group", "slow"],
        ),
        ("E8 stop on failure", vec!["--stop-on-failure"]),
        // AC4.13 (#458): a file macro stands for the scope's file.
        ("E8 file macro", vec!["tests/Unit/e8-junit.xml"]),
    ] {
        let text = preview(name);
        findings.check(
            needles.iter().all(|n| text.contains(n)),
            format!("AC4.2: the preview of {name:?} lacks {needles:?}: {text:?}"),
        );
    }

    // AC4.2/AC4.3/AC4.5/AC4.7/AC4.10: each scope runs exactly its tests.
    let unit_only = |tests: &BTreeMap<String, String>| {
        tests.contains_key("testSend")
            && tests.contains_key("testNotify")
            && tests.contains_key("testPasses")
            && !tests.contains_key("testDb")
            && !tests.contains_key("testSlugs")
    };
    select_config(&ide, "E8 directory");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(unit_only(&tests), format!("AC4.2 directory ran {tests:?}"));
    select_config(&ide, "E8 file");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(
        names(&tests) == ["testSend"],
        format!("AC4.2 file ran {tests:?}"),
    );
    select_config(&ide, "E8 class");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(tests.len() == 4, format!("AC4.2 class ran {tests:?}"));
    select_config(&ide, "E8 method");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(
        names(&tests) == ["testPasses"],
        format!("AC4.2 method ran {tests:?}"),
    );
    select_config(&ide, "E8 data set");
    let (finished, tests) = run_selected(&ide, "Run");
    findings.check(
        names(&tests) == ["testNamed with data set \"x@y\""],
        format!("AC4.2 data set ran {tests:?}: {finished}"),
    );
    select_config(&ide, "E8 suite unit");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(unit_only(&tests), format!("AC4.3 suite unit ran {tests:?}"));
    select_config(&ide, "E8 group db");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(
        names(&tests) == ["testDb"],
        format!("AC4.5 group db ran {tests:?}"),
    );
    select_config(&ide, "E8 feature without slow");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(
        names(&tests) == ["testDb", "testPlain"],
        format!("AC4.5 feature without slow ran {tests:?}"),
    );
    select_config(&ide, "E8 stop on failure");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(
        names(&tests) == ["testFails", "testPasses"],
        format!("AC4.7 --stop-on-failure ran {tests:?}"),
    );
    select_config(&ide, "E8 env");
    let (_, tests) = run_selected(&ide, "Run");
    findings.check(
        tests.get("testAppEnv").map(String::as_str) == Some("passed"),
        format!("AC4.7 APP_ENV=test: {tests:?}"),
    );

    // AC4.13 (#458): the file macro expands in the run too; PHPUnit writes
    // no directory called `$FILE_DIR$`.
    let junit = ide.project_root().join("tests/Unit/e8-junit.xml");
    select_config(&ide, "E8 file macro");
    let (finished, _) = run_selected(&ide, "Run");
    findings.check(
        junit.exists() && !ide.project_root().join("$FILE_DIR$").exists(),
        format!("AC4.13: --log-junit $FILE_DIR$/e8-junit.xml did not expand: {finished}"),
    );
    std::fs::remove_file(&junit).ok();

    // #456: a run whose runner never starts ends as a failure with its exit
    // code, and what php wrote to stderr is in the Tests dock's output.
    select_config(&ide, "E8 cannot start");
    let mark = ide.mark();
    click_run_menu_item(&ide, "Run");
    let finished = wait_for_test_run(&ide, mark);
    findings.check(
        finished["ok"] == false
            && finished["message"]
                .as_str()
                .is_some_and(|m| m.contains("exit code 3")),
        format!("#456: a run that never started finished as {finished}"),
    );
    findings.check(
        ide.events_since_of(mark, "test_output").iter().any(|e| {
            e["text"]
                .as_str()
                .is_some_and(|t| t.contains("e8 cannot start"))
        }),
        "#456: php's stderr is not in the Tests dock's output",
    );
    // #468: the earlier run's rows are not presented as this run's result.
    findings.check(
        ide.events_since_of(mark, "test_tree_changed")
            .last()
            .is_some_and(|e| e["nodes"] == 0),
        "#468: a run that never started left the previous run's rows in the Tests dock",
    );

    // AC4.10: Debug and Run with Coverage on a saved configuration.
    select_config(&ide, "E8 method");
    open(&ide, "OutcomesTest.php");
    toggle_breakpoint_at(&ide, 13);
    let mark = ide.mark();
    ide.focus_main();
    ide.key("shift+F9"); // debug.debug: the toolbar's Debug
    let stop = ide.wait_for_event_within(mark, REAL_TOOLCHAIN_TIMEOUT, "a stop or the end", |e| {
        e["ev"] == "debug_stopped" || e["ev"] == "test_run_finished"
    });
    findings.check(
        stop["ev"] == "debug_stopped",
        format!("AC4.10 Debug did not stop: {stop}"),
    );
    if stop["ev"] == "debug_stopped" {
        ide.key("F9");
    }
    let (_, tests) = run_and_collect(&ide, mark);
    findings.check(
        names(&tests) == ["testPasses"],
        format!("AC4.10 Debug ran {tests:?}"),
    );
    open(&ide, "Mailer.php");
    select_config(&ide, "E8 file");
    let mark = ide.mark();
    click_run_menu_item(&ide, "Run with Coverage");
    wait_for_test_run(&ide, mark);
    let coverage = answer(&ide, mark, |e| {
        e["ev"] == "coverage_lines"
            && e["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("src/Mailer.php"))
    });
    findings.check(
        coverage.is_some(),
        "AC4.10 Run with Coverage coloured nothing",
    );

    // AC4.9: a configuration missing a required field is refused with the
    // problem's sentence.
    select_config(&ide, "E8 broken");
    let mark = ide.mark();
    click_run_menu_item(&ide, "Run");
    let refusal = answer(&ide, mark, |e| {
        e["ev"] == "status_message" || e["ev"] == "test_run_refused"
    });
    findings.check(
        refusal.as_ref().is_some_and(|e| {
            e["text"]
                .as_str()
                .or(e["message"].as_str())
                .is_some_and(|t| t.contains("method"))
        }),
        format!("AC4.9: no refusal naming the method: {refusal:?}"),
    );
    findings.check(
        ide.events_since_of(mark, "test_run_started").is_empty(),
        "AC4.9: the broken configuration ran",
    );

    // AC6.1: a before-launch task runs first.
    let before = ide.project_root().join("before-launch.txt");
    select_config(&ide, "E8 before launch");
    let mark = ide.mark();
    click_run_menu_item(&ide, "Run");
    ide.wait_for_ev(mark, "test_run_started");
    findings.check(
        before.exists(),
        "AC6.1: the test run started before its before-launch task ran",
    );
    wait_for_test_run(&ide, mark);
    std::fs::remove_file(&before).ok();
    let mark = ide.mark();
    ide.focus_main();
    ide.key("shift+F9");
    ide.wait_for_ev(mark, "test_run_started");
    findings.check(
        before.exists(),
        "AC6.1: Debug skipped the before-launch task",
    );
    // The breakpoint AC4.10 left in `testPasses` stops this one too.
    let stop = ide.wait_for_event_within(mark, REAL_TOOLCHAIN_TIMEOUT, "a stop or the end", |e| {
        e["ev"] == "debug_stopped" || e["ev"] == "test_run_finished"
    });
    if stop["ev"] == "debug_stopped" {
        ide.key("F9");
    }
    wait_for_test_run(&ide, mark);

    // AC6.2: a failing before-launch task stops the launch, and says so.
    select_config(&ide, "E8 before launch fails");
    let mark = ide.mark();
    click_run_menu_item(&ide, "Run");
    let told = answer(&ide, mark, |e| {
        e["ev"] == "before_launch_failed"
            || e["ev"] == "status_message"
            || e["ev"] == "test_run_refused"
            || e["ev"] == "test_run_started"
    });
    findings.check(
        told.as_ref().is_some_and(|e| e["ev"] != "test_run_started"),
        format!("AC6.2: a failed before-launch task: {told:?}"),
    );

    // AC6.3: a script whose PHPUnit before-launch fails does not run.
    select_config(&ide, "E8 script after failing tests");
    let mark = ide.mark();
    click_run_menu_item(&ide, "Run");
    wait_for_test_run(&ide, mark);
    let script = answer(&ide, mark, |e| {
        e["ev"] == "run_console_output"
            && e["text"]
                .as_str()
                .is_some_and(|t| t.contains("console ran"))
    });
    findings.check(
        script.is_none(),
        "AC6.3: the script ran after its tests failed",
    );

    // AC5.5: without allow-parallel, a second launch stops the first and
    // runs once it ended.
    select_config(&ide, "E8 slow");
    let first = ide.mark();
    click_run_menu_item(&ide, "Run");
    ide.wait_for_event(first, "the slow test to start", |e| {
        e["ev"] == "test_tree_row" && e["name"] == "testWaits" && e["status"] == "running"
    });
    let second = ide.mark();
    click_run_menu_item(&ide, "Run");
    let restarted = answer(&ide, second, |e| e["ev"] == "test_run_started");
    findings.check(
        restarted.is_some(),
        "AC5.5: the second launch never started",
    );
    findings.check(
        !ide.events_since_of(second, "test_run_finished").is_empty()
            || !ide.events_since_of(second, "test_run_stopped").is_empty(),
        "AC5.5: the second launch started without ending the first",
    );
    e2e::wait_for("the second run's test to start", || {
        let events = ide.events_since(second);
        let started = events.iter().position(|e| e["ev"] == "test_run_started")?;
        events[started..]
            .iter()
            .any(|e| {
                e["ev"] == "test_tree_row" && e["name"] == "testWaits" && e["status"] == "running"
            })
            .then_some(())
    });

    // AC5.6: with allow-parallel, a launch while a run goes on is refused.
    select_config(&ide, "E8 slow parallel");
    let parallel = ide.mark();
    click_run_menu_item(&ide, "Run");
    let refused = answer(&ide, parallel, |e| {
        e["ev"] == "test_run_refused" || e["ev"] == "test_run_started"
    });
    findings.check(
        refused
            .as_ref()
            .is_some_and(|e| e["ev"] == "test_run_refused"),
        format!("AC5.6: a parallel launch: {refused:?}"),
    );

    // AC5.7: Stop (Ctrl+F2) ends the PHP process; the test is not finished.
    let stopping = ide.mark();
    ide.focus_main();
    ide.key("ctrl+F2");
    let ended = answer(&ide, stopping, |e| {
        e["ev"] == "test_run_stopped" || e["ev"] == "test_run_finished"
    });
    findings.check(ended.is_some(), "AC5.7: Ctrl+F2 did not stop the test run");
    if ended.is_none() {
        // The Tests dock's own Stop, so the flow can go on.
        let toolbar = ide
            .events()
            .into_iter()
            .rev()
            .find(|e| e["ev"] == "tests_toolbar_rects")
            .expect("the Tests dock's toolbar");
        let stop = toolbar["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "stop")
            .unwrap()
            .clone();
        let (x, y) = rect_centre(&stop["rect"]);
        ide.click_at(x, y, 1);
        let ended = answer(&ide, stopping, |e| {
            e["ev"] == "test_run_stopped" || e["ev"] == "test_run_finished"
        });
        findings.check(
            ended.is_some(),
            "AC5.7: the Tests dock's Stop did not stop the run either",
        );
    }
    let status = ide
        .events_since_of(stopping, "test_tree_row")
        .into_iter()
        .rev()
        .find(|e| e["name"] == "testWaits")
        .map(|e| e["status"].as_str().unwrap_or_default().to_string());
    findings.check(
        status.as_deref() != Some("passed") && status.as_deref() != Some("running"),
        format!("AC5.7: the stopped test is marked {status:?}"),
    );
    let php = Command::new("pgrep")
        .args(["-f", "SlowTest.php"])
        .output()
        .expect("pgrep");
    findings.check(
        !php.status.success(),
        format!(
            "AC5.7: PHP still runs after Stop: {}",
            String::from_utf8_lossy(&php.stdout)
        ),
    );
    if ide
        .events_since_of(stopping, "test_run_finished")
        .is_empty()
        && ide.events_since_of(stopping, "test_run_stopped").is_empty()
    {
        wait_for_test_run(&ide, stopping);
    }

    // AC5.4: an edited temporary wins over the next gutter run's derived one.
    reopen(&ide, "OutcomesTest.php");
    let outcomes = ide
        .events()
        .into_iter()
        .find(|e| e["ev"] == "tab_added" && e["title"] == "OutcomesTest.php")
        .and_then(|e| e["tab_id"].as_u64())
        .expect("the OutcomesTest.php tab");
    let mark = ide.mark();
    gutter(&ide, outcomes, OUTCOMES_CLASS, "Run 'OutcomesTest'");
    wait_for_test_run(&ide, mark);
    // The edit the configuration dialog would save: runner options on the
    // temporary the gutter run stored.
    let settings_path = ide.project_root().join(".ide/settings.toml");
    let stored = std::fs::read_to_string(&settings_path).expect("settings");
    // The id holds a backslash, so TOML writes it as a 'literal' string.
    let id = "phpunit:class:tests/Unit/OutcomesTest.php:";
    match stored.find(id) {
        Some(at) => {
            let table = at
                + stored[at..]
                    .find("[run_config.phpunit]\n")
                    .expect("its phpunit table");
            let insert = table + "[run_config.phpunit]\n".len();
            let edited = format!(
                "{}runner_options = \"--stop-on-failure\"\n{}",
                &stored[..insert],
                &stored[insert..]
            );
            std::fs::write(&settings_path, edited).expect("editing the temporary");
            let mark = ide.mark();
            gutter(&ide, outcomes, OUTCOMES_CLASS, "Run 'OutcomesTest'");
            let (_, tests) = run_and_collect(&ide, mark);
            findings.check(
                names(&tests) == ["testFails", "testPasses"],
                format!("AC5.4: the edited temporary's --stop-on-failure was not used: {tests:?}"),
            );
        }
        None => findings.check(false, format!("AC5.4: no class temporary in {stored}")),
    }

    // #457: Save in the Run Configurations dialog refreshes the combo.
    check_save_refreshes_combo(&ide, &mut findings);

    ide.focus_main();
    assert_eq!(ide.quit(), 0);
    findings.finish();
}
