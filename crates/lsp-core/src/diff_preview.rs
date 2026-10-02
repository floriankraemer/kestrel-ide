//! Whole-file before/after text and line hunks for one document in a
//! pending refactoring, for `RefactorPreviewDialog`'s diff panel (F3-15).
//!
//! `DocumentEdits` only carries the edits themselves — the preview needs the
//! finished text too, and a diff between the two, which is what this module
//! adds on top of [`crate::workspace_edit::apply_to_text`]. Kept out of
//! `workspace_edit` itself: that module is about what a `WorkspaceEdit`
//! means and how it applies, not about presenting the result.

use crate::workspace_edit::{apply_to_text, DocumentEdits, EditError, TextEdit};

/// One document's diff for the preview: the text it applies against, the
/// text it would produce, and the line hunks between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old_text: String,
    pub new_text: String,
    pub hunks: Vec<editor_core::diff::Hunk>,
}

/// Apply `doc`'s edits to `old_text` and diff the result against it.
///
/// Errors are `apply_to_text`'s — a stale or malformed edit is refused here
/// exactly as it would be when actually applied, so the preview never shows
/// a result the real apply could not produce. A diff over
/// [`editor_core::diff::MAX_DIFF_BYTES`] answers no hunks rather than
/// failing the whole preview: the two texts are still shown, just without
/// change markers, which is the same ceiling the gutter would apply.
pub fn file_diff(old_text: &str, doc: &DocumentEdits) -> Result<FileDiff, EditError> {
    let new_text = apply_to_text(old_text, &doc.edits)?;
    let hunks = editor_core::diff::diff_lines(old_text, &new_text).unwrap_or_default();
    Ok(FileDiff {
        old_text: old_text.to_string(),
        new_text,
        hunks,
    })
}

/// The minimal line-level edits that turn `old` into `new` — for a
/// formatter that hands back whole-file text, so the editor applies a few
/// small replacements (keeping the caret, folds and one undo step) instead
/// of replacing the document.
///
/// One edit per changed run of lines, last edit first — the order every
/// producer hands the editor, since each edit is applied against the text
/// the earlier ones already changed. A diff over
/// [`editor_core::diff::MAX_DIFF_BYTES`] falls back to one edit replacing
/// everything.
pub fn edits_between(old: &str, new: &str) -> Vec<TextEdit> {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let Ok(hunks) = editor_core::diff::diff_lines(old, new) else {
        return if old == new {
            Vec::new()
        } else {
            vec![replace_lines(
                &old_lines,
                0..old_lines.len(),
                new.to_string(),
            )]
        };
    };
    hunks
        .iter()
        .rev()
        .map(|h| replace_lines(&old_lines, h.old.clone(), new_lines[h.new.clone()].concat()))
        .collect()
}

/// An edit replacing `old_lines[range]` (whole lines, newline included).
/// A range reaching a last line that has no newline ends at that line's end
/// instead of at the start of a line that does not exist.
fn replace_lines(old_lines: &[&str], range: std::ops::Range<usize>, new_text: String) -> TextEdit {
    let (end_line, end_character) = match old_lines.get(range.end.wrapping_sub(1)) {
        Some(last) if range.end == old_lines.len() && !last.ends_with('\n') => {
            (range.end - 1, last.encode_utf16().count())
        }
        _ => (range.end, 0),
    };
    TextEdit {
        start_line: range.start as u32,
        start_character: 0,
        end_line: end_line as u32,
        end_character: end_character as u32,
        new_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_edit::TextEdit;
    use editor_core::diff::HunkKind;

    fn doc(edits: Vec<TextEdit>) -> DocumentEdits {
        DocumentEdits {
            uri: "file:///a.rs".to_string(),
            path: "a.rs".to_string(),
            version: None,
            edits,
        }
    }

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, text: &str) -> TextEdit {
        TextEdit {
            start_line: sl,
            start_character: sc,
            end_line: el,
            end_character: ec,
            new_text: text.to_string(),
        }
    }

    #[test]
    fn a_rename_produces_the_new_text_and_one_modified_hunk() {
        let old = "let alpha = 1;\n";
        let d = doc(vec![edit(0, 4, 0, 9, "beta")]);
        let diff = file_diff(old, &d).unwrap();
        assert_eq!(diff.new_text, "let beta = 1;\n");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].kind, HunkKind::Modified);
    }

    #[test]
    fn no_edits_means_no_hunks() {
        let old = "unchanged\n";
        let diff = file_diff(old, &doc(vec![])).unwrap();
        assert_eq!(diff.new_text, old);
        assert!(diff.hunks.is_empty());
    }

    #[test]
    fn an_out_of_bounds_edit_is_refused_like_apply_to_text() {
        let old = "one line\n";
        let d = doc(vec![edit(9, 0, 9, 1, "x")]);
        assert_eq!(file_diff(old, &d), Err(EditError::RangeOutOfBounds));
    }

    #[test]
    fn an_insertion_produces_an_added_hunk() {
        let old = "a\nc\n";
        let d = doc(vec![edit(1, 0, 1, 0, "b\n")]);
        let diff = file_diff(old, &d).unwrap();
        assert_eq!(diff.new_text, "a\nb\nc\n");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].kind, HunkKind::Added);
    }

    fn roundtrip(old: &str, new: &str) -> Vec<TextEdit> {
        let edits = edits_between(old, new);
        assert_eq!(
            apply_to_text(old, &edits).unwrap(),
            new,
            "{old:?} -> {new:?}"
        );
        edits
    }

    #[test]
    fn identical_texts_need_no_edits() {
        assert!(roundtrip("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn only_the_changed_lines_are_replaced() {
        let edits = roundtrip("a\n\tb\nc\n", "a\n    b\nc\n");
        assert_eq!(edits.len(), 1);
        assert_eq!(
            (
                edits[0].start_line,
                edits[0].end_line,
                edits[0].new_text.as_str()
            ),
            (1, 2, "    b\n")
        );
    }

    #[test]
    fn insertions_deletions_and_several_hunks_round_trip() {
        roundtrip("a\nc\n", "a\nb\nc\n");
        roundtrip("a\nb\nc\n", "a\nc\n");
        let edits = roundtrip("1\n2\n3\n4\n5\n", "1\nx\n3\ny\n5\n");
        assert!(edits[0].start_line > edits[1].start_line, "descending");
        roundtrip("", "<?php\n");
        roundtrip("<?php\n", "");
    }

    #[test]
    fn a_last_line_without_a_newline_is_handled_with_utf16_columns() {
        roundtrip("a\nb", "a\nb\n");
        roundtrip("a\nb", "a\n\u{1F600}x");
        roundtrip("a\r\nb\r\n", "a\r\nc\r\n");
        let edits = edits_between("é😀", "x");
        assert_eq!((edits[0].end_line, edits[0].end_character), (0, 3));
    }
}
