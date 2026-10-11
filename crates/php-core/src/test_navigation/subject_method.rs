//! Go to Test Subject from a test method (E8's G11): PhpStorm lands on the
//! tested method when it can tell which one it is, else on the class.
//!
//! The method is, in order: what the test method's own docblock says it
//! covers (`@covers Class::method`); the class-level `#[CoversMethod(Class::class,
//! 'method')]` or `@covers Class::method` whose method the test's name
//! names; the name convention alone (`testFooBar` tests `fooBar`) when a
//! subject declares that method.

use std::collections::BTreeMap;
use std::path::Path;

use tree_sitter::Node;

use super::Declarations;
use crate::composer::Psr4Root;
use crate::psr4;
use crate::tests::MarkerScope;

/// A method a test says it covers, its class as written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CoveredMethod {
    class: String,
    /// From a `@covers` annotation, whose class is fully qualified with or
    /// without the leading `\`.
    annotation: bool,
    method: String,
}

/// The `#[CoversMethod]` attributes and `@covers Class::method` annotations
/// in `text`.
fn covered_methods(text: &str) -> Vec<CoveredMethod> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices("CoversMethod(") {
        let arguments = &text[at + "CoversMethod(".len()..];
        let arguments = arguments.split(')').next().unwrap_or_default();
        let mut parts = arguments.split(',');
        let class = parts.next().unwrap_or_default().trim();
        let method = parts.next().unwrap_or_default().trim();
        let (Some(class), Some(method)) = (
            class.strip_suffix("::class"),
            method
                .strip_prefix(['\'', '"'])
                .and_then(|m| m.strip_suffix(['\'', '"'])),
        ) else {
            continue;
        };
        found.push(CoveredMethod {
            class: class.trim().to_string(),
            annotation: false,
            method: method.to_string(),
        });
    }
    for (at, _) in text.match_indices("@covers") {
        let rest = &text[at + "@covers".len()..];
        if !rest.starts_with([' ', '\t']) {
            continue;
        }
        let target = rest.split_whitespace().next().unwrap_or_default();
        let Some((class, method)) = target.split_once("::") else {
            continue;
        };
        let method = method.trim_end_matches("()");
        if class.is_empty() || method.is_empty() || method.starts_with('<') {
            continue;
        }
        found.push(CoveredMethod {
            class: class.to_string(),
            annotation: true,
            method: method.to_string(),
        });
    }
    found
}

/// The method the name convention reads into a test name: `testFooBar`
/// and `test_fooBar` test `fooBar`; a name without the prefix (an `#[Test]`
/// method) tests the method of its own name.
fn conventional_method(test: &str) -> String {
    let Some(rest) = test.strip_prefix("test") else {
        return test.to_string();
    };
    let rest = rest.trim_start_matches('_');
    let mut chars = rest.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => test.to_string(),
    }
}

/// The lines just above `line` that belong to its declaration: the
/// docblock and attributes.
fn preamble(text: &str, line: u32) -> String {
    let lines: Vec<&str> = text.lines().take(line as usize).collect();
    let start = lines
        .iter()
        .rposition(|line| {
            let line = line.trim();
            !(line.starts_with("/**")
                || line.starts_with('*')
                || line.starts_with("#[")
                || line.is_empty())
        })
        .map_or(0, |index| index + 1);
    lines[start..].join("\n")
}

/// The 0-based line of the method `name` declares in `text` (PHP method
/// names are case-insensitive).
pub(super) fn method_line(text: &str, name: &str) -> Option<u32> {
    fn visit(text: &str, node: Node<'_>, name: &str) -> Option<u32> {
        if node.kind() == "method_declaration" {
            if let Some(found) = node.child_by_field_name("name") {
                if text[found.byte_range()].eq_ignore_ascii_case(name) {
                    return u32::try_from(found.start_position().row).ok();
                }
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
        children
            .into_iter()
            .find_map(|child| visit(text, child, name))
    }
    let tree = crate::tests::parse(text)?;
    visit(text, tree.root_node(), name)
}

/// Where in its subjects a jump from the test method around `caret_line`
/// lands: the file of an explicitly covered method first in `files`, and
/// for each file in `files` the 0-based line of the tested method when it
/// declares it. Empty when the caret is not in a PHPUnit test method.
pub(super) fn subject_lines(
    roots: &[Psr4Root],
    project: &Path,
    text: &str,
    declarations: &Declarations,
    caret_line: u32,
    files: &mut Vec<String>,
) -> BTreeMap<String, u32> {
    let mut lines = BTreeMap::new();
    let Some(marker) = crate::tests::marker_at(text, caret_line)
        .filter(|marker| marker.scope == MarkerScope::Test && !marker.method.is_empty())
    else {
        return lines;
    };
    let candidate = conventional_method(&marker.method);
    let own = covered_methods(&preamble(text, marker.line));
    let named = covered_methods(text)
        .into_iter()
        .filter(|covered| covered.method.eq_ignore_ascii_case(&candidate));
    let explicit = own.into_iter().chain(named).find_map(|covered| {
        let class = if covered.annotation {
            covered.class.trim_start_matches('\\').to_string()
        } else {
            declarations.resolve(&covered.class)
        };
        let file = psr4::class_file(roots, project, &class)
            .or_else(|| psr4::class_file(roots, project, &declarations.resolve(&covered.class)))?;
        Some((file, covered.method))
    });
    let read = |file: &str| std::fs::read_to_string(project.join(file)).ok();
    if let Some((file, method)) = explicit {
        if let Some(line) = read(&file).and_then(|subject| method_line(&subject, &method)) {
            lines.insert(file.clone(), line);
        }
        files.retain(|kept| *kept != file);
        files.insert(0, file);
        return lines;
    }
    for file in files.iter() {
        if let Some(line) = read(file).and_then(|subject| method_line(&subject, &candidate)) {
            lines.insert(file.clone(), line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_convention_strips_the_test_prefix() {
        assert_eq!(conventional_method("testFooBar"), "fooBar");
        assert_eq!(conventional_method("test_fooBar"), "fooBar");
        assert_eq!(conventional_method("it_sends"), "it_sends");
        assert_eq!(conventional_method("test"), "test");
    }

    #[test]
    fn covered_methods_read_attributes_and_annotations() {
        let text = "#[CoversMethod(Bar::class, 'run')]\n/** @covers \\App\\Qux::go() */\n/** @covers ::alone @covers Baz */";
        assert_eq!(
            covered_methods(text),
            [
                CoveredMethod {
                    class: "Bar".into(),
                    annotation: false,
                    method: "run".into()
                },
                CoveredMethod {
                    class: "\\App\\Qux".into(),
                    annotation: true,
                    method: "go".into()
                },
            ]
        );
    }

    #[test]
    fn the_preamble_is_the_docblock_and_attributes_above_a_line() {
        let text = "class T {\n    public function a() {}\n\n    /**\n     * @covers X::y\n     */\n    #[Test]\n    public function b() {}\n";
        assert_eq!(
            preamble(text, 7),
            "\n    /**\n     * @covers X::y\n     */\n    #[Test]"
        );
    }

    #[test]
    fn a_method_is_found_by_name_ignoring_case() {
        let text = "<?php\nclass Bar {\n    public function run() {}\n    public function fooBar() {}\n}\n";
        assert_eq!(method_line(text, "FOOBAR"), Some(3));
        assert_eq!(method_line(text, "missing"), None);
    }
}
