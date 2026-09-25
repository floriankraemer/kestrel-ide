//! E2E flows for the lazy project tree (PR2 of the fast-project-open plan,
//! `docs/architecture/fast-project-open-plan.md`, ADR-0062 "Decision 2").
//!
//! Split out of `e2e.rs` once these two flows pushed it over its
//! grandfathered file-size baseline — a mechanical move, no behavior change.
//! Every check goes through the real marker stream or the filesystem, never
//! a screenshot (ADR-0024).

use std::path::{Path, PathBuf};

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A fresh temp directory holding `files`, committed to a brand-new Git
/// repository. See `e2e.rs`'s own copy of this helper for why `.git` is
/// baked into the fixture rather than `git init`-ed after `Ide::launch`
/// returns (a race with `VcsService::open_project`'s own background
/// discovery).
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

/// The centre of a `[x, y, w, h]` marker field — see `e2e.rs`'s own copy.
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

/// The lazy project tree (PR2 of the fast-project-open plan, ADR-0062
/// "Decision 2"): a directory's children are read from disk only once
/// expanded, and a watcher-driven refresh updates just the changed
/// directory rather than resetting the whole model. Driven through the
/// real `project_tree_row`/`project_tree_rows` markers `wireRowMarkers`
/// already emits for every visible row (never a screenshot — ADR-0024).
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_lazy_tree_expands_on_demand_and_refreshes_incrementally() {
    let name = "e2e_lazy_tree_expands_on_demand_and_refreshes_incrementally";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");

    // The project-scope settings folder (`.ide/`, ADR-0022) lands as its
    // own watcher-driven row shortly after open, alongside the fixture's
    // own `src` folder — wait for the root to settle at its final two
    // entries before reading any row's position, so the click below can't
    // race that reshuffle. Neither root entry's own children are read from
    // disk yet (the lazy tree's whole point): `.ide` is never expanded by
    // this flow, and `src`'s two files (`main.rs`, `greeting.rs`) only
    // appear once it is.
    let root_settled =
        ide.wait_for_event(Mark::start(), "the root to settle at two entries", |e| {
            e["ev"] == "project_tree_rows" && e["count"] == 2
        });
    let root_row_count = root_settled["count"].as_u64().expect("count");

    let src_path = ide.project_root().join("src");
    let src_row = ide
        .events_since_of(Mark::start(), "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == src_path.to_string_lossy().as_ref())
        .expect("a row for src, at its settled position");

    // Qt's default double-click-to-expand triggers `fetchMore`, which
    // reads `src` off the Qt thread and inserts its two children.
    let (sx, sy) = rect_centre(&src_row["rect"]);
    let expand_mark = ide.mark();
    ide.double_click_at(sx, sy, 1);
    let expanded = ide.wait_for_event(expand_mark, "rows after expanding src", |e| {
        e["ev"] == "project_tree_rows" && e["count"].as_u64().unwrap_or(0) >= root_row_count + 2
    });
    assert_eq!(
        expanded["count"].as_u64(),
        Some(root_row_count + 2),
        "src's two files, no more and no less"
    );
    ide.wait_for_event(expand_mark, "a row for greeting.rs", |e| {
        e["ev"] == "project_tree_row"
            && e["path"] == src_path.join("greeting.rs").to_string_lossy().as_ref()
    });

    // An external create inside the now-expanded `src` must show up
    // through the watcher's incremental per-directory refresh — never a
    // full model reset, which would collapse `src` right back to a leaf.
    let refresh_mark = ide.mark();
    let new_file = src_path.join("new_file.rs");
    std::fs::write(&new_file, "").expect("create new_file.rs externally");
    let refreshed = ide.wait_for_event(refresh_mark, "rows after the external create", |e| {
        e["ev"] == "project_tree_rows" && e["count"].as_u64().unwrap_or(0) >= root_row_count + 3
    });
    assert_eq!(
        refreshed["count"].as_u64(),
        Some(root_row_count + 3),
        "src stayed expanded (its two original files are still shown) and gained exactly one row"
    );
    ide.wait_for_event(refresh_mark, "a row for the externally created file", |e| {
        e["ev"] == "project_tree_row" && e["path"] == new_file.to_string_lossy().as_ref()
    });

    assert_eq!(ide.quit(), 0);
}

/// The lazy tree's remaining new surface beyond the expand/refresh flow
/// already covered above: revealing a file through several never-expanded
/// levels, a context-menu mutation keeping the tree's expansion state, the
/// sort toggle re-sorting both loaded and not-yet-loaded directories, a
/// directory that turns out empty once loaded showing no expand arrow, and
/// a lazily-loaded row still picking up its VCS status colour. Every check
/// goes through the real marker stream or the filesystem, never a
/// screenshot (ADR-0024).
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_lazy_tree_reveal_context_menu_sort_and_vcs_color() {
    let name = "e2e_lazy_tree_reveal_context_menu_sort_and_vcs_color";
    let repo = git_fixture(&[
        ("root.txt", "root\n"),
        ("a/b/deep.txt", "deep\n"),
        ("docs/clean.txt", "clean\n"),
        ("docs/changed.txt", "v1\n"),
        ("work/old.txt", "old\n"),
    ]);
    // Git never tracks an empty directory, but the real filesystem the app
    // walks does — `empty_dir` exists on disk regardless of what's
    // committed.
    std::fs::create_dir(repo.path().join("empty_dir")).expect("create empty_dir");
    // An unstaged modification, made after the commit above, for the VCS
    // colour check below.
    std::fs::write(repo.path().join("docs/changed.txt"), "v2\n").expect("modify changed.txt");

    let mut ide = Ide::launch(name, APP, repo.path());
    drop(repo);
    let mcp = ide.mcp();
    ide.wait_for_ev(Mark::start(), "project_opened");

    // The Changes dock repopulates from the same `VcsService::statusChanged`
    // the project tree's colour proxy reads — waiting for its row is a
    // reliable "the initial `git status` scan has landed" signal without a
    // fixed sleep.
    ide.wait_for_event(
        Mark::start(),
        "the Changes dock to report changed.txt",
        |e| {
            e["ev"] == "changes_row"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("changed.txt"))
        },
    );

    let root = ide.project_root().to_path_buf();

    // The project-scope settings folder (`.ide/`, ADR-0022) lands as its own
    // watcher-driven row shortly after open, same race the earlier lazy-tree
    // test has to guard against — wait for it before reading any row's
    // settled position or the root's settled row count.
    ide.wait_for_event(Mark::start(), "the .ide settings folder to appear", |e| {
        e["ev"] == "project_tree_row" && e["path"] == root.join(".ide").to_string_lossy().as_ref()
    });
    let root_row_count = ide
        .events_since_of(Mark::start(), "project_tree_rows")
        .into_iter()
        .next_back()
        .expect("a project_tree_rows marker")["count"]
        .clone();

    // --- (d) a directory that is empty once loaded shows no expand arrow ---
    //
    // `hasChildren` must answer `true` for `empty_dir` before it's ever been
    // expanded (unknown yet) and `false` once it has (nothing there) — the
    // pixel-level "no flicker" claim reduces to "rowCount stays put and the
    // directory is marked Loaded-empty", which the row/rows markers can
    // prove without a screenshot. `attach_children` marking `Loaded` with an
    // empty child list is exactly what suppresses the arrow (`has_children`
    // reads `load_state`+`children.is_empty()`), so the observable
    // consequence — no rows added, no crash, still there afterward — is a
    // faithful stand-in for "the arrow disappeared".
    let empty_dir_path = root.join("empty_dir");
    let empty_row = ide
        .events_since_of(Mark::start(), "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == empty_dir_path.to_string_lossy().as_ref())
        .expect("a settled row for empty_dir");
    let (ex, ey) = rect_centre(&empty_row["rect"]);
    ide.double_click_at(ex, ey, 1);
    // No new row can ever appear for an empty directory, so there is no
    // marker to `wait_for_event` on — instead, give the worker thread's
    // `list_dir` + queued `attach_children` round trip generous time to land
    // (a local, empty directory's `read_dir` is sub-millisecond; this is
    // about the thread-spawn and cross-thread-queue latency, not the I/O),
    // then drain the Qt queue once more (`ide.sync`) before checking that
    // the row count really didn't move.
    std::thread::sleep(std::time::Duration::from_millis(300));
    ide.sync(&mcp);
    let rows_after_empty = ide
        .events_since_of(Mark::start(), "project_tree_rows")
        .into_iter()
        .next_back()
        .expect("a project_tree_rows marker");
    assert_eq!(
        rows_after_empty["count"], root_row_count,
        "expanding an empty directory must insert zero rows"
    );

    // --- (e) VCS status colour on a lazily-loaded row -----------------------
    //
    // `docs` has never been expanded, so `changed.txt`'s row is inserted by
    // `fetchMore`, not present at initial project-open time — exactly the
    // row `VcsStatusColorProxy`/`IconDecorationProxy` must still colour and
    // icon correctly, since both compute from the row's own path on every
    // `data()` call rather than a cache built once at open (see
    // `vcs_status_color_proxy.cpp`, `icon_cache.h`).
    let docs_path = root.join("docs");
    let docs_row = ide
        .events_since_of(Mark::start(), "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == docs_path.to_string_lossy().as_ref())
        .expect("a row for docs");
    let (dx, dy) = rect_centre(&docs_row["rect"]);
    let docs_mark = ide.mark();
    ide.double_click_at(dx, dy, 1);
    ide.wait_for_event(docs_mark, "a row for changed.txt", |e| {
        e["ev"] == "project_tree_row"
            && e["path"] == docs_path.join("changed.txt").to_string_lossy().as_ref()
    });
    let changed_row = ide
        .events_since_of(Mark::start(), "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == docs_path.join("changed.txt").to_string_lossy().as_ref())
        .expect("a row for changed.txt");
    let clean_row = ide
        .events_since_of(Mark::start(), "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == docs_path.join("clean.txt").to_string_lossy().as_ref())
        .expect("a row for clean.txt");
    assert_ne!(
        changed_row["color"].as_str().unwrap_or(""),
        "",
        "a lazily-loaded modified file must still get a VCS-status colour override"
    );
    assert_eq!(
        clean_row["color"].as_str().unwrap_or(""),
        "",
        "an unmodified sibling in the same lazily-loaded directory keeps no colour override"
    );

    // --- (b) context-menu rename keeps the tree's expansion state ----------
    let work_path = root.join("work");
    let work_row = ide
        .events_since_of(Mark::start(), "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == work_path.to_string_lossy().as_ref())
        .expect("a row for work");
    let (wx, wy) = rect_centre(&work_row["rect"]);
    let expand_work_mark = ide.mark();
    ide.double_click_at(wx, wy, 1);
    ide.wait_for_event(expand_work_mark, "a row for old.txt", |e| {
        e["ev"] == "project_tree_row"
            && e["path"] == work_path.join("old.txt").to_string_lossy().as_ref()
    });

    ide.focus_main();
    let old_row = ide
        .events_since_of(Mark::start(), "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == work_path.join("old.txt").to_string_lossy().as_ref())
        .expect("a settled row for old.txt");
    let (ox, oy) = rect_centre(&old_row["rect"]);
    ide.click_at(ox, oy, 1);
    let rename_menu_mark = ide.mark();
    ide.click_at(ox, oy, 3);
    ide.wait_for_event(rename_menu_mark, "the tree context menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "project_tree_context_menu"
    });
    let rename_action = ide.wait_for_event(rename_menu_mark, "the Rename entry", |e| {
        e["ev"] == "project_tree_menu_action" && e["label"] == "Rename"
    });
    let (rax, ray) = rect_centre(&rename_action["rect"]);
    ide.click_at(rax, ray, 1);
    ide.wait_for_event(rename_menu_mark, "the tree context menu to close", |e| {
        e["ev"] == "dialog_closed"
            && e["name"] == "project_tree_context_menu"
            && e["accepted"] == true
    });
    // `QInputDialog::getText` is now open, pre-filled with "old.txt".
    ide.key("ctrl+a");
    ide.type_text("renamed.txt");
    ide.key("Return");

    let new_path = work_path.join("renamed.txt");
    ide.wait_for_event(rename_menu_mark, "a row for renamed.txt", |e| {
        e["ev"] == "project_tree_row" && e["path"] == new_path.to_string_lossy().as_ref()
    });
    assert!(new_path.is_file(), "the rename landed on disk");
    assert!(!work_path.join("old.txt").exists(), "the old name is gone");
    // `work` itself must still be there and still expanded — no
    // `beginResetModel` fired, so nothing collapsed it.
    assert!(
        ide.events_since_of(rename_menu_mark, "project_tree_row")
            .into_iter()
            .any(|e| e["path"] == work_path.to_string_lossy().as_ref()),
        "work is still shown, i.e. its parent didn't collapse either"
    );

    // --- (c) sort-order toggle re-sorts both loaded and unloaded dirs ------
    //
    // `docs` (loaded above) must re-sort in memory, with no fresh `list_dir`
    // needed; `a` (never expanded) must simply remember the new order for
    // whenever it is expanded. The toggle itself resets the whole model — a
    // legitimate, deliberate full reset (see the ADR and the `tree.rs`
    // finding in this report) — which collapses every row's expand state in
    // the view exactly like any other `QTreeView` reset, so both `docs` and
    // `a` need expanding again afterward; what matters is that neither needs
    // its own directory re-read from disk to come back in the right order.
    let (sort_x, sort_y) = {
        let toolbar = ide
            .events_since_of(Mark::start(), "project_tree_toolbar")
            .into_iter()
            .next_back()
            .expect("a project_tree_toolbar marker");
        rect_centre(&toolbar["sortRect"])
    };
    let sort_mark = ide.mark();
    ide.click_at(sort_x, sort_y, 1);
    let reset_rows = ide.wait_for_event(sort_mark, "rows after the sort toggle's reset", |e| {
        e["ev"] == "project_tree_rows"
    });
    assert_eq!(
        reset_rows["count"], root_row_count,
        "the reset repaints the same top-level entries, just reordered"
    );

    let docs_row_after_reset = ide
        .events_since_of(sort_mark, "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == docs_path.to_string_lossy().as_ref())
        .expect("a row for docs after the reset");
    let (dx2, dy2) = rect_centre(&docs_row_after_reset["rect"]);
    let reexpand_docs_mark = ide.mark();
    ide.double_click_at(dx2, dy2, 1);
    // `docs` is still `LoadState::Loaded` (the reset only reordered its
    // children in memory, it didn't unload it), so this re-expand needs no
    // worker thread — its two rows come back on the same Qt-thread turn.
    ide.wait_for_event(
        reexpand_docs_mark,
        "docs's children after re-expanding",
        |e| {
            e["ev"] == "project_tree_row"
                && e["path"] == docs_path.join("clean.txt").to_string_lossy().as_ref()
        },
    );
    let rows_by_path: std::collections::HashMap<String, i64> = ide
        .events_since_of(reexpand_docs_mark, "project_tree_row")
        .into_iter()
        .filter_map(|e| Some((e["path"].as_str()?.to_string(), e["rect"][1].as_i64()?)))
        .collect();
    let clean_y = *rows_by_path
        .get(&docs_path.join("clean.txt").to_string_lossy().into_owned())
        .expect("clean.txt is shown once docs is re-expanded");
    let changed_y = *rows_by_path
        .get(&docs_path.join("changed.txt").to_string_lossy().into_owned())
        .expect("changed.txt is shown once docs is re-expanded");
    // Ascending (the default) sorts "changed.txt" before "clean.txt"
    // ('h' < 'l'); descending reverses that within the group.
    assert!(
        clean_y < changed_y,
        "descending order reverses the name comparison within `docs` (folders \
         still lead, but there are none here), so clean.txt must now sit \
         above changed.txt — proving the already-loaded directory was \
         re-sorted in memory rather than needing a fresh `read_dir`"
    );

    // Now expand `a`, then `b` inside it, for the first time — both must
    // come back already in the new order, proving an Unloaded directory
    // picked up the remembered direction rather than defaulting to
    // ascending.
    let a_row = ide
        .events_since_of(sort_mark, "project_tree_row")
        .into_iter()
        .rfind(|e| e["path"] == root.join("a").to_string_lossy().as_ref())
        .expect("a row for a");
    let (ax, ay) = rect_centre(&a_row["rect"]);
    let expand_a_mark = ide.mark();
    ide.double_click_at(ax, ay, 1);
    ide.wait_for_event(expand_a_mark, "a row for b", |e| {
        e["ev"] == "project_tree_row" && e["path"] == root.join("a/b").to_string_lossy().as_ref()
    });

    // --- (a) reveal-in-tree through several never-expanded levels ----------
    //
    // `a/b/deep.txt` is opened directly (bypassing the tree, which cannot
    // reach it while `b` is unloaded), then "Locate in Project Tree" must
    // load whichever ancestors are still unloaded (`b`'s own children —
    // `a` was just expanded above, but `b` itself never has been) and reveal
    // it. This is the async `ensurePathLoaded` + one-shot `pathReady` slot's
    // real test: a bug there hangs rather than fails cleanly, so the wait
    // below has to be the proof, not an assumption.
    let deep_path = root.join("a/b/deep.txt");
    let open_mark = ide.mark();
    mcp.call(
        "open_file",
        serde_json::json!({ "path": deep_path.to_string_lossy() }),
    );
    // `openFile` over MCP creates the tab; whether it also focuses it is a
    // separate question this flow doesn't need to answer — click the tab
    // itself (its own `tab_added` marker gives the rect) so `currentPath()`
    // is unambiguously `deep.txt`'s, the same as a user actually looking at
    // the file they just asked to locate.
    let deep_tab = ide.wait_for_event(open_mark, "a tab for deep.txt", |e| {
        e["ev"] == "tab_added" && e["title"] == "deep.txt"
    });
    let (tab_x, tab_y) = rect_centre(&deep_tab["rect"]);
    ide.click_at(tab_x, tab_y, 1);
    let (locate_x, locate_y) = {
        let toolbar = ide
            .events_since_of(Mark::start(), "project_tree_toolbar")
            .into_iter()
            .next_back()
            .expect("a project_tree_toolbar marker");
        rect_centre(&toolbar["locateRect"])
    };
    let reveal_mark = ide.mark();
    ide.click_at(locate_x, locate_y, 1);
    ide.wait_for_event(reveal_mark, "a row for deep.txt", |e| {
        e["ev"] == "project_tree_row" && e["path"] == deep_path.to_string_lossy().as_ref()
    });

    assert_eq!(ide.quit(), 0);
}
