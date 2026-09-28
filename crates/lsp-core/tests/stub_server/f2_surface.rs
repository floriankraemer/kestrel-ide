//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

#[test]
fn signature_help_is_parsed_from_every_response_shape() {
    let (manager, _rx, uri) = session_with_open_document();

    // Line 0: `[start, end]` parameter labels.
    let offsets = manager
        .signature_help(uri, 0, 0)
        .expect("signatureHelp")
        .expect("a signature");
    assert_eq!(
        offsets.resolved_signature().unwrap().parameters[1].label,
        "value: T"
    );
    assert_eq!(offsets.resolved_parameter(), Some(1));

    // Line 1: substring parameter labels, resolved to the same shape.
    let substrings = manager
        .signature_help(uri, 1, 0)
        .expect("signatureHelp")
        .expect("a signature");
    assert_eq!(
        substrings.resolved_signature().unwrap().parameters[0].range,
        Some((10, 22)),
        "a substring label must reach the view as offsets like any other",
    );

    // Line 2: an overload set whose signature-level index disagrees with the
    // response-level one.
    let overloads = manager
        .signature_help(uri, 2, 0)
        .expect("signatureHelp")
        .expect("a signature");
    assert_eq!(overloads.resolved_signature().unwrap().label, "f(a)");
    assert_eq!(overloads.resolved_parameter(), Some(0));

    // Line 3: a malformed reply — `signatures` is an object. No panic, no
    // hint.
    assert!(manager
        .signature_help(uri, 3, 0)
        .expect("signatureHelp")
        .is_none());

    // Line 4: an active parameter past the signature's own arity.
    let past_end = manager
        .signature_help(uri, 4, 0)
        .expect("signatureHelp")
        .expect("a signature");
    assert_eq!(past_end.resolved_parameter(), None);

    // Anywhere else: null is "no hint here", not an error.
    assert!(manager
        .signature_help(uri, 9, 0)
        .expect("signatureHelp")
        .is_none());

    manager.stop(LANG);
}
#[test]
fn document_highlights_keep_the_kind_of_each_occurrence() {
    use lsp_core::HighlightKind;
    let (manager, _rx, uri) = session_with_open_document();

    let highlights = manager.document_highlights(uri, 0, 0).expect("highlights");
    assert_eq!(
        highlights.iter().map(|h| h.kind).collect::<Vec<_>>(),
        vec![
            HighlightKind::Text,
            HighlightKind::Read,
            HighlightKind::Write
        ],
        "a write and a read want different emphasis, so the kind survives",
    );

    assert_eq!(
        manager.document_highlights(uri, 1, 0).expect("highlights")[0].kind,
        HighlightKind::Text,
        "a kind we do not know still gets painted, neutrally",
    );
    assert!(
        manager
            .document_highlights(uri, 3, 0)
            .expect("highlights")
            .is_empty(),
        "a malformed entry is dropped, not panicked on",
    );
    assert!(manager
        .document_highlights(uri, 9, 0)
        .expect("highlights")
        .is_empty());

    manager.stop(LANG);
}
#[test]
fn inlay_hints_are_asked_for_a_viewport_and_not_for_the_whole_file() {
    use lsp_core::InlayHintKind;
    let (manager, _rx, uri) = session_with_open_document();

    // The stub echoes the range it was given, so the request's own line
    // range is observable from the reply.
    let hints = manager.inlay_hints(uri, 120, 168).expect("inlayHint");
    assert_eq!(hints.len(), 2);
    assert_eq!(hints[0].line, 120);
    assert_eq!(hints[0].label, ": i32");
    assert_eq!(hints[0].kind, InlayHintKind::Type);
    assert!(hints[0].padding_left);
    assert_eq!(
        hints[1].line, 168,
        "the last visible line is included and no line beyond it is asked for",
    );
    assert_eq!(hints[1].label, "value:", "label parts arrive concatenated");
    assert_eq!(hints[1].kind, InlayHintKind::Parameter);

    assert!(
        manager
            .inlay_hints(uri, 900, 950)
            .expect("inlayHint")
            .is_empty(),
        "a hint whose label is the wrong type is dropped, not panicked on",
    );

    manager.stop(LANG);
}
#[test]
fn intentions_merge_both_requests_into_one_grouped_list() {
    use lsp_core::IntentionGroup;
    let (manager, _rx, uri) = session_with_open_document();

    let diagnostic = json!({
        "range": {"start": {"line": 4, "character": 0},
                  "end": {"line": 4, "character": 7}},
        "severity": 1,
        "code": "E0433",
        "message": "failed to resolve: use of undeclared type `HashMap`",
    });
    let intentions = manager
        .intentions(uri, (4, 0), (4, 0), std::slice::from_ref(&diagnostic))
        .expect("intentions");

    assert_eq!(
        intentions.iter().map(|i| i.title()).collect::<Vec<_>>(),
        vec![
            "Import `HashMap`",
            "Extract into function",
            "Organize imports"
        ],
        "quick fix, then refactoring, then source — and each exactly once",
    );
    assert_eq!(intentions[0].group, IntentionGroup::QuickFix);
    assert!(intentions[0].preferred);
    assert_eq!(intentions[1].group, IntentionGroup::Refactor);
    assert_eq!(
        intentions[1].item.raw["data"]["scope"], "diagnostic",
        "the diagnostic-scoped copy of a duplicate is the one kept",
    );
    assert_eq!(intentions[2].group, IntentionGroup::Source);

    assert!(
        lsp_core::suggests_organize_imports(&diagnostic),
        "an unresolved-symbol diagnostic is what makes Organize Imports a quick fix",
    );

    manager.stop(LANG);
}
#[test]
fn organize_imports_survives_a_server_that_ignores_the_only_filter() {
    let (manager, _rx, uri) = session_with_open_document();

    // The stub answers nothing at all to a filtered request, which is the
    // behaviour `needs_unfiltered_retry` exists for.
    let action = manager
        .organize_imports(uri, 42)
        .expect("organizeImports")
        .expect("an action");
    assert_eq!(action.title, "Organize imports");
    assert_eq!(action.kind.as_deref(), Some("source.organizeImports"));
    assert!(
        action.edit.is_some(),
        "the retried answer is the real action, edit and all",
    );

    manager.stop(LANG);
}
#[test]
fn the_f2_capabilities_are_advertised_because_they_are_implemented() {
    let (manager, _rx, _uri) = session_with_open_document();

    let capabilities = manager
        .request(LANG, "stub/clientCapabilities", json!({}))
        .expect("the stub hands back what we sent");
    let text_document = &capabilities["textDocument"];

    assert_eq!(
        text_document["signatureHelp"]["signatureInformation"]["parameterInformation"]
            ["labelOffsetSupport"],
        true,
        "offsets are unambiguous where a substring label is not",
    );
    assert_eq!(
        text_document["signatureHelp"]["signatureInformation"]["activeParameterSupport"], true,
        "the only way an overload can say it takes fewer arguments",
    );
    assert!(text_document["documentHighlight"].is_object());
    assert!(text_document["inlayHint"].is_object());
    assert!(
        text_document["codeAction"]["codeActionLiteralSupport"]["codeActionKind"]["valueSet"]
            .as_array()
            .expect("a kind list")
            .contains(&json!("source.organizeImports")),
    );

    manager.stop(LANG);
}
