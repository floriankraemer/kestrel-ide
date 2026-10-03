//! End-to-end flow for the buffer-editing gestures that must be exactly one
//! Ctrl+Z each — Replace All and accepting a completion (F0-18, #142).
//!
use e2e::{mcp::Mcp, Ide, Mark};
use serde_json::json;

use crate::support::{
    buffer, fixture, fixture_text, open_file, rect_centre, route_rust_at_stub, wait_for_index, APP,
};

/// F0-18 (#142): Replace All and accepting a completion each cross the seam
/// as one `Vec<FfiTextEdit>` and are spliced inside one `beginEditBlock`, so
/// each is exactly one Ctrl+Z. One flow carries both halves — the E2E budget
/// (`next-five-features-plan.md`, Risk #14) is why it is not two.
///
/// The property regresses silently: the edits still apply, every other test
/// stays green, and only the undo depth changes — which is why each half
/// asserts the buffer *after* the undo, not just after the edit.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_replace_all_and_completion_are_one_undo_each() {
    let name = "e2e_replace_all_and_completion_are_one_undo_each";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    let original = fixture_text("tiny", "src/main.rs");
    assert_eq!(
        original.matches("world").count(),
        2,
        "the fixture's shape changed"
    );
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");

    // Replace All: two matches, one splice.
    let mark = ide.mark();
    ide.key("ctrl+r");
    let bar = ide.wait_for_event(mark, "the replace bar to open", |e| {
        e["ev"] == "find_bar_shown" && e["replace"] == true
    });
    ide.type_text("world");
    let (x, y) = rect_centre(&bar["replace_rect"]);
    ide.click_at(x, y, 1);
    ide.type_text("planet");
    let (x, y) = rect_centre(&bar["replace_all_rect"]);
    ide.click_at(x, y, 1);
    let applied = ide.wait_for_ev(mark, "edits_applied");
    assert_eq!(
        applied["count"].as_u64(),
        Some(2),
        "Replace All did not splice both matches"
    );
    // Escape closes the bar and hands focus back to the editor — Ctrl+Z
    // must reach the document, not the replace field's own undo stack.
    ide.key("Escape");

    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        original.replace("world", "planet"),
        "Replace All did not replace both matches"
    );

    undo_once_and_save(&ide, &mcp, tab_id);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo Replace All"
    );

    // Completion: the stub answers line 0 with `pop`/`push`/`#[allow]`;
    // which one `lsp_core::completion` ranks first is its unit tests'
    // business, this only needs the top row to be the one inserted.
    ide.key("ctrl+Home");
    let mark = ide.mark();
    ide.key("ctrl+space");
    let shown = ide.wait_for_ev(mark, "completion_shown");
    assert_eq!(shown["count"].as_u64(), Some(3), "the stub's line-0 items");
    ide.key("Return");
    let applied = ide.wait_for_ev(mark, "edits_applied");
    assert_eq!(applied["count"].as_u64(), Some(1));

    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(&mcp);
    assert_eq!(
        buffer(&mcp, tab_id),
        format!("#[allow(dead_code)]{original}"),
        "the completion was not inserted at (0,0)"
    );

    undo_once_and_save(&ide, &mcp, tab_id);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "one Ctrl+Z did not undo the completion"
    );

    assert_eq!(ide.quit(), 0);
}

/// R1: Tab/Shift+Tab over a selection, and Enter between a bracket pair —
/// the same real keystrokes an end user presses, through the widget's
/// `keyPressEvent`, not a direct MCP call, so a regression that only shows
/// up at the Qt seam (the wrong key falling through to `QPlainTextEdit`,
/// say) still fails this.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_indent_tab_and_enter_between_braces() {
    let name = "e2e_indent_tab_and_enter_between_braces";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    let original = fixture_text("tiny", "src/main.rs");
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // Select the file's first three lines ("mod greeting;", a blank line,
    // "fn main() {") and indent them. The blank line contributes no edit
    // (`indent_selection` skips blank lines), so the splice is two edits.
    //
    // Shift is held down for all three `Down`s in one xdotool invocation
    // rather than sent as three separate `ide.key("shift+Down")` calls:
    // each of those presses and releases Shift on its own, and three bare
    // Shift presses inside JetBrains' double-Shift window
    // (`IdeMainWindow::kDoubleShiftMs`) pop open Search Everywhere — a
    // gesture a real user holding Shift down the whole time never makes.
    ide.key("ctrl+Home");
    e2e::xdotool::run(&[
        "keydown", "shift", "key", "Down", "key", "Down", "key", "Down", "keyup", "shift",
    ]);
    let mark = ide.mark();
    ide.key("Tab");
    let applied = ide.wait_for_ev(mark, "edits_applied");
    assert_eq!(
        applied["count"].as_u64(),
        Some(2),
        "Tab did not indent both non-blank lines"
    );
    // `read_buffer` answers from the rope, which is one save behind the
    // live widget (`editor_core::Document`'s own doc comment) — every
    // buffer assertion below saves first, the same as the Replace-All flow
    // above.
    save_and_sync(&ide, &mcp, tab_id);
    let indented = original.replacen(
        "mod greeting;\n\nfn main() {",
        "    mod greeting;\n\n    fn main() {",
        1,
    );
    assert_eq!(
        buffer(&mcp, tab_id),
        indented,
        "Tab did not indent the selected lines"
    );

    // Shift+Tab undoes exactly that indentation, back to the original text.
    let mark = ide.mark();
    ide.key("shift+Tab");
    let applied = ide.wait_for_ev(mark, "edits_applied");
    assert_eq!(
        applied["count"].as_u64(),
        Some(2),
        "Shift+Tab did not unindent both lines"
    );
    save_and_sync(&ide, &mcp, tab_id);
    assert_eq!(
        buffer(&mcp, tab_id),
        original,
        "Shift+Tab did not restore the original indent"
    );

    // Enter between a bracket pair with nothing between them opens a
    // three-line block. The fixture's last line, `fn empty() {}`, is
    // already such a pair — reaching it by navigation only (`ctrl+End`,
    // `Up` to its line, `End` to its close, `Left` between the two) means
    // no typing, so nothing here depends on auto-close/type-over timing.
    ide.key("ctrl+End");
    ide.sync(&mcp);
    ide.key("Up");
    ide.sync(&mcp);
    ide.key("End");
    ide.sync(&mcp);
    ide.key("Left"); // caret now between the `{` and `}` of `fn empty() {}`
    ide.sync(&mcp);
    ide.key("Return");
    ide.sync(&mcp);
    // `read_buffer` needs a save to see anything (the rope is one save
    // behind the live widget), and saving trims the inner line's "    " —
    // it is trailing whitespace on a line with nothing else on it, exactly
    // as `trim_trailing_whitespace` (on by default) is supposed to. So the
    // saved shape has an empty middle line, not an indented one; the
    // indent itself is `enter_between_pair`'s own unit test's job.
    save_and_sync(&ide, &mcp, tab_id);
    assert_eq!(
        buffer(&mcp, tab_id),
        original.replacen("fn empty() {}", "fn empty() {\n\n}", 1),
        "Enter between `{{` and `}}` did not open a three-line block"
    );

    assert_eq!(ide.quit(), 0);
}

/// R2: CamelHumps ranking (typing `fBr` finds `fooBar`, which has a hump
/// starting with `f`, but not `objBarrel`, which has none) and a snippet's
/// tab stops (accepting lands the caret on `$1`, and Tab moves it to `$0`) —
/// both through the same stub server the completion half of
/// `e2e_replace_all_and_completion_are_one_undo_each` already routes.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_completion_ranks_camel_humps_and_walks_snippet_tab_stops() {
    let name = "e2e_completion_ranks_camel_humps_and_walks_snippet_tab_stops";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");

    // Line 1 (the blank line right after "mod greeting;"): the stub's own
    // snippet item, `map(${1:f})$0`. "map" sorts before "max" (neither has
    // a sortText, so the label breaks the tie), so it is the default row.
    ide.key("ctrl+Home");
    ide.key("Down");
    let mark = ide.mark();
    ide.key("ctrl+space");
    let shown = ide.wait_for_ev(mark, "completion_shown");
    assert_eq!(shown["count"].as_u64(), Some(2), "the stub's line-1 items");
    ide.key("Return");
    ide.wait_for_ev(mark, "edits_applied");
    ide.sync(&mcp);

    // "map(f)" was spliced in at column 0; the caret should now sit on the
    // "f" default text of `$1`, columns 4..5.
    let after_accept = mcp.call("get_cursor_position", json!({"tab_id": tab_id}));
    assert_eq!(
        (
            after_accept["line"].as_u64(),
            after_accept["column"].as_u64()
        ),
        (Some(2), Some(5)),
        "the caret did not land on the snippet's first tab stop"
    );

    ide.key("Tab");
    ide.sync(&mcp);
    let after_tab = mcp.call("get_cursor_position", json!({"tab_id": tab_id}));
    assert_eq!(
        (after_tab["line"].as_u64(), after_tab["column"].as_u64()),
        (Some(2), Some(6)),
        "Tab did not move the caret from $1 to $0"
    );

    // The blank line between the two functions (real line 6): the stub's
    // CamelHumps fixture. Typing "fBr" and asking explicitly must find only
    // "fooBar" — "objBarrel" has no hump `f` can start.
    ide.key("ctrl+End");
    ide.key("Up");
    ide.key("Up");
    let mark = ide.mark();
    ide.type_text("fBr");
    ide.key("ctrl+space");
    let shown = ide.wait_for_ev(mark, "completion_shown");
    assert_eq!(
        shown["count"].as_u64(),
        Some(1),
        "only fooBar CamelHumps-matches \"fBr\""
    );
    ide.key("Escape");

    // Save before quitting: an unsaved tab prompts on Ctrl+Q, and this flow
    // deliberately left one dirty (the snippet accept and the typed "fBr"),
    // same as every other flow in this file that reaches a quit.
    save_and_sync(&ide, &mcp, tab_id);
    assert_eq!(ide.quit(), 0);
}

/// Ctrl+S, then wait for the tab to go clean — what every buffer assertion
/// needs first, since `read_buffer` answers from the rope and that is one
/// save behind the live widget.
fn save_and_sync(ide: &Ide, mcp: &Mcp, tab_id: u64) {
    let mark = ide.mark();
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the tab to go clean after saving", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(mcp);
}

/// One Ctrl+Z, then a save so `read_buffer` sees the result — the "after
/// one undo" half every one-undo flow asserts.
fn undo_once_and_save(ide: &Ide, mcp: &Mcp, tab_id: u64) {
    let mark = ide.mark();
    ide.key("ctrl+z");
    ide.wait_for_event(mark, "the tab to go dirty again", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == true
    });
    ide.key("ctrl+s");
    ide.wait_for_event(mark, "the undone tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["tab_id"].as_u64() == Some(tab_id) && e["dirty"] == false
    });
    ide.sync(mcp);
}

/// A Composer project (PSR-4 `App\` -> `src/`) with a two-property class,
/// a directory to create into, a scratch script and a template.
fn php_editing_fixture() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp PHP project");
    let write = |relative: &str, text: &str| {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).expect("directory");
        std::fs::write(path, text).expect("fixture file");
    };
    write(
        "composer.json",
        r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
    );
    write(
        "src/User.php",
        "<?php\n\nnamespace App;\n\nclass User\n{\n    private string $name;\n    public readonly int $id;\n}\n",
    );
    write(
        "src/Sub/Marker.php",
        "<?php\n\nnamespace App\\Sub;\n\nclass Marker\n{\n}\n",
    );
    write("scratch.php", "<?php\n\n$xs = [1, 2];\necho 'hi';\n");
    write("view.php", "<h1>Hello</h1>\n<?php echo 1; ?>\n");
    dir
}

/// Click the entry of `menu_event` whose label starts with `prefix`.
fn click_menu_entry(ide: &Ide, mark: Mark, menu_event: &str, prefix: &str) {
    let entry = ide.wait_for_event(mark, &format!("a `{prefix}` entry in {menu_event}"), |e| {
        e["ev"] == menu_event
            && e["label"]
                .as_str()
                .is_some_and(|l| l.replace('&', "").starts_with(prefix))
    });
    let (x, y) = rect_centre(&entry["rect"]);
    ide.click_at(x, y, 1);
}

/// PHP parity G (and Y): the editor-only gestures a PHP developer reaches for.
///
/// - Alt+Insert > Getters and Setters on a class with a readonly property
///   (no setter for it).
/// - File > New > PHP Class in `src/Sub` creates `App\Sub\Thing`.
/// - Ctrl+Alt+T surrounds the selected line with `if`.
/// - `$xs.foreach` + Tab expands the postfix template.
/// - Ctrl+/ on an HTML line of a `.php` file writes an HTML comment.
#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_php_generate_templates_and_new_class() {
    let name = "e2e_php_generate_templates_and_new_class";
    let project = php_editing_fixture();
    let mut ide = Ide::launch(name, APP, project.path());
    drop(project);
    ide.wait_for_ev(Mark::start(), "project_opened");
    let mcp = ide.mcp();
    wait_for_index(&mcp);

    // Alt+Insert > Getters and Setters.
    let tab = open_file(&ide, "User.php");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");
    for _ in 0..6 {
        ide.key("Down"); // inside the class body
    }
    let mark = ide.mark();
    ide.key("alt+Insert");
    ide.wait_for_event(mark, "the Generate menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "generate_menu"
    });
    click_menu_entry(&ide, mark, "generate_menu_action", "Getters and Setters");
    let shown = ide.wait_for_event(mark, "the member picker to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "generate_members"
    });
    assert_eq!(shown["items"].as_array().map(Vec::len), Some(2), "{shown}");
    ide.focus_window(shown["window"].as_str().expect("the dialog's window"));
    ide.key("Return");
    ide.wait_for_event(mark, "the picker to accept", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "generate_members" && e["accepted"] == true
    });
    ide.focus_main();
    save_and_sync(&ide, &mcp, tab_id);
    let user = buffer(&mcp, tab_id);
    assert!(user.contains("public function getName(): string"), "{user}");
    assert!(
        user.contains("public function setName(string $name): void"),
        "{user}"
    );
    assert!(user.contains("public function getId(): int"), "{user}");
    assert!(
        !user.contains("setId"),
        "a readonly property has no setter:\n{user}"
    );

    // File > New > PHP Class, created in the selected `src/Sub`.
    let root = ide.project_root().to_path_buf();
    crate::support::settle(&ide);
    let latest_row = |suffix: &str| {
        ide.events_since_of(Mark::start(), "project_tree_row")
            .into_iter()
            .rfind(|e| e["path"].as_str().is_some_and(|p| p.ends_with(suffix)))
            .unwrap_or_else(|| panic!("no tree row for {suffix}"))
    };
    let (x, y) = rect_centre(&latest_row("/src")["rect"]);
    let mark = ide.mark();
    ide.double_click_at(x, y, 1);
    ide.wait_for_event(mark, "the Sub row after expanding src", |e| {
        e["ev"] == "project_tree_row" && e["path"].as_str().is_some_and(|p| p.ends_with("/src/Sub"))
    });
    let (x, y) = rect_centre(&latest_row("/src/Sub")["rect"]);
    ide.click_at(x, y, 1);
    ide.focus_main();
    let mark = ide.mark();
    ide.key("alt+f");
    click_menu_entry(&ide, mark, "file_menu_action", "New");
    click_menu_entry(&ide, mark, "new_menu_action", "PHP Class");
    let prompt = ide.wait_for_event(mark, "the name prompt to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "input_dialog"
    });
    ide.focus_window(prompt["window"].as_str().expect("the prompt's window"));
    ide.type_text("Thing");
    ide.key("Return");
    ide.wait_for_event(mark, "a tab for Thing.php", |e| {
        e["ev"] == "tab_added" && e["title"] == "Thing.php"
    });
    // Creating a file is not an external change: no "modified outside the
    // editor" prompt may follow it.
    ide.focus_main();
    let thing = std::fs::read_to_string(root.join("src/Sub/Thing.php")).expect("the new class");
    assert!(thing.contains("namespace App\\Sub;"), "{thing}");
    assert!(thing.contains("class Thing"), "{thing}");

    // Ctrl+Alt+T: surround the selected line with `if`; `$xs.foreach` + Tab.
    let tab = open_file(&ide, "scratch.php");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");
    ide.key("Down");
    ide.key("Down");
    ide.key("Down"); // `echo 'hi';`
    ide.key("shift+End");
    let mark = ide.mark();
    ide.key("ctrl+alt+t");
    ide.wait_for_event(mark, "the Surround With menu to open", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "live_templates_menu"
    });
    click_menu_entry(&ide, mark, "live_templates_menu_action", "if —");
    ide.wait_for_event(mark, "the Surround With menu to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "live_templates_menu"
    });
    ide.focus_main();
    // The template's snippet session owns Tab until it ends.
    ide.key("Escape");
    ide.key("ctrl+End");
    // The `shift+End` above and the `$` below are two lone Shift presses: any
    // gap under `IdeMainWindow`'s 300 ms double-Shift window opens Search
    // Everywhere, which swallows the typing.
    std::thread::sleep(std::time::Duration::from_millis(400));
    ide.type_text("$xs.foreach");
    ide.key("Tab");
    save_and_sync(&ide, &mcp, tab_id);
    let scratch = buffer(&mcp, tab_id);
    assert!(scratch.contains("if ("), "{scratch}");
    assert!(scratch.contains("echo 'hi';"), "{scratch}");
    assert!(scratch.contains("foreach ($xs as"), "{scratch}");

    // Ctrl+/ on the HTML line of a `.php` file.
    let tab = open_file(&ide, "view.php");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    ide.key("ctrl+Home");
    ide.key("ctrl+slash");
    save_and_sync(&ide, &mcp, tab_id);
    assert!(
        buffer(&mcp, tab_id).starts_with("<!-- <h1>Hello</h1> -->"),
        "{:?}",
        buffer(&mcp, tab_id)
    );

    assert_eq!(ide.quit(), 0);
}
