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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestMarker {
    /// 0-based line of the class/method name or of the call.
    pub line: u32,
    pub scope: MarkerScope,
    /// What the menu calls it: the class, method or Pest description.
    pub name: String,
    /// What the `--filter` flag takes to run exactly this.
    pub filter: String,
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
        let extends_test_case = {
            let mut cursor = class.walk();
            let found = class.children(&mut cursor).any(|child| {
                child.kind() == "base_clause"
                    && self
                        .text_of(child)
                        .trim_start_matches("extends")
                        .trim()
                        .ends_with("TestCase")
            });
            found
        };
        let (Some(name), Some(body)) = (
            class.child_by_field_name("name"),
            class.child_by_field_name("body"),
        ) else {
            return;
        };
        if !extends_test_case {
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
            let docblock_test =
                index
                    .checked_sub(1)
                    .map(|prev| members[prev])
                    .is_some_and(|prev| {
                        prev.kind() == "comment" && self.text_of(prev).contains("@test")
                    });
            if method_name.starts_with("test") || self.has_test_attribute(*member) || docblock_test
            {
                self.out.push(TestMarker {
                    line: method.start_position().row as u32,
                    scope: MarkerScope::Test,
                    name: method_name.to_string(),
                    // A data provider suffixes the name: `m with data set #0`.
                    filter: format!(
                        "^{escaped}::{}( with data set .+)?$",
                        test_core::filter::escape_regex(method_name)
                    ),
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
        });
        true
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
}
