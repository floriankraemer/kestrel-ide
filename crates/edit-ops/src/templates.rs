//! Live templates: abbreviation, surround and postfix expansion (ADR-0072).
//!
//! A template body is an LSP snippet (see [`crate::snippet`]) with two extra
//! variables: [`SELECTION_VAR`] (surround) and [`EXPR_VAR`] (postfix). Every
//! entry point here is a pure function of the text and returns an
//! [`Expansion`]: one edit plus the tab stops (and the caret) to walk
//! afterwards, in absolute byte offsets of the *edited* text. The caller
//! applies the edit and starts a snippet session over the stops, exactly as
//! it does for an LSP snippet completion.
//!
//! Which templates exist, and whether one fits [`Site`], is settings-model's
//! business; this module only knows how to put a body into a buffer.

use std::ops::Range;

use editor_core::transaction::TextEdit;
use syntax_core::Language;

use crate::indent::IndentStyle;
use crate::snippet::{self, Placeholder};
use crate::syntax::Syntax;

/// Where a surround template puts the selected text.
pub const SELECTION_VAR: &str = "$SELECTION$";
/// Where a postfix template puts the expression before the dot.
pub const EXPR_VAR: &str = "$EXPR$";

/// What an expansion does to the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub edit: TextEdit,
    /// Tab stops in walking order; empty when the body has none.
    pub stops: Vec<Range<usize>>,
    /// Where the caret goes when there is no stop to select: the end of the
    /// inserted text.
    pub caret: usize,
}

/// The kind of place a template is being expanded at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// Where a statement may start: a block or the top level.
    Statement,
    /// Where an expression is expected.
    Expression,
    /// Directly inside a class-like body.
    ClassBody,
    /// Anything else (inside HTML, a comment, a string).
    Other,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word that ends at `caret`, if there is one.
pub fn word_before(text: &str, caret: usize) -> Option<Range<usize>> {
    let head = text.get(..caret)?;
    let len: usize = head
        .chars()
        .rev()
        .take_while(|c| is_word(*c))
        .map(char::len_utf8)
        .sum();
    (len > 0).then(|| caret - len..caret)
}

/// What kind of place `word` sits at, judged on the text with the word
/// swapped for a plain identifier so a half-typed abbreviation cannot
/// distort the parse.
pub fn site_at(language: Language, text: &str, word: Range<usize>) -> Site {
    const PROBE: &str = "a";
    let mut probed = String::with_capacity(text.len());
    probed.push_str(&text[..word.start]);
    probed.push_str(PROBE);
    probed.push_str(&text[word.end..]);
    let syntax = Syntax::parse(language, &probed);
    let Some(tree) = syntax.tree() else {
        return Site::Other;
    };
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(word.start, word.start + PROBE.len());
    while let Some(current) = node {
        let kind = current.kind();
        if kind.contains("declaration_list") || kind == "class_body" {
            return Site::ClassBody;
        }
        if kind.contains("string") || kind.contains("comment") || kind == "text" {
            return Site::Other;
        }
        // A lone identifier where only declarations or statements belong
        // parses as an error node: what surrounds it says which.
        if kind == "ERROR" && probed[current.byte_range()].trim() == PROBE {
            if let Some(parent) = current.parent() {
                if !parent.kind().contains("declaration_list") {
                    return Site::Statement;
                }
            }
        }
        if kind.ends_with("expression_statement") {
            let alone = probed[current.byte_range()].trim_end_matches(';').trim() == PROBE;
            return if alone {
                Site::Statement
            } else {
                Site::Expression
            };
        }
        node = current.parent();
    }
    Site::Other
}

/// Replace `word` with `body`.
pub fn expand(text: &str, word: Range<usize>, body: &str, style: IndentStyle) -> Expansion {
    let body = body.replace(SELECTION_VAR, "").replace(EXPR_VAR, "");
    render(text, word, &body, style)
}

/// Wrap `selection` in `body` at its [`SELECTION_VAR`].
///
/// A selection over several lines is widened to whole lines: it starts after
/// the first line's indentation (which stays where it is) and loses a
/// trailing newline, and the lines inside are re-indented relative to the
/// template.
pub fn surround(text: &str, selection: Range<usize>, body: &str, style: IndentStyle) -> Expansion {
    let mut range = selection;
    if text[range.clone()].contains('\n') {
        let indent_end = range.start - text[..range.start].rsplit('\n').next().map_or(0, str::len)
            + indent_of(text, range.start).len();
        range.start = range
            .start
            .min(indent_end)
            .max(line_start(text, range.start));
        range.start = indent_end.min(range.end);
        while range.end > range.start && text[..range.end].ends_with(['\n', '\r']) {
            range.end -= 1;
        }
    }
    let base = indent_of(text, range.start);
    let selected = relativize(&text[range.clone()], base);
    let body = body.replace(EXPR_VAR, "");
    let body = substitute(&body, SELECTION_VAR, &selected);
    render(text, range, &body, style)
}

/// A `expr.abbr` the caret sits at the end of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostfixSite {
    /// The `abbr` after the dot.
    pub abbreviation: Range<usize>,
    /// The expression before the dot.
    pub expr: Range<usize>,
}

// ponytail: a substring match on grammar node names stands in for a per-language
// table; a grammar that names its access/call nodes otherwise yields no postfix
// site, which only disables postfix templates there.
const CHAIN_KINDS: [&str; 7] = [
    "access",
    "call",
    "subscript",
    "member",
    "name",
    "parenthesized",
    "scope",
];

/// The `expr.abbr` ending at `caret`: the expression is the longest chain of
/// accesses, calls and subscripts that ends right before the dot.
pub fn postfix_site(language: Language, text: &str, caret: usize) -> Option<PostfixSite> {
    let abbreviation = word_before(text, caret)?;
    let dot = abbreviation.start.checked_sub(1)?;
    if text.as_bytes().get(dot) != Some(&b'.') || dot == 0 {
        return None;
    }
    let syntax = Syntax::parse(language, &text[..dot]);
    let tree = syntax.tree()?;
    let mut node = tree.root_node().descendant_for_byte_range(dot - 1, dot)?;
    if node.end_byte() != dot {
        return None;
    }
    while let Some(parent) = node.parent() {
        let kind = parent.kind();
        if parent.end_byte() != dot || !CHAIN_KINDS.iter().any(|k| kind.contains(k)) {
            break;
        }
        node = parent;
    }
    let chain_kind = node.kind();
    if node.parent().is_none()
        || !CHAIN_KINDS.iter().any(|k| chain_kind.contains(k)) && !is_leaf_value(chain_kind)
    {
        return None;
    }
    Some(PostfixSite {
        abbreviation,
        expr: node.start_byte()..dot,
    })
}

fn is_leaf_value(kind: &str) -> bool {
    kind.contains("variable")
        || kind.contains("string")
        || kind.contains("integer")
        || kind.contains("float")
        || kind.contains("array")
        || kind == "this"
}

/// Replace `expr.abbr` with `body`, its [`EXPR_VAR`] standing for the expression.
pub fn postfix(text: &str, site: &PostfixSite, body: &str, style: IndentStyle) -> Expansion {
    let base = indent_of(text, site.expr.start);
    let expr = relativize(&text[site.expr.clone()], base);
    let body = body.replace(SELECTION_VAR, "");
    let body = substitute(&body, EXPR_VAR, &expr);
    render(text, site.expr.start..site.abbreviation.end, &body, style)
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The leading whitespace of the line containing `at`.
fn indent_of(text: &str, at: usize) -> &str {
    let start = line_start(text, at);
    let rest = &text[start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// `value` with `base` removed from the start of every line after the first.
fn relativize(value: &str, base: &str) -> String {
    let mut lines = value.split('\n');
    let mut out = lines.next().unwrap_or("").to_string();
    for line in lines {
        out.push('\n');
        out.push_str(line.strip_prefix(base).unwrap_or(line));
    }
    out
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

/// Put `value` where `var` is, continuation lines indented like the line
/// `var` stands on, and escaped so the snippet parser leaves it alone.
fn substitute(body: &str, var: &str, value: &str) -> String {
    let Some(at) = body.find(var) else {
        return body.to_string();
    };
    let lead = indent_of(body, at);
    let value = escape(value).replace('\n', &format!("\n{lead}"));
    body.replacen(var, &value, 1)
}

/// Indent, parse and position: the part every entry point shares.
fn render(text: &str, range: Range<usize>, body: &str, style: IndentStyle) -> Expansion {
    let base = indent_of(text, range.start);
    let unit = style.unit();
    let source = body
        .split('\n')
        .map(|line| {
            let tabs = line.len() - line.trim_start_matches('\t').len();
            format!("{}{}", unit.repeat(tabs), &line[tabs..])
        })
        .enumerate()
        .map(|(i, line)| {
            if i == 0 || line.is_empty() {
                line
            } else {
                format!("{base}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut parsed = snippet::parse(&source);
    let end_of_text = parsed.text.chars().count();
    if !parsed.placeholders.is_empty() && parsed.placeholders.iter().all(|p| p.number != 0) {
        parsed.placeholders.push(Placeholder {
            number: 0,
            range: end_of_text..end_of_text,
        });
    }
    let byte_at = |chars: usize| {
        parsed
            .text
            .char_indices()
            .nth(chars)
            .map_or(parsed.text.len(), |(i, _)| i)
    };
    let origin = range.start;
    let stops = snippet::stops(&parsed)
        .into_iter()
        .map(|r| origin + byte_at(r.start)..origin + byte_at(r.end))
        .collect();
    let caret = origin + parsed.text.len();
    Expansion {
        edit: TextEdit::new(range, parsed.text),
        stops,
        caret,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntax_core::language_by_id;

    fn php() -> Language {
        language_by_id("php").expect("php is in the catalog")
    }

    fn apply(text: &str, e: &Expansion) -> String {
        let mut out = text.to_string();
        out.replace_range(e.edit.range.clone(), &e.edit.text);
        out
    }

    const FORE: &str = "foreach ($1 as $2) {\n\t$0\n}";

    #[test]
    fn the_word_before_the_caret_is_the_abbreviation() {
        assert_eq!(word_before("  fore", 6), Some(2..6));
        assert_eq!(word_before("a fore ", 7), None);
        assert_eq!(word_before("$x->é_1", 8), Some(4..8));
    }

    #[test]
    fn expanding_indents_continuation_lines_and_lists_stops() {
        let text = "<?php\nfunction f() {\n    fore\n}\n";
        let word = word_before(text, text.find("fore").unwrap() + 4).unwrap();
        assert_eq!(&text[word.clone()], "fore");
        let e = expand(text, word, FORE, IndentStyle::default());
        let out = apply(text, &e);
        assert_eq!(
            out,
            "<?php\nfunction f() {\n    foreach ( as ) {\n        \n    }\n}\n"
        );
        // $1 and $2 are empty, $0 is the indented blank line.
        assert_eq!(e.stops.len(), 3);
        assert_eq!(&out[e.stops[0].clone()], "");
        assert!(out[..e.stops[0].start].ends_with("foreach ("));
        assert!(out[..e.stops[2].start].ends_with("{\n        "));
    }

    #[test]
    fn default_text_is_selected_and_a_missing_final_stop_is_appended() {
        let e = expand("x", 0..1, "if (${1:cond}) {}", IndentStyle::default());
        assert_eq!(e.edit.text, "if (cond) {}");
        assert_eq!(e.stops, vec![4..8, 12..12]);
        assert_eq!(e.caret, 12);
    }

    #[test]
    fn a_body_without_stops_puts_the_caret_at_the_end() {
        let e = expand("ab", 0..2, "dd();", IndentStyle::default());
        assert!(e.stops.is_empty());
        assert_eq!(e.caret, 5);
    }

    #[test]
    fn tabs_follow_the_indent_style() {
        let style = IndentStyle {
            tab_width: 2,
            use_spaces: true,
        };
        let e = expand("x", 0..1, "{\n\t$0\n}", style);
        assert_eq!(e.edit.text, "{\n  \n}");
    }

    #[test]
    fn surround_wraps_whole_lines_and_reindents_them() {
        let text = "<?php\n    a();\n    b();\n";
        let start = text.find("    a").unwrap();
        let end = text.len();
        let e = surround(
            text,
            start..end,
            "if ($1) {\n\t$SELECTION$\n}",
            IndentStyle::default(),
        );
        assert_eq!(
            apply(text, &e),
            "<?php\n    if () {\n        a();\n        b();\n    }\n"
        );
    }

    #[test]
    fn surround_escapes_what_the_snippet_grammar_would_eat() {
        let text = "$x = ${y};";
        let e = surround(
            text,
            0..text.len(),
            "try {\n\t$SELECTION$\n}",
            IndentStyle::default(),
        );
        assert_eq!(e.edit.text, "try {\n    $x = ${y};\n}");
    }

    #[test]
    fn expand_drops_the_surround_variable() {
        let e = expand("x", 0..1, "{ $SELECTION$ }", IndentStyle::default());
        assert_eq!(e.edit.text, "{  }");
    }

    #[test]
    fn a_postfix_site_covers_the_whole_access_chain() {
        let text = "<?php\n$a->b()->c.foreach";
        let site = postfix_site(php(), text, text.len()).unwrap();
        assert_eq!(&text[site.expr.clone()], "$a->b()->c");
        assert_eq!(&text[site.abbreviation.clone()], "foreach");
    }

    #[test]
    fn a_postfix_expression_stops_at_an_operator() {
        let text = "<?php\n$x = $items.foreach";
        let site = postfix_site(php(), text, text.len()).unwrap();
        assert_eq!(&text[site.expr.clone()], "$items");
    }

    #[test]
    fn postfix_replaces_expression_and_abbreviation() {
        let text = "<?php\nfunction f() {\n    $xs.foreach\n}\n";
        let caret = text.find(".foreach").unwrap() + ".foreach".len();
        let site = postfix_site(php(), text, caret).unwrap();
        let e = postfix(
            text,
            &site,
            "foreach ($EXPR$ as $1) {\n\t$0\n}",
            IndentStyle::default(),
        );
        assert_eq!(
            apply(text, &e),
            "<?php\nfunction f() {\n    foreach ($xs as ) {\n        \n    }\n}\n"
        );
    }

    #[test]
    fn there_is_no_postfix_site_without_a_dot_or_an_expression() {
        let plain = "<?php\n$xs foreach";
        assert!(postfix_site(php(), plain, plain.len()).is_none());
        let bare = "<?php\n.foreach";
        assert!(postfix_site(php(), bare, bare.len()).is_none());
    }

    #[test]
    fn the_site_distinguishes_statement_class_body_and_expression() {
        let at = |text: &str| {
            let caret = text.find('|').unwrap();
            let clean = text.replacen('|', "fore", 1);
            site_at(php(), &clean, caret..caret + 4)
        };
        assert_eq!(at("<?php\nfunction f() {\n    |\n}\n"), Site::Statement);
        assert_eq!(at("<?php\nclass A {\n    |\n}\n"), Site::ClassBody);
        assert_eq!(at("<?php\n$x = |;\n"), Site::Expression);
        assert_eq!(at("<?php\nfoo(|);\n"), Site::Expression);
    }
}
