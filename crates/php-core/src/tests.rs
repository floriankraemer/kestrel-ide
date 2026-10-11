//! Where the editor's gutter puts Run/Debug/Coverage markers in a PHP test
//! file (PHP parity plan, T2), and the `--filter` string each one runs.
//!
//! Two shapes are recognised: PHPUnit classes (a class extending a
//! `*TestCase`; `test*` methods, `#[Test]` attributes and `@test`
//! docblocks) and Pest files (`test(`/`it(` calls, nested in `describe(`).
//! The parse goes through `syntax-core`'s registry — the grammar the editor
//! already highlights with.

use tree_sitter::Node;

/// What a marker runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerScope {
    /// A whole PHPUnit class, or a Pest `describe` block.
    Group,
    /// One PHPUnit method or one Pest `test`/`it` call.
    Test,
    /// Every test in the file; only [`marker_at`] produces it.
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestMarker {
    /// 0-based line of the class/method name or of the call.
    pub line: u32,
    pub scope: MarkerScope,
    /// What the menu calls it: the class, method or Pest description.
    pub name: String,
    /// What the `--filter` flag takes to run exactly this.
    /// Empty for [`MarkerScope::File`], which runs the file without a filter.
    pub filter: String,
    /// First and last 0-based line of the class, method or call, so a caret
    /// anywhere inside it selects this marker.
    pub span: (u32, u32),
}

/// The test a caret on `caret_line` (0-based) runs: the innermost method or
/// Pest call around it, else the enclosing class or `describe`, else the whole
/// file when it holds tests at all. `None` for a file without tests.
pub fn marker_at(text: &str, caret_line: u32) -> Option<TestMarker> {
    let all = markers(text);
    if all.is_empty() {
        return None;
    }
    // Markers are in pre-order, so on equal spans (a one-line class or
    // `describe`) the innermost comes last: search from the back.
    let enclosing = all
        .iter()
        .rev()
        .filter(|m| (m.span.0..=m.span.1).contains(&caret_line))
        .min_by_key(|m| m.span.1 - m.span.0);
    Some(enclosing.cloned().unwrap_or(TestMarker {
        line: 0,
        scope: MarkerScope::File,
        name: String::new(),
        filter: String::new(),
        span: (0, u32::MAX),
    }))
}

fn span_of(node: Node<'_>) -> (u32, u32) {
    (
        node.start_position().row as u32,
        node.end_position().row as u32,
    )
}

/// Every runnable test position in `text`, in file order. Empty for a file
/// that is not a test or does not parse.
pub fn markers(text: &str) -> Vec<TestMarker> {
    let Some(tree) = parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = Walker {
        text,
        namespace: String::new(),
        describes: Vec::new(),
        out: &mut out,
    };
    walker.visit(tree.root_node());
    out
}

fn parse(text: &str) -> Option<tree_sitter::Tree> {
    let language = syntax_core::language_by_id("php")?;
    let compiled = syntax_core::registry().compiled(language)?.ok()?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&compiled.grammar).ok()?;
    parser.parse(text, None)
}

struct Walker<'a> {
    text: &'a str,
    namespace: String,
    /// Descriptions of the enclosing Pest `describe` calls.
    describes: Vec<String>,
    out: &'a mut Vec<TestMarker>,
}

impl Walker<'_> {
    fn text_of(&self, node: Node<'_>) -> &str {
        &self.text[node.byte_range()]
    }

    fn visit(&mut self, node: Node<'_>) {
        match node.kind() {
            "namespace_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.namespace = self.text_of(name).to_string();
                }
            }
            "class_declaration" => {
                self.class(node);
                return;
            }
            "function_call_expression" if self.pest_call(node) => return,
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child);
        }
    }

    fn class(&mut self, class: Node<'_>) {
        let (extends_test_case, is_abstract) = {
            let mut cursor = class.walk();
            let mut extends = false;
            let mut is_abstract = false;
            for child in class.children(&mut cursor) {
                extends |= child.kind() == "base_clause"
                    && self
                        .text_of(child)
                        .trim_start_matches("extends")
                        .trim()
                        .ends_with("TestCase");
                is_abstract |= child.kind() == "abstract_modifier";
            }
            (extends, is_abstract)
        };
        let (Some(name), Some(body)) = (
            class.child_by_field_name("name"),
            class.child_by_field_name("body"),
        ) else {
            return;
        };
        // An abstract base class cannot run; its tests run through subclasses.
        if !extends_test_case || is_abstract {
            return;
        }
        let qualified = if self.namespace.is_empty() {
            self.text_of(name).to_string()
        } else {
            format!("{}\\{}", self.namespace, self.text_of(name))
        };
        let escaped = test_core::filter::escape_regex(&qualified);
        self.out.push(TestMarker {
            line: name.start_position().row as u32,
            scope: MarkerScope::Group,
            name: self.text_of(name).to_string(),
            filter: format!("^{escaped}::"),
            span: span_of(class),
        });
        let mut cursor = body.walk();
        let members: Vec<Node<'_>> = body.named_children(&mut cursor).collect();
        for (index, member) in members.iter().enumerate() {
            if member.kind() != "method_declaration" || !self.is_public(*member) {
                continue;
            }
            let Some(method) = member.child_by_field_name("name") else {
                continue;
            };
            let method_name = self.text_of(method);
            let comment = index
                .checked_sub(1)
                .map(|prev| members[prev])
                .filter(|prev| prev.kind() == "comment");
            let docblock_test = comment.is_some_and(|c| self.text_of(c).contains("@test"));
            if method_name.starts_with("test") || self.has_test_attribute(*member) || docblock_test
            {
                let mut span = span_of(*member);
                // Any comment right above the method belongs to it.
                if let Some(comment) = comment {
                    span.0 = span_of(comment).0;
                }
                self.out.push(TestMarker {
                    line: method.start_position().row as u32,
                    scope: MarkerScope::Test,
                    name: method_name.to_string(),
                    // A data provider suffixes the name: `m with data set #0`.
                    filter: format!(
                        "^{escaped}::{}( with data set .+)?$",
                        test_core::filter::escape_regex(method_name)
                    ),
                    span,
                });
            }
        }
    }

    fn is_public(&self, method: Node<'_>) -> bool {
        let mut cursor = method.walk();
        let mut modifiers = method
            .children(&mut cursor)
            .filter(|c| c.kind() == "visibility_modifier")
            .map(|c| self.text_of(c));
        modifiers.all(|m| m == "public")
    }

    fn has_test_attribute(&self, method: Node<'_>) -> bool {
        let mut cursor = method.walk();
        let found = method
            .children(&mut cursor)
            .filter(|c| c.kind() == "attribute_list")
            .any(|list| {
                let mut cursor = list.walk();
                let mut stack = vec![list];
                let mut found = false;
                while let Some(node) = stack.pop() {
                    if node.kind() == "attribute" {
                        let name = self
                            .text_of(node)
                            .split('(')
                            .next()
                            .unwrap_or_default()
                            .trim();
                        found |= name == "Test" || name.ends_with("\\Test");
                    }
                    stack.extend(node.children(&mut cursor));
                }
                found
            });
        found
    }

    /// Handles `test(`/`it(`/`describe(`; false for any other call.
    fn pest_call(&mut self, call: Node<'_>) -> bool {
        let Some(function) = call.child_by_field_name("function") else {
            return false;
        };
        let callee = self.text_of(function);
        if !matches!(callee, "test" | "it" | "describe") {
            return false;
        }
        let Some(description) = self.first_string_argument(call) else {
            return false;
        };
        if callee == "describe" {
            self.describes.push(format!("`{description}`"));
            self.out.push(TestMarker {
                line: call.start_position().row as u32,
                scope: MarkerScope::Group,
                name: description,
                filter: test_core::filter::escape_regex(&format!(
                    "{} → ",
                    self.describes.join(" → ")
                )),
                span: span_of(call),
            });
            let mut cursor = call.walk();
            for child in call.children(&mut cursor) {
                self.visit(child);
            }
            self.describes.pop();
            return true;
        }
        let name = if callee == "it" {
            format!("it {description}")
        } else {
            description.clone()
        };
        let full = if self.describes.is_empty() {
            name
        } else {
            format!("{} → {name}", self.describes.join(" → "))
        };
        self.out.push(TestMarker {
            line: call.start_position().row as u32,
            scope: MarkerScope::Test,
            name: description,
            filter: test_core::filter::escape_regex(&full),
            span: span_of(self.call_chain(call)),
        });
        true
    }

    /// The call with the `->with(..)->skip()` modifiers chained onto it.
    fn call_chain<'t>(&self, call: Node<'t>) -> Node<'t> {
        let mut outer = call;
        while let Some(parent) = outer.parent().filter(|p| {
            p.kind() == "member_call_expression"
                && p.child_by_field_name("object").map(|o| o.id()) == Some(outer.id())
        }) {
            outer = parent;
        }
        outer
    }

    fn first_string_argument(&self, call: Node<'_>) -> Option<String> {
        let arguments = call.child_by_field_name("arguments")?;
        let mut cursor = arguments.walk();
        let argument = arguments.named_children(&mut cursor).next()?;
        let mut cursor = argument.walk();
        let value = if argument.kind() == "argument" {
            argument.named_children(&mut cursor).next()?
        } else {
            argument
        };
        if !matches!(value.kind(), "string" | "encapsed_string") {
            return None;
        }
        let raw = self.text_of(value);
        if value.kind() == "encapsed_string" && raw.contains('$') {
            return None;
        }
        let quote = raw.chars().next()?;
        let inner = raw.get(1..raw.len().checked_sub(1)?)?;
        Some(inner.replace(&format!("\\{quote}"), &quote.to_string()))
    }
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    fn filters(text: &str) -> Vec<(u32, MarkerScope, String)> {
        markers(text)
            .into_iter()
            .map(|m| (m.line, m.scope, m.filter))
            .collect()
    }

    #[test]
    fn a_phpunit_class_marks_the_class_and_each_test_method() {
        let text = r#"<?php
namespace App\Tests;

use PHPUnit\Framework\Attributes\Test;
use PHPUnit\Framework\TestCase;

final class GreeterTest extends TestCase
{
    public function testGreets(): void {}

    #[Test]
    public function it_adds(): void {}

    /** @test */
    public function it_subtracts(): void {}

    public function helper(): void {}

    private function testPrivate(): void {}
}
"#;
        assert_eq!(
            filters(text),
            vec![
                (
                    6,
                    MarkerScope::Group,
                    r"^App\\Tests\\GreeterTest::".to_string()
                ),
                (
                    8,
                    MarkerScope::Test,
                    r"^App\\Tests\\GreeterTest::testGreets( with data set .+)?$".to_string()
                ),
                (
                    11,
                    MarkerScope::Test,
                    r"^App\\Tests\\GreeterTest::it_adds( with data set .+)?$".to_string()
                ),
                (
                    14,
                    MarkerScope::Test,
                    r"^App\\Tests\\GreeterTest::it_subtracts( with data set .+)?$".to_string()
                ),
            ]
        );
    }

    #[test]
    fn markers_carry_the_name_the_menu_shows() {
        let text = "<?php\nclass GreeterTest extends TestCase {\n public function testGreets() {}\n}\ntest('adds', fn () => 1);\ndescribe('G', function () {});\n";
        let names: Vec<String> = markers(text).into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["GreeterTest", "testGreets", "adds", "G"]);
    }

    #[test]
    fn a_class_that_is_not_a_test_case_has_no_markers() {
        assert!(markers("<?php\nclass Greeter { public function testLike() {} }\n").is_empty());
    }

    #[test]
    fn a_framework_test_case_base_counts() {
        let text = "<?php\nclass A extends \\Symfony\\Bundle\\FrameworkBundle\\Test\\KernelTestCase {\n public function testX() {}\n}\n";
        assert_eq!(markers(text).len(), 2);
    }

    #[test]
    fn pest_calls_and_describe_blocks_are_marked_with_their_description() {
        let text = r#"<?php
test('adds numbers', function () {
    expect(1 + 1)->toBe(2);
});

it('does x (really)', fn () => true)->with([1, 2]);

describe('Greeter', function () {
    it('greets', function () {});
});
"#;
        assert_eq!(
            filters(text),
            vec![
                (1, MarkerScope::Test, "adds numbers".to_string()),
                (5, MarkerScope::Test, r"it does x \(really\)".to_string()),
                (7, MarkerScope::Group, "`Greeter` → ".to_string()),
                (8, MarkerScope::Test, "`Greeter` → it greets".to_string()),
            ]
        );
    }

    #[test]
    fn a_pest_call_with_a_computed_description_is_skipped() {
        assert!(markers("<?php\ntest($name, function () {});\n").is_empty());
    }

    const CLASS_FILE: &str = "<?php
class GreeterTest extends TestCase
{
    /** @test */
    public function it_adds(): void
    {
        $x = 1;
    }

    public function helper(): void
    {
    }

    #[Test]
    public function it_subs(): void {}
}

class OtherTest extends TestCase
{
    public function testB() {}
}
";

    fn at(text: &str, line: u32) -> Option<(MarkerScope, String)> {
        marker_at(text, line).map(|m| (m.scope, m.name))
    }

    #[test]
    fn the_caret_anywhere_in_a_test_method_selects_it() {
        let method = (MarkerScope::Test, "it_adds".to_string());
        // docblock, signature, body
        for line in [3, 4, 6] {
            assert_eq!(at(CLASS_FILE, line), Some(method.clone()), "line {line}");
        }
        assert_eq!(
            at(CLASS_FILE, 13),
            Some((MarkerScope::Test, "it_subs".into()))
        );
    }

    #[test]
    fn the_caret_in_a_non_test_method_or_the_class_head_selects_the_class() {
        let class = Some((MarkerScope::Group, "GreeterTest".to_string()));
        assert_eq!(at(CLASS_FILE, 10), class);
        assert_eq!(at(CLASS_FILE, 2), class);
    }

    #[test]
    fn each_class_of_a_file_is_selected_on_its_own() {
        assert_eq!(
            at(CLASS_FILE, 19),
            Some((MarkerScope::Test, "testB".into()))
        );
        assert_eq!(
            at(CLASS_FILE, 17),
            Some((MarkerScope::Group, "OtherTest".into()))
        );
    }

    #[test]
    fn a_caret_outside_every_class_selects_the_file() {
        let selected = marker_at(CLASS_FILE, 0).unwrap();
        assert_eq!(selected.scope, MarkerScope::File);
        assert_eq!(selected.filter, "");
    }

    #[test]
    fn a_file_without_tests_selects_nothing() {
        assert!(marker_at("<?php\nclass Greeter { function run() {} }\n", 1).is_none());
        assert!(marker_at("<?php\n", 0).is_none());
    }

    #[test]
    fn pest_selects_the_innermost_call_then_the_describe_then_the_file() {
        let text = "<?php\nuse function Pest\\x;\ndescribe('G', function () {\n    it('greets', function () {\n        expect(1)->toBe(1);\n    });\n    // gap\n});\ntest('top', fn () => 1);\n";
        assert_eq!(at(text, 4), Some((MarkerScope::Test, "greets".into())));
        assert_eq!(at(text, 6), Some((MarkerScope::Group, "G".into())));
        assert_eq!(at(text, 8), Some((MarkerScope::Test, "top".into())));
        assert_eq!(marker_at(text, 1).unwrap().scope, MarkerScope::File);
    }

    #[test]
    fn a_pest_call_spans_its_chained_modifiers() {
        let text = "<?php\ntest('a', function () {\n})\n    ->with([1])\n    ->skip();\n";
        let marker = marker_at(text, 4).unwrap();
        assert_eq!(
            (marker.scope, marker.name.as_str()),
            (MarkerScope::Test, "a")
        );
        assert_eq!(marker.span, (1, 4));
    }

    #[test]
    fn any_comment_above_a_method_is_in_its_span() {
        let text = "<?php\nclass ATest extends TestCase\n{\n    /** Adds. */\n    public function testAdds() {}\n}\n";
        assert_eq!(at(text, 3), Some((MarkerScope::Test, "testAdds".into())));
    }

    #[test]
    fn equal_spans_go_to_the_innermost_marker() {
        let text = "<?php\nclass ATest extends TestCase { public function testA() {} }\n";
        assert_eq!(at(text, 1), Some((MarkerScope::Test, "testA".into())));
        let pest = "<?php\ndescribe('G', function () { it('x', fn () => 1); });\n";
        assert_eq!(at(pest, 1), Some((MarkerScope::Test, "x".into())));
    }

    #[test]
    fn an_abstract_test_class_has_no_markers() {
        let text = "<?php\nabstract class BaseTest extends TestCase\n{\n    public function testA() {}\n}\n";
        assert!(markers(text).is_empty());
        assert!(marker_at(text, 3).is_none());
    }
}
