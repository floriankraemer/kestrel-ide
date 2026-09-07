//! Line equality under each "ignore whitespace" setting.
//!
//! A [`Hunk`]'s line ranges are token *indices*, not content, so swapping
//! the token type's `Eq`/`Hash` for a whitespace-blind one is most of the
//! feature — the diff algorithm and the hunk builder never know. The one
//! mode that changes *which* lines are tokens at all
//! ([`WhitespaceMode::IgnoreAllAndBlankLines`]) keeps a token→line map per
//! side and translates the hunks back afterwards.

use std::hash::{Hash, Hasher};

use imara_diff::{Algorithm, Diff, InternedInput};

use super::tokens::Tokens;
use super::{build_hunks, Hunk};

/// Which whitespace differences count as a change — the diff viewer's
/// "Ignore whitespace" menu, in JetBrains' order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhitespaceMode {
    /// Every byte matters.
    #[default]
    Exact,
    /// Leading and trailing whitespace is ignored; inner whitespace is not.
    TrimEnds,
    /// All whitespace is ignored — `a b` and `a\t\tb` compare equal.
    IgnoreAll,
    /// As [`Self::IgnoreAll`], and a line that is only whitespace is not
    /// a line at all: adding or removing blank lines is not a change.
    IgnoreAllAndBlankLines,
}

impl WhitespaceMode {
    fn skips_blank_lines(self) -> bool {
        self == Self::IgnoreAllAndBlankLines
    }
}

/// A line, compared and hashed by the rule its mode names rather than by
/// its exact bytes.
#[derive(Clone, Copy)]
struct WsLine<'a> {
    text: &'a str,
    mode: WhitespaceMode,
}

impl PartialEq for WsLine<'_> {
    fn eq(&self, other: &Self) -> bool {
        match self.mode {
            WhitespaceMode::Exact => self.text == other.text,
            WhitespaceMode::TrimEnds => self.text.trim() == other.text.trim(),
            WhitespaceMode::IgnoreAll | WhitespaceMode::IgnoreAllAndBlankLines => self
                .text
                .split_whitespace()
                .eq(other.text.split_whitespace()),
        }
    }
}

impl Eq for WsLine<'_> {}

impl Hash for WsLine<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.mode {
            WhitespaceMode::Exact => self.text.hash(state),
            WhitespaceMode::TrimEnds => self.text.trim().hash(state),
            WhitespaceMode::IgnoreAll | WhitespaceMode::IgnoreAllAndBlankLines => {
                for word in self.text.split_whitespace() {
                    word.hash(state);
                }
                // A separator between words so "ab" "c" and "a" "bc" don't
                // collide.
                0u8.hash(state);
            }
        }
    }
}

/// One side's tokens plus, for the blank-line-skipping mode, which line each
/// token came from.
struct Side<'a> {
    tokens: Vec<WsLine<'a>>,
    /// `line_of[token_index]` — the identity map unless blank lines were
    /// skipped.
    line_of: Vec<usize>,
    line_count: usize,
}

impl<'a> Side<'a> {
    fn new(text: &'a str, mode: WhitespaceMode) -> Self {
        let mut tokens = Vec::new();
        let mut line_of = Vec::new();
        let mut line_count = 0;
        for (line, text) in imara_diff::sources::lines(text).enumerate() {
            line_count = line + 1;
            if mode.skips_blank_lines() && text.trim().is_empty() {
                continue;
            }
            tokens.push(WsLine { text, mode });
            line_of.push(line);
        }
        Self {
            tokens,
            line_of,
            line_count,
        }
    }

    /// A half-open token range back to the half-open line range it covers.
    /// Past the last token means "the end of the text" — so a hunk that
    /// ends in skipped blank lines still reaches the end of the file.
    fn lines(&self, tokens: std::ops::Range<usize>) -> std::ops::Range<usize> {
        let line_at = |token: usize| self.line_of.get(token).copied().unwrap_or(self.line_count);
        line_at(tokens.start)..line_at(tokens.end)
    }
}

/// Line hunks between `before` and `after` under `mode`. The caller has
/// already applied the size ceiling.
pub(super) fn diff_lines_with(before: &str, after: &str, mode: WhitespaceMode) -> Vec<Hunk> {
    let old = Side::new(before, mode);
    let new = Side::new(after, mode);
    let input = InternedInput::new(Tokens(&old.tokens), Tokens(&new.tokens));
    let diff = Diff::compute(Algorithm::Histogram, &input);
    build_hunks(&diff, |o, n| (old.lines(o), new.lines(n)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{diff_lines_opts, HunkKind};

    fn hunks(before: &str, after: &str, mode: WhitespaceMode) -> Vec<Hunk> {
        diff_lines_opts(before, after, mode).unwrap()
    }

    #[test]
    fn ignoring_whitespace_collapses_a_whitespace_only_edit() {
        assert!(
            hunks("a\n", "a \n", WhitespaceMode::IgnoreAll).is_empty(),
            "a trailing-space-only edit must vanish with IgnoreAll"
        );
        assert!(
            hunks("  a\tb  \n", "a b\n", WhitespaceMode::IgnoreAll).is_empty(),
            "reflowed inner whitespace must still compare equal"
        );
    }

    #[test]
    fn ignoring_whitespace_still_reports_a_real_content_change() {
        let hunks = hunks("a\n", "a b\n", WhitespaceMode::IgnoreAll);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Modified);
    }

    #[test]
    fn trimming_ends_ignores_indentation_but_not_inner_whitespace() {
        assert!(hunks("  a b\n", "a b  \n", WhitespaceMode::TrimEnds).is_empty());
        assert_eq!(
            hunks("a b\n", "a  b\n", WhitespaceMode::TrimEnds).len(),
            1,
            "inner whitespace is still a change when only the ends are trimmed"
        );
    }

    #[test]
    fn exact_mode_through_the_generic_path_matches_the_str_path() {
        let before = "1\n2\n3\n4\n";
        let after = "1\nX\n3\n\n4\n";
        assert_eq!(
            diff_lines_with(before, after, WhitespaceMode::Exact),
            hunks(before, after, WhitespaceMode::Exact)
        );
    }

    #[test]
    fn skipping_blank_lines_makes_an_inserted_blank_line_vanish() {
        assert!(hunks("a\nb\n", "a\n\nb\n", WhitespaceMode::IgnoreAllAndBlankLines).is_empty());
        assert!(hunks(
            "a\n\n\nb\n",
            "a\nb\n",
            WhitespaceMode::IgnoreAllAndBlankLines
        )
        .is_empty());
        assert_eq!(
            hunks("a\nb\n", "a\n\nb\n", WhitespaceMode::IgnoreAll).len(),
            1,
            "IgnoreAll alone still sees the blank line"
        );
    }

    #[test]
    fn skipping_blank_lines_keeps_real_hunks_on_their_real_line_numbers() {
        let before = "a\n\nb\nc\n";
        let after = "a\n\n\nb\nC\n";
        let hunks = hunks(before, after, WhitespaceMode::IgnoreAllAndBlankLines);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old, 3..4, "`c` is old line 3");
        assert_eq!(
            hunks[0].new,
            4..5,
            "`C` is new line 4, after two blank lines"
        );
        assert_eq!(hunks[0].kind, HunkKind::Modified);
    }

    #[test]
    fn a_hunk_at_the_end_of_a_file_that_ends_in_blank_lines_reaches_the_end() {
        let before = "a\nb\n\n\n";
        let after = "a\n\n\n";
        let hunks = hunks(before, after, WhitespaceMode::IgnoreAllAndBlankLines);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Removed);
        assert_eq!(hunks[0].old.start, 1);
        assert!(
            hunks[0].old.end <= 4 && hunks[0].old.end >= 2,
            "the removed range must stay inside the file: {:?}",
            hunks[0].old
        );
        assert_eq!(hunks[0].new.start, hunks[0].new.end);
        assert!(hunks[0].new.start <= 3);
    }

    #[test]
    fn skipping_blank_lines_on_two_empty_or_all_blank_texts_is_no_change() {
        assert!(hunks("", "\n\n", WhitespaceMode::IgnoreAllAndBlankLines).is_empty());
        assert!(hunks("\n", "", WhitespaceMode::IgnoreAllAndBlankLines).is_empty());
    }
}
