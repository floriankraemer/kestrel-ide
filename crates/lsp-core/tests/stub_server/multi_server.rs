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
    let caps = manager.capabilities(LANG);
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
