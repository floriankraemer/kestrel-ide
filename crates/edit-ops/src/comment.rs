//! Toggling comments.
//!
//! # The rule
//!
//! **If any covered line is uncommented, comment all of them; uncomment
//! only when every covered line is already commented.** This is the
//! JetBrains rule, and it is the one that makes `Ctrl+/` pressed twice on
//! a mixed block a no-op rather than a shuffle: any other rule turns a
//! selection where one line happens to be commented into a mess the user
//! then has to fix by hand.
//!
//! Blank lines inside a selection are skipped entirely — they are neither
//! commented nor counted when deciding — so a paragraph separated by empty
//! lines does not come back with `//` on the empty ones.
//!
//! The token goes at the **common indent** of the covered lines: the
//! shallowest indentation any of them has. Commenting a block therefore
//! leaves its shape intact instead of pushing every token to column 0.
//!
//! Every caret is served by one [`Transaction`], so a three-caret toggle is
//! one undo entry.

use std::collections::BTreeSet;

use editor_core::offsets::{line_of, line_range, line_starts};
use editor_core::selection::SelectionSet;
use editor_core::transaction::{TextEdit, Transaction};
use syntax_core::{Language, LanguageMap};

use crate::syntax::Tokens;

/// Toggle line comments over every line any caret touches.
///
/// A language with no line comment (HTML, CSS, Markdown) falls back to
/// [`toggle_block`], which is what `Ctrl+/` has to do there.
///
/// A line written in another language than the file's (the HTML of a `.php`
/// file, the `<script>` in it, the text around Twig tags) is commented with
/// *that* language's syntax, one line at a time.
pub fn toggle_line(language: Language, text: &str, selection: &SelectionSet) -> Transaction {
    let starts = line_starts(text);
    let lines: Vec<usize> = covered_lines(text, &starts, selection)
        .into_iter()
        .filter(|line| !is_blank(text, &starts, *line))
        .collect();
    let map = LanguageMap::of(language, text);
    let host_style = comment_syntax(language);
    // A language that comments exactly as the file's own does is the file's
    // own as far as this goes. It matters once a line is commented: the
    // comment `<!-- … -->` of a Markdown file is itself parsed as injected
    // HTML, and must not send the toggle back down the mixed path.
    let written_in: Vec<Language> = lines
        .iter()
        .map(|line| map.at(starts[*line] + indent_len(text, &starts, *line)))
        .map(|inner| {
            if comment_syntax(inner) == host_style {
                language
            } else {
                inner
            }
        })
        .collect();
    if written_in.iter().all(|l| *l == language) {
        return toggle_host_lines(language, text, selection);
    }
    toggle_mixed_lines(text, &starts, &lines, &written_in)
}

fn comment_syntax(language: Language) -> (Option<String>, Option<(String, String)>) {
    let tokens = Tokens::of(language);
    (tokens.line_comment, tokens.block_comment)
}

/// How one line is commented out: a line token, or a block pair wrapped
/// around its content.
enum LineStyle {
    Line(String),
    Block(String, String),
}

impl LineStyle {
    fn of(language: Language) -> Option<Self> {
        let tokens = Tokens::of(language);
        match (tokens.line_comment, tokens.block_comment) {
            (Some(token), _) => Some(Self::Line(token)),
            (None, Some((open, close))) => Some(Self::Block(open, close)),
            (None, None) => None,
        }
    }

    fn is_applied_to(&self, content: &str) -> bool {
        let content = content.trim();
        match self {
            Self::Line(token) => content.starts_with(token),
            Self::Block(open, close) => {
                content.len() >= open.len() + close.len()
                    && content.starts_with(open)
                    && content.ends_with(close)
            }
        }
    }
}

/// The toggle for a selection that spans languages: each line is judged and
/// edited in its own language, under the same rule as [`toggle_host_lines`] —
/// comment everything unless everything is already commented.
fn toggle_mixed_lines(
    text: &str,
    starts: &[usize],
    lines: &[usize],
    written_in: &[Language],
) -> Transaction {
    let styled: Vec<(usize, LineStyle)> = lines
        .iter()
        .zip(written_in)
        .filter_map(|(line, language)| Some((*line, LineStyle::of(*language)?)))
        .collect();
    if styled.is_empty() {
        return Transaction::empty();
    }
    let all_commented = styled
        .iter()
        .all(|(line, style)| style.is_applied_to(&text[line_range(text, starts, *line)]));
    if all_commented {
        return Transaction::new(
            styled
                .iter()
                .flat_map(|(line, style)| match style {
                    LineStyle::Line(token) => vec![uncomment(text, starts, *line, token)],
                    LineStyle::Block(open, close) => unwrap_line(text, starts, *line, open, close),
                })
                .collect(),
        );
    }
    let line_style_column = styled
        .iter()
        .filter(|(_, style)| matches!(style, LineStyle::Line(_)))
        .map(|(line, _)| indent_len(text, starts, *line))
        .min()
        .unwrap_or(0);
    Transaction::new(
        styled
            .iter()
            .flat_map(|(line, style)| match style {
                LineStyle::Line(token) => {
                    vec![TextEdit::insert(
                        starts[*line] + line_style_column,
                        format!("{token} "),
                    )]
                }
                LineStyle::Block(open, close) => {
                    let range = line_range(text, starts, *line);
                    let start = range.start + indent_len(text, starts, *line);
                    let end = range.start + text[range].trim_end().len();
                    vec![
                        TextEdit::insert(start, format!("{open} ")),
                        TextEdit::insert(end, format!(" {close}")),
                    ]
                }
            })
            .collect(),
    )
}

/// Remove a line's block delimiters and the single space
/// [`toggle_mixed_lines`] puts inside them.
fn unwrap_line(
    text: &str,
    starts: &[usize],
    line: usize,
    open: &str,
    close: &str,
) -> Vec<TextEdit> {
    let range = line_range(text, starts, line);
    let start = range.start + indent_len(text, starts, line);
    let end = range.start + text[range].trim_end().len();
    let mut open_end = start + open.len();
    if text[open_end..end].starts_with(' ') {
        open_end += 1;
    }
    let mut close_start = end - close.len();
    if close_start > open_end && text[..close_start].ends_with(' ') {
        close_start -= 1;
    }
    vec![
        TextEdit::delete(start..open_end),
        TextEdit::delete(close_start..end),
    ]
}

/// [`toggle_line`] for lines all written in the file's own language.
fn toggle_host_lines(language: Language, text: &str, selection: &SelectionSet) -> Transaction {
    let tokens = Tokens::of(language);
    let Some(token) = tokens.line_comment.clone() else {
        return toggle_block_with(&tokens, text, selection);
    };
    let starts = line_starts(text);
    let lines: Vec<usize> = covered_lines(text, &starts, selection)
        .into_iter()
        .filter(|line| !is_blank(text, &starts, *line))
        .collect();
    if lines.is_empty() {
        return Transaction::empty();
    }

    let commented = |line: &usize| {
        let range = line_range(text, &starts, *line);
        text[range].trim_start().starts_with(&token)
    };
    if lines.iter().all(commented) {
        return Transaction::new(
            lines
                .iter()
                .map(|line| uncomment(text, &starts, *line, &token))
                .collect(),
        );
    }

    let column = lines
        .iter()
        .map(|line| indent_len(text, &starts, *line))
        .min()
        .unwrap_or(0);
    Transaction::new(
        lines
            .iter()
            .map(|line| {
                let at = starts[*line] + column;
                TextEdit::insert(at, format!("{token} "))
            })
            .collect(),
    )
}

/// Toggle a block comment around every caret's selection.
///
/// A caret with a selection wraps exactly what it selected — which is what
/// makes `/* */` usable mid-expression — and a collapsed caret wraps its
/// line's content. A language with no block comment falls back to
/// [`toggle_line`].
///
/// The delimiters are those of the language the first caret is in, so a
/// block comment in the markup of a `.php` file is `<!-- -->`, not `/* */`.
pub fn toggle_block(language: Language, text: &str, selection: &SelectionSet) -> Transaction {
    let first = selection.carets().first().map_or(0, |caret| caret.start());
    let tokens = Tokens::of(LanguageMap::of(language, text).at(first));
    if tokens.block_comment.is_none() {
        return toggle_line(language, text, selection);
    }
    toggle_block_with(&tokens, text, selection)
}

fn toggle_block_with(tokens: &Tokens, text: &str, selection: &SelectionSet) -> Transaction {
    let Some((open, close)) = tokens.block_comment.clone() else {
        // Neither kind of comment: the language genuinely has none (JSON),
        // and inventing one would corrupt the file.
        return Transaction::empty();
    };
    let starts = line_starts(text);
    let mut edits = Vec::new();
    for caret in selection.carets() {
        let range = if caret.is_empty() {
            let line = line_range(text, &starts, line_of(&starts, caret.head));
            let start = line.start + indent_len(text, &starts, line_of(&starts, caret.head));
            let end = line.start + text[line.clone()].trim_end().len();
            start..end.max(start)
        } else {
            caret.range()
        };

        let inner = &text[range.clone()];
        if inner.starts_with(&open)
            && inner.ends_with(&close)
            && inner.len() >= open.len() + close.len()
        {
            // Already wrapped by exactly this selection: unwrap.
            edits.push(TextEdit::delete(range.start..range.start + open.len()));
            edits.push(TextEdit::delete(range.end - close.len()..range.end));
        } else if text[..range.start].ends_with(&open) && text[range.end..].starts_with(&close) {
            // Wrapped by delimiters just outside the selection.
            edits.push(TextEdit::delete(range.start - open.len()..range.start));
            edits.push(TextEdit::delete(range.end..range.end + close.len()));
        } else {
            edits.push(TextEdit::insert(range.start, open.clone()));
            edits.push(TextEdit::insert(range.end, close.clone()));
        }
    }
    Transaction::new(edits)
}

/// The lines any caret touches. A selection ending exactly at a line start
/// does not claim that line — dragging down to column 0 selects the lines
/// above it, and commenting the one below would surprise.
fn covered_lines(text: &str, starts: &[usize], selection: &SelectionSet) -> Vec<usize> {
    let mut lines = BTreeSet::new();
    for caret in selection.carets() {
        let first = line_of(starts, caret.start());
        let mut last = line_of(starts, caret.end().min(text.len()));
        if last > first && caret.end() == starts[last] {
            last -= 1;
        }
        lines.extend(first..=last);
    }
    lines.into_iter().collect()
}

fn indent_len(text: &str, starts: &[usize], line: usize) -> usize {
    let range = line_range(text, starts, line);
    let content = &text[range];
    content.len() - content.trim_start().len()
}

fn is_blank(text: &str, starts: &[usize], line: usize) -> bool {
    text[line_range(text, starts, line)].trim().is_empty()
}

/// Remove the token, plus the single space [`toggle_line`] puts after it,
/// so commenting and uncommenting are exact inverses.
fn uncomment(text: &str, starts: &[usize], line: usize, token: &str) -> TextEdit {
    let range = line_range(text, starts, line);
    let at = range.start + indent_len(text, starts, line);
    let mut end = at + token.len();
    if text[end..range.end].starts_with(' ') {
        end += 1;
    }
    TextEdit::delete(at..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::selection::{Caret, SelectionSet};
    use syntax_core::{language_by_id, BUILTIN_LANGUAGES};

    fn lang(id: &str) -> Language {
        language_by_id(id).unwrap_or_else(|| panic!("{id} is in the catalog"))
    }

    fn set(carets: &[(usize, usize)]) -> SelectionSet {
        SelectionSet::from_carets(
            carets
                .iter()
                .map(|(anchor, head)| Caret::new(*anchor, *head))
                .collect(),
            0,
        )
        .expect("within the caret ceiling")
    }

    fn at(offset: usize) -> SelectionSet {
        SelectionSet::single(Caret::at(offset))
    }

    fn toggled(id: &str, text: &str, selection: &SelectionSet) -> String {
        toggle_line(lang(id), text, selection)
            .apply(text)
            .expect("applies")
    }

    #[test]
    fn a_single_line_is_commented_and_uncommented() {
        let text = "let x = 1;\n";
        let once = toggled("rust", text, &at(0));
        assert_eq!(once, "// let x = 1;\n");
        assert_eq!(toggled("rust", &once, &at(0)), text);
    }

    #[test]
    fn a_mixed_selection_comments_every_line() {
        let text = "a();\n// b();\nc();\n";
        let selection = set(&[(0, text.len())]);
        assert_eq!(
            toggled("rust", text, &selection),
            "// a();\n// // b();\n// c();\n"
        );
    }

    #[test]
    fn a_fully_commented_selection_is_uncommented() {
        let text = "// a();\n// b();\n";
        assert_eq!(
            toggled("rust", text, &set(&[(0, text.len())])),
            "a();\nb();\n"
        );
    }

    #[test]
    fn the_token_lands_at_the_common_indent_and_keeps_the_shape() {
        let text = "    if x {\n        y();\n    }\n";
        let selection = set(&[(0, text.len())]);
        assert_eq!(
            toggled("rust", text, &selection),
            "    // if x {\n    //     y();\n    // }\n"
        );
    }

    #[test]
    fn blank_lines_inside_a_selection_are_skipped() {
        let text = "a();\n\n   \nb();\n";
        assert_eq!(
            toggled("rust", text, &set(&[(0, text.len())])),
            "// a();\n\n   \n// b();\n"
        );
    }

    #[test]
    fn a_selection_ending_at_a_line_start_does_not_claim_that_line() {
        let text = "a();\nb();\n";
        assert_eq!(toggled("rust", text, &set(&[(0, 5)])), "// a();\nb();\n");
    }

    #[test]
    fn three_carets_are_one_transaction() {
        let text = "a();\nb();\nc();\n";
        let selection = set(&[(0, 0), (5, 5), (10, 10)]);
        let transaction = toggle_line(lang("rust"), text, &selection);
        assert_eq!(transaction.edits.len(), 3);
        assert_eq!(
            transaction.apply(text).expect("applies"),
            "// a();\n// b();\n// c();\n"
        );
    }

    #[test]
    fn a_language_without_a_line_comment_falls_back_to_block() {
        let text = "<p>hi</p>\n";
        assert_eq!(toggled("html", text, &at(0)), "<!--<p>hi</p>-->\n");
    }

    #[test]
    fn the_html_of_a_php_file_is_commented_with_html_syntax() {
        let text = "<p>hi</p>\n<?php\necho 1;\n?>\n";
        let once = toggled("php", text, &at(0));
        assert_eq!(once, "<!-- <p>hi</p> -->\n<?php\necho 1;\n?>\n");
        assert_eq!(toggled("php", &once, &at(0)), text);
    }

    #[test]
    fn the_php_of_a_php_file_keeps_its_line_comment() {
        let text = "<p>hi</p>\n<?php\necho 1;\n?>\n";
        let caret = text.find("echo").unwrap();
        assert_eq!(
            toggled("php", text, &at(caret)),
            "<p>hi</p>\n<?php\n// echo 1;\n?>\n"
        );
    }

    #[test]
    fn a_selection_across_html_and_php_comments_each_line_in_its_own_language() {
        let text = "<p>hi</p>\n<?php\necho 1;\n?>\n";
        let echo = text.find("echo").unwrap();
        let once = toggled("php", text, &set(&[(0, 0), (echo, echo)]));
        assert_eq!(once, "<!-- <p>hi</p> -->\n<?php\n// echo 1;\n?>\n");
        let echo = once.find("// echo").unwrap();
        assert_eq!(toggled("php", &once, &set(&[(0, 0), (echo, echo)])), text);
    }

    #[test]
    fn javascript_in_a_script_element_is_commented_as_javascript() {
        let text = "<script>\nlet a = 1;\n</script>\n";
        let caret = text.find("let").unwrap();
        assert_eq!(
            toggled("html", text, &at(caret)),
            "<script>\n// let a = 1;\n</script>\n"
        );
    }

    #[test]
    fn twig_tags_use_twig_comments_and_the_markup_around_them_html() {
        let text = "<p>x</p>\n{% if a %}\n";
        assert_eq!(
            toggled("twig", text, &at(0)),
            "<!-- <p>x</p> -->\n{% if a %}\n"
        );
        let caret = text.find("{%").unwrap();
        assert_eq!(
            toggled("twig", text, &at(caret)),
            "<p>x</p>\n{#{% if a %}#}\n"
        );
    }

    #[test]
    fn a_block_comment_in_php_markup_uses_html_delimiters() {
        let text = "<p>hi</p>\n<?php\necho 1;\n?>\n";
        assert_eq!(
            toggle_block(lang("php"), text, &at(0))
                .apply(text)
                .expect("applies"),
            "<!--<p>hi</p>-->\n<?php\necho 1;\n?>\n"
        );
    }

    #[test]
    fn blade_comments_use_the_blade_syntax() {
        let text = "<p>x</p>\n";
        assert_eq!(toggled("blade", text, &at(0)), "{{--<p>x</p>--}}\n");
    }

    #[test]
    fn a_language_with_no_comment_syntax_at_all_changes_nothing() {
        let text = "{\"a\": 1}\n";
        assert_eq!(toggled("json", text, &at(0)), text);
    }

    #[test]
    fn a_block_comment_wraps_a_partial_line_selection_mid_expression() {
        let text = "foo(a, b);\n";
        let selection = set(&[(4, 8)]);
        let commented = toggle_block(lang("rust"), text, &selection)
            .apply(text)
            .expect("applies");
        assert_eq!(commented, "foo(/*a, b*/);\n");

        // Toggling the same span back — now offset by the opener — unwraps.
        let inner = set(&[(6, 10)]);
        assert_eq!(
            toggle_block(lang("rust"), &commented, &inner)
                .apply(&commented)
                .expect("applies"),
            text
        );
    }

    #[test]
    fn a_block_toggle_on_a_collapsed_caret_wraps_the_lines_content() {
        let text = "  a();\n";
        assert_eq!(
            toggle_block(lang("rust"), text, &at(3))
                .apply(text)
                .expect("applies"),
            "  /*a();*/\n"
        );
    }

    #[test]
    fn a_language_without_a_block_comment_falls_back_to_line() {
        let text = "x = 1\n";
        assert_eq!(
            toggle_block(lang("python"), text, &at(0))
                .apply(text)
                .expect("applies"),
            "# x = 1\n"
        );
    }

    /// The property that matters, over the whole catalog rather than once
    /// per language: whatever a toggle does, doing it again undoes it.
    /// Language #32 is covered the day its row is added.
    #[test]
    fn toggling_twice_restores_the_text_in_every_registered_language() {
        let text = "alpha\n    beta\n\ngamma\n";
        for def in BUILTIN_LANGUAGES {
            let language = lang(def.id);
            let selection = set(&[(0, text.len())]);
            let once = toggle_line(language, text, &selection)
                .apply(text)
                .expect("applies");
            if def.line_comment.is_none() && def.block_comment.is_none() {
                assert_eq!(once, text, "{}: no comment syntax, so no change", def.id);
                continue;
            }
            assert_ne!(once, text, "{}: toggling changed nothing", def.id);

            let twice_selection = set(&[(0, once.len())]);
            let twice = toggle_line(language, &once, &twice_selection)
                .apply(&once)
                .expect("applies");
            assert_eq!(twice, text, "{}: toggle is not its own inverse", def.id);
        }
    }
}
