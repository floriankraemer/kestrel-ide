//! End-to-end flow for the PHP tooling plan's analyzer surface (E2): a
//! `phpstan`-shaped analyzer's findings reach the editor's squiggles and the
//! Problems dock, and double-clicking a row navigates to it.
//!
//! Its own test binary for the reason `e2e_vcs.rs` gives — `e2e.rs` sits at
//! its ratcheted size ceiling — and `make e2e` runs it with the others.
//! ADR-0046 records the E2E budget moving from 15 to 16 for this flow.
//!
//! No real PHPStan or Composer is installed anywhere this suite runs
//! (Risk #7 in the plan): `analysis_core::bin::stub_analyzer` (E1) plays
//! `vendor/bin/phpstan`, printing a canned `checkstyle-xml` finding against
//! the fixture's own `src/Greeter.php` — the same file, line, column and
//! message as `analysis-core/tests/fixtures/checkstyle_one_file.xml`, so the
//! two stay in sync rather than drifting apart as two hand-maintained copies
//! of the same fixture data.
//!
//! `env!("CARGO_BIN_EXE_stub_analyzer")` cannot be used here: Cargo only
//! sets `CARGO_BIN_EXE_<name>` for a binary's *own* package's tests (the
//! same reason `lsp_core::bin::stub_server` is only ever `env!()`-located
//! from within `lsp-core` itself, never from `crates/app`). `e2e-ci`
//! (`Makefile`) builds `stub_analyzer` explicitly, right beside its own
//! `cargo build --bin stub_server -p lsp-core` line, so its executable
//! lands in the same target directory as `app`'s own binary; its path is
//! derived from `CARGO_BIN_EXE_app`'s directory instead — see
//! [`stub_analyzer_bin`].
//!
//! There is no `OnType`/`OnSave` wiring from a live keystroke or a save into
//! `analysis_core::Scheduler::schedule_file_run` yet — only the "Inspect
//! Project" menu action (B9) reaches `AnalysisServiceRust` today, and that
//! is a real, if narrower, path for a project's own findings to reach the
//! editor. This flow drives that action rather than typing and waiting out
//! a debounce interval nothing currently arms.

use std::path::{Path, PathBuf};

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

/// `stub_analyzer`'s executable — see this file's own doc comment for why
/// `env!("CARGO_BIN_EXE_stub_analyzer")` cannot be used directly.
fn stub_analyzer_bin() -> PathBuf {
    let name = if cfg!(windows) {
        "stub_analyzer.exe"
    } else {
        "stub_analyzer"
    };
    let path = Path::new(APP).with_file_name(name);
    assert!(
        path.is_file(),
        "{} does not exist — run via `make e2e`, which builds it \
         (`cargo build --bin stub_analyzer -p analysis-core`) before this \
         test; `cargo test -p app` alone never builds a binary that only \
         `analysis-core` declares",
        path.display()
    );
    path
}

/// A Composer project declaring `phpstan/phpstan` in `require-dev`, with
/// `vendor/bin/phpstan` seeded to `stub_analyzer`'s binary and a
/// `src/Greeter.php` long enough for the stub's canned line-10 finding to
/// land on a real line. `$name` on line 10 is genuinely unbound in this
/// file, so a reader who goes looking sees the same bug PHPStan reports —
/// nothing here is asserting against a lie.
fn php_analyzer_fixture() -> tempfile::TempDir {
    const GREETER_PHP: &str = "<?php\n\
                                \n\
                                class Greeter\n\
                                {\n    public function greet(): string\n    {\n\
                                \x20\x20\x20\x20\x20\x20\x20\x20// step one\n\
                                \x20\x20\x20\x20\x20\x20\x20\x20// step two\n\
                                \x20\x20\x20\x20\x20\x20\x20\x20// step three\n\
                                \x20\x20\x20\x20\x20\x20\x20\x20$greeting = $name;\n\
                                \x20\x20\x20\x20\x20\x20\x20\x20return $greeting;\n    }\n}\n";

    let dir = tempfile::TempDir::new().expect("temp analyzer fixture dir");
    std::fs::write(
        dir.path().join("composer.json"),
        r#"{"require-dev": {"phpstan/phpstan": "^1.0"}}"#,
    )
    .expect("composer.json");
    std::fs::create_dir_all(dir.path().join("src")).expect("src dir");
    std::fs::write(dir.path().join("src/Greeter.php"), GREETER_PHP).expect("Greeter.php");

    let vendor_bin = dir.path().join("vendor/bin");
    std::fs::create_dir_all(&vendor_bin).expect("vendor/bin");
    std::fs::copy(stub_analyzer_bin(), vendor_bin.join("phpstan"))
        .expect("seeding the stub analyzer");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            vendor_bin.join("phpstan"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("making the stub analyzer executable");
    }
    dir
}

/// Open one file through Go to File, returning its `tab_added` marker.
/// Duplicated from `e2e_run.rs` by the same judgement that duplicated it
/// from `e2e.rs`: a twenty-line helper is not worth a crate between the test
/// binaries.
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

/// Open the "Ana&lysis" menu via its mnemonic, waiting for it to actually be
/// on screen before anything clicks into it.
fn open_analysis_menu(ide: &Ide) -> Mark {
    let mark = ide.mark();
    ide.key("alt+l");
    ide.wait_for_event(mark, "the Analysis menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "analysis_menu"
    });
    mark
}

/// The centre of a widget rectangle a marker reported, in screen
/// coordinates. Duplicated from `e2e_run.rs` for the same reason `open_file`
/// above is.
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

/// Search Everywhere's fuzzy filename match needs the project index built
/// first, or an early `open_file` call races an empty result set. Duplicated
/// from `e2e.rs` for the same reason every other helper here is.
fn wait_for_index(mcp: &e2e::mcp::Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", serde_json::json!({}))["ready"] == true).then_some(())
    });
}

fn cursor(mcp: &e2e::mcp::Mcp, tab_id: u64) -> (u32, u32) {
    let position = mcp.call(
        "get_cursor_position",
        serde_json::json!({ "tab_id": tab_id }),
    );
    (
        position["line"].as_u64().unwrap_or(0) as u32,
        position["column"].as_u64().unwrap_or(0) as u32,
    )
}

/// B/C's verification scenario from the plan: open a fixture project whose
/// `vendor/bin/phpstan` is the stub analyzer, confirm findings appear as
/// squiggles, that the Problems dock shows them with source `PHPStan`
/// (`plugin.toml`'s `name`, not its lowercase `id`), and that double-clicking
/// a row navigates to it.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_analyzer_findings_appear_inline_and_in_problems() {
    let name = "e2e_analyzer_findings_appear_inline_and_in_problems";
    let project = php_analyzer_fixture();
    let mut ide = Ide::launch(name, APP, project.path());
    drop(project);
    let mcp = ide.mcp();

    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    // Open the file before running the analyzer: `EditorTabs::
    // applyDiagnostics` only repaints tabs that are already open, so this is
    // what proves a finding reaches an *already-visible* editor, not merely
    // the store.
    let tab = open_file(&ide, "Greeter.php");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    let before = cursor(&mcp, tab_id);

    let mark = open_analysis_menu(&ide);
    let inspect = ide.wait_for_event(mark, "the Inspect Project menu item", |e| {
        e["ev"] == "analysis_menu_action" && e["label"] == "Inspect Project"
    });
    let (x, y) = rect_centre(&inspect["rect"]);
    ide.click_at(x, y, 1);
    ide.wait_for_event(mark, "the Analysis menu to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "analysis_menu"
    });

    // The squiggle: `diagnostics_applied` is `EditorTabs::applyDiagnostics`
    // reporting it repainted this file's spans (ADR-0046's wiring, extended
    // in this change to include `AnalysisService`).
    ide.wait_for_event(
        mark,
        "the analyzer's finding to reach the editor as a squiggle",
        |e| {
            e["ev"] == "diagnostics_applied"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("Greeter.php"))
                && e["count"].as_u64().unwrap_or(0) > 0
        },
    );

    // The Problems dock: one row, named after the analyzer.
    let row = ide.wait_for_event(mark, "the finding to reach the Problems dock", |e| {
        e["ev"] == "problem_row"
            && e["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("Greeter.php"))
            && e["source"] == "PHPStan"
    });
    ide.wait_for_event(mark, "the Problems dock to report a total", |e| {
        e["ev"] == "problems_refreshed" && e["total"].as_u64().unwrap_or(0) > 0
    });

    // Double-clicking the row navigates the already-open tab to line 10
    // (1-based in the row, 0-based from `get_cursor_position`) — the
    // canned finding's own line.
    let (rx, ry) = rect_centre(&row["rect"]);
    ide.double_click_at(rx, ry, 1);
    e2e::wait_for("the caret to land on the finding's line", || {
        let after = cursor(&mcp, tab_id);
        (after != before && after.0 == 9).then_some(())
    });

    assert_eq!(ide.quit(), 0);
}
