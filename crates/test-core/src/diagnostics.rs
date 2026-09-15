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
//! that text.
//!
//! Two shapes are handled, tried in this order:
//!
//! 1. A genuine JVM frame for the failing test's own class
//!    (`at com.example.GreeterTest.deliberatelyFails(GreeterTest.java:16)`).
//!    A real Surefire report is never trimmed of JDK-internal frames
//!    (`at java.base/java.util.ArrayList.forEach(ArrayList.java:1596)`), and
//!    those sort *after* the test's own frame in the trace — the old
//!    "last line" heuristic below picked exactly that JDK-internal line
//!    instead of the real one (jvm-build-tools plan review finding 3). Once
//!    a class-matching frame is found, its file is resolved to a real path
//!    under `work_dir` by package-path convention
//!    (`com.example.GreeterTest` -> `**/com/example/GreeterTest.java`) using
//!    the same symlink-safe walker `runner`'s `report_glob` reads use
//!    (finding 2's fix). If no such frame is found, or the file cannot be
//!    resolved, no diagnostic is emitted for that failure — a wrong
//!    location is worse than none, so this path never falls through to the
//!    heuristic below.
//! 2. PHPUnit's own trace shape (no dotted `Class.method` frames at all,
//!    since PHPUnit's namespace separator is `\` and its own separator
//!    before a method is `::`, not `.`): the same `path` `run_core::links`
//!    documents (ends in a `.`-extension, `line` half is plain digits),
//!    scanned from the last line first since PHPUnit's own trace lists the
//!    assertion site last, closest to the failure.
//!
//! ponytail: a naive heuristic rather than `run_core::links`' full regex
//! catalogue for shape 2 — this crate cannot depend on `run_core`
//! (layering.md's `test-core` row lists no adapter-layer crates), and a
//! wrong file only means one failing test's squiggle lands in the wrong
//! place, never anything that panics or drops the tree update. Upgrade
//! path: promote `run_core::links`' candidate-finding into a Qt-free crate
//! both can depend on, if this heuristic ever mislocates something that
//! matters.

use std::collections::HashMap;
use std::path::Path;

use diagnostics_core::{Diagnostic, Position, Range, Severity};

use crate::filter::split_class_method;
use crate::runner::glob_matches;
use crate::tree::{TestFailure, TestId, TestTree};

/// Every currently-failing test's diagnostic, grouped by the file URI it
/// was located in. A failure whose text carries no recognisable location —
/// including a JVM frame whose class matched but whose source file could
/// not be found under `work_dir` — contributes nothing here: it is still a
/// red row in the Tests dock, just not a squiggle, which is a smaller loss
/// than guessing.
pub fn diagnostics_by_file(
    tree: &TestTree,
    framework_name: &str,
    work_dir: &Path,
) -> HashMap<String, Vec<Diagnostic>> {
    let mut grouped: HashMap<String, Vec<Diagnostic>> = HashMap::new();
    for node in tree.failing_nodes() {
        let Some(failure) = &node.failure else {
            continue;
        };
        let Some((path, line)) = locate(failure, &node.id, work_dir) else {
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

/// Find `failure`'s location: a JVM frame for `id`'s own class first (see
/// the module doc for why this must win outright, resolve-or-skip, rather
/// than ever falling back once it starts down that path), then the generic
/// last-line-first heuristic PHPUnit's own trace shape needs.
fn locate(failure: &TestFailure, id: &TestId, work_dir: &Path) -> Option<(String, u32)> {
    let (class, method) = split_class_method(id);
    if method.is_some() {
        if let Some((file_name, line_no)) = find_class_frame(&failure.details, class) {
            return resolve_java_source(work_dir, class, &file_name).map(|path| (path, line_no));
        }
    }
    failure
        .details
        .lines()
        .rev()
        .find_map(location_in_line)
        .or_else(|| location_in_line(&failure.message))
}

/// The first line shaped `at <class>.<method>(<File>:<Line>)` whose class
/// is exactly `class` — scanning top to bottom, since a Java stack trace
/// lists the innermost (closest to the failure) frame first and Surefire
/// never trims the JDK-internal frames below it. `class` is matched with a
/// trailing `.` so `GreeterTest` cannot match a frame that merely starts
/// with the same characters (`GreeterTestHelper.foo(...)`).
fn find_class_frame(details: &str, class: &str) -> Option<(String, u32)> {
    let prefix = format!("{class}.");
    details.lines().find_map(|line| {
        let line = line.trim();
        let after_at = line.strip_prefix("at ")?;
        let after_class = after_at.strip_prefix(&prefix)?;
        let open = after_class.find('(')?;
        let inside = after_class[open + 1..].trim_end_matches(')');
        let (file_name, line_str) = inside.rsplit_once(':')?;
        let line_no: u32 = line_str.parse().ok()?;
        if file_name.is_empty() {
            return None;
        }
        Some((file_name.to_string(), line_no))
    })
}

/// Resolve a JVM class's source file to a real path under `work_dir`, by
/// package-path convention: `com.example.GreeterTest` searches for
/// `**/com/example/GreeterTest.java` — a bounded pattern (only paths
/// actually ending in that package/class shape can match), walked with the
/// same symlink-safe, `.git`/`node_modules`-pruning walker `runner`'s own
/// `report_glob` reads use (finding 2), not a second one. Several matches
/// (a class name reused across multi-module fixtures, most commonly a
/// project's own tests) resolve to the first in sorted order, for a
/// deterministic result rather than "whichever the filesystem happened to
/// return first".
fn resolve_java_source(work_dir: &Path, class: &str, file_name: &str) -> Option<String> {
    let package_path = class.replace('.', "/");
    let pattern = format!("**/{package_path}.java");
    let mut matches = glob_matches(work_dir, &pattern);
    matches.retain(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == file_name)
    });
    matches.sort();
    matches
        .into_iter()
        .next()
        .map(|path| path.to_string_lossy().into_owned())
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
        let grouped = diagnostics_by_file(&tree, "phpunit", Path::new("."));
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
        let grouped = diagnostics_by_file(&tree, "phpunit", Path::new("."));
        let uri = diagnostics_core::uri_from_path("/project/tests/GreeterTest.php");
        assert_eq!(grouped[&uri].len(), 1);
    }

    #[test]
    fn a_method_name_with_a_colon_but_no_extension_is_not_mistaken_for_a_path() {
        let tree = failing_tree("Tests\\GreeterTest::testAdds:20");
        let grouped = diagnostics_by_file(&tree, "phpunit", Path::new("."));
        assert!(grouped.is_empty());
    }

    #[test]
    fn details_with_no_location_at_all_contributes_nothing() {
        let tree = failing_tree("no location information here");
        let grouped = diagnostics_by_file(&tree, "phpunit", Path::new("."));
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
        assert!(diagnostics_by_file(&tree, "phpunit", Path::new(".")).is_empty());
    }

    /// A real, untrimmed Surefire trace: JDK-internal frames
    /// (`java.base/java.util.ArrayList.forEach`) sort *after* the test's
    /// own frame, exactly the shape that used to make the old last-line
    /// heuristic pick `ArrayList.java:1596` instead of the real assertion
    /// site (review finding 3).
    const REAL_SUREFIRE_TRACE: &str = "org.opentest4j.AssertionFailedError: expected: <Hello, World!> but was: <Hello, Nobody!>\n\
        \tat org.junit.jupiter.api.AssertionFailureBuilder.build(AssertionFailureBuilder.java:151)\n\
        \tat org.junit.jupiter.api.AssertionFailureBuilder.buildAndThrow(AssertionFailureBuilder.java:132)\n\
        \tat org.junit.jupiter.api.AssertEquals.failNotEqual(AssertEquals.java:197)\n\
        \tat org.junit.jupiter.api.AssertEquals.assertEquals(AssertEquals.java:182)\n\
        \tat org.junit.jupiter.api.AssertEquals.assertEquals(AssertEquals.java:177)\n\
        \tat org.junit.jupiter.api.Assertions.assertEquals(Assertions.java:1145)\n\
        \tat com.example.GreeterTest.deliberatelyFails(GreeterTest.java:16)\n\
        \tat java.base/java.lang.reflect.Method.invoke(Method.java:580)\n\
        \tat java.base/java.util.ArrayList.forEach(ArrayList.java:1596)\n\
        \tat java.base/java.util.ArrayList.forEach(ArrayList.java:1596)\n";

    fn jvm_failing_tree(details: &str) -> TestTree {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "com.example.GreeterTest".into(),
        });
        tree.apply(TeamCityEvent::TestStarted {
            parent: Some(TestId("com.example.GreeterTest".into())),
            name: "deliberatelyFails".into(),
        });
        tree.apply(TeamCityEvent::TestFailed {
            parent: Some(TestId("com.example.GreeterTest".into())),
            name: "deliberatelyFails".into(),
            message: "expected: <Hello, World!> but was: <Hello, Nobody!>".into(),
            details: details.into(),
        });
        tree
    }

    #[test]
    fn find_class_frame_picks_the_test_s_own_frame_not_the_last_jdk_internal_one() {
        let found = find_class_frame(REAL_SUREFIRE_TRACE, "com.example.GreeterTest");
        assert_eq!(found, Some(("GreeterTest.java".to_string(), 16)));
    }

    #[test]
    fn a_jvm_failure_locates_the_real_source_file_under_work_dir() {
        let work_dir = tempfile::tempdir().unwrap();
        let src_dir = work_dir.path().join("src/test/java/com/example");
        std::fs::create_dir_all(&src_dir).unwrap();
        let source_file = src_dir.join("GreeterTest.java");
        std::fs::write(&source_file, "class GreeterTest {}").unwrap();

        let tree = jvm_failing_tree(REAL_SUREFIRE_TRACE);
        let grouped = diagnostics_by_file(&tree, "junit-maven", work_dir.path());

        let uri = diagnostics_core::uri_from_path(source_file.to_str().unwrap());
        assert_eq!(grouped[&uri].len(), 1);
        assert_eq!(
            grouped[&uri][0].range.start.line, 15,
            "GreeterTest.java:16 is 0-indexed line 15"
        );
    }

    #[test]
    fn a_jvm_failure_whose_source_file_cannot_be_found_emits_no_diagnostic() {
        let work_dir = tempfile::tempdir().unwrap();
        let tree = jvm_failing_tree(REAL_SUREFIRE_TRACE);
        let grouped = diagnostics_by_file(&tree, "junit-maven", work_dir.path());
        assert!(
            grouped.is_empty(),
            "a class-matching frame with no resolvable file must not fall back to a guess"
        );
    }
}
