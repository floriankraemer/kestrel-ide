//! R3's end-to-end flow: quick documentation composes the LSP hover and
//! every diagnostic covering the caret into one popup.
//!
//! Its own test binary for the reason `e2e_analysis.rs` and `e2e_vcs.rs`
//! give — `e2e.rs` sits at its ratcheted size ceiling (`scripts/
//! check-file-size.sh`) — and `make e2e` runs it with the others.
//!
//! `lsp-core`'s X2 stub server plays the language server: it publishes one
//! canned diagnostic on `textDocument/didOpen` (line 0, columns 0-4) and
//! answers `textDocument/hover` for line 0 with `MarkupContent` Markdown, so
//! opening the fixture's `main.rs` and asking for documentation at (0,0) —
//! Ctrl+Alt+Q, not a simulated mouse dwell, which has no reliable timing
//! under headless Xvfb — exercises both halves of `compose_hover_html` in
//! one flow: the server's own answer and the diagnostic beside it.

use std::path::{Path, PathBuf};

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn wait_for_index(mcp: &e2e::mcp::Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", serde_json::json!({}))["ready"] == true).then_some(())
    });
}

fn open_search_popup(ide: &Ide, shortcut: &str) -> Mark {
    let main_window = ide.window().to_string();
    let mark = ide.mark();
    ide.key(shortcut);
    ide.wait_for_event(mark, "the search popup to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "search_everywhere"
    });
    ide.wait_for_focus_change(&main_window);
    ide.wait_for_ev(mark, "search_results");
    ide.mark()
}

fn accept_top_hit(ide: &Ide, mark: Mark, query: &str) {
    ide.type_text(query);
    let hits = ide.wait_for_event(mark, &format!("results for `{query}`"), |e| {
        e["ev"] == "search_results" && e["count"].as_u64().unwrap_or(0) > 0
    });
    assert!(hits["count"].as_u64().unwrap() > 0);
    ide.key("Return");
    ide.wait_for_event(mark, "the search popup to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "search_everywhere" && e["accepted"] == true
    });
    ide.focus_main();
}

fn open_file(ide: &Ide, name: &str) -> serde_json::Value {
    let mark = open_search_popup(ide, "ctrl+shift+n");
    accept_top_hit(ide, mark, name);
    ide.wait_for_event(mark, &format!("a tab for `{name}`"), |e| {
        e["ev"] == "tab_added" && e["title"] == name
    })
}

/// Where `stub_server` lands — see `e2e.rs`'s own copy of this function for
/// why `CARGO_BIN_EXE_stub_server` is not an option here.
fn stub_server_path() -> PathBuf {
    Path::new(APP).with_file_name("stub_server")
}

/// Route the `rust` language id at the stub server rather than a real
/// `rust-analyzer` — see `e2e.rs`'s own copy for the full reasoning.
fn route_rust_at_stub(ide: &mut Ide) {
    assert_eq!(ide.quit(), 0);
    let mut settings = app_config::load(&ide.config_dir()).expect("settings just written");
    settings
        .language_servers
        .push(app_config::LanguageServerSetting {
            language_id: "rust".to_string(),
            command: Some(stub_server_path().to_string_lossy().into_owned()),
            ..Default::default()
        });
    app_config::save(&ide.config_dir(), &settings).expect("seeding the stub server override");
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_quick_documentation_composes_hover_and_diagnostic() {
    let name = "e2e_quick_documentation_composes_hover_and_diagnostic";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    open_file(&ide, "main.rs");
    ide.key("ctrl+Home");

    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the quick-documentation popup to show", |e| {
        e["ev"] == "hover_popup_shown"
    });
    let html = shown["html"].as_str().expect("html");
    assert!(
        // `markdown_preview::render` syntax-highlights the fenced block, so
        // `fn` and `main()` land in separate tags rather than one run of
        // plain text.
        html.contains("main()") && html.contains("entry"),
        "the server's hover markdown should render: {html}"
    );
    assert!(
        html.contains("canned diagnostic"),
        "the diagnostic covering (0,0) should be composed in: {html}"
    );

    assert_eq!(ide.quit(), 0);
}
