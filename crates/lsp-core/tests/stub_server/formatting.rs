//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

#[test]
fn formatting_returns_the_servers_edits() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).unwrap();
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });
    manager
        .did_open("file:///workspace/a.stub", LANG, "text")
        .unwrap();

    let outcome = manager
        .format("file:///workspace/a.stub", &formatting_options(2))
        .unwrap();
    match outcome {
        lsp_core::formatting::FormattingOutcome::Edits(edits) => {
            assert_eq!(edits.len(), 1);
            assert_eq!(edits[0].new_text, "  ");
        }
        other => panic!("expected edits, got {other:?}"),
    }
    manager.stop_all();
}
// An empty list and a null both mean "already formatted", which is a
// different message to the user than "this language has no formatter".
#[test]
fn an_empty_or_null_reply_is_already_formatted_not_unsupported() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).unwrap();
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });
    manager
        .did_open("file:///workspace/a.stub", LANG, "text")
        .unwrap();

    for tab_size in [4, 8] {
        assert_eq!(
            manager
                .format("file:///workspace/a.stub", &formatting_options(tab_size))
                .unwrap(),
            lsp_core::formatting::FormattingOutcome::AlreadyFormatted,
            "tab size {tab_size}"
        );
    }
    manager.stop_all();
}
// A server that does not implement formatting answers MethodNotFound rather
// than an empty list. That must surface as Unsupported, not as an error the
// user sees as a failure.
#[test]
fn method_not_found_becomes_unsupported_rather_than_an_error() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).unwrap();
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });
    manager
        .did_open("file:///workspace/a.stub", LANG, "text")
        .unwrap();

    assert_eq!(
        manager
            .format("file:///workspace/a.stub", &formatting_options(99))
            .unwrap(),
        lsp_core::formatting::FormattingOutcome::Unsupported
    );
    manager.stop_all();
}
// The stub never implements rangeFormatting, so this exercises the fall back
// to whole-document formatting: reformatting more than was asked beats
// reformatting nothing, and the preview shows what changed either way.
#[test]
fn range_formatting_falls_back_to_whole_document() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).unwrap();
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });
    manager
        .did_open("file:///workspace/a.stub", LANG, "text")
        .unwrap();

    // tabSize 2 makes the whole-document path return edits, so seeing them
    // here proves the fall back happened.
    let outcome = manager
        .format_range(
            "file:///workspace/a.stub",
            (0, 0),
            (0, 4),
            &formatting_options(2),
        )
        .unwrap();
    match outcome {
        lsp_core::formatting::FormattingOutcome::Edits(edits) => {
            assert_eq!(edits.len(), 1)
        }
        other => panic!("expected the whole-document fall back, got {other:?}"),
    }
    manager.stop_all();
}
