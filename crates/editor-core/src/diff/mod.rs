//! Line and intra-line differences between two texts.
//!
//! Deliberately Git-free. A diff is two texts and what changed between them,
//! and the four places this IDE needs one are not all about version control:
//! the rename preview, the project-wide replace preview, the AI's apply flow
//! and the VCS gutter. Putting this in a `vcs-core` would mean a rename
//! preview needed a repository to show a diff, and a project with no
//! repository got none.
//!
//! # Layout
//!
//! - this module: the types, [`diff_lines`] and its whitespace-aware sibling;
//! - [`whitespace`]: what "the same line" means under each whitespace mode;
//! - [`inline`]: what changed *within* a modified hunk's lines;
//! - [`revert`]: the edit that undoes a hunk, the gutter's and the diff
//!   viewer's "revert this change".
//!
//! # Ceilings
//!
//! Diffing is linear in the texts, but the gutter runs it on every keystroke,
//! so callers get [`MAX_DIFF_BYTES`] to decide against rather than a
//! surprise. Past it, say so — a gutter that quietly shows no markers on a
//! large file is indistinguishable from one that thinks nothing changed.

mod inline;
mod revert;
mod tokens;
mod whitespace;

use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};

pub use inline::diff_inline;
pub use revert::{revert_hunk_edit, revert_hunks, LineEdit};
pub use whitespace::WhitespaceMode;

/// Texts above this are not diffed. Matches the highlighting ceiling in
/// `syntax-core`, so a file that is too big to colour is also too big to
/// mark up — one threshold for the user to understand rather than two.
pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

/// What happened to a run of lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Removed,
    Modified,
}

/// A run of changed lines, as 0-based half-open line ranges into each side.
///
/// An empty `old` means the lines were added; an empty `new` means they were
/// removed; both non-empty means modified. The kind is precomputed because
/// every consumer wants it and deriving it from two empty-range checks at
/// each call site is how they end up disagreeing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
    pub kind: HunkKind,
}

/// A changed span **within** a line, as byte offsets into that line.
///
/// This is what makes a diff readable when someone renamed one identifier on
/// a 200-character line: without it the whole line is highlighted and the
/// reader has to find the difference themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSpan {
    /// 0-based line number on the side this span belongs to.
    pub line: usize,
    pub range: Range<usize>,
}

/// Intra-line detail for one modified hunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineDiff {
    pub removed: Vec<InlineSpan>,
    pub added: Vec<InlineSpan>,
}

/// Why a diff was not produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffError {
    /// One of the texts is past [`MAX_DIFF_BYTES`]. Callers must say so
    /// rather than showing an empty diff, which reads as "nothing changed".
    TooLarge,
}

/// Line-level hunks between `before` and `after`, every byte significant.
pub fn diff_lines(before: &str, after: &str) -> Result<Vec<Hunk>, DiffError> {
    diff_lines_opts(before, after, WhitespaceMode::Exact)
}

/// Line-level hunks under a [`WhitespaceMode`] — the diff viewer's "Ignore
/// whitespace" menu.
///
/// Line *ranges* never depend on the mode, only which lines compare equal
/// does, so the same [`build_hunks`] maps a computed [`imara_diff::Diff`]
/// to [`Hunk`]s regardless of which token source produced it. [`Exact`]
/// goes through `imara_diff`'s own `&str` source, the others through
/// [`whitespace`]'s rule-carrying tokens.
///
/// [`Exact`]: WhitespaceMode::Exact
pub fn diff_lines_opts(
    before: &str,
    after: &str,
    mode: WhitespaceMode,
) -> Result<Vec<Hunk>, DiffError> {
    if before.len() > MAX_DIFF_BYTES || after.len() > MAX_DIFF_BYTES {
        return Err(DiffError::TooLarge);
    }
    if mode != WhitespaceMode::Exact {
        return Ok(whitespace::diff_lines_with(before, after, mode));
    }
    let input = InternedInput::new(before, after);
    let diff = Diff::compute(Algorithm::Histogram, &input);
    Ok(build_hunks(&diff, |old, new| (old, new)))
}

/// `imara_diff`'s hunks as [`Hunk`]s. `to_lines` maps each side's token
/// range to a line range — the identity when tokens *are* lines.
fn build_hunks(
    diff: &Diff,
    to_lines: impl Fn(Range<usize>, Range<usize>) -> (Range<usize>, Range<usize>),
) -> Vec<Hunk> {
    diff.hunks()
        .map(|h| {
            let (old, new) = to_lines(
                h.before.start as usize..h.before.end as usize,
                h.after.start as usize..h.after.end as usize,
            );
            let kind = match (old.is_empty(), new.is_empty()) {
                (true, false) => HunkKind::Added,
                (false, true) => HunkKind::Removed,
                _ => HunkKind::Modified,
            };
            Hunk { old, new, kind }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(before: &str, after: &str) -> Vec<HunkKind> {
        diff_lines(before, after)
            .unwrap()
            .iter()
            .map(|h| h.kind)
            .collect()
    }

    #[test]
    fn identical_texts_have_no_hunks() {
        assert!(diff_lines("a\nb\n", "a\nb\n").unwrap().is_empty());
    }

    #[test]
    fn a_pure_insertion_is_added() {
        assert_eq!(kinds("a\nc\n", "a\nb\nc\n"), vec![HunkKind::Added]);
    }

    #[test]
    fn a_pure_deletion_is_removed() {
        assert_eq!(kinds("a\nb\nc\n", "a\nc\n"), vec![HunkKind::Removed]);
    }

    #[test]
    fn a_replacement_is_modified() {
        assert_eq!(kinds("a\nb\nc\n", "a\nB\nc\n"), vec![HunkKind::Modified]);
    }

    #[test]
    fn insertion_at_the_start_and_end_are_found() {
        assert_eq!(kinds("b\n", "a\nb\n"), vec![HunkKind::Added]);
        assert_eq!(kinds("a\n", "a\nb\n"), vec![HunkKind::Added]);
    }

    #[test]
    fn a_file_that_becomes_empty_and_one_that_gains_content() {
        assert_eq!(kinds("a\nb\n", ""), vec![HunkKind::Removed]);
        assert_eq!(kinds("", "a\nb\n"), vec![HunkKind::Added]);
    }

    #[test]
    fn hunks_are_ascending_and_do_not_overlap() {
        let before = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let after = "1\nX\n3\n4\nY\n6\n7\nZ\n";
        let hunks = diff_lines(before, after).unwrap();
        assert!(hunks.len() >= 2);
        for pair in hunks.windows(2) {
            assert!(
                pair[0].new.end <= pair[1].new.start,
                "hunks overlap: {:?}",
                pair
            );
            assert!(pair[0].old.end <= pair[1].old.start);
        }
    }

    #[test]
    fn crlf_is_not_mistaken_for_a_change() {
        // Same content, same terminators: no hunks. (Mixed terminators are a
        // real difference and are reported as one, which is correct.)
        assert!(diff_lines("a\r\nb\r\n", "a\r\nb\r\n").unwrap().is_empty());
    }

    #[test]
    fn a_whitespace_only_change_is_still_a_change() {
        assert_eq!(kinds("a\n", "a \n"), vec![HunkKind::Modified]);
    }

    #[test]
    fn a_text_past_the_ceiling_is_refused_rather_than_reported_as_unchanged() {
        let big = "x\n".repeat(MAX_DIFF_BYTES);
        assert_eq!(diff_lines(&big, "y\n"), Err(DiffError::TooLarge));
        assert_eq!(diff_lines("y\n", &big), Err(DiffError::TooLarge));
    }
}
