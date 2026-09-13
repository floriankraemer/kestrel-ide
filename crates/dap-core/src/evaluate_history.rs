//! Evaluate expression history (R5): what the Evaluate box and the Watches
//! tree remember of what the user typed, most recent first.
//!
//! Qt-free and unit-tested on purpose — "was this typed before, and in what
//! order" is a rule, not a widget's business.

use std::collections::VecDeque;

/// A bounded, most-recent-first list of evaluated expressions. Re-evaluating
/// an expression that is already in the history moves it to the front
/// rather than duplicating it — the same "recent" a shell history gives.
pub struct EvaluateHistory {
    entries: VecDeque<String>,
    capacity: usize,
}

impl EvaluateHistory {
    pub fn new(capacity: usize) -> EvaluateHistory {
        EvaluateHistory {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// Record an evaluation. Blank expressions are not history — there is
    /// nothing there to recall.
    pub fn record(&mut self, expression: &str) {
        let expression = expression.trim();
        if expression.is_empty() {
            return;
        }
        self.entries.retain(|existing| existing != expression);
        self.entries.push_front(expression.to_string());
        self.entries.truncate(self.capacity);
    }

    /// Every remembered expression, most recent first.
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

impl Default for EvaluateHistory {
    /// 50: enough to page back through a debugging session without the list
    /// itself becoming something to search.
    fn default() -> EvaluateHistory {
        EvaluateHistory::new(50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_expression_comes_first() {
        let mut history = EvaluateHistory::default();
        history.record("a");
        history.record("b");
        assert_eq!(history.entries().collect::<Vec<_>>(), vec!["b", "a"]);
    }

    #[test]
    fn re_evaluating_moves_it_to_the_front_instead_of_duplicating_it() {
        let mut history = EvaluateHistory::default();
        history.record("a");
        history.record("b");
        history.record("a");
        assert_eq!(history.entries().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn a_blank_expression_is_not_recorded() {
        let mut history = EvaluateHistory::default();
        history.record("   ");
        assert_eq!(history.entries().count(), 0);
    }

    #[test]
    fn capacity_drops_the_oldest_entry() {
        let mut history = EvaluateHistory::new(2);
        history.record("a");
        history.record("b");
        history.record("c");
        assert_eq!(history.entries().collect::<Vec<_>>(), vec!["c", "b"]);
    }
}
