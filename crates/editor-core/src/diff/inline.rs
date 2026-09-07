//! What changed *within* a modified hunk's lines.
//!
//! Each side of the hunk is one block of text (its lines joined back with
//! `\n`), tokenised by the [`HighlightMode`], and the two token lists are
//! diffed like lines are. That handles a 2-line-to-3-line rewrite the same
//! way as a one-word rename: there is no line-to-line pairing to invent,
//! because the block is compared as a whole and every changed token is
//! mapped back to the line it sits on.

use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};

use super::tokens::Tokens;
use super::{Hunk, HunkKind, InlineDiff, InlineSpan};

/// How finely a modified hunk is compared — the diff viewer's "Highlighting
/// mode" menu, in JetBrains' order.
///
/// [`Lines`](Self::Lines) and [`None`](Self::None) both produce no spans;
/// they differ only in what the *view* paints, which is why both exist here
/// rather than the view inventing a fifth state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightMode {
    /// Identifiers, whitespace runs and single punctuation characters.
    #[default]
    Words,
    /// Every character on its own.
    Chars,
    /// Whole lines only.
    Lines,
    /// No highlighting at all, not even line backgrounds.
    None,
}

/// Intra-line spans for a modified hunk, by word.
pub fn diff_inline(before: &str, after: &str, hunk: &Hunk) -> InlineDiff {
    diff_inline_opts(before, after, hunk, HighlightMode::Words)
}

/// Intra-line spans for a modified hunk under `mode`. Empty for an added or
/// removed hunk (nothing to pair), and for the two line-level modes.
pub fn diff_inline_opts(before: &str, after: &str, hunk: &Hunk, mode: HighlightMode) -> InlineDiff {
    if hunk.kind != HunkKind::Modified {
        return InlineDiff::default();
    }
    let tokenize: fn(&str) -> Vec<&str> = match mode {
        HighlightMode::Words => word_tokens,
        HighlightMode::Chars => char_tokens,
        HighlightMode::Lines | HighlightMode::None => return InlineDiff::default(),
    };
    let old = Block::new(before, hunk.old.clone());
    let new = Block::new(after, hunk.new.clone());
    let old_tokens = tokenize(&old.text);
    let new_tokens = tokenize(&new.text);

    let input = InternedInput::new(Tokens(&old_tokens), Tokens(&new_tokens));
    let diff = Diff::compute(Algorithm::Histogram, &input);

    let old_starts = token_starts(&old_tokens);
    let new_starts = token_starts(&new_tokens);
    let mut out = InlineDiff::default();
    for h in diff.hunks() {
        let before = h.before.start as usize..h.before.end as usize;
        let after = h.after.start as usize..h.after.end as usize;
        old.spans_for(
            byte_range(&old_starts, old.text.len(), before),
            &mut out.removed,
        );
        new.spans_for(
            byte_range(&new_starts, new.text.len(), after),
            &mut out.added,
        );
    }
    out
}

/// Byte offset of each token's start; tokens tile the text they came from.
fn token_starts(tokens: &[&str]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(tokens.len());
    let mut offset = 0;
    for token in tokens {
        starts.push(offset);
        offset += token.len();
    }
    starts
}

/// A half-open token range as a byte range; past the last token is the end
/// of the text.
fn byte_range(starts: &[usize], text_len: usize, tokens: Range<usize>) -> Range<usize> {
    let at = |token: usize| starts.get(token).copied().unwrap_or(text_len);
    at(tokens.start)..at(tokens.end)
}

/// One side of a hunk: its lines joined back into a block, with each line's
/// place in the block kept so a byte range maps back onto its lines.
struct Block {
    text: String,
    /// Byte offset of each line's start in `text`, plus each line's length
    /// (excluding the joining `\n`).
    lines: Vec<(usize, usize)>,
    first_line: usize,
}

impl Block {
    fn new(side: &str, range: Range<usize>) -> Self {
        let mut text = String::new();
        let mut lines = Vec::new();
        for (i, line) in side.lines().skip(range.start).take(range.len()).enumerate() {
            if i > 0 {
                text.push('\n');
            }
            lines.push((text.len(), line.len()));
            text.push_str(line);
        }
        Self {
            text,
            lines,
            first_line: range.start,
        }
    }

    /// Append one [`InlineSpan`] per line the byte range touches, each
    /// clipped to that line's content (the joining `\n` is nobody's).
    fn spans_for(&self, bytes: Range<usize>, out: &mut Vec<InlineSpan>) {
        if bytes.is_empty() {
            return;
        }
        for (i, &(start, len)) in self.lines.iter().enumerate() {
            let from = bytes.start.max(start);
            let to = bytes.end.min(start + len);
            if from < to {
                out.push(InlineSpan {
                    line: self.first_line + i,
                    range: from - start..to - start,
                });
            }
        }
    }
}

/// Runs of identifier characters, runs of blanks, and every other character
/// (punctuation, `\n`) on its own — so a renamed identifier lights up as a
/// word and `f(a)` → `f(a, b)` lights up `, b`, not the identifier beside it.
fn word_tokens(text: &str) -> Vec<&str> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Blank,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c == ' ' || c == '\t' {
            Class::Blank
        } else {
            Class::Other
        }
    };
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut current: Option<Class> = None;
    for (i, c) in text.char_indices() {
        let next = class(c);
        let breaks = match &current {
            None => false,
            Some(Class::Other) => true,
            Some(prev) => *prev != next,
        };
        if breaks {
            tokens.push(&text[start..i]);
            start = i;
        }
        current = Some(next);
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

fn char_tokens(text: &str) -> Vec<&str> {
    text.char_indices()
        .map(|(i, c)| &text[i..i + c.len_utf8()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_lines;

    fn inline(before: &str, after: &str, mode: HighlightMode) -> InlineDiff {
        let hunks = diff_lines(before, after).unwrap();
        assert_eq!(hunks.len(), 1, "these fixtures are one hunk each");
        diff_inline_opts(before, after, &hunks[0], mode)
    }

    fn removed<'a>(before: &'a str, d: &InlineDiff) -> Vec<&'a str> {
        let lines: Vec<&str> = before.lines().collect();
        d.removed
            .iter()
            .map(|s| &lines[s.line][s.range.clone()])
            .collect()
    }

    fn added<'a>(after: &'a str, d: &InlineDiff) -> Vec<&'a str> {
        let lines: Vec<&str> = after.lines().collect();
        d.added
            .iter()
            .map(|s| &lines[s.line][s.range.clone()])
            .collect()
    }

    #[test]
    fn one_renamed_identifier_narrows_to_that_word() {
        let before = "let alpha = compute(1);\n";
        let after = "let beta = compute(1);\n";
        let d = inline(before, after, HighlightMode::Words);
        assert_eq!(removed(before, &d), vec!["alpha"]);
        assert_eq!(added(after, &d), vec!["beta"]);
    }

    #[test]
    fn an_appended_argument_narrows_to_the_tail() {
        let before = "f(a)\n";
        let after = "f(a, b)\n";
        let d = inline(before, after, HighlightMode::Words);
        assert!(d.removed.is_empty());
        assert_eq!(added(after, &d), vec![", b"]);
    }

    #[test]
    fn by_character_narrows_to_the_changed_characters() {
        let before = "alpha\n";
        let after = "alpXa\n";
        let d = inline(before, after, HighlightMode::Chars);
        assert_eq!(removed(before, &d), vec!["h"]);
        assert_eq!(added(after, &d), vec!["X"]);
        let words = inline(before, after, HighlightMode::Words);
        assert_eq!(removed(before, &words), vec!["alpha"]);
    }

    #[test]
    fn an_uneven_hunk_still_gets_intra_line_detail() {
        // A Modified hunk whose sides have different line counts — built by
        // hand, since the line diff would call this a pure insertion — is
        // compared as one block, so the inserted line is found and the
        // surviving line stays clean.
        let before = "let a = 1;\nfoo();\n";
        let after = "let a = 1;\nbar(x);\nfoo();\n";
        let hunk = Hunk {
            old: 1..2,
            new: 1..3,
            kind: HunkKind::Modified,
        };
        let d = diff_inline_opts(before, after, &hunk, HighlightMode::Words);
        assert!(d.removed.is_empty(), "`foo();` survives unchanged");
        assert_eq!(added(after, &d), vec!["bar(x);"]);
        assert_eq!(d.added[0].line, 1);
    }

    #[test]
    fn a_change_spanning_lines_yields_one_span_per_line() {
        let before = "aa bb\ncc dd\n";
        let after = "aa XX\nYY dd\n";
        let d = inline(before, after, HighlightMode::Words);
        assert_eq!(removed(before, &d), vec!["bb", "cc"]);
        assert_eq!(d.removed[0].line, 0);
        assert_eq!(d.removed[1].line, 1);
        assert_eq!(added(after, &d), vec!["XX", "YY"]);
    }

    #[test]
    fn line_modes_produce_no_spans() {
        let before = "a\n";
        let after = "b\n";
        assert_eq!(
            inline(before, after, HighlightMode::Lines),
            InlineDiff::default()
        );
        assert_eq!(
            inline(before, after, HighlightMode::None),
            InlineDiff::default()
        );
    }

    #[test]
    fn added_and_removed_hunks_have_nothing_to_pair() {
        let hunks = diff_lines("a\n", "a\nb\n").unwrap();
        assert_eq!(
            diff_inline("a\n", "a\nb\n", &hunks[0]),
            InlineDiff::default()
        );
    }

    #[test]
    fn intra_line_spans_never_split_a_multi_byte_character() {
        let before = "let 中文 = 1;\n";
        let after = "let 中文 = 2;\n";
        for mode in [HighlightMode::Words, HighlightMode::Chars] {
            let d = inline(before, after, mode);
            for span in d.removed.iter() {
                assert!(before.is_char_boundary(span.range.start));
                assert!(before.is_char_boundary(span.range.end));
            }
            assert_eq!(removed(before, &d), vec!["1"]);
            assert_eq!(added(after, &d), vec!["2"]);
        }
    }

    #[test]
    fn word_tokens_split_identifiers_blanks_and_punctuation() {
        assert_eq!(
            word_tokens("foo_1(a,  b)\nx"),
            vec!["foo_1", "(", "a", ",", "  ", "b", ")", "\n", "x"]
        );
        assert!(word_tokens("").is_empty());
    }
}
