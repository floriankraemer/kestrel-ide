//! Building a `--filter` pattern from tree nodes (D6): "rerun all failed",
//! "rerun this one node" — the tree's context menu actions, in the tool's
//! own PCRE-flavoured `--filter` dialect (PHPUnit's, and any future
//! framework whose manifest reuses the same `output-format`).

use crate::tree::{NodeKind, TestId, TestTree};

/// Escape a string for literal use inside a PCRE `--filter` pattern. A
/// test's qualified name can itself carry regex metacharacters — a
/// data-provider-suffixed name commonly has `(`, `)`, `#`, `"` — while the
/// namespace separator `\` is left alone: a literal backslash has no
/// special meaning to PCRE on its own, so escaping it would only double it
/// uselessly and make the pattern harder to read in a log.
fn escape_regex(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if ".+*?()[]{}^$|/#".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// The `--filter` pattern for one node: an exact match for a leaf test, or
/// a prefix match for a suite. PHPUnit's own qualified test names are
/// `Suite::method`, so "every test in this suite" means "starts with the
/// suite's id followed by `::`" — matching the suite id exactly would
/// select nothing, since only leaf tests are ever actually run.
pub fn for_node(tree: &TestTree, id: &TestId) -> String {
    match tree.node(id).map(|node| node.kind) {
        Some(NodeKind::Suite) => format!("^{}::", escape_regex(id.as_str())),
        _ => format!("^{}$", escape_regex(id.as_str())),
    }
}

/// The `--filter` alternation selecting exactly the given leaf tests —
/// "Rerun Failed".
pub fn for_many(ids: &[TestId]) -> String {
    let alternatives: Vec<String> = ids.iter().map(|id| escape_regex(id.as_str())).collect();
    format!("^(?:{})$", alternatives.join("|"))
}

/// Append a rerun `pattern` to a framework's base `args`, in whichever of
/// its two mutually-exclusive styles the manifest declares (`plugin-api`'s
/// `TestFrameworkContribution` validates exactly one is set): a
/// `filter_flag` is pushed as its own argument followed by `pattern`
/// (PHPUnit's/Gradle's `--filter foo`, `--tests foo`); a `filter_template`
/// has `{pattern}` substituted inside one argument (Maven's
/// `-Dtest={pattern}`, which Maven would not parse split across two argv
/// entries). Neither set means no rerun narrowing is possible for this
/// framework — `args` is returned unchanged, same as calling this with no
/// pattern at all.
///
/// This is the one call site `runner::run`'s caller (`ui-shell`'s
/// `TestServiceRust::start`) uses for both styles, kept here rather than
/// duplicated in the adapter so it gets the same unit tests every other
/// `test-core` rule does (jvm-build-tools plan C1).
pub fn apply_filter(
    args: &[String],
    filter_flag: Option<&str>,
    filter_template: Option<&str>,
    pattern: &str,
) -> Vec<String> {
    let mut args = args.to_vec();
    if let Some(flag) = filter_flag {
        args.push(flag.to_string());
        args.push(pattern.to_string());
    } else if let Some(template) = filter_template {
        args.push(template.replace("{pattern}", pattern));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teamcity::TeamCityEvent;

    fn tree_with_suite_and_test() -> TestTree {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "Tests\\GreeterTest".into(),
        });
        tree.apply(TeamCityEvent::TestStarted {
            parent: Some(TestId("Tests\\GreeterTest".into())),
            name: "testGreets".into(),
        });
        tree
    }

    #[test]
    fn a_leaf_test_is_matched_exactly() {
        let tree = tree_with_suite_and_test();
        let pattern = for_node(&tree, &TestId("Tests\\GreeterTest::testGreets".into()));
        assert_eq!(pattern, "^Tests\\GreeterTest::testGreets$");
    }

    #[test]
    fn a_suite_is_matched_as_a_prefix_not_exactly() {
        let tree = tree_with_suite_and_test();
        let pattern = for_node(&tree, &TestId("Tests\\GreeterTest".into()));
        assert_eq!(pattern, "^Tests\\GreeterTest::");
    }

    #[test]
    fn an_unknown_id_falls_back_to_an_exact_match() {
        let tree = TestTree::new();
        let pattern = for_node(&tree, &TestId("Whatever::testX".into()));
        assert_eq!(pattern, "^Whatever::testX$");
    }

    #[test]
    fn several_ids_become_one_alternation() {
        let pattern = for_many(&[TestId("A::testOne".into()), TestId("B::testTwo".into())]);
        assert_eq!(pattern, "^(?:A::testOne|B::testTwo)$");
    }

    #[test]
    fn regex_metacharacters_in_a_data_provider_suffix_are_escaped() {
        let pattern = for_many(&[TestId("A::testX with data set #1 (a, b)".into())]);
        assert_eq!(pattern, "^(?:A::testX with data set \\#1 \\(a, b\\))$");
    }

    #[test]
    fn a_filter_flag_is_pushed_as_flag_then_pattern() {
        let args = apply_filter(&["test".into()], Some("--tests"), None, "com.example.ATest");
        assert_eq!(args, vec!["test", "--tests", "com.example.ATest"]);
    }

    #[test]
    fn a_filter_template_substitutes_the_pattern_placeholder_in_one_argument() {
        let args = apply_filter(
            &["-B".into(), "test".into()],
            None,
            Some("-Dtest={pattern}"),
            "com.example.ATest",
        );
        assert_eq!(args, vec!["-B", "test", "-Dtest=com.example.ATest"]);
    }

    #[test]
    fn neither_style_set_leaves_args_unchanged() {
        let args = apply_filter(&["test".into()], None, None, "whatever");
        assert_eq!(args, vec!["test"]);
    }
}
