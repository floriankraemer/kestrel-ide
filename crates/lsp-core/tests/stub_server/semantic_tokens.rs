//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

/// C9: `semantic_tokens_legend` reads the legend from `initialize`'s static
/// `semanticTokensProvider`, and `semantic_tokens` decodes the canned
/// response into the tokens `crates/lsp-core/src/bin/stub_server.rs`
/// documents — through the real reader thread and `dispatch`, not just
/// `lsp_core::semantic_tokens` in isolation.
#[test]
fn semantic_tokens_are_read_from_the_static_initialize_capability() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_semantic_tokens_static())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    let legend = manager
        .semantic_tokens_legend(LANG)
        .expect("the static capability was read at connect time");
    assert_eq!(legend.token_types[1], "type");
    assert_eq!(legend.token_types[12], "function");
    assert!(legend.full);

    let result = manager
        .semantic_tokens(LANG, "file:///stub/main.rs")
        .expect("the stub answers textDocument/semanticTokens/full");
    let (result_id, tokens) =
        lsp_core::parse_semantic_tokens_full(&result).expect("the stub sent data");
    assert_eq!(result_id, Some("1".to_string()));
    assert_eq!(tokens.len(), 3);
    // Same-line, multiple tokens: the second token's column is relative to
    // the first token's own start (0 + 4 = 4), not to column 0.
    assert_eq!(tokens[0].line, 0);
    assert_eq!(tokens[0].start_char, 0);
    assert_eq!(tokens[1].line, 0);
    assert_eq!(tokens[1].start_char, 4);
    // Line advance: the third token resets its column to its own delta.
    assert_eq!(tokens[2].line, 1);
    assert_eq!(tokens[2].start_char, 0);

    let scope = lsp_core::semantic_token_scope(&legend, &tokens[1])
        .expect("\"function\" with defaultLibrary resolves");
    assert_eq!(scope.name(), "function.builtin");

    manager.stop(LANG);
}
/// C9: a server with nothing to say for a document (the stub's
/// `uri.ends_with("empty.rs")` case) answers `null`, which
/// `parse_semantic_tokens_full` reports as `None` rather than an empty
/// token list mistaken for "the server said nothing was highlighted".
#[test]
fn semantic_tokens_null_response_is_not_an_error() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_semantic_tokens_static())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    let result = manager
        .semantic_tokens(LANG, "file:///stub/empty.rs")
        .expect("the stub answers even with nothing to say");
    assert!(lsp_core::parse_semantic_tokens_full(&result).is_none());

    manager.stop(LANG);
}
/// C9: a server with no semantic-tokens capability at all — the plain
/// `stub_config()`, which advertises neither the static capability nor
/// runs the dynamic registration — must never be asked: `semantic_tokens_legend`
/// answers `None`, which is this client's generic gate on both paths.
#[test]
fn semantic_tokens_legend_is_none_for_a_server_that_never_advertised_it() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(manager.semantic_tokens_legend(LANG).is_none());

    manager.stop(LANG);
}
/// C9: csharp-ls's suspected path — declaring `textDocument/semanticTokens`
/// via `client/registerCapability` rather than `initialize`'s static
/// result. `semantic_tokens_legend` must pick it up exactly as it does the
/// static path, through the real reader thread's `client/registerCapability`
/// handling, not a direct call into `crate::semantic_tokens`.
#[test]
fn semantic_tokens_legend_is_read_from_a_dynamic_registration() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(
        manager.semantic_tokens_legend(LANG).is_none(),
        "nothing has registered yet"
    );

    let outcome = manager
        .request(LANG, "stub/semanticTokensRegisterRun", json!({}))
        .expect("the stub runs its registration sequence");
    assert_eq!(outcome["registered"], true);

    let legend = manager
        .semantic_tokens_legend(LANG)
        .expect("the dynamic registration's legend was captured");
    assert_eq!(legend.token_types[15], "keyword");
    assert!(legend.full);

    manager.stop(LANG);
}
