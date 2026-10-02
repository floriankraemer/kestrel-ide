//! ADR-0067: a server running in a container (modelled by a `sh` that drops
//! the `-w <cwd>` the container host puts before the command) reads and
//! answers in the container's paths, while the caller keeps the project's.

use crate::support::*;
use process_exec::host::{ContainerHost, ExecHost, PathMap};

/// `wire_log` receives a copy of everything the client wrote to the server.
fn container_host(root: &std::path::Path, wire_log: &std::path::Path) -> ExecHost {
    ExecHost::Container(ContainerHost {
        program: "sh".into(),
        prefix_args: vec![
            "-c".into(),
            format!(r#"shift 2; tee {} | "$@""#, wire_log.display()),
            "sh".into(),
        ],
        engine_env: vec![],
        via_wsl: false,
        verb_args: vec![],
        target: vec![],
        path_map: PathMap::new(root, "/workspace"),
    })
}

#[test]
fn the_root_and_published_diagnostics_use_each_sides_own_paths() {
    let root = tempfile::tempdir().unwrap();
    let wire_log = root.path().join("wire.log");
    let (manager, rx) = LspManager::new(lsp_core::uri_from_path(&root.path().to_string_lossy()));
    manager
        .start_on(
            &tagged_config("a", ""),
            container_host(root.path(), &wire_log),
        )
        .expect("the stub starts through the fake container");

    let init = manager
        .request(LANG, "stub/initializeParams", json!({}))
        .unwrap();
    assert!(init["processId"].is_null(), "{init}");

    let file = root.path().join("a.php");
    let local_uri = lsp_core::uri_from_path(&file.to_string_lossy());
    manager.did_open(&local_uri, LANG, "<?php").unwrap();

    // The stub publishes under the URI it was told (`/workspace/a.php`);
    // the caller gets the local file back.
    let published = wait_for(&rx, "diagnostics", |e| match e {
        LspEvent::Diagnostics { uri, .. } => Some(uri.clone()),
        _ => None,
    });
    assert_eq!(published, local_uri);

    let wire = std::fs::read_to_string(&wire_log).unwrap();
    assert!(wire.contains(r#""rootUri":"file:///workspace""#), "{wire}");
    assert!(
        wire.contains(r#""uri":"file:///workspace/a.php""#),
        "{wire}"
    );
    assert!(!wire.contains(&local_uri), "{wire}");
    manager.stop(LANG);
}
