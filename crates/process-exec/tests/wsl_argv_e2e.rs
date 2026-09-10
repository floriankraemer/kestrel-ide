//! Proves the whole `process_exec::run` argv-wrapping path — classification,
//! `-d`/`--cd`/`-e`, `WSLENV` — without a real Windows machine or a real
//! WSL install: a fake `wsl.exe` script on `PATH` echoes its own argv back
//! as JSON, the same trick `analysis-core`'s scheduler tests use against
//! `/bin/sh`. See `docs/architecture/remote-wsl-plan.md`'s Verification
//! section.
//!
//! The work_dir is a UNC path that exists nowhere:
//! [`process_exec::host::ExecHost::for_path`] classifies it by spelling
//! alone, and a remote command is not launched from its work_dir, so the
//! fake `wsl.exe` runs with no such directory on disk.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

// `PATH` is process-global; serialize the two tests in this binary that
// touch it so they cannot see each other's prepended fake-bin directory.
static PATH_LOCK: Mutex<()> = Mutex::new(());

/// A fake `wsl.exe` on `PATH`, in its own tempdir, plus the UNC-spelled
/// `work_dir` for `run` — a path that is classified, never opened.
struct Fixture {
    _bin_dir: tempfile::TempDir,
    work_dir: PathBuf,
}

fn build_fixture(distro: &str, remote_tail: &str) -> Fixture {
    let bin_dir = tempfile::tempdir().unwrap();
    let script_path = bin_dir.path().join("wsl.exe");
    let mut script = fs::File::create(&script_path).unwrap();
    // Emits its own argv (skipping argv[0]) as a JSON array of strings.
    write!(
        script,
        r#"#!/bin/sh
# `resolve_program`'s login-shell probe (`-e /bin/sh -lc 'command -v git'`)
# hits this same fake `wsl.exe` first; answer it directly so the real
# argv-capturing run below still sees the plain program name "git", not a
# probe response.
for arg in "$@"; do
    if [ "$arg" = "-lc" ]; then
        echo "git"
        exit 0
    fi
done
printf '['
first=1
for arg in "$@"; do
    if [ "$first" = 1 ]; then first=0; else printf ','; fi
    printf '"%s"' "$(printf '%s' "$arg" | sed 's/\\/\\\\/g; s/"/\\"/g')"
done
printf ']'
"#
    )
    .unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    // Deliberately not created on disk: `ExecHost` only parses the UNC
    // spelling of a remote work_dir, and `wsl.exe` is no longer launched
    // *from* it (see `ExecHost::command`), so nothing here touches the path.
    // Creating it would write `/wsl.localhost` at the filesystem root, which
    // only succeeds when the test runs as root — the trap #251/#252 hit.
    let work_dir = PathBuf::from(format!("//wsl.localhost/{distro}/{remote_tail}"));

    Fixture {
        _bin_dir: bin_dir,
        work_dir,
    }
}

fn with_fake_wsl_on_path(distro: &str, remote_tail: &str) -> String {
    let _guard = PATH_LOCK.lock().unwrap();
    let fixture = build_fixture(distro, remote_tail);
    let original_path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: serialized by PATH_LOCK above; nothing else in this binary
    // reads/writes PATH concurrently.
    unsafe {
        std::env::set_var(
            "PATH",
            format!("{}:{original_path}", fixture._bin_dir.path().display()),
        );
    }
    let output = process_exec::run(
        "git",
        &["status", "--grep=a b $x"],
        &fixture.work_dir,
        None,
        Duration::from_secs(5),
        &[("GIT_TERMINAL_PROMPT", "0")],
    )
    .unwrap();
    // SAFETY: same guard.
    unsafe {
        std::env::set_var("PATH", original_path);
    }
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn run_wraps_a_remote_work_dir_in_wsl_exe_with_the_expected_argv() {
    let stdout = with_fake_wsl_on_path("Ubuntu", "tmp/x");

    let argv: Vec<String> = serde_json_lite_parse(&stdout);
    assert_eq!(
        argv,
        vec![
            "-d".to_string(),
            "Ubuntu".to_string(),
            "--cd".to_string(),
            "/tmp/x".to_string(),
            "-e".to_string(),
            "git".to_string(),
            "status".to_string(),
            "--grep=a b $x".to_string(),
        ]
    );
}

/// A minimal JSON-string-array parser — the fake script's whole output
/// shape — so this test needs no `serde_json` dev-dependency.
fn serde_json_lite_parse(json: &str) -> Vec<String> {
    let inner = json.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split("\",\"")
        .map(|s| {
            s.trim_matches('"')
                .replace("\\\"", "\"")
                .replace("\\\\", "\\")
        })
        .collect()
}
