//! E2 (the jvm-build-tools plan): end-to-end flows for Gradle/Maven support
//! (ADR-0057) against the *real* toolchains — nightly only, never part of
//! the per-PR `e2e-ci` budget (ADR-0057 §6's own "E2E flow budget note").
//!
//! Gated at runtime on `IDE_E2E_JVM=1`, the same variable `make jvm-ci`
//! exports before running this file — `crates/jvm-build-core/tests/gradle_
//! integration.rs`/`maven_integration.rs` gate the identical concern at
//! *compile* time instead (`#![cfg(feature = "jvm-integration")]`), which
//! does not fit here: `app`'s own test binary has no such feature, and this
//! file's two tests need to *skip*, not fail to build, on every host that
//! is not the `linux-jvm` image (a real JDK, Gradle and Maven on `PATH`).
//!
//! Both scenarios each get their own [`Ide::launch`] — never shared, the
//! same isolation every other multi-scenario E2E file already gives each
//! `#[test]`. Every fixture is copied into a fresh `tempfile::tempdir()`
//! before launch (`Ide::launch` already does this once; Gradle's own
//! `build/` and `.gradle/` directories a real sync/run writes would
//! otherwise land inside the checked-in fixture tree in-place).
//!
//! Harness traps this file deliberately avoids (see the project's own
//! headless-E2E notes): every menu/dialog/tree interaction below is a mouse
//! click on a rect a marker reported, never a keyboard mnemonic (an Alt
//! chord stops carrying Alt the instant this process has ever `type`d into
//! a widget); Search Everywhere (`ctrl+shift+a`/`ctrl+shift+n`) is used
//! only before either scenario's first *editor* `type_text` call, per its
//! own "reliable only before editor typing" trap — scenario 1 never types
//! into the editor at all, and scenario 2 uses Search Everywhere exactly
//! once, before its own `ctrl+space`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use e2e::{Ide, Mark};
use serde_json::Value;

const APP: &str = env!("CARGO_BIN_EXE_app");

/// Review fix #6: `e2e::wait::DEFAULT_TIMEOUT` (60s) is tight for a real
/// Gradle/Maven sync, build or test run in Docker — every wait in this
/// file for one of those three (never for anything the harness itself
/// drives, which stays on the default) gets this ceiling instead via
/// `Ide::wait_for_event_within`.
const REAL_TOOLCHAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Skip (never fail) unless `IDE_E2E_JVM=1` — set by `make jvm-ci` inside
/// the `linux-jvm` image, the only place a real `gradle`/`mvn`/JDK are on
/// `PATH`. Called first thing in every `#[test]` below.
fn require_jvm_e2e() -> bool {
    let enabled = std::env::var("IDE_E2E_JVM").as_deref() == Ok("1");
    if !enabled {
        eprintln!(
            "skipping: set IDE_E2E_JVM=1 to run (needs a real JDK/Gradle/Maven — `make test-jvm`/`make jvm-ci`, inside the `linux-jvm` image)"
        );
    }
    enabled
}

/// A4/A5's own fixture projects, not `crates/app/tests/fixtures` (every
/// other E2E file's `fixture()` looks there instead) — this suite is the
/// one E2E flow that reuses `jvm-build-core`'s real-toolchain fixtures
/// rather than a fixture of its own, so the "does the tree Search
/// Everywhere → Build Tools shows agree with what `gradle`/`mvn` itself
/// resolved" question is answered against the same project A4/A5's own
/// integration tests already validate.
fn jvm_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../jvm-build-core/tests/fixtures")
        .join(name)
}

fn copy_dir_all(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let entry = entry.expect("readable fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir_all(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("fixture file");
        }
    }
}

/// The centre of a `[x, y, w, h]` rect a marker reported — the only way to
/// click a widget this harness has no handle on (`e2e.rs`'s own
/// `rect_centre`, duplicated per that file's own doc comment: a ten-line
/// helper is not worth a shared crate between test binaries).
fn rect_centre(rect: &Value) -> (i32, i32) {
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

/// The rect a `build_tools_row` mark carries for a tree item that is not
/// currently visible (a collapsed ancestor) — `[0,0,0,0]`, `e2e_mark.h`'s
/// own convention for "no rect yet" every rect-carrying mark in this repo
/// shares.
fn rect_zero() -> Value {
    serde_json::json!([0, 0, 0, 0])
}

/// Review fix #2: a short, bounded, non-panicking probe for a
/// `completion_shown` marker since `mark` — every other wait in this file
/// goes through `Ide::wait_for_ev`, which panics past its (60s) deadline,
/// so calling it from *inside* an outer retry loop (as this test's Maven
/// scenario poll used to) leaves that loop unable to actually retry: the
/// very first not-ready-yet attempt eats the whole budget and the test
/// fails before a second `ctrl+space` is ever sent. This one returns
/// `None` on its own short timeout instead, handing control back to the
/// caller's loop.
fn probe_completion_shown(ide: &Ide, mark: Mark, timeout: Duration) -> Option<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(shown) = ide
            .events_since_of(mark, "completion_shown")
            .into_iter()
            .next()
        {
            return Some(shown);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Open Search Everywhere, type `query`, accept the top hit — `e2e.rs`'s
/// own `open_search_popup`/`accept_top_hit`, collapsed into one call since
/// neither scenario below needs the two halves separately.
fn search_everywhere_accept(ide: &Ide, shortcut: &str, query: &str) {
    let main_window = ide.window().to_string();
    let mark = ide.mark();
    ide.key(shortcut);
    ide.wait_for_event(mark, "the search popup to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "search_everywhere"
    });
    ide.wait_for_focus_change(&main_window);
    ide.wait_for_ev(mark, "search_results");

    let mark = ide.mark();
    ide.type_text(query);
    ide.wait_for_event(mark, &format!("results for `{query}`"), |e| {
        e["ev"] == "search_results" && e["count"].as_u64().unwrap_or(0) > 0
    });
    ide.key("Return");
    ide.wait_for_event(mark, "the search popup to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "search_everywhere" && e["accepted"] == true
    });
    ide.focus_main();
}

/// Gradle flow: fixture sync gated behind the trust banner, the dock's
/// tree, running a task, and a failing test reaching the Tests dock.
#[test]
#[ignore = "E2E: needs a real JDK/Gradle/Maven; run via `make jvm-ci` inside the `linux-jvm` image"]
fn e2e_gradle_sync_run_and_test() {
    if !require_jvm_e2e() {
        return;
    }
    let name = "e2e_gradle_sync_run_and_test";
    let mut ide = Ide::launch(name, APP, jvm_fixture("gradle-single"));
    ide.wait_for_ev(Mark::start(), "project_opened");

    // 1. The trust banner (B4): a Gradle marker file with nothing in
    // `[build_tools] trusted_roots` yet shows "Gradle project detected.
    // Load it?" above the editor — never auto-synced, ADR-0057 §3.
    let banner = ide.wait_for_event(Mark::start(), "the Gradle trust banner", |e| {
        e["ev"] == "build_tools_banner" && e["kind"] == "trust_gradle"
    });
    let (x, y) = rect_centre(&banner["primary_rect"]);

    let mark = ide.mark();
    ide.click_at(x, y, 1);

    // 2. Sync runs the real `gradle` binary (no wrapper in this fixture,
    // `run_core::toolchain::gradle_program` falls back to system `gradle`
    // — the `linux-jvm` image's own PATH entry, A2) against the init
    // script's `ideModel` task; `build_tools_synced` fires once it is no
    // longer in flight either way, so a real failure reads as a readable
    // assertion failure rather than a timeout. Review fix #6: 180s, not
    // the harness's 60s default — a real Gradle sync in Docker is not
    // this harness's own UI driving itself.
    let synced =
        ide.wait_for_event_within(mark, REAL_TOOLCHAIN_TIMEOUT, "the sync to finish", |e| {
            e["ev"] == "build_tools_synced"
        });
    assert_eq!(
        synced["failed"], false,
        "Gradle sync failed against the real toolchain"
    );

    // 3. Open the Build Tools dock through Search Everywhere — *before*
    // any editor typing happens anywhere in this test (there is none), the
    // one condition `ctrl+shift+a` needs to stay reliable.
    let mark = ide.mark();
    search_everywhere_accept(&ide, "ctrl+shift+a", "Build Tools");
    let rows = ide.wait_for_event(mark, "the Build Tools tree to report its rows", |e| {
        e["ev"] == "build_tools_rows"
    });
    let row_marks = ide.events_since_of(mark, "build_tools_row");
    assert!(
        row_marks.len() >= rows["count"].as_u64().unwrap_or(0) as usize,
        "fewer build_tools_row marks than build_tools_rows announced"
    );
    let labels: Vec<String> = row_marks
        .iter()
        .filter_map(|e| e["label"].as_str().map(str::to_string))
        .collect();
    for expected in ["Tasks", "build", "test", "Dependencies"] {
        assert!(
            labels.iter().any(|l| l == expected),
            "expected a `{expected}` row among {labels:?}"
        );
    }

    // 4. The runnable `build` task row starts out collapsed two levels
    // deep (`Tasks` -> the `build` task-group -> the `build` task itself
    // — Gradle's own task group happens to share the task's name), so its
    // rect is `[0,0,0,0]` until both ancestors are expanded. Qt's default
    // `QTreeWidget` behaviour toggles expansion on a double-click
    // regardless of `runnable` (only a runnable row's double-click also
    // reaches `runNode`, `build_tools_panel.cpp`'s own `itemDoubleClicked`
    // connect), and each expansion re-fires `build_tools_row` — the
    // `itemExpanded` connect that codepath added alongside this test.
    let tasks_row = row_marks
        .iter()
        .find(|e| e["label"] == "Tasks")
        .expect("a `Tasks` row");
    let (x, y) = rect_centre(&tasks_row["rect"]);
    let mark = ide.mark();
    ide.double_click_at(x, y, 1);
    ide.wait_for_ev(mark, "build_tools_rows");
    let build_group_row = ide
        .events_since_of(mark, "build_tools_row")
        .into_iter()
        .find(|e| e["label"] == "build" && e["runnable"] == false && e["rect"] != rect_zero())
        .expect("the `build` task-group row, now visible");
    let (x, y) = rect_centre(&build_group_row["rect"]);
    let mark = ide.mark();
    ide.double_click_at(x, y, 1);
    ide.wait_for_ev(mark, "build_tools_rows");
    let build_row = ide
        .events_since_of(mark, "build_tools_row")
        .into_iter()
        .find(|e| e["label"] == "build" && e["runnable"] == true && e["rect"] != rect_zero())
        .expect("the runnable `build` task row, now visible");
    let (x, y) = rect_centre(&build_row["rect"]);

    let mark = ide.mark();
    ide.double_click_at(x, y, 1);
    let started = ide.wait_for_event(mark, "the run console to appear", |e| {
        e["ev"] == "run_console_tab_added"
    });
    let console_id = started["console_id"].as_u64().expect("console_id");
    // Review fix #6: a real `gradle build` task, not this harness's own UI —
    // 180s, not the harness's 60s default.
    ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        "the `build` task to finish",
        |e| e["ev"] == "run_console_finished" && e["console_id"].as_u64() == Some(console_id),
    );

    // 5. Open the Tests dock the same way, and click its "Run All" button
    // (a toolbar `QToolButton`, not a menu action — `tests_panel.cpp` has
    // no menu of its own beyond the View menu's dock-toggle entry).
    let mark = ide.mark();
    search_everywhere_accept(&ide, "ctrl+shift+a", "Tests");
    let toolbar = ide.wait_for_event(mark, "the Tests dock's toolbar rects", |e| {
        e["ev"] == "tests_toolbar_rects"
    });
    let run_all_rect = toolbar["buttons"]
        .as_array()
        .expect("buttons array")
        .iter()
        .find(|b| b["name"] == "run_all")
        .expect("a run_all button entry")["rect"]
        .clone();
    let (x, y) = rect_centre(&run_all_rect);

    let mark = ide.mark();
    ide.click_at(x, y, 1);
    // Review fix #6: a real `gradle test` run, not this harness's own UI —
    // 180s, not the harness's 60s default.
    ide.wait_for_event_within(
        mark,
        REAL_TOOLCHAIN_TIMEOUT,
        "the test run to finish",
        |e| e["ev"] == "test_run_finished",
    );

    // 6. `GreeterTest::deliberatelyFails` (A4's own fixture) landed in the
    // tree with a failed status. JUnit 5 reports the method as
    // `deliberatelyFails()` (parens included), and the tree is rebuilt
    // wholesale on every TeamCity event — so the row repeats, first as
    // `running`, and only its *last* mark is the settled status.
    let rows = ide.events_since_of(mark, "test_tree_row");
    let fails = rows
        .iter()
        .rev()
        .find(|e| {
            e["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("deliberatelyFails"))
        })
        .unwrap_or_else(|| panic!("no `deliberatelyFails` row among {rows:?}"));
    assert_eq!(fails["status"], "failed");

    assert_eq!(ide.quit(), 0);
}

/// Maven flow: `pom.xml` coordinate completion from the local repository
/// index, offline — no trust banner, no sync, since D5's completion reads
/// straight from `~`'s local repo/`.ide/settings.toml`'s configured one
/// and the file on disk, independent of `BuildToolsService`'s synced
/// model (ADR-0057's "read-only, delegated" rule applies to the model, not
/// to build-file editing, which never needed a sync in the first place).
#[test]
#[ignore = "E2E: needs a real JDK/Gradle/Maven; run via `make jvm-ci` inside the `linux-jvm` image"]
fn e2e_maven_pom_completion() {
    if !require_jvm_e2e() {
        return;
    }
    let name = "e2e_maven_pom_completion";

    // The `linux-jvm` image's fixture-prewarm step (`docker/Dockerfile`)
    // already resolved `maven-single`'s own dependencies — including
    // `org.junit.jupiter:junit-jupiter`'s transitive `junit-jupiter-api`
    // — into `/opt/jvm-cache/m2` at image-build time. `repo_index` reads
    // `[build_tools.maven].local_repository` when set rather than
    // `~/.m2` (empty: `Ide::launch` gives every scenario an isolated
    // `HOME`), so this project-scoped `.ide/settings.toml` points it at
    // that already-warm cache; `offline = true` keeps the flow
    // deterministic by skipping D5's Maven Central fallback entirely —
    // the local-repo answer alone is what this test asserts. Written
    // through `app_config::project_settings` rather than a hand-rolled
    // TOML string, the same reason every other seeded-settings E2E test
    // does, so a future field rename here fails to compile instead of
    // silently writing a key nothing reads any more.
    let staged = tempfile::tempdir().expect("staging dir");
    copy_dir_all(&jvm_fixture("maven-single"), staged.path());
    let project_settings = app_config::project_settings::ProjectSettings {
        build_tools: Some(app_config::BuildToolsProjectSettings {
            maven: app_config::MavenToolSettings {
                local_repository: Some(PathBuf::from("/opt/jvm-cache/m2")),
                offline: Some(true),
                ..app_config::MavenToolSettings::default()
            },
            ..app_config::BuildToolsProjectSettings::default()
        }),
        ..app_config::project_settings::ProjectSettings::default()
    };
    app_config::project_settings::save(staged.path(), &project_settings)
        .expect("seed .ide/settings.toml");

    // Review fix #1: this scenario claims to exercise the Maven local
    // repository, but the `linux-jvm` image also exports a *global*
    // `GRADLE_USER_HOME=/opt/jvm-cache/gradle` (`docker/Dockerfile`'s
    // fixture-prewarm), which `Command::new` inherits into the launched
    // app same as any other environment variable `Ide::launch` does not
    // explicitly override. `maven-single`'s own dependencies happen to
    // also be resolvable out of that prewarmed Gradle module cache, so
    // without pointing `GRADLE_USER_HOME` somewhere empty for *this*
    // process, `junit-jupiter-api` could appear in completion via the
    // Gradle path and the assertion below would pass for the wrong
    // reason, exercising nothing this fix actually changed.
    let empty_gradle_home = tempfile::tempdir().expect("empty GRADLE_USER_HOME");
    let mut ide = Ide::launch_with_env(
        name,
        APP,
        staged.path(),
        &[(
            "GRADLE_USER_HOME",
            empty_gradle_home.path().to_str().expect("utf-8 path"),
        )],
    );
    ide.wait_for_ev(Mark::start(), "project_opened");

    // Open pom.xml through Go to File — the one Search Everywhere use in
    // this scenario, before the only editor typing it does (there is
    // none: every caret move below is a key chord, never `type_text`).
    let mark = ide.mark();
    search_everywhere_accept(&ide, "ctrl+shift+n", "pom.xml");
    ide.wait_for_event(mark, "a tab for `pom.xml`", |e| {
        e["ev"] == "tab_added" && e["title"] == "pom.xml"
    });

    // Line 22 of this fixture's `pom.xml` is
    // `            <artifactId>junit-jupiter</artifactId>` — the caret
    // lands right after `junit-jupiter` (37 columns in from the start of
    // the line, verified against the fixture's own text rather than
    // assumed), inside the declared coordinate `ctrl+space` completes.
    ide.key("ctrl+Home");
    for _ in 0..21 {
        ide.key("Down");
    }
    ide.key("Home");
    for _ in 0..37 {
        ide.key("Right");
    }

    // `shared_repo_index` (the local-repository scan `completion_at`
    // reads) is built off the Qt thread on project open and delivered
    // asynchronously (`BuildToolsService::project_opened`, review fix
    // #2) — nothing marks "the index is ready" on its own, so this polls
    // `ctrl+space` itself rather than assuming the first press lands
    // after that delivery. Each retry is a real transition check (a fresh
    // `completion_shown` marker, `probe_completion_shown`'s own short,
    // bounded, non-panicking wait — never `Ide::wait_for_ev`, whose 60s
    // panic-on-timeout would eat the outer loop's entire budget on the
    // first not-ready-yet attempt and leave nothing to retry with).
    let labels = e2e::wait_for(
        "`junit-jupiter-api` to appear in pom.xml completion",
        || {
            let mark = ide.mark();
            ide.key("ctrl+space");
            let shown = probe_completion_shown(&ide, mark, Duration::from_secs(5))?;
            let labels: Vec<String> = shown["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if labels.iter().any(|l| l == "junit-jupiter-api") {
                Some(labels)
            } else {
                ide.key("Escape");
                None
            }
        },
    );
    assert!(
        labels.iter().any(|l| l == "junit-jupiter-api"),
        "expected `junit-jupiter-api` among {labels:?}"
    );

    assert_eq!(ide.quit(), 0);
}
