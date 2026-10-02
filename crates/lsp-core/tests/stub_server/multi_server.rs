//! ADR-0066: several servers for one language, driven by the tagged stub
//! profile (`src/bin/stub_server/profile.rs`).

use crate::support::*;

#[test]
fn initialization_options_reach_the_server_and_its_capabilities_are_kept() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    let cfg = ServerConfig {
        initialization_options: json!({"licenceKey": "k"}),
        ..tagged_config("a", "hover")
    };
    manager.start(&cfg).expect("stub starts");

    let params = manager
        .request(LANG, "stub/initializeParams", json!({}))
        .unwrap();
    assert_eq!(params["initializationOptions"], json!({"licenceKey": "k"}));
    let caps = manager.capabilities("a");
    assert_eq!(caps["hoverProvider"], json!(true));
    assert!(caps["completionProvider"].is_object(), "the stub's own");
    manager.stop(LANG);
}

#[test]
fn a_server_without_initialization_options_is_sent_none() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&tagged_config("a", "")).expect("stub starts");
    let params = manager
        .request(LANG, "stub/initializeParams", json!({}))
        .unwrap();
    assert!(params.get("initializationOptions").is_none(), "{params}");
    manager.stop(LANG);
}

fn two_servers() -> (LspManager, Receiver<LspEvent>) {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&ServerConfig {
            priority: 0,
            ..tagged_config("a", "")
        })
        .expect("a starts");
    manager
        .start(&ServerConfig {
            priority: 1,
            ..tagged_config("b", "")
        })
        .expect("b starts");
    (manager, rx)
}

#[test]
fn two_servers_share_a_language_and_each_reports_ready_under_its_id() {
    let (manager, rx) = two_servers();
    let mut ready = Vec::new();
    while ready.len() < 2 {
        ready.push(wait_for(&rx, "ServerReady", |e| match e {
            LspEvent::ServerReady { server_id, .. } => Some(server_id.clone()),
            _ => None,
        }));
    }
    ready.sort();
    assert_eq!(ready, ["a", "b"]);
    assert!(manager.is_server_running("a") && manager.is_server_running("b"));
    manager.stop(LANG);
    assert!(!manager.is_running(LANG));
}

#[test]
fn did_open_reaches_every_server_and_each_publishes_under_its_own_id() {
    let (manager, rx) = two_servers();
    manager
        .did_open("file:///workspace/a.stub", LANG, "hello\n")
        .expect("didOpen is sent");
    let mut sources = Vec::new();
    while sources.len() < 2 {
        sources.push(wait_for(&rx, "Diagnostics", |e| match e {
            LspEvent::Diagnostics { server_id, .. } => Some(server_id.clone()),
            _ => None,
        }));
    }
    sources.sort();
    assert_eq!(sources, ["a", "b"]);
    manager.stop(LANG);
}

#[test]
fn stopping_one_server_leaves_the_other_running() {
    let (manager, _rx) = two_servers();
    manager.stop_server("a");
    assert!(!manager.is_server_running("a"));
    assert!(manager.is_server_running("b"));
    assert!(manager.is_running(LANG));
    assert_eq!(
        manager.request(LANG, "stub/tag", json!({})).unwrap(),
        json!("b"),
        "the survivor answers"
    );
    manager.stop(LANG);
}

#[test]
fn the_lower_priority_number_answers_first() {
    let (manager, _rx) = two_servers();
    assert_eq!(
        manager.request(LANG, "stub/tag", json!({})).unwrap(),
        json!("a")
    );
    manager.stop(LANG);
}

#[test]
fn a_server_with_diagnostics_off_publishes_nothing() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&ServerConfig {
            diagnostics: false,
            ..tagged_config("quiet", "")
        })
        .expect("starts");
    manager
        .did_open("file:///workspace/a.stub", LANG, "hello\n")
        .unwrap();
    // A request round trip proves the didOpen was processed first.
    manager.request(LANG, "stub/tag", json!({})).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(event, LspEvent::Diagnostics { .. }), "{event:?}");
    }
    manager.stop(LANG);
}
