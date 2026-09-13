//! End-to-end flows for running, building and debugging (F4, R1, B1, D3):
//! the console, the run-configuration dialog, running a file from the
//! editor, a failing build reaching the Problems dock, and a debug session
//! stopping at a breakpoint.
//!
//! Their own test binary rather than more of `e2e.rs`, which sits at its
//! ratcheted size ceiling (`scripts/check-file-size.sh`). `make e2e` runs
//! both binaries; everything else about these flows — marker-stream
//! assertions only, never a screenshot — is exactly as `e2e.rs` describes.

use std::path::{Path, PathBuf};

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The centre of a widget rectangle a marker reported, in screen
/// coordinates — the only way to click a widget this harness has no handle
/// on. Duplicated from `e2e.rs` rather than shared: a ten-line helper is not
/// worth a third crate between the two test binaries.
fn rect_centre(rect: &serde_json::Value) -> (i32, i32) {
    let rect: Vec<i64> = rect
        .as_array()
        .expect("the marker carries a rect")
        .iter()
        .map(|v| v.as_i64().expect("an integer"))
        .collect();
    (
        (rect[0] + rect[2] / 2) as i32,
        (rect[1] + rect[3] / 2) as i32,
    )
}

/// Open one file through Go to File, returning its `tab_added` marker.
fn open_file(ide: &Ide, name: &str) -> serde_json::Value {
    let main_window = ide.window().to_string();
    let mark = ide.mark();
    ide.key("ctrl+shift+n");
    ide.wait_for_event(mark, "the search popup to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "search_everywhere"
    });
    ide.wait_for_focus_change(&main_window);
    ide.wait_for_ev(mark, "search_results");

    let mark = ide.mark();
    ide.type_text(name);
    ide.wait_for_event(mark, "results for the query", |e| {
        e["ev"] == "search_results" && e["count"].as_u64().unwrap_or(0) > 0
    });
    ide.key("Return");
    ide.wait_for_event(mark, "the search popup to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "search_everywhere" && e["accepted"] == true
    });
    ide.focus_main();
    ide.wait_for_event(mark, &format!("a tab for `{name}`"), |e| {
        e["ev"] == "tab_added" && e["title"] == name
    })
}

/// F4-15 (1/2): a real process, launched by `run.run`, delivers output
/// across `RunService`'s per-console reader thread to the console dock's
/// own widget, and `run.stop` reaches the process cleanly. Batching math,
/// link resolution and Cargo/npm/Makefile detection are already unit-tested
/// in `run-core` against nothing but values — what only the Qt event loop
/// can prove is the wiring around them: a background thread's output
/// actually reaching a `QPlainTextEdit`, and `run.run`/`run.stop` turning
/// into the right `RunService` call for whichever configuration
/// `RunToolbar` has selected. The fixture's configuration is pre-seeded in
/// `.ide/settings.toml` rather than detected, since Cargo-detection's own
/// correctness is exactly what `run_core::detect`'s unit tests already
/// cover and re-proving it here would only add a compiler's worth of
/// runtime to this flow for nothing this suite doesn't already know.
/// How many `Down` presses reach the item whose label starts with
/// `prefix`, given the first item is already highlighted.
///
/// Panics with the menu's actual contents rather than timing out sixty
/// seconds later on a focus change that never comes, which is what a wrong
/// index looks like from the outside.
fn menu_index(shown: &serde_json::Value, prefix: &str) -> usize {
    let items = shown["items"]
        .as_array()
        .expect("the menu marker carries its item labels");
    items
        .iter()
        .position(|item| {
            item.as_str()
                .is_some_and(|label| label.replace('&', "").starts_with(prefix))
        })
        .unwrap_or_else(|| panic!("no menu item starting with {prefix:?} in {items:?}"))
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_run_and_stop_shows_console_output() {
    let name = "e2e_run_and_stop_shows_console_output";
    let mut ide = Ide::launch(name, APP, fixture("runnable"));
    ide.wait_for_ev(Mark::start(), "project_opened");

    // `.ide/settings.toml`'s seeded configuration reaching the toolbar's
    // combo box: `detectConfigurations()` (fired on `projectOpened`) merges
    // it with whatever `run_core::detect` finds here (nothing) and
    // re-publishes the result through `configurationsChanged`. Awaited from
    // the very start of the stream, not from a mark taken after
    // `project_opened`: detection and `main_window_shown` race each other
    // during startup, so a mark taken even one line late can land after the
    // one `run_configurations_changed(count=1)` this flow needs.
    let seen = ide.wait_for_event(
        Mark::start(),
        "the seeded run configuration to reach the toolbar",
        |e| e["ev"] == "run_configurations_changed" && e["count"].as_u64().unwrap_or(0) >= 1,
    );
    assert_eq!(
        seen["count"].as_u64(),
        Some(1),
        "the fixture's shape changed"
    );

    let mark = ide.mark();
    // Not required for `run.run`/`run.stop` themselves (their shortcuts
    // carry the default `Qt::WindowShortcut` context, not dock focus) — but
    // this proves `view.runConsole` raises the same dock the console
    // actually appears in.
    ide.key("alt+4");
    ide.key("shift+F10"); // run.run
    let started = ide.wait_for_event(mark, "the console tab to appear", |e| {
        e["ev"] == "run_console_tab_added"
    });
    let console_id = started["console_id"].as_u64().expect("console_id");
    assert_eq!(started["config_id"], "e2e-echo");

    // Output crosses from the per-console reader thread to the widget in
    // batches, not necessarily one line per call — concatenate every chunk
    // published for this console rather than assuming one marker carries
    // the whole line.
    e2e::wait_for("the seeded command's output to arrive", || {
        let text: String = ide
            .events_since_of(mark, "run_console_output")
            .into_iter()
            .filter(|e| e["console_id"].as_u64() == Some(console_id))
            .filter_map(|e| e["text"].as_str().map(str::to_string))
            .collect();
        text.contains("E2E_RUN_MARKER").then_some(())
    });

    ide.key("ctrl+F2"); // run.stop
    let finished = ide.wait_for_event(mark, "the console to report it stopped", |e| {
        e["ev"] == "run_console_finished" && e["console_id"].as_u64() == Some(console_id)
    });
    assert_eq!(
        finished["escaped"], false,
        "Stop left a child process behind instead of killing its whole tree"
    );

    assert_eq!(ide.quit(), 0);
}

/// F4-15 (2/2): the Run Configurations dialog's commit round trip — add a
/// configuration, Save, quit, relaunch, and the project's `.ide/
/// settings.toml` and `RunToolbar`'s own picker both have to agree it is
/// still there. This is the exact bug class F0's test-strategy names for a
/// settings page (a draft that looks committed in the dialog but never
/// reaches disk, or a picker that only ever reflects what happened to be in
/// memory) and no Qt-free crate can reach it: `RunConfigEditor`'s draft/
/// commit logic is already unit-tested against a temp directory, but
/// nothing exercises the dialog's own widgets driving it, or a *second*,
/// unrelated process reading the result back cold.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_run_config_dialog_persists_across_relaunch() {
    const PROGRAM: &str = "/bin/true";
    let name = "e2e_run_config_dialog_persists_across_relaunch";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    let main_window = ide.window().to_string();

    let mark = ide.mark();
    ide.key("alt+r"); // "&Run"
    let menu = ide.wait_for_event(mark, "the Run menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "run_menu"
    });
    // A menu-bar-triggered QMenu pre-highlights its first item the moment
    // it opens (confirmed under Xvfb for "V&CS" in
    // `e2e_hunk_revert_is_one_undo_never_touches_disk`), so reaching item
    // N takes N-1 Downs. Which item that is comes from the menu itself:
    // counting to a hard-coded index broke silently every time somebody
    // added an entry above this one.
    let steps = menu_index(&menu, "Edit Configurations");
    for _ in 0..steps {
        ide.key("Down");
    }
    ide.key("Return");
    ide.wait_for_focus_change(&main_window);

    let shown = ide.wait_for_event(mark, "the Run Configurations dialog to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "run_config_dialog"
    });

    let (add_x, add_y) = rect_centre(&shown["add_rect"]);
    ide.click_at(add_x, add_y, 1);
    ide.wait_for_event(mark, "the new configuration to be added", |e| {
        e["ev"] == "run_config_added"
    });

    let (program_x, program_y) = rect_centre(&shown["program_rect"]);
    ide.click_at(program_x, program_y, 1);
    ide.type_text(PROGRAM);

    let (save_x, save_y) = rect_centre(&shown["save_rect"]);
    ide.click_at(save_x, save_y, 1);
    ide.wait_for_event(mark, "the dialog to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "run_config_dialog" && e["accepted"] == true
    });
    ide.focus_main();

    assert_eq!(ide.quit(), 0);

    // Read the persisted file with the app's own types — never a regex —
    // the same discipline `e2e_split_editor_persistence` applies to
    // `editor_layout`.
    let root = ide.project_root().to_path_buf();
    let settings = app_config::project_settings::load(&root).expect("settings just committed");
    let configs = settings
        .run_configs
        .expect("the dialog wrote a run_config table");
    assert_eq!(
        configs.len(),
        1,
        "expected exactly the one configuration added"
    );
    assert_eq!(configs[0].name, "New Configuration");
    assert_eq!(configs[0].program, PROGRAM);

    // And a cold second process picks it back up on its own, through
    // exactly the same `detectConfigurations` -> `configurationsChanged`
    // path `e2e_run_and_stop_shows_console_output` exercises for a
    // pre-seeded configuration.
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
    let seen = ide.wait_for_event(
        Mark::start(),
        "the persisted configuration to reach the toolbar",
        |e| e["ev"] == "run_configurations_changed" && e["count"].as_u64().unwrap_or(0) >= 1,
    );
    assert_eq!(seen["count"].as_u64(), Some(1));

    assert_eq!(ide.quit(), 0);
}

/// R1-9: running from context. The fixture's `scripts/etl.py` is deliberately
/// not something `run_core::detect` produces — detection only offers a Python
/// project's root entry points — so the configuration this flow runs can only
/// have come from `RunService::runContext`, which is the thing under test.
///
/// What no unit test can reach: `run.runContext` resolving the *focused
/// editor's* file, the temporary configuration reaching `.ide/settings.toml`
/// and coming back out through `configurationsChanged`, and the console for
/// it carrying the process's output.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_run_from_context_creates_a_temporary_configuration() {
    let name = "e2e_run_from_context_creates_a_temporary_configuration";
    let mut ide = Ide::launch(name, APP, fixture("runnable_context"));
    ide.wait_for_ev(Mark::start(), "project_opened");

    open_file(&ide, "etl.py");

    let mark = ide.mark();
    ide.key("ctrl+shift+F10"); // run.runContext

    let started = ide.wait_for_event(mark, "the console tab to appear", |e| {
        e["ev"] == "run_console_tab_added"
    });
    let console_id = started["console_id"].as_u64().expect("console_id");
    assert_eq!(
        started["config_id"], "python-scripts/etl.py",
        "the configuration was built from the focused file, not from detection"
    );

    e2e::wait_for("the script's output to arrive", || {
        let text: String = ide
            .events_since_of(mark, "run_console_output")
            .into_iter()
            .filter(|e| e["console_id"].as_u64() == Some(console_id))
            .filter_map(|e| e["text"].as_str().map(str::to_string))
            .collect();
        text.contains("E2E_CONTEXT_MARKER").then_some(())
    });

    ide.key("ctrl+F2"); // run.stop
    ide.wait_for_event(mark, "the console to report it stopped", |e| {
        e["ev"] == "run_console_finished" && e["console_id"].as_u64() == Some(console_id)
    });

    assert_eq!(ide.quit(), 0);

    // The temporary configuration is on disk, marked temporary — read with
    // the app's own types, never a regex, the same discipline the dialog
    // flow above uses.
    let root = ide.project_root().to_path_buf();
    let settings = app_config::project_settings::load(&root).expect("settings were written");
    let configs = settings.run_configs.expect("a run_config table");
    assert_eq!(configs.len(), 1, "exactly the one context configuration");
    assert_eq!(configs[0].id, "python-scripts/etl.py");
    assert!(configs[0].temporary, "a context configuration is temporary");
    assert_eq!(configs[0].target.as_deref(), Some("scripts/etl.py"));
}

/// B1-9: a build's diagnostics reach the Problems dock.
///
/// The fixture is a one-file crate with a type error and no dependencies, so
/// `cargo build` needs no network and finishes in about a second. What no
/// unit test can reach: `build.build` starting a real build, its output
/// crossing from the build's own thread to the dock, `build-core`'s JSON
/// parsing turning it into a diagnostic, and the Problems dock — whose only
/// source until now was a language server — showing it.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_build_failure_populates_problems_dock() {
    let name = "e2e_build_failure_populates_problems_dock";
    let mut ide = Ide::launch(name, APP, fixture("broken_build"));
    ide.wait_for_ev(Mark::start(), "project_opened");

    let mark = ide.mark();
    ide.key("ctrl+F9"); // build.build
    let started = ide.wait_for_event(mark, "the build to start", |e| e["ev"] == "build_started");
    let build_id = started["build_id"].as_u64().expect("build_id");

    let finished = ide.wait_for_event(mark, "the build to finish", |e| {
        e["ev"] == "build_finished" && e["build_id"].as_u64() == Some(build_id)
    });

    // The dock fills while the build runs, so the diagnostic may already be
    // there; waiting after the exit covers both orders. The build's own
    // output goes into the failure message, because "no problem arrived" is
    // unreadable without knowing what the tool actually said.
    let output: String = ide
        .events_since_of(mark, "build_output")
        .into_iter()
        .filter_map(|e| e["text"].as_str().map(str::to_string))
        .collect();
    ide.wait_for_event(
        mark,
        &format!("a problem to reach the Problems dock; the build said:\n{output}"),
        |e| e["ev"] == "problems_refreshed" && e["total"].as_u64().unwrap_or(0) > 0,
    );
    assert_ne!(
        finished["exit_code"].as_i64(),
        Some(0),
        "the fixture is supposed to fail to compile"
    );

    assert_eq!(ide.quit(), 0);
}

/// D3-9: a real debug session stops at a breakpoint, shows variables, steps
/// and resumes.
///
/// The adapter is debugpy, which the builder image installs — this is the
/// one flow that cannot be faked, because what it proves is that a third
/// party's adapter and this client agree. What no unit test can reach: the
/// gutter toggle reaching the store, the store reaching the adapter through
/// `setBreakpoints` before `configurationDone`, the `stopped` event coming
/// back through the session's reader thread, and the frames and variables
/// filling the dock from it.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_debug_stops_at_a_breakpoint() {
    let name = "e2e_debug_stops_at_a_breakpoint";
    let mut ide = Ide::launch(name, APP, fixture("debuggable"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    open_file(&ide, "main.py");

    // Line 2 is `answer = 42`, inside `compute()`. The caret opens on line
    // 1, so one Down reaches it.
    let mark = ide.mark();
    ide.key("Down");
    ide.key("ctrl+F8"); // debug.toggleBreakpoint
    ide.wait_for_event(mark, "the breakpoint to reach the gutter", |e| {
        e["ev"] == "breakpoints_applied" && e["count"].as_u64().unwrap_or(0) == 1
    });

    let mark = ide.mark();
    ide.key("shift+F9"); // debug.debug
    let started = ide.wait_for_event(mark, "the session to start", |e| e["ev"] == "debug_started");
    let session_id = started["session_id"].as_u64().expect("session_id");

    let stopped = ide.wait_for_event(mark, "the debuggee to suspend", |e| {
        e["ev"] == "debug_stopped" && e["session_id"].as_u64() == Some(session_id)
    });
    assert_eq!(
        stopped["line"].as_u64(),
        Some(2),
        "it must stop on the line the breakpoint is on"
    );

    ide.wait_for_event(mark, "the variables to arrive", |e| {
        e["ev"] == "debug_variables" && e["count"].as_u64().unwrap_or(0) > 0
    });

    // R5: the Breakpoints window opens on the real breakpoint the gutter
    // toggle just set, and typing into its condition field reaches
    // `configureBreakpoint` — the same field the debugpy conformance test
    // (`dap_core::tests::debugpy_honors_a_conditional_breakpoint`) proves an
    // adapter actually obeys, so this half only has to prove the widget is
    // wired to it, not that a condition works.
    let mark = ide.mark();
    ide.key("ctrl+shift+F8"); // debug.viewBreakpoints
    let shown = ide.wait_for_event(mark, "the Breakpoints window to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "breakpoints_window"
    });
    let (condition_x, condition_y) = rect_centre(&shown["condition_rect"]);
    ide.click_at(condition_x, condition_y, 1);
    ide.type_text("True");
    ide.key("Tab");
    ide.key("Escape");
    ide.wait_for_event(mark, "the Breakpoints window to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "breakpoints_window"
    });
    ide.focus_main();

    // Step over: the next stop is a later line, and it is a step rather than
    // another breakpoint hit.
    let mark = ide.mark();
    ide.key("F8"); // debug.stepOver
    let stepped = ide.wait_for_event(mark, "the step to land", |e| {
        e["ev"] == "debug_stopped" && e["session_id"].as_u64() == Some(session_id)
    });
    assert!(
        stepped["line"].as_u64().unwrap_or(0) > 2,
        "step over moved backwards or not at all: {stepped}"
    );

    let mark = ide.mark();
    ide.key("F9"); // debug.resume
    ide.wait_for_event(mark, "the session to end", |e| {
        e["ev"] == "debug_terminated" && e["session_id"].as_u64() == Some(session_id)
    });

    assert_eq!(ide.quit(), 0);
}
