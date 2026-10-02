use super::*;

fn wsl_host() -> ExecHost {
    ExecHost::for_path(Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj"))
}

#[test]
fn uri_for_is_unchanged_on_a_local_host() {
    assert_eq!(
        uri_for(&ExecHost::Local, "/home/f/proj/src/main.rs"),
        crate::diagnostics::uri_from_path("/home/f/proj/src/main.rs")
    );
}

#[test]
fn uri_for_translates_a_windows_unc_path_to_a_linux_file_uri() {
    let host = wsl_host();
    assert_eq!(
        uri_for(&host, r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs"),
        "file:///home/f/proj/src/main.rs"
    );
}

#[test]
fn path_for_translates_a_linux_file_uri_back_to_the_unc_path() {
    let host = wsl_host();
    assert_eq!(
        path_for(&host, "file:///home/f/proj/src/main.rs"),
        Some(r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs".to_string())
    );
}

#[test]
fn uri_for_then_path_for_round_trips_to_identity() {
    let host = wsl_host();
    let original = r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs";
    let uri = uri_for(&host, original);
    assert_eq!(path_for(&host, &uri).as_deref(), Some(original));
}

#[test]
fn new_translates_root_uri_for_a_wsl_root() {
    let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path(
        "//wsl.localhost/Ubuntu/home/f/proj",
    ));
    assert_eq!(manager.root_uri, "file:///home/f/proj");
    assert!(manager.host.is_remote());
}

#[test]
fn new_leaves_root_uri_unchanged_for_a_local_root() {
    let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path("/home/f/proj"));
    assert_eq!(manager.root_uri, "file:///home/f/proj");
    assert!(!manager.host.is_remote());
}

#[test]
fn a_server_on_the_projects_own_host_gets_the_uri_unchanged() {
    let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path(
        "//wsl.localhost/Ubuntu/home/f/proj",
    ));
    let uri = "file:///home/f/proj/src/main.rs";
    assert_eq!(manager.uri_on(&manager.host.clone(), uri), uri);
}

#[test]
fn a_server_on_another_host_gets_the_uri_that_host_spells() {
    let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path(
        "//wsl.localhost/Ubuntu/home/f/proj",
    ));
    // The project is in the distro; this server runs on the Windows side
    // and sees the document through its UNC path.
    assert_eq!(
        manager.uri_on(&ExecHost::Local, "file:///home/f/proj/src/main.rs"),
        "file:////wsl.localhost/Ubuntu/home/f/proj/src/main.rs"
    );
}

/// normalize_uri applied twice must be a no-op — `format_range` falling
/// back to `self.format(uri, options)` and every other re-entrant call
/// in this crate depends on it.
#[test]
fn normalize_uri_is_idempotent() {
    let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path(
        "//wsl.localhost/Ubuntu/home/f/proj",
    ));
    let naive = crate::diagnostics::uri_from_path("//wsl.localhost/Ubuntu/home/f/proj/src/main.rs");
    let once = manager.normalize_uri(&naive);
    let twice = manager.normalize_uri(&once);
    assert_eq!(once, "file:///home/f/proj/src/main.rs");
    assert_eq!(once, twice);
}

/// W3-3: a server binary absent from the distro is reported plainly,
/// not as a generic spawn failure — proven with the same fake-`wsl.exe`
/// trick `process-exec`'s own tests use, since `command -v` (and
/// therefore `resolve_program`) genuinely finds nothing for it.
#[test]
fn starting_a_server_missing_from_the_distro_says_so() {
    use std::sync::Mutex;
    static PATH_LOCK: Mutex<()> = Mutex::new(());
    let _guard = PATH_LOCK.lock().unwrap();

    let bin_dir = tempfile::tempdir().unwrap();
    let script_path = bin_dir.path().join("wsl.exe");
    std::fs::write(&script_path, "#!/bin/sh\nexit 1\n").unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
    }
    std::fs::set_permissions(&script_path, perms).unwrap();

    // Not created on disk, and deliberately so: the WSL root is only
    // ever parsed for its UNC spelling — nothing here touches the
    // filesystem under it. Creating it would write `/wsl.localhost` at
    // the filesystem root, which only succeeds when the test runs as
    // root (see #251 for the same trap in analysis-core).
    let root = PathBuf::from("//wsl.localhost/Ubuntu/tmp/lsp-core-e2e");

    let original_path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: serialized by PATH_LOCK.
    unsafe {
        std::env::set_var(
            "PATH",
            format!("{}:{original_path}", bin_dir.path().display()),
        );
    }
    let (manager, _rx) =
        LspManager::new(crate::diagnostics::uri_from_path(&root.to_string_lossy()));
    let result = manager.start(&ServerConfig {
        id: "rust".to_string(),
        language_id: "rust".to_string(),
        name: "rust-analyzer".to_string(),
        command: "rust-analyzer".to_string(),
        args: Vec::new(),
        enabled: true,
        settings_section: None,
        settings: Value::Null,
        initialization_options: Value::Null,
        diagnostics: true,
        posix_only: false,
        exec: crate::catalog::ServerExec::Host,
        priority: 0,
        source: crate::catalog::ServerSource::Builtin,
    });
    unsafe {
        std::env::set_var("PATH", original_path);
    }

    let err = result.unwrap_err();
    assert!(
        matches!(&err, LspError::Spawn { source, .. } if source.to_string().contains("not found inside the WSL distro")),
        "expected a WSL-specific not-found message, got {err:?}"
    );
}
