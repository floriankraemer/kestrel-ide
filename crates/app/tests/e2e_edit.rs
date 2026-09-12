//! End-to-end flow for the buffer-editing gestures that must be exactly one
//! Ctrl+Z each — Replace All and accepting a completion (F0-18, #142).
//!
//! Its own test binary for the reason `e2e_vcs.rs` gives — `e2e.rs` sits at
//! its ratcheted size ceiling — and `make e2e` runs it with the others.

use std::path::{Path, PathBuf};

use e2e::{mcp::Mcp, Ide, Mark};
use serde_json::json;

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The fixture file's content as it is checked in — the independent answer
/// to "did the app change this file", read without going through the app.
fn fixture_text(name: &str, relative: &str) -> String {
    std::fs::read_to_string(fixture(name).join(relative)).expect("fixture file")
}

/// Search Everywhere's fuzzy filename match needs the project index built
/// first, or an early `open_file` call races an empty result set. Duplicated
/// from `e2e.rs` for the same reason every other helper here is.
fn wait_for_index(mcp: &Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", json!({}))["ready"] == true).then_some(())
    });
}

/// Open one file through Go to File, returning its `tab_added` marker.
/// Duplicated from `e2e_analysis.rs` by the same judgement that duplicated
/// it from `e2e.rs`: a twenty-line helper is not worth a crate between the
/// test binaries.
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

fn buffer(mcp: &Mcp, tab_id: u64) -> String {
    mcp.call("read_buffer", json!({ "tab_id": tab_id }))["content"]
        .as_str()
        .expect("read_buffer returns a string")
        .to_string()
}

/// The centre of a `[x, y, w, h]` marker field.
fn rect_centre(rect: &serde_json::Value) -> (i32, i32) {
    let n = |i: usize| rect[i].as_i64().expect("rect component") as i32;
    (n(0) + n(2) / 2, n(1) + n(3) / 2)
}

/// Route the `rust` language id at `lsp-core`'s stub server — see
/// `e2e.rs`'s `route_rust_at_stub` for why this needs a relaunch.
fn route_rust_at_stub(ide: &mut Ide) {
    assert_eq!(ide.quit(), 0);
    let mut settings = app_config::load(&ide.config_dir()).expect("settings just written");
    settings
        .language_servers
        .push(app_config::LanguageServerSetting {
            language_id: "rust".to_string(),
            command: Some(
                Path::new(APP)
                    .with_file_name("stub_server")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ..Default::default()
        });
    app_config::save(&ide.config_dir(), &settings).expect("seeding the stub server override");
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
}

/// F0-18 (#142): Replace All and accepting a completion each cross the seam
/// as one `Vec<FfiTextEdit>` and are spliced inside one `beginEditBlock`, so
/// each is exactly one Ctrl+Z. One flow carries both halves — the E2E budget
/// (`next-five-features-plan.md`, Risk #14) is why it is not two.
///
/// The property regresses silently: the edits still apply, every other test
/// stays green, and only the undo depth changes — which is why each half
/// asserts the buffer *after* the undo, not just after the edit.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_replace_all_and_completion_are_one_undo_each() {
    let name = "e2e_replace_all_and_completion_are_one_undo_each";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    let original = fixture_text("tiny", "src/main.rs");
    assert_eq!(
        original.matches("world").count(),
        2,
        "the fixture's shape changed"
    );
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");

    // Replace All: two matches, one splice.
    let mark = ide.mark();
    ide.key("ctrl+r");
    let bar = ide.wait_for_event(mark, "the replace bar to open", |e| {
        e["ev"] == "find_bar_shown" && e["replace"] == true
    });
    ide.type_text("world");
    let (x, y) = rect_centre(&bar["replace_rect"]);
    ide.click_at(x, y, 1);
    ide.type_text("planet");
    let (x, y) = rect_centre(&bar["replace_all_rect"]);
    ide.click_at(x, y, 1);
    let applied = ide.wait_for_ev(mark, "edits_applied");
    assert_eq!(
        applied["count"].as_u64(),
        Some(2),
        "Replace All did not splice both matches"
    );
    // Escape closes the bar and hands focus back to the editor — Ctrl+Z
    // must reach the document, not the replace field's own undo stack.
    ide.key("Escape");

    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        original.replace("world", "planet"),
        "Replace All did not replace both matches"
    );

    undo_once_and_save(&ide, &mcp, tab_id);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo Replace All"
    );

    // Completion: the stub answers line 0 with `pop`/`push`/`#[allow]`;
    // which one `lsp_core::completion` ranks first is its unit tests'
    // business, this only needs the top row to be the one inserted.
    ide.key("ctrl+Home");
    let mark = ide.mark();
    ide.key("ctrl+space");
    let shown = ide.wait_for_ev(mark, "completion_shown");
    assert_eq!(shown["count"].as_u64(), Some(3), "the stub's line-0 items");
    ide.key("Return");
    let applied = ide.wait_for_ev(mark, "edits_applied");
    assert_eq!(applied["count"].as_u64(), Some(1));

    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        format!("#[allow(dead_code)]{original}"),
        "the completion was not inserted at (0,0)"
    );

    undo_once_and_save(&ide, &mcp, tab_id);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo the completion"
    );

    assert_eq!(ide.quit(), 0);
}

/// One Ctrl+Z, then a save so `read_buffer` sees the result — the "after
/// one undo" half every one-undo flow asserts.
fn undo_once_and_save(ide: &Ide, mcp: &Mcp, tab_id: u64) {
    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the tab to go dirty again", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the undone tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(mcp);
}
