//! R3's end-to-end flow: quick documentation composes the LSP hover and
//! every diagnostic covering the caret into one popup.
//!
//! `lsp-core`'s X2 stub server plays the language server: it publishes one
//! canned diagnostic on `textDocument/didOpen` (line 0, columns 0-4) and
//! answers `textDocument/hover` for line 0 with `MarkupContent` Markdown, so
//! opening the fixture's `main.rs` and asking for documentation at (0,0) —
//! Ctrl+Alt+Q, not a simulated mouse dwell, which has no reliable timing
//! under headless Xvfb — exercises both halves of the hover card in
//! one flow: the server's own answer and the diagnostic beside it.

use std::time::{Duration, Instant};

use e2e::{mcp::Mcp, Ide, Mark};
use serde_json::{json, Value};

use crate::support::{
    buffer, fixture, fixture_text, open_file, rect_centre, route_rust_at_stub,
    route_rust_at_stub_with, wait_for_index, APP,
};

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_quick_documentation_composes_hover_and_diagnostic() {
    let name = "e2e_quick_documentation_composes_hover_and_diagnostic";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    open_file(&ide, "main.rs");
    ide.key("ctrl+Home");

    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the quick-documentation popup to show", |e| {
        e["ev"] == "hover_popup_shown"
    });
    let html = shown["html"].as_str().expect("html");
    assert!(
        // `markdown_preview::render` syntax-highlights the fenced block, so
        // `fn` and `main()` land in separate tags rather than one run of
        // plain text.
        html.contains("main()") && html.contains("entry"),
        "the server's hover markdown should render: {html}"
    );
    assert!(
        html.contains("canned diagnostic"),
        "the diagnostic covering (0,0) should be composed in: {html}"
    );

    assert_eq!(ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_fills_in_the_fix_row_when_the_server_answers() {
    let name = "e2e_hover_card_fills_in_the_fix_row_when_the_server_answers";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    open_file(&ide, "main.rs");
    ide.key("ctrl+Home");

    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the card to show with its fixes loading", |e| {
        e["ev"] == "hover_popup_shown"
    });
    assert!(
        shown["html"]
            .as_str()
            .expect("html")
            .contains("Looking for fixes"),
        "fixes are asked for after the card is up: {shown}"
    );

    let updated = ide.wait_for_event(mark, "the fix row to arrive", |e| {
        e["ev"] == "hover_popup_updated"
    });
    let html = updated["html"].as_str().expect("html");
    assert!(
        html.contains("href=\"ide:fix/0\"") && html.contains("Import `HashMap`"),
        "the preferred quick fix should be the fix link: {html}"
    );
    assert!(
        html.contains("ide:more/0"),
        "and More actions follows it: {html}"
    );
    assert!(!html.contains("Looking for fixes"));

    // Screenshot for the PR, only when asked for.
    if let Ok(path) = std::env::var("IDE_SHOT") {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::process::Command::new("import")
            .args(["-window", "root", &path])
            .status();
    }
    assert_eq!(ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_gains_a_source_footer_from_the_index() {
    let name = "e2e_hover_card_gains_a_source_footer_from_the_index";
    let mut ide = Ide::launch(name, APP, fixture("tiny"));
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub(&mut ide);

    let mcp = ide.mcp();
    wait_for_index(&mcp);
    open_file(&ide, "main.rs");
    // Line 4 is `    println!("{}", greeting::greet("world"));`: the end of the
    // line, then 14 back lands inside `greet`.
    ide.key("ctrl+Home");
    for _ in 0..3 {
        ide.key("Down");
    }
    ide.key("End");
    for _ in 0..14 {
        ide.key("Left");
    }

    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the card to show", |e| e["ev"] == "hover_popup_shown");
    assert!(
        !shown["html"].as_str().expect("html").contains("Source:"),
        "the card is not held up by the index: {shown}"
    );

    let declaration = fixture_text("tiny", "src/greeting.rs")
        .lines()
        .position(|line| line.contains("fn greet("))
        .expect("the fixture declares greet")
        + 1;
    let footer = format!("src/greeting.rs:{declaration}");
    let updated = ide.wait_for_event(mark, "the source footer to arrive", |e| {
        e["ev"] == "hover_popup_updated"
            && e["html"].as_str().is_some_and(|h| h.contains("Source:"))
    });
    let html = updated["html"].as_str().expect("html");
    assert!(
        html.contains("href=\"ide:source\"") && html.contains(&footer),
        "the footer names the declaration project-relative: {html}"
    );

    if let Ok(path) = std::env::var("IDE_SHOT") {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::process::Command::new("import")
            .args(["-window", "root", &path])
            .status();
    }
    assert_eq!(ide.quit(), 0);
}

// ---------------------------------------------------------------------------
// H7 acceptance flows. The stub server plays the language server (its
// `STUB_LSP_*` knobs shape the fix row); the card is driven with the real
// pointer and keyboard, and observed through the popup's markers, which carry
// its rect and the click point of every anchor in it.
// ---------------------------------------------------------------------------

const IMPORT: &str = "use std::collections::HashMap;";

/// One launched app on the `tiny` fixture with `main.rs` open, its language
/// server the stub.
struct Session {
    ide: Ide,
    mcp: Mcp,
    tab_id: u64,
}

fn session(
    name: &str,
    envs: &[(&str, &str)],
    tweak: impl FnOnce(&mut app_config::Settings),
) -> Session {
    let mut ide = Ide::launch_with_env(name, APP, fixture("tiny"), envs);
    ide.wait_for_ev(Mark::start(), "project_opened");
    route_rust_at_stub_with(&mut ide, tweak);
    let mcp = ide.mcp();
    wait_for_index(&mcp);
    let tab = open_file(&ide, "main.rs");
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    Session { ide, mcp, tab_id }
}

fn ints(value: &Value) -> Vec<i32> {
    value
        .as_array()
        .expect("an array")
        .iter()
        .map(|v| v.as_i64().expect("an integer") as i32)
        .collect()
}

/// Where to click the anchor `href` of a popup marker.
fn link(marker: &Value, href: &str) -> (i32, i32) {
    let found = marker["links"]
        .as_array()
        .expect("links")
        .iter()
        .find(|l| l["href"] == href)
        .unwrap_or_else(|| panic!("no `{href}` link in {marker}"));
    (
        found["x"].as_i64().unwrap() as i32,
        found["y"].as_i64().unwrap() as i32,
    )
}

/// The stub's import fix landed at the top of the buffer since `mark`.
fn assert_import_applied(s: &Session, mark: Mark) {
    s.ide.wait_for_ev(mark, "workspace_edit_applied");
    s.ide.focus_main();
    s.ide.sync(&s.mcp);
    let text = buffer(&s.mcp, s.tab_id);
    assert!(
        text.starts_with(IMPORT),
        "the import is not at the top:\n{text}"
    );
    // A dirty tab would hold Ctrl+Q behind the unsaved-changes prompt.
    s.ide.key("ctrl+s");
    s.ide.wait_for_event(mark, "the tab to be saved", |e| {
        e["ev"] == "tab_dirty" && e["dirty"] == false
    });
}

fn has_fix_row(marker: &Value) -> bool {
    marker["html"]
        .as_str()
        .is_some_and(|h| h.contains("ide:fix/0"))
}

/// `$IDE_THEME` (`light`, `dark`) as the active theme, for the PR screenshots.
fn themed(settings: &mut app_config::Settings) {
    if let Ok(theme) = std::env::var("IDE_THEME") {
        settings.theme = theme;
    }
}

/// The card shows all of its document: nothing clipped below, no bar needed.
fn assert_fits(marker: &Value) {
    let doc = ints(&marker["doc"]);
    assert!(
        doc[1] <= doc[3],
        "the document ({}px) is taller than the card's viewport ({}px)",
        doc[1],
        doc[3]
    );
    assert!(
        doc[0] <= doc[2],
        "the document ({}px) is wider than the card's viewport ({}px)",
        doc[0],
        doc[2]
    );
}

/// The caret inside `greet` on the fixture's call line (line 4: the end of
/// the line, then 14 back).
fn put_caret_in_greet(ide: &Ide) {
    ide.key("ctrl+Home");
    for _ in 0..3 {
        ide.key("Down");
    }
    ide.key("End");
    for _ in 0..14 {
        ide.key("Left");
    }
}

/// A screenshot of the whole screen to the path in `$var`, when it is set —
/// for the PR, not an assertion.
fn shot(ide: &Ide, var: &str) {
    if let Ok(path) = std::env::var(var) {
        let _ = ide;
        std::thread::sleep(Duration::from_millis(500));
        let _ = std::process::Command::new("import")
            .args(["-window", "root", &path])
            .status();
    }
}

/// Ctrl+Alt+Q at the caret (0,0): the card, pinned, with its fix row arrived.
fn pinned_card(ide: &Ide) -> (Value, Value) {
    ide.key("ctrl+Home");
    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the card to show", |e| e["ev"] == "hover_popup_shown");
    let updated = ide.wait_for_event(mark, "the fix row", |e| {
        e["ev"] == "hover_popup_updated" && has_fix_row(e)
    });
    (shown, updated)
}

/// Park the pointer inside the editor, then move it onto the word at
/// `anchor`: a real dwell start. Returns when the pointer arrived.
fn move_onto_word(ide: &Ide, anchor: &[i32]) -> Instant {
    ide.mouse_move(anchor[0] + 300, anchor[1] + anchor[3] * 6);
    let started = Instant::now();
    ide.mouse_move(anchor[0] + 6, anchor[1] + anchor[3] / 2);
    started
}

/// The word under the caret (0,0), found with a Ctrl+Alt+Q probe that is
/// closed again.
fn probe_anchor(ide: &Ide) -> Vec<i32> {
    ide.key("ctrl+Home");
    let mark = ide.mark();
    ide.key("ctrl+alt+q");
    let shown = ide.wait_for_event(mark, "the probe card", |e| e["ev"] == "hover_popup_shown");
    ide.key("Escape");
    ide.wait_for_ev(mark, "hover_popup_hidden");
    ints(&shown["anchor"])
}

/// Dwell on the word and wait for the card (and, when `with_fix`, its fixes).
fn dwell_card(ide: &Ide, anchor: &[i32]) -> (Value, Duration) {
    let mark = ide.mark();
    let started = move_onto_word(ide, anchor);
    let shown = ide.wait_for_event(mark, "the dwell card", |e| e["ev"] == "hover_popup_shown");
    (shown, started.elapsed())
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_lays_out_problem_fix_signature_docs_source_in_order() {
    let mut s = session(
        "e2e_hover_card_lays_out_problem_fix_signature_docs_source_in_order",
        &[("STUB_LSP_GREET_DIAGNOSTIC", "1")],
        themed,
    );
    put_caret_in_greet(&s.ide);
    let mark = s.ide.mark();
    s.ide.key("ctrl+alt+q");
    let shown = s
        .ide
        .wait_for_event(mark, "the card", |e| e["ev"] == "hover_popup_shown");
    // Fix and footer both arrive after the card is up; wait for the last.
    let full = s
        .ide
        .wait_for_event(mark, "the fix row and the source footer", |e| {
            e["ev"] == "hover_popup_updated"
                && has_fix_row(e)
                && e["html"].as_str().is_some_and(|h| h.contains("Source:"))
        });
    shot(&s.ide, "IDE_SHOT_CARD");

    let html = full["html"].as_str().unwrap();
    let at = |needle: &str| {
        html.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {html}"))
    };
    assert!(at("greet is deprecated") < at("ide:fix/0"), "{html}");
    assert!(at("ide:fix/0") < at("ide:more/0"), "{html}");
    assert!(at("ide:more/0") < at("class=\"signature\""), "{html}");
    assert!(at("class=\"signature\"") < at("class=\"doc\""), "{html}");
    assert!(at("class=\"doc\"") < at("class=\"source\""), "{html}");
    assert!(
        html.contains("W0042"),
        "the diagnostic code is shown: {html}"
    );
    assert!(
        html.contains("class=\"fixrow\""),
        "the fix row is its own indented block"
    );
    assert!(
        html.contains("<span style=\"color:#"),
        "the signature is syntax-coloured: {html}"
    );

    // Anchored under the word, on screen, within the size cap.
    let (rect, anchor) = (ints(&full["rect"]), ints(&shown["anchor"]));
    assert!(
        rect[1] >= anchor[1] + anchor[3],
        "below the word: {rect:?} {anchor:?}"
    );
    assert!(rect[2] > 240 && rect[2] <= 560, "card width {rect:?}");
    assert!(
        rect[0] >= 0 && rect[0] + rect[2] <= 1600,
        "on screen {rect:?}"
    );
    assert_fits(&full);
    s.ide.focus_main();
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_applies_its_fix_from_a_click_and_the_problem_clears() {
    let mut s = session(
        "e2e_hover_card_applies_its_fix_from_a_click_and_the_problem_clears",
        &[("STUB_LSP_CLEAR_ON_CHANGE", "1")],
        |_| {},
    );
    let (_, updated) = pinned_card(&s.ide);
    assert!(!buffer(&s.mcp, s.tab_id).contains(IMPORT));

    let (x, y) = link(&updated, "ide:fix/0");
    let mark = s.ide.mark();
    s.ide.click_at(x, y, 1);
    s.ide.wait_for_ev(mark, "hover_popup_hidden");
    assert_import_applied(&s, mark);
    s.ide.wait_for_event(mark, "the squiggle to clear", |e| {
        e["ev"] == "diagnostics_applied" && e["count"] == 0
    });
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_apply_preferred_fix_shortcut_uses_the_caret_without_a_card() {
    let mut s = session(
        "e2e_apply_preferred_fix_shortcut_uses_the_caret_without_a_card",
        &[("STUB_LSP_CLEAR_ON_CHANGE", "1")],
        |_| {},
    );
    s.ide.key("ctrl+Home");
    let mark = s.ide.mark();
    s.ide.key("alt+shift+Return");
    assert_import_applied(&s, mark);
    s.ide.wait_for_event(mark, "the squiggle to clear", |e| {
        e["ev"] == "diagnostics_applied" && e["count"] == 0
    });
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_apply_preferred_fix_shortcut_prefers_the_visible_card() {
    let mut s = session(
        "e2e_apply_preferred_fix_shortcut_prefers_the_visible_card",
        &[],
        |_| {},
    );
    let anchor = probe_anchor(&s.ide);
    // The caret goes to a line with no problem of its own: without the card
    // there would be nothing to apply.
    for _ in 0..4 {
        s.ide.key("Down");
    }
    let mark = s.ide.mark();
    dwell_card(&s.ide, &anchor);
    s.ide.wait_for_event(mark, "the fix row", |e| {
        e["ev"] == "hover_popup_updated" && has_fix_row(e)
    });
    s.ide.key("alt+shift+Return");
    assert_import_applied(&s, mark);
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_closes_on_escape_typing_click_outside_and_scroll() {
    let mut s = session(
        "e2e_hover_card_closes_on_escape_typing_click_outside_and_scroll",
        &[],
        |_| {},
    );
    // Long enough to scroll; the caret then returns to the top.
    let text = buffer(&s.mcp, s.tab_id);
    s.mcp.call(
        "edit_buffer",
        json!({ "tab_id": s.tab_id, "content": format!("{text}{}", "\n".repeat(60)) }),
    );
    s.ide.sync(&s.mcp);
    s.ide.key("ctrl+Home");
    let anchor = probe_anchor(&s.ide);

    for trigger in ["Escape", "typing", "click outside", "scroll"] {
        let (_, _) = dwell_card(&s.ide, &anchor);
        let mark = s.ide.mark();
        match trigger {
            "Escape" => s.ide.key("Escape"),
            "typing" => s.ide.key("Right"),
            "click outside" => s.ide.click_at(700, 620, 1),
            _ => {
                for _ in 0..3 {
                    s.ide.click(5);
                }
            }
        }
        s.ide
            .wait_for_event(mark, &format!("the card to close on {trigger}"), |e| {
                e["ev"] == "hover_popup_hidden"
            });
        // Back to the top for the next round.
        s.ide.focus_main();
        s.ide.key("ctrl+Home");
    }
    // The padding was an unsaved edit; saved, quitting asks nothing.
    s.mcp.call("save_buffer", json!({ "tab_id": s.tab_id }));
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_stays_while_the_pointer_travels_in_and_closes_after_it_leaves() {
    let mut s = session(
        "e2e_hover_card_stays_while_the_pointer_travels_in_and_closes_after_it_leaves",
        &[],
        |_| {},
    );
    let anchor = probe_anchor(&s.ide);
    let (shown, _) = dwell_card(&s.ide, &anchor);
    let rect = ints(&shown["rect"]);

    // Travel from the word into the card in steps, as a hand would.
    let mark = s.ide.mark();
    let (from, to) = (
        (anchor[0] + 6, anchor[1] + anchor[3] / 2),
        (rect[0] + 40, rect[1] + rect[3] / 2),
    );
    for step in 1..=8 {
        s.ide.mouse_move(
            from.0 + (to.0 - from.0) * step / 8,
            from.1 + (to.1 - from.1) * step / 8,
        );
    }
    // Well past the 300 ms grace: still open.
    std::thread::sleep(Duration::from_millis(900));
    assert!(
        s.ide.events_since_of(mark, "hover_popup_hidden").is_empty(),
        "the card must stay open with the pointer inside it"
    );

    // Out of the card, away from the word: closes after the grace.
    let left = Instant::now();
    s.ide.mouse_move(700, 620);
    s.ide
        .wait_for_event(mark, "the card to close once the pointer left", |e| {
            e["ev"] == "hover_popup_hidden"
        });
    assert!(
        left.elapsed() >= Duration::from_millis(200),
        "not before the grace: {:?}",
        left.elapsed()
    );
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_opens_after_the_dwell_delay_under_the_word() {
    let mut s = session(
        "e2e_hover_card_opens_after_the_dwell_delay_under_the_word",
        &[],
        |_| {},
    );
    let anchor = probe_anchor(&s.ide);
    let (shown, waited) = dwell_card(&s.ide, &anchor);
    // Default delay 500 ms; generous both ways, the harness adds latency.
    assert!(
        waited >= Duration::from_millis(350),
        "not instant: {waited:?}"
    );
    assert!(waited < Duration::from_secs(5), "but prompt: {waited:?}");
    let rect = ints(&shown["rect"]);
    assert!(
        rect[1] >= anchor[1] + anchor[3],
        "under the word: {rect:?} {anchor:?}"
    );
    assert!(
        rect[0] >= anchor[0] - 2 && rect[0] < anchor[0] + 40,
        "at the word: {rect:?} {anchor:?}"
    );
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_delay_setting_is_honoured() {
    let mut s = session("e2e_hover_delay_setting_is_honoured", &[], |settings| {
        settings.hover.delay_ms = 1500;
    });
    let anchor = probe_anchor(&s.ide);
    let (_, waited) = dwell_card(&s.ide, &anchor);
    assert!(
        waited >= Duration::from_millis(1200),
        "delay 1500 ms: {waited:?}"
    );
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_source_footer_jumps_to_the_declaration() {
    let mut s = session(
        "e2e_hover_source_footer_jumps_to_the_declaration",
        &[],
        |_| {},
    );
    // Line 4 is `    println!("{}", greeting::greet("world"));`: the end of the
    // line, then 14 back lands inside `greet`.
    s.ide.key("ctrl+Home");
    for _ in 0..3 {
        s.ide.key("Down");
    }
    s.ide.key("End");
    for _ in 0..14 {
        s.ide.key("Left");
    }
    let mark = s.ide.mark();
    s.ide.key("ctrl+alt+q");
    let updated = s.ide.wait_for_event(mark, "the source footer", |e| {
        e["ev"] == "hover_popup_updated"
            && e["html"].as_str().is_some_and(|h| h.contains("Source:"))
    });
    let greeting = fixture_text("tiny", "src/greeting.rs");
    let declaration = greeting
        .lines()
        .position(|line| line.contains("fn greet("))
        .expect("the fixture declares greet");
    let column = greeting
        .lines()
        .nth(declaration)
        .unwrap()
        .find("greet")
        .unwrap();
    let (x, y) = link(&updated, "ide:source");
    let mark = s.ide.mark();
    s.ide.click_at(x, y, 1);
    let tab = s.ide.wait_for_event(mark, "greeting.rs to open", |e| {
        e["ev"] == "tab_added" && e["title"] == "greeting.rs"
    });
    let tab_id = tab["tab_id"].as_u64().expect("tab_id");
    // The caret lands on the declaration's name (the tab opens at 1:0, so the
    // column is what shows the jump happened).
    let want = (declaration as u32 + 1, column as u32); // 1-based line
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = crate::support::cursor(&s.mcp, tab_id);
        if got == want {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "caret is at {got:?}, want {want:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    s.ide.focus_main();
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_settings_choose_what_the_card_shows() {
    for (docs, problems) in [(false, true), (true, false)] {
        let name = format!("e2e_hover_settings_docs_{docs}_problems_{problems}");
        let mut s = session(&name, &[], |settings| {
            settings.hover.docs_on_hover = docs;
            settings.hover.problems_on_hover = problems;
        });
        let anchor = probe_anchor(&s.ide);
        let (shown, _) = dwell_card(&s.ide, &anchor);
        let html = shown["html"].as_str().unwrap();
        assert_eq!(
            html.contains("canned diagnostic"),
            problems,
            "problems={problems}: {html}"
        );
        assert_eq!(html.contains("entry"), docs, "docs={docs}: {html}");
        assert_eq!(s.ide.quit(), 0);
    }
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_overflow_menu_lists_its_actions_and_toggle_persists() {
    let mut s = session(
        "e2e_hover_overflow_menu_lists_its_actions_and_toggle_persists",
        &[],
        |_| {},
    );
    let (_, updated) = pinned_card(&s.ide);
    let menu = ints(&updated["menu"]);
    let mark = s.ide.mark();
    s.ide.click_at(menu[0], menu[1], 1);
    s.ide.wait_for_event(mark, "the overflow menu", |e| {
        e["ev"] == "hover_menu_action"
    });
    std::thread::sleep(Duration::from_millis(300));
    let entries = s.ide.events_since_of(mark, "hover_menu_action");
    let labels: Vec<&str> = entries
        .iter()
        .map(|e| e["label"].as_str().unwrap())
        .collect();
    let bare = |l: &str| l.split('\t').next().unwrap().to_string();
    assert_eq!(
        labels.iter().map(|l| bare(l)).collect::<Vec<_>>(),
        [
            "Pin",
            "Copy",
            "Go to Declaration",
            "Show on Mouse Hover",
            "Hover Settings…"
        ]
    );
    let toggle = entries
        .iter()
        .find(|e| e["label"] == "Show on Mouse Hover")
        .unwrap();
    let (x, y) = rect_centre(&toggle["rect"]);
    s.ide.click_at(x, y, 1);
    e2e::wait_for("the toggle to be saved", || {
        (!app_config::load(&s.ide.config_dir())
            .unwrap()
            .hover
            .docs_on_hover)
            .then_some(())
    });
    // Live: the very next dwell shows problems only.
    s.ide.key("Escape");
    let anchor = ints(&updated["anchor"]);
    let (shown, _) = dwell_card(&s.ide, &anchor);
    assert!(!shown["html"].as_str().unwrap().contains("entry"));
    // Without a window manager a closed popup leaves no window focused.
    s.ide.focus_main();
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_more_actions_opens_the_intentions_menu_under_the_card() {
    let mut s = session(
        "e2e_more_actions_opens_the_intentions_menu_under_the_card",
        &[],
        |_| {},
    );
    let (_, updated) = pinned_card(&s.ide);
    let rect = ints(&updated["rect"]);
    let (x, y) = link(&updated, "ide:more/0");
    let mark = s.ide.mark();
    s.ide.click_at(x, y, 1);
    let shown = s.ide.wait_for_event(mark, "the intentions menu", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "intentions_menu"
    });
    assert_eq!(
        shown["x"].as_i64().unwrap() as i32,
        rect[0],
        "left-aligned with the card"
    );
    assert!(
        shown["y"].as_i64().unwrap() as i32 >= rect[1] + rect[3] - 2,
        "under the card"
    );
    let entries = s.ide.events_since_of(mark, "intentions_menu_action");
    assert!(
        entries.iter().any(|e| e["label"]
            .as_str()
            .is_some_and(|l| l.starts_with("Import `HashMap`"))),
        "{entries:?}"
    );
    s.ide.key("Escape");
    s.ide.wait_for_event(mark, "the menu to close", |e| {
        e["ev"] == "dialog_closed" && e["name"] == "intentions_menu"
    });
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_shows_loading_then_fix_and_drops_a_stale_answer() {
    let mut s = session(
        "e2e_hover_card_shows_loading_then_fix_and_drops_a_stale_answer",
        &[("STUB_LSP_FIX_DELAY_MS", "1500")],
        |_| {},
    );
    s.ide.key("ctrl+Home");
    let mark = s.ide.mark();
    s.ide.key("ctrl+alt+q");
    let shown = s
        .ide
        .wait_for_event(mark, "the card", |e| e["ev"] == "hover_popup_shown");
    assert!(shown["html"]
        .as_str()
        .unwrap()
        .contains("Looking for fixes"));
    // Closed before the server answers: the late answer must not resurface.
    s.ide.key("Escape");
    s.ide.wait_for_ev(mark, "hover_popup_hidden");
    std::thread::sleep(Duration::from_millis(2500));
    assert!(s
        .ide
        .events_since_of(mark, "hover_popup_updated")
        .is_empty());

    // And uninterrupted, the fix row replaces the loading line.
    let (_, updated) = pinned_card(&s.ide);
    assert!(!updated["html"]
        .as_str()
        .unwrap()
        .contains("Looking for fixes"));
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_card_without_a_fix_offers_no_fix_row() {
    let mut s = session(
        "e2e_hover_card_without_a_fix_offers_no_fix_row",
        &[("STUB_LSP_NO_FIX", "1")],
        |_| {},
    );
    s.ide.key("ctrl+Home");
    let mark = s.ide.mark();
    s.ide.key("ctrl+alt+q");
    s.ide.wait_for_ev(mark, "hover_popup_shown");
    let updated = s
        .ide
        .wait_for_event(mark, "the answer", |e| e["ev"] == "hover_popup_updated");
    let html = updated["html"].as_str().unwrap();
    assert!(
        !html.contains("Looking for fixes") && !html.contains("ide:fix"),
        "{html}"
    );
    assert!(html.contains("canned diagnostic"));
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_bulb_is_red_for_a_fix_and_yellow_for_an_intention() {
    let mut s = session(
        "e2e_bulb_is_red_for_a_fix_and_yellow_for_an_intention",
        &[],
        |_| {},
    );
    // The stub offers only refactorings on the next line: yellow there, red
    // back on the squiggle's own line.
    let mark = s.ide.mark();
    s.ide.key("Down");
    let yellow = s
        .ide
        .wait_for_event(mark, "the intention bulb", |e| e["ev"] == "intention_bulb");
    let mark = s.ide.mark();
    s.ide.key("Up");
    let red = s
        .ide
        .wait_for_event(mark, "the fix bulb", |e| e["ev"] == "intention_bulb");
    assert_eq!(red["fix"], true, "a quick fix at the squiggle: {red}");
    assert_eq!(yellow["fix"], false, "{yellow}");
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_long_signature_stays_inside_the_card_and_clear_of_the_menu_button() {
    let mut s = session(
        "e2e_long_signature_stays_inside_the_card_and_clear_of_the_menu_button",
        &[],
        |_| {},
    );
    // Line 8 (`fn empty`): the stub answers with a signature wider than any card.
    s.ide.key("ctrl+Home");
    for _ in 0..7 {
        s.ide.key("Down");
    }
    s.ide.key("Right");
    let mark = s.ide.mark();
    s.ide.key("ctrl+alt+q");
    let shown = s
        .ide
        .wait_for_event(mark, "the card", |e| e["ev"] == "hover_popup_shown");
    let rect = ints(&shown["rect"]);
    assert!(
        rect[2] == 560,
        "a signature this long fills the card's width: {rect:?}"
    );
    assert!(rect[3] < 400, "and wraps instead of scrolling: {rect:?}");
    assert_fits(&shown);
    // The signature wraps inside the first section, which reserves the ⋮
    // column (`td.sec`'s right padding), so no text runs under the button.
    let html = shown["html"].as_str().unwrap();
    assert!(html.contains("class=\"sec\"") && html.contains("very_long_signature"));
    if let Ok(path) = std::env::var("IDE_SHOT") {
        std::thread::sleep(Duration::from_millis(500));
        let _ = std::process::Command::new("import")
            .args(["-window", "root", &path])
            .status();
    }
    assert_eq!(s.ide.quit(), 0);
}

#[test]
#[ignore = "E2E: needs an X server; run via `make e2e`"]
fn e2e_hover_settings_entry_of_the_overflow_menu_opens_the_settings() {
    let mut s = session(
        "e2e_hover_settings_entry_of_the_overflow_menu_opens_the_settings",
        &[],
        |settings| {
            themed(settings);
            // Both hovers off: the Delay row must be greyed out.
            if std::env::var("IDE_SHOT_SETTINGS").is_ok() {
                settings.hover.docs_on_hover = false;
                settings.hover.problems_on_hover = false;
            }
        },
    );
    let (_, updated) = pinned_card(&s.ide);
    let menu = ints(&updated["menu"]);
    let mark = s.ide.mark();
    s.ide.click_at(menu[0], menu[1], 1);
    s.ide.wait_for_event(mark, "the overflow menu", |e| {
        e["ev"] == "hover_menu_action"
    });
    std::thread::sleep(Duration::from_millis(300));
    let entries = s.ide.events_since_of(mark, "hover_menu_action");
    let settings = entries
        .iter()
        .find(|e| e["label"] == "Hover Settings…")
        .unwrap();
    let (x, y) = rect_centre(&settings["rect"]);
    let mark = s.ide.mark();
    s.ide.click_at(x, y, 1);
    let dialog = s.ide.wait_for_event(mark, "the settings dialog", |e| {
        e["ev"] == "dialog_shown" && e["name"] == "settings_dialog"
    });
    shot(&s.ide, "IDE_SHOT_SETTINGS");
    // The Editor page is the one shown: its category row is the selected one.
    assert!(dialog["editor_category_rect"].is_array());
    s.ide.key("Escape");
    s.ide
        .wait_for_event(mark, "the settings dialog to close", |e| {
            e["ev"] == "dialog_closed" && e["name"] == "settings_dialog"
        });
    s.ide.focus_main();
    assert_eq!(s.ide.quit(), 0);
}
