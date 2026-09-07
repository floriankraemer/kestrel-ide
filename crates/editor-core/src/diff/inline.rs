//! What changed *within* a modified hunk's lines.

use std::ops::Range;

use super::{Hunk, HunkKind, InlineDiff, InlineSpan};

/// Intra-line spans for a modified hunk, by word.
///
/// Only meaningful when the hunk has the same number of lines on each side —
/// otherwise there is no line-to-line correspondence to compare, and
/// inventing one produces worse output than highlighting the whole hunk.
/// Returns an empty diff in that case, which callers render as a whole-line
/// change.
pub fn diff_inline(before: &str, after: &str, hunk: &Hunk) -> InlineDiff {
    if hunk.kind != HunkKind::Modified || hunk.old.len() != hunk.new.len() {
        return InlineDiff::default();
    }
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();

    let mut out = InlineDiff::default();
    for (old_line, new_line) in hunk.old.clone().zip(hunk.new.clone()) {
        let (Some(old_text), Some(new_text)) = (old_lines.get(old_line), new_lines.get(new_line))
        else {
            continue;
        };
        let (removed, added) = word_spans(old_text, new_text);
        if !removed.is_empty() {
            out.removed.push(InlineSpan {
                line: old_line,
                range: removed,
            });
        }
        if !added.is_empty() {
            out.added.push(InlineSpan {
                line: new_line,
                range: added,
            });
        }
    }
    out
}

/// Trim the common prefix and suffix of two lines, and report what is left.
///
/// A word diff proper would be better on a heavily rewritten line, but this
/// is the case that actually matters — one identifier renamed, one argument
/// added — and it costs nothing. The prefix and suffix are trimmed on
/// **character** boundaries, so a multi-byte character is never split.
fn word_spans(old: &str, new: &str) -> (Range<usize>, Range<usize>) {
    if old == new {
        return (0..0, 0..0);
    }
    let prefix = old
        .char_indices()
        .zip(new.char_indices())
        .take_while(|((_, a), (_, b))| a == b)
        .map(|((i, c), _)| i + c.len_utf8())
        .last()
        .unwrap_or(0);

    let mut suffix = 0;
    let old_tail = &old[prefix..];
    let new_tail = &new[prefix..];
    for (a, b) in old_tail.chars().rev().zip(new_tail.chars().rev()) {
        if a != b {
            break;
        }
        suffix += a.len_utf8();
    }
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;

    // Snap outward to word boundaries.
    //
    // Without this, renaming `alpha` to `beta` highlights `alph` and `bet`,
    // because the two share a trailing "a" that the suffix trim eats. That is
    // the minimal span and it is unreadable: the eye expects a renamed
    // identifier to light up as a word, not as a word minus one letter.
    let (old_start, old_end) = snap_to_words(old, prefix, old_end.max(prefix));
    let (new_start, new_end) = snap_to_words(new, prefix, new_end.max(prefix));
    (old_start..old_end, new_start..new_end)
}

/// Widen `start..end` to cover whole words where it already cuts through one.
///
/// Only extends across word characters, so punctuation-only changes stay
/// tight — `f(a)` to `f(a, b)` still highlights `, b` rather than swallowing
/// the identifier beside it.
fn snap_to_words(text: &str, start: usize, end: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';

    let mut start = start;
    while start > 0 {
        let prev = text[..start].chars().next_back().unwrap_or(' ');
        let next = text[start..].chars().next().unwrap_or(' ');
        if is_word(prev) && is_word(next) {
            start -= prev.len_utf8();
        } else {
            break;
        }
    }

    let mut end = end;
    while end < text.len() {
        let prev = text[..end].chars().next_back().unwrap_or(' ');
        let next = text[end..].chars().next().unwrap_or(' ');
        if is_word(prev) && is_word(next) {
            end += next.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_lines;

    #[test]
    fn one_renamed_identifier_narrows_to_that_word() {
        let before = "let alpha = compute(1);\n";
        let after = "let beta = compute(1);\n";
        let hunks = diff_lines(before, after).unwrap();
        let inline = diff_inline(before, after, &hunks[0]);
        assert_eq!(inline.removed.len(), 1);
        assert_eq!(&before[inline.removed[0].range.clone()], "alpha");
        assert_eq!(&after[inline.added[0].range.clone()], "beta");
    }

    #[test]
    fn an_appended_argument_narrows_to_the_tail() {
        let before = "f(a)\n";
        let after = "f(a, b)\n";
        let hunks = diff_lines(before, after).unwrap();
        let inline = diff_inline(before, after, &hunks[0]);
        assert_eq!(&after[inline.added[0].range.clone()], ", b");
    }

    #[test]
    fn intra_line_spans_never_split_a_multi_byte_character() {
        let before = "let 中文 = 1;\n";
        let after = "let 中文 = 2;\n";
        let hunks = diff_lines(before, after).unwrap();
        let inline = diff_inline(before, after, &hunks[0]);
        for span in inline.removed.iter().chain(&inline.added) {
            assert!(
                before.is_char_boundary(span.range.start)
                    || after.is_char_boundary(span.range.start),
                "span starts mid-character"
            );
        }
    }

    /// A hunk whose sides have different line counts has no line-to-line
    /// correspondence, so there is nothing honest to narrow to.
    #[test]
    fn an_uneven_hunk_has_no_intra_line_detail() {
        let before = "a\nb\n";
        let after = "a\nb1\nb2\n";
        let hunks = diff_lines(before, after).unwrap();
        assert_eq!(diff_inline(before, after, &hunks[0]), InlineDiff::default());
    }
}
