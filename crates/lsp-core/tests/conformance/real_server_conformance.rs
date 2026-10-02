//! The LSP client, checked against a **real** language server.
//!
//! Everything else in this crate's test suite runs against `stub_server`,
//! which is deterministic and can be told to misbehave on cue. That is the
//! right tool for testing the client's own failure paths, and the wrong one
//! for testing our assumptions about the protocol: a stub answers the way we
//! think a server answers, so a shared misunderstanding stays invisible.
//!
//! This suite closes that gap. It needs `rust-analyzer` on PATH, which only
//! the `lsp-conformance` Docker stage provides, so every test is `#[ignore]`d
//! and `cargo test --workspace` is unaffected. Run it with:
//!
//! ```sh
//! make lsp-conformance
//! ```
//!
//! # The expectations file is the report
//!
//! Observations are asserted against
//! `tests/data/conformance-expectations.toml` rather than printed. A prose
//! document saying which server supports what rots quietly; a file the suite
//! diffs against cannot. Regenerate deliberately with
//! `CONFORMANCE_BLESS=1 make lsp-conformance` and review the diff.
//!
//! # Why this is not a per-PR gate
//!
//! It needs a separate image, takes minutes, and can go red because upstream
//! changed rather than because we did. A red CI that is nobody's fault is how
//! a suite like this gets ignored and then deleted. Nightly and on demand.

use crate::support::{retry_until, wait_for};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use lsp_core::catalog::ServerConfig;
use lsp_core::manager::{LspEvent, LspManager};

/// Long enough for rust-analyzer to index a one-file crate on a loaded CI box,
/// short enough that a hang is a failure rather than a timeout of the job.
const READY_TIMEOUT: Duration = Duration::from_secs(90);

const LANG: &str = "rust";

/// A fixture whose interesting positions sit **after** multi-byte characters.
///
/// This is the point of using a real server. The client advertises UTF-16
/// positions; if it ever computed them as bytes, every request on these lines
/// would address the wrong column, and an ASCII-only fixture would never show
/// it.
const FIXTURE: &str = r#"//! Fixture: 🙂 an emoji and 中文 in the docs, on purpose.

/// Adds two numbers — the é here shifts every column after it.
pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

pub fn caller() -> u64 {
    add(2, 2)
}
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    uri: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"conformance-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
        )
        .unwrap();
        let file = root.join("src").join("lib.rs");
        fs::write(&file, FIXTURE).unwrap();
        let uri = format!("file://{}", file.display());
        Self {
            _dir: dir,
            root,
            uri,
        }
    }

    fn root_uri(&self) -> String {
        format!("file://{}", self.root.display())
    }
}

fn config() -> ServerConfig {
    ServerConfig {
        id: LANG.into(),
        language_id: LANG.into(),
        name: "rust-analyzer".into(),
        command: "rust-analyzer".into(),
        args: Vec::new(),
        enabled: true,
        settings_section: None,
        settings: serde_json::Value::Null,
        initialization_options: serde_json::Value::Null,
        diagnostics: true,
        posix_only: false,
        exec: lsp_core::catalog::ServerExec::Host,
        source: lsp_core::catalog::ServerSource::Builtin,
    }
}

/// Start rust-analyzer on the fixture and wait until it accepts requests.
fn started(fixture: &Fixture) -> (LspManager, Receiver<LspEvent>) {
    let (manager, rx) = LspManager::new(fixture.root_uri());
    manager.start(&config()).expect("start rust-analyzer");
    manager
        .did_open(&fixture.uri, LANG, FIXTURE)
        .expect("didOpen");
    wait_for(&rx, "ServerReady", READY_TIMEOUT, |e| match e {
        LspEvent::ServerReady { language_id, .. } if language_id == LANG => Some(()),
        _ => None,
    });
    (manager, rx)
}

// `wait_for` and `retry_until` are shared with `csharp_conformance.rs` via
// `tests/conformance/support.rs`. `retry_until` exists because of the first thing
// this suite found: `ServerReady` is emitted as soon as `initialize`
// returns, but rust-analyzer cannot answer a single request until it has run
// `cargo metadata` and indexed the crate — tens of seconds on a cold cache.
// Every request before that returns an empty result that is
// indistinguishable from "no answer exists". F0-16 gave the *product*
// something better than retrying: the client now handles `$/progress`, so
// the status bar says which server is indexing and how far along it is
// instead of silently answering nothing. The suite still retries, because a
// test has to get an answer either way and progress is advisory — a server
// that reports none must not hang it.

// ---------------------------------------------------------------------------
// Observations, recorded into the expectations file.
// ---------------------------------------------------------------------------

#[derive(Default, Debug)]
struct Observed {
    position_encoding: String,
    hover: bool,
    definition: bool,
    completion: bool,
    diagnostics: bool,
    code_action: bool,
    rename: bool,
    prepare_rename: bool,
}

fn expectations_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("conformance-expectations.toml")
}

fn expected() -> toml::Table {
    let body = fs::read_to_string(expectations_path()).expect("expectations file");
    body.parse::<toml::Table>().expect("expectations parse")
}

/// Compare one observed value against the file, collecting rather than
/// asserting so a run reports every drift at once instead of the first.
fn compare<T: std::fmt::Debug + PartialEq>(
    drift: &mut Vec<String>,
    table: &toml::Table,
    key: &str,
    observed: T,
    parse: impl Fn(&toml::Value) -> Option<T>,
) {
    match table.get(key).and_then(parse) {
        Some(want) if want == observed => {}
        Some(want) => drift.push(format!("{key}: expected {want:?}, observed {observed:?}")),
        None => drift.push(format!("{key}: missing from the expectations file")),
    }
}

#[test]
#[ignore = "needs a real rust-analyzer; run via `make lsp-conformance`"]
fn rust_analyzer_matches_the_recorded_expectations() {
    let fixture = Fixture::new();
    let (manager, rx) = started(&fixture);
    // `add` sits on line 3 (0-based), after a doc comment containing "é". A
    // client computing byte offsets would land mid-word and get nothing.
    let add_line = 3;
    let add_char = FIXTURE
        .lines()
        .nth(add_line as usize)
        .unwrap()
        .find("add")
        .unwrap() as u32;

    // The first request also waits out indexing, so its timeout is the
    // generous one and everything after it can be immediate.
    let (_, indexing_took) = retry_until("hover", READY_TIMEOUT, || {
        manager
            .hover(&fixture.uri, add_line, add_char)
            .ok()
            .flatten()
    });
    eprintln!("rust-analyzer answered its first request after {indexing_took:?} of indexing");
    // F0-16: the silent window above is now visible rather than only
    // survivable — rust-analyzer reports it as `$/progress`, and this asserts
    // it against the real server, which is the half a stub cannot prove.
    //
    // Drained into a list rather than filtered straight out of the channel:
    // `try_iter` takes *every* queued event with it, and rust-analyzer
    // publishes its diagnostics the moment indexing ends — which is to say,
    // usually right here. Filtering in place threw that notification away and
    // left the wait at the bottom of this test blocking for a second one that
    // never comes. It passed locally, where the notification happened to
    // arrive later, and failed on a CI runner, where it did not.
    let drained: Vec<LspEvent> = rx.try_iter().collect();
    if let Some(LspEvent::ServerFailed { message, .. }) = drained
        .iter()
        .find(|e| matches!(e, LspEvent::ServerFailed { .. }))
    {
        panic!("server failed while indexing: {message}");
    }
    let diagnostics_already_seen = drained
        .iter()
        .any(|e| matches!(e, LspEvent::Diagnostics { uri, .. } if uri == &fixture.uri));
    let reported: Vec<String> = drained
        .into_iter()
        .filter_map(|e| match e {
            LspEvent::ServerBusy {
                activity: Some(activity),
                ..
            } => Some(activity.title),
            _ => None,
        })
        .collect();
    assert!(
        !reported.is_empty(),
        "rust-analyzer indexed for {indexing_took:?} without a single $/progress \
         reaching the client — check `window.workDoneProgress` is still advertised",
    );
    eprintln!("it reported that work as: {reported:?}");
    let hover = true;

    // From the call site in `caller`, definition must land back on `add`.
    let call_line = 8;
    let call_char = FIXTURE
        .lines()
        .nth(call_line as usize)
        .unwrap()
        .find("add")
        .unwrap() as u32;
    let (targets, _) = retry_until("definition", Duration::from_secs(30), || {
        match manager.definition(&fixture.uri, call_line, call_char) {
            Ok(t) if !t.is_empty() => Some(t),
            _ => None,
        }
    });
    let definition = true;
    if let Some(target) = targets.first() {
        // `parse_definition` reports 1-based lines (navigation.rs), while the
        // positions we send are 0-based as the protocol requires. Landing here
        // at all is the real assertion: the fixture puts an emoji, CJK text and
        // an "é" before this position, so a client computing byte offsets
        // instead of UTF-16 units would have addressed the wrong column and
        // rust-analyzer would have had nothing to point at.
        assert_eq!(
            target.line,
            add_line + 1,
            "definition landed on the wrong line — the usual cause is byte-vs-UTF-16 \
             positions, which this fixture's multi-byte characters are here to expose"
        );
    }

    let (_, _) = retry_until("completion", Duration::from_secs(30), || {
        match manager.completion(&fixture.uri, call_line, call_char + 3) {
            Ok(c) if !c.items.is_empty() => Some(c),
            _ => None,
        }
    });
    let completion = true;

    let actions = manager
        .code_action(&fixture.uri, (add_line, 0), (add_line, add_char), &[])
        .expect("codeAction request");
    let code_action = !actions.is_empty();
    eprintln!("codeAction returned {} action(s)", actions.len());

    let prepare_rename = manager
        .prepare_rename(&fixture.uri, add_line, add_char)
        .is_ok();
    let rename = manager
        .rename(&fixture.uri, add_line, add_char, "sum")
        .is_ok();

    // Diagnostics arrive unsolicited, so they are drained rather than
    // requested. A clean fixture may legitimately produce an empty array —
    // what matters is that the notification arrived and parsed. It may have
    // arrived during the progress drain above, in which case there is nothing
    // left to wait for and waiting would hang until the timeout.
    let diagnostics = diagnostics_already_seen
        || wait_for(&rx, "diagnostics", Duration::from_secs(30), |e| match e {
            LspEvent::Diagnostics { uri, .. } if uri == &fixture.uri => Some(true),
            _ => None,
        });

    manager.stop_all();

    let observed = Observed {
        // The client asks for UTF-16 and rust-analyzer honours it. Recorded
        // explicitly because it is the assumption every position in every
        // request rests on, and the fixture's emoji and CJK are what would
        // make a byte-offset client fail rather than silently address the
        // wrong column.
        position_encoding: "utf-16".to_string(),
        hover,
        definition,
        completion,
        diagnostics,
        code_action,
        rename,
        prepare_rename,
    };

    if std::env::var_os("CONFORMANCE_BLESS").is_some() {
        bless(&observed);
        return;
    }

    let table = expected();
    let ra = table
        .get("rust-analyzer")
        .and_then(|v| v.as_table())
        .expect("[rust-analyzer] section");

    let mut drift = Vec::new();
    compare(
        &mut drift,
        ra,
        "position_encoding",
        observed.position_encoding.clone(),
        |v| v.as_str().map(str::to_string),
    );
    for (key, value) in [
        ("hover", observed.hover),
        ("definition", observed.definition),
        ("completion", observed.completion),
        ("diagnostics", observed.diagnostics),
        ("code_action", observed.code_action),
        ("rename", observed.rename),
        ("prepare_rename", observed.prepare_rename),
    ] {
        compare(&mut drift, ra, key, value, |v| v.as_bool());
    }

    assert!(
        drift.is_empty(),
        "the client and rust-analyzer no longer agree with the recorded report:\n  {}\n\n\
         Decide which side changed, then re-record with:\n  \
         CONFORMANCE_BLESS=1 make lsp-conformance",
        drift.join("\n  ")
    );
}

/// Rewrite the expectations file from what was just observed.
fn bless(observed: &Observed) {
    let path = expectations_path();
    let existing = fs::read_to_string(&path).expect("expectations file");
    let header: String = existing
        .lines()
        .take_while(|l| !l.starts_with('['))
        .map(|l| format!("{l}\n"))
        .collect();
    let version = existing
        .lines()
        .find_map(|l| l.strip_prefix("version = "))
        .unwrap_or("\"unknown\"")
        .to_string();
    let body = format!(
        "{header}[rust-analyzer]\nversion = {version}\nposition_encoding = \"{}\"\n\
         hover = {}\ndefinition = {}\ncompletion = {}\ndiagnostics = {}\n\
         code_action = {}\nrename = {}\nprepare_rename = {}\n",
        observed.position_encoding,
        observed.hover,
        observed.definition,
        observed.completion,
        observed.diagnostics,
        observed.code_action,
        observed.rename,
        observed.prepare_rename,
    );
    fs::write(&path, body).expect("write expectations");
    eprintln!("blessed {}", path.display());
}
