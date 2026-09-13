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

/// Open the Search Everywhere popup with `shortcut` and wait until its
/// opening query has been answered, so a later `search_results` marker can
/// only be one the test's own typing caused.
///
/// Duplicated from `e2e.rs` rather than shared, the same judgement
/// `git_fixture` above already makes about this file's own helpers.
fn open_search_popup(ide: &Ide, shortcut: &str) -> Mark {
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

/// Type a query into an open search popup and take the top hit.
fn accept_top_hit(ide: &Ide, mark: Mark, query: &str) {
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

/// The repo-wide Commit Log: open it (Find Action, since `view.vcsCommitLog`
/// has no default shortcut — same reach `e2e_file_history_lists_commits_
/// and_survives_the_context_menu` in `e2e.rs` uses for File History), expand
/// a row to see its full message, and double-click it open in the
/// commit-detail dock — asserting the dock shows the right number of
/// changed files.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_commit_log_expand_and_open_commit_detail() {
    let name = "e2e_commit_log_expand_and_open_commit_detail";

    // Two commits touching two different files, so the detail dock's
    // changed-file count (1) can't be confused with the log's own commit
    // count (2).
    let repo = git_fixture(&[("a.txt", "one\n")]);
    std::fs::write(repo.path().join("b.txt"), "two\n").expect("write b.txt");
    for args in [
        ["add", "b.txt"].as_slice(),
        &["commit", "--quiet", "-m", "add b"],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed");
    }

    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);
    ide.wait_for_ev(Mark::start(), "project_opened");

    let popup_mark = open_search_popup(&ide, "ctrl+shift+a");
    accept_top_hit(&ide, popup_mark, "Commit Log");
    // Fresh mark: the dock's own construction (while still hidden, tabbed
    // behind Terminal/Run/etc.) already emitted `commit_log_row` once at
    // geometry nobody could click — `showEvent` re-asks once actually
    // raised, and this mark is what isolates that second, real answer.
    let mark = ide.mark();

    let row0 = ide.wait_for_event(mark, "commit_log_row 0", |e| {
        e["ev"] == "commit_log_row" && e["row"] == 0
    });
    let (row_x, row_y) = rect_centre(&row0["rect"]);
    let rect: Vec<i64> = row0["rect"]
        .as_array()
        .expect("the marker carries a rect")
        .iter()
        .map(|v| v.as_i64().expect("an integer"))
        .collect();
    let arrow_x = rect[0] as i32 + 8;

    // Expand the newest commit's row (its own arrow, at the row's left
    // edge) to see the full message body.
    ide.click_at(arrow_x, row_y, 1);
    // Double-click the row's text to open it in the commit-detail dock.
    ide.double_click_at(row_x, row_y, 1);

    let detail = ide.wait_for_event(mark, "commit_detail_ready", |e| {
        e["ev"] == "commit_detail_ready"
    });
    assert_eq!(
        detail["files"].as_u64(),
        Some(1),
        "the detail dock should show exactly the one file 'add b' touched"
    );

    assert_eq!(ide.quit(), 0);
}

/// A point on a `changes_row` marker's checkbox glyph, `e2e.rs`'s own
/// `checkbox_point` — duplicated for the reason `git_fixture` above already
/// gives for this file's other helpers.
fn checkbox_point(rect: &serde_json::Value) -> (i32, i32) {
    let rect: Vec<i64> = rect
        .as_array()
        .expect("the marker carries a rect")
        .iter()
        .map(|v| v.as_i64().expect("an integer"))
        .collect();
    (rect[0] as i32 + 10, (rect[1] + rect[3] / 2) as i32)
}

/// Click a Changes-dock row's checkbox to stage it, retrying against a
/// freshly re-read row if the click does not land.
///
/// Root cause of `e2e_stage_and_commit_through_the_changes_dock`'s ~1-in-6
/// flakiness (alongside the stale-geometry issue `ChangesPanel::markShown`
/// fixes): `refreshStatus` — and with it, `ChangesPanel::refresh`'s full
/// tree rebuild — is watcher-driven, so something with nothing to do with
/// this click (the search index writing another segment file into
/// `.ide-index/`, still settling well after `wait_for_index` first reports
/// ready) can rebuild the row this is about to click into a brand-new
/// `QTreeWidgetItem` at any moment. An `xdotool`-level click's delivery
/// through the X server has no ordering guarantee against that rebuild the
/// way a click through Qt's own test framework would, so occasionally one
/// lands in the gap and never reaches a live item — confirmed by an actual
/// repro: `changes_row` kept reporting the same unstaged row, unchanged,
/// for the full 60s timeout with no `changes_row` "staged" and no
/// `vcs_failed` in between, meaning the click itself never registered.
///
/// Bounded and event-driven, not a blind sleep: a retry only happens if a
/// short settle window after a click sees no "staged" row, and the whole
/// loop still has a hard ceiling.
///
/// Returns the mark taken right before the click that worked, so a caller
/// can `wait_for_event` a marker (`changes_panel_shown`, say) published by
/// the same `refresh()` that produced the "staged" row.
fn stage_via_checkbox(ide: &Ide, mark: Mark, path: &str) -> Mark {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let row = ide.wait_for_event(mark, "the row to stage", |e| {
            e["ev"] == "changes_row" && e["path"] == path && e["group"] == "unstaged"
        });
        let (x, y) = checkbox_point(&row["rect"]);
        let click_mark = ide.mark();
        ide.click_at(x, y, 1);

        let settle = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        loop {
            let staged = ide
                .events_since_of(click_mark, "changes_row")
                .into_iter()
                .any(|e| e["path"] == path && e["group"] == "staged");
            if staged {
                return click_mark;
            }
            if std::time::Instant::now() >= settle {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out staging {path} via its checkbox after retrying the click"
        );
    }
}

/// Run `git args` against `root`, panicking on a non-zero exit — the same
/// shape `git_fixture`'s own closure uses, pulled out here because G9's
/// three tests below all set up history beyond what `git_fixture` builds.
fn git(root: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

/// Double-clicking a row in the Changes dock opens that file.
///
/// `changedFiles()` reports git's own vocabulary — repository-relative paths
/// — which is what the dock's checkboxes hand straight back to
/// `stageFile`/`unstageFile`. The double-click handler used to pass the same
/// string on to `openFile`, which opens it as a *filesystem* path: a relative
/// one resolves against the process working directory, which is the project
/// root only by accident in a dev run started inside it, and never in a
/// packaged build. Every double-click in `dist/windows` reported "Cannot open
/// file — The system cannot find the file specified. (os error 2)".
///
/// E2E rather than a unit test because the defect was the wiring, not the
/// join: a unit test of "repository root + relative path" passes just as
/// happily against the broken build, since nothing there proves the view asks
/// for it. What makes this test real is that `Ide::spawn` deliberately sets no
/// `current_dir`, so the app inherits the test runner's — never the project —
/// which is the condition a packaged build is always in.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_double_clicking_a_changed_file_opens_it() {
    let name = "e2e_double_clicking_a_changed_file_opens_it";

    let repo = git_fixture(&[("draft.txt", "first draft\n")]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);

    ide.wait_for_ev(Mark::start(), "project_opened");

    // Alt+9 is `vcs.view.changes`; read the geometry marker from the start of
    // the stream, since the dock may already be the visible tab.
    ide.key("alt+9");
    ide.wait_for_event(
        Mark::start(),
        "the Changes dock to report its geometry",
        |e| e["ev"] == "changes_panel_shown",
    );

    let mark = ide.mark();
    // A tracked file modified from outside, rather than a new untracked one:
    // this is the row that has HEAD text behind it, so the double-click
    // reaches the editable diff window instead of stopping at a plain open.
    std::fs::write(ide.project_root().join("draft.txt"), "second draft\n")
        .expect("modifying a tracked file from outside the app");

    let row = ide.wait_for_event(mark, "the modified file's row in the dock", |e| {
        e["ev"] == "changes_row" && e["path"] == "draft.txt" && e["group"] == "unstaged"
    });

    let opened = ide.mark();
    let (x, y) = rect_centre(&row["rect"]);
    ide.double_click_at(x, y, 1);

    // The whole point: a tab, not the "Cannot open file" dialog. Before the
    // fix `openFile` was handed `draft.txt` and went looking for it beside the
    // test runner, so this marker never arrived.
    ide.wait_for_event(opened, "a tab for the double-clicked file", |e| {
        e["ev"] == "tab_added" && e["title"] == "draft.txt"
    });

    assert_eq!(ide.quit(), 0);
}

/// A rename `git status --renames` only pairs with its old path once both
/// sides are in the index (an unstaged rename reads as a plain delete plus
/// an untracked add) — so this stages it with a real `git add -A`, the way
/// a user's own "Stage all" would, and checks the dock shows `R` for it:
/// `status.rs`'s rename parsing (G1/G2), visible end to end through the
/// row marker `changes_panel.cpp` reports from.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_a_renamed_and_staged_file_shows_letter_r() {
    let name = "e2e_a_renamed_and_staged_file_shows_letter_r";

    let repo = git_fixture(&[("old.txt", "content that survives the rename\n")]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);

    ide.wait_for_ev(Mark::start(), "project_opened");

    let mark = ide.mark();
    ide.key("alt+9");
    ide.wait_for_event(
        Mark::start(),
        "the Changes dock to report its geometry",
        |e| e["ev"] == "changes_panel_shown",
    );

    let root = ide.project_root().to_path_buf();
    std::fs::rename(root.join("old.txt"), root.join("new.txt")).expect("renaming old.txt");
    git(&root, &["add", "-A"]);

    let row = ide.wait_for_event(mark, "new.txt to show up renamed and staged", |e| {
        e["ev"] == "changes_row" && e["path"] == "new.txt" && e["group"] == "staged"
    });
    assert_eq!(
        row["status"], "R",
        "a staged rename should show the R status letter"
    );

    assert_eq!(ide.quit(), 0);
}

/// A real, unresolved merge conflict — built before launch so the app's own
/// first status read already sees it — shows up in its own "Merge
/// Conflicts" group with the `C` status letter, and offers no checkbox to
/// stage it with (the plan's own wording: resolving a conflict is not a
/// checkbox toggle). Verified by clicking exactly where a checkable row's
/// indicator would sit and then reading `git status` with the test's own,
/// independent `git` process — the same "trust the seam, not the mirror"
/// reasoning `status_porcelain` above already gives — to confirm the file
/// is still unmerged rather than staged.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_a_conflicted_file_shows_letter_c_with_no_checkbox() {
    let name = "e2e_a_conflicted_file_shows_letter_c_with_no_checkbox";

    let repo = git_fixture(&[("shared.txt", "base\n")]);
    git(repo.path(), &["checkout", "-b", "feature", "--quiet"]);
    std::fs::write(repo.path().join("shared.txt"), "feature change\n").expect("feature edit");
    git(repo.path(), &["commit", "--quiet", "-am", "feature change"]);
    git(repo.path(), &["checkout", "master", "--quiet"]);
    std::fs::write(repo.path().join("shared.txt"), "main change\n").expect("main edit");
    git(repo.path(), &["commit", "--quiet", "-am", "main change"]);
    // Deliberately unresolved: `git merge` exits non-zero here, which is
    // exactly what leaves the unmerged (`UU`) entry `status.rs`'s own
    // `ChangeKind::Conflicted` parsing reads.
    let _ = std::process::Command::new("git")
        .args(["merge", "feature", "--quiet", "--no-edit"])
        .current_dir(repo.path())
        .status()
        .expect("running git merge");

    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);
    ide.wait_for_ev(Mark::start(), "project_opened");

    let mark = ide.mark();
    ide.key("alt+9");
    let row = ide.wait_for_event(mark, "shared.txt to show up conflicted", |e| {
        e["ev"] == "changes_row" && e["path"] == "shared.txt" && e["group"] == "conflicts"
    });
    assert_eq!(
        row["status"], "C",
        "a conflicted file should show the C status letter"
    );

    let (checkbox_x, checkbox_y) = checkbox_point(&row["rect"]);
    ide.click_at(checkbox_x, checkbox_y, 1);

    let root = ide.project_root().to_path_buf();
    assert!(
        status_porcelain(&root).contains("UU shared.txt"),
        "clicking where a conflicted row's checkbox would be must not stage it"
    );

    assert_eq!(ide.quit(), 0);
}

/// Push's own ahead count, read off `changes_toolbar_shown`, starts at 0
/// right after this fixture's one push and moves to 1 the moment a real
/// commit lands through the Changes dock's own Commit button — the same
/// commit flow `e2e_stage_and_commit_through_the_changes_dock` drives, this
/// time against a branch with a real upstream rather than none at all.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_push_carries_the_ahead_count_after_a_local_commit() {
    let name = "e2e_push_carries_the_ahead_count_after_a_local_commit";

    let repo = git_fixture(&[("draft.txt", "first draft\n")]);
    // A bare remote pushed once before launch, closer to a real clone's own
    // starting point than a same-commit tracking branch declared without
    // ever exchanging history would be.
    let remote = tempfile::TempDir::new().expect("temp bare remote dir");
    git(remote.path(), &["init", "--quiet", "--bare"]);
    git(
        repo.path(),
        &["remote", "add", "origin", &remote.path().to_string_lossy()],
    );
    git(repo.path(), &["push", "--quiet", "-u", "origin", "master"]);

    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);
    drop(remote);

    ide.wait_for_ev(Mark::start(), "project_opened");

    let mark = ide.mark();
    ide.key("alt+9");
    let toolbar = ide.wait_for_event(
        Mark::start(),
        "the Changes toolbar to report its geometry",
        |e| e["ev"] == "changes_toolbar_shown",
    );
    assert_eq!(
        toolbar["ahead"].as_i64(),
        Some(0),
        "a branch just pushed should not already show ahead"
    );
    // Edit, stage, commit — written straight to disk, the same reach
    // `e2e_the_project_trees_git_submenu_stages_a_file` uses, since the
    // flow under test is the toolbar's ahead count, not the editor.
    std::fs::write(
        ide.project_root().join("draft.txt"),
        "first draft, revised\n",
    )
    .expect("editing draft.txt");
    let staged_mark = stage_via_checkbox(&ide, mark, "draft.txt");

    // Read fresh from after staging, not from `Mark::start()`: the very
    // first `changes_panel_shown` in the stream can predate the window's
    // own initial layout settling (`ChangesPanel::markShown`'s doc comment
    // has the full reasoning — this was the root cause of
    // `e2e_stage_and_commit_through_the_changes_dock`'s flakiness, and the
    // same stale-rect risk applied here).
    let shown = ide.wait_for_event(
        staged_mark,
        "the Changes dock to report its geometry",
        |e| e["ev"] == "changes_panel_shown",
    );

    let (message_x, message_y) = rect_centre(&shown["message_rect"]);
    ide.click_at(message_x, message_y, 1);
    ide.type_text("bump the draft");
    let (commit_x, commit_y) = rect_centre(&shown["commit_rect"]);
    ide.click_at(commit_x, commit_y, 1);

    ide.wait_for_event(mark, "the ahead count to move after the commit", |e| {
        e["ev"] == "changes_toolbar_shown" && e["ahead"].as_i64() == Some(1)
    });

    assert_eq!(ide.quit(), 0);
}

/// The subject of the repository's current `HEAD` commit, read with a plain
/// `git` subprocess from the *test* — never through the app — so a pass
/// proves the whole seam (Changes dock -> bridge -> `vcs-core` -> a real
/// `git` process) actually produced a commit, not that each layer's own
/// unit tests agree with each other. Duplicated from `e2e.rs`, moved here
/// with the test that uses it (R6: this file grew past its ceiling with the
/// three-hunk staging flow, and `e2e.rs` was already at its own
/// grandfathered baseline).
fn head_commit_subject(repo: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(repo)
        .output()
        .expect("git log");
    String::from_utf8(output.stdout)
        .expect("git log output is UTF-8")
        .trim()
        .to_string()
}

/// F3-17: staging a file and committing through the Changes dock's own
/// checkboxes and button produces a real commit — the one property no unit
/// test can prove, since it is specifically about the dock's widgets driving
/// the real seam (dock -> bridge -> `vcs-core` -> a `git` subprocess) rather
/// than each layer agreeing with itself. Verified with a `git log`/`git show`
/// run by the *test*, independent of anything the app itself would report.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_stage_and_commit_through_the_changes_dock() {
    const ORIGINAL: &str = "first draft\n";
    const EDITED: &str = "first draft, revised\n";
    // Lowercase, for the same reason `e2e_hunk_revert_is_one_undo_never_
    // touches_disk`'s edit is: no Shift for `xdotool type` to combine with a
    // modifier a preceding key chord left down.
    const MESSAGE: &str = "revise the draft";
    let name = "e2e_stage_and_commit_through_the_changes_dock";

    let repo = git_fixture(&[("draft.txt", ORIGINAL)]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);

    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let tab = open_file(&ide, "draft.txt");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // Show the Changes dock before editing, so the refresh a save triggers
    // (`EditorTabs::saveTab`) runs while the panel is already visible and
    // its rows lay out to real, clickable geometry. `vcs_menu.cpp` also
    // raises this dock on its own the moment the repository is discovered
    // (before this line ever runs, since the fixture's `.git` is already on
    // disk at launch), so Alt+9 below may find it already the visible tab
    // and toggle nothing — this flow never depends on that keystroke having
    // produced a marker of its own.
    let mark = ide.mark();
    ide.key("alt+9"); // vcs.view.changes' default shortcut (keymap.rs).

    ide.key("ctrl+Home");
    ide.key("End");
    ide.type_text(", revised");
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        ide.read_project_file("draft.txt"),
        EDITED,
        "the fixture's shape changed"
    );

    // The save above just made `EditorTabs::saveTab` ask `VcsService` to
    // look again — `stage_via_checkbox` waits for the row that answer
    // produced, then clicks its checkbox glyph itself (`Space` on the row
    // once merely current turned out not to toggle it — no default
    // `QAbstractItemView` keyboard binding does that; only clicking the
    // indicator does, confirmed against a real run under Xvfb).
    let staged_mark = stage_via_checkbox(&ide, mark, "draft.txt");

    // `ChangesPanel::refresh` (which just produced the "staged" row above)
    // re-publishes `changes_panel_shown` right after its row markers, so
    // this is read fresh from after `staged_mark` rather than from
    // whichever `changes_panel_shown` happened to be first in the whole
    // stream — the earliest one can predate the window's own initial
    // layout settling (the dock auto-raises on repository discovery, which
    // can run before the main window has finished laying itself out), and
    // a rect read from it is not reliably where the widgets ended up. See
    // `ChangesPanel::markShown`'s doc comment for the full reasoning; this
    // was the root cause of this flow's flakiness.
    let shown = ide.wait_for_event(
        staged_mark,
        "the Changes dock to report its geometry",
        |e| e["ev"] == "changes_panel_shown",
    );

    // Type the commit message and click Commit.
    let (message_x, message_y) = rect_centre(&shown["message_rect"]);
    ide.click_at(message_x, message_y, 1);
    ide.type_text(MESSAGE);
    let (commit_x, commit_y) = rect_centre(&shown["commit_rect"]);
    ide.click_at(commit_x, commit_y, 1);

    // The dock's own click is fire-and-forget (`ChangesPanel::doCommit`
    // queues the commit on `VcsService`'s worker thread and returns), so
    // this polls the filesystem — the one channel this harness trusts as
    // much as the marker stream (`crates/e2e/src/lib.rs`) — rather than
    // inventing a fixed delay.
    let repo_root = ide.project_root().to_path_buf();
    e2e::wait_for("the commit to land", || {
        (head_commit_subject(&repo_root) == MESSAGE).then_some(())
    });

    let output = std::process::Command::new("git")
        .args(["show", "HEAD:draft.txt"])
        .current_dir(&repo_root)
        .output()
        .expect("git show");
    assert_eq!(
        String::from_utf8(output.stdout).expect("git show output is UTF-8"),
        EDITED,
        "the commit did not carry the edited content"
    );

    assert_eq!(ide.quit(), 0);
}

/// The step a naive harness would spell `sleep`. Duplicated from `e2e.rs`
/// for the same reason `git_fixture` above is: a twenty-line helper is not
/// worth a third crate between the two test binaries.
fn wait_for_index(mcp: &e2e::mcp::Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", serde_json::json!({}))["ready"] == true).then_some(())
    });
}

/// Open one file through Go to File — `open_search_popup`/`accept_top_hit`
/// above already exist in this file for
/// `e2e_commit_log_expand_and_open_commit_detail`'s own Find Action reach;
/// this is the same two calls `e2e.rs`'s own `open_file` makes.
fn open_file(ide: &Ide, name: &str) -> serde_json::Value {
    let mark = open_search_popup(ide, "ctrl+shift+n");
    accept_top_hit(ide, mark, name);
    ide.wait_for_event(mark, &format!("a tab for `{name}`"), |e| {
        e["ev"] == "tab_added" && e["title"] == name
    })
}

/// R6: staging one hunk through `vcs.stageHunk` (the gutter popup's
/// keyboard equivalent, `vcs_menu.cpp`) must touch the index for that hunk
/// alone — the regression `stage_hunk_matching` (`vcs-core`) exists to fix.
/// Two edits far enough apart to diff as two separate hunks, caret parked
/// in the first one, staged, then `git diff --cached` on the *test's* own
/// `git` (never the app's own reporting) is asserted to carry only that
/// hunk's line, with the second edit's line still only in the unstaged
/// diff.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_stage_hunk_touches_only_that_hunks_index_entry() {
    const ORIGINAL: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
    let name = "e2e_stage_hunk_touches_only_that_hunks_index_entry";

    let repo = git_fixture(&[("draft.txt", ORIGINAL)]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);

    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let tab = open_file(&ide, "draft.txt");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // First hunk: replace "one" on line 1. Lowercase throughout this flow,
    // for the same reason `e2e_hunk_revert_is_one_undo_never_touches_disk`'s
    // edit is: `xdotool type`'s Shift for a capital letter can combine with
    // a modifier a preceding `xdotool key` chord has not yet released.
    ide.key("ctrl+Home");
    ide.key("End");
    let mark = ide.mark();
    ide.key("shift+Home");
    ide.type_text("uno");
    ide.wait_for_event(mark, "the tab to go dirty", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });

    // Second hunk: replace "ten" on the last content line — far enough from
    // the first that `diff_lines` reports two hunks, not one spanning both
    // (`CONTEXT_LINES` is 3 on each side; eight unchanged lines separate
    // them). Nine `Down`s from line 1 rather than `Ctrl+End`/`Up`: `ORIGINAL`
    // has exactly ten lines, and `Down`/`End`/`Home` are the same primitives
    // the first hunk's edit already used successfully, rather than a second
    // navigation idiom this suite has not exercised elsewhere.
    for _ in 0..9 {
        ide.key("Down");
    }
    ide.key("End");
    ide.key("shift+Home");
    ide.type_text("diez");
    ide.wait_for_event(mark, "the gutter to see both edits as two hunks", |e| {
        e["ev"] == "vcs_hunks_applied" && e["count"].as_u64() == Some(2)
    });

    // Caret back on the first hunk's line before staging — `stageHunkAtCaret`
    // (like `rollbackHunkAtCaret`) finds whichever cached hunk contains it.
    ide.key("ctrl+Home");

    // Drive `vcs.stageHunk` through the VCS menu, the same keyboard-only
    // reach `e2e_hunk_revert_is_one_undo_never_touches_disk` uses for
    // `vcs.rollbackHunk` — no coordinate computed for a 1px gutter marker.
    // Commit, Push, Pull, Fetch, Branches, (separator), Show Diff, Rollback
    // Hunk, Stage Hunk: one more Down than that test's 6, since Stage Hunk
    // sits right after Rollback Hunk (`vcs_menu.cpp`).
    ide.key("alt+c");
    ide.wait_for_event(mark, "the VCS menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "vcs_menu"
    });
    for _ in 0..7 {
        ide.key("Down");
    }
    ide.key("Return");
    ide.wait_for_event(mark, "the VCS menu to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "vcs_menu"
    });
    let staged = ide.wait_for_event(mark, "vcs_hunk_staged", |e| e["ev"] == "vcs_hunk_staged");
    assert_eq!(
        staged["hunk_index"].as_u64(),
        Some(0),
        "the caret's own hunk (the first) should have been the one staged"
    );

    // Staging writes straight into the index from the patch built out of the
    // buffer (never the file on disk, which is still `ORIGINAL` at this
    // point) — save now so the "still unstaged" assertion below compares
    // the index against a working tree that actually carries both edits,
    // the same shape a user would leave the file in.
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });

    let root = ide.project_root().to_path_buf();
    e2e::wait_for("the first hunk to reach the index", || {
        let staged_diff = String::from_utf8(
            std::process::Command::new("git")
                .args(["diff", "--cached"])
                .current_dir(&root)
                .output()
                .expect("git diff --cached")
                .stdout,
        )
        .expect("git diff --cached output is UTF-8");
        (staged_diff.contains("+uno") && !staged_diff.contains("+diez")).then_some(())
    });

    // The second hunk must still be unstaged, nowhere near the index.
    let staged_diff = String::from_utf8(
        std::process::Command::new("git")
            .args(["diff", "--cached"])
            .current_dir(&root)
            .output()
            .expect("git diff --cached")
            .stdout,
    )
    .expect("git diff --cached output is UTF-8");
    assert!(
        staged_diff.contains("+uno") && !staged_diff.contains("+diez"),
        "git diff --cached should carry only the staged hunk:\n{staged_diff}"
    );
    let unstaged_diff = String::from_utf8(
        std::process::Command::new("git")
            .args(["diff"])
            .current_dir(&root)
            .output()
            .expect("git diff")
            .stdout,
    )
    .expect("git diff output is UTF-8");
    assert!(
        unstaged_diff.contains("+diez") && !unstaged_diff.contains("+uno"),
        "the still-unstaged hunk should remain in the working tree, not the index:\n{unstaged_diff}"
    );

    assert_eq!(ide.quit(), 0);
}
