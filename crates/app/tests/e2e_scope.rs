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
