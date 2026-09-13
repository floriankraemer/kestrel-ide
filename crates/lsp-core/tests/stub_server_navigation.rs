//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/mod.rs` for the shared harness this
//! draws on.

#[path = "stub_server/mod.rs"]
mod stub_server;
use stub_server::*;

/// L3: the manager reduces every hover shape the stub can send to one text.
#[test]
fn hover_is_parsed_from_every_response_shape() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let uri = "file:///workspace/main.rs";
    manager
        .did_open(uri, LANG, "fn main() {}")
        .expect("didOpen");

    // Line 0: MarkupContent markdown. R3 moved Markdown rendering to
    // `ui-shell` (`markdown_preview::render`), so this crate's own
    // `to_tooltip_html` is exercised only by the non-Markdown shapes below;
    // here there is only the structured content itself to check.
    let markup = manager
        .hover(uri, 0, 0)
        .expect("hover")
        .expect("some hover");
    assert!(markup.markdown);
    assert_eq!(
        markup.value,
        "```rust\nfn main()\n```\nThe **entry** point."
    );

    // Line 1: the deprecated {language, value} MarkedString.
    let marked = manager
        .hover(uri, 1, 0)
        .expect("hover")
        .expect("some hover");
    assert_eq!(marked.value, "```rust\nfn main()\n```");

    // Line 2: an array of MarkedStrings.
    let array = manager
        .hover(uri, 2, 0)
        .expect("hover")
        .expect("some hover");
    assert!(array.value.starts_with("plain hover"));

    // Anywhere else: a null result is "nothing here", not an error.
    assert_eq!(manager.hover(uri, 7, 0).expect("hover"), None);

    manager.stop(LANG);
}
/// L4: definitions arrive as a Location, a Location array or a LocationLink
/// array, and the precedence rule sends everything else to the index.
#[test]
fn definitions_are_parsed_and_fall_back_to_the_index() {
    use lsp_core::{definition_outcome, DefinitionOutcome};

    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let uri = "file:///workspace/main.rs";
    manager
        .did_open(uri, LANG, "fn main() {}")
        .expect("didOpen");

    let single = manager.definition(uri, 0, 0).expect("definition");
    assert_eq!(single.len(), 1);
    assert_eq!((single[0].line, single[0].column), (4, 4));

    let many = manager.definition(uri, 0, 1).expect("definition");
    assert_eq!(
        many.len(),
        2,
        "ambiguity is the servers answer, not an error"
    );

    let link = manager.definition(uri, 0, 2).expect("definition");
    assert_eq!((link[0].line, link[0].column), (4, 4));

    // A server that knows nothing hands the gesture to ADR-0011's index.
    let nothing = manager.definition(uri, 0, 9).expect("definition");
    assert_eq!(
        definition_outcome(Some(Ok(nothing))),
        DefinitionOutcome::Index
    );
    assert_eq!(
        definition_outcome(Some(Ok(single.clone()))),
        DefinitionOutcome::Lsp(single)
    );

    manager.stop(LANG);

    // The server is gone: the index answers rather than the gesture failing.
    assert_eq!(
        definition_outcome(Some(manager.definition(uri, 0, 0))),
        DefinitionOutcome::Index
    );
}
