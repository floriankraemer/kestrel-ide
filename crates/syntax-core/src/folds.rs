//! Foldable regions (Task C, plus F0-165's 3a/3b): [`Highlighter::fold_ranges`]
//! and the structural helpers behind the declaration-anchor (3a) and
//! comment-block (3b) folds. Split out once `lib.rs` hit its ratcheted
//! file-size ceiling, the same reason `diff_tab.rs`/`project_open.rs` exist
//! as siblings in other crates rather than growing their `lib.rs` further.

use tree_sitter::{QueryCursor, StreamingIterator};

use crate::{FoldRange, Highlighter};

impl Highlighter {
    /// Foldable regions (Task C) in document order, from the current
    /// incremental tree — i.e. whatever `set_text`/`edit` last left behind.
    /// Does not reparse: call after `set_text`/`edit`, not instead of it.
    /// Empty for [`crate::Language::PLAIN_TEXT`], a language with no
    /// `folds.scm`, or before the first `set_text`/`edit` call.
    pub fn fold_ranges(&self) -> Vec<FoldRange> {
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        let mut ranges = Vec::new();
        if let Some(query) = self.compiled.as_ref().and_then(|c| c.folds.as_ref()) {
            let mut cursor = QueryCursor::new();
            let mut matches = cursor.matches(query, tree.root_node(), self.text.as_bytes());
            while let Some(m) = matches.next() {
                for capture in m.captures {
                    let capture_name = query.capture_names()[capture.index as usize];
                    if capture_name == "fold" {
                        ranges.push(FoldRange {
                            start: capture.node.start_byte(),
                            end: capture.node.end_byte(),
                            anchor: declaration_anchor(capture.node),
                        });
                    }
                }
            }
        }
        ranges.extend(comment_block_fold_ranges(tree.root_node()));
        ranges.extend(directive_block_fold_ranges(tree.root_node(), &self.text));
        ranges.sort_by_key(|r| (r.start, r.end));
        ranges.dedup();
        ranges
    }
}

/// 3a: the fold marker belongs on the declaration that owns a block, not on
/// the block's own opening brace. Starting from the `@fold`-captured node,
/// climb ancestors while each parent's end byte equals the fold node's end
/// byte (the block is the parent's trailing child) and the parent starts
/// earlier than the fold node. This is a structural property of the parse
/// tree — a block is almost always the last child of the construct that
/// owns it — so it needs no per-language knowledge. Falls back to the fold
/// node's own start when no such ancestor exists.
fn declaration_anchor(fold_node: tree_sitter::Node) -> usize {
    let end = fold_node.end_byte();
    let mut anchor = fold_node.start_byte();
    let mut current = fold_node;
    while let Some(parent) = current.parent() {
        if parent.end_byte() != end || parent.start_byte() >= current.start_byte() {
            break;
        }
        anchor = parent.start_byte();
        current = parent;
    }
    anchor
}

/// 3b: fold ranges for comment blocks — runs of 2+ adjacent sibling comment
/// nodes with no blank line or other content between them. Node-kind names
/// containing `"comment"` cover every grammar in this workspace (`comment`,
/// `line_comment`, `block_comment`, ...), so this needs no per-language
/// query. A plain recursive walk is enough; folds don't need query-level
/// captures. The anchor is always the run's first line — no ancestor climb
/// needed, unlike 3a's declaration blocks.
fn comment_block_fold_ranges(node: tree_sitter::Node) -> Vec<FoldRange> {
    let mut ranges = Vec::new();
    let mut cursor = node.walk();
    let children: Vec<tree_sitter::Node> = node.children(&mut cursor).collect();

    let mut i = 0;
    while i < children.len() {
        if is_comment_node(&children[i]) {
            let run_start = i;
            let mut j = i + 1;
            while j < children.len()
                && is_comment_node(&children[j])
                && no_blank_line_between(&children[j - 1], &children[j])
            {
                j += 1;
            }
            let first = children[run_start];
            let last = children[j - 1];
            // A run spans 2+ lines either because it merges 2+ sibling
            // comment nodes, or because a single node (e.g. a `/* */`
            // block comment) already spans multiple lines by itself.
            if last.end_position().row > first.start_position().row {
                let start = first.start_byte();
                let end = last.end_byte();
                ranges.push(FoldRange {
                    start,
                    end,
                    anchor: start,
                });
            }
            i = j;
        } else {
            i += 1;
        }
    }

    for child in node.children(&mut node.walk()) {
        ranges.extend(comment_block_fold_ranges(child));
    }
    ranges
}

/// Twig's grammar is flat: `{% if %}` and `{% endif %}` are sibling
/// `statement_directive` nodes, so no query can capture the block between
/// them. This pairs each opening directive with its `end…` partner by name
/// (a stack, so nesting works) and folds from the end of the opener to the
/// end of the closer; the anchor is the opener's line. Other grammars have no
/// `statement_directive` nodes and get nothing from this.
fn directive_block_fold_ranges(root: tree_sitter::Node, text: &str) -> Vec<FoldRange> {
    let mut ranges = Vec::new();
    let mut open: Vec<(String, tree_sitter::Node)> = Vec::new();
    let mut cursor = root.walk();
    for node in root.children(&mut cursor) {
        if node.kind() != "statement_directive" {
            continue;
        }
        let Some(word) = directive_word(&text[node.byte_range()]) else {
            continue;
        };
        if let Some(closed) = word.strip_prefix("end") {
            let Some(depth) = open.iter().rposition(|(name, _)| name == closed) else {
                continue;
            };
            let opener = open[depth].1;
            open.truncate(depth);
            // A directive node swallows the whitespace before its `{%`, so
            // the rows compared are those of the closing `%}` of each.
            if node.end_position().row > opener.end_position().row {
                let opener_text = &text[opener.byte_range()];
                ranges.push(FoldRange {
                    start: opener.end_byte(),
                    end: node.end_byte(),
                    anchor: opener.start_byte() + opener_text.len()
                        - opener_text.trim_start().len(),
                });
            }
        } else if opens_a_block(&word, &text[node.byte_range()]) {
            open.push((word, node));
        }
    }
    ranges
}

/// The tag name of a `{% … %}` directive: `if` for `{%- if x -%}`.
fn directive_word(directive: &str) -> Option<String> {
    let inner = directive
        .trim_start()
        .trim_start_matches("{%")
        .trim_start_matches(['-', '~'])
        .trim_start();
    let word: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!word.is_empty()).then_some(word)
}

/// Tags that are closed by `end<tag>`. `set` only does when it captures a
/// block (`{% set x %}…{% endset %}`) rather than assigning (`{% set x = 1 %}`).
fn opens_a_block(word: &str, directive: &str) -> bool {
    match word {
        "if" | "for" | "block" | "macro" | "embed" | "with" | "apply" | "autoescape"
        | "sandbox" | "verbatim" | "trans" | "cache" => true,
        "set" => !directive.contains('='),
        _ => false,
    }
}

fn is_comment_node(node: &tree_sitter::Node) -> bool {
    node.kind().contains("comment")
}

/// True when `b` follows `a` with no blank source line between them — same
/// row, or the very next row. Two or more rows apart means at least one
/// blank line separates them, breaking the run.
fn no_blank_line_between(a: &tree_sitter::Node, b: &tree_sitter::Node) -> bool {
    b.start_position().row.saturating_sub(a.end_position().row) <= 1
}

#[cfg(test)]
mod tests {
    use crate::{language_by_id, Language};

    use super::*;

    fn lang(id: &str) -> Language {
        language_by_id(id).expect("catalog language")
    }
    fn rust() -> Language {
        lang("rust")
    }
    fn json() -> Language {
        lang("json")
    }
    fn csharp() -> Language {
        lang("csharp")
    }
    fn java() -> Language {
        lang("java")
    }
    fn php() -> Language {
        lang("php")
    }

    const CSHARP_SNIPPET: &str = "class Greeter {\n    public string Name;\n\n    public Greeter(string name) {\n        Name = name;\n    }\n\n    public string Greet() {\n        // say hi\n        return \"Hello, \" + Name;\n    }\n}\n";
    const JAVA_SNIPPET: &str = "public class Greeter {\n    private String name;\n\n    public Greeter(String name) {\n        this.name = name;\n    }\n\n    public String greet() {\n        // say hi\n        return \"Hello, \" + name;\n    }\n}\n";
    const PHP_SNIPPET: &str = "<?php\nclass Greeter {\n    public string $name;\n\n    public function __construct(string $name) {\n        $this->name = $name;\n    }\n\n    public function greet(): string {\n        // say hi\n        return \"Hello, \" . $this->name;\n    }\n}\n";

    #[test]
    fn plain_text_has_no_fold_ranges() {
        let mut highlighter = Highlighter::new(Language::PLAIN_TEXT);
        highlighter.set_text("hello");
        assert!(highlighter.fold_ranges().is_empty());
    }

    #[test]
    fn fold_ranges_are_empty_before_any_parse() {
        assert!(Highlighter::new(rust()).fold_ranges().is_empty());
    }

    #[test]
    fn rust_function_body_is_foldable() {
        let text = "fn add(x: i32, y: i32) -> i32 {\n    x + y\n}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        let body = ranges
            .iter()
            .find(|r| &text[r.start..r.end] == "{\n    x + y\n}")
            .expect("expected the function body to be foldable");
        assert_eq!(&text[body.start..body.end], "{\n    x + y\n}");
    }

    #[test]
    fn rust_struct_body_is_foldable() {
        let text = "struct Point {\n    x: i32,\n    y: i32,\n}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| &text[r.start..r.end] == "{\n    x: i32,\n    y: i32,\n}"),
            "expected the struct body to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn json_object_is_foldable() {
        let text = "{\"a\": 1, \"b\": [1, 2, 3]}";
        let mut highlighter = Highlighter::new(json());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges.iter().any(|r| &text[r.start..r.end] == text),
            "expected the whole object to be foldable: {ranges:?}"
        );
        assert!(
            ranges.iter().any(|r| &text[r.start..r.end] == "[1, 2, 3]"),
            "expected the nested array to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn csharp_method_body_is_foldable() {
        let mut highlighter = Highlighter::new(csharp());
        highlighter.set_text(CSHARP_SNIPPET);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| CSHARP_SNIPPET[r.start..r.end].starts_with("{\n        // say hi")),
            "expected the Greet() body to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn csharp_class_body_is_foldable() {
        let mut highlighter = Highlighter::new(csharp());
        highlighter.set_text(CSHARP_SNIPPET);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| r.start == CSHARP_SNIPPET.find('{').unwrap()
                    && r.end == CSHARP_SNIPPET.rfind('}').unwrap() + 1),
            "expected the class body to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn java_method_body_is_foldable() {
        let mut highlighter = Highlighter::new(java());
        highlighter.set_text(JAVA_SNIPPET);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| JAVA_SNIPPET[r.start..r.end].starts_with("{\n        // say hi")),
            "expected the greet() body to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn java_class_body_is_foldable() {
        let mut highlighter = Highlighter::new(java());
        highlighter.set_text(JAVA_SNIPPET);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| r.start == JAVA_SNIPPET.find('{').unwrap()
                    && r.end == JAVA_SNIPPET.rfind('}').unwrap() + 1),
            "expected the class body to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn php_arrays_matches_groups_heredocs_attributes_and_doc_comments_fold() {
        let text = "<?php\nuse App\\Models\\{\n    User,\n    Post,\n};\n#[Route(\n    '/x',\n)]\nclass A {\n    /**\n     * Doc.\n     */\n    public function f(int $a) {\n        $x = [\n            1,\n        ];\n        $y = match ($a) {\n            1 => 'a',\n            default => 'b',\n        };\n        $h = <<<EOT\n        text\n        EOT;\n        switch ($a) {\n            case 1: break;\n        }\n    }\n}\nenum E {\n    case A;\n}\n";
        let mut highlighter = Highlighter::new(php());
        highlighter.set_text(text);
        let folded: Vec<&str> = highlighter
            .fold_ranges()
            .iter()
            .map(|r| &text[r.start..r.end])
            .collect();
        for opener in [
            "{\n    User,",
            "(\n    '/x',\n)",
            "/**\n     * Doc.",
            "[\n            1,",
            "{\n            1 => 'a',",
            "<<<EOT",
            "{\n            case 1: break;",
            "{\n    case A;",
        ] {
            assert!(
                folded.iter().any(|f| f.starts_with(opener)),
                "no fold starts with {opener:?}: {folded:?}"
            );
        }
    }

    #[test]
    fn twig_directive_pairs_fold_and_nest() {
        let text = "{% block body %}\n{% for u in users %}\n<li>{{ u }}</li>\n{% endfor %}\n{% set x = 1 %}\n{% endblock %}\n";
        let mut highlighter = Highlighter::new(lang("twig"));
        highlighter.set_text(text);
        let folded: Vec<&str> = highlighter
            .fold_ranges()
            .iter()
            .map(|r| &text[r.anchor..r.end])
            .collect();
        assert_eq!(
            folded,
            [
                text.trim_end(),
                "{% for u in users %}\n<li>{{ u }}</li>\n{% endfor %}"
            ]
        );
    }

    #[test]
    fn twig_does_not_pair_an_assignment_or_a_stray_end_tag() {
        let text = "{% set x = 1 %}\n{% endif %}\n{% if a %}\nx\n{% endif %}\n";
        let mut highlighter = Highlighter::new(lang("twig"));
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        assert_eq!(ranges.len(), 1, "{ranges:?}");
        assert!(text[ranges[0].anchor..ranges[0].end].starts_with("{% if a %}"));
    }

    #[test]
    fn blade_sections_loops_and_elements_fold() {
        let text = "@section('c')\n@foreach ($us as $u)\n<ul>\n<li>{{ $u }}</li>\n</ul>\n@endforeach\n@endsection\n";
        let mut highlighter = Highlighter::new(lang("blade"));
        highlighter.set_text(text);
        let folded: Vec<&str> = highlighter
            .fold_ranges()
            .iter()
            .map(|r| &text[r.anchor..r.end])
            .collect();
        assert!(folded.contains(&text.trim_end()), "{folded:?}");
        assert!(
            folded.iter().any(|f| f.starts_with("@foreach")),
            "{folded:?}"
        );
        assert!(folded.iter().any(|f| f.starts_with("<ul>")), "{folded:?}");
    }

    #[test]
    fn php_method_body_is_foldable() {
        let mut highlighter = Highlighter::new(php());
        highlighter.set_text(PHP_SNIPPET);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| PHP_SNIPPET[r.start..r.end].starts_with("{\n        // say hi")),
            "expected the greet() body to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn php_class_body_is_foldable() {
        let mut highlighter = Highlighter::new(php());
        highlighter.set_text(PHP_SNIPPET);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| r.start == PHP_SNIPPET.find('{').unwrap()
                    && r.end == PHP_SNIPPET.rfind('}').unwrap() + 1),
            "expected the class body to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn fold_ranges_reflect_incremental_edits() {
        let old_text = "fn foo() {\n    1\n}";
        let new_text = "fn foo() {\n    1 + 2\n}";

        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(old_text);
        // Insert " + 2" right after "1" (byte offset 15..15 -> 15..19).
        highlighter.edit(new_text, 15, 15, 19);

        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| &new_text[r.start..r.end] == "{\n    1 + 2\n}"),
            "expected the fold range to track the edit: {ranges:?}"
        );
    }

    // --- fold anchor (3a) ---

    #[test]
    fn rust_function_fold_anchors_on_the_fn_declaration() {
        let text = "fn add(x: i32, y: i32) -> i32 {\n    x + y\n}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        let body = ranges
            .iter()
            .find(|r| &text[r.start..r.end] == "{\n    x + y\n}")
            .expect("expected the function body to be foldable");
        assert_eq!(
            body.anchor, 0,
            "anchor should be the start of `fn add(...)`"
        );
    }

    #[test]
    fn rust_function_fold_anchors_on_multi_line_signature() {
        let text = "fn add(\n    x: i32,\n    y: i32,\n) -> i32 {\n    x + y\n}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        let body = ranges
            .iter()
            .find(|r| text[r.start..r.end].starts_with("{\n    x + y"))
            .expect("expected the function body to be foldable");
        // Anchor sits on the declaration's first line, even though the
        // brace itself is several lines further down.
        assert_eq!(body.anchor, 0);
        assert_eq!(&text[body.anchor..body.anchor + 2], "fn");
        // The hidden range still starts at the body, not the anchor —
        // callers collapse from `start`, not `anchor`.
        assert!(body.start > body.anchor);
    }

    #[test]
    fn csharp_class_fold_anchors_on_the_class_declaration() {
        let mut highlighter = Highlighter::new(csharp());
        highlighter.set_text(CSHARP_SNIPPET);
        let ranges = highlighter.fold_ranges();
        let class_body = ranges
            .iter()
            .find(|r| r.start == CSHARP_SNIPPET.find('{').unwrap())
            .expect("expected the class body to be foldable");
        assert_eq!(class_body.anchor, 0, "anchor should be `class Greeter`");
    }

    #[test]
    fn php_class_fold_anchors_on_the_class_declaration() {
        let mut highlighter = Highlighter::new(php());
        highlighter.set_text(PHP_SNIPPET);
        let ranges = highlighter.fold_ranges();
        let class_body = ranges
            .iter()
            .find(|r| r.start == PHP_SNIPPET.find('{').unwrap())
            .expect("expected the class body to be foldable");
        let class_keyword = PHP_SNIPPET.find("class").unwrap();
        assert_eq!(class_body.anchor, class_keyword);
    }

    // --- comment block folds (3b) ---

    #[test]
    fn rust_doc_comment_block_is_foldable() {
        let text = "/// First line.\n/// Second line.\n/// Third line.\nfn foo() {}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        let comment_run = ranges
            .iter()
            .find(|r| r.start == 0 && text[r.start..r.end].starts_with("///"))
            .expect("expected the doc comment run to be foldable");
        assert_eq!(comment_run.anchor, 0);
        // Each line-comment node's own span includes its trailing newline.
        assert_eq!(
            &text[comment_run.start..comment_run.end],
            "/// First line.\n/// Second line.\n/// Third line.\n"
        );
    }

    #[test]
    fn c_style_block_comment_spanning_lines_is_foldable() {
        let text = "/*\n * A block comment\n * across lines.\n */\nfn foo() {}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        let block_end = text.find("*/").unwrap() + "*/".len();
        assert!(
            ranges
                .iter()
                .any(|r| r.start == 0 && r.end == block_end && r.anchor == 0),
            "expected the block comment to be foldable: {ranges:?}"
        );
    }

    #[test]
    fn single_line_comment_is_not_foldable() {
        let text = "// just one line\nfn foo() {}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        assert!(
            !ranges.iter().any(|r| r.start == 0),
            "a single-line comment must not produce a fold range: {ranges:?}"
        );
    }

    #[test]
    fn comments_separated_by_a_blank_line_are_not_merged() {
        let text = "// first\n\n// second\nfn foo() {}";
        let mut highlighter = Highlighter::new(rust());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        assert!(
            !ranges.iter().any(|r| r.start == 0),
            "a blank line must break the comment run: {ranges:?}"
        );
    }

    #[test]
    fn java_doc_comment_block_is_foldable() {
        let text = "/**\n * Greets someone.\n * Twice, even.\n */\npublic class Foo {}";
        let mut highlighter = Highlighter::new(java());
        highlighter.set_text(text);
        let ranges = highlighter.fold_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| r.start == 0 && r.anchor == 0 && r.end < text.len()),
            "expected the javadoc block to be foldable: {ranges:?}"
        );
    }
}
