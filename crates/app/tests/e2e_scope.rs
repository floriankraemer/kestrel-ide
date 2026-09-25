//! ADR-0064: `.gitignore` no longer decides what the IDE can find — Go to
//! File still reaches a gitignored build artifact and a dotfile nested
//! under a dot-directory.
//!
//! A separate file rather than a new test in `e2e.rs`: that file is at its
//! own size baseline (`scripts/check-file-size.sh`) and this needs its own
//! small copies of `git_fixture`/`open_search_popup`/`accept_top_hit`
//! /`wait_for_index`, the same duplication `e2e_lazy_tree.rs` already
//! accepts — each E2E test binary is its own crate, so nothing here is
//! importable from `e2e.rs`.

use e2e::{mcp::Mcp, Ide, Mark};
use serde_json::json;

const APP: &str = env!("CARGO_BIN_EXE_app");

fn wait_for_index(mcp: &Mcp) {
    e2e::wait_for("the project index to finish building", || {
        (mcp.call("index_status", json!({}))["ready"] == true).then_some(())
    });
}

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

/// A fresh temp directory holding `files`, committed to a brand-new Git
/// repository — see `e2e.rs`'s own `git_fixture` for why the commit has to
/// exist before `Ide::launch` ever spawns the app.
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
    git(&["config", "user.email", "e2e@example.invalid"]);
    git(&["config", "user.name", "E2E"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "initial"]);
    dir
}

/// The centre of a `[x, y, w, h]` marker field, so a flow never computes a
/// click point from window geometry or font metrics. Duplicated from
/// `e2e_vcs.rs`, the same per-binary judgement this file's own module doc
/// already makes about `git_fixture`.
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

/// Right-click `folder`'s row in the project tree, click the context-menu
/// action labelled `label`, and wait for the menu to have closed having
/// accepted it. Collapsed from `e2e_vcs.rs`'s `open_tree_git_submenu` +
/// `click_labelled_action` pair into one call: the exclusion actions are
/// top-level entries, with no submenu to hover into first.
fn click_tree_menu_action(
    ide: &Ide,
    settle_file: &str,
    folder_path: &std::path::Path,
    label: &str,
) {
    // Forces a fresh tree layout report the same way `e2e_vcs.rs`'s
    // `open_tree_git_submenu` does — a row's rect from the tree's very
    // first layout pass is stale by the time the dock has its final size.
    let mark = ide.mark();
    std::fs::write(ide.project_root().join(settle_file), "settle\n")
        .expect("writing a file to force a fresh tree layout report");
    let folder_path_str = folder_path.to_string_lossy().into_owned();
    let row = ide.wait_for_event(
        mark,
        &format!("a settled tree row for {folder_path_str}"),
        |e| e["ev"] == "project_tree_row" && e["path"] == folder_path_str,
    );

    let (row_x, row_y) = rect_centre(&row["rect"]);
    ide.focus_main();
    ide.click_at(row_x, row_y, 1);
    ide.click_at(row_x, row_y, 3);
    ide.wait_for_event(mark, "the tree context menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "project_tree_context_menu"
    });

    let action = ide.wait_for_event(mark, &format!("{label} in the tree menu"), |e| {
        e["ev"] == "project_tree_menu_action" && e["label"] == label
    });
    let (action_x, action_y) = rect_centre(&action["rect"]);
    ide.click_at(action_x, action_y, 1);
    ide.wait_for_event(mark, "the tree context menu to close", |e| {
        e["ev"] == "dialog_closed"
            && e["name"] == "project_tree_context_menu"
            && e["accepted"] == true
    });
}

/// `find_files`, retried: a rescope (ADR-0064) flips the index briefly to
/// "not ready" while it reopens against the new scope
/// (`SearchModel::openIndex`'s delta reopen), and `Mcp::call` would panic
/// on that transient RPC error — `try_call` plus `e2e::wait_for` is the
/// same "wait for a transition, never a duration" rule `wait_for_index`
/// already follows, just tolerant of the index being briefly unavailable
/// mid-poll rather than only "not yet ready the first time".
fn find_files(mcp: &Mcp, query: &str) -> Vec<String> {
    let result = e2e::wait_for("the index to answer find_files", || {
        mcp.try_call("find_files", json!({"query": query})).ok()
    });
    result["files"]
        .as_array()
        .expect("find_files returns a files array")
        .iter()
        .filter_map(|file| file["path"].as_str().map(str::to_string))
        .collect()
}

/// Marking a folder Excluded (ADR-0064) drops every file under it from the
/// index — Go to File (Search Everywhere) can no longer find it — and
/// Cancel Exclusion brings it back. Drives the whole seam: the tree
/// context menu → `ProjectTreeModel::toggleExcluded` → `.ide/settings.toml`
/// → rescope → `SearchModel::openIndex`'s delta reopen.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_marking_a_folder_excluded_drops_it_from_search_and_cancelling_restores_it() {
    let workspace = git_fixture(&[
        ("README.md", "root\n"),
        ("lib/Widget.php", "<?php\nclass Widget {}\n"),
    ]);

    let name = "e2e_marking_a_folder_excluded_drops_it_from_search_and_cancelling_restores_it";
    // `Ide::launch` copies the fixture into its own fresh project root
    // (`crates/e2e/src/lib.rs`'s `launch_with_env`) rather than running the
    // app straight out of `workspace` — every path the flow drives against
    // (the tree row, `.ide/settings.toml`) has to be `ide.project_root()`'s,
    // not the fixture's own, or the tree row wait times out.
    let mut ide = Ide::launch(name, APP, workspace.path());
    drop(workspace);
    let lib_path = ide.project_root().join("lib");
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    assert!(
        find_files(&mcp, "Widget.php")
            .iter()
            .any(|path| path.ends_with("Widget.php")),
        "Widget.php should be found before lib/ is excluded"
    );

    click_tree_menu_action(
        &ide,
        "zzz-settle-1.txt",
        &lib_path,
        "Mark Directory as Excluded",
    );

    let settings = std::fs::read_to_string(ide.project_root().join(".ide/settings.toml"))
        .expect("reading .ide/settings.toml after excluding lib/");
    assert!(
        settings.contains("excluded") && settings.contains("lib"),
        ".ide/settings.toml should record lib/ as excluded, got:\n{settings}"
    );

    let after_exclude = e2e::wait_for("Widget.php to drop out of the index", || {
        let files = find_files(&mcp, "Widget.php");
        (!files.iter().any(|path| path.ends_with("Widget.php"))).then_some(files)
    });
    assert!(
        after_exclude.is_empty(),
        "excluding lib/ should drop Widget.php from find_files, got {after_exclude:?}"
    );

    click_tree_menu_action(&ide, "zzz-settle-2.txt", &lib_path, "Cancel Exclusion");

    let after_cancel = e2e::wait_for("Widget.php to come back into the index", || {
        let files = find_files(&mcp, "Widget.php");
        files
            .iter()
            .any(|path| path.ends_with("Widget.php"))
            .then_some(files)
    });
    assert!(after_cancel.iter().any(|path| path.ends_with("Widget.php")));

    assert_eq!(ide.quit(), 0);
}

/// `dist/app.js` is a `.gitignore`d build artifact and `.github/workflows/
/// ci.yml` a dotfile nested under a dot-directory — under the old
/// `.gitignore`/hidden-file rules neither was ever reachable by Go to File.
/// ADR-0064 stopped reading `.gitignore` for project scope at all, so both
/// are now found by name, same as any other file.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_go_to_file_finds_a_gitignored_file_and_a_dotdir_file() {
    let workspace = git_fixture(&[
        (".gitignore", "dist/\n"),
        ("README.md", "root\n"),
        ("dist/app.js", "console.log('bundled');\n"),
        (".github/workflows/ci.yml", "name: CI\n"),
    ]);

    let name = "e2e_go_to_file_finds_a_gitignored_file_and_a_dotdir_file";
    let mut ide = Ide::launch(name, APP, workspace.path());
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let mark = open_search_popup(&ide, "ctrl+shift+n");
    accept_top_hit(&ide, mark, "app.js");
    ide.wait_for_event(mark, "a tab for the gitignored file", |e| {
        e["ev"] == "tab_added" && e["title"] == "app.js"
    });

    let mark = open_search_popup(&ide, "ctrl+shift+n");
    accept_top_hit(&ide, mark, "ci.yml");
    ide.wait_for_event(mark, "a tab for the dotdir file", |e| {
        e["ev"] == "tab_added" && e["title"] == "ci.yml"
    });

    assert_eq!(ide.quit(), 0);
}

/// T5: adding `generated` to the global Ignored Names list through the
/// Project Scope settings page drops a `generated/marker.txt` file out of
/// the index — the settings-page mirror of T4's own tree-action test.
///
/// The `mcp` handle taken before Settings opens is reused after OK on
/// purpose: OK must not restart an MCP server whose settings did not
/// change (a restart re-binds and re-tokens, cutting off connected
/// clients), so a stale handle here fails with `ECONNREFUSED` if that
/// regresses.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_adding_generated_to_ignored_names_through_settings_drops_it_from_the_index() {
    let workspace = git_fixture(&[("README.md", "root\n"), ("generated/marker.txt", "built\n")]);

    let name = "e2e_adding_generated_to_ignored_names_through_settings_drops_it_from_the_index";
    let mut ide = Ide::launch(name, APP, workspace.path());
    drop(workspace);
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    assert!(
        find_files(&mcp, "marker.txt")
            .iter()
            .any(|path| path.ends_with("marker.txt")),
        "generated/marker.txt should be found before generated is ignored"
    );

    let mark = ide.mark();
    ide.key("ctrl+comma");
    let shown = ide.wait_for_event(mark, "the Settings dialog to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "settings_dialog"
    });
    let (cx, cy) = rect_centre(&shown["project_scope_category_rect"]);
    ide.click_at(cx, cy, 1);
    let page = ide.wait_for_event(mark, "the Project Scope page's own rects", |e| {
        e["ev"] == "project_scope_page_shown"
    });

    let (input_x, input_y) = rect_centre(&page["ignored_input_rect"]);
    ide.click_at(input_x, input_y, 1);
    ide.type_text("generated");
    let (add_x, add_y) = rect_centre(&page["add_ignored_rect"]);
    ide.click_at(add_x, add_y, 1);

    let (ok_x, ok_y) = rect_centre(&shown["ok_rect"]);
    ide.click_at(ok_x, ok_y, 1);
    ide.wait_for_event(mark, "the dialog to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "settings_dialog" && e["accepted"] == true
    });
    ide.focus_main();

    let settings = e2e::wait_for("settings.toml to record generated", || {
        std::fs::read_to_string(ide.config_dir().join("settings.toml"))
            .ok()
            .filter(|text| text.contains("generated"))
    });
    assert!(
        settings.contains("generated"),
        "settings.toml should record generated as ignored, got:\n{settings}"
    );

    let after_ok = e2e::wait_for("marker.txt to drop out of the index", || {
        let files = find_files(&mcp, "marker.txt");
        (!files.iter().any(|path| path.ends_with("marker.txt"))).then_some(files)
    });
    assert!(
        after_ok.is_empty(),
        "ignoring generated should drop marker.txt from find_files, got {after_ok:?}"
    );

    assert_eq!(ide.quit(), 0);
}

/// T5 review follow-up: Cancel must actually discard the draft. Adds
/// `generated` to Ignored Names through the page (as the test above does)
/// but closes with Cancel (`Escape`) instead of OK — `settings.toml` must
/// not record it, and `generated/marker.txt` must still be in the index,
/// proving nothing was ever written or rescoped.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_cancelling_project_scope_settings_discards_the_draft() {
    let workspace = git_fixture(&[("README.md", "root\n"), ("generated/marker.txt", "built\n")]);

    let name = "e2e_cancelling_project_scope_settings_discards_the_draft";
    let mut ide = Ide::launch(name, APP, workspace.path());
    drop(workspace);
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    let mark = ide.mark();
    ide.key("ctrl+comma");
    let shown = ide.wait_for_event(mark, "the Settings dialog to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "settings_dialog"
    });
    let (cx, cy) = rect_centre(&shown["project_scope_category_rect"]);
    ide.click_at(cx, cy, 1);
    let page = ide.wait_for_event(mark, "the Project Scope page's own rects", |e| {
        e["ev"] == "project_scope_page_shown"
    });

    let (input_x, input_y) = rect_centre(&page["ignored_input_rect"]);
    ide.click_at(input_x, input_y, 1);
    ide.type_text("generated");
    let (add_x, add_y) = rect_centre(&page["add_ignored_rect"]);
    ide.click_at(add_x, add_y, 1);

    ide.key("Escape");
    ide.wait_for_event(mark, "the dialog to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "settings_dialog" && e["accepted"] == false
    });
    ide.focus_main();

    let settings =
        std::fs::read_to_string(ide.config_dir().join("settings.toml")).unwrap_or_default();
    assert!(
        !settings.contains("generated"),
        "Cancel should not have written generated to settings.toml, got:\n{settings}"
    );
    assert!(
        find_files(&mcp, "marker.txt")
            .iter()
            .any(|path| path.ends_with("marker.txt")),
        "Cancel should leave the index untouched — marker.txt should still be found"
    );

    assert_eq!(ide.quit(), 0);
}
