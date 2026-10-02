//! End-to-end flow for the PHP tooling plan's analyzer surface (E2): a
//! `phpstan`-shaped analyzer's findings reach the editor's squiggles and the
//! Problems dock, double-clicking a row navigates to it, and (R4, the
//! IntelliJ-parity refinement plan) F2 walks the caret between the two
//! findings this fixture now reports.
//!
//! ADR-0046 records the E2E budget moving from 15 to 16 for this flow;
//! ADR-0066 records the PHP parity flows added since.
//!
//! No real PHPStan or Composer is installed anywhere this suite runs
//! (Risk #7 in the plan): `analysis_core::bin::stub_analyzer` (E1) plays
//! `vendor/bin/phpstan`, printing a canned `checkstyle-xml` finding against
//! the fixture's own `src/Greeter.php` — the same file, line, column and
//! message as `analysis-core/tests/fixtures/checkstyle_one_file.xml`, so the
//! two stay in sync rather than drifting apart as two hand-maintained copies
//! of the same fixture data.
//!
//! `env!("CARGO_BIN_EXE_stub_analyzer")` cannot be used here: Cargo only
//! sets `CARGO_BIN_EXE_<name>` for a binary's *own* package's tests (the
//! same reason `lsp_core::bin::stub_server` is only ever `env!()`-located
//! from within `lsp-core` itself, never from `crates/app`). `e2e-ci`
//! (`Makefile`) builds `stub_analyzer` explicitly, right beside its own
//! `cargo build --bin stub_server -p lsp-core` line, so its executable
//! lands in the same target directory as `app`'s own binary; its path is
//! derived from `CARGO_BIN_EXE_app`'s directory instead — see
//! [`stub_analyzer_bin`].
//!
//! Live runs (`OnType`/`OnSave` through
//! `analysis_core::Scheduler::schedule_file_run`) are wired in
//! `AnalysisServiceRust`, and `settings_model::analysis::file_jobs` decides
//! which analyzer runs for which event; they are covered by unit tests
//! there. This flow drives the "Inspect Project" menu action instead of
//! typing and waiting out a debounce interval: it is the deterministic path
//! for a project's findings to reach the editor.

use std::path::{Path, PathBuf};

use e2e::{Ide, Mark};

use crate::support::{
    buffer, cursor, open_file, rect_centre, route_language_at_stubs, wait_for_index, StubServer,
    APP,
};

/// `stub_analyzer`'s executable — see this file's own doc comment for why
/// `env!("CARGO_BIN_EXE_stub_analyzer")` cannot be used directly.
fn stub_analyzer_bin() -> PathBuf {
    let name = if cfg!(windows) {
        "stub_analyzer.exe"
    } else {
        "stub_analyzer"
    };
    let path = Path::new(APP).with_file_name(name);
    assert!(
        path.is_file(),
        "{} does not exist — run via `make e2e`, which builds it \
         (`cargo build --bin stub_analyzer -p analysis-core`) before this \
         test; `cargo test -p app` alone never builds a binary that only \
         `analysis-core` declares",
        path.display()
    );
    path
}

/// A Composer project declaring `phpstan/phpstan` in `require-dev`, with
/// `vendor/bin/phpstan` seeded to `stub_analyzer`'s binary and a
/// `src/Greeter.php` long enough for both of the stub's canned findings
/// (line 10 and line 20, `checkstyle_one_file.xml`'s own two rows — R4's
/// F2 flow needs two diagnostics to navigate between) to land on real
/// lines. `$name` on line 10 is genuinely unbound and `$x` on line 20 is
/// genuinely unused in this file, so a reader who goes looking sees the
/// same two bugs PHPStan reports — nothing here is asserting against a lie.
fn php_analyzer_fixture() -> tempfile::TempDir {
    const GREETER_LINES: &[&str] = &[
        "<?php",
        "",
        "class Greeter",
        "{",
        "    public function greet(): string",
        "    {",
        "        // step one",
        "        // step two",
        "        // step three",
        "        $greeting = $name;", // line 10: PHPStan's undefined-variable error
        "        return $greeting;",
        "    }",
        "",
        "    public function unused(): void",
        "    {",
        "        // step four",
        "        // step five",
        "        // step six",
        "        // step seven",
        "        $x = 1;", // line 20: PHPStan's unused-variable warning
        "    }",
        "}",
    ];

    let dir = tempfile::TempDir::new().expect("temp analyzer fixture dir");
    std::fs::write(
        dir.path().join("composer.json"),
        r#"{"require-dev": {"phpstan/phpstan": "^1.0"}}"#,
    )
    .expect("composer.json");
    std::fs::create_dir_all(dir.path().join("src")).expect("src dir");
    let greeter_php = GREETER_LINES.join("\n") + "\n";
    std::fs::write(dir.path().join("src/Greeter.php"), greeter_php).expect("Greeter.php");

    let vendor_bin = dir.path().join("vendor/bin");
    std::fs::create_dir_all(&vendor_bin).expect("vendor/bin");
    std::fs::copy(stub_analyzer_bin(), vendor_bin.join("phpstan"))
        .expect("seeding the stub analyzer");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            vendor_bin.join("phpstan"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("making the stub analyzer executable");
    }
    dir
}

/// Open the "Ana&lysis" menu via its mnemonic, waiting for it to actually be
/// on screen before anything clicks into it.
fn open_analysis_menu(ide: &Ide) -> Mark {
    let mark = ide.mark();
    ide.key("alt+l");
    ide.wait_for_event(mark, "the Analysis menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "analysis_menu"
    });
    mark
}

/// B/C's verification scenario from the plan: open a fixture project whose
/// `vendor/bin/phpstan` is the stub analyzer, confirm findings appear as
/// squiggles, that the Problems dock shows them with source `PHPStan`
/// (`plugin.toml`'s `name`, not its lowercase `id`), and that double-clicking
/// a row navigates to it.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_analyzer_findings_appear_inline_and_in_problems() {
    let name = "e2e_analyzer_findings_appear_inline_and_in_problems";
    let project = php_analyzer_fixture();
    let mut ide = Ide::launch(name, APP, project.path());
    drop(project);
    let mcp = ide.mcp();

    ide.wait_for_ev(Mark::start(), "project_opened");
    wait_for_index(&mcp);

    // Open the file before running the analyzer: `EditorTabs::
    // applyDiagnostics` only repaints tabs that are already open, so this is
    // what proves a finding reaches an *already-visible* editor, not merely
    // the store.
    let tab = open_file(&ide, "Greeter.php");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    let before = cursor(&mcp, tab_id);

    let mark = open_analysis_menu(&ide);
    let inspect = ide.wait_for_event(mark, "the Inspect Project menu item", |e| {
        e["ev"] == "analysis_menu_action" && e["label"] == "Inspect Project"
    });
    let (x, y) = rect_centre(&inspect["rect"]);
    ide.click_at(x, y, 1);
    ide.wait_for_event(mark, "the Analysis menu to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "analysis_menu"
    });

    // The squiggle: `diagnostics_applied` is `EditorTabs::applyDiagnostics`
    // reporting it repainted this file's spans (ADR-0046's wiring, extended
    // in this change to include `AnalysisService`).
    ide.wait_for_event(
        mark,
        "the analyzer's finding to reach the editor as a squiggle",
        |e| {
            e["ev"] == "diagnostics_applied"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("Greeter.php"))
                && e["count"].as_u64().unwrap_or(0) > 0
        },
    );

    // The Problems dock: one row, named after the analyzer.
    let row = ide.wait_for_event(mark, "the finding to reach the Problems dock", |e| {
        e["ev"] == "problem_row"
            && e["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("Greeter.php"))
            && e["source"] == "PHPStan"
    });
    ide.wait_for_event(mark, "the Problems dock to report a total", |e| {
        e["ev"] == "problems_refreshed" && e["total"].as_u64().unwrap_or(0) > 0
    });

    // Double-clicking the row navigates the already-open tab to line 10
    // (1-based in the row and from `get_cursor_position`) — the
    // canned finding's own line.
    let (rx, ry) = rect_centre(&row["rect"]);
    ide.double_click_at(rx, ry, 1);
    e2e::wait_for("the caret to land on the finding's line", || {
        let after = cursor(&mcp, tab_id);
        (after != before && after.0 == 10).then_some(())
    });

    // R4: F2 (`code.nextDiagnostic`) walks to this file's other finding
    // (line 20's warning) from wherever the double-click above just left
    // the caret, and a second F2 wraps back to line 10's — the fixture's
    // only two diagnostics — `diagnostics_core::DiagnosticStore::
    // next_after`'s wrap-around contract, driven the same way `EditorTabs::
    // goToDiagnostic` reads the caret's own position. Kept inside this same
    // flow (ADR-0046's E2E budget) rather than as a second test.
    ide.focus_main();
    ide.key("F2");
    e2e::wait_for("F2 to land on the second diagnostic (line 20)", || {
        (cursor(&mcp, tab_id).0 == 20).then_some(())
    });
    ide.key("F2");
    e2e::wait_for(
        "a second F2 to wrap back to the first diagnostic (line 10)",
        || (cursor(&mcp, tab_id).0 == 10).then_some(()),
    );

    assert_eq!(ide.quit(), 0);
}

/// [`php_analyzer_fixture`] plus PHP_CodeSniffer: `squizlabs/php_codesniffer`
/// in `require-dev` and `vendor/bin/phpcs` seeded to the same stub.
fn php_two_tools_fixture() -> tempfile::TempDir {
    let dir = php_analyzer_fixture();
    std::fs::write(
        dir.path().join("composer.json"),
        r#"{"require-dev": {"phpstan/phpstan": "^1.0", "squizlabs/php_codesniffer": "^3.0"}}"#,
    )
    .expect("composer.json");
    let bin = dir.path().join("vendor/bin");
    std::fs::copy(bin.join("phpstan"), bin.join("phpcs")).expect("seeding phpcs");
    dir
}

/// Ctrl+S, then wait for the tab to go clean — `read_buffer` answers from
/// the rope, which is one save behind the live widget.
fn save_and_sync(ide: &Ide, mcp: &e2e::mcp::Mcp, tab_id: u64) {
    let mark = ide.mark();
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(mcp);
}

/// The labels of the `intentions_menu_action` marks since `mark`.
fn intention_labels(ide: &Ide, mark: Mark) -> Vec<String> {
    ide.events_since_of(mark, "intentions_menu_action")
        .iter()
        .filter_map(|e| {
            e["label"]
                .as_str()
                .map(|l| l.split('\t').next().unwrap().to_string())
        })
        .collect()
}

/// Wait for a Problems row from `source` on `Greeter.php` after `mark`.
fn wait_for_problem(ide: &Ide, mark: Mark, source: &str) -> serde_json::Value {
    ide.wait_for_event(
        mark,
        &format!("a `{source}` row in the Problems dock"),
        |e| {
            e["ev"] == "problem_row"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("Greeter.php"))
                && e["source"] == source
        },
    )
}

/// PHP parity E2: two language servers and two analyzers on one PHP file,
/// the whole way to the user's eye. Two stub servers play Intelephense and
/// Phpactor (tagged profiles, `STUB_LSP_TAG`), `vendor/bin/{phpstan,phpcs}`
/// are the stub analyzer.
///
/// - Both servers' diagnostics reach Problems under their own source.
/// - Typing a finding runs PHPCS (it reads the buffer) and not PHPStan (it
///   reads only the saved file); saving runs PHPStan.
/// - Completion merges both servers' items, `shared` once.
/// - Alt+Enter offers each server's fix and a suppress action per analyzer;
///   the PHPStan one inserts its comment above the line, and one Ctrl+Z
///   removes it.
/// - `fore` + Tab expands the PHP live template.
///
/// ADR-0066's E2E budget note counts this flow.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_php_two_servers_and_on_save_analysis() {
    let name = "e2e_php_two_servers_and_on_save_analysis";
    let project = php_two_tools_fixture();
    let mut ide = Ide::launch(name, APP, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_language_at_stubs(
        &mut ide,
        "php",
        &[
            StubServer {
                id: "intelephense",
                caps: "completion,codeAction,hover",
            },
            StubServer {
                id: "phpactor",
                caps: "completion,codeAction,hover",
            },
        ],
    );
    let mcp = ide.mcp();
    wait_for_index(&mcp);

    // Both servers publish on open, each under its own source.
    let mark = ide.mark();
    let tab = open_file(&ide, "Greeter.php");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    wait_for_problem(&ide, mark, "stub_intelephense");
    wait_for_problem(&ide, mark, "stub_phpactor");

    ide.focus_main();
    ide.key("ctrl+End");
    // Completion: both servers answer, `shared` is listed once.
    let mark = ide.mark();
    ide.key("ctrl+space");
    let shown = ide.wait_for_event(mark, "the merged completion list", |e| {
        e["ev"] == "completion_shown"
            && e["labels"]
                .as_array()
                .is_some_and(|l| l.iter().any(|x| x == "only_phpactor"))
    });
    let labels: Vec<&str> = shown["labels"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .collect();
    assert!(labels.contains(&"only_intelephense"), "{labels:?}");
    assert_eq!(
        labels.iter().filter(|l| **l == "shared").count(),
        1,
        "{labels:?}"
    );
    ide.key("Escape");

    // On type: PHPCS reads the buffer from stdin, PHPStan waits for a save.
    let mark = ide.mark();
    ide.type_text("// STUB_FINDING");
    wait_for_problem(&ide, mark, "PHP_CodeSniffer");
    assert!(
        !ide.events_since_of(mark, "problem_row")
            .iter()
            .any(|e| e["source"] == "PHPStan"),
        "PHPStan analysed an unsaved buffer"
    );

    // On save: PHPStan runs against the file on disk.
    let mark = ide.mark();
    ide.key("ctrl+s");
    wait_for_problem(&ide, mark, "PHPStan");

    // Alt+Enter on the finding's line (saving appended the final newline and
    // left the caret below it): each server's fix and a suppress action per
    // analyzer.
    ide.key("Up");
    let before = {
        ide.sync(&mcp);
        buffer(&mcp, tab_id)
    };
    let mark = ide.mark();
    ide.key("alt+Return");
    ide.wait_for_event(mark, "the intentions menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "intentions_menu"
    });
    ide.wait_for_event(mark, "the Suppress PHPStan entry", |e| {
        e["ev"] == "intentions_menu_action"
            && e["label"]
                .as_str()
                .is_some_and(|l| l.starts_with("Suppress PHPStan.stubFinding (PHPStan)"))
    });
    let labels = intention_labels(&ide, mark);
    for wanted in [
        "fix from intelephense",
        "fix from phpactor",
        "Suppress PHPStan.stubFinding (PHPStan)",
        "Suppress Stub.Sniff.Finding (PHP_CodeSniffer)",
    ] {
        assert!(
            labels.iter().any(|l| l == wanted),
            "no `{wanted}` in {labels:?}"
        );
    }
    let suppress = ide
        .events_since_of(mark, "intentions_menu_action")
        .into_iter()
        .find(|e| {
            e["label"]
                .as_str()
                .is_some_and(|l| l.starts_with("Suppress PHPStan"))
        })
        .unwrap();
    let (x, y) = rect_centre(&suppress["rect"]);
    ide.click_at(x, y, 1);
    ide.wait_for_event(mark, "the intentions menu to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "intentions_menu" && e["accepted"] == true
    });
    ide.focus_main();
    ide.sync(&mcp);
    let after = buffer(&mcp, tab_id);
    let finding = before
        .lines()
        .position(|l| l == "// STUB_FINDING")
        .expect("the typed finding");
    let lines: Vec<&str> = after.lines().collect();
    assert_eq!(
        lines[finding], "// @phpstan-ignore PHPStan.stubFinding",
        "{after:?}"
    );
    assert_eq!(lines[finding + 1], "// STUB_FINDING", "{after:?}");

    // One Ctrl+Z restores the buffer.
    ide.key("ctrl+z");
    save_and_sync(&ide, &mcp, tab_id);
    assert_eq!(
        buffer(&mcp, tab_id),
        before,
        "one Ctrl+Z did not undo the suppress"
    );

    // The PHP live template: `fore` + Tab.
    ide.key("ctrl+End");
    ide.key("Return");
    ide.type_text("fore");
    ide.key("Tab");
    save_and_sync(&ide, &mcp, tab_id);
    assert!(
        buffer(&mcp, tab_id).contains("foreach ("),
        "{:?}",
        buffer(&mcp, tab_id)
    );

    assert_eq!(ide.quit(), 0);
}
