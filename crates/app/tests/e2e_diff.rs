//! End-to-end flow for the JetBrains-style diff viewer (F3-24): the editable
//! HEAD-vs-working-tree window, its toolbar, and the apply chevron.
//!
//! Its own test binary for the reason `e2e_vcs.rs` gives — `e2e.rs` sits at
//! its ratcheted size ceiling — and `make e2e` runs it with the others.

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

/// A fresh temp directory holding `files`, committed to a brand-new Git
/// repository. Duplicated from `e2e_vcs.rs` by the same judgement that
/// duplicated it from `e2e.rs`: a twenty-line fixture is not worth a crate
/// between the test binaries, and `.git` must exist before launch because
/// `VcsService::open_project` discovers it the instant the project opens.
fn git_fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp git fixture dir");
    for (relative, content) in files {
        std::fs::write(dir.path().join(relative), content).expect("fixture file");
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
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "user.name", "E2E"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "initial"]);
    dir
}

/// The centre of a `[x, y, w, h]` marker field.
fn rect_centre(rect: &serde_json::Value) -> (i32, i32) {
    let n = |i: usize| rect[i].as_i64().expect("rect component") as i32;
    (n(0) + n(2) / 2, n(1) + n(3) / 2)
}

/// Open one file through Go to File and wait for its tab.
fn open_file(ide: &Ide, name: &str) {
    let main_window = ide.window().to_string();
    let mark = ide.mark();
    ide.key("ctrl+shift+n");
    ide.wait_for_event(mark, "the search popup to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "search_everywhere"
    });
    ide.wait_for_focus_change(&main_window);
    ide.wait_for_ev(mark, "search_results");
    let mark = ide.mark();
    ide.type_text(name);
    ide.wait_for_event(mark, &format!("results for `{name}`"), |e| {
        e["ev"] == "search_results" && e["count"].as_u64().unwrap_or(0) > 0
    });
    ide.key("Return");
    ide.wait_for_event(mark, "the search popup to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "search_everywhere" && e["accepted"] == true
    });
    ide.focus_main();
    ide.wait_for_event(mark, &format!("a tab for `{name}`"), |e| {
        e["ev"] == "tab_added" && e["title"] == name
    });
}

/// Pick the `steps`-th entry below the current one in a combo box at
/// `rect`: click it open, arrow down, accept.
fn pick_combo_entry(ide: &Ide, rect: &serde_json::Value, steps: usize) {
    let (x, y) = rect_centre(rect);
    ide.click_at(x, y, 1);
    for _ in 0..steps {
        ide.key("Down");
    }
    ide.key("Return");
}

fn recomputed(ide: &Ide, mark: Mark, hunks: u64, whitespace: &str) -> serde_json::Value {
    ide.wait_for_event(
        mark,
        &format!("a diff recompute with {hunks} hunks under `{whitespace}`"),
        |e| e["ev"] == "diff_recomputed" && e["hunks"] == hunks && e["whitespace"] == whitespace,
    )
}

/// The working tree differs from HEAD in three ways: a rewritten line, a
/// line that only gained trailing whitespace, and a deleted line. The
/// viewer must show three hunks, drop to two under "Ignore whitespaces",
/// switch to the unified viewer and back, and apply a hunk through its
/// chevron so that the live buffer — recomputed from the editor, not from
/// disk — shows one hunk fewer.
#[test]
#[ignore = "drives the real binary under Xvfb; run with `make e2e`"]
fn e2e_diff_window_jetbrains_controls() {
    let original: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let fixture = git_fixture(&[("notes.txt", &original)]);
    let modified: String = (1..=20)
        .filter(|&i| i != 10)
        .map(|i| match i {
            2 => "line TWO\n".to_string(),
            5 => "line 5   \n".to_string(),
            _ => format!("line {i}\n"),
        })
        .collect();
    std::fs::write(fixture.path().join("notes.txt"), &modified).expect("working-tree edit");

    let mut ide = Ide::launch("e2e_diff_window_jetbrains_controls", APP, fixture.path());
    open_file(&ide, "notes.txt");
    // The gutter's hunks are the sign the repository was discovered and the
    // file diffed; Show Diff needs both.
    ide.wait_for_event(
        Mark::start(),
        "the working-tree hunks to reach the gutter",
        |e| e["ev"] == "vcs_hunks_applied" && e["count"].as_u64().unwrap_or(0) > 0,
    );

    // VCS ▸ Show Diff, by its on-screen rect: the menu's Ctrl+Alt+G does not
    // reach Qt under bare Xvfb, and counting arrow presses would break the
    // moment the menu gains an entry.
    let opened = ide.mark();
    ide.key("alt+c"); // "V&CS" — see vcs_menu.cpp for why not Alt+V.
    let show_diff = ide.wait_for_event(opened, "Show Diff in the VCS menu", |e| {
        e["ev"] == "vcs_menu_action" && e["label"] == "Show Diff"
    });
    let (x, y) = rect_centre(&show_diff["rect"]);
    ide.click_at(x, y, 1);

    let toolbar = ide.wait_for_ev(opened, "diff_toolbar_shown");
    recomputed(&ide, opened, 3, "exact");

    // Unified viewer and back.
    let mark = ide.mark();
    pick_combo_entry(&ide, &toolbar["viewer_rect"], 1);
    ide.wait_for_event(mark, "the unified viewer", |e| {
        e["ev"] == "diff_viewer_mode" && e["mode"] == "unified"
    });
    let mark = ide.mark();
    let (x, y) = rect_centre(&toolbar["viewer_rect"]);
    ide.click_at(x, y, 1);
    ide.key("Up");
    ide.key("Return");
    ide.wait_for_event(mark, "the side-by-side viewer again", |e| {
        e["ev"] == "diff_viewer_mode" && e["mode"] == "side_by_side"
    });

    // The first hunk's chevron replaces `line TWO` with HEAD's `line 2` in
    // the live buffer; the recompute that follows reads that buffer.
    let chevron = ide.wait_for_event(opened, "the first hunk's chevron", |e| {
        e["ev"] == "diff_chevron" && e["hunk_index"] == 0
    });
    let mark = ide.mark();
    let (x, y) = rect_centre(&chevron["rect"]);
    ide.click_at(x, y, 1);
    ide.wait_for_ev(mark, "diff_hunk_applied");
    recomputed(&ide, mark, 2, "exact");

    // "Ignore whitespaces" is the third entry: the trailing-space hunk goes.
    let mark = ide.mark();
    pick_combo_entry(&ide, &toolbar["whitespace_rect"], 2);
    recomputed(&ide, mark, 1, "ignore_all");

    // The chevron edited the live buffer, so the file on disk still has
    // `line TWO` until the window's own Ctrl+S — the same save the tab has.
    // (Without it, Ctrl+Q would stop at the unsaved-changes prompt.) The
    // save rules trim line 5's trailing spaces on the way out, so only the
    // deleted line 10 is left of the working-tree edits.
    let expected: String = (1..=20)
        .filter(|&i| i != 10)
        .map(|i| format!("line {i}\n"))
        .collect();
    ide.key("ctrl+s");
    e2e::wait_for("the applied hunk to reach the file on disk", || {
        (ide.read_project_file("notes.txt") == expected).then_some(())
    });

    ide.focus_main();
    assert_eq!(ide.quit(), 0);
}
