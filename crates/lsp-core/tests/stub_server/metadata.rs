//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

/// C12: `fetch_metadata` round-trips a `csharp/metadata` request through the
/// reader thread and returns the decompiled source text, the same shape
/// `resolve_completion_item` proves above for `completionItem/resolve`.
/// This only proves the client speaks the wire shape `manager.rs` assumes
/// (`{textDocument: {uri}}` in, `{source: string}` out) — that shape is
/// itself unverified against a real csharp-ls (see `fetch_metadata`'s doc
/// comment).
#[test]
fn fetch_metadata_returns_the_stub_servers_decompiled_source() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    let source = manager
        .fetch_metadata(LANG, "csharp:/metadata/Projects/x/Console.cs")
        .expect("the stub answers csharp/metadata");
    assert!(source.contains("csharp:/metadata/Projects/x/Console.cs"));

    manager.stop(LANG);
}
/// A response with no `"source"` field (the stub's stand-in for whatever a
/// real server sends when it cannot serve the metadata — `useMetadataUris`
/// off, an unknown assembly, or a malformed reply) is a clean
/// [`lsp_core::LspError::Protocol`], not a panic and not an empty string
/// mistaken for real content — this is the case the C12 UI guard refuses on.
#[test]
fn fetch_metadata_with_no_source_field_is_a_protocol_error() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    let err = manager
        .fetch_metadata(LANG, "csharp:/metadata/missing.cs")
        .unwrap_err();
    assert!(matches!(err, lsp_core::LspError::Protocol(_)));

    manager.stop(LANG);
}
