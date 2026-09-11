//! W2-1: `vcs_core::cli::run` needs zero changes to run `git` inside a WSL
//! distro — `process_exec::run` classifies `work_dir` and wraps the argv on
//! its own (W1-7). This proves that claim end to end with the same fake
//! `wsl.exe`-on-`PATH` trick `process-exec`'s own tests use, rather than
//! trusting it by inspection.
//!
//! `gix::discover`'s own cwd spelling is untouched by this: `ExecHost` only
//! ever looks at the path string it is handed, and `to_remote` tolerates
//! any of the four UNC spellings regardless of which one `gix` or `Qt`
//! produced (see `process_exec::host`'s
//! `to_remote_tolerates_a_different_spelling_than_the_stored_host` test) —
//! so there is nothing for `vcs-core` itself to special-case.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

static PATH_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn cli_run_wraps_git_through_wsl_exe_for_a_remote_work_dir() {
    let _guard = PATH_LOCK.lock().unwrap();

    let bin_dir = tempfile::tempdir().unwrap();
    let script_path = bin_dir.path().join("wsl.exe");
    let mut script = fs::File::create(&script_path).unwrap();
    write!(
        script,
        r#"#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = "-lc" ]; then
        echo "git"
        exit 0
    fi
done
first=1
for arg in "$@"; do
    if [ "$first" = 1 ]; then first=0; else printf ' '; fi
    printf '%s' "$arg"
done
"#
    )
    .unwrap();
    drop(script);
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    // Not created on disk, and deliberately so: the WSL root is only ever
    // parsed for its UNC spelling — wsl.exe is handed the distro path it
    // names, and the process itself is spawned from a local directory
    // (#255). Creating it would write `/wsl.localhost` at the filesystem
    // root, which only succeeds when the test runs as root (see #251, #252
    // for the same trap in analysis-core and lsp-core).
    let work_dir = std::path::PathBuf::from("//wsl.localhost/Ubuntu/tmp/vcs-core-e2e");

    let original_path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: serialized by PATH_LOCK; nothing else in this test binary
    // touches PATH concurrently.
    unsafe {
        std::env::set_var(
            "PATH",
            format!("{}:{original_path}", bin_dir.path().display()),
        );
    }
    let result = vcs_core::cli::run(&work_dir, &["status"]);
    unsafe {
        std::env::set_var("PATH", original_path);
    }

    let stdout = result.expect("git status should succeed against the fake wsl.exe");
    assert_eq!(stdout, "-d Ubuntu --cd /tmp/vcs-core-e2e -e git status");
}
