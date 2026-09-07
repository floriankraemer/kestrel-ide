//! Undoing a hunk: the edit that puts the *before* side back.
//!
//! Reverting must be **spliced into the open buffer**, exactly like every
//! other edit source this IDE has (a reformat, an intention, an applied AI
//! block) — so the same `beginEditBlock` makes it one `Ctrl+Z`, and a file
//! with unsaved changes elsewhere never gets clobbered by a whole-file
//! write. [`revert_hunk_edit`] is that edit; [`revert_hunks`] is the
//! whole-text form that doubles as the strongest test invariant a diff has.

use super::Hunk;

/// A whole-line replacement, in the shape the bridge turns directly into
/// an `FfiTextEdit` — the spirit of `lsp_core::workspace_edit::TextEdit`
/// (a range plus replacement text) but not its units: an LSP edit
/// addresses UTF-16 characters because a server can touch part of a line,
/// while a hunk revert only ever replaces whole lines (that is what a
/// [`Hunk`] *is*), so this is a half-open **line** range instead.
/// `start_line == end_line` is a pure insertion, exactly as an empty LSP
/// range is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEdit {
    /// 0-based, inclusive.
    pub start_line: usize,
    /// 0-based, exclusive.
    pub end_line: usize,
    /// Replacement text for the range, each line newline-terminated.
    pub new_text: String,
}

/// The edit that reverts one hunk of `after`, given the `before` text.
///
/// Both the VCS gutter's "Revert hunk" (`before` = `HEAD`'s copy) and the
/// diff viewer's apply chevron (`before` = whatever the left pane shows)
/// are this one function — a chevron needs no repository.
///
/// A trailing-newline mismatch at end-of-file is not handled, matching
/// `vcs_core::staging::hunk_patch`'s same simplification and for the same
/// reason: every caller today reads real files, which end in a newline.
pub fn revert_hunk_edit(before: &str, hunk: &Hunk) -> LineEdit {
    let old_lines: Vec<&str> = before.lines().collect();
    let mut new_text = String::new();
    for line in &old_lines[hunk.old.clone()] {
        new_text.push_str(line);
        new_text.push('\n');
    }
    LineEdit {
        start_line: hunk.new.start,
        end_line: hunk.new.end,
        new_text,
    }
}

/// Apply every hunk's *before* side back over `after`, undoing them all.
///
/// The strongest invariant available for testing a diff: reverting every
/// hunk of a diff must reproduce the original text exactly. Hunks are
/// applied in descending order so earlier line numbers stay valid.
pub fn revert_hunks(before: &str, after: &str, hunks: &[Hunk]) -> String {
    let old_lines: Vec<&str> = before.lines().collect();
    let mut new_lines: Vec<String> = after.lines().map(str::to_string).collect();

    for hunk in hunks.iter().rev() {
        let replacement: Vec<String> = old_lines[hunk.old.clone()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        new_lines.splice(hunk.new.clone(), replacement);
    }

    let mut out = new_lines.join("\n");
    // `lines()` drops the trailing terminator, so it is restored from the
    // text the result is meant to equal. Getting this wrong is how a revert
    // silently strips or adds a final newline.
    if before.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_lines;

    /// Apply a [`LineEdit`] to `text`, so a test can assert the *result* of
    /// splicing rather than just the edit's shape.
    fn apply(text: &str, edit: &LineEdit) -> String {
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        let replacement: Vec<String> = edit.new_text.lines().map(str::to_string).collect();
        lines.splice(edit.start_line..edit.end_line, replacement);
        let mut out = lines.join("\n");
        if text.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out
    }

    #[test]
    fn reverting_a_modification_replaces_just_that_line() {
        let before = "one\ntwo\nthree\n";
        let after = "one\nTWO\nthree\n";
        let hunk = diff_lines(before, after).unwrap().remove(0);
        let edit = revert_hunk_edit(before, &hunk);
        assert_eq!(edit.start_line, 1);
        assert_eq!(edit.end_line, 2);
        assert_eq!(edit.new_text, "two\n");
        assert_eq!(apply(after, &edit), before);
    }

    #[test]
    fn reverting_an_addition_deletes_the_added_lines() {
        let before = "a\nc\n";
        let after = "a\nb\nc\n";
        let hunk = diff_lines(before, after).unwrap().remove(0);
        let edit = revert_hunk_edit(before, &hunk);
        assert_eq!(edit.start_line, 1);
        assert_eq!(edit.end_line, 2);
        assert_eq!(edit.new_text, "");
        assert_eq!(apply(after, &edit), before);
    }

    #[test]
    fn reverting_a_deletion_reinserts_the_removed_lines() {
        let before = "a\nb\nc\n";
        let after = "a\nc\n";
        let hunk = diff_lines(before, after).unwrap().remove(0);
        let edit = revert_hunk_edit(before, &hunk);
        assert_eq!(
            edit.start_line, edit.end_line,
            "an insertion targets a point, not a range"
        );
        assert_eq!(edit.new_text, "b\n");
        assert_eq!(apply(after, &edit), before);
    }

    #[test]
    fn reverting_one_of_two_hunks_leaves_the_other_alone() {
        let before = "1\n2\n3\n4\n5\n";
        let after = "1\nX\n3\nY\n5\n";
        let hunks = diff_lines(before, after).unwrap();
        assert_eq!(hunks.len(), 2);

        let edit = revert_hunk_edit(before, &hunks[1]);
        let reverted = apply(after, &edit);
        assert!(reverted.contains('X'), "the first hunk must be untouched");
        assert!(!reverted.contains('Y'), "the second hunk must be reverted");
        assert!(reverted.contains('4'));
    }

    #[test]
    fn reverting_every_hunk_reproduces_the_before_text() {
        let cases = [
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("a\nc\n", "a\nb\nc\n"),
            ("a\nb\nc\n", "a\nc\n"),
            ("", "a\n"),
            ("a\n", ""),
            ("one\ntwo\nthree\nfour\n", "one\n2\nthree\n4\nfive\n"),
            ("x", "y"),
            ("a\nb\nc\nd\ne\nf\n", "a\nc\ne\n"),
            ("é\n中\n🙂\n", "é\nCHANGED\n🙂\n"),
            ("no trailing newline", "no trailing newline!"),
        ];
        for (before, after) in cases {
            let hunks = diff_lines(before, after).unwrap();
            assert_eq!(
                revert_hunks(before, after, &hunks),
                before,
                "reverting {after:?} did not reproduce {before:?}"
            );
        }
    }

    #[test]
    fn reverting_one_hunk_leaves_the_others_correctly_offset() {
        let before = "1\n2\n3\n4\n5\n";
        let after = "1\nX\n3\nY\n5\n";
        let hunks = diff_lines(before, after).unwrap();
        assert_eq!(hunks.len(), 2, "expected two separate hunks");

        // Revert only the second; the first must be untouched.
        let reverted = revert_hunks(before, after, &hunks[1..]);
        assert!(reverted.contains("X"), "the first hunk was reverted too");
        assert!(!reverted.contains("Y"), "the second hunk was not reverted");
    }
}
