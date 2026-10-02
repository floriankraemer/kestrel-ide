//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

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

    // Line 5: a doc far taller than any hover card (the card-scrolling E2E).
    let tall = manager
        .hover(uri, 5, 0)
        .expect("hover")
        .expect("some hover");
    assert!(tall
        .value
        .starts_with("```rust\nfn tall()\n```\nDoc line 1."));
    assert!(tall.value.ends_with("Doc line 60."));

    // Anywhere else (the stub answers lines 0-3, 5 and 7): a null result is "nothing here", not an error.
    assert_eq!(manager.hover(uri, 9, 0).expect("hover"), None);

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

/// N1/N2: implementation and type definition share definition's answer shapes.
#[test]
fn implementations_and_type_definitions_are_parsed_like_definitions() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let uri = "file:///workspace/main.rs";
    manager
        .did_open(uri, LANG, "fn main() {}")
        .expect("didOpen");

    let implementations = manager.implementation(uri, 0, 1).expect("implementation");
    assert_eq!(
        implementations.len(),
        2,
        "several implementations are normal"
    );
    let type_definition = manager.type_definition(uri, 0, 2).expect("typeDefinition");
    assert_eq!((type_definition[0].line, type_definition[0].column), (4, 4));
    assert!(manager
        .implementation(uri, 0, 9)
        .expect("implementation")
        .is_empty());
    manager.stop(LANG);
}

/// N2: Go to Declaration asks `declaration` of a server that offers it, and
/// `definition` of one that does not.
#[test]
fn go_to_declaration_prefers_the_declaration_request_when_offered() {
    let uri = "file:///workspace/main.rs";
    for (caps, expected_line) in [("declaration", 8), ("", 1)] {
        let (manager, _rx) = LspManager::new("file:///workspace");
        manager
            .start(&tagged_config("a", caps))
            .expect("stub starts");
        manager
            .did_open(uri, LANG, "fn main() {}")
            .expect("didOpen");
        let targets = manager.go_to_declaration(uri, 0, 0).expect("declaration");
        assert_eq!(targets[0].line, expected_line, "caps: {caps:?}");
        manager.stop(LANG);
    }
}

/// N3: `workspace/symbol` of every server, parsed and merged across servers.
#[test]
fn workspace_symbols_come_from_every_server_of_the_language() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    for (id, priority) in [("a", 0), ("b", 1)] {
        manager
            .start(&ServerConfig {
                priority,
                ..tagged_config(id, "workspaceSymbol")
            })
            .expect("stub starts");
    }
    let symbols = manager.workspace_symbols("sym").expect("workspace/symbol");
    let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
    // "Shared" is answered by both servers and listed once.
    assert_eq!(names, ["Shared", "Syma", "Symb"]);
    assert!(symbols.iter().all(|s| s.is_class_like()));
    manager.stop(LANG);

    assert!(manager
        .workspace_symbols("sym")
        .expect("no server is not an error here")
        .is_empty());
}
