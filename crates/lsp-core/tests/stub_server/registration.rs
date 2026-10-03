//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

use std::thread;

/// csharp-ls declares most of its capabilities via dynamic registration
/// rather than up front, so a client that cannot answer
/// `client/registerCapability` never sees them. This drives the stub through
/// the real register-then-unregister sequence on the actual reader thread
/// and checks the registry on the other side of it, not just the unit-level
/// `Registrations` struct.
#[test]
fn register_capability_then_unregister_round_trips_through_the_reader_thread() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(
        !manager.method_registered(LANG, "workspace/didChangeWatchedFiles"),
        "nothing has registered yet",
    );

    // The stub pauses between its register and unregister requests, wide
    // enough that the test thread can observe the registration actually
    // land — through the real reader thread, not a direct call into
    // `Registrations` — before it is taken away again.
    let outcome = thread::scope(|scope| {
        let run = scope.spawn(|| {
            manager
                .request(LANG, "stub/registerCapabilityRun", json!({"pause_ms": 200}))
                .expect("the stub runs its register/unregister sequence")
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && !manager.method_registered(LANG, "workspace/didChangeWatchedFiles")
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            manager.method_registered(LANG, "workspace/didChangeWatchedFiles"),
            "a client that answers client/registerCapability with \"not \
             implemented\" is what makes csharp-ls never see its own \
             capabilities",
        );

        run.join().expect("stub run thread did not panic")
    });

    assert_eq!(outcome["registered"], true);
    assert_eq!(outcome["unregistered"], true);
    assert!(
        !manager.method_registered(LANG, "workspace/didChangeWatchedFiles"),
        "client/unregisterCapability must remove the registration again",
    );

    manager.stop(LANG);
}
/// C5: while the stub's `**/*.rs` watcher (`stub/registerCapabilityRun`) is
/// live, `did_change_watched_files` must filter out the non-matching path,
/// batch the matching ones into one notification, and send nothing at all
/// once the registration is gone again.
#[test]
fn did_change_watched_files_filters_and_batches_through_the_real_registration() {
    use lsp_core::watched_files::FileChangeKind;
    use std::path::PathBuf;

    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    thread::scope(|scope| {
        let run = scope.spawn(|| {
            manager
                .request(LANG, "stub/registerCapabilityRun", json!({"pause_ms": 300}))
                .expect("the stub runs its register/unregister sequence")
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && !manager.method_registered(LANG, "workspace/didChangeWatchedFiles")
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(manager.method_registered(LANG, "workspace/didChangeWatchedFiles"));

        manager
            .did_change_watched_files(
                LANG,
                &[
                    (
                        PathBuf::from("/workspace/src/a.rs"),
                        FileChangeKind::Changed,
                    ),
                    (
                        PathBuf::from("/workspace/README.md"),
                        FileChangeKind::Changed,
                    ),
                ],
            )
            .expect("filtering and sending must not error");

        run.join().expect("stub run thread did not panic");
    });

    let sent = manager
        .request(LANG, "stub/lastWatchedFilesChange", json!({}))
        .expect("stub answers");
    let changes = sent["changes"].as_array().expect("changes array");
    assert_eq!(changes.len(), 1, "only the .rs path matches **/*.rs");
    assert_eq!(changes[0]["uri"], "file:///workspace/src/a.rs");
    assert_eq!(changes[0]["type"], FileChangeKind::Changed as u8);

    // The registration is gone again after the stub's unregister — sending
    // must now be a no-op rather than waking a server that asked for
    // nothing.
    assert!(!manager.method_registered(LANG, "workspace/didChangeWatchedFiles"));
    manager
        .did_change_watched_files(
            LANG,
            &[(
                PathBuf::from("/workspace/src/b.rs"),
                FileChangeKind::Created,
            )],
        )
        .expect("no-op send must not error");
    let after_unregister = manager
        .request(LANG, "stub/lastWatchedFilesChange", json!({}))
        .expect("stub answers");
    assert_eq!(
        after_unregister, sent,
        "no new notification should have been sent once nothing is registered",
    );

    manager.stop(LANG);
}
/// C6: `workspace/configuration` answers the pulled section with the
/// server's configured settings, and any other section with `null` — the
/// full round trip through the real reader thread and `dispatch`, not just
/// `configuration::resolve` in isolation.
#[test]
fn workspace_configuration_answers_the_configured_section_and_nulls_the_rest() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_settings())
        .expect("stub starts");

    let answer = manager
        .request(LANG, "stub/configurationRun", json!({}))
        .expect("the stub's configuration pull does not deadlock us");
    let items = answer.as_array().expect("array reply, one per item");
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0],
        json!({"analyzersEnabled": true}),
        "the configured section"
    );
    assert_eq!(
        items[1],
        serde_json::Value::Null,
        "a section this client has no opinion on"
    );

    manager.stop(LANG);
}
/// C6: `update_settings` replaces the stored settings and sends
/// `workspace/didChangeConfiguration` with them pushed under the server's
/// section (a server that does not re-pull still sees them), so a pull
/// issued afterwards sees the new settings too.
#[test]
fn update_settings_changes_what_the_next_pull_answers() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_settings())
        .expect("stub starts");

    manager
        .update_settings(LANG, json!({"analyzersEnabled": false}))
        .expect("the server is running");

    let answer = manager
        .request(LANG, "stub/configurationRun", json!({}))
        .expect("pull after the settings change");
    assert_eq!(answer[0], json!({"analyzersEnabled": false}));

    let pushed = manager
        .request(LANG, "stub/lastConfigurationChange", json!({}))
        .expect("the stub reports what it was sent");
    assert_eq!(
        pushed,
        json!({"settings": {"csharp": {"analyzersEnabled": false}}}),
        "the notification carries the section's settings, not null"
    );

    manager.stop(LANG);
}

/// A server with no `settings_section` never opted into configuration: its
/// notification keeps `null`, the re-pull trigger.
#[test]
fn update_settings_without_a_section_pushes_null() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    manager.update_settings(LANG, json!({"a": 1})).unwrap();

    let pushed = manager
        .request(LANG, "stub/lastConfigurationChange", json!({}))
        .unwrap();
    assert_eq!(pushed, json!({"settings": null}));
    manager.stop(LANG);
}
/// C6: a language with no running server is reported the same way every
/// other per-language method reports it, not a silent no-op.
#[test]
fn update_settings_on_a_server_that_is_not_running_is_an_error() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    assert!(matches!(
        manager.update_settings(LANG, json!({})),
        Err(LspError::NoServer(_))
    ));
}
