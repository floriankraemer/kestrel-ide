//! Auto-close, type-over, smart backspace and surround.
//!
//! # Type-over
//!
//! Typing a closing bracket (`)`, `]`, `}`) when an identical one sits
//! under the caret always skips over it rather than inserting a second
//! one — IntelliJ's own rule, and it applies whether or not this session
//! put that closer there: a `)` the user typed, pasted, or opened the file
//! with is still a `)` to step past, not to duplicate.
//!
//! Quotes are narrower: the same character opens and closes a string, so
//! an untracked quote could just as easily be content to wrap as a closer
//! to skip. [`PairTracker`] remembers only the quote (and bracket) closers
//! it itself inserted, moves them as later edits shift them, and forgets
//! all of them the moment an edit it did not produce lands
//! ([`PairTracker::invalidate`]) — that tracked state is what still makes
//! quote type-over work, on the caller's side of the seam, explicit rather
//! than guessed at from the buffer.
//!
//! # Paste is not typing
//!
//! [`PairTracker::paste`] inserts exactly what it was given. Pasting
//! `foo(bar` must not produce `foo(bar)`, which is what happens whenever
//! paste is routed through the per-character path.
//!
//! # Where auto-close is suppressed
//!
//! - inside a string, character literal or comment, per the grammar;
//! - for a quote whose next character is a word character (`foo|bar`);
//! - for a quote whose previous character is a word character, `&` or `<`
//!   — which is what keeps a Rust lifetime `&'a str` from becoming
//!   `&'' a str`, without a per-language table: after `&` or `<` an
//!   apostrophe is not opening a literal in any language that has both.

use editor_core::selection::{Caret, SelectionSet};
use editor_core::transaction::{TextEdit, Transaction};
use syntax_core::Language;

use crate::syntax::{Syntax, Tokens};

/// One edit and where the carets end up once it is applied.
///
/// The selection is returned rather than derived with
/// [`editor_core::transaction::map_carets`] because pairing is exactly the
/// case that mapping gets wrong: after inserting `()` the caret belongs
/// *between* the two characters, and after typing over a closer nothing is
/// inserted at all yet the caret still moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeEdit {
    pub transaction: Transaction,
    pub selection: SelectionSet,
}

/// A closing delimiter this session inserted, in current-buffer
/// coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Closer {
    offset: usize,
    text: String,
}

/// Remembers the closers auto-close inserted, so type-over can apply to
/// those and only those. One per editor tab.
#[derive(Debug, Default)]
pub struct PairTracker {
    closers: Vec<Closer>,
}

impl PairTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every tracked closer. The caller calls this for any change it
    /// did not route through this type: an external edit, an undo, a
    /// refactoring, a file reload. A tracked offset that survived one of
    /// those would type over a character somebody else's edit put there.
    pub fn invalidate(&mut self) {
        self.closers.clear();
    }

    /// How many closers are being tracked. For tests and diagnostics.
    pub fn tracked(&self) -> usize {
        self.closers.len()
    }

    /// Type one character at every caret.
    ///
    /// A caret with a selection surrounds it when the character opens a
    /// pair, and replaces it otherwise. A collapsed caret types over a
    /// tracked closer, auto-closes a pair, or inserts plainly.
    pub fn type_char(
        &mut self,
        language: Language,
        text: &str,
        selection: &SelectionSet,
        ch: char,
    ) -> TypeEdit {
        let tokens = Tokens::of(language);
        let typed = ch.to_string();
        let closing = tokens.closing_for(&typed);
        // A non-quote closing delimiter (`)`, `]`, `}`) types over an
        // identical character already under the caret whether or not this
        // session put it there — IntelliJ's own rule, and simpler than the
        // tracked-only one this module used to apply: a closer the user
        // never inserted (typed, pasted, or loaded from disk) still reads
        // as "step past it", the same way it reads to a human. Quotes stay
        // tracked-only (`types_over` below): the same character opens and
        // closes a string, so an untracked quote could just as easily be
        // content to wrap rather than a closer to skip.
        let is_bracket_closer = !tokens.is_quote(&typed) && tokens.open_for(&typed).is_some();
        let types_over = self.closers.iter().any(|c| c.text == typed);
        if closing.is_none() && !types_over && !is_bracket_closer {
            // Nothing about this character is a pair: plain typing, and the
            // tracked closers only need moving.
            let transaction = Transaction::type_text(selection, &typed);
            let plan: Vec<Plan> = selection
                .carets()
                .iter()
                .map(|caret| Plan::head(caret.start() + typed.len(), typed.len() as isize))
                .collect();
            return self.finish(selection, transaction, plan);
        }

        // Only now is a parse worth its cost: it is needed to know whether
        // the caret sits in a string or a comment.
        let syntax = Syntax::parse(language, text);
        let mut edits = Vec::new();
        let mut plan = Vec::new();
        let mut fresh: Vec<(usize, String)> = Vec::new();

        for caret in selection.carets() {
            let (start, end) = (caret.start(), caret.end());
            if !caret.is_empty() {
                match &closing {
                    // Surround wraps, it never replaces: the selection is
                    // kept, shifted past the opener.
                    Some(close) => {
                        edits.push(TextEdit::insert(start, typed.clone()));
                        edits.push(TextEdit::insert(end, close.clone()));
                        plan.push(Plan::span(
                            start + typed.len(),
                            end + typed.len(),
                            caret.is_reversed(),
                            (typed.len() + close.len()) as isize,
                        ));
                    }
                    None => {
                        edits.push(TextEdit::new(caret.range(), typed.clone()));
                        plan.push(Plan::head(
                            start + typed.len(),
                            typed.len() as isize - (end - start) as isize,
                        ));
                    }
                }
                continue;
            }

            let tracked_index = self
                .closers
                .iter()
                .position(|c| c.offset == start && c.text == typed);
            let adjacent_matches = is_bracket_closer
                && text[start..].chars().next().map(|c| c.to_string()).as_ref() == Some(&typed);
            if tracked_index.is_some() || adjacent_matches {
                // Type-over: either we put this closer here, or (brackets
                // only) an identical one already sits under the caret.
                // Nothing is inserted; the caret steps past it.
                if let Some(index) = tracked_index {
                    self.closers.remove(index);
                }
                plan.push(Plan::head(start + typed.len(), 0));
                continue;
            }

            match &closing {
                Some(close) if auto_closes(&tokens, &syntax, text, start, &typed) => {
                    edits.push(TextEdit::insert(start, format!("{typed}{close}")));
                    let at = start + typed.len();
                    fresh.push((plan.len(), close.clone()));
                    plan.push(Plan::head(at, (typed.len() + close.len()) as isize));
                }
                _ => {
                    edits.push(TextEdit::insert(start, typed.clone()));
                    plan.push(Plan::head(start + typed.len(), typed.len() as isize));
                }
            }
        }

        let (result, resolved) = self.finish_with(selection, Transaction::new(edits), plan);
        for (index, close) in fresh {
            // The closer sits exactly where that caret ended up, which the
            // plan already worked out against the running shift.
            self.closers.push(Closer {
                offset: resolved[index].head,
                text: close,
            });
        }
        self.closers.sort_by_key(|c| c.offset);
        result
    }

    /// Backspace at every caret, deleting a pair as a unit.
    ///
    /// A caret sitting between an opener and its matching closer with
    /// nothing between them deletes both; with content between them it
    /// deletes one character, exactly like an ordinary backspace.
    pub fn backspace(
        &mut self,
        language: Language,
        text: &str,
        selection: &SelectionSet,
    ) -> TypeEdit {
        let tokens = Tokens::of(language);
        let mut edits = Vec::new();
        let mut plan = Vec::new();
        for caret in selection.carets() {
            if !caret.is_empty() {
                edits.push(TextEdit::delete(caret.range()));
                plan.push(Plan::head(
                    caret.start(),
                    -((caret.end() - caret.start()) as isize),
                ));
                continue;
            }
            let head = caret.head;
            let Some(previous) = text[..head].chars().next_back() else {
                plan.push(Plan::head(head, 0));
                continue;
            };
            let before = previous.to_string();
            let after = text[head..].chars().next().map(|c| c.to_string());
            let pairs_up = match (tokens.closing_for(&before), &after) {
                (Some(close), Some(next)) => &close == next,
                _ => false,
            };
            let start = head - before.len();
            if pairs_up {
                let end = head + after.map_or(0, |next| next.len());
                edits.push(TextEdit::delete(start..end));
                plan.push(Plan::head(start, -((end - start) as isize)));
            } else {
                edits.push(TextEdit::delete(start..head));
                plan.push(Plan::head(start, -(before.len() as isize)));
            }
        }
        // A deletion can remove or straddle a tracked closer; rather than
        // reason about which, drop them all. The cost is one lost
        // type-over, which is invisible; the alternative is a stale offset.
        self.invalidate();
        self.finish(selection, Transaction::new(edits), plan)
    }

    /// Insert `pasted` at every caret verbatim — no pairing, no
    /// auto-close, nothing added.
    pub fn paste(&mut self, selection: &SelectionSet, pasted: &str) -> TypeEdit {
        self.invalidate();
        let transaction = Transaction::type_text(selection, pasted);
        let plan: Vec<Plan> = selection
            .carets()
            .iter()
            .map(|caret| {
                Plan::head(
                    caret.start() + pasted.len(),
                    pasted.len() as isize - (caret.end() - caret.start()) as isize,
                )
            })
            .collect();
        self.finish(selection, transaction, plan)
    }

    /// Apply the running shift to the planned caret positions and to the
    /// tracked closers, and hand back the result.
    fn finish(
        &mut self,
        selection: &SelectionSet,
        transaction: Transaction,
        plan: Vec<Plan>,
    ) -> TypeEdit {
        self.finish_with(selection, transaction, plan).0
    }

    fn finish_with(
        &mut self,
        selection: &SelectionSet,
        transaction: Transaction,
        plan: Vec<Plan>,
    ) -> (TypeEdit, Vec<Caret>) {
        let mut delta = 0isize;
        let mut carets = Vec::with_capacity(plan.len());
        for step in plan {
            carets.push(step.resolve(delta));
            delta += step.delta;
        }
        for closer in &mut self.closers {
            closer.offset = shift(closer.offset, &transaction);
        }
        let resolved = carets.clone();
        let selection = SelectionSet::from_carets(carets, selection.primary_index())
            .unwrap_or_else(|_| selection.clone());
        (
            TypeEdit {
                transaction,
                selection,
            },
            resolved,
        )
    }
}

/// Where one caret lands, expressed against the pre-edit text plus the
/// running length change of the carets before it.
#[derive(Debug, Clone, Copy)]
struct Plan {
    anchor: usize,
    head: usize,
    delta: isize,
}

impl Plan {
    fn head(at: usize, delta: isize) -> Self {
        Self {
            anchor: at,
            head: at,
            delta,
        }
    }

    fn span(start: usize, end: usize, reversed: bool, delta: isize) -> Self {
        let (anchor, head) = if reversed { (end, start) } else { (start, end) };
        Self {
            anchor,
            head,
            delta,
        }
    }

    fn resolve(&self, delta: isize) -> Caret {
        Caret::new(
            (self.anchor as isize + delta).max(0) as usize,
            (self.head as isize + delta).max(0) as usize,
        )
    }
}

/// Where `offset` ends up once `transaction` is applied. Only edits that
/// end at or before it can move it.
fn shift(offset: usize, transaction: &Transaction) -> usize {
    let mut out = offset as isize;
    for edit in &transaction.edits {
        if edit.range.end <= offset {
            out += edit.text.len() as isize - (edit.range.end - edit.range.start) as isize;
        }
    }
    out.max(0) as usize
}

fn auto_closes(tokens: &Tokens, syntax: &Syntax, text: &str, offset: usize, typed: &str) -> bool {
    if syntax.in_literal_or_comment(offset) {
        return false;
    }
    if !tokens.is_quote(typed) {
        return true;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if text[offset..].chars().next().is_some_and(is_word) {
        return false;
    }
    match text[..offset].chars().next_back() {
        // `&'a`, `<'a`: an apostrophe here opens a lifetime, not a literal.
        Some('&') | Some('<') => false,
        Some(previous) => !is_word(previous) && previous.to_string() != *typed,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntax_core::language_by_id;

    fn lang(id: &str) -> Language {
        language_by_id(id).unwrap_or_else(|| panic!("{id} is in the catalog"))
    }

    fn at(offset: usize) -> SelectionSet {
        SelectionSet::single(Caret::at(offset))
    }

    fn span(anchor: usize, head: usize) -> SelectionSet {
        SelectionSet::single(Caret::new(anchor, head))
    }

    /// Type `ch` and return the resulting text plus the primary caret.
    fn typed(
        tracker: &mut PairTracker,
        id: &str,
        text: &str,
        selection: &SelectionSet,
        ch: char,
    ) -> (String, usize) {
        let result = tracker.type_char(lang(id), text, selection, ch);
        let out = result.transaction.apply(text).expect("applies");
        (out, result.selection.primary().head)
    }

    #[test]
    fn typing_an_opening_bracket_inserts_the_pair_with_the_caret_inside() {
        let mut tracker = PairTracker::new();
        let (text, head) = typed(&mut tracker, "rust", "fn f() {}\n", &at(9), '(');
        assert_eq!(text, "fn f() {}()\n");
        assert_eq!(head, 10);
        assert_eq!(tracker.tracked(), 1);
    }

    #[test]
    fn typing_the_closer_of_a_pair_we_inserted_types_over_it() {
        let mut tracker = PairTracker::new();
        let start = "let x = ";
        let opened = tracker.type_char(lang("rust"), start, &at(8), '(');
        let text = opened.transaction.apply(start).expect("applies");
        assert_eq!(text, "let x = ()");

        let closed = tracker.type_char(lang("rust"), &text, &opened.selection, ')');
        assert!(closed.transaction.is_empty(), "nothing should be inserted");
        assert_eq!(closed.selection.primary().head, 10);
        assert_eq!(tracker.tracked(), 0);
    }

    #[test]
    fn typing_a_closer_before_a_bracket_we_did_not_insert_still_types_over_it() {
        // IntelliJ's own rule: an identical closer under the caret is
        // always stepped past, tracked or not — the fix for FX's console
        // finding (typing `)` after an auto-inserted `)` used to double it).
        let mut tracker = PairTracker::new();
        let (text, head) = typed(&mut tracker, "rust", "foo()\n", &at(4), ')');
        assert_eq!(text, "foo()\n");
        assert_eq!(head, 5);
    }

    #[test]
    fn a_closer_typed_at_a_caret_with_no_adjacent_match_still_inserts() {
        let mut tracker = PairTracker::new();
        let (text, head) = typed(&mut tracker, "rust", "foo\n", &at(3), ')');
        assert_eq!(text, "foo)\n");
        assert_eq!(head, 4);
    }

    #[test]
    fn an_intervening_edit_forgets_the_tracked_closer_but_a_bracket_still_types_over() {
        let mut tracker = PairTracker::new();
        let opened = tracker.type_char(lang("rust"), "f = ", &at(4), '(');
        let text = opened.transaction.apply("f = ").expect("applies");
        assert_eq!(tracker.tracked(), 1);

        // Something else changed the buffer — a refactoring, an undo, the
        // watcher. The tracked closer is forgotten either way…
        tracker.invalidate();
        let closed = tracker.type_char(lang("rust"), &text, &opened.selection, ')');
        // …but a bracket closer still types over an identical one under the
        // caret regardless of tracking (IntelliJ's rule, see the module doc
        // comment) — the untracked `)` is not re-inserted.
        assert_eq!(closed.transaction.apply(&text).expect("applies"), "f = ()");
    }

    #[test]
    fn an_intervening_edit_forgets_the_tracked_quote_and_a_quote_does_not_type_over() {
        // Quotes are the narrower case the module doc comment calls out:
        // the same character opens and closes a string, so once tracking
        // is lost a quote is content to wrap, not a closer to skip.
        let mut tracker = PairTracker::new();
        let opened = tracker.type_char(lang("rust"), "s = ", &at(4), '"');
        let text = opened.transaction.apply("s = ").expect("applies");
        assert_eq!(tracker.tracked(), 1);

        tracker.invalidate();
        let closed = tracker.type_char(lang("rust"), &text, &opened.selection, '"');
        assert_eq!(
            closed.transaction.apply(&text).expect("applies"),
            "s = \"\"\""
        );
    }

    #[test]
    fn typing_inside_a_pair_keeps_the_closer_trackable() {
        let mut tracker = PairTracker::new();
        let mut text = "f".to_string();
        let mut selection = at(1);
        for ch in "(abc".chars() {
            let result = tracker.type_char(lang("rust"), &text, &selection, ch);
            text = result.transaction.apply(&text).expect("applies");
            selection = result.selection;
        }
        assert_eq!(text, "f(abc)");

        let closed = tracker.type_char(lang("rust"), &text, &selection, ')');
        assert!(closed.transaction.is_empty());
        assert_eq!(closed.selection.primary().head, 6);
    }

    #[test]
    fn pasting_a_string_full_of_brackets_inserts_nothing_extra() {
        let mut tracker = PairTracker::new();
        let pasted = "foo(bar[baz{\"q\"";
        let result = tracker.paste(&at(0), pasted);
        assert_eq!(result.transaction.apply("").expect("applies"), pasted);
        assert_eq!(result.selection.primary().head, pasted.len());
        assert_eq!(tracker.tracked(), 0);
    }

    #[test]
    fn auto_close_is_suppressed_inside_a_string_and_a_comment() {
        let mut tracker = PairTracker::new();
        let text = "let s = \"ab\";\n";
        let inside = text.find("ab").expect("fixture") + 1;
        let (out, _) = typed(&mut tracker, "rust", text, &at(inside), '(');
        assert_eq!(out, "let s = \"a(b\";\n");

        let commented = "// ab\n";
        let (out, _) = typed(&mut tracker, "rust", commented, &at(4), '(');
        assert_eq!(out, "// a(b\n");
    }

    #[test]
    fn a_quote_before_a_word_character_does_not_auto_close() {
        let mut tracker = PairTracker::new();
        let (out, _) = typed(&mut tracker, "rust", "foobar\n", &at(3), '"');
        assert_eq!(out, "foo\"bar\n");
    }

    #[test]
    fn a_rust_lifetime_does_not_auto_close_its_apostrophe() {
        let mut tracker = PairTracker::new();
        let (out, head) = typed(&mut tracker, "rust", "fn f(x: &str) {}\n", &at(9), '\'');
        assert_eq!(out, "fn f(x: &'str) {}\n");
        assert_eq!(head, 10);
        assert_eq!(tracker.tracked(), 0, "nothing to type over later");

        // A generic parameter list is the other lifetime position.
        let (out, _) = typed(&mut tracker, "rust", "struct S<>;\n", &at(9), '\'');
        assert_eq!(out, "struct S<'>;\n");
    }

    #[test]
    fn a_c_character_literal_still_auto_closes() {
        // The same apostrophe, a different position: in C after `= ` it
        // opens a character literal and the pair is what the user wants.
        let mut tracker = PairTracker::new();
        let (out, head) = typed(&mut tracker, "c", "char c = ;\n", &at(9), '\'');
        assert_eq!(out, "char c = '';\n");
        assert_eq!(head, 10);
    }

    #[test]
    fn smart_backspace_between_a_pair_deletes_both() {
        let mut tracker = PairTracker::new();
        let text = "foo()\n";
        let result = tracker.backspace(lang("rust"), text, &at(4));
        assert_eq!(result.transaction.apply(text).expect("applies"), "foo\n");
        assert_eq!(result.selection.primary().head, 3);
    }

    #[test]
    fn smart_backspace_with_content_between_deletes_one_character() {
        let mut tracker = PairTracker::new();
        let text = "foo(a)\n";
        let result = tracker.backspace(lang("rust"), text, &at(5));
        assert_eq!(result.transaction.apply(text).expect("applies"), "foo()\n");
        assert_eq!(result.selection.primary().head, 4);
    }

    #[test]
    fn surround_wraps_the_selection_rather_than_replacing_it() {
        let mut tracker = PairTracker::new();
        let text = "value\n";
        let result = tracker.type_char(lang("rust"), text, &span(0, 5), '"');
        assert_eq!(
            result.transaction.apply(text).expect("applies"),
            "\"value\"\n"
        );
        let caret = result.selection.primary();
        assert_eq!((caret.start(), caret.end()), (1, 6));
    }

    #[test]
    fn surround_with_three_carets_wraps_all_three_in_one_transaction() {
        let mut tracker = PairTracker::new();
        let text = "aaa bbb ccc\n";
        let selection = SelectionSet::from_carets(
            vec![Caret::new(0, 3), Caret::new(4, 7), Caret::new(8, 11)],
            0,
        )
        .expect("three carets");
        let result = tracker.type_char(lang("rust"), text, &selection, '(');
        assert_eq!(
            result.transaction.apply(text).expect("applies"),
            "(aaa) (bbb) (ccc)\n"
        );
        assert_eq!(result.selection.len(), 3);
        let spans: Vec<(usize, usize)> = result
            .selection
            .carets()
            .iter()
            .map(|c| (c.start(), c.end()))
            .collect();
        assert_eq!(spans, vec![(1, 4), (7, 10), (13, 16)]);
    }

    #[test]
    fn three_carets_auto_close_in_one_transaction() {
        let mut tracker = PairTracker::new();
        let text = "a\nb\nc\n";
        let selection =
            SelectionSet::from_carets(vec![Caret::at(1), Caret::at(3), Caret::at(5)], 0)
                .expect("three carets");
        let result = tracker.type_char(lang("rust"), text, &selection, '(');
        assert_eq!(
            result.transaction.apply(text).expect("applies"),
            "a()\nb()\nc()\n"
        );
        assert_eq!(
            result
                .selection
                .carets()
                .iter()
                .map(|c| c.head)
                .collect::<Vec<_>>(),
            vec![2, 6, 10]
        );
        assert_eq!(tracker.tracked(), 3);
    }

    #[test]
    fn typing_an_ordinary_character_is_a_plain_insertion() {
        let mut tracker = PairTracker::new();
        let (out, head) = typed(&mut tracker, "rust", "ab\n", &at(1), 'x');
        assert_eq!(out, "axb\n");
        assert_eq!(head, 2);
    }

    #[test]
    fn a_language_with_no_pairs_at_all_just_types() {
        let mut tracker = PairTracker::new();
        let (out, _) = typed(&mut tracker, "rust", "", &at(0), '(');
        assert_eq!(out, "()");

        let plain = PairTracker::new().type_char(Language::PLAIN_TEXT, "", &at(0), '(');
        assert_eq!(plain.transaction.apply("").expect("applies"), "(");
    }
}
