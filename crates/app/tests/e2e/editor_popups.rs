//! R3's end-to-end flow: quick documentation composes the LSP hover and
//! every diagnostic covering the caret into one popup.
//!
//! `lsp-core`'s X2 stub server plays the language server: it publishes one
//! canned diagnostic on `textDocument/didOpen` (line 0, columns 0-4) and
//! answers `textDocument/hover` for line 0 with `MarkupContent` Markdown, so
//! opening the fixture's `main.rs` and asking for documentation at (0,0) —
//! Ctrl+Alt+Q, not a simulated mouse dwell, which has no reliable timing
//! under headless Xvfb — exercises both halves of the hover card in
//! one flow: the server's own answer and the diagnostic beside it.

use e2e::{Ide, Mark};

use crate::support::{fixture, fixture_text, open_file, route_rust_at_stub, wait_for_index, APP};

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

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_fills_in_the_fix_row_when_the_server_answers() {
    let name = "e2e_hover_card_fills_in_the_fix_row_when_the_server_answers";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    open_file(&ide, "main.rs");
    ide.key("ctrl+Home");

    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the card to show with its fixes loading", |e| {
        e["ev"] == "hover_popup_shown"
    });
    assert!(
        shown["html"]
            .as_str()
            .expect("html")
            .contains("Looking for fixes"),
        "fixes are asked for after the card is up: {shown}"
    );

    let updated = ide.wait_for_event(mark, "the fix row to arrive", |e| {
        e["ev"] == "hover_popup_updated"
    });
    let html = updated["html"].as_str().expect("html");
    assert!(
        html.contains("href=\"ide:fix/0\"") && html.contains("Import `HashMap`"),
        "the preferred quick fix should be the fix link: {html}"
    );
    assert!(
        html.contains("ide:more/0"),
        "and More actions follows it: {html}"
    );
    assert!(!html.contains("Looking for fixes"));

    // Screenshot for the PR, only when asked for.
    if let Ok(path) = std::env::var("IDE_SHOT") {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::process::Command::new("import")
            .args(["-window", "root", &path])
            .status();
    }
    assert_eq!(ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_gains_a_source_footer_from_the_index() {
    let name = "e2e_hover_card_gains_a_source_footer_from_the_index";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    open_file(&ide, "main.rs");
    // Line 4 is `    println!("{}", greeting::greet("world"));`: the end of the
    // line, then 14 back lands inside `greet`.
    ide.key("ctrl+Home");
    for _ in 0..3 {
        ide.key("Down");
    }
    ide.key("End");
    for _ in 0..14 {
        ide.key("Left");
    }

    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the card to show", |e| e["ev"] == "hover_popup_shown");
    assert!(
        !shown["html"].as_str().expect("html").contains("Source:"),
        "the card is not held up by the index: {shown}"
    );

    let declaration = fixture_text("tiny", "src/greeting.rs")
        .lines()
        .position(|line| line.contains("fn greet("))
        .expect("the fixture declares greet")
        + 1;
    let footer = format!("src/greeting.rs:{declaration}");
    let updated = ide.wait_for_event(mark, "the source footer to arrive", |e| {
        e["ev"] == "hover_popup_updated"
            && e["html"].as_str().is_some_and(|h| h.contains("Source:"))
    });
    let html = updated["html"].as_str().expect("html");
    assert!(
        html.contains("href=\"ide:source\"") && html.contains(&footer),
        "the footer names the declaration project-relative: {html}"
    );

    if let Ok(path) = std::env::var("IDE_SHOT") {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::process::Command::new("import")
            .args(["-window", "root", &path])
            .status();
    }
    assert_eq!(ide.quit(), 0);
}
