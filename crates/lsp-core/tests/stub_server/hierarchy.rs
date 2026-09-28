//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

#[test]
fn call_and_type_hierarchy_are_advertised_with_dynamic_registration() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let capabilities = manager
        .request(LANG, "stub/clientCapabilities", json!({}))
        .expect("the stub hands back what we sent");
    assert_eq!(
        capabilities["textDocument"]["callHierarchy"]["dynamicRegistration"], true,
        "csharp-ls is not confirmed to declare this statically",
    );
    assert_eq!(
        capabilities["textDocument"]["typeHierarchy"]["dynamicRegistration"],
        true,
    );

    manager.stop(LANG);
}
#[test]
fn call_hierarchy_supported_is_false_for_a_server_that_never_advertised_it() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(!manager.call_hierarchy_supported(LANG));

    manager.stop(LANG);
}
#[test]
fn call_hierarchy_supported_is_read_from_a_dynamic_registration() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(
        !manager.call_hierarchy_supported(LANG),
        "nothing registered yet"
    );

    let outcome = manager
        .request(LANG, "stub/callHierarchyRegisterRun", json!({}))
        .expect("the stub runs its registration sequence");
    assert_eq!(outcome["registered"], true);

    assert!(manager.call_hierarchy_supported(LANG));

    manager.stop(LANG);
}
/// C11 end to end: `prepareCallHierarchy` -> `incomingCalls` ->
/// `outgoingCalls`, through the real reader thread, not `lsp_core::hierarchy`
/// in isolation. `outgoingCalls` on the item itself proves the populated
/// path; a second call on the leaf item (line 9) proves an empty array comes
/// back as a real, final answer — not something the client second-guesses.
#[test]
fn call_hierarchy_prepares_and_walks_incoming_and_outgoing_calls() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_call_hierarchy_static())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });
    assert!(manager.call_hierarchy_supported(LANG));

    let items = manager
        .prepare_call_hierarchy(LANG, "file:///workspace/main.cs", 3, 0)
        .expect("the stub answers prepareCallHierarchy");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "DoWork");

    let incoming = manager
        .incoming_calls(LANG, &items[0].raw)
        .expect("the stub answers incomingCalls");
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].from.name, "Main");
    assert_eq!(incoming[0].from_ranges.len(), 1);

    let outgoing = manager
        .outgoing_calls(LANG, &items[0].raw)
        .expect("the stub answers outgoingCalls");
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].to.name, "Helper");

    // Asking about the leaf item itself (line 9) answers with an empty
    // array — a real answer, not "the server had nothing".
    let leaf = manager
        .prepare_call_hierarchy(LANG, "file:///workspace/main.cs", 9, 0)
        .expect("the stub answers");
    let leaf_outgoing = manager
        .outgoing_calls(LANG, &leaf[0].raw)
        .expect("the stub answers with an empty array, not an error");
    assert!(leaf_outgoing.is_empty(), "Helper calls nothing further");

    manager.stop(LANG);
}
/// C11: no call at all — line 99 is the stub's "nothing here" convention,
/// same as `textDocument/hover`'s.
#[test]
fn prepare_call_hierarchy_returns_nothing_where_the_server_has_no_answer() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_call_hierarchy_static())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    let items = manager
        .prepare_call_hierarchy(LANG, "file:///workspace/main.cs", 99, 0)
        .expect("a null result is not an error");
    assert!(items.is_empty());

    manager.stop(LANG);
}
#[test]
fn type_hierarchy_supported_is_false_for_a_server_that_never_advertised_it() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(!manager.type_hierarchy_supported(LANG));

    manager.stop(LANG);
}
#[test]
fn type_hierarchy_supported_is_read_from_a_dynamic_registration() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(
        !manager.type_hierarchy_supported(LANG),
        "nothing registered yet"
    );

    let outcome = manager
        .request(LANG, "stub/typeHierarchyRegisterRun", json!({}))
        .expect("the stub runs its registration sequence");
    assert_eq!(outcome["registered"], true);

    assert!(manager.type_hierarchy_supported(LANG));

    manager.stop(LANG);
}
/// C11 end to end: `prepareTypeHierarchy` -> `supertypes`/`subtypes`,
/// through the real reader thread.
#[test]
fn type_hierarchy_prepares_and_walks_supertypes_and_subtypes() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_type_hierarchy_static())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });
    assert!(manager.type_hierarchy_supported(LANG));

    let items = manager
        .prepare_type_hierarchy(LANG, "file:///workspace/shapes.cs", 3, 0)
        .expect("the stub answers prepareTypeHierarchy");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Circle");

    let supertypes = manager
        .supertypes(LANG, &items[0].raw)
        .expect("the stub answers supertypes");
    assert_eq!(supertypes.len(), 1);
    assert_eq!(supertypes[0].name, "Shape");

    let subtypes = manager
        .subtypes(LANG, &items[0].raw)
        .expect("the stub answers subtypes");
    assert!(subtypes.is_empty(), "the stub gives Circle no subtypes");

    manager.stop(LANG);
}
/// C11: the pure fallback logic — no LSP process, no index — proving
/// `type_hierarchy_outcome` reaches for the index-derived fallback exactly
/// when `definition_outcome` would (ADR-0016's precedent), and that call
/// hierarchy's absence of a fallback (see `lsp_core::hierarchy` module docs)
/// is a deliberate asymmetry, not something this test needs to cover — there
/// is nothing to fall back to.
#[test]
fn type_hierarchy_outcome_falls_back_to_the_index_exactly_like_definition_outcome() {
    use lsp_core::completion::TextRange;
    use lsp_core::hierarchy::{type_hierarchy_outcome, HierarchyItem};

    let index_item = HierarchyItem {
        name: "Shape".into(),
        kind: 5,
        detail: None,
        uri: "file:///workspace/shapes.cs".into(),
        range: TextRange {
            start_line: 0,
            start_character: 0,
            end_line: 0,
            end_character: 5,
        },
        selection_range: TextRange {
            start_line: 0,
            start_character: 0,
            end_line: 0,
            end_character: 5,
        },
        data: None,
        raw: json!({"name": "Shape"}),
    };
    let fallback = vec![index_item.clone()];

    // No server was asked at all.
    assert_eq!(
        type_hierarchy_outcome(None, fallback.clone()),
        lsp_core::hierarchy::TypeHierarchyOutcome::Index(fallback.clone())
    );
    // A server exists but knows nothing here.
    assert_eq!(
        type_hierarchy_outcome(Some(Ok(vec![])), fallback.clone()),
        lsp_core::hierarchy::TypeHierarchyOutcome::Index(fallback.clone())
    );
    // Not currently running.
    assert_eq!(
        type_hierarchy_outcome(
            Some(Err(LspError::NotRunning("csharp".into()))),
            fallback.clone()
        ),
        lsp_core::hierarchy::TypeHierarchyOutcome::Index(fallback.clone())
    );
    // The server answered for real: its answer wins over the index.
    assert_eq!(
        type_hierarchy_outcome(Some(Ok(vec![index_item.clone()])), vec![]),
        lsp_core::hierarchy::TypeHierarchyOutcome::Lsp(vec![index_item])
    );
}
