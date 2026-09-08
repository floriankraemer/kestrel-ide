//! End-to-end flows: the real binary, under Xvfb, driven with xdotool.
//!
//! Every test here is `#[ignore]`d, so `cargo test --workspace` is exactly as
//! fast as it was before this file existed. `make e2e` runs them.
//!
//! They live in `crates/app` rather than `crates/e2e` for one reason:
//! `CARGO_BIN_EXE_app` is only defined for integration tests of the crate
//! that declares the binary. The harness itself is `crates/e2e`, which
//! depends on no workspace crate.
//!
//! What these cover is what nothing else can: signal/slot wiring, cross-
//! thread delivery, widget lifetime, index-identity mapping at the model
//! edge, keyboard and focus routing, dialog flows, and the view restoring
//! itself from persisted state. Everything that would still be meaningful
//! with the Qt event loop removed is a unit test somewhere else.
//!
//! One product limitation shapes what can be asserted where: MCP's
//! `read_buffer` answers from `editor_core::Document`'s rope, which is
//! populated on open and refreshed only on save — it does not see unsaved
//! typing. Dirty state is therefore observed through the marker stream and
//! content through the file on disk.

use std::path::{Path, PathBuf};

use e2e::{mcp::Mcp, Ide, Mark};
use serde_json::json;

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The fixture file's content as it is checked in — the independent answer
/// to "did the app change this file", read without going through the app.
fn fixture_text(name: &str, relative: &str) -> String {
    std::fs::read_to_string(fixture(name).join(relative)).expect("fixture file")
}

/// Wait until the project index reports itself complete.
///
/// The step a naive harness would spell `sleep`. Everything that searches —
/// Go to File, Search Everywhere, the name-based rename — answers "still
/// being built" until this is true, and a fixed delay only makes that
/// answer intermittent.
fn wait_for_index(mcp: &Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", json!({}))["ready"] == true).then_some(())
    });
}

/// Open the Search Everywhere popup with `shortcut` and wait until its
/// opening query has been answered, so a later `search_results` marker can
/// only be one the test's own typing caused.
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

/// Open one file through Go to File, returning its `tab_added` marker.
fn open_file(ide: &Ide, name: &str) -> serde_json::Value {
    let mark = open_search_popup(ide, "ctrl+shift+n");
    accept_top_hit(ide, mark, name);
    ide.wait_for_event(mark, &format!("a tab for `{name}`"), |e| {
        e["ev"] == "tab_added" && e["title"] == name
    })
}

fn buffer(mcp: &Mcp, tab_id: u64) -> String {
    mcp.call("read_buffer", json!({ "tab_id": tab_id }))["content"]
        .as_str()
        .expect("read_buffer returns a string")
        .to_string()
}

/// The centre of a `[x, y, w, h]` marker field — used for `changes_row`'s
/// and `changes_panel_shown`'s rects so a flow never computes a click point
/// from window geometry or font metrics.
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

/// A point on a `changes_row` marker's checkbox glyph — measured against a
/// real screenshot under Xvfb (10px in from the row's own left edge, at its
/// vertical centre), not the row's text label: `QAbstractItemView` toggles a
/// checkable item's check state only on a genuine click on the indicator
/// itself, no keyboard binding does it.
fn checkbox_point(rect: &serde_json::Value) -> (i32, i32) {
    let rect: Vec<i64> = rect
        .as_array()
        .expect("the marker carries a rect")
        .iter()
        .map(|v| v.as_i64().expect("an integer"))
        .collect();
    (rect[0] as i32 + 10, (rect[1] + rect[3] / 2) as i32)
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
    // only; `--global` would leak between test runs on a shared machine.
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "user.name", "E2E"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "initial"]);
    dir
}

/// The subject of the repository's current `HEAD` commit, read with a plain
/// `git` subprocess from the *test* — never through the app — so a pass
/// proves the whole seam (Changes dock -> bridge -> `vcs-core` -> a real
/// `git` process) actually produced a commit, not that each layer's own
/// unit tests agree with each other.
fn head_commit_subject(repo: &Path) -> String {
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

// ---------------------------------------------------------------------------

/// The harness itself: a seeded launch reaches a mapped, focused window with
/// its project open, and Ctrl+Q exits cleanly. Every flow below assumes this,
/// so it is worth failing on its own.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_launches_with_its_project_and_quits() {
    let mut ide = Ide::launch(
        "e2e_launches_with_its_project_and_quits",
        APP,
        fixture("tiny"),
    );

    let opened = ide.wait_for_ev(Mark::start(), "project_opened");
    assert_eq!(
        Path::new(opened["root"].as_str().expect("root is a string")),
        ide.project_root(),
        "the app opened a different project than the one it was seeded with"
    );

    assert_eq!(ide.quit(), 0);
}

/// Opening a project (ADR-0037: the directory walk moved off the Qt thread)
/// still completes and leaves both the sidebar tree and the search index
/// fully populated — not just "some project opened", the specific one this
/// launch was seeded with, tree and index agreeing on every file in it.
///
/// `async_open` has enough files and a nested subdirectory precisely so a
/// walk that raced its own installation (partial tree, or a tree that
/// disagreed with the index another async worker built over the same
/// files) would show up here as a missing or wrong-content file, not as a
/// timeout — a stuck walk already fails via `wait_for_ev`'s own timeout, so
/// this test's job is proving the walk's *result* is complete and correct,
/// not that it eventually happens at all.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_open_folder_walk_completes_correctly() {
    let name = "e2e_open_folder_walk_completes_correctly";
    let mut ide = Ide::launch(name, APP, fixture("async_open"));
    let mcp = ide.mcp();

    let opened = ide.wait_for_ev(Mark::start(), "project_opened");
    assert_eq!(
        Path::new(opened["root"].as_str().expect("root is a string")),
        ide.project_root(),
        "the app opened a different project than the one it was seeded with"
    );
    wait_for_index(&mcp);

    // A file several directories deep, only reachable at all if the async
    // walk actually recursed into `src/nested/deep` rather than stopping
    // short or racing its own tree installation.
    let original = fixture_text("async_open", "src/nested/deep/target.rs");
    let tab = open_file(&ide, "target.rs");
    assert_eq!(
        buffer(&mcp, tab["tab_id"].as_u64().expect("tab_id")),
        original
    );

    assert_eq!(ide.quit(), 0);
}

/// Open, edit, save, undo.
///
/// Two assertions here have no other net. The marker's `tab_id` and MCP's
/// view of the open buffers must agree — a disagreement is the identity
/// mapping bug class, where an off-by-one at the model edge closes the wrong
/// tab. And **one** Ctrl+Z must undo the whole edit: that is the edit-block
/// granularity guard, and C++ is where the `beginEditBlock` lives.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_open_project_edit_save() {
    let name = "e2e_open_project_edit_save";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let original = fixture_text("tiny", "src/main.rs");
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // The view says which widget index it put the tab at; MCP says which
    // TabId the session believes is open. They are two different layers'
    // answers to the same question and must be the same answer.
    let buffers = mcp.call("list_open_buffers", json!({}));
    let buffers = buffers.as_array().expect("a list of buffers");
    assert_eq!(buffers.len(), 1, "expected exactly one open buffer");
    assert_eq!(buffers[0]["tab_id"].as_u64(), Some(tab_id));
    assert_eq!(buffers[0]["title"], tab["title"]);
    assert_eq!(tab["index"].as_i64(), Some(0));
    assert_eq!(buffer(&mcp, tab_id), original);

    // One character, so that one Ctrl+Z is unambiguously one edit rather
    // than a bet on Qt's keystroke coalescing.
    let mark = ide.mark();
    ide.type_text("x");
    ide.wait_for_event(mark, "the tab to go dirty", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });

    let edited = format!("x{original}");
    let mark = ide.mark();
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        ide.read_project_file("src/main.rs"),
        edited,
        "Ctrl+S did not write the edit to disk"
    );
    assert_eq!(buffer(&mcp, tab_id), edited);

    // The guard: one undo, the whole edit. Observed after a save because
    // `read_buffer` answers from `editor_core::Document`'s rope, which is
    // only refreshed on save (see the note at the top of this file).
    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the tab to go dirty again", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    let mark = ide.mark();
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the undone tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        ide.read_project_file("src/main.rs"),
        original,
        "one Ctrl+Z did not undo the whole edit"
    );
    assert_eq!(buffer(&mcp, tab_id), original);

    assert_eq!(ide.quit(), 0);
}

/// Search Everywhere, debounced, then a jump.
///
/// The assertion worth the flow is the count: ten keystrokes must produce at
/// most two delivered result sets. That proves both that typing is debounced
/// and that superseded generations were *discarded* rather than rendered —
/// the canonical cross-thread bug, and one `bridge.rs`'s own tests cannot
/// see because they test the cancel decision, not the delivery.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_search_everywhere_jump() {
    let name = "e2e_search_everywhere_jump";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    // Somewhere to come back to.
    let main_tab = open_file(&ide, "main.rs");
    let main_tab_id = main_tab["tab_id"].as_u64().expect("tab_id");
    let before_jump = cursor(&mcp, main_tab_id);

    const QUERY: &str = "shout_loud"; // ten keystrokes, exactly.
    assert_eq!(QUERY.len(), 10);
    let mark = open_search_popup(&ide, "ctrl+shift+e");
    accept_top_hit(&ide, mark, QUERY);

    let delivered = ide.events_since_of(mark, "search_results");
    assert!(
        delivered.len() <= 2,
        "{} result sets were delivered for {} keystrokes — either typing is \
         not debounced, or superseded generations are being rendered instead \
         of discarded: {delivered:?}",
        delivered.len(),
        QUERY.len()
    );

    let tab = ide.wait_for_event(mark, "a tab for the symbol's file", |e| {
        e["ev"] == "tab_added" && e["title"] == "greeting.rs"
    });
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // The expected line comes from the fixture, read here — not from the
    // index, which is the thing being checked.
    let declaration = fixture_text("tiny", "src/greeting.rs")
        .lines()
        .position(|line| line.contains(&format!("fn {QUERY}ly")))
        .expect("the fixture declares shout_loudly") as u32;
    e2e::wait_for("the caret to land on the declaration", || {
        (cursor(&mcp, tab_id).0 == declaration).then_some(())
    });

    // Back, and the pre-jump location is restored.
    let mark = ide.mark();
    ide.key("ctrl+alt+Left");
    e2e::wait_for("the caret to return to where the jump started", || {
        (cursor(&mcp, main_tab_id) == before_jump).then_some(())
    });
    let _ = mark;

    assert_eq!(ide.quit(), 0);
}

fn cursor(mcp: &Mcp, tab_id: u64) -> (u32, u32) {
    let position = mcp.call("get_cursor_position", json!({ "tab_id": tab_id }));
    (
        position["line"].as_u64().unwrap_or(0) as u32,
        position["column"].as_u64().unwrap_or(0) as u32,
    )
}

/// Rename through the preview dialog, cancelled and then applied.
///
/// Two dialog bug classes with no other net. Escape must leave every byte on
/// disk and in the buffer untouched — a cancel that applies anyway is
/// invisible to any test that does not look at the filesystem afterwards.
/// And the preview must list what the rule found, not a subset of it, so the
/// row count is compared against occurrences counted here from the fixture.
///
/// No language server is installed in `linux-builder`, so this exercises the
/// name-based fallback (`index_core::plan_index_rename`) — which is the path
/// the preview dialog exists for in the first place.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_rename_with_preview() {
    const SYMBOL: &str = "shout_loudly";
    let name = "e2e_rename_with_preview";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    // What the rule should find, counted independently of the index.
    let occurrences: usize = ["src/main.rs", "src/greeting.rs"]
        .iter()
        .map(|f| fixture_text("tiny", f).matches(SYMBOL).count())
        .sum();
    assert_eq!(occurrences, 2, "the fixture's shape changed");

    let tab = open_file(&ide, "greeting.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    let before = ide.project_snapshot();

    // Cancel first.
    let mark = ide.mark();
    start_rename(&ide, SYMBOL, "yell");
    let rows = ide.wait_for_ev(mark, "preview_rows");
    assert_eq!(
        rows["count"].as_u64(),
        Some(occurrences as u64),
        "the preview listed a subset of what the rename plan found"
    );
    ide.key("Escape");
    ide.wait_for_event(mark, "the preview to be cancelled", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "refactor_preview" && e["accepted"] == false
    });
    ide.focus_main();
    // A reply to an editor-touching MCP call is proof the Qt event loop has
    // run everything queued before it — so if a cancelled preview had
    // applied anything, its marker would already be here.
    ide.sync(&mcp);
    assert!(
        ide.events_since_of(mark, "workspace_edit_applied")
            .is_empty(),
        "Escape on the preview applied the rename anyway"
    );
    assert_eq!(
        ide.project_snapshot(),
        before,
        "Escape changed a file on disk"
    );
    assert_eq!(
        buffer(&mcp, tab_id),
        fixture_text("tiny", "src/greeting.rs")
    );

    // Now apply.
    let mark = ide.mark();
    start_rename(&ide, SYMBOL, "yell");
    ide.wait_for_ev(mark, "preview_rows");
    ide.key("Return");
    let applied = ide.wait_for_ev(mark, "workspace_edit_applied");
    assert_eq!(applied["documents"].as_u64(), Some(2));
    ide.focus_main();

    e2e::wait_for("the closed file to be rewritten on disk", || {
        ide.read_project_file("src/main.rs")
            .contains("yell")
            .then_some(())
    });
    ide.wait_for_event(mark, "the open file's buffer to be spliced", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });

    // One Ctrl+Z for the whole rename in this buffer — that is what the
    // single `beginEditBlock` in `applyBufferEdits` buys, and it is the only
    // thing checking it.
    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the buffer to return to its saved state", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.key("ctrl+s");
    ide.sync(&mcp);
    assert_eq!(
        ide.read_project_file("src/greeting.rs"),
        fixture_text("tiny", "src/greeting.rs"),
        "one Ctrl+Z did not undo the whole rename in the open buffer"
    );

    assert_eq!(ide.quit(), 0);
}

/// Put the caret inside `symbol`'s declaration and drive Shift+F6 up to the
/// preview dialog.
fn start_rename(ide: &Ide, symbol: &str, new_name: &str) {
    let mark = open_search_popup(ide, "ctrl+shift+o");
    accept_top_hit(ide, mark, symbol);
    // The jump lands at column 0; the declaration's name starts after
    // "pub fn ". One key past that is safely inside the word.
    for _ in 0..8 {
        ide.key("Right");
    }

    let main_window = ide.window().to_string();
    ide.key("shift+F6");
    // The name prompt is a QInputDialog — no marker of its own, so the
    // observable transition is the input focus moving off the main window.
    ide.wait_for_focus_change(&main_window);
    ide.type_text(new_name);
    ide.key("Return");
}

/// Two carets, one keystroke, one undo (F1-18).
///
/// The caret is walked to the middle of the fixture's first "world" by a
/// fixed count of `Right` presses from the start of the file — the same
/// keyboard-only positioning `start_rename` already uses, and deliberately
/// not Find-then-Escape: closing the find bar hands focus back to the
/// editor asynchronously, and a `Ctrl+D` sent before that lands collapses
/// the selection a moment earlier, which is exactly the kind of race this
/// suite exists to not paper over with a wait.
///
/// The first `Ctrl+D` selects the word the (collapsed) caret sits in; the
/// second finds the next occurrence and adds a caret for it —
/// `SelectionSet::add_next_occurrence`'s own two-step rule.
///
/// Asserted through a save each time, not `read_buffer` on its own: MCP
/// answers from `editor_core::Document`'s rope, which `save_tab` is what
/// refreshes (see the note at the top of this file) — the same reason
/// `e2e_open_project_edit_save` saves before every `buffer()` call.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_multi_caret_edit_is_one_undo() {
    let name = "e2e_multi_caret_edit_is_one_undo";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let original = fixture_text("tiny", "src/main.rs");
    assert_eq!(
        original.matches("world").count(),
        2,
        "the fixture's shape changed"
    );

    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // Line 4 (0-indexed from Ctrl+Home): `    println!("{}", greeting::greet("world"));`.
    // Column 38 sits between the 'o' and the 'r' of "world" — inside the
    // word, which is what makes the first Ctrl+D select it rather than an
    // empty caret.
    ide.key("ctrl+Home");
    for _ in 0..3 {
        ide.key("Down");
    }
    for _ in 0..38 {
        ide.key("Right");
    }

    // First Ctrl+D selects "world"; the second adds the next occurrence.
    ide.key("ctrl+d");
    ide.key("ctrl+d");

    let mark = ide.mark();
    ide.type_text("!");
    ide.wait_for_event(mark, "the tab to go dirty", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);

    let edited = buffer(&mcp, tab_id);
    let mut expected = original.replacen("world", "!", 1);
    expected = expected.replacen("world", "!", 1);
    assert_eq!(
        edited, expected,
        "typing at two carets did not replace both selections with one character each"
    );

    // The guard: one undo restores both occurrences, because the whole
    // two-caret replacement crossed the seam as one `Vec<FfiTextEdit>` and
    // was spliced inside one `beginEditBlock` (ADR-0023).
    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the tab to go dirty again", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the undone tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo the two-caret edit"
    );

    assert_eq!(ide.quit(), 0);
}

/// Comment toggle and duplicate line, each its own undo (F1-18).
///
/// Reformat is not exercised here: no language server is installed in
/// `linux-builder` (F0-14's rust-analyzer image is a separate, opt-in
/// stage), so `code.reformat` has nothing to talk to in this environment.
/// Comment toggle and the line operations need no server at all — they are
/// `syntax-core`'s registry and a rope, which is exactly why they are
/// covered here instead.
///
/// Asserted through a save each time, not `read_buffer` on its own — see
/// the note on `e2e_multi_caret_edit_is_one_undo`.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_comment_toggle_and_duplicate_line() {
    let name = "e2e_comment_toggle_and_duplicate_line";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let original = fixture_text("tiny", "src/main.rs");
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");

    // Comment the first line, save, then undo and save again.
    let mark = ide.mark();
    ide.key("ctrl+slash");
    ide.wait_for_event(mark, "the tab to go dirty", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert!(
        buffer(&mcp, tab_id).starts_with("// mod greeting;"),
        "Ctrl+/ did not comment the first line"
    );

    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the tab to go dirty again", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the undone tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo the comment toggle"
    );

    // Duplicate the first line, save, then undo and save again.
    let mark = ide.mark();
    ide.key("ctrl+alt+d");
    ide.wait_for_event(mark, "the tab to go dirty", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert!(
        buffer(&mcp, tab_id).starts_with("mod greeting;\nmod greeting;\n"),
        "Ctrl+Alt+D did not duplicate the first line"
    );

    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the tab to go dirty again", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the undone tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo the duplicated line"
    );

    assert_eq!(ide.quit(), 0);
}

/// Where `stub_server` lands: Cargo places every workspace binary in the
/// same `target/<profile>/` directory as `app`'s own, and
/// `CARGO_BIN_EXE_stub_server` is not an option here — Cargo only sets a
/// binary's `CARGO_BIN_EXE_*` for integration tests of the crate that
/// declares it (`lsp-core`'s own, not `app`'s; see
/// `lsp-core/tests/stub_server_session.rs:15`).
fn stub_server_path() -> PathBuf {
    Path::new(APP).with_file_name("stub_server")
}

/// Route the `rust` language id at `lsp-core`'s X2 stub server rather than a
/// real `rust-analyzer` — not installed in this image, and the point of F2's
/// flows is the client's own behaviour, which the stub is built to exercise
/// deterministically by request line (`stub_server.rs`'s own doc comment).
///
/// Requires a restart: `LanguageService` resolves the server table once, on
/// `openProject`, from whatever `app-config` already has on disk — so the
/// override has to be written before the project opens, not after.
fn route_rust_at_stub(ide: &mut Ide) {
    assert_eq!(ide.quit(), 0);
    let mut settings = app_config::load(&ide.config_dir()).expect("settings just written");
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

/// F2-8/F2-10: Alt+Enter merges the diagnostic-scoped and range-scoped
/// intentions at the caret into one grouped popup, and applying one goes
/// through the same one-`beginEditBlock` pending-refactor protocol every
/// other refactoring does.
///
/// The caret sits at (0,0), which is inside `stub_server`'s canned
/// diagnostic (line 0, columns 0-4) — so this also proves
/// `intentions::assemble` dedups the one action both the diagnostic-scoped
/// and the range-scoped request return, rather than listing it twice.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_alt_enter_applies_an_intention_in_one_undo() {
    let name = "e2e_alt_enter_applies_an_intention_in_one_undo";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    let original = fixture_text("tiny", "src/main.rs");
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");

    let mark = ide.mark();
    ide.key("alt+Return");
    let shown = ide.wait_for_event(mark, "the intentions menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "intentions_menu"
    });
    assert_eq!(
        shown["count"].as_u64(),
        Some(1),
        "the stub's one action at (0,0) should not be listed twice"
    );
    ide.key("Down");
    ide.key("Return");
    ide.wait_for_event(mark, "the intentions menu to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "intentions_menu" && e["accepted"] == true
    });
    ide.wait_for_ev(mark, "workspace_edit_applied");
    ide.focus_main();

    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert!(
        buffer(&mcp, tab_id).starts_with("extracted()"),
        "the intention's edit was not applied"
    );

    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the tab to go dirty again", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the undone tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo the intention"
    );

    assert_eq!(ide.quit(), 0);
}

/// F2-3: a resource operation ahead of its text edits, previewed and
/// applied through `RefactorPreviewDialog` exactly like a multi-file
/// rename — creating a file is never a same-file change, so
/// `EditPlan::touches_other_files` is true even though only one *other*
/// file is a resource operation, not a text edit.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_intention_creates_a_file_through_the_preview() {
    let name = "e2e_intention_creates_a_file_through_the_preview";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    assert!(
        !fixture("tiny").join("src/extracted.rs").exists(),
        "the fixture's shape changed"
    );

    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");
    for _ in 0..5 {
        ide.key("Down"); // Line 5 (0-indexed from Ctrl+Home): the closing `}`.
    }

    let mark = ide.mark();
    ide.key("alt+Return");
    ide.wait_for_event(mark, "the intentions menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "intentions_menu"
    });
    ide.key("Down"); // A freshly-opened QMenu highlights nothing on its own.
    ide.key("Return"); // The stub offers exactly one action at this line.
    ide.wait_for_event(mark, "the intentions menu to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "intentions_menu" && e["accepted"] == true
    });

    let rows = ide.wait_for_ev(mark, "preview_rows");
    assert_eq!(
        rows["files"].as_u64(),
        Some(2),
        "the preview should list the created file and the edited one"
    );
    ide.key("Return");
    let applied = ide.wait_for_ev(mark, "workspace_edit_applied");
    assert_eq!(applied["documents"].as_u64(), Some(2));
    ide.focus_main();

    ide.wait_for_event(mark, "the open file's buffer to be spliced", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert!(
        buffer(&mcp, tab_id).ends_with("moved\n") || buffer(&mcp, tab_id).contains("moved"),
        "the original file's edit was not applied"
    );

    e2e::wait_for("the created file to appear on disk", || {
        ide.read_project_file("src/extracted.rs")
            .contains("moved here")
            .then_some(())
    });

    assert_eq!(ide.quit(), 0);
}

/// F3-11/F3-16: reverting a hunk splices the edit back into the buffer —
/// never the file on disk — so the app's own undo stack, not a second write,
/// is what gets a user back to what they had before Revert. Proven two ways:
/// the file on disk keeps whatever was last *saved* right through the
/// revert, and one Ctrl+Z lands exactly on the pre-revert text (round-tripped
/// through the same modification-tracking every other edit uses, not a
/// second, hand-rolled "are these bytes equal" check).
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hunk_revert_is_one_undo_never_touches_disk() {
    const ORIGINAL: &str = "line one\nline two\nline three\n";
    // Lowercase only: `xdotool type`'s Shift for a capital can land on top
    // of a modifier a preceding `xdotool key` chord has not yet released,
    // turning it into an unrelated `Ctrl+Shift+<letter>` shortcut — Search
    // Everywhere and friends all live on that chord (`keymap.rs`).
    const EDITED: &str = "line one edited\nline two\nline three\n";
    let name = "e2e_hunk_revert_is_one_undo_never_touches_disk";

    let repo = git_fixture(&[("notes.txt", ORIGINAL)]);
    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo); // already copied into the app's own temp project root

    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let tab = open_file(&ide, "notes.txt");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // One hunk: append to the first line, entirely in the buffer.
    ide.key("ctrl+Home");
    ide.key("End");
    let mark = ide.mark();
    ide.type_text(" edited");
    ide.wait_for_event(mark, "the tab to go dirty", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    // The gutter's hunk cache is what `vcs.rollbackHunk` reads at the caret;
    // reverting before this fires would find nothing there yet and silently
    // do nothing.
    ide.wait_for_event(mark, "the gutter to see the edit as a hunk", |e| {
        e["ev"] == "vcs_hunks_applied" && e["count"].as_u64().unwrap_or(0) > 0
    });

    // Save, so the disk copy is `EDITED` — the baseline the "never touches
    // disk" assertion below is measured against.
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        EDITED,
        "the edit did not land in the buffer"
    );
    assert_eq!(
        ide.read_project_file("notes.txt"),
        EDITED,
        "the fixture's shape changed"
    );

    // Revert through the VCS menu — no default keyboard shortcut is bound to
    // `vcs.rollbackHunk` (`app-config/src/keymap.rs`), so this drives the
    // same QAction through the menu bar instead. Keyboard-only throughout:
    // no coordinate is computed for a 1px gutter marker.
    let mark = ide.mark();
    ide.key("alt+c"); // "V&CS" — see vcs_menu.cpp for why not Alt+V.
    ide.wait_for_event(mark, "the VCS menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "vcs_menu"
    });
    // Commit, Push, Pull, Fetch, Branches, (separator), Show Diff, Rollback
    // Hunk: unlike a bare `exec()` popup (the tab context menu's own
    // Down-count in `e2e_split_editor_persistence` relies on nothing being
    // highlighted yet), a menu-bar-triggered QMenu pre-highlights its first
    // item the moment it opens — confirmed against a real run under Xvfb,
    // not assumed — so reaching the 7th item takes 6 more Downs, not 7.
    for _ in 0..6 {
        ide.key("Down");
    }
    ide.key("Return");
    ide.wait_for_event(mark, "the VCS menu to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "vcs_menu"
    });
    ide.wait_for_ev(mark, "vcs_hunk_reverted");

    // `read_buffer` cannot referee this step (the module doc comment: it
    // answers from the rope, which only a *save* refreshes — a revert never
    // saves), so the buffer's new content is checked the way the gutter
    // itself would notice: the didChange debounce re-diffs the live buffer
    // against `HEAD` and should now find nothing, which does not depend on
    // a save and says more than "some edit happened".
    ide.wait_for_event(
        mark,
        "the gutter to see no more difference from HEAD",
        |e| e["ev"] == "vcs_hunks_applied" && e["count"].as_u64() == Some(0),
    );
    assert_eq!(
        ide.read_project_file("notes.txt"),
        EDITED,
        "the revert wrote to disk instead of only splicing the buffer"
    );
    ide.wait_for_event(
        mark,
        "the tab to go dirty (buffer no longer matches the saved disk copy)",
        |e| e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true,
    );

    // One Ctrl+Z, not two: the revert went through `beginEditBlock` exactly
    // like every other edit `applyEditsTo` makes.
    ide.key("ctrl+z");
    ide.wait_for_event(
        mark,
        "the tab to go clean again (back to what was saved)",
        |e| e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false,
    );
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        EDITED,
        "one Ctrl+Z did not undo the hunk revert"
    );

    assert_eq!(ide.quit(), 0);
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
    // disk at launch) — so its geometry marker is read from the start of
    // the stream, not from a mark taken here, in case Alt+9 finds it
    // already the visible tab and toggles nothing.
    let mark = ide.mark();
    ide.key("alt+9"); // vcs.view.changes' default shortcut (keymap.rs).
    let shown = ide.wait_for_event(
        Mark::start(),
        "the Changes dock to report its geometry",
        |e| e["ev"] == "changes_panel_shown",
    );

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
    // look again — this is the row that answer produced.
    let row = ide.wait_for_event(mark, "the file to show up as an unstaged change", |e| {
        e["ev"] == "changes_row" && e["path"] == "draft.txt" && e["group"] == "unstaged"
    });

    // Stage it: a real click on the checkbox glyph itself — `Space` on the
    // row once merely current turned out not to toggle it (no default
    // `QAbstractItemView` keyboard binding does that; only clicking the
    // indicator does), confirmed against a real run under Xvfb rather than
    // assumed. The glyph sits a fixed, style-drawn offset in from the row's
    // own left edge, which the row's marked rect gives without this flow
    // computing it from indentation or icon metrics.
    let (checkbox_x, checkbox_y) = checkbox_point(&row["rect"]);
    ide.click_at(checkbox_x, checkbox_y, 1);
    ide.wait_for_event(mark, "the file to move to Staged Changes", |e| {
        e["ev"] == "changes_row" && e["path"] == "draft.txt" && e["group"] == "staged"
    });

    // Type the commit message and click Commit — both rects came from the
    // same `changes_panel_shown` marker taken when the dock was first shown.
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

/// Issue #164 regression: `fileHistory` used to hand `gix` an absolute path
/// (defect B, always-empty history) and `showContextMenu` kept raw
/// `QListWidgetItem*` across `menu.exec()`'s nested loop (defect A, the
/// UAF). The re-entrant clear behind A is unreachable from outside the
/// process here (the popup owns the keyboard grab, so no `xdotool` event
/// can drive a tab switch while it's up) — this proves the count B zeroed,
/// and exercises the fixed `showContextMenu` on the locals it now reads
/// before `exec()`.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_file_history_lists_commits_and_survives_the_context_menu() {
    let name = "e2e_file_history_lists_commits_and_survives_the_context_menu";

    let repo = git_fixture(&[("history.txt", "v1\n"), ("other.txt", "unrelated\n")]);
    // A second commit on history.txt so "lists everything" and "lists
    // nothing" can't look the same.
    std::fs::write(repo.path().join("history.txt"), "v2\n").expect("edit history.txt");
    for args in [
        ["add", "history.txt"].as_slice(),
        &["commit", "--quiet", "-m", "second"],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed");
    }

    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo); // already copied into the app's own temp project root

    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let tab = open_file(&ide, "history.txt");
    let editor_tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // `view.vcsHistory` has no default shortcut; reach it via Find Action,
    // same as `vcs_menu.cpp`'s View entry: shows the dock, calls
    // `setCurrentFile`.
    let mark = open_search_popup(&ide, "ctrl+shift+a");
    accept_top_hit(&ide, mark, "File History");

    // The marker carries the file's absolute path — `VcsService` reports
    // what it was asked about, and the panel passes it through — so match on
    // the name rather than on equality with a relative path the app never
    // produces. (This assertion was comparing against `"history.txt"`, which
    // stopped being what the marker says when #170 fixed the panel; the
    // suite is nightly, so nothing turned red in a PR.)
    let ready = ide.wait_for_event(mark, "history_ready for history.txt", |e| {
        e["ev"] == "history_ready"
            && e["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("history.txt"))
    });
    assert_eq!(
        ready["count"].as_u64(),
        Some(2),
        "history did not list both commits"
    );

    let row0 = ide.wait_for_event(mark, "history_row 0", |e| {
        e["ev"] == "history_row"
            && e["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("history.txt"))
            && e["row"] == 0
    });

    // Select then right-click the newest commit's row to raise the menu.
    let (row_x, row_y) = rect_centre(&row0["rect"]);
    ide.click_at(row_x, row_y, 1);
    let menu_mark = ide.mark();
    ide.click_at(row_x, row_y, 3);
    ide.wait_for_event(menu_mark, "the context menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "file_history_context_menu"
    });
    // One row selected -> one action; Down then Return selects it whether
    // or not `exec()` pre-highlighted it.
    ide.key("Down");
    ide.key("Return");
    ide.wait_for_event(menu_mark, "the context menu to close", |e| {
        e["ev"] == "dialog_closed"
            && e["name"] == "file_history_context_menu"
            && e["accepted"] == true
    });

    // A new diff tab, distinct from the plain editor tab, proves
    // `compareRevisions_` ran on the revision read before `exec()` rather
    // than crashing or reading a dangling item — and its panes have to carry
    // the newest commit's `"v2\n"` and the working tree's `"v2\n"`, not the
    // two empty ones this kept passing on (#210).
    let diff_tab = ide.wait_for_event(menu_mark, "a diff tab for history.txt", |e| {
        e["ev"] == "tab_added" && e["title"] == "history.txt"
    });
    let diff_tab_id = diff_tab["tab_id"].as_u64().expect("tab_id");
    assert_ne!(diff_tab_id, editor_tab_id, "no new tab opened");
    ide.assert_diff_panes(menu_mark, diff_tab_id, 3, 3);

    assert_eq!(ide.quit(), 0);
}

/// F0-10/ADR-0022, driven end to end rather than only through
/// `settings-model`'s unit tests (#143): switch the Settings dialog's
/// scope to "This project", change the tab width, OK, and check that
/// exactly one field reached `<project>/.ide/settings.toml` — not the
/// whole `[editing]` section re-serialised, and not the global file. Then
/// a cold relaunch picks the override back up.
///
/// This is also the regression test for the bug walking this checklist by
/// hand actually found: the origin badge (`AppSettings::fieldOrigin`) used
/// to answer "which layer does the *effective* value come from" regardless
/// of which layer's page was open, so the Global page showed "from
/// project" over its own, unrelated value the moment any project override
/// existed anywhere. `settings-model::scope`'s own unit tests
/// (`viewing_global_never_reports_a_project_override_that_is_not_on_screen`)
/// cover the fix directly; this flow is the proof the dialog really wires
/// the corrected function in, through the real Qt widgets.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_project_scope_settings_write_only_what_changed() {
    let mut ide = Ide::launch(
        "e2e_project_scope_settings_write_only_what_changed",
        APP,
        fixture("tiny"),
    );
    ide.wait_for_ev(Mark::start(), "project_opened");

    let mark = ide.mark();
    ide.key("ctrl+comma");
    let shown = ide.wait_for_event(mark, "the Settings dialog to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "settings_dialog"
    });

    // "Editing" in the category list, then the scope combo: Down selects
    // "This project" (index 1) from whichever item — "All projects
    // (global)" — a freshly launched dialog starts on.
    let (category_x, category_y) = rect_centre(&shown["editing_category_rect"]);
    ide.click_at(category_x, category_y, 1);

    let (scope_x, scope_y) = rect_centre(&shown["scope_rect"]);
    ide.click_at(scope_x, scope_y, 1);
    ide.key("Down");
    ide.key("Return");
    let switched = ide.wait_for_event(mark, "the Editing page to rebuild for project scope", |e| {
        e["ev"] == "settings_scope_switched"
    });

    // A `QSpinBox`'s own up/down arrows are too narrow a target to trust
    // without the width `settings_dialog.cpp`'s own comment explains is
    // not settled yet at this point — clicking near the field's left edge
    // and editing with the keyboard needs only its (already-settled)
    // top-left corner.
    let top_left = switched["tab_width_top_left"]
        .as_array()
        .expect("tab_width_top_left is a [x, y] pair");
    let field_x = top_left[0].as_i64().unwrap() as i32 + 10;
    let field_y = top_left[1].as_i64().unwrap() as i32 + 10;
    ide.click_at(field_x, field_y, 1);
    ide.key("ctrl+a");
    ide.type_text("2");
    ide.key("Tab");

    let (ok_x, ok_y) = rect_centre(&shown["ok_rect"]);
    ide.click_at(ok_x, ok_y, 1);
    ide.wait_for_event(mark, "the dialog to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "settings_dialog" && e["accepted"] == true
    });
    ide.focus_main();

    assert_eq!(ide.quit(), 0);

    // The app's own types, never a regex (this file's own rule) — and the
    // whole point of the check: only `tab_width` is a real value, the
    // rest stay at the "inherit" sentinel (`app_config::editing`'s own
    // empty-means-unset convention), because nothing else was touched.
    let root = ide.project_root().to_path_buf();
    let settings = app_config::project_settings::load(&root).expect("settings just committed");
    let editing = settings
        .editing
        .expect("the dialog wrote an [editing] override");
    assert_eq!(editing.tab_width, 2);
    assert_eq!(editing.use_spaces, None);
    assert_eq!(editing.trim_trailing_whitespace, None);
    assert_eq!(editing.insert_final_newline, None);

    // A cold second process reads the override straight back — the same
    // relaunch-and-reread proof `e2e_run_config_dialog_persists_across_relaunch`
    // gives its own persisted state.
    ide.relaunch();
    ide.wait_for_ev(Mark::start(), "project_opened");
    let reloaded = app_config::project_settings::load(ide.project_root())
        .expect("settings survive a cold relaunch");
    assert_eq!(reloaded.editing.expect("still overridden").tab_width, 2);

    assert_eq!(ide.quit(), 0);
}
/// The Preview dock (ADR-0033): opens a Markdown file, toggles the dock,
/// waits for the first render, edits the buffer, and waits for a second,
/// later revision — proving the debounce fires and a stale render is not
/// what the dock ends up showing. Every assertion is against the marker
/// stream, never a screenshot, per this file's own rule.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_markdown_preview_dock() {
    let name = "e2e_markdown_preview_dock";
    let mut ide = Ide::launch(name, APP, fixture("markdown"));
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let tab = open_file(&ide, "demo.md");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    let mark = ide.mark();
    ide.key("ctrl+alt+v");
    let first = ide.wait_for_event(mark, "the first preview render", |e| {
        e["ev"] == "preview_ready" && e["tab_id"].as_u64() == Some(tab_id)
    });
    let first_revision = first["revision"].as_u64().expect("revision");

    // One character is enough: the debounce timer only cares that
    // `contentsChanged` fired, not how much changed.
    let mark = ide.mark();
    ide.type_text("x");
    let second = ide.wait_for_event(mark, "a later preview revision", |e| {
        e["ev"] == "preview_ready" && e["tab_id"].as_u64() == Some(tab_id)
    });
    assert!(
        second["revision"].as_u64().expect("revision") > first_revision,
        "the edit must produce a strictly later revision, not a repeat of the first"
    );

    // Undo the one-character edit rather than leaving the tab dirty: a
    // dirty tab makes Ctrl+Q pop the "save before closing?" dialog, which
    // this test has no business asserting on.
    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the undo to clean the tab", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });

    assert_eq!(ide.quit(), 0);
}

/// The `ui_locale` setting (ADR-0049): a cold-launched process with no
/// `settings.toml` reports the "en" default, and a `settings.toml` seeded
/// with `ui_locale = "de"` before a relaunch takes effect — both read
/// through the marker `installUiTranslators` emits at startup, since a
/// German button label is not something this suite screenshots. That
/// translator install (and, for "de", the `.qm` it loads) is what this test
/// actually exercises; spot-checking translated label text in the running
/// window is not automated here, see the PR description for what manual
/// verification covered instead.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_ui_locale_setting_takes_effect_on_relaunch() {
    let name = "e2e_ui_locale_setting_takes_effect_on_relaunch";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    let active = ide.wait_for_ev(Mark::start(), "ui_locale_active");
    assert_eq!(
        active["locale"], "en",
        "default locale before any setting is saved"
    );
    assert_eq!(ide.quit(), 0);

    let mut settings =
        app_config::load(&ide.config_dir()).expect("settings written by the first launch");
    settings.ui_locale = "de".to_string();
    app_config::save(&ide.config_dir(), &settings).expect("seeding ui_locale = de");

    ide.relaunch();
    let active = ide.wait_for_ev(Mark::start(), "ui_locale_active");
    assert_eq!(active["locale"], "de");
    ide.wait_for_ev(Mark::start(), "project_opened");

    assert_eq!(ide.quit(), 0);
}
