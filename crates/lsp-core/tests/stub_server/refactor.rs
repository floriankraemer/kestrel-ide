//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

use std::sync::Arc;
use std::thread;

/// The refactoring requests RF3 taught the stub. These go through the
/// generic `request` because the typed `LspManager` methods land with RF6 —
/// what is being proven here is that the fixture answers each shape, so the
/// parsers built on top of it can be tested offline.
#[test]
fn code_actions_are_answered_in_every_shape_the_parser_must_handle() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let request = |line: u64| {
        manager
            .request(
                LANG,
                "textDocument/codeAction",
                json!({
                    "textDocument": {"uri": "file:///workspace/main.rs"},
                    "range": {"start": {"line": line, "character": 0},
                              "end": {"line": line, "character": 4}},
                    "context": {"diagnostics": []},
                }),
            )
            .expect("the stub answers")
    };

    let with_edit = request(0);
    assert_eq!(with_edit[0]["kind"], "refactor.extract.function");
    assert!(with_edit[0]["edit"]["documentChanges"].is_array());

    let with_command = request(1);
    assert_eq!(
        with_command[0]["command"], "stub.plain",
        "a bare Command has its command as a string",
    );
    assert_eq!(
        with_command[1]["command"]["command"], "stub.applyEdit",
        "a CodeAction has its command as an object",
    );

    assert!(
        request(2)[0]["edit"].is_null(),
        "an unresolved item carries neither an edit nor a command",
    );
    assert_eq!(
        request(3)[0]["disabled"]["reason"],
        "selection is not an expression"
    );
    assert!(request(9).is_null(), "no actions here");
}
#[test]
fn an_unresolved_code_action_gains_its_edit_from_resolve() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let unresolved = json!({"title": "Needs resolving", "kind": "refactor.inline",
                            "data": {"token": 42}});
    let resolved = manager
        .request(LANG, "codeAction/resolve", unresolved)
        .expect("the stub resolves it");

    assert_eq!(
        resolved["title"], "Needs resolving",
        "the item comes back whole"
    );
    assert!(resolved["edit"]["changes"].is_object());
}
#[test]
fn prepare_rename_and_rename_answer_in_every_shape() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let at = |method: &'static str, line: u64, extra: serde_json::Value| {
        let mut params = json!({
            "textDocument": {"uri": "file:///workspace/main.rs"},
            "position": {"line": line, "character": 5},
        });
        if let Some(object) = extra.as_object() {
            for (key, value) in object {
                params[key] = value.clone();
            }
        }
        manager.request(LANG, method, params)
    };

    assert!(
        at("textDocument/prepareRename", 0, json!({})).unwrap()["start"].is_object(),
        "line 0 answers with a bare Range",
    );
    assert_eq!(
        at("textDocument/prepareRename", 1, json!({})).unwrap()["placeholder"],
        "old_name",
    );
    assert_eq!(
        at("textDocument/prepareRename", 2, json!({})).unwrap()["defaultBehavior"],
        true,
    );
    assert!(at("textDocument/prepareRename", 9, json!({}))
        .unwrap()
        .is_null());

    let rename = json!({"newName": "renamed"});
    let versioned = at("textDocument/rename", 0, rename.clone()).unwrap();
    assert_eq!(
        versioned["documentChanges"][0]["textDocument"]["version"],
        1
    );
    assert_eq!(
        versioned["documentChanges"][0]["edits"][0]["newText"],
        "renamed",
    );

    let legacy = at("textDocument/rename", 1, rename.clone()).unwrap();
    assert!(legacy["changes"]["file:///workspace/main.rs"].is_array());

    assert!(at("textDocument/rename", 2, rename.clone())
        .unwrap()
        .is_null());
    assert!(
        at("textDocument/rename", 9, rename).is_err(),
        "an un-renameable element answers with an error, not a null",
    );
}
/// The command-driven refactoring shape: the server asks *us* to apply an
/// edit in the middle of an `executeCommand` and blocks on the answer.
///
/// With no refactoring in flight the request is refused outright — a server
/// rewriting the user's files unprompted is what that rule exists to stop —
/// and, crucially, refused without stalling anything.
#[test]
fn an_unsolicited_apply_edit_is_refused_without_troubling_the_editor() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    assert!(!manager.refactor_active());

    let answer = manager
        .request(
            LANG,
            "workspace/executeCommand",
            json!({"command": "stub.applyEdit",
                   "arguments": ["file:///workspace/main.rs"]}),
        )
        .expect("the command completes without deadlocking");

    assert_eq!(
        answer["clientApplied"], false,
        "nobody asked for this edit, so the server is told it was not applied",
    );
    assert!(
        !matches!(rx.try_recv(), Ok(LspEvent::ApplyEdit { .. })),
        "an unsolicited edit must never reach the UI",
    );

    let echo = manager
        .request(LANG, "stub/echo", json!({"tag": "after"}))
        .expect("the session survives the round trip");
    assert_eq!(echo["tag"], "after");
}
/// The whole handshake, end to end: a refactoring is in flight, the server
/// asks for an edit mid-command, the editor claims it, and the command
/// completes with the server told it was applied.
#[test]
fn an_edit_asked_for_during_a_refactoring_reaches_the_editor_and_is_applied() {
    let (manager, rx) = LspManager::new("file:///workspace");
    let manager = Arc::new(manager);
    manager.start(&stub_config()).expect("stub starts");

    // The guard is what makes the server's request legitimate. It is held
    // across the command, exactly as a refactoring gesture would hold it.
    let session = manager.begin_refactor();
    assert!(manager.refactor_active());

    let commanded = Arc::clone(&manager);
    let command = thread::spawn(move || {
        commanded.request(
            LANG,
            "workspace/executeCommand",
            json!({"command": "stub.applyEdit",
                   "arguments": ["file:///workspace/main.rs"]}),
        )
    });

    // The editor's side: the edit arrives as an event, with the server still
    // blocked on the gate.
    let (label, edit, gate) = wait_for(&rx, "the applyEdit request", |event| match event {
        LspEvent::ApplyEdit {
            label, edit, gate, ..
        } => Some((label.clone(), edit.clone(), gate.clone())),
        _ => None,
    });
    assert_eq!(label.as_deref(), Some("Extract class"));
    assert_eq!(
        edit["documentChanges"][0]["textDocument"]["uri"], "file:///workspace/main.rs",
        "the raw WorkspaceEdit is handed over for the UI side to parse",
    );
    // The invariant this whole design exists for: the server's read thread
    // is not blocked while the edit waits, so everything else keeps working.
    // A `didOpen` sent now must still come back with its diagnostics.
    manager
        .did_open("file:///workspace/other.rs", LANG, "fn other() {}\n")
        .expect("the session is still usable mid-handshake");
    wait_for(
        &rx,
        "diagnostics while an edit is pending",
        |event| match event {
            LspEvent::Diagnostics { uri, .. } if uri.ends_with("other.rs") => Some(()),
            _ => None,
        },
    );

    assert!(gate.claim(), "the editor takes the edit");

    let answer = command.join().unwrap().expect("the command completes");
    assert_eq!(
        answer["clientApplied"], true,
        "the server hears that its edit was applied",
    );
    drop(session);
    assert!(!manager.refactor_active());
}
/// The gate is bounded: an editor that never answers must not park the
/// server's thread for ever, and must not then be allowed to apply anything.
#[test]
fn the_gate_refuses_a_late_claim_so_the_answer_is_never_a_lie() {
    let (gate, gate_rx) = lsp_core::ApplyEditGate::new();
    assert!(gate.close(), "the wait gave up first");
    assert!(
        !gate.claim(),
        "a UI arriving after the timeout must not apply text the server was told about",
    );
    drop(gate_rx);
}
/// RF6: the typed refactoring requests, driven the way the UI will drive
/// them. The stub varies its answers by position, so one test walks the
/// shapes each parser has to survive.
#[test]
fn the_typed_rename_requests_reach_the_server_and_parse() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let uri = "file:///workspace/main.rs";
    manager
        .did_open(uri, LANG, "fn old_name() {}\n")
        .expect("didOpen");

    let prepared = manager
        .prepare_rename(uri, 1, 5)
        .expect("the stub answers")
        .expect("line 1 is renameable");
    assert_eq!(prepared.placeholder.as_deref(), Some("old_name"));
    assert!(
        manager.prepare_rename(uri, 9, 0).unwrap().is_none(),
        "line 9 cannot be renamed",
    );

    let documents = manager
        .rename(uri, 0, 5, "new_name")
        .expect("rename answers");
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].version, Some(1));
    assert_eq!(documents[0].edits[0].new_text, "new_name");

    // The legacy `changes` shape parses to the same thing, minus the version.
    let legacy = manager
        .rename(uri, 1, 5, "new_name")
        .expect("rename answers");
    assert_eq!(legacy[0].version, None);
    assert_eq!(legacy[0].path, "/workspace/main.rs");

    assert!(
        manager.rename(uri, 2, 5, "new_name").unwrap().is_empty(),
        "a null answer is no edits, which the caller resolves to the index",
    );
}
#[test]
fn the_typed_code_action_requests_reach_the_server_and_parse() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    let uri = "file:///workspace/main.rs";
    manager
        .did_open(uri, LANG, "fn main() {}\n")
        .expect("didOpen");

    let extract = manager
        .code_action(uri, (0, 0), (0, 4), &["refactor.extract"])
        .expect("the stub answers");
    assert_eq!(extract.len(), 1);
    assert_eq!(
        extract[0].kind.as_deref(),
        Some("refactor.extract.function")
    );
    assert!(!extract[0].needs_resolve());

    let unresolved = manager
        .code_action(uri, (2, 0), (2, 4), &[])
        .expect("the stub answers");
    assert!(unresolved[0].needs_resolve());
    let resolved = manager
        .resolve_code_action(LANG, &unresolved[0])
        .expect("resolve answers");
    assert!(
        !resolved[0].needs_resolve(),
        "resolving fills in the edit the server withheld",
    );

    assert!(
        manager
            .code_action(uri, (9, 0), (9, 4), &[])
            .unwrap()
            .is_empty(),
        "no actions here",
    );
}
/// The capabilities are what a server reads to decide whether to offer these
/// features at all, so they are asserted where they actually land: in the
/// `initialize` the stub received.
#[test]
fn the_refactoring_capabilities_are_advertised() {
    let (manager, _rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");

    let capabilities = manager
        .request(LANG, "stub/clientCapabilities", json!({}))
        .expect("the stub hands back what we sent");

    assert_eq!(capabilities["workspace"]["applyEdit"], true);
    assert_eq!(
        capabilities["workspace"]["workspaceEdit"]["documentChanges"], true,
        "versions are what let a stale edit be caught before it is applied",
    );
    assert_eq!(
        capabilities["workspace"]["workspaceEdit"]["resourceOperations"],
        json!(["create", "rename", "delete"]),
        "advertised because app_core::apply_file_ops performs them (F2); \
         without these, every extract-to-new-file refactoring is refused whole",
    );
    assert_eq!(
        capabilities["workspace"]["workspaceEdit"]["failureHandling"],
        "abort",
    );
    assert!(capabilities["workspace"]["executeCommand"].is_object());
    assert_eq!(
        capabilities["textDocument"]["rename"]["prepareSupport"],
        true
    );
    assert_eq!(
        capabilities["textDocument"]["codeAction"]["resolveSupport"]["properties"],
        json!(["edit"]),
    );
    assert_eq!(
        capabilities["textDocument"]["codeAction"]["disabledSupport"], true,
        "a disabled action is shown greyed, so servers may send them",
    );
    assert!(
        capabilities["textDocument"]["codeAction"]["codeActionLiteralSupport"]["codeActionKind"]
            ["valueSet"]
            .as_array()
            .expect("a kind list")
            .contains(&json!("refactor.extract"))
    );
}
