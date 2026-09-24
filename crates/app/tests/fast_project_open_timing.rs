//! Manual timing probe for the fast project open plan
//! (`docs/architecture/fast-project-open-plan.md`), PR1: time from
//! `main_window_shown` to the project tree's first painted row, and whether
//! the UI stays responsive (a menu still opens) while the watcher registers
//! in the background for a large project. Not part of the regular suite —
//! run by hand under `make e2e`, following the same real-binary-under-Xvfb
//! method `settings_dialog_timing.rs` uses for its own latency probe.
//!
//! `main_window_shown` and `project_tree_rows` both carry an `elapsed_ms`
//! field on the same process-entry clock (`e2e_mark.h`'s `e2eElapsedMs()`),
//! added alongside this test — comparing them tells us the tree's own paint
//! latency without racing the marker file the way bracketing the wait calls
//! with `Instant::now()` would (both events can already exist in the file
//! by the time this test starts polling for them).

use std::path::{Path, PathBuf};
use std::time::Instant;

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A directory tree big enough to make the pre-PR1 eager walk and
/// synchronous watcher registration visibly slow: `dirs` directories,
/// flat under `root`, `files_per_dir` empty files in each.
fn generate_large_tree(root: &Path, dirs: usize, files_per_dir: usize) {
    for i in 0..dirs {
        let dir = root.join(format!("d{i}"));
        std::fs::create_dir(&dir).expect("create dir");
        for j in 0..files_per_dir {
            std::fs::write(dir.join(format!("f{j}.txt")), "").expect("create file");
        }
    }
}

#[test]
#[ignore]
fn open_large_project_paints_tree_before_watcher_settles() {
    let mut ide = Ide::launch("fast_project_open_timing", APP, fixture("tiny"));

    // Populate the already-seeded project root directly, rather than
    // pointing `Ide::launch` at a fixture this size: that would copy the
    // generated tree a second time (fixture -> temp project dir) for no
    // reason, doubling the setup cost of a probe that isn't measuring
    // fixture-copy speed. `last-project.txt` was seeded once, at `launch`,
    // with this same path, and survives `quit`/`relaunch` untouched, so
    // `reopenLastProject` opens the now-large tree on the next launch.
    ide.quit();
    let setup_start = Instant::now();
    generate_large_tree(ide.project_root(), 20_000, 15);
    eprintln!(
        "generated 20,000 dirs / 300,000 files in {:?}",
        setup_start.elapsed()
    );
    ide.relaunch();

    let shown = ide.wait_for_ev(Mark::start(), "main_window_shown");
    let shown_ms = shown["elapsed_ms"].as_i64().expect("elapsed_ms");

    let painted = ide.wait_for_event(Mark::start(), "the tree to paint its first rows", |e| {
        e["ev"] == "project_tree_rows" && e["count"].as_u64().unwrap_or(0) > 0
    });
    let painted_ms = painted["elapsed_ms"].as_i64().expect("elapsed_ms");

    eprintln!(
        "main_window_shown at {shown_ms}ms; first tree row painted at {painted_ms}ms \
         ({}ms after shown)",
        painted_ms - shown_ms
    );

    // UI responsiveness during open: opening a menu re-enters the Qt event
    // loop, so it can only succeed if that loop is actually pumping — a
    // menu that never lays out its actions (`e2eMarkMenuActions` fires from
    // `aboutToShow`, timer-deferred by one turn) means the UI is frozen,
    // exactly what watcher registration running on the Qt thread used to
    // cause for a tree this size.
    let menu_mark = ide.mark();
    let menu_wait_start = Instant::now();
    ide.key("alt+f"); // "&File"
    ide.wait_for_event(menu_mark, "the File menu to lay out its actions", |e| {
        e["ev"] == "file_menu_action"
    });
    eprintln!(
        "File menu opened and laid out in {:?} (while the watcher may still be registering)",
        menu_wait_start.elapsed()
    );
    ide.key("Escape");

    ide.quit();
}
