//! C10: a fake `docker`/`podman` CLI, so `e2e_containers` (`crates/app/
//! tests/e2e_containers.rs`) needs no real engine installed — the same role
//! `lsp_core::bin::stub_server` and `analysis_core::bin::stub_analyzer` play
//! for their own E2E flows.
//!
//! Two environment variables drive it:
//!
//! - `IDE_STUB_ENGINE_DATA`: the directory canned answers come from.
//!   Defaults to `container-core`'s own `testdata/` directory (this file's
//!   crate root) — the E2E fixture reuses the exact same `inspect/{docker,
//!   podman}` and `probe/*_version.json` files the unit tests already
//!   exercise, rather than a parallel copy, exactly as the C10 plan
//!   entry asks for.
//! - `IDE_STUB_ENGINE_LOG`: a file this binary appends one JSON line to per
//!   invocation (`{"argv":[...]}`), so a test can assert exactly what
//!   `container-core` shelled out to.
//!
//! Called as `docker` or `podman` (a symlink/copy the test sets up on a
//! scratch `PATH` entry — see that test file's own doc comment for why
//! `env!("CARGO_BIN_EXE_stub_engine")` cannot be used directly from
//! `crates/app`): `argv[0]`'s basename picks which engine's canned
//! subdirectory answers `ps -aq`/`inspect`/etc, so the same binary serves
//! both of the fixture's connections.
//!
//! Known ceiling: every canned answer is static, keyed only by resource
//! kind and (for inspect) the ids the caller actually asked for — good
//! enough for the read-mostly E2E flows this backs (dock tree, Start,
//! Log tab, run-config preview, gutter/lens derivation), not a real
//! engine's state machine. `start`/`stop`/etc. don't mutate anything a
//! later `ps -aq` would reflect; a test that needs that would have to grow
//! this stub first.

use std::env;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

fn main() {
    let raw_args: Vec<String> = env::args().collect();
    let program = raw_args.first().cloned().unwrap_or_default();
    let args = &raw_args[1.min(raw_args.len())..];

    log_invocation(&raw_args);

    let engine = engine_name(&program);
    let data = data_dir();
    let code = dispatch(&engine, &data, args);
    std::process::exit(code);
}

/// `argv[0]`'s basename, `.exe` stripped, lowercased — `"podman"` if that's
/// what it contains, `"docker"` for anything else (including a plain
/// `stub_engine` run with no engine-shaped name, so `cargo run --bin
/// stub_engine -- version --format json` works for a quick manual check).
fn engine_name(program: &str) -> String {
    let base = Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("docker")
        .to_lowercase();
    if base.contains("podman") {
        "podman".to_string()
    } else {
        "docker".to_string()
    }
}

/// `container-core`'s own `testdata/` — a compile-time default, so it is
/// correct wherever the workspace was checked out, matching `IDE_STUB_
/// ENGINE_DATA`'s doc comment above.
fn default_data_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"))
}

fn data_dir() -> PathBuf {
    env::var_os("IDE_STUB_ENGINE_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir)
}

fn log_invocation(argv: &[String]) {
    let Some(path) = env::var_os("IDE_STUB_ENGINE_LOG") else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let line = serde_json::json!({ "argv": argv });
    let _ = writeln!(file, "{line}");
}

// --- canned-data helpers -----------------------------------------------

fn read_json_array(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(items)) => items,
        Ok(Value::Null) => Vec::new(),
        Ok(other) => vec![other],
        Err(_) => Vec::new(),
    }
}

fn read_to_string(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn str_field<'a>(entry: &'a Value, field: &str) -> &'a str {
    entry.get(field).and_then(Value::as_str).unwrap_or("")
}

/// Docker/Podman's own short id: 12 hex characters, `sha256:` stripped —
/// what `ps -aq`/`image ls -q` print, and what a real caller's `inspect`
/// argv then carries back in here.
fn short_id(id: &str) -> String {
    id.strip_prefix("sha256:")
        .unwrap_or(id)
        .chars()
        .take(12)
        .collect()
}

/// Every non-flag token in `args` — the id list for `start`/`stop`/`rm`/
/// `inspect`/etc, whose only flags are boolean (`-f`, `-it`) or, for
/// `exec`'s `-u 0`, a flag-then-value pair this also skips.
fn ids_only(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "-u" {
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn print_ids(entries: &[Value], field: &str) {
    let mut seen = Vec::new();
    for entry in entries {
        let id = short_id(str_field(entry, field));
        if id.is_empty() || seen.contains(&id) {
            continue;
        }
        println!("{id}");
        seen.push(id);
    }
}

/// `inspect`'s answer: every entry whose `field` (prefix-matched, the way
/// a short id matches a full one) is one of `requested`. Unmatched
/// requested ids are silently dropped rather than erroring — good enough
/// for a stub nothing here asks to fail this way.
fn inspect(entries: &[Value], field: &str, requested: &[String]) -> i32 {
    let matches: Vec<&Value> = entries
        .iter()
        .filter(|entry| {
            let value = str_field(entry, field);
            requested
                .iter()
                .any(|id| value.starts_with(id.as_str()) || id.starts_with(&short_id(value)))
        })
        .collect();
    println!("{}", serde_json::to_string(&matches).unwrap_or_default());
    0
}

/// A long-running command's blocking half: sleeps forever until the test
/// harness kills this process (`Child::kill`/`PtySession` teardown) — the
/// same shape a real `logs -f`/`events`/`compose up` never returns from on
/// its own.
fn block_forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

// --- dispatch ------------------------------------------------------------

fn dispatch(engine: &str, data: &Path, args: &[String]) -> i32 {
    let first = args.first().map(String::as_str).unwrap_or("");
    let second = args.get(1).map(String::as_str).unwrap_or("");

    match (first, second) {
        ("version", _) if args.contains(&"--format".to_string()) => {
            let path = data.join("probe").join(format!("{engine}_version.json"));
            match read_to_string(&path) {
                Some(text) => {
                    print!("{text}");
                    0
                }
                None => engine_error(engine, "version"),
            }
        }
        ("ps", "-aq") => {
            print_ids(&containers(data, engine), "Id");
            0
        }
        ("container", "inspect") => inspect(&containers(data, engine), "Id", &ids_only(&args[2..])),
        ("image", "ls") => {
            print_ids(&images(data, engine), "Id");
            0
        }
        ("image", "inspect") => inspect(&images(data, engine), "Id", &ids_only(&args[2..])),
        ("volume", "ls") => {
            print_ids(&volumes(data, engine), "Name");
            0
        }
        ("volume", "inspect") => inspect(&volumes(data, engine), "Name", &ids_only(&args[2..])),
        ("network", "ls") => {
            print_ids(&networks(data, engine), "Id");
            0
        }
        ("network", "inspect") => inspect(&networks(data, engine), "Id", &ids_only(&args[2..])),
        ("pod", "ls") if engine == "podman" => {
            let path = data.join("inspect/podman/pods.json");
            println!(
                "{}",
                read_to_string(&path).unwrap_or_else(|| "[]".to_string())
            );
            0
        }
        ("events", _) => {
            if let Ok(after) = env::var("IDE_STUB_ENGINE_EVENT_AFTER_MS") {
                if let Ok(ms) = after.parse::<u64>() {
                    std::thread::sleep(Duration::from_millis(ms));
                    let path = data.join("inspect").join(engine).join("events.jsonl");
                    if let Some(text) = read_to_string(&path) {
                        if let Some(line) = text.lines().next() {
                            println!("{line}");
                            let _ = std::io::stdout().flush();
                        }
                    }
                }
            }
            block_forever()
        }
        ("start", _) | ("stop", _) | ("restart", _) | ("pause", _) | ("unpause", _) => {
            for id in ids_only(&args[1..]) {
                println!("{id}");
            }
            0
        }
        ("rm", _) => {
            for id in ids_only(&args[1..]) {
                println!("{id}");
            }
            0
        }
        ("container", "prune")
        | ("image", "prune")
        | ("network", "prune")
        | ("volume", "prune") => {
            println!("Total reclaimed space: 0B");
            0
        }
        ("top", _) => {
            let path = data.join("top.txt");
            print!(
                "{}",
                read_to_string(&path).unwrap_or_else(|| {
                    "PID   USER   TIME   COMMAND\n1     root   0:00   sh\n".to_string()
                })
            );
            0
        }
        ("logs", _) => {
            let path = data.join("logs/lines.txt");
            if let Some(text) = read_to_string(&path) {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            block_forever()
        }
        ("attach", _) => block_forever(),
        ("exec", _) => {
            // `-it`/`-u 0` are `exec`'s own flags, ahead of the id; the
            // command that follows keeps its own flags (`-la`) untouched —
            // `ids_only` would strip those too, so this walks `args`
            // itself rather than reusing it.
            let mut rest = args[1..].iter();
            let mut id = None;
            while let Some(token) = rest.next() {
                if token == "-u" {
                    rest.next();
                    continue;
                }
                if token.starts_with('-') {
                    continue;
                }
                id = Some(token);
                break;
            }
            let command: Vec<&str> = rest.map(String::as_str).collect();
            // `ls -la ...` (the Files tab) gets a canned listing; anything
            // else (the Terminal/Exec shell probe) just succeeds with no
            // output, since nothing in this suite drives an actual
            // interactive shell through the stub.
            if id.is_some() && command.first() == Some(&"ls") {
                let path = data.join("files/ls.txt");
                print!(
                    "{}",
                    read_to_string(&path).unwrap_or_else(|| "total 0\n".to_string())
                );
            }
            0
        }
        ("pull", _) => {
            let reference = args.get(1).cloned().unwrap_or_default();
            println!("Using default tag: latest");
            println!("latest: Pulling from {reference}");
            println!(
                "Digest: sha256:0000000000000000000000000000000000000000000000000000000000000000"
            );
            println!("Status: Downloaded newer image for {reference}");
            0
        }
        ("tag", _) | ("rmi", _) | ("save", _) | ("load", _) => 0,
        ("history", _) => {
            let path = if engine == "podman" {
                data.join("history/podman.json")
            } else {
                data.join("history/docker.jsonl")
            };
            print!("{}", read_to_string(&path).unwrap_or_default());
            0
        }
        ("context", "ls") => {
            let path = data.join("discovery/docker_context_ls.jsonl");
            print!("{}", read_to_string(&path).unwrap_or_default());
            0
        }
        ("machine", "list") if engine == "podman" => {
            let path = data.join("machine/list.json");
            println!(
                "{}",
                read_to_string(&path).unwrap_or_else(|| "[]".to_string())
            );
            0
        }
        ("machine", "start") | ("machine", "stop") if engine == "podman" => 0,
        ("compose", _) => compose(data, args),
        _ => engine_error(engine, first),
    }
}

fn compose(data: &Path, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--services") {
        let path = data.join("compose/services.txt");
        print!(
            "{}",
            read_to_string(&path).unwrap_or_else(|| "web\ndb\n".to_string())
        );
        return 0;
    }
    if args.iter().any(|a| a == "config") {
        let path = data.join("compose/config.json");
        println!(
            "{}",
            read_to_string(&path).unwrap_or_else(|| "{}".to_string())
        );
        return 0;
    }
    if args.iter().any(|a| a == "up") {
        println!("Creating network...");
        println!("Container started");
        block_forever();
    }
    // down/stop/scale: nothing further needed by this suite.
    0
}

fn engine_error(engine: &str, subcommand: &str) -> i32 {
    eprintln!("{engine}: '{subcommand}' is not a {engine} command.");
    eprintln!("See '{engine} --help'.");
    1
}

fn containers(data: &Path, engine: &str) -> Vec<Value> {
    read_json_array(&data.join("inspect").join(engine).join("containers.json"))
}

fn images(data: &Path, engine: &str) -> Vec<Value> {
    read_json_array(&data.join("inspect").join(engine).join("images.json"))
}

fn volumes(data: &Path, engine: &str) -> Vec<Value> {
    read_json_array(&data.join("inspect").join(engine).join("volumes.json"))
}

fn networks(data: &Path, engine: &str) -> Vec<Value> {
    read_json_array(&data.join("inspect").join(engine).join("networks.json"))
}
