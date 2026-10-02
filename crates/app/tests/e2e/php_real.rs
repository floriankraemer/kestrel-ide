//! E3 (the PHP parity plan, ADR-0066): nightly end-to-end flows against the
//! *real* PHP toolchain — PHP 8.3, Composer, Intelephense, Phpactor, Xdebug
//! and vscode-php-debug — never part of the per-PR `e2e-ci` budget
//! (ADR-0057 §6's budget note applies here too).
//!
//! Gated at runtime on `IDE_E2E_PHP=1`, which `make php-ci` exports inside
//! the `linux-php` image, the only place those tools are on `PATH`. Every
//! flow gets its own `Ide::launch` over a fresh copy of the `php_app`
//! fixture with a real `composer install` (from the image's prewarmed
//! cache, symlinks and all) already run in it.

use std::path::{Path, PathBuf};
use std::process::Command;

use e2e::{Ide, Mark};
use serde_json::Value;

use crate::run::click_run_menu_item;
use crate::support::{buffer, fixture, open_file, rect_centre, wait_for_index, APP};

/// `e2e::wait::DEFAULT_TIMEOUT` is tight for a real Composer, Intelephense
/// index or PHPStan run in Docker, so every wait on one of those gets this
/// ceiling via `Ide::wait_for_event_within`.
const REAL_TOOLCHAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Skip (never fail) unless `IDE_E2E_PHP=1` — set by `make php-ci` inside
/// the `linux-php` image. Called first thing in every `#[test]` below.
fn require_php_e2e() -> bool {
    let enabled = std::env::var("IDE_E2E_PHP").as_deref() == Ok("1");
    if !enabled {
        eprintln!(
            "skipping: set IDE_E2E_PHP=1 to run (needs real PHP, Composer and language servers — `make test-php`, inside the `linux-php` image)"
        );
    }
    enabled
}

/// A fresh copy of the `php_app` fixture with `composer install` run in it.
fn php_project() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp PHP project");
    copy_fixture(&fixture("php_app"), dir.path());
    let output = Command::new("composer")
        .args(["install", "--no-interaction", "--no-progress", "--quiet"])
        .current_dir(dir.path())
        .output()
        .expect("running composer");
    assert!(
        output.status.success(),
        "composer install failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    dir
}

fn copy_fixture(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let entry = entry.expect("fixture entry");
        let target: PathBuf = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            std::fs::create_dir_all(&target).expect("fixture dir");
            copy_fixture(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("fixture file");
        }
    }
}

/// Launch the IDE over `project` with this process's `HOME` (the image's
/// `/opt/vscode-home`) rather than the harness' throwaway one, so the PHP
/// debug adapter's `$HOME/.vscode/extensions` auto-locate finds
/// vscode-php-debug.
fn launch_php(name: &str, project: &Path) -> Ide {
    let home = std::env::var("HOME").expect("HOME");
    Ide::launch_with_env(name, APP, project, &[("HOME", home.as_str())])
}

/// Write the project's own `.ide/settings.toml` before the app launches.
fn write_project_settings(project: &Path, toml: &str) {
    std::fs::create_dir_all(project.join(".ide")).expect(".ide");
    std::fs::write(project.join(".ide/settings.toml"), toml).expect("project settings");
}

/// Wait for a Problems row from `source` on `file` after `mark`.
fn wait_for_problem(ide: &Ide, mark: Mark, source: &str, file: &str) -> Value {
    ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        &format!("a `{source}` problem on {file}"),
        |e| {
            e["ev"] == "problem_row"
                && e["source"] == source
                && e["path"].as_str().is_some_and(|p| p.ends_with(file))
        },
    )
}

/// Show a dock through the View menu (most docks have no default shortcut)
/// and focus the main window again.
fn open_view_item(ide: &Ide, label: &str) -> Mark {
    let mark = ide.mark();
    ide.key("alt+v");
    let item = ide.wait_for_event(mark, &format!("the {label} item in the View menu"), |e| {
        e["ev"] == "view_menu_action" && e["label"] == label
    });
    let (x, y) = rect_centre(&item["rect"]);
    ide.click_at(x, y, 1);
    mark
}

/// Open the Composer dock and return its rows once it reports some.
fn open_composer_dock(ide: &Ide) -> Vec<Value> {
    let mark = open_view_item(ide, "Composer");
    let rows = ide.wait_for_event(mark, "the Composer dock's rows", |e| {
        e["ev"] == "composer_rows" && e["rows"].as_array().is_some_and(|r| !r.is_empty())
    });
    ide.focus_main();
    rows["rows"].as_array().cloned().expect("rows array")
}

/// Flow a: the real servers, analyzers, navigation, formatting and the
/// Composer dock, against a project `composer install` really populated.
///
/// - Intelephense and PHP_CodeSniffer report on open; PHPStan (saved-only)
///   reports after a save, under the file's absolute path although PHPStan
///   prints it relative (followups: "PHPStan relative paths").
/// - Ctrl+B on the `implements` target opens the interface through the
///   real language servers.
/// - Reformat Code runs the real php-cs-fixer on a dotfile temp copy that
///   honours the project's `.php-cs-fixer.dist.php`.
/// - The Composer dock lists the locked dev packages as installed.
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_php_real_servers_analysis_navigate_format() {
    if !require_php_e2e() {
        return;
    }
    let name = "e2e_php_real_servers_analysis_navigate_format";
    let project = php_project();
    write_project_settings(project.path(), "[php]\nformatter = \"php-cs-fixer\"\n");
    let mut ide = launch_php(name, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    let mcp = ide.mcp();
    wait_for_index(&mcp);

    let mark = ide.mark();
    let tab = open_file(&ide, "Greeter.php");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    wait_for_problem(&ide, mark, "intelephense", "src/Greeter.php");
    wait_for_problem(&ide, mark, "PHP_CodeSniffer", "src/Greeter.php");

    // PHPStan runs on save and reports the planted return-type error.
    let mark = ide.mark();
    ide.focus_main();
    ide.key("ctrl+End");
    ide.key("ctrl+s");
    let row = wait_for_problem(&ide, mark, "PHPStan", "src/Greeter.php");
    assert!(row["path"].as_str().unwrap().starts_with('/'), "{row}");

    // Format: Reformat Code.
    ide.key("ctrl+alt+l");
    let mcp_ref = &mcp;
    e2e::wait_for("the buffer to be formatted by php-cs-fixer", || {
        let text = buffer(mcp_ref, tab_id);
        (text.contains("whisper(string $name): string")
            && text.contains("\n        return strtolower"))
        .then_some(())
    });
    ide.key("ctrl+s");
    e2e::wait_for("the formatted file to reach the disk", || {
        ide.read_project_file("src/Greeter.php")
            .contains("whisper(string $name): string")
            .then_some(())
    });

    // Composer dock.
    let rows = open_composer_dock(&ide);
    let phpunit = rows
        .iter()
        .find(|r| r["name"] == "phpunit/phpunit")
        .unwrap_or_else(|| panic!("no phpunit/phpunit row in {rows:?}"));
    assert_eq!(phpunit["installed"], true, "{phpunit}");
    assert_eq!(phpunit["kind"], "dev-package", "{phpunit}");

    // Navigate: the caret at the end of `implements GreeterInterface`.
    ide.key("ctrl+Home");
    for _ in 0..6 {
        ide.key("Down");
    }
    ide.key("End");
    let mark = ide.mark();
    ide.key("ctrl+b");
    ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        "a tab for GreeterInterface.php",
        |e| e["ev"] == "tab_added" && e["title"] == "GreeterInterface.php",
    );

    assert_eq!(ide.quit(), 0);
}

/// Put a breakpoint on 1-based `line` of the open file's tab and wait until
/// it reaches the gutter.
fn toggle_breakpoint_at(ide: &Ide, line: usize) {
    ide.focus_main();
    ide.key("ctrl+Home");
    for _ in 1..line {
        ide.key("Down");
    }
    let mark = ide.mark();
    ide.key("ctrl+F8"); // debug.toggleBreakpoint
    ide.wait_for_event(mark, "the breakpoint to reach the gutter", |e| {
        e["ev"] == "breakpoints_applied" && e["count"].as_u64().unwrap_or(0) >= 1
    });
}

/// Wait for a stop at `line`, then for its variables.
fn wait_for_stop_at(ide: &Ide, mark: Mark, line: u64) -> Value {
    let stopped = ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        &format!("a stop at line {line}"),
        |e| e["ev"] == "debug_stopped" && e["line"].as_u64() == Some(line),
    );
    ide.wait_for_event(mark, "the variables to arrive", |e| {
        e["ev"] == "debug_variables" && e["count"].as_u64().unwrap_or(0) > 0
    });
    stopped
}

/// Flow b1: a real Xdebug CLI session. Debugging a run configuration starts
/// vscode-php-debug listening, then `php bin/console.php` with the Xdebug
/// environment dialling back; the breakpoint stops, resume ends the run.
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_php_real_xdebug_cli_breakpoint() {
    if !require_php_e2e() {
        return;
    }
    let name = "e2e_php_real_xdebug_cli_breakpoint";
    let project = php_project();
    write_project_settings(
        project.path(),
        "[[run_config]]\nid = \"php-cli\"\nname = \"console script\"\nprogram = \"php\"\nargs = [\"bin/console.php\", \"Xdebug\"]\ntoolchain = \"php\"\n",
    );
    let mut ide = launch_php(name, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    ide.wait_for_event(
        Mark::start(),
        "the run configurations to reach the toolbar",
        |e| e["ev"] == "run_configurations_changed" && e["count"].as_u64().unwrap_or(0) >= 1,
    );
    open_file(&ide, "console.php");
    toggle_breakpoint_at(&ide, 9);

    let mark = ide.mark();
    ide.key("shift+F9"); // debug.debug
    let started = ide.wait_for_event(mark, "the session to start", |e| e["ev"] == "debug_started");
    let session_id = started["session_id"].as_u64().expect("session_id");
    wait_for_stop_at(&ide, mark, 9);

    // Resume: the script finishes. The listener stays up for the next
    // connection (ADR-0069), so the session itself does not end here.
    let mark = ide.mark();
    ide.key("F9"); // debug.resume
    let finished =
        ide.wait_for_event_within(mark, REAL_TOOLCHAIN_TIMEOUT, "the script to finish", |e| {
            e["ev"] == "run_console_finished"
        });
    assert_eq!(finished["exit_code"], 0, "{finished}");
    let output: String = ide
        .events_since_of(mark, "run_console_output")
        .iter()
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert!(output.contains("Hello, Xdebug!"), "{output:?}");
    assert!(
        ide.events_since_of(mark, "debug_terminated").is_empty(),
        "session {session_id} ended with the run although the listener stays up"
    );
    assert_eq!(ide.quit(), 0);
}

/// A TCP port nothing listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Kills the child on drop so a failed flow never leaves `php -S` behind.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `php -S` on `port` serving `public/`, with Xdebug dialling the IDE's
/// default port when a request carries `XDEBUG_TRIGGER`.
fn spawn_php_server(root: &Path, port: u16) -> KillOnDrop {
    let child = Command::new("php")
        .args([
            "-dxdebug.mode=debug",
            "-dxdebug.start_with_request=trigger",
            "-dxdebug.client_host=127.0.0.1",
            "-dxdebug.client_port=9003",
            "-S",
            &format!("127.0.0.1:{port}"),
            "-t",
            "public",
        ])
        .current_dir(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawning php -S");
    e2e::wait_for("php -S to accept connections", || {
        std::net::TcpStream::connect(("127.0.0.1", port))
            .ok()
            .map(|_| ())
    });
    KillOnDrop(child)
}

/// A request that Xdebug debugs; the answer body, once the request ends.
fn get_with_trigger(port: u16, query: &str) -> std::thread::JoinHandle<String> {
    let url = format!("http://127.0.0.1:{port}/?XDEBUG_TRIGGER=1&{query}");
    std::thread::spawn(move || {
        let output = Command::new("curl")
            .args(["-s", "--max-time", "120", &url])
            .output()
            .expect("running curl");
        String::from_utf8_lossy(&output.stdout).into_owned()
    })
}

/// Flow b2: the listen toggle against `php -S`. One listen session sees two
/// triggered requests in turn: each stops at the breakpoint in
/// `public/index.php`, with no second `debug_started`, so vscode-php-debug
/// keeps the session alive when a connection ends.
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_php_real_listen_session_serves_triggered_requests() {
    if !require_php_e2e() {
        return;
    }
    const LISTEN: &str = "Start Listening for PHP Debug Connections";
    let name = "e2e_php_real_listen_session_serves_triggered_requests";
    let project = php_project();
    let mut ide = launch_php(name, project.path());
    let root = ide.project_root().to_path_buf();
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    open_file(&ide, "index.php");
    toggle_breakpoint_at(&ide, 9);

    let mark = ide.mark();
    assert!(
        !click_run_menu_item(&ide, LISTEN),
        "listening from the start"
    );
    let started = ide.wait_for_event(mark, "the listen session to start", |e| {
        e["ev"] == "debug_started"
    });
    let session_id = started["session_id"].as_u64().expect("session_id");
    let port = free_port();
    let _server = spawn_php_server(&root, port);

    // First request.
    let mark = ide.mark();
    let first = get_with_trigger(port, "name=One");
    wait_for_stop_at(&ide, mark, 9);
    let mark = ide.mark();
    ide.key("F9"); // debug.resume
    assert_eq!(first.join().expect("curl"), "Hello, One!");

    // Second request, same session.
    let second = get_with_trigger(port, "name=Two");
    wait_for_stop_at(&ide, mark, 9);
    assert!(
        ide.events_since_of(mark, "debug_started").is_empty(),
        "the second connection opened a second session"
    );
    assert!(
        ide.events_since_of(mark, "debug_terminated").is_empty(),
        "the first connection ending terminated the listen session"
    );
    ide.key("F9");
    assert_eq!(second.join().expect("curl"), "Hello, Two!");

    let mark = ide.mark();
    assert!(
        click_run_menu_item(&ide, LISTEN),
        "the toggle shows it is on"
    );
    ide.wait_for_event(mark, "the listen session to end", |e| {
        e["ev"] == "debug_terminated" && e["session_id"].as_u64() == Some(session_id)
    });
    assert_eq!(ide.quit(), 0);
}

/// Click the gutter's Run icon on 0-based `line` of tab `tab_id` (its latest
/// reported position), then the popup entry whose label starts with
/// `prefix`.
fn choose_gutter_action(ide: &Ide, tab_id: u64, line: u64, prefix: &str) {
    let rect = e2e::wait_for(
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
    );
    let (x, y) = rect_centre(&rect);
    let mark = ide.mark();
    ide.click_at(x, y, 1);
    let item = ide.wait_for_event(mark, &format!("the `{prefix}` gutter entry"), |e| {
        e["ev"] == "run_gutter_menu_action"
            && e["label"].as_str().is_some_and(|l| l.starts_with(prefix))
    });
    let (x, y) = rect_centre(&item["rect"]);
    ide.click_at(x, y, 1);
    ide.focus_main();
}

/// Wait for the test run that started after `mark` to finish.
fn wait_for_test_run(ide: &Ide, mark: Mark) -> Value {
    ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        "the test run to finish",
        |e| e["ev"] == "test_run_finished",
    )
}

/// The last reported status of the test row called `name` after `mark`
/// (the tree is rebuilt on every TeamCity event, so only the last mark of a
/// row is its settled state).
fn test_status(ide: &Ide, mark: Mark, name: &str) -> Option<String> {
    ide.events_since_of(mark, "test_tree_row")
        .iter()
        .rev()
        .find(|e| e["name"] == name)
        .and_then(|e| e["status"].as_str().map(str::to_string))
}

/// Flow c1: a test run from the editor gutter, a namespaced rerun from the
/// Tests dock, and a debug run from the gutter.
///
/// - `ctrl+shift+F10` on `testGreetsByName` offers Run, Debug and Run with
///   Coverage; Run passes on the real framework (Pest, which runs the
///   PHPUnit class and takes the `Tests\GreeterTest::testGreetsByName`
///   filter).
/// - Right-click > Rerun Test on that row in the Tests dock reruns it with a
///   namespaced filter (the backslashes survive).
/// - Debug stops at a breakpoint inside the test, under real Xdebug.
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_php_real_gutter_test_run_rerun_and_debug() {
    if !require_php_e2e() {
        return;
    }
    let name = "e2e_php_real_gutter_test_run_rerun_and_debug";
    let project = php_project();
    let mut ide = launch_php(name, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    let test_tab = open_file(&ide, "GreeterTest.php")["tab_id"]
        .as_u64()
        .expect("tab_id");

    // The gutter marks the class and its methods.
    let mark = ide.mark();
    choose_gutter_action(&ide, test_tab, 11, "Run 'testGreetsByName'");
    let finished = wait_for_test_run(&ide, mark);
    assert_eq!(finished["ok"], true, "{finished}");
    assert_eq!(
        test_status(&ide, mark, "Greets by name").as_deref(),
        Some("passed"),
        "{:?}",
        ide.events_since_of(mark, "test_tree_row")
    );

    // A breakpoint inside the test body, set while the editor still has the
    // keyboard focus (the Tests dock takes it).
    toggle_breakpoint_at(&ide, 15);

    // Rerun from the Tests dock's context menu.
    let mark = open_view_item(&ide, "Tests");
    let rects = ide.wait_for_event(mark, "the Tests dock's rows laid out", |e| {
        e["ev"] == "test_tree_rects" && e["rows"].as_array().is_some_and(|r| !r.is_empty())
    });
    let row = rects["rows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["name"] == "Greets by name"))
        .unwrap_or_else(|| panic!("no test row in {rects}"));
    let (x, y) = rect_centre(&row["rect"]);
    let mark = ide.mark();
    ide.click_at(x, y, 3);
    let rerun = ide.wait_for_event(mark, "the Rerun Test entry", |e| {
        e["ev"] == "tests_menu_action" && e["label"] == "Rerun Test"
    });
    assert_eq!(rerun["enabled"], true, "{rerun}");
    let (x, y) = rect_centre(&rerun["rect"]);
    ide.click_at(x, y, 1);
    let finished = wait_for_test_run(&ide, mark);
    assert_eq!(finished["ok"], true, "{finished}");
    assert_eq!(
        test_status(&ide, mark, "Greets by name").as_deref(),
        Some("passed")
    );
    ide.focus_main();

    // Debug from the gutter: the breakpoint set above stops it.
    let mark = ide.mark();
    choose_gutter_action(&ide, test_tab, 11, "Debug 'testGreetsByName'");
    wait_for_stop_at(&ide, mark, 15);
    let mark = ide.mark();
    ide.key("F9"); // debug.resume
    let finished = wait_for_test_run(&ide, mark);
    assert_eq!(finished["ok"], true, "{finished}");

    assert_eq!(ide.quit(), 0);
}

/// Flow c2: coverage and the Pest marker.
///
/// - "Run with Coverage" on a PHPUnit test colours `src/Greeter.php`: `greet`
///   is covered, the planted `shout` is not.
/// - A Pest `it(...)` line in a Pest file gets its own gutter marker and
///   runs under the real Pest filter shape.
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_php_real_coverage_and_pest_marker() {
    if !require_php_e2e() {
        return;
    }
    let name = "e2e_php_real_coverage_and_pest_marker";
    let project = php_project();
    let mut ide = launch_php(name, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    open_file(&ide, "Greeter.php");
    let test_tab = open_file(&ide, "GreeterTest.php")["tab_id"]
        .as_u64()
        .expect("tab_id");

    let mark = ide.mark();
    choose_gutter_action(&ide, test_tab, 11, "Run 'testGreetsByName' with Coverage");
    let finished = wait_for_test_run(&ide, mark);
    assert_eq!(finished["ok"], true, "{finished}");
    let coverage = ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        "coverage for Greeter.php",
        |e| {
            e["ev"] == "coverage_lines"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("src/Greeter.php"))
        },
    );
    let lines = |key: &str| -> Vec<u64> {
        coverage[key]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default()
    };
    // `greet`'s body is line 15 (1-based) and `shout`'s line 21, which the
    // test never calls; the marks are 0-based line numbers.
    assert!(lines("covered").contains(&14), "{coverage}");
    assert!(lines("uncovered").contains(&20), "{coverage}");

    // The Pest marker.
    let pest_tab = open_file(&ide, "GreeterPestTest.php")["tab_id"]
        .as_u64()
        .expect("tab_id");
    let mark = ide.mark();
    choose_gutter_action(&ide, pest_tab, 6, "Run '");
    let finished = wait_for_test_run(&ide, mark);
    assert_eq!(finished["ok"], true, "{finished}");
    assert_eq!(
        test_status(&ide, mark, "it greets by name").as_deref(),
        Some("passed")
    );
    assert_eq!(
        test_status(&ide, mark, "whispers in lower case"),
        None,
        "the marker ran the whole file instead of one test"
    );

    assert_eq!(ide.quit(), 0);
}

/// Flow d: the `[php]` interpreter is a compose service (`make
/// test-php-container`, which also exports `IDE_E2E_PHP_CONTAINER=1`, the
/// service's bind-mount path as `TMPDIR` and `IDE_PHP_E2E_DIR`).
///
/// With `container_target` set, PHP_CodeSniffer runs through
/// `docker compose exec` inside the `php:8.3-cli` service and its findings
/// come back under the *local* path, and a gutter test run executes in the
/// container the same way. The project sits at the same path on the host,
/// in this test container and in the service, so no path needs translating
/// beyond the identity the target's path map gives.
#[test]
#[ignore = "E2E: needs an X server, the linux-php image and the host Docker socket; run via `make test-php-container`"]
fn e2e_php_container_interpreter_analyses_and_runs_tests() {
    if !require_php_e2e() {
        return;
    }
    if std::env::var("IDE_E2E_PHP_CONTAINER").as_deref() != Ok("1") {
        eprintln!("skipping: set IDE_E2E_PHP_CONTAINER=1 (`make test-php-container`)");
        return;
    }
    let name = "e2e_php_container_interpreter_analyses_and_runs_tests";
    let compose = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docker/php-compose.yml");
    let compose = compose.canonicalize().expect("docker/php-compose.yml");
    let project = php_project();
    let mut ide = launch_php(name, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");

    // The target's workdir is the project's own path, which is only known
    // now: write the settings and restart.
    assert_eq!(ide.quit(), 0);
    let root = ide.project_root().to_path_buf();
    std::fs::write(
        root.join("tests/ContainerHostTest.php"),
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace Tests;\n\nuse PHPUnit\\Framework\\TestCase;\n\n\
         final class ContainerHostTest extends TestCase\n{\n    public function testRunsInTheComposeService(): void\n    {\n        \
         $this->assertSame('php-e2e-container', gethostname());\n    }\n}\n",
    )
    .expect("ContainerHostTest.php");
    write_project_settings(
        &root,
        &format!(
            "[php]\ncontainer_target = \"php-e2e\"\ncontainer_mode = \"exec\"\n\n\
             [[containers.target]]\nid = \"php-e2e\"\nname = \"php-e2e\"\n\
             source = \"compose-service\"\ncompose_files = [{compose:?}]\n\
             service = \"php\"\nworkdir = {root:?}\n"
        ),
    );
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");

    // PHP_CodeSniffer runs in the service and reports under the local path.
    let mark = ide.mark();
    open_file(&ide, "Greeter.php");
    wait_for_problem(&ide, mark, "PHP_CodeSniffer", "src/Greeter.php");

    // A test that only passes where the compose service runs it.
    let mark = ide.mark();
    let tab = open_file(&ide, "ContainerHostTest.php");
    let test_tab = tab["tab_id"].as_u64().expect("tab_id");
    choose_gutter_action(&ide, test_tab, 10, "Run 'testRunsInTheComposeService'");
    let finished = wait_for_test_run(&ide, mark);
    assert_eq!(finished["ok"], true, "{finished}");
    assert_eq!(
        test_status(&ide, mark, "Runs in the compose service").as_deref(),
        Some("passed"),
        "{:?}",
        ide.events_since_of(mark, "test_tree_row")
    );
    assert_eq!(ide.quit(), 0);
}
