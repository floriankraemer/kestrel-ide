//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

/// The defect the F0-15 conformance run found: `ServerReady` fires the moment
/// `initialize` returns, but rust-analyzer answers everything with nothing
/// until it has indexed. `$/progress` is the protocol's way of saying so, and
/// the stub sends the same sequence on demand.
#[test]
fn a_progress_run_drives_the_server_from_indexing_to_idle() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    // The stub answers this only after every notification of the run is
    // written, so nothing below races the reader thread.
    let outcome = manager
        .request(LANG, "stub/indexingRun", json!({}))
        .expect("the stub runs its progress sequence");
    assert_eq!(
        outcome["created"], true,
        "a client that refuses window/workDoneProgress/create is what makes a \
         real server stop reporting progress at all",
    );

    let states: Vec<Option<lsp_core::ServerActivity>> = (0..3)
        .map(|_| {
            wait_for(&rx, "ServerBusy", |e| match e {
                LspEvent::ServerBusy { activity, .. } => Some(activity.clone()),
                _ => None,
            })
        })
        .collect();

    assert_eq!(
        states,
        vec![
            Some(lsp_core::ServerActivity {
                title: "Indexing".into(),
                percentage: Some(0),
            }),
            Some(lsp_core::ServerActivity {
                title: "Indexing".into(),
                percentage: Some(60),
            }),
            // `end` closes the last open token: the server is idle, and its
            // empty answers now mean "no answer exists" rather than "not yet".
            None,
        ],
    );

    manager.stop(LANG);
}
/// Most servers never send `$/progress` at all. The state is advisory and
/// nothing waits on it, so such a server is usable from `ServerReady` onwards
/// and simply never reports being busy — the alternative, treating "no
/// progress yet" as "not ready", would hang the IDE on every one of them.
#[test]
fn a_server_that_never_reports_progress_is_usable_immediately() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    // Answered, not deferred: readiness is not gated on progress.
    let answer = manager
        .request(LANG, "stub/echo", json!({"value": "hello"}))
        .expect("a request is served without any progress having been reported");
    assert_eq!(answer["value"], "hello");

    // And no busy state was ever claimed for it.
    let deadline = Instant::now() + Duration::from_millis(300);
    while let Ok(event) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        assert!(
            !matches!(event, LspEvent::ServerBusy { .. }),
            "a silent server must never be reported busy: {event:?}",
        );
    }

    manager.stop(LANG);
}
/// A server that dies mid-index can never end the token it opened, so the
/// manager ends it on the server's behalf. Without this the status bar would
/// sit on a dead server's last percentage forever.
#[test]
fn a_server_that_dies_mid_progress_stops_being_busy() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });
    // A run that never ends its token, the way a server killed mid-index
    // leaves it.
    manager
        .request(LANG, "stub/indexingRun", json!({"finish": false}))
        .expect("the stub begins work and leaves it open");
    wait_for(&rx, "ServerBusy", |e| match e {
        LspEvent::ServerBusy { activity, .. } if activity.is_some() => Some(()),
        _ => None,
    });

    manager.stop(LANG);

    // Nothing more is emitted after a stop (the supervisor returns), so the
    // clear has to arrive before the connection is torn down — assert on the
    // events already queued.
    let cleared = rx
        .try_iter()
        .any(|e| matches!(e, LspEvent::ServerBusy { activity: None, .. }));
    assert!(cleared, "a dead server's open work is closed on its behalf");
}
/// F0-16: without this capability a server has no permission to open a
/// progress token, and rust-analyzer stays silent for exactly the seconds in
/// which it cannot answer anything.
#[test]
fn work_done_progress_is_advertised() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let capabilities = manager
        .request(LANG, "stub/clientCapabilities", json!({}))
        .expect("the stub hands back what we sent");
    assert_eq!(capabilities["window"]["workDoneProgress"], true);

    manager.stop(LANG);
}
