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

fn started_pair(a_caps: &str, b_caps: &str) -> (LspManager, Receiver<LspEvent>) {
    let (manager, rx) = LspManager::new("file:///workspace");
    for (priority, (id, caps)) in [("a", a_caps), ("b", b_caps)].into_iter().enumerate() {
        manager
            .start(&ServerConfig {
                priority,
                ..tagged_config(id, caps)
            })
            .expect("starts");
    }
    (manager, rx)
}

const HOVER_PARAMS: fn() -> serde_json::Value = || {
    json!({"textDocument": {"uri": "file:///workspace/a.stub"},
           "position": {"line": 0, "character": 0}})
};

fn hover_text(manager: &LspManager) -> Result<String, LspError> {
    let result = manager.request(LANG, "textDocument/hover", HOVER_PARAMS())?;
    Ok(result["contents"]["value"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

#[test]
fn a_first_method_goes_to_the_first_capable_server() {
    let (manager, _rx) = started_pair("hover", "hover");
    assert_eq!(hover_text(&manager).unwrap(), "hover a");
    manager.stop(LANG);
}

#[test]
fn a_first_method_skips_a_server_that_does_not_declare_it() {
    let (manager, _rx) = started_pair("", "hover");
    assert_eq!(hover_text(&manager).unwrap(), "hover b");
    manager.stop(LANG);
}

#[test]
fn a_method_no_server_offers_is_method_not_found() {
    let (manager, _rx) = started_pair("", "");
    let err = hover_text(&manager).unwrap_err();
    assert!(
        matches!(err, LspError::Response { code: -32601, .. }),
        "{err:?}"
    );
    manager.stop(LANG);
}

#[test]
fn a_command_goes_to_the_server_that_lists_it() {
    let (manager, _rx) = started_pair("executeCommand", "executeCommand");
    let ran = manager
        .request(
            LANG,
            "workspace/executeCommand",
            json!({"command": "cmd.b", "arguments": []}),
        )
        .unwrap();
    assert_eq!(ran["ranOn"], "b");
    manager.stop(LANG);
}

#[test]
fn a_tagged_item_is_resolved_by_its_origin_and_sent_back_untagged() {
    let (manager, _rx) = started_pair("completion", "completion");
    let mut item = json!({"label": "x"});
    lsp_core::routing::tag_origin(&mut item, "b");
    let resolved = manager
        .request(LANG, "completionItem/resolve", item)
        .unwrap();
    assert_eq!(resolved["detail"], "resolved by b");
    assert!(
        resolved.get(lsp_core::routing::ORIGIN_KEY).is_none(),
        "the server must see the item it wrote: {resolved}"
    );
    manager.stop(LANG);
}

#[test]
fn request_all_returns_every_capable_servers_answer_in_order() {
    let (manager, _rx) = started_pair("hover", "hover");
    let answers = manager.request_all(
        LANG,
        "textDocument/hover",
        HOVER_PARAMS(),
        Duration::from_secs(5),
    );
    let tags: Vec<_> = answers
        .iter()
        .map(|(id, r)| {
            (
                id.as_str(),
                r.as_ref().unwrap()["contents"]["value"].clone(),
            )
        })
        .collect();
    assert_eq!(tags, [("a", json!("hover a")), ("b", json!("hover b"))]);
    manager.stop(LANG);
}

fn open_pair_with_document(
    a_caps: &str,
    b_caps: &str,
) -> (LspManager, Receiver<LspEvent>, &'static str) {
    let (manager, rx) = started_pair(a_caps, b_caps);
    let uri = "file:///workspace/a.stub";
    manager.did_open(uri, LANG, "hello\n").expect("didOpen");
    (manager, rx, uri)
}

#[test]
fn completion_merges_without_duplicates_and_resolves_at_the_origin() {
    let (manager, _rx, uri) = open_pair_with_document("completion", "completion");
    let list = manager.completion(uri, 0, 0).expect("completion");
    let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["shared", "only_a", "only_b"],
        "shared appears once"
    );
    assert!(
        list.is_incomplete,
        "a's list is incomplete, so the merge is"
    );

    let from_b = list.items.iter().find(|i| i.label == "only_b").unwrap();
    let resolved = manager
        .resolve_completion_item(LANG, &from_b.raw)
        .expect("resolve");
    assert_eq!(resolved["detail"], "resolved by b");
    manager.stop(LANG);
}

#[test]
fn code_actions_from_both_servers_run_and_resolve_at_their_origin() {
    let (manager, _rx, uri) =
        open_pair_with_document("codeAction,executeCommand", "codeAction,executeCommand");
    let actions = manager
        .code_action(uri, (0, 0), (0, 1), &[])
        .expect("code actions");
    let titles: Vec<_> = actions.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["fix from a", "run a", "fix from b", "run b"]);

    let fix_b = actions.iter().find(|a| a.title == "fix from b").unwrap();
    let resolved = manager.resolve_code_action(LANG, fix_b).expect("resolve");
    assert_eq!(resolved[0].title, "fix from b (resolved by b)");

    let run_b = actions.iter().find(|a| a.title == "run b").unwrap();
    let command = run_b.command.clone().expect("a command");
    let ran = manager.execute_command(LANG, &command).expect("execute");
    assert_eq!(ran["ranOn"], "b");
    manager.stop(LANG);
}

#[test]
fn references_and_workspace_symbols_merge_without_duplicates() {
    let (manager, _rx, uri) =
        open_pair_with_document("references,workspaceSymbol", "references,workspaceSymbol");
    let refs = manager
        .request(
            LANG,
            "textDocument/references",
            json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 0},
                   "context": {"includeDeclaration": true}}),
        )
        .unwrap();
    assert_eq!(refs.as_array().unwrap().len(), 3, "shared + a + b: {refs}");
    let symbols = manager
        .request(LANG, "workspace/symbol", json!({"query": ""}))
        .unwrap();
    assert_eq!(symbols.as_array().unwrap().len(), 3, "{symbols}");
    manager.stop(LANG);
}

#[test]
fn a_merged_answer_survives_one_server_being_stopped() {
    let (manager, _rx, uri) = open_pair_with_document("completion", "completion");
    manager.stop_server("b");
    let list = manager.completion(uri, 0, 0).expect("completion");
    let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["shared", "only_a"]);
    manager.stop(LANG);
}

#[test]
fn a_server_started_late_gets_the_open_document_and_the_others_do_not_get_it_twice() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&tagged_config("a", "")).expect("a starts");
    let uri = "file:///workspace/a.stub";
    manager.did_open(uri, LANG, "hello\n").unwrap();
    manager.did_change(uri, "hello!\n").unwrap();
    wait_for(&rx, "a's diagnostics", |e| match e {
        LspEvent::Diagnostics { server_id, .. } if server_id == "a" => Some(()),
        _ => None,
    });

    manager
        .start(&ServerConfig {
            priority: 1,
            ..tagged_config("b", "")
        })
        .expect("b starts");
    manager.did_open(uri, LANG, "hello!\n").unwrap();
    wait_for(&rx, "b's diagnostics", |e| match e {
        LspEvent::Diagnostics {
            server_id, version, ..
        } if server_id == "b" => {
            assert_eq!(
                *version,
                Some(2),
                "the version carries on, it does not restart"
            );
            Some(())
        }
        _ => None,
    });
    manager.request(LANG, "stub/tag", json!({})).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(&event, LspEvent::Diagnostics { server_id, .. } if server_id == "a"),
            "a was sent didOpen twice: {event:?}"
        );
    }
    manager.stop(LANG);
}
