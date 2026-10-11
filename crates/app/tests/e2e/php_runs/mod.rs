//! E8 (the PHPUnit editor runs plan): the acceptance flows of
//! `docs/architecture/phpunit-editor-runs-acceptance.md`, on Linux, against
//! real PHPUnit 11 and Pest 3 in the `linux-php` image.
//!
//! Gated on `IDE_E2E_PHP=1` like `php_real` (`make test-php`). Every flow
//! launches over a fresh copy of the `php_suites` fixture with
//! `composer install` run in its root and in the two packages that have
//! their own test framework (`packages/billing`, `packages/pest`). The flows
//! are grouped as the spec's §4 suggests, so a handful of launches cover the
//! journeys; each assertion names the AC it proves. This module holds the
//! fixture and the helpers; each child module holds one launch group.

mod caret_and_gutter;
mod configurations;
mod go_to_test;
mod project_tree;
mod tests_dock;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use e2e::{Ide, Mark};
use serde_json::Value;

use crate::php_real::{
    choose_gutter_action, copy_fixture, launch_php, open_view_item, require_php_e2e,
    toggle_breakpoint_at, wait_for_stop_at, wait_for_test_run, write_project_settings,
    REAL_TOOLCHAIN_TIMEOUT,
};
use crate::run::click_run_menu_item;
use crate::support::{accept_top_hit, fixture, open_file, open_search_popup, rect_centre, shot};

// --- the fixture ----------------------------------------------------------

/// `OutcomesTest.php`'s marker lines, 0-based.
const OUTCOMES_CLASS: u64 = 8;
const TEST_PASSES: u64 = 10;
const TEST_FAILS: u64 = 16;
const TEST_SKIPPED: u64 = 21;
const TEST_ERRORS: u64 = 26;

/// `DataSetsTest.php`'s method lines, 0-based.
const TEST_NAMED: u64 = 26;
const TEST_UNNAMED: u64 = 32;

/// `packages/pest/tests/Feature/NestedTest.php`, 0-based.
const PEST_IT_ADDS: u64 = 6;

/// A fresh `php_suites` with its three Composer installs done.
fn suites_project() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp PHP project");
    copy_fixture(&fixture("php_suites"), dir.path());
    for package in [".", "packages/billing", "packages/pest"] {
        composer_install(&dir.path().join(package));
    }
    dir
}

fn composer_install(dir: &Path) {
    let output = Command::new("composer")
        .args(["install", "--no-interaction", "--no-progress", "--quiet"])
        .current_dir(dir)
        .output()
        .expect("running composer");
    assert!(
        output.status.success(),
        "composer install in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Launch over a fresh `php_suites`, with `settings` as its project
/// settings when given, and wait for the project to open.
fn launch_suites(name: &str, settings: Option<&str>) -> Ide {
    let project = suites_project();
    if let Some(toml) = settings {
        write_project_settings(project.path(), toml);
    }
    let mut ide = launch_php(name, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    // Without a window manager the window opens at 1280x800, where a test
    // file's later markers and the Tests dock's rows fall off screen: come
    // back at the screen's size.
    assert_eq!(ide.quit(), 0);
    let mut global = app_config::load(&ide.config_dir()).expect("settings");
    global.window_geometry = app_config::WindowGeometry {
        x: 0,
        y: 0,
        width: 1600,
        height: 1160,
    };
    app_config::save(&ide.config_dir(), &global).expect("the window size");
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
    ide
}

/// Fold the Tests dock row called `name` (a suite) so later rows stay on
/// screen; the mark its new layout is reported after.
fn fold_row(ide: &Ide, mark: Mark, name: &str) {
    let (x, y) = tests_row(ide, mark, name, |n| n == name);
    ide.click_at(x, y, 1);
    let folded = ide.mark();
    ide.key("Left");
    ide.wait_for_ev(folded, "test_tree_rects");
}

/// The ACs a flow found broken. A failed AC is recorded rather than ending
/// the flow, so one defect does not hide the ACs after it; the flow still
/// fails, listing every one, once it reached its end ([`Findings::finish`]).
#[derive(Default)]
struct Findings(Vec<String>);

impl Findings {
    fn check(&mut self, ok: bool, what: impl Into<String>) {
        if !ok {
            let what = what.into();
            eprintln!("AC FAILED: {what}");
            self.0.push(what);
        }
    }

    fn finish(self) {
        assert!(self.0.is_empty(), "failed ACs:\n{}", self.0.join("\n"));
    }
}

// --- the editor -------------------------------------------------------------

/// Open `name` through Go to File; its tab id.
fn open(ide: &Ide, name: &str) -> u64 {
    open_file(ide, name)["tab_id"].as_u64().expect("tab_id")
}

/// Bring the already open `name` to the front through Go to File.
fn reopen(ide: &Ide, name: &str) {
    let mark = open_search_popup(ide, "ctrl+shift+n");
    accept_top_hit(ide, mark, name);
}

/// Choose the `prefix` entry of the gutter menu on 0-based `line` of the
/// current editor, tab `tab_id`, scrolled into view first: the window is
/// 1280x800 under bare Xvfb, so a test file's later markers are off screen.
fn gutter(ide: &Ide, tab_id: u64, line: u64, prefix: &str) {
    caret_to(ide, line as usize + 1);
    choose_gutter_action(ide, tab_id, line, prefix);
}

/// Put the caret at the start of 1-based `line` of the current editor.
fn caret_to(ide: &Ide, line: usize) {
    ide.focus_main();
    ide.key("ctrl+Home");
    for _ in 1..line {
        ide.key("Down");
    }
}

/// The gutter's statuses of tab `tab_id` as last reported after `mark`:
/// 0-based line → (status, tooltip).
fn marker_statuses(ide: &Ide, mark: Mark, tab_id: u64) -> Option<BTreeMap<u64, (String, String)>> {
    let event = ide
        .events_since_of(mark, "run_marker_statuses")
        .into_iter()
        .rev()
        .find(|e| e["tab_id"].as_u64() == Some(tab_id))?;
    Some(
        event["markers"]
            .as_array()
            .expect("markers")
            .iter()
            .map(|m| {
                (
                    m["line"].as_u64().expect("line"),
                    (
                        m["status"].as_str().unwrap_or_default().to_string(),
                        m["tooltip"].as_str().unwrap_or_default().to_string(),
                    ),
                )
            })
            .collect(),
    )
}

/// Wait until tab `tab_id`'s gutter reports `line` as `status`.
fn wait_for_marker(
    ide: &Ide,
    mark: Mark,
    tab_id: u64,
    line: u64,
    status: &str,
) -> (String, String) {
    e2e::wait_for(
        &format!("line {line} of tab {tab_id} to show `{status}`"),
        || {
            marker_statuses(ide, mark, tab_id)?
                .remove(&line)
                .filter(|(s, _)| s == status)
        },
    )
}

// --- the Tests dock -----------------------------------------------------------

/// Each test (not suite) the run after `mark` ran, by name, with its last
/// status. A narrowed run updates the tree an earlier run left rather than
/// replacing it (`test_core::preparation::replaces_tree`), so a test counts
/// as run when its row went through `running` after `mark`.
fn ran_tests(ide: &Ide, mark: Mark) -> BTreeMap<String, String> {
    let rows: Vec<Value> = ide
        .events_since_of(mark, "test_tree_row")
        .into_iter()
        .filter(|row| row["suite"] == false)
        .collect();
    let mut tests = BTreeMap::new();
    for row in &rows {
        let id = row["id"].as_str().unwrap_or_default();
        let ran = rows
            .iter()
            .any(|r| r["id"] == id && r["status"] == "running");
        if ran {
            tests.insert(
                row["name"].as_str().unwrap_or_default().to_string(),
                row["status"].as_str().unwrap_or_default().to_string(),
            );
        }
    }
    tests
}

/// The tests the run started after `mark` reported, once it finished.
fn run_and_collect(ide: &Ide, mark: Mark) -> (Value, BTreeMap<String, String>) {
    let finished = wait_for_test_run(ide, mark);
    (finished, ran_tests(ide, mark))
}

fn names(tests: &BTreeMap<String, String>) -> Vec<&str> {
    tests.keys().map(String::as_str).collect()
}

/// The toolbar combo's selection as last reported after `mark`.
fn selected_config(ide: &Ide, mark: Mark) -> Option<Value> {
    ide.events_since_of(mark, "run_config_selected")
        .into_iter()
        .last()
}

/// Wait until the combo selects the configuration called `name`.
fn wait_for_selected(ide: &Ide, mark: Mark, name: &str) -> Value {
    ide.wait_for_event(mark, &format!("the combo to select `{name}`"), |e| {
        e["ev"] == "run_config_selected" && e["name"] == name
    })
}

/// The status-bar sentence shown after `mark` that contains `needle`.
fn wait_for_status(ide: &Ide, mark: Mark, needle: &str) -> String {
    ide.wait_for_event(
        mark,
        &format!("a status message containing `{needle}`"),
        |e| e["ev"] == "status_message" && e["text"].as_str().is_some_and(|t| t.contains(needle)),
    )["text"]
        .as_str()
        .unwrap()
        .to_string()
}

// --- the gutter ------------------------------------------------------------

/// The gutter's Run lines of `file` as last reported after `mark`.
fn wait_for_gutter_lines(ide: &Ide, mark: Mark, file: &str) -> Vec<u64> {
    let event = ide.wait_for_event(mark, &format!("the gutter lines of {file}"), |e| {
        e["ev"] == "gutter_run_lines"
            && e["path"].as_str().is_some_and(|p| p.ends_with(file))
            && e["lines"].as_array().is_some_and(|l| !l.is_empty())
    });
    let mut lines: Vec<u64> = event["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    lines.sort_unstable();
    lines
}

/// Open the gutter menu of `line` and return its entries' labels, then
/// close it again.
fn gutter_menu_labels(ide: &Ide, tab_id: u64, line: u64) -> Vec<String> {
    caret_to(ide, line as usize + 1);
    let rect = latest_run_icon(ide, tab_id, line);
    let (x, y) = rect_centre(&rect);
    let mark = ide.mark();
    ide.click_at(x, y, 1);
    ide.wait_for_ev(mark, "run_gutter_menu_action");
    // Every entry is marked in one go once the menu is laid out.
    let labels = ide
        .events_since_of(mark, "run_gutter_menu_action")
        .iter()
        .filter_map(|e| e["label"].as_str().map(str::to_string))
        .collect();
    ide.key("Escape");
    ide.focus_main();
    labels
}

fn latest_run_icon(ide: &Ide, tab_id: u64, line: u64) -> Value {
    e2e::wait_for(
        &format!("a run icon on line {line} of tab {tab_id}"),
        || {
            ide.events()
                .into_iter()
                .rev()
                .find(|e| {
                    e["ev"] == "run_icon"
                        && e["tab_id"].as_u64() == Some(tab_id)
                        && e["line"].as_u64() == Some(line)
                })
                .map(|e| e["rect"].clone())
        },
    )
}

/// Hover the Run icon on `line` and return the tooltip it shows.
fn hover_marker(ide: &Ide, tab_id: u64, line: u64) -> Value {
    caret_to(ide, line as usize + 1);
    let rect = latest_run_icon(ide, tab_id, line);
    let (x, y) = rect_centre(&rect);
    let mark = ide.mark();
    // Two moves: Qt only sees motion, and the first may land as the enter.
    ide.mouse_move(x + 1, y);
    ide.mouse_move(x, y);
    ide.wait_for_event(mark, &format!("the tooltip of line {line}"), |e| {
        e["ev"] == "run_marker_tooltip"
            && e["tab_id"].as_u64() == Some(tab_id)
            && e["line"].as_u64() == Some(line)
    })
}

// --- the Tests dock's rows ---------------------------------------------------

/// The latest Tests dock row layout reported after `mark` that has a row
/// whose name satisfies `pick`, and that row.
fn latest_row(
    ide: &Ide,
    mark: Mark,
    what: &str,
    pick: &impl Fn(&str) -> bool,
) -> (Vec<Value>, Value) {
    let has_row = |e: &Value| {
        e["ev"] == "test_tree_rects"
            && e["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|r| r["name"].as_str().is_some_and(pick)))
    };
    ide.wait_for_event(mark, &format!("the Tests dock row {what}"), has_row);
    let rects = ide
        .events_since(mark)
        .into_iter()
        .rev()
        .find(|e| has_row(e))
        .unwrap();
    let rows = rects["rows"].as_array().unwrap().clone();
    let row = rows
        .iter()
        .find(|r| r["name"].as_str().is_some_and(pick))
        .unwrap()
        .clone();
    (rows, row)
}

/// The on-screen centre of the Tests dock row whose name satisfies `pick`.
/// The dock shows a handful of rows, so a row scrolled out of view is
/// reached the way a user reaches it: click a visible row, then walk the
/// selection with Down until it is the one (the tree scrolls along).
fn tests_row(ide: &Ide, mark: Mark, what: &str, pick: impl Fn(&str) -> bool) -> (i32, i32) {
    let shown = ide
        .events()
        .into_iter()
        .rev()
        .find(|e| e["ev"] == "test_tree_rects")
        .is_some_and(|e| e["shown"] == true);
    let mark = if shown {
        mark
    } else {
        // Another tab of the bottom area is in front: bring the dock back.
        let raised = open_view_item(ide, "Tests");
        ide.wait_for_event(raised, "the Tests dock in front", |e| {
            e["ev"] == "test_tree_rects" && e["shown"] == true
        });
        raised
    };
    let (rows, row) = latest_row(ide, mark, what, &pick);
    if row["visible"] == true {
        return rect_centre(&row["rect"]);
    }
    // The visible row nearest the target, and the way to walk from it.
    let target_y = row["rect"][1].as_i64().unwrap_or(0);
    let first = rows
        .iter()
        .filter(|r| r["visible"] == true)
        .min_by_key(|r| (r["rect"][1].as_i64().unwrap_or(0) - target_y).abs())
        .expect("a visible Tests dock row");
    let key = if target_y < first["rect"][1].as_i64().unwrap_or(0) {
        "Up"
    } else {
        "Down"
    };
    let (x, y) = rect_centre(&first["rect"]);
    ide.click_at(x, y, 1);
    let target = row["id"].clone();
    for _ in 0..40 {
        let moved = ide.mark();
        ide.key(key);
        let selected = ide.wait_for_ev(moved, "test_failure_details");
        if selected["id"] == target {
            let (_, row) = latest_row(ide, moved, what, &pick);
            assert_eq!(row["visible"], true, "{what} is selected but not shown");
            return rect_centre(&row["rect"]);
        }
    }
    panic!("{what} could not be reached with {key}");
}

/// How long a gesture the product may not answer at all gets before the AC
/// is recorded as failed: a bound on a hang, never a speed assertion.
const NO_ANSWER: std::time::Duration = std::time::Duration::from_secs(15);

/// The first marker after `mark` that `predicate` accepts, or `None` once
/// [`NO_ANSWER`] passed — for an AC a defect can leave unanswered, which is
/// recorded as a finding instead of ending the flow.
fn answer(ide: &Ide, mark: Mark, predicate: impl Fn(&Value) -> bool) -> Option<Value> {
    let deadline = std::time::Instant::now() + NO_ANSWER;
    loop {
        if let Some(found) = ide.events_since(mark).into_iter().find(|e| predicate(e)) {
            return Some(found);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The caret line a jump after `mark` landed on in a file ending in `file`,
/// or `None` when no jump came.
fn landing(ide: &Ide, mark: Mark, file: &str) -> Option<u64> {
    answer(ide, mark, |e| {
        e["ev"] == "opened_at" && e["path"].as_str().is_some_and(|p| p.ends_with(file))
    })
    .and_then(|e| e["caret_line"].as_u64())
}

/// Press the Tests dock's `name` toolbar button (`run_all`, `run_failed`,
/// `stop`) and return the tests the run it started ran.
fn press_dock_button(ide: &Ide, name: &str) -> (Value, BTreeMap<String, String>) {
    // Any row: brings the dock to the front when another tab hides it.
    tests_row(ide, Mark::start(), "any row", |_| true);
    let toolbar = ide
        .events()
        .into_iter()
        .rev()
        .find(|e| e["ev"] == "tests_toolbar_rects")
        .expect("the Tests dock's toolbar");
    let button = toolbar["buttons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == name)
        .unwrap()
        .clone();
    let (x, y) = rect_centre(&button["rect"]);
    let mark = ide.mark();
    ide.click_at(x, y, 1);
    run_and_collect(ide, mark)
}

/// Run `prefix` from the gutter, then show the Tests dock with the rows the
/// run left; the mark taken before the run.
fn gutter_run_then_dock(ide: &Ide, tab_id: u64, line: u64, prefix: &str) -> Mark {
    let mark = ide.mark();
    gutter(ide, tab_id, line, prefix);
    wait_for_test_run(ide, mark);
    mark
}
