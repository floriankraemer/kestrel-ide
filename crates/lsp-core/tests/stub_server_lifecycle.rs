//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/mod.rs` for the shared harness this
//! draws on.

#[path = "stub_server/mod.rs"]
mod stub_server;
use stub_server::*;

use std::thread;

#[test]
fn initialize_and_shutdown_lifecycle() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    assert!(manager.is_running(LANG));

    let restarts = wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { restarts, .. } => Some(*restarts),
        _ => None,
    });
    assert_eq!(restarts, 0, "a first launch is not a restart");

    manager.stop(LANG);
    assert!(!manager.is_running(LANG));
    // The server is gone, so requests fail instead of hanging.
    assert!(matches!(
        manager.request(LANG, "stub/echo", json!({})),
        Err(LspError::NoServer(_))
    ));
}
#[test]
fn a_missing_executable_fails_the_start() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    let err = manager
        .start(&config("definitely-not-a-language-server", &[]))
        .expect_err("a missing binary cannot start");
    assert!(matches!(err, LspError::Spawn { .. }), "got {err:?}");
    assert!(!manager.is_running(LANG));
}
#[test]
fn did_open_publishes_diagnostics_as_an_event() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    manager
        .did_open("file:///workspace/a.stub", LANG, "hello\n")
        .expect("didOpen is sent");
    assert_eq!(
        manager.document_version("file:///workspace/a.stub"),
        Some(1)
    );

    let (uri, version, message) = wait_for(&rx, "diagnostics", |e| match e {
        LspEvent::Diagnostics {
            uri,
            version,
            diagnostics,
            ..
        } => Some((uri.clone(), *version, diagnostics[0].message.clone())),
        _ => None,
    });
    assert_eq!(uri, "file:///workspace/a.stub");
    assert_eq!(version, Some(1));
    assert_eq!(message, "canned diagnostic");

    // The manager owns the version counter, not the caller.
    assert_eq!(
        manager
            .did_change("file:///workspace/a.stub", "bye\n")
            .unwrap(),
        2
    );
    assert_eq!(
        manager.document_version("file:///workspace/a.stub"),
        Some(2)
    );
    manager.did_close("file:///workspace/a.stub").unwrap();
    assert_eq!(manager.document_version("file:///workspace/a.stub"), None);
}
#[test]
fn responses_are_matched_to_their_own_requests() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    // The slow request is issued first and answered last; each caller must
    // still get its own payload back.
    std::thread::scope(|scope| {
        let slow = scope
            .spawn(|| manager.request(LANG, "stub/echo", json!({"tag": "slow", "delay_ms": 400})));
        std::thread::sleep(Duration::from_millis(50));
        let fast = manager
            .request(LANG, "stub/echo", json!({"tag": "fast"}))
            .expect("fast echo answers");
        assert_eq!(fast["tag"], "fast");
        let slow = slow.join().unwrap().expect("slow echo answers");
        assert_eq!(slow["tag"], "slow");
    });
}
#[test]
fn an_unimplemented_request_returns_the_servers_error() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    // Deliberately a method the stub has no arm for at all — `rename` and
    // `codeAction` are implemented now (RF3), so they no longer test this.
    let err = manager
        .request(LANG, "textDocument/formatting", json!({}))
        .expect_err("the stub implements no formatting");
    assert!(
        matches!(err, LspError::Response { code: -32601, .. }),
        "got {err:?}"
    );
}
#[test]
fn a_slow_request_times_out_and_is_cancelled() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let err = manager
        .request_with_timeout(
            LANG,
            "stub/echo",
            json!({"delay_ms": 5000}),
            Duration::from_millis(100),
        )
        .expect_err("the response arrives far too late");
    assert!(matches!(err, LspError::Timeout { .. }), "got {err:?}");

    // The late response must not be mistaken for the next request's.
    let next = manager
        .request(LANG, "stub/echo", json!({"tag": "next"}))
        .expect("the session survives a cancelled request");
    assert_eq!(next["tag"], "next");
}
#[test]
fn a_server_to_client_request_is_answered_rather_than_ignored() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let answer = manager
        .request(LANG, "stub/askClient", json!({}))
        .expect("the stub's own request does not deadlock us");
    assert_eq!(answer, "asked");
}
#[test]
fn a_server_that_dies_mid_session_is_respawned() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&dying_stub_config()).expect("stub starts");
    wait_for(&rx, "first ServerReady", |e| match e {
        LspEvent::ServerReady { restarts: 0, .. } => Some(()),
        _ => None,
    });

    manager
        .did_open("file:///workspace/a.stub", LANG, "boom\n")
        .expect("didOpen is sent");

    // It publishes its diagnostic, then exits(1) without an `exit` request.
    wait_for(&rx, "diagnostics", |e| match e {
        LspEvent::Diagnostics { .. } => Some(()),
        _ => None,
    });
    let retry_in = wait_for(&rx, "ServerExited", |e| match e {
        LspEvent::ServerExited {
            restarts: 1,
            retry_in,
            ..
        } => Some(*retry_in),
        _ => None,
    });
    assert!(
        retry_in >= Duration::from_millis(200),
        "backoff starts at 200ms"
    );

    wait_for(&rx, "ServerReady after respawn", |e| match e {
        LspEvent::ServerReady { restarts: 1, .. } => Some(()),
        _ => None,
    });
    assert!(manager.is_running(LANG));

    // The respawned server is a working session, not just a live process.
    let echoed = manager
        .request(LANG, "stub/echo", json!({"tag": "after-respawn"}))
        .expect("the respawned server answers");
    assert_eq!(echoed["tag"], "after-respawn");
}
/// The whole L2 path minus Qt: a real child server publishes diagnostics,
/// the event lands in the store the adapter keeps, and the store yields the
/// rows the Problems panel renders.
#[test]
fn published_diagnostics_become_problem_rows() {
    use diagnostics_core::{DiagnosticCounts, DiagnosticStore, Severity};
    use lsp_core::uri_from_path;

    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let uri = uri_from_path("/workspace/a b.stub");
    manager.did_open(&uri, LANG, "hello\n").expect("didOpen");

    let mut store = DiagnosticStore::new();
    let (published_uri, diagnostics) = wait_for(&rx, "diagnostics", |e| match e {
        LspEvent::Diagnostics {
            uri, diagnostics, ..
        } => Some((uri.clone(), diagnostics.clone())),
        _ => None,
    });
    store.replace(LANG, &published_uri, lsp_core::to_diagnostics(diagnostics));

    let rows = store.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, "/workspace/a b.stub");
    assert_eq!(rows[0].line, 1);
    assert_eq!(rows[0].column, 0);
    assert_eq!(rows[0].severity, Severity::Error);
    assert_eq!(rows[0].message, "canned diagnostic");
    assert_eq!(rows[0].source, "stub_server");
    assert_eq!(
        store.counts(),
        DiagnosticCounts {
            errors: 1,
            ..DiagnosticCounts::default()
        }
    );

    // Closing the document is what drops its rows from the panel.
    manager.did_close(&uri).expect("didClose");
    store.remove(LANG, &uri);
    assert!(store.rows().is_empty());
}
#[test]
fn a_request_the_server_never_answers_times_out_without_wedging_the_session() {
    let (manager, _rx, _uri) = session_with_open_document();

    let err = manager
        .request_with_timeout(LANG, "stub/silence", json!({}), Duration::from_millis(200))
        .expect_err("a request nobody answers cannot succeed");
    assert!(matches!(err, LspError::Timeout { .. }), "got {err:?}");

    assert_eq!(
        manager
            .request(LANG, "stub/echo", json!({"still": "here"}))
            .expect("the connection survived")["still"],
        "here",
    );

    manager.stop(LANG);
}
#[test]
fn a_reply_that_arrives_after_its_request_was_answered_goes_nowhere() {
    let (manager, _rx, _uri) = session_with_open_document();

    // The stub answers this id twice: once now, once after the caller has
    // already been given the first answer and moved on.
    assert_eq!(
        manager
            .request(LANG, "stub/lateDuplicate", json!({"delay_ms": 100}))
            .expect("the first answer"),
        "first",
    );
    thread::sleep(Duration::from_millis(250));

    // The superseded reply must not be handed to the next request.
    assert_eq!(
        manager
            .request(LANG, "stub/echo", json!({"fresh": true}))
            .expect("the next request gets its own answer")["fresh"],
        true,
    );

    manager.stop(LANG);
}
#[test]
fn a_framed_message_that_is_not_a_response_is_skipped() {
    let (manager, _rx, _uri) = session_with_open_document();

    assert_eq!(
        manager
            .request(LANG, "stub/garbage", json!({}))
            .expect("the response after the garbage still arrives"),
        "still alive",
    );

    manager.stop(LANG);
}
