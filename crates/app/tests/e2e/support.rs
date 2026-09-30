//! Helpers shared across this binary's E2E modules. The harness itself —
//! `Ide`, `Mark`, `Mcp`, `wait_for` — lives in `crates/e2e`; everything here
//! is app-specific: fixture paths, marker-driven UI actions repeated across
//! more than one flow, and the small JSON-marker plumbing every flow needs.

use std::path::{Path, PathBuf};

use e2e::{mcp::Mcp, Ide, Mark};
use serde_json::{json, Value};

pub(crate) const APP: &str = env!("CARGO_BIN_EXE_app");

pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The fixture file's content as it is checked in — the independent answer
/// to "did the app change this file", read without going through the app.
pub(crate) fn fixture_text(name: &str, relative: &str) -> String {
    std::fs::read_to_string(fixture(name).join(relative)).expect("fixture file")
}

/// Wait until the project index reports itself complete.
///
/// The step a naive harness would spell `sleep`. Everything that searches —
/// Go to File, Search Everywhere, the name-based rename — answers "still
/// being built" until this is true, and a fixed delay only makes that
/// answer intermittent.
pub(crate) fn wait_for_index(mcp: &Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", json!({}))["ready"] == true).then_some(())
    });
}

/// Open the Search Everywhere popup with `shortcut` and wait until its
/// opening query has been answered, so a later `search_results` marker can
/// only be one the test's own typing caused.
pub(crate) fn open_search_popup(ide: &Ide, shortcut: &str) -> Mark {
    let main_window = ide.window().to_string();
    let mark = ide.mark();
    ide.key(shortcut);
    ide.wait_for_event(mark, "the search popup to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "search_everywhere"
    });
    ide.wait_for_focus_change(&main_window);
    ide.wait_for_ev(mark, "search_results");
    ide.mark()
}

/// Wait for the refactor preview to be up and give it the input focus, so a
/// following `Return`/`Escape` reaches the dialog. Returns its
/// `preview_rows` marker. Keying on `preview_rows` alone raced the dialog
/// being shown and focused, and the key was dropped (#346).
pub(crate) fn focus_refactor_preview(ide: &Ide, mark: Mark) -> Value {
    let rows = ide.wait_for_ev(mark, "preview_rows");
    let shown = ide.wait_for_event(mark, "the refactor preview to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "refactor_preview"
    });
    ide.focus_window(
        shown["window"]
            .as_str()
            .expect("dialog_shown carries its window"),
    );
    rows
}

/// Type a query into an open search popup and take the top hit.
pub(crate) fn accept_top_hit(ide: &Ide, mark: Mark, query: &str) {
    ide.type_text(query);
    let hits = ide.wait_for_event(mark, &format!("results for `{query}`"), |e| {
        e["ev"] == "search_results" && e["count"].as_u64().unwrap_or(0) > 0
    });
    assert!(hits["count"].as_u64().unwrap() > 0);
    ide.key("Return");
    ide.wait_for_event(mark, "the search popup to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "search_everywhere" && e["accepted"] == true
    });
    ide.focus_main();
}

/// Open one file through Go to File, returning its `tab_added` marker.
pub(crate) fn open_file(ide: &Ide, name: &str) -> Value {
    let mark = open_search_popup(ide, "ctrl+shift+n");
    accept_top_hit(ide, mark, name);
    ide.wait_for_event(mark, &format!("a tab for `{name}`"), |e| {
        e["ev"] == "tab_added" && e["title"] == name
    })
}

pub(crate) fn buffer(mcp: &Mcp, tab_id: u64) -> String {
    mcp.call("read_buffer", json!({ "tab_id": tab_id }))["content"]
        .as_str()
        .expect("read_buffer returns a string")
        .to_string()
}

pub(crate) fn cursor(mcp: &Mcp, tab_id: u64) -> (u32, u32) {
    let position = mcp.call("get_cursor_position", json!({ "tab_id": tab_id }));
    (
        position["line"].as_u64().unwrap_or(0) as u32,
        position["column"].as_u64().unwrap_or(0) as u32,
    )
}

/// The centre of a `[x, y, w, h]` rect a marker reported — the only way to
/// click a widget this harness has no handle on.
pub(crate) fn rect_centre(rect: &Value) -> (i32, i32) {
    let rect: Vec<i64> = rect
        .as_array()
        .expect("the marker carries a rect")
        .iter()
        .map(|v| v.as_i64().expect("an integer"))
        .collect();
    (
        (rect[0] + rect[2] / 2) as i32,
        (rect[1] + rect[3] / 2) as i32,
    )
}

/// A fresh temp directory holding `files`, committed to a brand-new Git
/// repository.
///
/// `VcsService::open_project` discovers `.git` on a background thread the
/// instant `ProjectTreeModel::projectOpened` fires during startup (see
/// `wireVcsService`, `crates/ui-shell/cpp/editor_tabs_vcs.cpp`) — before a
/// test gets to run a single line, and with nothing that ever re-checks once
/// that first answer is in. A `git init` run against `Ide::project_root()`
/// after `Ide::launch` returns would race that thread and, on the losing
/// side, leave the app permanently believing the project is not a
/// repository. Baking `.git` into the directory `Ide::launch` copies from
/// sidesteps the race entirely: `copy_tree` faithfully copies dotdirs, so by
/// the time the app process is even spawned, `.git` has already been in the
/// (temporary, throwaway) project root for as long as every other file has.
pub(crate) fn git_fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp git fixture dir");
    for (relative, content) in files {
        let path = dir.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture subdirectory");
        }
        std::fs::write(&path, content).expect("fixture file");
    }
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "--quiet"]);
    // A fresh `git` has no identity configured in CI; scoped to this repo
    // only — `--global` would leak between test runs on a shared machine.
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "user.name", "E2E"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "initial"]);
    dir
}

/// Where `stub_server` lands: Cargo places every workspace binary in the
/// same `target/<profile>/` directory as `app`'s own, and
/// `CARGO_BIN_EXE_stub_server` is not an option here — Cargo only sets a
/// binary's `CARGO_BIN_EXE_*` for integration tests of the crate that
/// declares it (`lsp-core`'s own, not `app`'s).
pub(crate) fn stub_server_path() -> PathBuf {
    Path::new(APP).with_file_name("stub_server")
}

/// Route the `rust` language id at `lsp-core`'s stub server rather than a
/// real `rust-analyzer` — not installed in this image, and the point of
/// these flows is the client's own behaviour, which the stub is built to
/// exercise deterministically by request line (`stub_server.rs`'s own doc
/// comment).
///
/// Requires a restart: `LanguageService` resolves the server table once, on
/// `openProject`, from whatever `app-config` already has on disk — so the
/// override has to be written before the project opens, not after.
pub(crate) fn route_rust_at_stub(ide: &mut Ide) {
    route_rust_at_stub_with(ide, |_| {});
}

/// [`route_rust_at_stub`], also letting the flow seed other settings in the
/// same relaunch.
pub(crate) fn route_rust_at_stub_with(
    ide: &mut Ide,
    tweak: impl FnOnce(&mut app_config::Settings),
) {
    assert_eq!(ide.quit(), 0);
    let mut settings = app_config::load(&ide.config_dir()).expect("settings just written");
    tweak(&mut settings);
    settings
        .language_servers
        .push(app_config::LanguageServerSetting {
            language_id: "rust".to_string(),
            command: Some(stub_server_path().to_string_lossy().into_owned()),
            ..Default::default()
        });
    app_config::save(&ide.config_dir(), &settings).expect("seeding the stub server override");
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
}

/// Click the project tree's `.ide` row, so the window has a settled,
/// on-screen row to act against — the tree's own rows are the one thing
/// with a reported rect at this point, and clicking `.ide` only selects it
/// (a file row would open a tab). The *latest* report of the row: the tree
/// publishes its rows once before the main window is laid out (tiny, wrong
/// rects) and again after, and `main_window_shown` has already been waited
/// for here.
pub(crate) fn settle(ide: &Ide) {
    let row = ide
        .events()
        .into_iter()
        .rev()
        .find(|e| {
            e["ev"] == "project_tree_row"
                && e["path"].as_str().is_some_and(|p| p.ends_with("/.ide"))
        })
        .expect("the project tree reported its .ide row");
    let (x, y) = rect_centre(&row["rect"]);
    ide.click_at(x, y, 1);
    ide.focus_main();
}

/// The first row of `kind` in a tree-changed marker's `rows` array, and (for
/// the containers tree, which can hold more than one row of the same kind)
/// narrowed further to `id` when one is given.
pub(crate) fn find_row<'a>(rows: &'a [Value], kind: &str, id: Option<&str>) -> Option<&'a Value> {
    rows.iter()
        .find(|row| row["kind"] == kind && id.is_none_or(|wanted| row["id"] == wanted))
}

/// Wait for a `containers_tree_changed` marker whose rows include a `kind`
/// (optionally `id`-scoped) row, and return that row.
pub(crate) fn wait_for_containers_row(
    ide: &Ide,
    mark: Mark,
    kind: &str,
    id: Option<&str>,
) -> Value {
    let kind = kind.to_string();
    let id = id.map(str::to_string);
    let event = ide.wait_for_event(
        mark,
        &format!("a `{kind}` row in the containers tree"),
        |e| {
            e["ev"] == "containers_tree_changed"
                && e["rows"]
                    .as_array()
                    .is_some_and(|rows| find_row(rows, &kind, id.as_deref()).is_some())
        },
    );
    find_row(
        event["rows"].as_array().expect("rows array"),
        &kind,
        id.as_deref(),
    )
    .expect("just matched above")
    .clone()
}

/// Wait for a `database_tree_changed` marker whose rows include a `kind`
/// row, and return that row.
pub(crate) fn wait_for_database_row(ide: &Ide, mark: Mark, kind: &str) -> Value {
    let kind = kind.to_string();
    let event = ide.wait_for_event(mark, &format!("a `{kind}` row in the database tree"), |e| {
        e["ev"] == "database_tree_changed"
            && e["rows"]
                .as_array()
                .is_some_and(|rows| find_row(rows, &kind, None).is_some())
    });
    find_row(event["rows"].as_array().expect("rows array"), &kind, None)
        .expect("just matched above")
        .clone()
}

/// A screenshot of the whole screen to `<dir>/<name>.png`, where `<dir>` is
/// `$IDE_SHOT_DIR` or, failing that, an existing `target/shots`: a diagnostic
/// for the PR, never an assertion (ADR-0024).
pub(crate) fn shot(name: &str) {
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
    let dir = std::env::var_os("IDE_SHOT_DIR")
        .map(PathBuf::from)
        .or_else(|| default.is_dir().then_some(default));
    if let Some(dir) = dir {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::process::Command::new("import")
            .args(["-window", "root"])
            .arg(dir.join(format!("{name}.png")))
            .status();
    }
}
