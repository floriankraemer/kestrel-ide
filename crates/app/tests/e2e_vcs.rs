//! End-to-end flows for version control that do not fit in `e2e.rs`, which
//! sits at its ratcheted size ceiling (`scripts/check-file-size.sh`).
//!
//! Its own test binary for that reason alone — everything else about these
//! flows is exactly as `e2e.rs` describes, and `make e2e` runs both.

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

/// A fresh temp directory holding `files`, committed to a brand-new Git
/// repository.
///
/// Duplicated from `e2e.rs` rather than shared, the same judgement
/// `e2e_run.rs` makes about its own helpers: a twenty-line fixture is not
/// worth a third crate between the test binaries. The reason it bakes
/// `.git` in rather than running `git init` after launch is `e2e.rs`'s:
/// `VcsService::open_project` discovers `.git` on a background thread the
/// instant the project opens, and an `init` racing that thread can leave the
/// app permanently believing the project is not a repository.
fn git_fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
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
    // only, since `--global` would leak between test runs on a shared
    // machine.
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "user.name", "E2E"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "initial"]);
    dir
}

/// The Changes dock used to be blind to anything the app did not do itself:
/// `refreshStatus` had exactly two callers, a save and repository
/// discovery, so a `git pull`, a rebase, a branch switch or an edit made in
/// a terminal changed the worktree and the dock went on showing what it had
/// last read. This drives the relay that fixes it — the tree's filesystem
/// watcher, coalesced, asking `VcsService` to look again — through a write
/// made from outside the process, which is the only way to prove the app
/// heard something it did not cause.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_an_external_change_reaches_the_changes_dock() {
    let name = "e2e_an_external_change_reaches_the_changes_dock";

    let repo = git_fixture(&[("draft.txt", "first draft\n")]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);

    ide.wait_for_ev(Mark::start(), "project_opened");

    // Alt+9 is `vcs.view.changes` (keymap.rs); the dock may already be the
    // visible tab, so the geometry marker is read from the start of the
    // stream rather than from a mark taken after the key.
    ide.key("alt+9");
    ide.wait_for_event(
        Mark::start(),
        "the Changes dock to report its geometry",
        |e| e["ev"] == "changes_panel_shown",
    );

    let mark = ide.mark();
    // Written straight to disk, by the test rather than through the editor:
    // no tab, no save, nothing in-process to notice it. Only the watcher can
    // report this.
    std::fs::write(ide.project_root().join("outside.txt"), "written outside\n")
        .expect("writing a file into the project from outside the app");

    ide.wait_for_event(mark, "the externally written file to reach the dock", |e| {
        e["ev"] == "changes_row" && e["path"] == "outside.txt" && e["group"] == "untracked"
    });

    assert_eq!(ide.quit(), 0);
}

/// The centre of a `[x, y, w, h]` marker field, so a flow never computes a
/// click point from window geometry or font metrics.
fn rect_centre(rect: &serde_json::Value) -> (i32, i32) {
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

/// `git status --porcelain`, read by the test with its own `git` process
/// rather than through the app — so a pass proves the whole seam (tree menu
/// → bridge → `vcs-core` → a real `git`) actually moved the index, not that
/// each layer agrees with its own mocks.
fn status_porcelain(root: &std::path::Path) -> String {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .expect("running git status")
}

/// Right-click `file`'s row in the project tree and hover the Git entry so
/// its submenu is open. Returns the mark the submenu's own
/// `project_tree_git_action` markers come after.
fn open_tree_git_submenu(ide: &Ide, file: &str) -> Mark {
    // The tree reports every visible row's rect, and reports again whenever
    // anything moves a row — including the very first layout, whose rects are
    // stale by the time the dock has its final size. So force one fresh
    // report and read *that* one: a new file in the project reaches the tree
    // through the filesystem watcher, which is a structural change.
    //
    // (The Project dock is already visible on a fresh profile. `alt+1` would
    // toggle it — i.e. hide it — which is how the first version of this flow
    // ended up right-clicking an empty dock.)
    let mark = ide.mark();
    std::fs::write(ide.project_root().join("zzz-settle.txt"), "settle\n")
        .expect("writing a file to force a fresh tree layout report");
    let row = ide.wait_for_event(mark, &format!("a settled tree row for {file}"), |e| {
        e["ev"] == "project_tree_row" && e["path"].as_str().is_some_and(|path| path.ends_with(file))
    });

    let (row_x, row_y) = rect_centre(&row["rect"]);
    // No window manager under Xvfb, so nothing has given the window the
    // input focus since it mapped; a click into an unfocused toplevel is
    // delivered, but the menu it raises has nowhere to take a grab from.
    ide.focus_main();
    ide.click_at(row_x, row_y, 1);
    ide.click_at(row_x, row_y, 3);
    ide.wait_for_event(mark, "the tree context menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "project_tree_context_menu"
    });

    // Both menus report where their entries are, so this hovers and clicks
    // them rather than counting `Down` presses — a count silently re-targets
    // itself the day someone adds an entry above the one it meant.
    let git = ide.wait_for_event(mark, "the Git entry in the tree menu", |e| {
        e["ev"] == "project_tree_menu_action" && e["label"] == "Git"
    });
    let (git_x, git_y) = rect_centre(&git["rect"]);
    // Hovering opens a submenu; clicking a submenu parent does not.
    ide.mouse_move(git_x, git_y);
    mark
}

/// The project tree's Git submenu stages the file it was opened on.
///
/// Staging is the entry worth driving end to end rather than one of the
/// read-only ones: it is the one that proves the *absolute* path the tree
/// holds survives the trip to `vcs-core`, which wants a repository-relative
/// one. `stageFile` was reachable only from the Changes dock before this,
/// which happens to hand it relative paths already.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_the_project_trees_git_submenu_stages_a_file() {
    let name = "e2e_the_project_trees_git_submenu_stages_a_file";

    let repo = git_fixture(&[("draft.txt", "first draft\n")]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);

    ide.wait_for_ev(Mark::start(), "project_opened");

    // Modified, so the submenu's Stage File is enabled. Written from the
    // test: the flow under test is the menu, not the editor.
    let file = ide.project_root().join("draft.txt");
    std::fs::write(&file, "first draft, revised\n").expect("editing draft.txt");
    ide.wait_for_event(Mark::start(), "the change to reach the app", |e| {
        e["ev"] == "changes_row" && e["path"] == "draft.txt"
    });

    let mark = open_tree_git_submenu(&ide, "draft.txt");

    let stage = ide.wait_for_event(mark, "Stage File in the Git submenu", |e| {
        e["ev"] == "project_tree_git_action" && e["label"] == "Stage File"
    });
    assert_eq!(
        stage["enabled"],
        serde_json::Value::Bool(true),
        "Stage File was disabled for a file with unstaged changes"
    );
    let (stage_x, stage_y) = rect_centre(&stage["rect"]);
    ide.click_at(stage_x, stage_y, 1);
    ide.wait_for_event(mark, "the tree context menu to close", |e| {
        e["ev"] == "dialog_closed"
            && e["name"] == "project_tree_context_menu"
            && e["accepted"] == true
    });

    // `M ` in the first column: staged, with nothing left unstaged.
    let root = ide.project_root().to_path_buf();
    e2e::wait_for("draft.txt to be staged", || {
        status_porcelain(&root)
            .starts_with("M  draft.txt")
            .then_some(())
    });

    assert_eq!(ide.quit(), 0);
}

/// Drop dock `title` from the persisted layout, which is what a layout
/// saved by a build that predates that dock looks like. ADS's `saveState`
/// is `qCompress`ed XML (a 4-byte big-endian length, then a zlib stream)
/// and its `restoreState` takes plain XML back, so the edited layout is
/// stored uncompressed.
fn forget_dock_in_saved_layout(ide: &Ide, title: &str) {
    use base64::Engine;
    use std::io::Read;
    let engine = base64::engine::general_purpose::STANDARD;
    let mut settings = app_config::load(&ide.config_dir()).expect("settings just written");
    let state = engine
        .decode(&settings.window_state)
        .expect("window_state is base64");
    let xml = if state.starts_with(b"<?xml") {
        state
    } else {
        let mut xml = Vec::new();
        flate2::read::ZlibDecoder::new(&state[4..])
            .read_to_end(&mut xml)
            .expect("window_state is qCompress'ed XML");
        xml
    };
    let xml = String::from_utf8(xml).expect("ADS state is XML text");
    let element = format!("<Widget Name=\"{title}\" Closed=\"1\"/>");
    assert!(
        xml.contains(&element),
        "the saved layout should hold the closed {title} dock: {xml}"
    );
    settings.window_state = engine.encode(xml.replace(&element, ""));
    app_config::save(&ide.config_dir(), &settings).expect("rewriting the layout");
}

/// A dock the saved layout does not know — File History here, but any
/// dock added after the user's layout was last saved — is left without a
/// dock area by `restoreState`, and every dock area that existed before
/// the restore is deleted by it. Showing such a dock used to hand ADS a
/// dangling area to place it next to (an access violation on Windows,
/// undefined on Linux), and since the crash meant no layout was ever saved
/// again, it repeated on every launch. Driven through the tree's Git
/// submenu, the path the Windows crash dump named.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_file_history_opens_after_a_layout_saved_without_its_dock() {
    let name = "e2e_file_history_opens_after_a_layout_saved_without_its_dock";
    let repo = git_fixture(&[("history.txt", "v1\n")]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);
    ide.wait_for_ev(Mark::start(), "project_opened");
    assert_eq!(ide.quit(), 0);
    forget_dock_in_saved_layout(&ide, "File History");

    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");

    let mark = open_tree_git_submenu(&ide, "history.txt");
    let history = ide.wait_for_event(mark, "Show File History in the Git submenu", |e| {
        e["ev"] == "project_tree_git_action" && e["label"] == "Show File History"
    });
    let (x, y) = rect_centre(&history["rect"]);
    ide.click_at(x, y, 1);

    let ready = ide.wait_for_event(mark, "history_ready for history.txt", |e| {
        e["ev"] == "history_ready"
            && e["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("history.txt"))
    });
    assert_eq!(
        ready["count"].as_u64(),
        Some(1),
        "history did not list the commit"
    );

    assert_eq!(ide.quit(), 0);
}
