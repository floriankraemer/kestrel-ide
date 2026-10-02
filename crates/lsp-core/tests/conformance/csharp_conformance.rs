//! The LSP client, checked against a **real** `csharp-ls`.
//!
//! Sibling of `real_server_conformance.rs` (rust-analyzer) — see that file's
//! header and `docs/architecture/lsp-conformance.md` for why this suite
//! exists, why it is `#[ignore]`d, and why the expectations file is the
//! report rather than a printed log.
//!
//! csharp-ls earns a second real server (`docs/architecture/lsp-conformance.md`
//! §"Why rust-analyzer only, for now" explains the bar) because C4–C7 of the
//! C# plan built client support for a path no real server had ever exercised
//! it against: dynamic capability registration (`client/registerCapability`)
//! and pulled configuration (`workspace/configuration`). The stub answers the
//! way we assumed a server would; csharp-ls is the first check of whether a
//! real one agrees.
//!
//! This suite deliberately covers only what C4, C6 and C7 added client
//! support for — the surfaces that would silently regress without a real
//! server to catch it — not everything csharp-ls can do.
//!
//! Run it with `make lsp-conformance`, same as rust-analyzer's suite; both
//! run from the one `lsp-conformance-ci` invocation.

use crate::support::{retry_until, wait_for};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use lsp_core::catalog::ServerConfig;
use lsp_core::manager::{LspEvent, LspManager};
use serde_json::json;

/// csharp-ls resolves a solution and warms up Roslyn before it can answer
/// anything usable — slower than rust-analyzer's `cargo metadata` pass on a
/// loaded CI box, hence the generous budget.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

const LANG: &str = "csharp";

/// Pinned in `docker/Dockerfile`'s `lsp-conformance` stage — kept here only
/// for the blessed expectations file's `version` line, not compared against
/// the running server (nothing exposes its own version over LSP).
const CSHARP_LS_VERSION: &str = "0.27.0";

/// A fixture whose interesting positions sit **after** multi-byte characters
/// — the same reasoning as `real_server_conformance.rs`'s `FIXTURE`: the
/// client advertises UTF-16 positions, and only a real server can show a
/// byte-offset bug that ASCII-only text would hide.
const FIXTURE: &str = r#"// Fixture: 🙂 an emoji and 中文 in the comments, on purpose.

namespace ConformanceFixture;

public class Calculator
{
    /// <summary>Adds two numbers — the é here shifts every column after it.</summary>
    public int Add(int left, int right)
    {
        return left + right;
    }

    public int CallAdd()
    {
        return Add(2, 2);
    }
}
"#;

// TFM matches the pinned .NET SDK in docker/Dockerfile's lsp-conformance
// stage (10.0) — the same version the pinned csharp-ls's own tool payload
// targets.
const CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net10.0</TargetFramework>
    <Nullable>enable</Nullable>
  </PropertyGroup>
</Project>
"#;

fn sln(csproj_name: &str) -> String {
    format!(
        "Microsoft Visual Studio Solution File, Format Version 12.00\n\
         # Visual Studio Version 17\n\
         Project(\"{{9A19103F-16F7-4668-BE54-9A1E7A4F7556}}\") = \"ConformanceFixture\", \
         \"{csproj_name}\", \"{{2150E333-8FDC-42A3-9474-1A3956D46DE8}}\"\n\
         EndProject\n\
         Global\n\
         \tGlobalSection(SolutionConfigurationPlatforms) = preSolution\n\
         \t\tDebug|Any CPU = Debug|Any CPU\n\
         \tEndGlobalSection\n\
         EndGlobal\n"
    )
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    uri: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        fs::write(
            root.join("ConformanceFixture.sln"),
            sln("ConformanceFixture.csproj"),
        )
        .unwrap();
        fs::write(root.join("ConformanceFixture.csproj"), CSPROJ).unwrap();
        let file = root.join("Calculator.cs");
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
        name: "csharp-ls".into(),
        command: "csharp-ls".into(),
        args: Vec::new(),
        enabled: true,
        // Mirrors the built-in `csharp` plugin's contribution (C2/C3):
        // pulled configuration under the `csharp` section, analyzers on.
        settings_section: Some("csharp".into()),
        settings: json!({"analyzersEnabled": true}),
        initialization_options: serde_json::Value::Null,
        diagnostics: true,
        posix_only: false,
        exec: lsp_core::catalog::ServerExec::Host,
        source: lsp_core::catalog::ServerSource::Plugin {
            plugin_id: "csharp".into(),
        },
    }
}

/// Start csharp-ls on the fixture and wait until it accepts requests.
fn started(fixture: &Fixture) -> (LspManager, Receiver<LspEvent>, bool) {
    let (manager, rx) = LspManager::new(fixture.root_uri());
    manager.start(&config()).expect("start csharp-ls");
    manager
        .did_open(&fixture.uri, LANG, FIXTURE)
        .expect("didOpen");
    let completion_resolve_supported = wait_for(&rx, "ServerReady", READY_TIMEOUT, |e| match e {
        LspEvent::ServerReady {
            language_id,
            completion_resolve_supported,
            ..
        } if language_id == LANG => Some(*completion_resolve_supported),
        _ => None,
    });
    (manager, rx, completion_resolve_supported)
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

#[derive(Default, Debug)]
struct Observed {
    initialize: bool,
    register_capability: bool,
    completion: bool,
    completion_resolve_supported: bool,
    completion_resolve: bool,
    hover: bool,
}

/// Compare one observed bool against the file, collecting rather than
/// asserting so a run reports every drift at once instead of the first.
fn compare(drift: &mut Vec<String>, table: &toml::Table, key: &str, observed: bool) {
    match table.get(key).and_then(|v| v.as_bool()) {
        Some(want) if want == observed => {}
        Some(want) => drift.push(format!("{key}: expected {want:?}, observed {observed:?}")),
        None => drift.push(format!("{key}: missing from the expectations file")),
    }
}

#[test]
#[ignore = "needs a real csharp-ls; run via `make lsp-conformance`"]
fn csharp_ls_matches_the_recorded_expectations() {
    let fixture = Fixture::new();
    let (manager, rx, completion_resolve_supported) = started(&fixture);
    let initialize = true;

    // csharp-ls declares its capabilities through dynamic registration
    // (client/registerCapability) rather than statically in the `initialize`
    // result — this is the C4 path, unproven against a real server until
    // now. `didOpen` above is enough to make it register something for
    // `textDocument/didOpen`-adjacent sync methods; drain events briefly to
    // give the registration request time to arrive before asking.
    let drained: Vec<LspEvent> = {
        std::thread::sleep(Duration::from_millis(500));
        rx.try_iter().collect()
    };
    for event in &drained {
        if let LspEvent::ServerFailed { message, .. } = event {
            panic!("server failed while waiting for registrations: {message}");
        }
    }
    let register_capability = manager.method_registered(LANG, "textDocument/didChangeWatchedFiles")
        || manager.method_registered(LANG, "workspace/didChangeWatchedFiles")
        || manager.method_registered(LANG, "textDocument/completion")
        || manager.method_registered(LANG, "workspace/didChangeConfiguration");

    // From `CallAdd`, complete on `Add(` — proves completion lands at a
    // position after the fixture's emoji, CJK and "é" (the UTF-16 point of
    // the whole suite), and it is the C7 completionItem/resolve round trip
    // this suite exists to prove against something real.
    let call_line = FIXTURE
        .lines()
        .position(|l| l.contains("return Add"))
        .unwrap() as u32;
    let call_char = FIXTURE
        .lines()
        .nth(call_line as usize)
        .unwrap()
        .find("Add")
        .unwrap() as u32;

    let (completion_list, _) = retry_until("completion", Duration::from_secs(60), || match manager
        .completion(&fixture.uri, call_line, call_char + 3)
    {
        Ok(c) if !c.items.is_empty() => Some(c),
        _ => None,
    });
    let completion = true;

    let completion_resolve = if completion_resolve_supported {
        completion_list
            .items
            .first()
            .map(|item| manager.resolve_completion_item(LANG, &item.raw).is_ok())
            .unwrap_or(false)
    } else {
        false
    };

    let add_line = FIXTURE
        .lines()
        .position(|l| l.contains("public int Add"))
        .unwrap() as u32;
    let add_char = FIXTURE
        .lines()
        .nth(add_line as usize)
        .unwrap()
        .find("Add")
        .unwrap() as u32;
    let (_, _) = retry_until("hover", Duration::from_secs(60), || {
        manager
            .hover(&fixture.uri, add_line, add_char)
            .ok()
            .flatten()
    });
    let hover = true;

    manager.stop_all();

    let observed = Observed {
        initialize,
        register_capability,
        completion,
        completion_resolve_supported,
        completion_resolve,
        hover,
    };

    if std::env::var_os("CONFORMANCE_BLESS").is_some() {
        bless(&observed);
        return;
    }

    let table = expected();
    let cs = table
        .get("csharp-ls")
        .and_then(|v| v.as_table())
        .expect("[csharp-ls] section");

    let mut drift = Vec::new();
    for (key, value) in [
        ("initialize", observed.initialize),
        ("register_capability", observed.register_capability),
        ("completion", observed.completion),
        (
            "completion_resolve_supported",
            observed.completion_resolve_supported,
        ),
        ("completion_resolve", observed.completion_resolve),
        ("hover", observed.hover),
    ] {
        compare(&mut drift, cs, key, value);
    }

    assert!(
        drift.is_empty(),
        "the client and csharp-ls no longer agree with the recorded report:\n  {}\n\n\
         Decide which side changed, then re-record with:\n  \
         CONFORMANCE_BLESS=1 make lsp-conformance",
        drift.join("\n  ")
    );
}

/// Rewrite the expectations file's `[csharp-ls]` section from what was just
/// observed, leaving `[rust-analyzer]` and everything above the first
/// `[csharp-ls]`/`[rust-analyzer]` header untouched.
fn bless(observed: &Observed) {
    let path = expectations_path();
    let existing = fs::read_to_string(&path).expect("expectations file");
    let mut kept = String::new();
    let mut in_csharp_section = false;
    for line in existing.lines() {
        if line.starts_with("[csharp-ls]") {
            in_csharp_section = true;
            continue;
        }
        if in_csharp_section && line.starts_with('[') {
            in_csharp_section = false;
        }
        if !in_csharp_section {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    let section = format!(
        "\n[csharp-ls]\nversion = \"{CSHARP_LS_VERSION}\"\n\
         initialize = {}\nregister_capability = {}\ncompletion = {}\n\
         completion_resolve_supported = {}\ncompletion_resolve = {}\nhover = {}\n",
        observed.initialize,
        observed.register_capability,
        observed.completion,
        observed.completion_resolve_supported,
        observed.completion_resolve,
        observed.hover,
    );
    fs::write(&path, format!("{kept}{section}")).expect("write expectations");
    eprintln!("blessed {}", path.display());
}
