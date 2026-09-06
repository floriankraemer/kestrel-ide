//! End-to-end flow for the Help menu's About dialog: its own test binary for
//! the same reason `e2e_minimap.rs`/`e2e_vcs.rs` are — `e2e.rs` sits at its
//! ratcheted size ceiling (`scripts/check-file-size.sh`). `make e2e` runs all
//! of them.

use std::path::{Path, PathBuf};

use e2e::{Ide, Mark};

const APP: &str = env!("CARGO_BIN_EXE_app");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The About dialog reports the build this binary actually is.
///
/// The interesting half is not that a dialog opens: it is that `build.rs`'s
/// `cargo:rustc-env` metadata survived the trip through `AppInfo` into the
/// view. A build script that silently stopped running leaves `env!` frozen at
/// whatever it emitted first, and this is the only place that shows.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_about_reports_the_version_this_build_was_made_from() {
    let name = "e2e_about_reports_the_version_this_build_was_made_from";

    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");

    // `help.about` ships unbound, so Find Action is the reachable route —
    // the same way `e2e.rs`'s file-history flow reaches `view.vcsHistory`.
    // Inlined rather than shared: every other `e2e_*.rs` binary already
    // duplicates this much of the popup dance.
    let main_window = ide.window().to_string();
    let mark = ide.mark();
    ide.key("ctrl+shift+a");
    ide.wait_for_event(mark, "the search popup to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "search_everywhere"
    });
    ide.wait_for_focus_change(&main_window);
    ide.wait_for_ev(mark, "search_results");

    let mark = ide.mark();
    ide.type_text("About Kestrel");
    ide.wait_for_event(mark, "results for `About Kestrel`", |e| {
        e["ev"] == "search_results" && e["count"].as_u64().unwrap_or(0) > 0
    });
    ide.key("Return");
    ide.wait_for_event(mark, "the search popup to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "search_everywhere" && e["accepted"] == true
    });

    let shown = ide.wait_for_event(mark, "the About dialog", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "about_dialog"
    });

    // Shape, not equality: this binary's own `CARGO_PKG_VERSION` is `app`'s
    // and the dialog reports `ui-shell`'s. They match today and are free to
    // diverge, which would make an equality assertion red for no defect.
    let version = shown["version"].as_str().expect("version");
    assert_eq!(
        version.split('.').count(),
        3,
        "About showed `{version}`, which is not a version"
    );
    let hash = shown["hash"].as_str().expect("hash");
    assert!(
        !hash.is_empty(),
        "About showed no commit at all — build.rs emitted nothing"
    );

    ide.key("Escape");
    ide.wait_for_event(mark, "the About dialog to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "about_dialog"
    });
    // There is no window manager under Xvfb, so the input focus a closing
    // modal gives up lands nowhere — `Ctrl+Q` would go to no window at all.
    // Every other flow that raises a dialog takes the focus back the same
    // way (`accept_top_hit` in `e2e.rs`).
    ide.focus_main();

    assert_eq!(ide.quit(), 0);
}
