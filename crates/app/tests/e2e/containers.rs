//! C10: end-to-end flows for the containers plan (ADR-0055/ADR-0056) —
//! the dock connecting to a canned engine and showing its tree, a
//! lifecycle action reaching the engine and the Log tab opening on
//! selection, a compose run configuration's command preview, the
//! Dockerfile/compose gutter and compose lenses, Settings' Containers
//! page Test connection, and (database-tools-plan G1.6) disabling the
//! `containers` plugin from the same Settings dialog and relaunching to
//! confirm its View-menu entry is gone.
//!
//! Every flow drives a fake `docker`/`podman` CLI
//! (`container_core::bin::stub_engine`) rather than a real engine: none of
//! this suite's hosts can assume Docker or Podman is installed. `env!(
//! "CARGO_BIN_EXE_stub_engine")` cannot be used here — Cargo only sets a
//! binary's `CARGO_BIN_EXE_*` for integration tests of the crate that
//! declares it (the same reason `analysis.rs` locates `stub_analyzer`
//! by hand) — so [`stub_engine_bin`] derives its path from `CARGO_BIN_EXE_
//! app`'s own directory instead; `e2e-ci` (`Makefile`) builds it
//! explicitly (`cargo build --bin stub_engine -p container-core`) right
//! beside its `stub_server`/`stub_analyzer` lines, so its executable lands
//! in the same target directory `app`'s own binary does.
//!
//! [`stub_path_dir`] copies that one binary to two names, `docker` and
//! `podman`, in a scratch directory prepended to the launched process'
//! `PATH` (`Ide::launch_with_env` — a C10 addition to the shared harness,
//! since no existing flow needed to hand the spawned app extra
//! environment). `IDE_STUB_ENGINE_DATA` is left unset: the stub's own
//! default is `container-core`'s `testdata/` directory, so this suite
//! reuses the exact fixture JSON the crate's unit tests already exercise
//! rather than a parallel copy — `crates/container-core/testdata/README`-
//! adjacent files under `inspect/{docker,podman}` back `ps`/`inspect`,
//! `probe/*_version.json` backs `version`, and the small ones this task
//! added (`history/`, `logs/`, `compose/`) back the rest.
//!
//! Two harness traps this suite deliberately avoids (see the project's own
//! headless-E2E notes): waiting for two identical screenshots as a
//! "settled" signal (never done here — every wait is for a marker), and
//! assuming a freshly-shown widget's rect is valid the instant it appears
//! (`refreshE2eRects`'s `QTimer::singleShot(0, ...)` dance in `containers_
//! panel.cpp`, mirrored here by simply waiting for a `containers_tree_
//! changed`/`containers_toolbar_rects` marker whose payload already has
//! what is needed, rather than assuming the first one does).

use std::path::{Path, PathBuf};

use e2e::{Ide, Mark};
use serde_json::Value;

use crate::support::{
    fixture, open_file, rect_centre, settle, wait_for_containers_row as wait_for_row, APP,
};

/// `stub_engine`'s executable — see this file's own doc comment for why
/// `env!("CARGO_BIN_EXE_stub_engine")` cannot be used directly.
fn stub_engine_bin() -> PathBuf {
    let name = if cfg!(windows) {
        "stub_engine.exe"
    } else {
        "stub_engine"
    };
    let path = Path::new(APP).with_file_name(name);
    assert!(
        path.is_file(),
        "{} does not exist — run via `make e2e`, which builds it \
         (`cargo build --bin stub_engine -p container-core`) before this \
         test; `cargo test -p app` alone never builds a binary that only \
         `container-core` declares",
        path.display()
    );
    path
}

/// A scratch `PATH` entry with `stub_engine` copied to `docker` and
/// `podman` — the fixture's `.ide/settings.toml` names both as each
/// connection's `executable`.
fn stub_path_dir() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("stub engine PATH dir");
    for name in ["docker", "podman"] {
        std::fs::copy(stub_engine_bin(), dir.path().join(name)).expect("seeding the stub engine");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.path().join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .expect("making the stub engine executable");
        }
    }
    dir
}

/// One invocation line from `IDE_STUB_ENGINE_LOG`: `{"argv":[...]}` per
/// call `container-core` made through the stub — see `stub_engine.rs`'s
/// own doc comment.
fn stub_log(path: &Path) -> Vec<Vec<String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|entry| {
            entry["argv"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .collect()
}

/// Whether any logged invocation's argv starts with `prefix` (after its
/// own program name, `argv[0]`, which this suite never asserts about —
/// only what `container-core` passed it).
fn stub_log_has(log: &[Vec<String>], prefix: &[&str]) -> bool {
    log.iter().any(|argv| {
        argv.len() > prefix.len()
            && argv[1..1 + prefix.len()]
                .iter()
                .zip(prefix)
                .all(|(a, b)| a == b)
    })
}

/// Launch against the fixture project with a fake `docker`/`podman` on
/// `PATH`. Returns the `Ide` plus the directory that must outlive it (the
/// `PATH` entry — dropping it before `ide` would delete the very binaries
/// the running app still has open) and the invocation log's own path,
/// written inside that same directory rather than a second `TempDir` that
/// would have to be kept alive too.
fn launch_with_stub_engine(name: &str) -> (Ide, tempfile::TempDir, PathBuf) {
    let path_dir = stub_path_dir();
    let log_path = path_dir.path().join("invocations.jsonl");
    let path_value = format!(
        "{}:{}",
        path_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let ide = Ide::launch_with_env(
        name,
        APP,
        fixture("containers"),
        &[
            (
                "IDE_STUB_ENGINE_LOG",
                log_path.to_str().expect("utf-8 path"),
            ),
            ("PATH", path_value.as_str()),
        ],
    );
    (ide, path_dir, log_path)
}

/// Open the Containers dock through the View menu (there is no default
/// shortcut for `view.containers`) and wait until its tree is actually on
/// screen and laid out — a `containers_tree_changed` marker with at least
/// one row rect, never the first one (built while the dock is still
/// hidden, so every rect in it is empty).
fn open_containers_dock(ide: &Ide, mark: Mark) {
    ide.key("alt+v"); // "&View"
    let item = ide.wait_for_event(mark, "the Containers item in the View menu", |e| {
        e["ev"] == "view_menu_action" && e["label"] == "Containers"
    });
    let (x, y) = rect_centre(&item["rect"]);
    ide.click_at(x, y, 1);
    ide.wait_for_event(mark, "the containers tree to be laid out", |e| {
        e["ev"] == "containers_tree_changed"
            && e["rows"].as_array().is_some_and(|rows| !rows.is_empty())
    });
}

/// `crates/container-core/testdata/inspect/docker/containers.json`'s own
/// `redis` container — the fixture's only `running` one, so the sole
/// container row `canStart` is false for. Picking any *other* container
/// row is what makes the Start-toolbar-button click below land on an
/// enabled button instead of a silent no-op.
const RUNNING_CONTAINER_ID_PREFIX: &str = "a8ce980ef0eb";

fn find_stoppable_container_row(rows: &[Value]) -> Option<&Value> {
    rows.iter().find(|row| {
        row["kind"] == "container"
            && !row["id"]
                .as_str()
                .is_some_and(|id| id.contains(RUNNING_CONTAINER_ID_PREFIX))
    })
}

/// Wait for a `containers_toolbar_rects` marker naming `button`, and
/// return that entry.
fn wait_for_toolbar_button(ide: &Ide, mark: Mark, button: &str) -> Value {
    let button = button.to_string();
    let event = ide.wait_for_event(mark, &format!("the `{button}` toolbar button"), |e| {
        e["ev"] == "containers_toolbar_rects"
            && e["buttons"]
                .as_array()
                .is_some_and(|buttons| buttons.iter().any(|b| b["name"] == button))
    });
    event["buttons"]
        .as_array()
        .expect("buttons array")
        .iter()
        .find(|b| b["name"] == button)
        .expect("just matched above")
        .clone()
}

/// Connect the fixture's Docker connection and expand its Containers
/// group, returning the first `container`-kind row — the shared setup
/// behind the Start-action and Log-tab flows below.
fn connect_docker_and_expand_containers(ide: &Ide, mark: Mark) -> Value {
    let connection = wait_for_row(ide, mark, "connection", Some("docker-local"));
    let (x, y) = rect_centre(&connection["rect"]);
    ide.double_click_at(x, y, 1); // kind == "connection": connects on double-click.
    ide.wait_for_event(mark, "the Docker connection to report connected", |e| {
        e["ev"] == "containers_connection_state"
            && e["id"] == "docker-local"
            && e["state"] == "connected"
    });

    let group = wait_for_row(ide, mark, "containers-group", None);
    let (gx, gy) = rect_centre(&group["rect"]);
    ide.double_click_at(gx, gy, 1); // expands (Qt's default double-click-to-expand).

    // The Start button is only enabled for a *stoppable* container row —
    // the fixture's `redis` is already running, so this skips it.
    let event = ide.wait_for_event(mark, "a stoppable container row", |e| {
        e["ev"] == "containers_tree_changed"
            && e["rows"]
                .as_array()
                .is_some_and(|rows| find_stoppable_container_row(rows).is_some())
    });
    find_stoppable_container_row(event["rows"].as_array().expect("rows array"))
        .expect("just matched above")
        .clone()
}

// --- (a): dock connects and shows the canned tree ------------------------

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_containers_dock_connects_and_shows_canned_tree() {
    let name = "e2e_containers_dock_connects_and_shows_canned_tree";
    let (mut ide, _path_dir, log_path) = launch_with_stub_engine(name);
    ide.wait_for_ev(Mark::start(), "project_opened");
    settle(&ide);

    let mark = ide.mark();
    open_containers_dock(&ide, mark);

    let before = wait_for_row(&ide, mark, "connection", Some("docker-local"));
    let (x, y) = rect_centre(&before["rect"]);
    ide.double_click_at(x, y, 1);

    ide.wait_for_event(mark, "the Docker connection to report connected", |e| {
        e["ev"] == "containers_connection_state"
            && e["id"] == "docker-local"
            && e["state"] == "connected"
    });
    // The canned fixture has containers, so a real connect grows the tree
    // past the two bare connection rows it started with.
    ide.wait_for_event(mark, "the tree to grow past the two connection rows", |e| {
        e["ev"] == "containers_tree_changed" && e["nodes"].as_u64().unwrap_or(0) > 2
    });

    let log = stub_log(&log_path);
    assert!(
        stub_log_has(&log, &["version"]),
        "expected a `version` probe; log: {log:?}"
    );
    assert!(
        stub_log_has(&log, &["ps", "-aq"]),
        "expected a `ps -aq` listing; log: {log:?}"
    );
    assert!(
        stub_log_has(&log, &["container", "inspect"]),
        "expected a `container inspect` call; log: {log:?}"
    );

    assert_eq!(ide.quit(), 0);
}

// --- (b) + (c): Start reaches the engine; selecting a container opens Log ---

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_containers_start_action_and_log_tab() {
    let name = "e2e_containers_start_action_and_log_tab";
    let (mut ide, _path_dir, log_path) = launch_with_stub_engine(name);
    ide.wait_for_ev(Mark::start(), "project_opened");
    settle(&ide);

    let mark = ide.mark();
    open_containers_dock(&ide, mark);
    let container = connect_docker_and_expand_containers(&ide, mark);

    // Selecting the container (c): `ContainerDetailArea::onSelectionChanged`
    // opens the Log tab for any `kind == "container"` row automatically.
    let (cx, cy) = rect_centre(&container["rect"]);
    ide.click_at(cx, cy, 1);
    ide.wait_for_event(
        mark,
        "the Log tab to open for the selected container",
        |e| {
            e["ev"] == "containers_tab_opened"
                && e["kind"] == "log"
                && e["nodeId"] == container["id"]
        },
    );

    // Start (b): the toolbar button, once its rect is known.
    let start_button = wait_for_toolbar_button(&ide, mark, "start");
    let (sx, sy) = rect_centre(&start_button["rect"]);
    ide.click_at(sx, sy, 1);
    ide.wait_for_event(mark, "the start action to finish", |e| {
        e["ev"] == "containers_action" && e["action"] == "start" && e["ok"] == true
    });

    let log = stub_log(&log_path);
    assert!(
        stub_log_has(&log, &["logs", "-f"]),
        "expected a `logs -f` invocation (the Log tab); log: {log:?}"
    );
    assert!(
        stub_log_has(&log, &["start"]),
        "expected a `start` invocation; log: {log:?}"
    );

    assert_eq!(ide.quit(), 0);
}

// --- (d): a compose run configuration's command preview ------------------

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_containers_compose_run_config_preview() {
    let name = "e2e_containers_compose_run_config_preview";
    // A real `docker`/`podman` on PATH (the stub): the container page's
    // Services picker fires `requestComposeServices` as soon as the
    // pre-filled compose-file list loads, and a missing engine binary
    // would fail that call — harmless to the preview text this test
    // asserts on, but the stub keeps the flow clean end to end.
    let (mut ide, _path_dir, _log_path) = launch_with_stub_engine(name);
    ide.wait_for_ev(Mark::start(), "project_opened");
    settle(&ide);

    open_file(&ide, "compose.yml");

    let mark = ide.mark();
    ide.key("ctrl+shift+F10"); // run.runContext — routes through the same
                               // gutter popup a click on line 0 would.
    let menu = ide.wait_for_event(mark, "the compose gutter popup to open", |e| {
        e["ev"] == "run_gutter_menu_action"
            && e["label"]
                .as_str()
                .is_some_and(|l| l.contains("New Configuration"))
    });
    let (x, y) = rect_centre(&menu["rect"]);
    ide.click_at(x, y, 1);

    ide.wait_for_event(mark, "the Run Configurations dialog to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "run_config_dialog"
    });
    let preview = ide.wait_for_event(mark, "the compose command preview", |e| {
        e["ev"] == "run_config_preview" && e["kind"] == "compose"
    });
    let text = preview["preview"].as_str().unwrap_or("");
    assert!(text.contains("compose -f"), "preview was: {text:?}");
    assert!(text.contains("up"), "preview was: {text:?}");

    ide.key("Escape");
    ide.wait_for_event(mark, "the dialog to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "run_config_dialog"
    });
    ide.focus_main();

    assert_eq!(ide.quit(), 0);
}

// --- (e): Dockerfile/compose gutter and compose lenses -------------------

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_containers_gutter_and_compose_lenses() {
    let name = "e2e_containers_gutter_and_compose_lenses";
    let mut ide = Ide::launch(name, APP, fixture("containers"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    settle(&ide);

    let mark = ide.mark();
    open_file(&ide, "Dockerfile");
    let dockerfile_gutter = ide.wait_for_event(mark, "the Dockerfile's gutter run line", |e| {
        e["ev"] == "gutter_run_lines"
            && e["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("Dockerfile"))
    });
    let dockerfile_lines: Vec<i64> = dockerfile_gutter["lines"]
        .as_array()
        .expect("lines array")
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert!(
        dockerfile_lines.contains(&0),
        "expected the FROM line (0) to be runnable: {dockerfile_lines:?}"
    );

    let mark = ide.mark();
    open_file(&ide, "compose.yml");
    let compose_gutter = ide.wait_for_event(mark, "the compose file's gutter run lines", |e| {
        e["ev"] == "gutter_run_lines"
            && e["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("compose.yml"))
    });
    // `run_core::compose_run_lines` marks the `services:` line itself (runs
    // the whole project) plus one per service — 1 + 2 for this fixture.
    assert_eq!(
        compose_gutter["lines"]
            .as_array()
            .expect("lines array")
            .len(),
        3,
        "expected the `services:` line plus one per service (web, db): {compose_gutter}"
    );

    // Every declared service gets at least a status lens regardless of
    // whether any engine is connected (`lenses_for_service` never needs an
    // owning project) — so this needs no stub engine or connect step, only
    // the file open with its two services.
    let lenses = ide.wait_for_event(mark, "the compose file's lenses", |e| {
        e["ev"] == "compose_lenses"
            && e["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("compose.yml"))
    });
    assert_eq!(lenses["count"].as_u64(), Some(2));

    assert_eq!(ide.quit(), 0);
}

// --- (f): Settings > Containers > Test connection, plus G1.6's own
// disable-and-relaunch-removes-the-View-menu-entry proof, folded into the
// same flow rather than a fifteenth one — both start from the same
// Preferences dialog, and per-PR budget stays 14 flows either way.

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_containers_settings_test_connection() {
    let name = "e2e_containers_settings_test_connection";
    let (mut ide, _path_dir, _log_path) = launch_with_stub_engine(name);
    ide.wait_for_ev(Mark::start(), "project_opened");
    settle(&ide);

    // The Settings page edits the *global* layer by default and enables its
    // form (Test connection included) only for a selected row — the
    // fixture's connections live in the project layer, so seed one into
    // the global file the app is reading, through `app-config`'s own
    // read-modify-write rather than a hand-written TOML.
    app_config::update(&ide.config_dir(), |settings| {
        settings
            .containers
            .connections
            .push(app_config::ContainerConnectionSetting {
                id: "docker-global".to_string(),
                name: "Docker".to_string(),
                engine: "docker".to_string(),
                kind: "auto".to_string(),
                executable: "docker".to_string(),
                ..Default::default()
            });
    })
    .expect("seeding a global connection");

    let mark = ide.mark();
    ide.key("alt+f"); // "&File"
    let preferences = ide.wait_for_event(mark, "Preferences... in the File menu", |e| {
        e["ev"] == "file_menu_action" && e["label"] == "Preferences..."
    });
    let (px, py) = rect_centre(&preferences["rect"]);
    ide.click_at(px, py, 1);
    let dialog = ide.wait_for_event(mark, "the settings dialog to show", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "settings_dialog"
    });

    let (cx, cy) = rect_centre(&dialog["containers_category_rect"]);
    ide.click_at(cx, cy, 1);
    let shown = ide.wait_for_event(mark, "the Containers page's Test connection button", |e| {
        e["ev"] == "containers_settings_page_shown" && e["page"] == "Connections"
    });
    let (tx, ty) = rect_centre(&shown["test_button_rect"]);
    ide.click_at(tx, ty, 1);

    ide.wait_for_event(mark, "the test connection result", |e| {
        e["ev"] == "containers_test_connection_result" && e["ok"] == true
    });

    // G1.6: disabling the `containers` plugin from this same dialog and
    // relaunching removes its View-menu entry — the tool-windows
    // contribution point (G1.1/G1.3) only reads `disabled_plugins` at
    // startup, so a live toggle is not expected to change the menu until
    // the next launch, and this is that flow's own click-driven proof.
    let (plx, ply) = rect_centre(&dialog["plugins_category_rect"]);
    ide.click_at(plx, ply, 1);
    let rows = ide.wait_for_event(mark, "the Plugins page's own rows", |e| {
        e["ev"] == "plugins_page_rows"
            && e["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["id"] == "containers"))
    });
    let containers_row = rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "containers")
        .expect("the containers row, just asserted present");
    let (rx, ry) = rect_centre(&containers_row["rect"]);
    ide.click_at(rx, ry, 1);
    let toggle = ide.wait_for_event(mark, "the toggle for the selected containers row", |e| {
        e["ev"] == "plugins_page_toggle" && e["id"] == "containers"
    });
    assert_eq!(
        toggle["disable"], true,
        "containers starts enabled, so its own toggle offers to disable it"
    );
    let (tx, ty) = rect_centre(&toggle["rect"]);
    ide.click_at(tx, ty, 1);
    let toggled = ide.wait_for_event(
        mark,
        "the toggle to flip once containers is disabled",
        |e| e["ev"] == "plugins_page_toggle" && e["id"] == "containers" && e["disable"] == false,
    );
    let _ = toggled;

    ide.key("Escape");
    ide.wait_for_event(mark, "the settings dialog to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "settings_dialog"
    });
    ide.focus_main();

    assert_eq!(ide.quit(), 0);

    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
    let mark = ide.mark();
    ide.key("alt+v"); // "&View"
                      // Something else in the View menu is always there (Structure has no
                      // plugin/contribution gate at all) — proof the menu actually opened
                      // and reported its actions, so an *absent* Containers entry below
                      // means the entry is really gone, not that the mark never fired.
    ide.wait_for_event(mark, "the View menu to report its actions", |e| {
        e["ev"] == "view_menu_action" && e["label"] == "Structure"
    });
    let view_actions = ide.events_since_of(mark, "view_menu_action");
    assert!(
        view_actions.iter().all(|e| e["label"] != "Containers"),
        "disabling containers before the relaunch above must drop its View-menu entry, got: {view_actions:?}"
    );
    // The menu is still open (`view_menu_action`'s own `aboutToShow` never
    // closed it) — `ctrl+q` below is a global shortcut, but an open popup
    // menu still eats the keystroke before it reaches the window.
    ide.key("Escape");

    assert_eq!(ide.quit(), 0);
}
