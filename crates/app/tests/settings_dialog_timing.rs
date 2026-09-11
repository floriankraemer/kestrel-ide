//! Manual timing probe for issue #278 (Settings dialog opens with a 3-5s
//! delay). Not part of the regular suite — run by hand under `make shell` +
//! Xvfb while investigating. Left in the tree as a reusable repro rather
//! than a throwaway script, following the same real-binary-under-Xvfb
//! method `e2e.rs`'s other flows use.

use std::path::{Path, PathBuf};
use std::time::Instant;

use e2e::Ide;

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn rect_centre(rect: &serde_json::Value) -> (i32, i32) {
    let r: Vec<i64> = rect
        .as_array()
        .expect("rect array")
        .iter()
        .map(|v| v.as_i64().expect("rect component"))
        .collect();
    ((r[0] + r[2] / 2) as i32, (r[1] + r[3] / 2) as i32)
}

#[test]
#[ignore]
fn settings_dialog_open_latency() {
    let mut ide = Ide::launch("settings_dialog_open_latency", APP, fixture("tiny"));

    let opened = ide.mark();
    ide.key("alt+f"); // "&File"
    let preferences = ide.wait_for_event(opened, "Preferences... in the File menu", |e| {
        e["ev"] == "file_menu_action" && e["label"] == "Preferences..."
    });
    let (x, y) = rect_centre(&preferences["rect"]);

    let start = Instant::now();
    ide.click_at(x, y, 1);
    ide.wait_for_event(opened, "the settings dialog to show", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "settings_dialog"
    });
    let elapsed = start.elapsed();

    eprintln!("settings dialog open latency: {elapsed:?}");

    ide.key("Escape");
    ide.wait_for_event(opened, "the settings dialog to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "settings_dialog"
    });
    ide.focus_main();
    ide.quit();
}
