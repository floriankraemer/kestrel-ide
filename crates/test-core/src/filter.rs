//! Building a `--filter`/`--tests`/`-Dtest=` pattern from tree nodes (D6):
//! "rerun all failed", "rerun this one node" — the tree's context menu
//! actions, in whichever target-selection dialect the running framework's
//! manifest names ([`FilterDialect`], jvm-build-tools plan review finding
//! 1).
//!
//! A JVM test id is shaped exactly like a PHPUnit one — `Class::method`,
//! built by the same [`TestId::child`](crate::tree::TestId::child) rule —
//! but Surefire's `-Dtest=` and Gradle's `--tests` each speak a different
//! syntax over that id than PHPUnit's own PCRE `--filter` does. Building a
//! PHPUnit-shaped pattern for either of them compiles fine and matches
//! nothing at runtime: combined with `-Dsurefire.failIfNoSpecifiedTests=false`
//! (added so a legitimately test-less module never fails the build), a
//! Maven rerun silently "passes" having run zero tests.

use crate::tree::{NodeKind, TestId, TestTree};

/// Which target-selection syntax a test framework's rerun flag/template
/// speaks. Named by a manifest's optional `filter-dialect` field
/// (`plugin_api::TestFrameworkContribution::filter_dialect`); this crate has
/// no dependency on `plugin-api` (layering.md), so the bridge passes the raw
/// `Option<&str>` through [`parse_filter_dialect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterDialect {
    /// PHPUnit's own PCRE `--filter`: an anchored regex over the qualified
    /// `Namespace\Class::method` name. The only dialect that existed before
    /// this field did, and every manifest's default.
    PhpUnitRegex,
    /// Maven Surefire's/Failsafe's `-Dtest=`: `Class#method`, comma-joined
    /// for several, or a bare `Class` to select a whole suite.
    Surefire,
    /// Gradle's `--tests`: a dotted `pkg.Class.method` pattern (only `*` is
    /// a wildcard), or a bare `pkg.Class` to select a whole suite.
    Gradle,
}

/// A manifest's `filter-dialect` string that no dialect below understands —
/// a typed error the caller can turn into a refused run, never a silent
/// fallback to PHPUnit's regex dialect against a JVM tool that does not
/// speak it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownFilterDialect(pub String);

impl std::fmt::Display for UnknownFilterDialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown filter dialect: {}", self.0)
    }
}

impl std::error::Error for UnknownFilterDialect {}

/// Parse a `TestFrameworkContribution::filter_dialect` value. `None` (the
/// field absent from a manifest, including every contribution written
/// before this field existed) defaults to [`FilterDialect::PhpUnitRegex`],
/// so php-tools' own `phpunit` row keeps behaving exactly as before.
pub fn parse_filter_dialect(value: Option<&str>) -> Result<FilterDialect, UnknownFilterDialect> {
    match value {
        None | Some("phpunit-regex") => Ok(FilterDialect::PhpUnitRegex),
        Some("surefire") => Ok(FilterDialect::Surefire),
        Some("gradle") => Ok(FilterDialect::Gradle),
        Some(other) => Err(UnknownFilterDialect(other.to_string())),
    }
}

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

/// Split a JVM-shaped [`TestId`] — `"com.example.GreeterTest::deliberatelyFails"`
/// for a leaf test, or a bare `"com.example.GreeterTest"` for a suite —
/// into its class and, for a leaf, method name. Both Surefire's and
/// Gradle's dialects need exactly this split, just joined back together
/// with a different separator.
pub(crate) fn split_class_method(id: &TestId) -> (&str, Option<&str>) {
    match id.as_str().split_once("::") {
        Some((class, method)) => (class, Some(method)),
        None => (id.as_str(), None),
    }
}

fn surefire_pattern(id: &TestId) -> String {
    match split_class_method(id) {
        (class, Some(method)) => format!("{class}#{method}"),
        (class, None) => class.to_string(),
    }
}

fn gradle_pattern(id: &TestId) -> String {
    match split_class_method(id) {
        (class, Some(method)) => format!("{class}.{method}"),
        (class, None) => class.to_string(),
    }
}

/// The rerun pattern(s) for one node: an exact match for a leaf test, a
/// prefix/whole-class match for a suite. Returned as a `Vec` because
/// Gradle's `--tests` has no comma-alternation of its own — selecting
/// several specific tests in one invocation means repeating the flag, which
/// [`apply_filter`] does once per element — while PHPUnit's and Surefire's
/// dialects always produce exactly one pattern.
pub fn for_node(tree: &TestTree, id: &TestId, dialect: FilterDialect) -> Vec<String> {
    match dialect {
        FilterDialect::PhpUnitRegex => {
            let pattern = match tree.node(id).map(|node| node.kind) {
                Some(NodeKind::Suite) => format!("^{}::", escape_regex(id.as_str())),
                _ => format!("^{}$", escape_regex(id.as_str())),
            };
            vec![pattern]
        }
        FilterDialect::Surefire => vec![surefire_pattern(id)],
        FilterDialect::Gradle => vec![gradle_pattern(id)],
    }
}

/// The rerun pattern(s) selecting exactly the given leaf tests — "Rerun
/// Failed". PHPUnit's dialect folds them into one alternation, Surefire's
/// into one comma-joined `-Dtest=` value; Gradle's dialect has no such
/// joining syntax, so it returns one pattern per id, each becoming its own
/// `--tests` occurrence.
pub fn for_many(ids: &[TestId], dialect: FilterDialect) -> Vec<String> {
    match dialect {
        FilterDialect::PhpUnitRegex => {
            let alternatives: Vec<String> =
                ids.iter().map(|id| escape_regex(id.as_str())).collect();
            vec![format!("^(?:{})$", alternatives.join("|"))]
        }
        FilterDialect::Surefire => {
            let entries: Vec<String> = ids.iter().map(surefire_pattern).collect();
            vec![entries.join(",")]
        }
        FilterDialect::Gradle => ids.iter().map(gradle_pattern).collect(),
    }
}

/// Append rerun `patterns` to a framework's base `args`, in whichever of its
/// two mutually-exclusive styles the manifest declares (`plugin-api`'s
/// `TestFrameworkContribution` validates exactly one is set): a
/// `filter_flag` is pushed as its own argument followed by each pattern in
/// turn (PHPUnit's/Gradle's `--filter foo`, `--tests foo` — repeated once
/// per pattern, which is what lets Gradle's dialect select several specific
/// tests with no alternation syntax of its own); a `filter_template` has
/// `{pattern}` substituted inside one argument using only the first pattern
/// (Maven's `-Dtest={pattern}`, which Maven would not parse split across two
/// argv entries, and whose dialect already joins several tests into that
/// one pattern before this is called). Neither set means no rerun narrowing
/// is possible for this framework — `args` is returned unchanged, same as
/// calling this with no patterns at all.
///
/// This is the one call site `runner::run`'s caller (`ui-shell`'s
/// `TestServiceRust::start`) uses for both styles, kept here rather than
/// duplicated in the adapter so it gets the same unit tests every other
/// `test-core` rule does (jvm-build-tools plan C1).
pub fn apply_filter(
    args: &[String],
    filter_flag: Option<&str>,
    filter_template: Option<&str>,
    patterns: &[String],
) -> Vec<String> {
    let mut args = args.to_vec();
    if patterns.is_empty() {
        return args;
    }
    if let Some(flag) = filter_flag {
        for pattern in patterns {
            args.push(flag.to_string());
            args.push(pattern.clone());
        }
    } else if let Some(template) = filter_template {
        if let Some(pattern) = patterns.first() {
            args.push(template.replace("{pattern}", pattern));
        }
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

    fn jvm_tree_with_suite_and_test() -> TestTree {
        let mut tree = TestTree::new();
        tree.apply(TeamCityEvent::SuiteStarted {
            parent: None,
            name: "com.example.GreeterTest".into(),
        });
        tree.apply(TeamCityEvent::TestStarted {
            parent: Some(TestId("com.example.GreeterTest".into())),
            name: "deliberatelyFails".into(),
        });
        tree
    }

    #[test]
    fn parse_filter_dialect_defaults_to_phpunit_regex_when_absent() {
        assert_eq!(parse_filter_dialect(None), Ok(FilterDialect::PhpUnitRegex));
    }

    #[test]
    fn parse_filter_dialect_accepts_the_three_known_values() {
        assert_eq!(
            parse_filter_dialect(Some("phpunit-regex")),
            Ok(FilterDialect::PhpUnitRegex)
        );
        assert_eq!(
            parse_filter_dialect(Some("surefire")),
            Ok(FilterDialect::Surefire)
        );
        assert_eq!(
            parse_filter_dialect(Some("gradle")),
            Ok(FilterDialect::Gradle)
        );
    }

    #[test]
    fn parse_filter_dialect_rejects_an_unknown_value_as_a_typed_error_not_a_panic() {
        let err = parse_filter_dialect(Some("checkstyle-xml")).unwrap_err();
        assert_eq!(err, UnknownFilterDialect("checkstyle-xml".to_string()));
    }

    // --- phpunit-regex dialect --------------------------------------------

    #[test]
    fn phpunit_leaf_is_matched_exactly() {
        let tree = tree_with_suite_and_test();
        let pattern = for_node(
            &tree,
            &TestId("Tests\\GreeterTest::testGreets".into()),
            FilterDialect::PhpUnitRegex,
        );
        assert_eq!(pattern, vec!["^Tests\\GreeterTest::testGreets$"]);
    }

    #[test]
    fn phpunit_suite_is_matched_as_a_prefix_not_exactly() {
        let tree = tree_with_suite_and_test();
        let pattern = for_node(
            &tree,
            &TestId("Tests\\GreeterTest".into()),
            FilterDialect::PhpUnitRegex,
        );
        assert_eq!(pattern, vec!["^Tests\\GreeterTest::"]);
    }

    #[test]
    fn phpunit_unknown_id_falls_back_to_an_exact_match() {
        let tree = TestTree::new();
        let pattern = for_node(
            &tree,
            &TestId("Whatever::testX".into()),
            FilterDialect::PhpUnitRegex,
        );
        assert_eq!(pattern, vec!["^Whatever::testX$"]);
    }

    #[test]
    fn phpunit_several_ids_become_one_alternation() {
        let pattern = for_many(
            &[TestId("A::testOne".into()), TestId("B::testTwo".into())],
            FilterDialect::PhpUnitRegex,
        );
        assert_eq!(pattern, vec!["^(?:A::testOne|B::testTwo)$"]);
    }

    #[test]
    fn phpunit_regex_metacharacters_in_a_data_provider_suffix_are_escaped() {
        let pattern = for_many(
            &[TestId("A::testX with data set #1 (a, b)".into())],
            FilterDialect::PhpUnitRegex,
        );
        assert_eq!(
            pattern,
            vec!["^(?:A::testX with data set \\#1 \\(a, b\\))$"]
        );
    }

    // --- surefire dialect ---------------------------------------------------

    #[test]
    fn surefire_leaf_is_class_hash_method() {
        let tree = jvm_tree_with_suite_and_test();
        let pattern = for_node(
            &tree,
            &TestId("com.example.GreeterTest::deliberatelyFails".into()),
            FilterDialect::Surefire,
        );
        assert_eq!(pattern, vec!["com.example.GreeterTest#deliberatelyFails"]);
    }

    #[test]
    fn surefire_suite_is_the_bare_class_name() {
        let tree = jvm_tree_with_suite_and_test();
        let pattern = for_node(
            &tree,
            &TestId("com.example.GreeterTest".into()),
            FilterDialect::Surefire,
        );
        assert_eq!(pattern, vec!["com.example.GreeterTest"]);
    }

    #[test]
    fn surefire_several_ids_are_comma_joined_into_one_pattern() {
        let pattern = for_many(
            &[
                TestId("com.a.ATest::testOne".into()),
                TestId("com.b.BTest::testTwo".into()),
            ],
            FilterDialect::Surefire,
        );
        assert_eq!(pattern, vec!["com.a.ATest#testOne,com.b.BTest#testTwo"]);
    }

    // --- gradle dialect -------------------------------------------------

    #[test]
    fn gradle_leaf_is_dotted_class_and_method() {
        let tree = jvm_tree_with_suite_and_test();
        let pattern = for_node(
            &tree,
            &TestId("com.example.GreeterTest::deliberatelyFails".into()),
            FilterDialect::Gradle,
        );
        assert_eq!(pattern, vec!["com.example.GreeterTest.deliberatelyFails"]);
    }

    #[test]
    fn gradle_suite_is_the_bare_dotted_class_name() {
        let tree = jvm_tree_with_suite_and_test();
        let pattern = for_node(
            &tree,
            &TestId("com.example.GreeterTest".into()),
            FilterDialect::Gradle,
        );
        assert_eq!(pattern, vec!["com.example.GreeterTest"]);
    }

    #[test]
    fn gradle_several_ids_become_one_pattern_each_not_one_joined_pattern() {
        let pattern = for_many(
            &[
                TestId("com.a.ATest::testOne".into()),
                TestId("com.b.BTest::testTwo".into()),
            ],
            FilterDialect::Gradle,
        );
        assert_eq!(pattern, vec!["com.a.ATest.testOne", "com.b.BTest.testTwo"]);
    }

    // --- apply_filter ---------------------------------------------------

    #[test]
    fn a_filter_flag_is_pushed_as_flag_then_pattern() {
        let args = apply_filter(
            &["test".into()],
            Some("--tests"),
            None,
            &["com.example.ATest".to_string()],
        );
        assert_eq!(args, vec!["test", "--tests", "com.example.ATest"]);
    }

    #[test]
    fn a_filter_flag_is_repeated_once_per_pattern() {
        let args = apply_filter(
            &["test".into()],
            Some("--tests"),
            None,
            &["A.testOne".to_string(), "B.testTwo".to_string()],
        );
        assert_eq!(
            args,
            vec!["test", "--tests", "A.testOne", "--tests", "B.testTwo"]
        );
    }

    #[test]
    fn a_filter_template_substitutes_the_pattern_placeholder_in_one_argument() {
        let args = apply_filter(
            &["-B".into(), "test".into()],
            None,
            Some("-Dtest={pattern}"),
            &["com.example.ATest".to_string()],
        );
        assert_eq!(args, vec!["-B", "test", "-Dtest=com.example.ATest"]);
    }

    #[test]
    fn neither_style_set_leaves_args_unchanged() {
        let args = apply_filter(&["test".into()], None, None, &["whatever".to_string()]);
        assert_eq!(args, vec!["test"]);
    }

    #[test]
    fn no_patterns_at_all_leaves_args_unchanged() {
        let args = apply_filter(&["test".into()], Some("--tests"), None, &[]);
        assert_eq!(args, vec!["test"]);
    }
}
