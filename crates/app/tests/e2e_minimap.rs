//! End-to-end flow for the editor minimap (issue #199): its own test binary
//! for the same reason `e2e_vcs.rs`/`e2e_preview.rs` are — `e2e.rs` sits at
//! its ratcheted size ceiling (`scripts/check-file-size.sh`). `make e2e`
//! runs all of them.

use e2e::{mcp::Mcp, Ide, Mark};
use serde_json::{json, Value};

const APP: &str = env!("CARGO_BIN_EXE_app");

fn wait_for_index(mcp: &Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", json!({}))["ready"] == true).then_some(())
    });
}

/// Open one file through Go to File, returning its `tab_added` marker.
/// Duplicated from `e2e.rs`, as every other `e2e_*.rs` binary already
/// duplicates it: a helper this small is not worth a crate between them.
fn open_file(ide: &Ide, name: &str) -> Value {
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

fn rect_centre(rect: &Value) -> (i32, i32) {
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

/// A fixture directory holding one file long enough that the minimap
/// actually compresses it, so a click near the bottom of the strip
/// unambiguously lands the scrollbar in the lower part of the file rather
/// than somewhere indistinguishable from the top.
const FIXTURE_LINES: usize = 4000;

fn big_file_fixture() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp minimap fixture dir");
    let mut content = String::new();
    for i in 0..FIXTURE_LINES {
        content.push_str(&format!("fn line_{i}() {{}}\n"));
    }
    std::fs::write(dir.path().join("big.rs"), content).expect("fixture file");
    dir
}

/// Polls the marker stream for `minimap_shown` until the same rect is seen
/// twice in a row — `Minimap::resizeEvent` fires once per layout pass while
/// the tab settles into the window, and this is the "stopped changing"
/// signal that the strip's on-screen rect is now final, the same
/// quiescence a screenshot-based check would wait for by eye.
fn wait_for_settled_minimap_rect(ide: &Ide, mark: Mark) -> Value {
    let mut previous: Option<Value> = None;
    e2e::wait_for("the minimap's geometry to settle", || {
        let latest = ide.events_since_of(mark, "minimap_shown").pop()?;
        if previous.as_ref() == Some(&latest) {
            return Some(latest);
        }
        previous = Some(latest);
        None
    })
}

/// Drags the minimap's slider by pressing near the bottom of the strip, and
/// separately proves the "Show minimap" setting toggle: press near the
/// bottom scrolls the editor into the lower part of a 4000-line file
/// (`Minimap::mouseReleaseEvent`'s own `minimap_scrolled` marker), then
/// Settings > Editor's master checkbox turns the strip off live
/// (`CodeEditor::setMinimapOptions`'s `minimap_visible` marker) and the
/// choice survives a quit into `settings.toml`.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_minimap_drag_scrolls_the_editor() {
    let fixture = big_file_fixture();
    let mut ide = Ide::launch("e2e_minimap_drag_scrolls_the_editor", APP, fixture.path());
    drop(fixture);

    ide.wait_for_ev(Mark::start(), "project_opened");
    let mcp = ide.mcp();
    wait_for_index(&mcp);

    let mark = ide.mark();
    open_file(&ide, "big.rs");
    let shown = wait_for_settled_minimap_rect(&ide, mark);
    let rect = shown["rect"].as_array().expect("minimap rect");
    let x = rect[0].as_i64().unwrap() as i32 + rect[2].as_i64().unwrap() as i32 / 2;
    let bottom_y = rect[1].as_i64().unwrap() as i32 + rect[3].as_i64().unwrap() as i32 - 5;

    let mark = ide.mark();
    ide.click_at(x, bottom_y, 1);
    let scrolled = ide.wait_for_event(mark, "the minimap press to scroll the editor", |e| {
        e["ev"] == "minimap_scrolled"
    });
    let first_line = scrolled["first_line"].as_i64().expect("first_line");
    assert!(
        first_line > (FIXTURE_LINES as i64) / 2,
        "a press near the bottom of the strip must land in the lower half \
         of the file, got first_line={first_line} of {FIXTURE_LINES}"
    );

    // Settings > Editor: turn the master checkbox off, live-previewed
    // immediately and persisted on OK.
    let mark = ide.mark();
    ide.key("ctrl+comma");
    let dialog = ide.wait_for_event(mark, "the Settings dialog to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "settings_dialog"
    });

    let (category_x, category_y) = rect_centre(&dialog["editor_category_rect"]);
    ide.click_at(category_x, category_y, 1);
    // The Editor page's checkbox only has a real on-screen position once
    // the category switch actually shows it (settings_dialog.cpp's own
    // `editor_page_shown` marker, mirroring `settings_scope_switched` for
    // the Editing page's tab-width spinner) — and, like that marker, only
    // its top-left corner is trustworthy this early, its width still
    // reading as the whole pre-layout field column for one more turn.
    let editor_page = ide.wait_for_event(mark, "the Editor page to become current", |e| {
        e["ev"] == "editor_page_shown"
    });

    let top_left = editor_page["minimap_check_top_left"]
        .as_array()
        .expect("minimap_check_top_left is a [x, y] pair");
    let check_x = top_left[0].as_i64().unwrap() as i32 + 10;
    let check_y = top_left[1].as_i64().unwrap() as i32 + 10;
    ide.click_at(check_x, check_y, 1);
    ide.wait_for_event(mark, "the minimap to report itself hidden", |e| {
        e["ev"] == "minimap_visible" && e["enabled"] == false
    });

    let (ok_x, ok_y) = rect_centre(&dialog["ok_rect"]);
    ide.click_at(ok_x, ok_y, 1);
    ide.wait_for_event(mark, "the dialog to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "settings_dialog" && e["accepted"] == true
    });
    ide.focus_main();

    let config_dir = ide.config_dir();
    assert_eq!(ide.quit(), 0);

    let settings = app_config::load(&config_dir).expect("settings just committed");
    assert!(
        !settings.minimap.enabled,
        "the OK'd toggle must reach settings.toml"
    );
}
