//! Carets and selections: the state a multi-caret edit is computed from.
//!
//! A [`SelectionSet`] is one or more [`Caret`]s with a designated primary.
//! Its invariant — restored on every mutation, never lazily — is that the
//! carets are sorted by position and that no two of them overlap or touch.
//! Normalisation happens **before** an edit is computed, never after: two
//! carets left sitting on the same offset would produce two edits at that
//! offset, and the classic multi-caret bug is the doubled character that
//! falls out of it.
//!
//! Offsets are **byte** offsets into the text the carets belong to, matching
//! tree-sitter and the project index. The FFI seam speaks UTF-16 code units;
//! [`crate::offsets`] is the one place that conversion happens.
//!
//! Every entry point here takes `text: &str` rather than a [`crate::Document`]:
//! the rope is only refreshed on save, so it is one save behind the live Qt
//! buffer at all times. This is the same stateless shape
//! [`crate::find_matches`] already has, for the same reason.
//!
//! **Ceiling: 1024 carets** ([`MAX_CARETS`]). An operation that would exceed
//! it is refused with [`SelectionError::TooManyCarets`] rather than silently
//! truncating — a selection that quietly stops covering what the user dragged
//! over is worse than one that refuses.

use crate::offsets::{clamp_to_boundary, line_of, line_range, line_starts};

/// The most carets one [`SelectionSet`] may hold.
///
/// Not a performance limit so much as a blast radius: every caret is an edit
/// in one transaction crossing the FFI seam per keystroke, and there is a
/// number past which "select all occurrences" is a mistake the user wants
/// refused rather than obeyed.
pub const MAX_CARETS: usize = 1024;

/// One caret: a fixed `anchor` and a moving `head`, both byte offsets.
///
/// `anchor == head` is a plain cursor; otherwise the caret carries a
/// selection, which runs backwards when `head < anchor`. The direction is
/// preserved because Shift+Arrow must shrink the selection from the end the
/// user grew it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub anchor: usize,
    pub head: usize,
}

impl Caret {
    /// A collapsed caret — no selection.
    pub fn at(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    /// A caret selecting `anchor..head`, in whichever direction they name.
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// The lower of the two offsets.
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// The higher of the two offsets.
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// Whether this is a plain cursor rather than a selection.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Whether the head sits before the anchor.
    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// The byte range this caret covers, half-open.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start()..self.end()
    }

    /// This caret respanned over `start..end`, keeping its direction.
    fn respan(&self, start: usize, end: usize) -> Self {
        if self.is_reversed() {
            Self {
                anchor: end,
                head: start,
            }
        } else {
            Self {
                anchor: start,
                head: end,
            }
        }
    }
}

/// Why a selection operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    /// The operation would push the set past [`MAX_CARETS`]. Nothing was
    /// changed — the alternative, truncating, hides the refusal in a place
    /// the user only finds by counting.
    TooManyCarets { requested: usize, max: usize },
}

impl SelectionError {
    /// Refusing more carets than the editor works with (ADR-0003 §4:
    /// 900–999 is `editor-core`/`edit-ops`' range).
    pub const CODE_TOO_MANY_CARETS: i32 = 900;

    /// The variant's stable numeric code. Append-only: this crosses the FFI
    /// seam, where it used to arrive as a bare `1` that meant nothing on its
    /// own.
    pub fn code(&self) -> i32 {
        match self {
            SelectionError::TooManyCarets { .. } => Self::CODE_TOO_MANY_CARETS,
        }
    }
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionError::TooManyCarets { requested, max } => write!(
                f,
                "{requested} carets is more than the {max} this editor works with at once"
            ),
        }
    }
}

impl std::error::Error for SelectionError {}

/// One or more carets with a primary, kept sorted and non-overlapping.
///
/// A set is never empty: collapsing, merging and mapping all preserve at
/// least the primary caret, so callers never have to handle "no caret".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionSet {
    /// Sorted ascending by `start()`, no two overlapping or touching.
    carets: Vec<Caret>,
    /// Index into `carets`. Always in range.
    primary: usize,
}

impl SelectionSet {
    /// A set holding one caret, which is therefore the primary.
    pub fn single(caret: Caret) -> Self {
        Self {
            carets: vec![caret],
            primary: 0,
        }
    }

    /// A set from arbitrary carets, normalised. `primary` indexes `carets`
    /// as given; if it is out of range the first caret becomes primary.
    ///
    /// Refuses more than [`MAX_CARETS`] carets *after* normalisation, so a
    /// list that merges down to a legal size is accepted.
    pub fn from_carets(carets: Vec<Caret>, primary: usize) -> Result<Self, SelectionError> {
        if carets.is_empty() {
            return Ok(Self::single(Caret::at(0)));
        }
        let primary = if primary < carets.len() { primary } else { 0 };
        let (carets, primary) = normalise(carets, primary);
        check_ceiling(carets.len())?;
        Ok(Self { carets, primary })
    }

    /// The carets, ascending by position. Never empty.
    pub fn carets(&self) -> &[Caret] {
        &self.carets
    }

    /// The caret keyboard navigation and single-caret operations act on.
    pub fn primary(&self) -> Caret {
        self.carets[self.primary]
    }

    /// Index of the primary caret within [`SelectionSet::carets`].
    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// How many carets the set holds; at least one.
    pub fn len(&self) -> usize {
        self.carets.len()
    }

    /// Always `false` — a set always holds at least one caret. Present so
    /// clippy's `len_without_is_empty` does not push callers toward a check
    /// that can never fire.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Whether more than one caret is active — what the view asks before
    /// routing a keystroke through a transaction.
    pub fn is_multi(&self) -> bool {
        self.carets.len() > 1
    }

    /// Add a caret, which becomes the primary (Alt+Click, Ctrl+D).
    ///
    /// If it overlaps or touches an existing caret the two merge, and the
    /// merged caret is the primary — clicking inside an existing selection
    /// moves the focus there rather than growing the set.
    pub fn add_caret(&mut self, caret: Caret) -> Result<(), SelectionError> {
        let mut carets = self.carets.clone();
        let added = carets.len();
        carets.push(caret);
        let (carets, primary) = normalise(carets, added);
        // Checked before anything is written back, so a refusal leaves the
        // set exactly as it was.
        check_ceiling(carets.len())?;
        self.carets = carets;
        self.primary = primary;
        Ok(())
    }

    /// Drop every secondary caret (Esc).
    ///
    /// The primary keeps its selection — Esc's first job is to stop editing
    /// in N places, not to lose what is selected. A second Esc collapsing
    /// the selection is the view's decision, not this type's.
    pub fn collapse_to_primary(&mut self) {
        let primary = self.primary();
        self.carets = vec![primary];
        self.primary = 0;
    }

    /// Ctrl+D: select the next occurrence of what the primary caret holds.
    ///
    /// With an empty primary caret the first press selects the **word under
    /// the caret** in place and adds nothing — the same two-step every editor
    /// with this binding uses, so that Ctrl+D Ctrl+D selects one word and
    /// then its next occurrence.
    ///
    /// Otherwise the primary's text is searched for, starting after the
    /// primary and **wrapping at the end of the text**; occurrences already
    /// covered by a caret are skipped, so repeated presses walk forward
    /// instead of re-finding the caret they started from. The new caret
    /// becomes the primary.
    ///
    /// **Whole-word rule**: the search is whole-word exactly when the primary
    /// selection is itself a whole word *in situ* — i.e. bounded by non-word
    /// characters in `text`. Selecting `value` matches `value` but not
    /// `values`; selecting the `alu` inside `value` matches substrings, which
    /// is the only reading under which a deliberate partial selection does
    /// anything at all.
    ///
    /// Returns whether anything changed.
    pub fn add_next_occurrence(&mut self, text: &str) -> Result<bool, SelectionError> {
        let primary = self.primary();
        if primary.is_empty() {
            let Some(word) = word_at(text, primary.head) else {
                return Ok(false);
            };
            self.carets[self.primary] = Caret::new(word.start, word.end);
            return Ok(true);
        }

        let start = clamp_to_boundary(text, primary.start());
        let end = clamp_to_boundary(text, primary.end());
        if start >= end {
            return Ok(false);
        }
        let needle = &text[start..end];
        let whole_word = is_word_boundary_span(text, start, end);

        let mut from = end;
        let mut wrapped = false;
        loop {
            let found = match text[from..].find(needle) {
                Some(at) => from + at,
                None if wrapped => return Ok(false),
                None => {
                    wrapped = true;
                    from = 0;
                    continue;
                }
            };
            let hit_end = found + needle.len();
            if wrapped && found >= end {
                // Back past where we started without a free occurrence.
                return Ok(false);
            }
            from = found + 1;
            while from < text.len() && !text.is_char_boundary(from) {
                from += 1;
            }
            if whole_word && !is_word_boundary_span(text, found, hit_end) {
                continue;
            }
            if self.covers(found, hit_end) {
                continue;
            }
            self.add_caret(Caret::new(found, hit_end))?;
            return Ok(true);
        }
    }

    /// Whether any caret already spans exactly, or contains, `start..end`.
    fn covers(&self, start: usize, end: usize) -> bool {
        self.carets
            .iter()
            .any(|c| c.start() <= start && c.end() >= end)
    }

    /// Move every caret by `motion` at once — the tested rule that lifts the
    /// ADR-0023 ceiling ("more than one caret collapses to the primary")
    /// for the keys it actually applies to. `extend` is Shift held: the
    /// anchor stays put and only the head moves, same as a single caret's
    /// arrow keys. `tab_width` only matters for `Up`/`Down`, which keep the
    /// caret's visual column the way [`column_block`] already does for a
    /// column-selection drag.
    pub fn move_carets(
        &self,
        text: &str,
        motion: CaretMotion,
        extend: bool,
        tab_width: usize,
    ) -> Result<SelectionSet, SelectionError> {
        let starts = line_starts(text);
        let mapped: Vec<Caret> = self
            .carets
            .iter()
            .map(|caret| {
                let head = moved_head(text, &starts, caret, motion, tab_width);
                let anchor = if extend { caret.anchor } else { head };
                Caret::new(anchor, head)
            })
            .collect();
        SelectionSet::from_carets(mapped, self.primary)
    }
}

/// A keyboard motion every caret in a [`SelectionSet`] can make at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaretMotion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
}

/// Where `caret`'s head goes for `motion`, un-extended.
///
/// A caret that already carries a selection collapses to the edge `Left` or
/// `Right` points at rather than moving past it — the same "first press
/// dismisses the selection" rule a single caret's arrow keys already follow.
/// Every other motion (and the two horizontal ones once there is no
/// selection to collapse) reads from the head.
fn moved_head(
    text: &str,
    starts: &[usize],
    caret: &Caret,
    motion: CaretMotion,
    tab_width: usize,
) -> usize {
    if !caret.is_empty() {
        match motion {
            CaretMotion::Left => return caret.start(),
            CaretMotion::Right => return caret.end(),
            _ => {}
        }
    }
    match motion {
        CaretMotion::Left => prev_char(text, caret.head),
        CaretMotion::Right => next_char(text, caret.head),
        CaretMotion::Home => line_range(text, starts, line_of(starts, caret.head)).start,
        CaretMotion::End => line_range(text, starts, line_of(starts, caret.head)).end,
        CaretMotion::Up => vertical(text, starts, caret.head, tab_width, -1),
        CaretMotion::Down => vertical(text, starts, caret.head, tab_width, 1),
        CaretMotion::WordLeft => word_left(text, caret.head),
        CaretMotion::WordRight => word_right(text, caret.head),
    }
}

fn prev_char(text: &str, offset: usize) -> usize {
    let offset = clamp_to_boundary(text, offset);
    match text[..offset].chars().next_back() {
        Some(c) => offset - c.len_utf8(),
        None => 0,
    }
}

fn next_char(text: &str, offset: usize) -> usize {
    let offset = clamp_to_boundary(text, offset);
    match text[offset..].chars().next() {
        Some(c) => offset + c.len_utf8(),
        None => text.len(),
    }
}

/// The visual column of `offset` on its own line — a tab counts as the
/// distance to the next tab stop, matching [`column_block`]'s own rule so a
/// caret moved by Up/Down lines up with one dragged out as a column block.
fn visual_column_of(text: &str, starts: &[usize], offset: usize, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    let range = line_range(text, starts, line_of(starts, offset));
    let mut column = 0usize;
    for ch in text[range.start..offset.clamp(range.start, range.end)].chars() {
        column += if ch == '\t' {
            tab_width - (column % tab_width)
        } else {
            1
        };
    }
    column
}

/// `offset` moved `delta` lines up (negative) or down (positive), landing on
/// the target line at the same visual column — clipped to that line's own
/// length rather than padded, the same ragged-line rule [`column_block`]
/// uses.
fn vertical(text: &str, starts: &[usize], offset: usize, tab_width: usize, delta: isize) -> usize {
    let line = line_of(starts, offset) as isize;
    let last = starts.len().saturating_sub(1) as isize;
    let target = (line + delta).clamp(0, last) as usize;
    let column = visual_column_of(text, starts, offset, tab_width);
    let range = line_range(text, starts, target);
    range.start + byte_at_visual_column(&text[range.clone()], column, tab_width)
}

/// One word (or one run of non-word characters) to the left of `offset` —
/// the same target Ctrl+Backspace uses, and what Ctrl+Left lands on.
fn word_left(text: &str, offset: usize) -> usize {
    let mut at = clamp_to_boundary(text, offset);
    while let Some(c) = text[..at].chars().next_back().filter(|c| !is_word_byte(*c)) {
        at -= c.len_utf8();
    }
    while let Some(c) = text[..at].chars().next_back().filter(|c| is_word_byte(*c)) {
        at -= c.len_utf8();
    }
    at
}

/// The mirror of [`word_left`], one word (or non-word run) to the right.
fn word_right(text: &str, offset: usize) -> usize {
    let mut at = clamp_to_boundary(text, offset);
    while let Some(c) = text[at..].chars().next().filter(|c| !is_word_byte(*c)) {
        at += c.len_utf8();
    }
    while let Some(c) = text[at..].chars().next().filter(|c| is_word_byte(*c)) {
        at += c.len_utf8();
    }
    at
}

/// Alt+Shift+drag: one caret per line between two (line, visual column)
/// corners.
///
/// Columns are **visual**, so a tab counts as the distance to the next tab
/// stop rather than as one character. A column landing inside a tab snaps to
/// the far side of it — a caret never splits a tab.
///
/// **Ragged lines are clipped, not padded**: a line shorter than the
/// requested column contributes a caret at its end, and a line shorter than
/// both columns contributes an empty caret there. Padding would mean writing
/// spaces into lines the user only dragged across, and a selection must not
/// change text before an edit is even asked for. The clipped carets are kept
/// rather than dropped, so typing into a column block still types on every
/// line the user covered.
///
/// Refuses a block taller than [`MAX_CARETS`] lines.
pub fn column_block(
    text: &str,
    anchor_line: usize,
    anchor_col: usize,
    head_line: usize,
    head_col: usize,
    tab_width: usize,
) -> Result<SelectionSet, SelectionError> {
    let starts = line_starts(text);
    let last = starts.len().saturating_sub(1);
    let top = anchor_line.min(head_line).min(last);
    let bottom = anchor_line.max(head_line).min(last);
    check_ceiling(bottom - top + 1)?;

    let primary_line = head_line.min(last);
    let mut carets = Vec::with_capacity(bottom - top + 1);
    let mut primary = 0;
    for line in top..=bottom {
        let range = line_range(text, &starts, line);
        let line_text = &text[range.clone()];
        let anchor = range.start + byte_at_visual_column(line_text, anchor_col, tab_width);
        let head = range.start + byte_at_visual_column(line_text, head_col, tab_width);
        if line == primary_line {
            primary = carets.len();
        }
        carets.push(Caret::new(anchor, head));
    }
    SelectionSet::from_carets(carets, primary)
}

/// Byte offset within `line_text` of visual column `column`.
///
/// Tabs advance to the next multiple of `tab_width`; every other character
/// counts as one column. Wide (East Asian) characters are counted as one —
/// the same approximation the editor's own painter makes today.
// ponytail: single-width assumption; revisit with the painter if CJK columns
// ever have to line up exactly.
fn byte_at_visual_column(line_text: &str, column: usize, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    let mut visual = 0usize;
    for (byte, ch) in line_text.char_indices() {
        if visual >= column {
            return byte;
        }
        visual += if ch == '\t' {
            tab_width - (visual % tab_width)
        } else {
            1
        };
    }
    line_text.len()
}

/// Sorts and merges, returning the carets and the new index of the caret
/// that was at `primary`.
///
/// Merging is "overlapping **or touching**": carets at the same offset are
/// one caret, and a selection ending where the next begins is one selection.
/// Touching counts because the two would otherwise produce two edits meeting
/// at a point, which no user asked for and no undo entry can explain.
///
/// A merged caret keeps the direction of the earliest caret in the merge.
fn normalise(mut carets: Vec<Caret>, primary: usize) -> (Vec<Caret>, usize) {
    let primary_caret = carets[primary];
    // Stable, so the primary's relative position among equals survives.
    carets.sort_by_key(|c| (c.start(), c.end()));

    let mut merged: Vec<Caret> = Vec::with_capacity(carets.len());
    for caret in carets {
        match merged.last_mut() {
            Some(prev) if caret.start() <= prev.end() => {
                let start = prev.start();
                let end = prev.end().max(caret.end());
                *prev = prev.respan(start, end);
            }
            _ => merged.push(caret),
        }
    }

    let primary = merged
        .iter()
        .position(|c| c.start() <= primary_caret.start() && c.end() >= primary_caret.end())
        .unwrap_or(0);
    (merged, primary)
}

fn check_ceiling(count: usize) -> Result<(), SelectionError> {
    if count > MAX_CARETS {
        return Err(SelectionError::TooManyCarets {
            requested: count,
            max: MAX_CARETS,
        });
    }
    Ok(())
}

fn is_word_byte(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Whether `start..end` is bounded by non-word characters on both sides.
fn is_word_boundary_span(text: &str, start: usize, end: usize) -> bool {
    let before_ok = text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word_byte(c));
    let after_ok = text[end..].chars().next().is_none_or(|c| !is_word_byte(c));
    before_ok && after_ok
}

/// The word containing or immediately preceding `offset`, if any.
///
/// Preceding, because a caret sitting at the end of a word (where it lands
/// after typing it) means that word to the user, not the whitespace after it.
fn word_at(text: &str, offset: usize) -> Option<std::ops::Range<usize>> {
    let offset = clamp_to_boundary(text, offset);
    let here = text[offset..].chars().next().filter(|c| is_word_byte(*c));
    let before = text[..offset]
        .chars()
        .next_back()
        .filter(|c| is_word_byte(*c));
    if here.is_none() && before.is_none() {
        return None;
    }

    let mut start = offset;
    for (byte, ch) in text[..offset].char_indices().rev() {
        if !is_word_byte(ch) {
            break;
        }
        start = byte;
    }
    let mut end = offset;
    for (byte, ch) in text[offset..].char_indices() {
        if !is_word_byte(ch) {
            break;
        }
        end = offset + byte + ch.len_utf8();
    }
    if start == end {
        None
    } else {
        Some(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(offsets: &[usize]) -> SelectionSet {
        SelectionSet::from_carets(offsets.iter().copied().map(Caret::at).collect(), 0).unwrap()
    }

    fn spans(s: &SelectionSet) -> Vec<(usize, usize)> {
        s.carets().iter().map(|c| (c.start(), c.end())).collect()
    }

    #[test]
    fn a_collapsed_caret_is_not_a_selection() {
        let caret = Caret::at(4);
        assert!(caret.is_empty());
        assert_eq!((caret.start(), caret.end()), (4, 4));
    }

    #[test]
    fn a_reversed_caret_orders_its_ends() {
        let caret = Caret::new(9, 3);
        assert!(!caret.is_empty());
        assert!(caret.is_reversed());
        assert_eq!((caret.start(), caret.end()), (3, 9));
    }

    #[test]
    fn two_carets_at_the_same_offset_collapse() {
        let s = set(&[5, 5]);
        assert_eq!(s.len(), 1);
        assert_eq!(spans(&s), vec![(5, 5)]);
    }

    #[test]
    fn carets_are_sorted_ascending() {
        let s = set(&[9, 1, 5]);
        assert_eq!(spans(&s), vec![(1, 1), (5, 5), (9, 9)]);
    }

    #[test]
    fn overlapping_selections_merge() {
        let s = SelectionSet::from_carets(vec![Caret::new(2, 8), Caret::new(5, 12)], 0).unwrap();
        assert_eq!(spans(&s), vec![(2, 12)]);
    }

    #[test]
    fn touching_selections_merge() {
        let s = SelectionSet::from_carets(vec![Caret::new(0, 4), Caret::new(4, 7)], 0).unwrap();
        assert_eq!(spans(&s), vec![(0, 7)]);
    }

    #[test]
    fn a_merged_caret_keeps_the_earliest_direction() {
        let s = SelectionSet::from_carets(vec![Caret::new(8, 2), Caret::new(5, 12)], 0).unwrap();
        assert_eq!(s.carets(), &[Caret::new(12, 2)]);
    }

    #[test]
    fn adjacent_empty_carets_do_not_merge() {
        // The backspace case: carets one byte apart are two carets, or the
        // classic double-delete appears.
        let s = set(&[5, 6]);
        assert_eq!(spans(&s), vec![(5, 5), (6, 6)]);
    }

    #[test]
    fn adding_a_caret_makes_it_primary() {
        let mut s = set(&[1]);
        s.add_caret(Caret::at(7)).unwrap();
        assert_eq!(s.primary(), Caret::at(7));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn adding_a_caret_inside_a_selection_merges_and_focuses_it() {
        let mut s = SelectionSet::single(Caret::new(2, 10));
        s.add_caret(Caret::at(5)).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s.primary(), Caret::new(2, 10));
    }

    #[test]
    fn collapse_to_primary_keeps_only_the_primary_and_its_selection() {
        let mut s = set(&[1, 5]);
        s.add_caret(Caret::new(20, 24)).unwrap();
        s.collapse_to_primary();
        assert_eq!(s.carets(), &[Caret::new(20, 24)]);
        assert_eq!(s.primary_index(), 0);
    }

    #[test]
    fn the_ceiling_refuses_rather_than_truncating() {
        let carets: Vec<Caret> = (0..=MAX_CARETS).map(|i| Caret::at(i * 2)).collect();
        let err = SelectionSet::from_carets(carets, 0).unwrap_err();
        assert_eq!(
            err,
            SelectionError::TooManyCarets {
                requested: MAX_CARETS + 1,
                max: MAX_CARETS,
            }
        );
    }

    #[test]
    fn a_set_exactly_at_the_ceiling_is_accepted() {
        let carets: Vec<Caret> = (0..MAX_CARETS).map(|i| Caret::at(i * 2)).collect();
        assert_eq!(
            SelectionSet::from_carets(carets, 0).unwrap().len(),
            MAX_CARETS
        );
    }

    #[test]
    fn a_refused_add_leaves_the_set_untouched() {
        let carets: Vec<Caret> = (0..MAX_CARETS).map(|i| Caret::at(i * 2)).collect();
        let mut s = SelectionSet::from_carets(carets, 0).unwrap();
        let before = s.clone();
        assert!(s.add_caret(Caret::at(MAX_CARETS * 2 + 4)).is_err());
        assert_eq!(s, before);
    }

    #[test]
    fn carets_merging_below_the_ceiling_are_accepted() {
        // Every caret at the same offset: over the ceiling as a list, one
        // caret once normalised.
        let carets: Vec<Caret> = (0..MAX_CARETS + 50).map(|_| Caret::at(3)).collect();
        assert_eq!(SelectionSet::from_carets(carets, 0).unwrap().len(), 1);
    }

    // --- next occurrence -------------------------------------------------

    #[test]
    fn next_occurrence_first_selects_the_word_under_the_caret() {
        let text = "let value = value + 1;";
        let mut s = SelectionSet::single(Caret::at(6));
        assert!(s.add_next_occurrence(text).unwrap());
        assert_eq!(s.len(), 1);
        assert_eq!(&text[s.primary().range()], "value");
    }

    #[test]
    fn next_occurrence_takes_the_word_ending_at_the_caret() {
        let text = "value = 1";
        let mut s = SelectionSet::single(Caret::at(5));
        assert!(s.add_next_occurrence(text).unwrap());
        assert_eq!(&text[s.primary().range()], "value");
    }

    #[test]
    fn next_occurrence_on_whitespace_does_nothing() {
        let text = "a   b";
        let mut s = SelectionSet::single(Caret::at(2));
        assert!(!s.add_next_occurrence(text).unwrap());
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn next_occurrence_adds_the_following_match_and_makes_it_primary() {
        let text = "value = value + value";
        let mut s = SelectionSet::single(Caret::new(0, 5));
        assert!(s.add_next_occurrence(text).unwrap());
        assert_eq!(spans(&s), vec![(0, 5), (8, 13)]);
        assert_eq!(s.primary(), Caret::new(8, 13));
        assert!(s.add_next_occurrence(text).unwrap());
        assert_eq!(spans(&s), vec![(0, 5), (8, 13), (16, 21)]);
    }

    #[test]
    fn next_occurrence_wraps_at_the_end_and_skips_the_caret_it_started_from() {
        let text = "value = value";
        let mut s = SelectionSet::single(Caret::new(8, 13));
        // Nothing after offset 13, so it wraps to the occurrence at 0.
        assert!(s.add_next_occurrence(text).unwrap());
        assert_eq!(spans(&s), vec![(0, 5), (8, 13)]);
        // And now there is nothing left that is not already selected.
        assert!(!s.add_next_occurrence(text).unwrap());
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn next_occurrence_is_whole_word_when_the_selection_is_a_whole_word() {
        let text = "value values value";
        let mut s = SelectionSet::single(Caret::new(0, 5));
        assert!(s.add_next_occurrence(text).unwrap());
        // "values" at 6 is skipped; the match at 13 is taken.
        assert_eq!(spans(&s), vec![(0, 5), (13, 18)]);
    }

    #[test]
    fn next_occurrence_is_a_substring_search_for_a_partial_selection() {
        let text = "value values";
        // "alu" inside "value" — not a whole word in situ.
        let mut s = SelectionSet::single(Caret::new(1, 4));
        assert!(s.add_next_occurrence(text).unwrap());
        assert_eq!(spans(&s), vec![(1, 4), (7, 10)]);
    }

    #[test]
    fn next_occurrence_handles_multi_byte_text() {
        let text = "héllo = héllo";
        let mut s = SelectionSet::single(Caret::new(0, 6));
        assert!(s.add_next_occurrence(text).unwrap());
        assert_eq!(spans(&s), vec![(0, 6), (9, 15)]);
        assert_eq!(&text[s.primary().range()], "héllo");
    }

    // --- column selection ------------------------------------------------

    #[test]
    fn column_block_makes_one_caret_per_line() {
        let text = "abcdef\nabcdef\nabcdef\n";
        let s = column_block(text, 0, 1, 2, 4, 4).unwrap();
        assert_eq!(spans(&s), vec![(1, 4), (8, 11), (15, 18)]);
        assert_eq!(s.primary(), Caret::new(15, 18));
    }

    #[test]
    fn column_block_clips_ragged_lines_instead_of_padding() {
        let text = "abcdefgh\nab\n\nabcdefgh";
        let s = column_block(text, 0, 3, 3, 6, 4).unwrap();
        assert_eq!(
            spans(&s),
            vec![
                (3, 6),   // full line
                (11, 11), // "ab" is shorter than both columns: empty caret at its end
                (12, 12), // empty line
                (16, 19), // full line
            ]
        );
        // The clipped carets are kept, not dropped.
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn column_block_counts_a_tab_as_its_visual_width() {
        // "\tx" — the tab occupies columns 0..4, so column 4 is "x".
        let text = "\txy\nabcdefg";
        let s = column_block(text, 0, 4, 1, 6, 4).unwrap();
        assert_eq!(spans(&s), vec![(1, 3), (8, 10)]);
        assert_eq!(&text[1..3], "xy");
    }

    #[test]
    fn a_column_inside_a_tab_snaps_past_it() {
        let text = "\tx";
        // Column 2 falls inside the tab (which spans 0..4); it must not split it.
        let s = column_block(text, 0, 0, 0, 2, 4).unwrap();
        assert_eq!(spans(&s), vec![(0, 1)]);
    }

    #[test]
    fn column_block_lands_on_char_boundaries_across_a_four_byte_character() {
        // "🙂" is one visual column here and four bytes.
        let text = "a🙂b\nabcd";
        let s = column_block(text, 0, 1, 1, 3, 4).unwrap();
        assert_eq!(spans(&s), vec![(1, 6), (8, 10)]);
        assert_eq!(&text[1..6], "🙂b");
    }

    #[test]
    fn column_block_clamps_lines_past_the_end() {
        let text = "ab\ncd";
        let s = column_block(text, 0, 0, 99, 2, 4).unwrap();
        assert_eq!(spans(&s), vec![(0, 2), (3, 5)]);
    }

    // --- caret motion ------------------------------------------------------

    #[test]
    fn left_and_right_move_every_caret_by_one_character() {
        let text = "abc\ndef";
        let s = set(&[1, 5]);
        let moved = s.move_carets(text, CaretMotion::Right, false, 4).unwrap();
        assert_eq!(spans(&moved), vec![(2, 2), (6, 6)]);
        let back = moved
            .move_carets(text, CaretMotion::Left, false, 4)
            .unwrap();
        assert_eq!(spans(&back), vec![(1, 1), (5, 5)]);
    }

    #[test]
    fn left_on_a_selection_collapses_to_its_start_without_moving_further() {
        let s = SelectionSet::single(Caret::new(2, 8));
        let moved = s
            .move_carets("0123456789", CaretMotion::Left, false, 4)
            .unwrap();
        assert_eq!(spans(&moved), vec![(2, 2)]);
    }

    #[test]
    fn shift_extends_instead_of_collapsing() {
        let s = set(&[1]);
        let moved = s
            .move_carets("abcdef", CaretMotion::Right, true, 4)
            .unwrap();
        assert_eq!(moved.carets(), &[Caret::new(1, 2)]);
    }

    #[test]
    fn home_and_end_land_on_the_lines_own_bounds() {
        let text = "  abc\ndef";
        let s = set(&[3]);
        let home = s.move_carets(text, CaretMotion::Home, false, 4).unwrap();
        assert_eq!(spans(&home), vec![(0, 0)]);
        let end = s.move_carets(text, CaretMotion::End, false, 4).unwrap();
        assert_eq!(spans(&end), vec![(5, 5)]);
    }

    #[test]
    fn down_keeps_every_carets_own_visual_column_and_clamps_at_the_end() {
        let text = "abcdef\nab\nabcdef";
        let s = set(&[3, 13]); // column 3 on line 0, column 3 (of 6) on line 2
        let down = s.move_carets(text, CaretMotion::Down, false, 4).unwrap();
        // Line 0 -> line 1 clips to "ab"'s length; line 2 is already last, so
        // it clamps in place rather than falling off the end of the text.
        assert_eq!(spans(&down), vec![(9, 9), (13, 13)]);
    }

    #[test]
    fn word_motions_move_every_caret_to_the_next_word_boundary() {
        let text = "one two three";
        let s = set(&[0, 4]);
        let moved = s
            .move_carets(text, CaretMotion::WordRight, false, 4)
            .unwrap();
        assert_eq!(spans(&moved), vec![(3, 3), (7, 7)]);
        let back = moved
            .move_carets(text, CaretMotion::WordLeft, false, 4)
            .unwrap();
        assert_eq!(spans(&back), vec![(0, 0), (4, 4)]);
    }

    #[test]
    fn a_column_block_taller_than_the_ceiling_is_refused() {
        let text = "x\n".repeat(MAX_CARETS + 1);
        let err = column_block(&text, 0, 0, MAX_CARETS, 1, 4).unwrap_err();
        assert!(matches!(err, SelectionError::TooManyCarets { .. }));
    }
}
