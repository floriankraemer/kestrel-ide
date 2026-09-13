//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/mod.rs` for the shared harness this
//! draws on.

#[path = "stub_server/mod.rs"]
mod stub_server;
use stub_server::*;

/// L5: both response shapes, the insertion precedence, and the server's
/// ordering and filtering, driven through a real child process.
#[test]
fn completions_are_parsed_ordered_and_filtered() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let uri = "file:///workspace/main.rs";
    manager
        .did_open(uri, LANG, "fn main() {}")
        .expect("didOpen");

    // The trigger characters reach the UI through ServerReady, not a getter.
    // F2-9's signature-help triggers arrive on the same event, read from the
    // same `initialize` result.
    let (triggers, signature_triggers) = wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady {
            trigger_characters,
            signature_triggers,
            ..
        } => Some((trigger_characters.clone(), signature_triggers.clone())),
        _ => None,
    });
    assert_eq!(triggers, [".", ":"]);
    assert!(lsp_core::should_request(
        &triggers,
        "self.",
        false,
        &lsp_core::CompletionTracker::default(),
    ));
    assert!(signature_triggers.supported);
    assert_eq!(signature_triggers.trigger, ["(", ","]);
    assert_eq!(signature_triggers.retrigger, [")"]);

    // Line 0: a bare CompletionItem[], ordered by sortText, not by label.
    let plain = manager.completion(uri, 0, 0).expect("completion");
    assert!(!plain.is_incomplete);
    let ordered: Vec<String> = lsp_core::filter_completions(&plain.items, "p")
        .into_iter()
        .map(|m| m.item.label)
        .collect();
    assert_eq!(ordered, ["pop", "push"], "sortText wins over the label");
    // filterText, not the label, decides what a prefix matches.
    let filtered = lsp_core::filter_completions(&plain.items, "allo");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].item.label, "#[allow]");
    assert_eq!(filtered[0].item.insert, "#[allow(dead_code)]", "insertText");

    // Line 1: a CompletionList, incomplete, with a snippet and a textEdit.
    let list = manager.completion(uri, 1, 0).expect("completion");
    assert!(list.is_incomplete, "ask again as the word grows");
    let snippet = &list.items[0];
    assert!(
        snippet.is_snippet,
        "insertTextFormat: 2 — snippetSupport is now advertised as true"
    );
    let edit = &list.items[1];
    assert_eq!(edit.insert, "max()");
    assert_eq!(
        edit.range
            .expect("a textEdit names its range")
            .start_character,
        2
    );

    // Anywhere else: an empty list, which is not an error.
    assert!(manager
        .completion(uri, 7, 0)
        .expect("completion")
        .items
        .is_empty());

    manager.stop(LANG);
}
/// A server that never advertises `completionProvider.resolveProvider`
/// reports `completion_resolve_supported: false` on `ServerReady` — the flag
/// that gates every caller from spending the round trip on a server that
/// cannot answer it.
#[test]
fn server_ready_reports_no_resolve_support_when_not_advertised() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let supported = wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady {
            completion_resolve_supported,
            ..
        } => Some(*completion_resolve_supported),
        _ => None,
    });
    assert!(!supported);
    manager.stop(LANG);
}
/// The same event reports `true` for a server that advertises
/// `completionProvider.resolveProvider: true`, the way csharp-ls does.
#[test]
fn server_ready_reports_resolve_support_when_advertised() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_completion_resolve())
        .expect("stub starts");
    let supported = wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady {
            completion_resolve_supported,
            ..
        } => Some(*completion_resolve_supported),
        _ => None,
    });
    assert!(supported);
    manager.stop(LANG);
}
/// `resolve_completion_item` round-trips the item through the reader thread
/// and returns whatever the server added — here, the `additionalTextEdits`
/// that simulate a `using` insertion.
#[test]
fn resolve_completion_item_returns_the_resolved_item() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_completion_resolve())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    let item = json!({"label": "List", "kind": 7});
    let resolved = manager
        .resolve_completion_item(LANG, &item)
        .expect("the stub answers completionItem/resolve");
    assert_eq!(resolved["label"], "List");
    assert_eq!(
        resolved["additionalTextEdits"][0]["newText"],
        "using System.Collections.Generic;\n"
    );

    manager.stop(LANG);
}
/// A `completionItem/resolve` that never answers is bounded by the same
/// timeout/`$/cancelRequest` path every other request uses — proven here
/// through `request_with_timeout` directly (with a short deadline, rather
/// than `resolve_completion_item`'s own [`lsp_core::DEFAULT_REQUEST_TIMEOUT`],
/// so the test does not have to wait ten seconds) since `resolve_completion_item`
/// is a thin wrapper with no logic of its own to test separately — see its
/// source in `manager.rs`. The accept-path fallback this unblocks — apply the
/// unresolved item's own edit rather than hang — is exercised at the
/// `ui-shell` layer, in `crates/ui-shell/src/bridge/language/mod.rs`.
#[test]
fn a_resolve_that_never_answers_times_out_and_is_cancelled() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_completion_resolve())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    let item = json!({"label": "stub/silence"});
    let result = manager.request_with_timeout(
        LANG,
        "completionItem/resolve",
        item,
        Duration::from_millis(200),
    );
    assert!(matches!(
        result,
        Err(LspError::Timeout { method }) if method == "completionItem/resolve"
    ));

    // The server is still alive and answering: the request was cancelled,
    // not the connection torn down.
    let echoed = manager
        .request(LANG, "stub/echo", json!({"tag": "after-resolve-timeout"}))
        .expect("the server is still usable after a cancelled resolve");
    assert_eq!(echoed["tag"], "after-resolve-timeout");

    manager.stop(LANG);
}
