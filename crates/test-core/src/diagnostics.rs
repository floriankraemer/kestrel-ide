//! A failing test becomes a diagnostic (D3), published into the same
//! `diagnostics_core::DiagnosticStore` a language server or an analyzer
//! publishes into (ADR-0046) — reused, not a second Problems surface for
//! tests. See ADR-0048 for why this belongs to `test-core` rather than
//! `ui-shell` alone: locating *which file* a failure happened in is a rule
//! about test-framework output, and it deserves the same unit tests a
//! parser gets.
//!
//! Neither TeamCity nor JUnit XML gives a failure a structured
//! `file`/`line` the way an LSP diagnostic does — only free-text (a
//! message and a stack trace/diff). [`locate`] recovers a `path:line` from
//! that text with the same `path` half rule `run_core::links` documents
//! (ends in a `.`-extension, `line` half is plain digits), scanning from
//! the last line first since PHPUnit's own trace lists the assertion site
//! last, closest to the failure.
//!
//! ponytail: a naive last-line-first heuristic rather than
//! `run_core::links`' full regex catalogue — this crate cannot depend on
//! `run_core` (layering.md's `test-core` row lists no adapter-layer
//! crates), and a wrong file only means one failing test's squiggle lands
//! in the wrong place, never anything that panics or drops the tree
//! update. Upgrade path: promote `run_core::links`' candidate-finding into
//! a Qt-free crate both can depend on, if this heuristic ever
//! mislocates something that matters.

use std::collections::HashMap;

use diagnostics_core::{Diagnostic, Position, Range, Severity};

use crate::tree::{TestFailure, TestTree};

/// Every currently-failing test's diagnostic, grouped by the file URI it
/// was located in. A failure whose text carries no recognisable
/// `path:line` contributes nothing here — it is still a red row in the
/// Tests dock, just not a squiggle, which is a smaller loss than guessing.
pub fn diagnostics_by_file(
    tree: &TestTree,
    framework_name: &str,
) -> HashMap<String, Vec<Diagnostic>> {
    let mut grouped: HashMap<String, Vec<Diagnostic>> = HashMap::new();
    for node in tree.failing_nodes() {
        let Some(failure) = &node.failure else {
            continue;
        };
        let Some((path, line)) = locate(failure) else {
            continue;
        };
        let uri = diagnostics_core::uri_from_path(&path);
        grouped.entry(uri).or_default().push(Diagnostic {
            range: Range {
                start: Position {
                    line: line.saturating_sub(1),
                    character: 0,
                },
                end: None,
            },
            severity: Severity::Error,
            message: failure.message.clone(),
            source: framework_name.to_string(),
            raw: None,
        });
    }
    grouped
}

/// Find the last `path:line` shaped span in `failure`'s details, falling
/// back to its message. Scanning from the end of the details text first:
/// PHPUnit's own stack trace lists outer frames first and the assertion
/// site last, so the last match is the one worth underlining.
fn locate(failure: &TestFailure) -> Option<(String, u32)> {
    failure
        .details
        .lines()
        .rev()
        .find_map(location_in_line)
        .or_else(|| location_in_line(&failure.message))
}

fn location_in_line(line: &str) -> Option<(String, u32)> {
    let line = line.trim().trim_end_matches(')');
    let colon = line.rfind(':')?;
    let (before, after) = line.split_at(colon);
    let line_no: u32 = after[1..].parse().ok()?;
    let path = before.rsplit(['(', ' ']).next().unwrap_or(before);
    if path.is_empty() || !path.contains('.') {
        return None;
    }
    Some((path.to_string(), line_no))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teamcity::TeamCityEvent;

    fn failing_tree(details: &str) -> TestTree {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestStarted {
            parent: None,
            name: "testAdds".into(),
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent: None,
            name: "testAdds".into(),
            message: "Failed asserting that 1 matches 2.".into(),
            details: details.into(),
        });
        tree
    }

    #[test]
    fn a_trailing_stack_frame_locates_the_file() {
        let tree = failing_tree("at Tests\\GreeterTest::testAdds (GreeterTest.php:20)");
        let grouped = diagnostics_by_file(&tree, "phpunit");
        let uri = diagnostics_core::uri_from_path("GreeterTest.php");
        assert_eq!(grouped[&uri].len(), 1);
        assert_eq!(grouped[&uri][0].range.start.line, 19);
        assert_eq!(grouped[&uri][0].source, "phpunit");
        assert_eq!(
            grouped[&uri][0].message,
            "Failed asserting that 1 matches 2."
        );
    }

    #[test]
    fn a_bare_path_colon_line_with_no_parens_also_locates() {
        let tree = failing_tree("/project/tests/GreeterTest.php:20");
        let grouped = diagnostics_by_file(&tree, "phpunit");
        let uri = diagnostics_core::uri_from_path("/project/tests/GreeterTest.php");
        assert_eq!(grouped[&uri].len(), 1);
    }

    #[test]
    fn a_method_name_with_a_colon_but_no_extension_is_not_mistaken_for_a_path() {
        let tree = failing_tree("Tests\\GreeterTest::testAdds:20");
        let grouped = diagnostics_by_file(&tree, "phpunit");
        assert!(grouped.is_empty());
    }

    #[test]
    fn details_with_no_location_at_all_contributes_nothing() {
        let tree = failing_tree("no location information here");
        let grouped = diagnostics_by_file(&tree, "phpunit");
        assert!(grouped.is_empty());
    }

    #[test]
    fn a_passing_test_contributes_nothing() {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::TestStarted {
            parent: None,
            name: "testOk".into(),
        });
        tree.apply(TeamCityEvent::TestFinished {
            parent: None,
            name: "testOk".into(),
            duration_ms: Some(1),
        });
        assert!(diagnostics_by_file(&tree, "phpunit").is_empty());
    }
}
