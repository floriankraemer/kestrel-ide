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

use crate::support::{fixture, open_file, route_rust_at_stub, wait_for_index, APP};

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
