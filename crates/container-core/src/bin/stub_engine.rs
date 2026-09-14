//! C10: a fake `docker`/`podman` CLI — the thin shell around
//! `container_core::stub_engine`'s pure logic. See that module's doc
//! comment for the full design (env vars, testdata layout, known
//! ceiling); this file only reads the environment, loads the canned data,
//! calls [`container_core::stub_engine::respond`], and acts on the
//! result — no dispatch logic of its own.

use std::env;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use container_core::stub_engine::{engine_name, log_line, respond, StubData};

fn main() {
    let raw_args: Vec<String> = env::args().collect();
    let program = raw_args.first().cloned().unwrap_or_default();
    let args = raw_args[1.min(raw_args.len())..].to_vec();

    log_invocation(&raw_args);

    let engine = engine_name(&program);
    let data = StubData::load(&data_dir(), &engine);
    let event_after_ms = env::var("IDE_STUB_ENGINE_EVENT_AFTER_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_millis);

    let response = respond(&args, &data, event_after_ms);
    print!("{}", response.stdout);
    eprint!("{}", response.stderr);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    if response.blocks {
        if let Some((delay, line)) = response.defer {
            std::thread::sleep(delay);
            print!("{line}");
            let _ = std::io::stdout().flush();
        }
        block_forever();
    }
    std::process::exit(response.exit_code);
}

/// `container-core`'s own `testdata/` — a compile-time default, so it is
/// correct wherever the workspace was checked out.
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
    let _ = writeln!(file, "{}", log_line(argv));
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
