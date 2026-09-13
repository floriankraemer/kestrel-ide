//! Which tab stop a just-accepted snippet is sitting on (R2), and what
//! Tab/Shift+Tab do to it.
//!
//! Deliberately opaque about what a "range" is: `edit_ops::snippet` produces
//! char offsets into the inserted text, and the one caller that owns both
//! the insertion point and the live document (`ui-shell`'s `EditorOps`,
//! ADR-0023 — a snippet session is exactly the kind of per-tab, gesture-
//! driven state that lives there, not on `Document`) turns those into
//! absolute positions before handing them here. This type only walks the
//! list it is given.
//!
//! **Ceiling**: a stop's range is fixed at the position it had when the
//! session began. Typing at an earlier stop does not re-anchor a later
//! one — real linked-editing (IntelliJ/VS Code re-measure every mirrored
//! range after each keystroke) is future work; this is enough for the
//! common case of accepting a snippet and tabbing through its stops
//! untouched, or replacing exactly one stop's placeholder before moving on.

use std::ops::Range;

/// One accepted snippet's tab stops and which one is current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetSession {
    stops: Vec<Range<usize>>,
    current: usize,
}

impl SnippetSession {
    /// `None` for a snippet with no tab stops at all (nothing to walk, so
    /// there is nothing to track).
    pub fn new(stops: Vec<Range<usize>>) -> Option<Self> {
        if stops.is_empty() {
            None
        } else {
            Some(Self { stops, current: 0 })
        }
    }

    /// The stop the caret is on right now.
    pub fn current(&self) -> Range<usize> {
        self.stops[self.current].clone()
    }

    /// Whether Tab still has somewhere to go — `false` on the last stop
    /// (`$0`, or the highest-numbered one when the snippet has no `$0`),
    /// which is where the session ends per its own doc comment.
    pub fn is_last(&self) -> bool {
        self.current + 1 == self.stops.len()
    }

    /// Tab: move to the next stop, or `None` when already on the last —
    /// nowhere further to go, so the keystroke falls through to whatever
    /// Tab ordinarily does.
    pub fn advance(&mut self) -> Option<Range<usize>> {
        if self.is_last() {
            return None;
        }
        self.current += 1;
        Some(self.current())
    }

    /// Shift+Tab: back to the previous stop, or `None` already on the
    /// first.
    pub fn retreat(&mut self) -> Option<Range<usize>> {
        if self.current == 0 {
            return None;
        }
        self.current -= 1;
        Some(self.current())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_stops_is_no_session() {
        assert!(SnippetSession::new(Vec::new()).is_none());
    }

    #[test]
    fn a_single_stop_starts_and_ends_on_it() {
        let session = SnippetSession::new(std::iter::once(3..3).collect()).unwrap();
        assert_eq!(session.current(), 3..3);
        assert!(session.is_last());
    }

    #[test]
    fn tab_walks_forward_and_stops_at_the_end() {
        let mut session = SnippetSession::new(vec![0..4, 6..6, 9..9]).unwrap();
        assert_eq!(session.current(), 0..4);
        assert!(!session.is_last());
        assert_eq!(session.advance(), Some(6..6));
        assert!(!session.is_last());
        assert_eq!(session.advance(), Some(9..9));
        assert!(session.is_last(), "the last stop ends the session");
        assert_eq!(session.advance(), None, "nowhere further to go");
    }

    #[test]
    fn shift_tab_walks_backward_and_stops_at_the_start() {
        let mut session = SnippetSession::new(vec![0..4, 6..6]).unwrap();
        session.advance();
        assert_eq!(session.retreat(), Some(0..4));
        assert_eq!(session.retreat(), None, "already on the first stop");
    }
}
