//! Split out of `stub_server_session.rs` (#162) once it crossed the
//! file-size ceiling — see `stub_server/support.rs` for the shared harness this
//! draws on.

use crate::support::*;

use std::sync::Arc;
use std::thread;

/// C10: a server with no code-lens capability at all — neither the static
/// capability nor a dynamic registration — must never be asked:
/// `code_lenses_supported` answers `false`, the same generic gate C9's
/// `semantic_tokens_legend` provides for its own feature.
#[test]
fn code_lenses_supported_is_false_for_a_server_that_never_advertised_it() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(!manager.code_lenses_supported(LANG));

    manager.stop(LANG);
}
/// C10: `code_lenses_supported` reads the static `codeLensProvider`
/// capability, and `code_lenses` parses the stub's canned response — one
/// lens that already carries its `command`, one that needs
/// `codeLens/resolve` — through the real reader thread and `dispatch`, not
/// just `lsp_core::code_lens` in isolation.
#[test]
fn code_lenses_are_read_from_the_static_initialize_capability() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_code_lens_static())
        .expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(manager.code_lenses_supported(LANG));

    let lenses = manager
        .code_lenses(LANG, "file:///workspace/main.rs")
        .expect("the stub answers textDocument/codeLens");
    assert_eq!(lenses.len(), 2);
    assert_eq!(lenses[0].range.start_line, 0);
    let command = lenses[0].command.as_ref().expect("already resolved");
    assert_eq!(command.title, "1 reference");
    assert!(!lenses[0].needs_resolve());
    assert!(lenses[1].needs_resolve(), "the second lens only has data");

    let resolved = manager
        .resolve_code_lens(LANG, &lenses[1].raw)
        .expect("the stub resolves it");
    assert_eq!(resolved["command"]["command"], "stub.applyEdit");

    manager.stop(LANG);
}
/// C10: csharp-ls's suspected path — declaring `textDocument/codeLens` via
/// `client/registerCapability` rather than `initialize`'s static result.
/// `code_lenses_supported` must pick it up through the same
/// `Registrations` registry C5's watched-files matching already reads, with
/// no bespoke per-feature storage needed for it (unlike C9's legend, a
/// `CodeLensOptions` carries nothing else this client reads back out).
#[test]
fn code_lenses_supported_is_read_from_a_dynamic_registration() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager.start(&stub_config()).expect("stub starts");
    wait_for(&rx, "ServerReady", |e| match e {
        LspEvent::ServerReady { .. } => Some(()),
        _ => None,
    });

    assert!(
        !manager.code_lenses_supported(LANG),
        "nothing has registered yet"
    );

    let outcome = manager
        .request(LANG, "stub/codeLensRegisterRun", json!({}))
        .expect("the stub runs its registration sequence");
    assert_eq!(outcome["registered"], true);

    assert!(manager.code_lenses_supported(LANG));

    manager.stop(LANG);
}
/// C10 end to end: fetch, resolve, and run a lens's command through the
/// *existing* `workspace/executeCommand` path — no second execution method —
/// with the session gate open around it exactly as a code action's command
/// is run (`run_action` in `ui-shell`). The resolved command is
/// `stub.applyEdit`, so this is the same command-driven-refactoring shape
/// `an_edit_asked_for_during_a_refactoring_reaches_the_editor_and_is_applied`
/// proves for a code action, reached this time by a lens click.
#[test]
fn a_lens_click_resolves_and_executes_through_the_gated_command_path() {
    let (manager, rx) = LspManager::new("file:///workspace");
    let manager = Arc::new(manager);
    manager
        .start(&stub_config_with_code_lens_static())
        .expect("stub starts");

    let lenses = manager
        .code_lenses(LANG, "file:///workspace/main.rs")
        .expect("the stub answers");
    let unresolved = lenses
        .into_iter()
        .find(|l| l.needs_resolve())
        .expect("the second lens needs resolving");

    // The guard is what makes the command's resulting applyEdit legitimate —
    // a lens click is exactly the user gesture the gate exists for.
    let session = manager.begin_refactor();
    assert!(manager.refactor_active());

    let resolved = manager
        .resolve_code_lens(LANG, &unresolved.raw)
        .expect("the stub resolves it");
    let command = lsp_core::parse_code_lenses(&json!([resolved]))
        .into_iter()
        .next()
        .and_then(|l| l.command)
        .expect("the resolved lens carries a command");
    assert_eq!(command.command, "stub.applyEdit");

    let commanded = Arc::clone(&manager);
    let running = thread::spawn(move || commanded.execute_command(LANG, &command));

    let (label, gate) = wait_for(&rx, "the applyEdit request", |event| match event {
        LspEvent::ApplyEdit { label, gate, .. } => Some((label.clone(), gate.clone())),
        _ => None,
    });
    assert_eq!(label.as_deref(), Some("Extract class"));
    assert!(gate.claim(), "the editor takes the edit");

    let answer = running.join().unwrap().expect("the command completes");
    assert_eq!(
        answer["clientApplied"], true,
        "the server hears that its edit was applied",
    );
    drop(session);
    assert!(!manager.refactor_active());

    manager.stop(LANG);
}
/// C10: the other half of the same proof — running a lens's command with no
/// refactoring session open must be refused exactly the way an unsolicited
/// `workspace/applyEdit` already is (`apply_edit::UNSOLICITED_REASON`), not
/// a special case of its own. This is the gate `an_unsolicited_apply_edit_is_refused_without_troubling_the_editor`
/// exercises for a code action, reached here by `execute_command` directly —
/// the same call a lens click makes.
#[test]
fn running_a_lens_command_with_no_session_open_is_refused_like_any_unsolicited_edit() {
    let (manager, rx) = LspManager::new("file:///workspace");
    manager
        .start(&stub_config_with_code_lens_static())
        .expect("stub starts");
    assert!(!manager.refactor_active());

    let command = lsp_core::CommandRef {
        title: "Run".to_string(),
        command: "stub.applyEdit".to_string(),
        arguments: vec![json!("file:///workspace/main.rs")],
    };
    let answer = manager
        .execute_command(LANG, &command)
        .expect("the command completes without deadlocking");

    assert_eq!(
        answer["clientApplied"], false,
        "nobody asked for this edit, so the server is told it was not applied",
    );
    assert!(
        !matches!(rx.try_recv(), Ok(LspEvent::ApplyEdit { .. })),
        "an unsolicited edit must never reach the UI",
    );

    manager.stop(LANG);
}
