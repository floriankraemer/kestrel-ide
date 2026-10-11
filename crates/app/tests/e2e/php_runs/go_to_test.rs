//! J9 (Go to Test / Test Subject) and G11.

use super::*;

// --- J9: Go to Test / Test Subject --------------------------------------------

/// What Ctrl+Shift+T answered after `mark`: the chooser's entries, or the
/// file it opened directly, or the status-bar sentence.
enum GoTo {
    Chooser(Vec<Value>),
    Opened(String, u64),
    Status(String),
}

fn go_to_test(ide: &Ide) -> GoTo {
    let mark = ide.mark();
    ide.focus_main();
    ide.key("ctrl+shift+T");
    let first =
        ide.wait_for_event_within(mark, REAL_TOOLCHAIN_TIMEOUT, "Go to Test's answer", |e| {
            e["ev"] == "goto_test_menu_action"
                || e["ev"] == "opened_at"
                || (e["ev"] == "status_message"
                    && e["text"]
                        .as_str()
                        .is_some_and(|t| !t.starts_with("Still indexing")))
        });
    match first["ev"].as_str() {
        Some("goto_test_menu_action") => {
            // Every entry is marked in one go.
            GoTo::Chooser(ide.events_since_of(mark, "goto_test_menu_action"))
        }
        Some("opened_at") => GoTo::Opened(
            first["path"].as_str().unwrap().to_string(),
            first["caret_line"].as_u64().unwrap(),
        ),
        _ => GoTo::Status(first["text"].as_str().unwrap().to_string()),
    }
}

/// The enabled entries' labels of a chooser.
fn choices(menu: &[Value]) -> Vec<String> {
    menu.iter()
        .filter(|e| e["enabled"] == true)
        .filter_map(|e| e["label"].as_str().map(str::to_string))
        .collect()
}

/// J9 (Go to Test / Test Subject) and G11 (a test method lands on the
/// tested method).
#[test]
#[ignore = "E2E: needs an X server and the linux-php image; run via `make test-php`"]
fn e2e_phpunit_go_to_test_and_subject() {
    if !require_php_e2e() {
        return;
    }
    let mut ide = launch_suites("e2e_phpunit_go_to_test_and_subject", None);
    let mut findings = Findings::default();

    // AC9.8 + AC9.1: from the subject, the convention's test answers at
    // once (the covering ones are still being indexed), then the chooser
    // lists MailerTest first and NotifierTest after.
    let mailer_tab = open_file(&ide, "Mailer.php");
    let mark = ide.mark();
    let mut chooser = None;
    for _ in 0..10 {
        match go_to_test(&ide) {
            GoTo::Chooser(menu) => {
                chooser = Some(menu);
                break;
            }
            GoTo::Opened(path, _) => {
                findings.check(
                    path.ends_with("tests/Unit/MailerTest.php"),
                    format!("AC9.8: opened {path}"),
                );
                reopen(&ide, "Mailer.php");
            }
            GoTo::Status(text) => {
                findings.check(false, format!("AC9.1: Go to Test said {text:?}"));
                break;
            }
        }
    }
    if !ide.events_since_of(mark, "opened_at").iter().any(|e| {
        e["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("MailerTest.php"))
    }) {
        // The index was ready on the first press: nothing to note.
    } else {
        findings.check(
            ide.events_since_of(mark, "status_message").iter().any(|e| {
                e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Still indexing"))
            }),
            "AC9.8: the first answer came without the still-indexing note",
        );
    }
    match chooser {
        Some(menu) => {
            let labels = choices(&menu);
            shot("e8_go_to_test_chooser_dark");
            findings.check(
                labels.len() == 2
                    && labels[0].contains("MailerTest")
                    && labels[1].contains("NotifierTest"),
                format!("AC9.1: the chooser lists {labels:?}"),
            );
            // AC9.10: the first entry is preselected (#459), so Enter alone
            // opens it; the title row is no keyboard stop.
            let mark = ide.mark();
            ide.key("Return");
            let line = landing(&ide, mark, "tests/Unit/MailerTest.php");
            findings.check(
                line.is_some(),
                "AC9.10: Enter on the chooser did not open its preselected first entry",
            );
        }
        None => findings.check(false, "AC9.1: no chooser for Mailer"),
    }

    // AC9.6: the Navigate menu names the action after where it leads.
    let label = |ide: &Ide| {
        let mark = ide.mark();
        ide.focus_main();
        ide.key("alt+n");
        let entry = ide.wait_for_event(mark, "Navigate > Test", |e| {
            e["ev"] == "navigate_menu_action"
                && (e["label"] == "Test" || e["label"] == "Test Subject")
        });
        // The second Escape leaves the menu bar, which keeps the keyboard.
        ide.key("Escape");
        ide.key("Escape");
        ide.focus_main();
        entry["label"].as_str().unwrap().to_string()
    };
    // MailerTest.php is in front now.
    findings.check(
        label(&ide) == "Test Subject",
        "AC9.6: in a test the entry is not \"Test Subject\"",
    );

    // G11 + AC9.3: from `testSend`, the subject opens on `send`.
    caret_to(&ide, 14);
    let test_tab = ide
        .events()
        .into_iter()
        .rev()
        .find(|e| {
            e["ev"] == "opened_at"
                && e["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("MailerTest.php"))
        })
        .and_then(|e| e["tab_id"].as_u64())
        .expect("the MailerTest.php tab");
    let caret = crate::support::cursor(&ide.mcp(), test_tab);
    findings.check(
        caret.0 == 13 || caret.0 == 14,
        format!("G11 setup: the caret is at {caret:?}"),
    );
    match go_to_test(&ide) {
        GoTo::Opened(path, line) => {
            findings.check(
                path.ends_with("src/Mailer.php"),
                format!("AC9.3: opened {path}"),
            );
            findings.check(
                line == 8,
                format!("G11: landed on line {line}, not `send` (8)"),
            );
        }
        GoTo::Chooser(menu) => {
            findings.check(false, format!("AC9.3: a chooser {:?}", choices(&menu)));
            ide.key("Escape");
        }
        GoTo::Status(text) => findings.check(false, format!("AC9.3: {text:?}")),
    }
    findings.check(
        label(&ide) == "Test",
        "AC9.6: in a subject the entry is not \"Test\"",
    );

    // AC9.6: the editor's context menu offers Go To > Test.
    let (x, y) = rect_centre(&mailer_tab["rect"]);
    let mark = ide.mark();
    ide.click_at(x + 200, y + 200, 3);
    let go_to = ide.wait_for_event(mark, "the editor's Go To entry", |e| {
        e["ev"] == "editor_context_menu_action" && e["label"] == "Go To"
    });
    let (x, y) = rect_centre(&go_to["rect"]);
    ide.click_at(x, y, 1);
    let entry = answer(&ide, mark, |e| e["ev"] == "editor_go_to_menu_action");
    findings.check(
        entry.as_ref().is_some_and(|e| e["label"] == "Test"),
        format!("AC9.6: the editor's Go To submenu: {entry:?}"),
    );
    ide.key("Escape");
    ide.key("Escape");
    ide.focus_main();

    // AC9.2: exactly one test opens directly.
    open(&ide, "Notifier.php");
    match go_to_test(&ide) {
        GoTo::Opened(path, _) => findings.check(
            path.ends_with("tests/Unit/NotifierTest.php"),
            format!("AC9.2: opened {path}"),
        ),
        GoTo::Chooser(menu) => {
            findings.check(false, format!("AC9.2: a chooser {:?}", choices(&menu)));
            ide.key("Escape");
        }
        GoTo::Status(text) => findings.check(false, format!("AC9.2: {text:?}")),
    }

    // AC9.4: NotifierTest covers Mailer and is named after Notifier.
    match go_to_test(&ide) {
        GoTo::Chooser(menu) => {
            let labels = choices(&menu);
            findings.check(
                labels.iter().any(|l| l.contains("Mailer"))
                    && labels.iter().any(|l| l.contains("Notifier")),
                format!("AC9.4: the subjects of NotifierTest: {labels:?}"),
            );
            ide.key("Escape");
        }
        GoTo::Opened(path, _) => findings.check(false, format!("AC9.4: opened only {path}")),
        GoTo::Status(text) => findings.check(false, format!("AC9.4: {text:?}")),
    }
    ide.focus_main();

    // AC9.5: no test: a status message and nothing else.
    open(&ide, "Orphan.php");
    match go_to_test(&ide) {
        GoTo::Status(text) => findings.check(
            text.contains("No test found for Orphan"),
            format!("AC9.5: {text:?}"),
        ),
        GoTo::Opened(path, _) => findings.check(false, format!("AC9.5: opened {path}")),
        GoTo::Chooser(menu) => {
            findings.check(false, format!("AC9.5: a chooser {:?}", choices(&menu)));
            ide.key("Escape");
        }
    }

    // AC9.7: a covering test created outside the IDE joins the list.
    std::fs::write(
        ide.project_root().join("tests/Unit/PostmanTest.php"),
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace Tests\\Unit;\n\nuse App\\Mailer;\n\
         use PHPUnit\\Framework\\Attributes\\CoversClass;\nuse PHPUnit\\Framework\\TestCase;\n\n\
         #[CoversClass(Mailer::class)]\nfinal class PostmanTest extends TestCase\n{\n    \
         public function testDelivers(): void\n    {\n        $this->assertTrue(true);\n    }\n}\n",
    )
    .expect("PostmanTest.php");
    reopen(&ide, "Mailer.php");
    let mut listed = false;
    for _ in 0..10 {
        match go_to_test(&ide) {
            GoTo::Chooser(menu) => {
                listed = choices(&menu).iter().any(|l| l.contains("PostmanTest"));
                ide.key("Escape");
                ide.focus_main();
                if listed {
                    break;
                }
            }
            GoTo::Opened(..) => reopen(&ide, "Mailer.php"),
            GoTo::Status(_) => break,
        }
    }
    findings.check(
        listed,
        "AC9.7: a covering test created outside the IDE is not listed",
    );

    ide.focus_main();
    assert_eq!(ide.quit(), 0);
    findings.finish();
}
