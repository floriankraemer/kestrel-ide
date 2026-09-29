//! Issue #361: MCP `read_buffer` answers with what is on screen, not the
//! last saved text.

use e2e::{Ide, Mark};

use crate::support::{buffer, fixture, fixture_text, open_file, wait_for_index, APP};

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_read_buffer_sees_unsaved_typing() {
    let mut ide = Ide::launch("e2e_read_buffer_sees_unsaved_typing", APP, fixture("tiny"));
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let original = fixture_text("tiny", "src/main.rs");
    let tab_id = open_file(&ide, "main.rs")["tab_id"]
        .as_u64()
        .expect("tab_id");

    ide.key("ctrl+Home");
    let mark = ide.mark();
    ide.type_text("// unsaved ");
    ide.wait_for_event(mark, "the tab to go dirty", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.sync(&mcp);

    let live = buffer(&mcp, tab_id);
    assert!(
        live.starts_with("// unsaved "),
        "unsaved typing not visible:\n{live}"
    );
    assert_eq!(
        fixture_text("tiny", "src/main.rs"),
        original,
        "reading the buffer must not save it"
    );

    // A dirty tab would hold Ctrl+Q behind the unsaved-changes prompt.
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    assert_eq!(ide.quit(), 0);
}
