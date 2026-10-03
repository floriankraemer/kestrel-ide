//! Format-on-save off the Qt thread: the per-tab state of a save that waits
//! for a formatter (a container `run` can take seconds).
//!
//! Ctrl+S starts a format on a worker ([`PendingSaves::begin`]) and returns
//! at once, so the UI keeps answering. The worker's answer is parked with
//! [`PendingSaves::formatted`]; the view is then told, hands over the
//! buffer's revision and text, and [`PendingSaves::take`] decides what is
//! written:
//!
//! ```text
//! Idle --begin--> Formatting{revision, ticket} --formatted--> Ready --take--> Idle (view writes)
//!   ^                    |                                      |
//!   +------cancel--------+--------------------------------------+
//! ```
//!
//! Writing is not a resting state: the view applies the edits and writes
//! the file synchronously inside the same Qt-thread call that took them.
//!
//! The formatter's answer is a whole-file rewrite of the text it was given,
//! so it applies only to that exact text. When the buffer moved on while
//! the formatter ran (the user kept typing), the save goes ahead with what
//! the buffer holds now, unformatted, and says so — re-running would chase
//! a user who is still typing, and dropping the save would lose the Ctrl+S.

use std::collections::HashMap;

use crate::TabId;

/// Why a save went ahead without the formatter's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatSkipped {
    /// The formatter failed or timed out: `(tool name, its message)`.
    Failed { tool: String, message: String },
    /// The buffer changed while the formatter ran.
    BufferChanged,
    /// A save that does not wait for a formatter (closing, quitting): the
    /// formatter can take seconds, and these saves must happen now.
    NotWaited,
}

/// What the formatter produced for the text it was given: `Ok(None)` when no
/// formatter applied after all (no language server for the file).
pub type FormatOutcome = Result<Option<String>, FormatSkipped>;

/// The text to write, decided by [`PendingSaves::take`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveWrite {
    /// The formatted whole text, when it applies to the buffer as it is.
    pub formatted: Option<String>,
    /// Why formatting was skipped, for the notice the view shows.
    pub skipped: Option<FormatSkipped>,
}

/// What [`PendingSaves::begin`] asks of the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// Run the formatter and report back with this ticket.
    Start(u64),
    /// The same text is already being formatted: nothing to start.
    AlreadyFormatting,
}

#[derive(Debug)]
struct Pending {
    ticket: u64,
    revision: i64,
    text: String,
    outcome: Option<FormatOutcome>,
}

/// Every tab's pending format-on-save.
#[derive(Debug, Default)]
pub struct PendingSaves {
    tabs: HashMap<TabId, Pending>,
    last_ticket: u64,
}

impl PendingSaves {
    /// Ctrl+S on `tab` at buffer `revision` holding `text`. A second Ctrl+S
    /// on unchanged text joins the run in flight; on changed text it starts
    /// over, and the older run's answer is dropped when it arrives.
    pub fn begin(&mut self, tab: TabId, revision: i64, text: &str) -> Begin {
        if let Some(pending) = self.tabs.get(&tab) {
            if pending.outcome.is_none() && pending.revision == revision && pending.text == text {
                return Begin::AlreadyFormatting;
            }
        }
        self.last_ticket += 1;
        let ticket = self.last_ticket;
        self.tabs.insert(
            tab,
            Pending {
                ticket,
                revision,
                text: text.to_string(),
                outcome: None,
            },
        );
        Begin::Start(ticket)
    }

    /// The text a started run formats.
    pub fn text(&self, tab: TabId, ticket: u64) -> Option<&str> {
        self.tabs
            .get(&tab)
            .filter(|p| p.ticket == ticket)
            .map(|p| p.text.as_str())
    }

    /// The worker answered for `ticket`. True when the view must now be
    /// told; false for an answer nobody waits for any more (cancelled,
    /// superseded, or the tab closed).
    pub fn formatted(&mut self, tab: TabId, ticket: u64, outcome: FormatOutcome) -> bool {
        match self.tabs.get_mut(&tab) {
            Some(pending) if pending.ticket == ticket && pending.outcome.is_none() => {
                pending.outcome = Some(outcome);
                true
            }
            _ => false,
        }
    }

    /// The view, told the answer is in, hands over the buffer as it is now.
    /// `None` when there is nothing to write (no answer ready for this tab).
    pub fn take(&mut self, tab: TabId, revision: i64, text: &str) -> Option<SaveWrite> {
        self.tabs.get(&tab)?.outcome.as_ref()?;
        let pending = self.tabs.remove(&tab)?;
        let unchanged = pending.revision == revision && pending.text == text;
        let write = match pending.outcome? {
            _ if !unchanged => SaveWrite {
                formatted: None,
                skipped: Some(FormatSkipped::BufferChanged),
            },
            Ok(formatted) => SaveWrite {
                formatted,
                skipped: None,
            },
            Err(skipped) => SaveWrite {
                formatted: None,
                skipped: Some(skipped),
            },
        };
        Some(write)
    }

    /// Forget `tab`'s pending save: the tab closed, or a synchronous save
    /// (closing, quitting) wrote it without waiting.
    pub fn cancel(&mut self, tab: TabId) {
        self.tabs.remove(&tab);
    }

    /// Whether a format-on-save is running or waiting to be written.
    pub fn is_pending(&self, tab: TabId) -> bool {
        self.tabs.contains_key(&tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: TabId = TabId(7);

    fn started(saves: &mut PendingSaves, revision: i64, text: &str) -> u64 {
        match saves.begin(TAB, revision, text) {
            Begin::Start(ticket) => ticket,
            other => panic!("expected a start, got {other:?}"),
        }
    }

    #[test]
    fn an_unchanged_buffer_gets_the_formatted_text() {
        let mut saves = PendingSaves::default();
        let ticket = started(&mut saves, 3, "a  =1;");
        assert_eq!(saves.text(TAB, ticket), Some("a  =1;"));
        assert!(
            saves.take(TAB, 3, "a  =1;").is_none(),
            "nothing until the answer"
        );
        assert!(saves.formatted(TAB, ticket, Ok(Some("a = 1;".into()))));
        let write = saves.take(TAB, 3, "a  =1;").unwrap();
        assert_eq!(write.formatted.as_deref(), Some("a = 1;"));
        assert_eq!(write.skipped, None);
        assert!(!saves.is_pending(TAB), "back to idle");
    }

    #[test]
    fn typing_while_formatting_saves_the_buffer_unformatted() {
        let mut saves = PendingSaves::default();
        let ticket = started(&mut saves, 3, "a  =1;");
        saves.formatted(TAB, ticket, Ok(Some("a = 1;".into())));
        let write = saves.take(TAB, 4, "a  =1;x").unwrap();
        assert_eq!(write.formatted, None);
        assert_eq!(write.skipped, Some(FormatSkipped::BufferChanged));
    }

    #[test]
    fn same_revision_with_different_text_is_a_changed_buffer() {
        // Qt can hand back an earlier revision after an undo/redo dance;
        // the text decides too.
        let mut saves = PendingSaves::default();
        let ticket = started(&mut saves, 3, "one");
        saves.formatted(TAB, ticket, Ok(Some("ONE".into())));
        let write = saves.take(TAB, 3, "two").unwrap();
        assert_eq!(write.skipped, Some(FormatSkipped::BufferChanged));
    }

    #[test]
    fn a_failed_formatter_still_saves_and_says_why() {
        let mut saves = PendingSaves::default();
        let ticket = started(&mut saves, 1, "x");
        let failure = FormatSkipped::Failed {
            tool: "PHP CS Fixer".into(),
            message: "the formatter timed out".into(),
        };
        saves.formatted(TAB, ticket, Err(failure.clone()));
        let write = saves.take(TAB, 1, "x").unwrap();
        assert_eq!(write.formatted, None);
        assert_eq!(write.skipped, Some(failure));
    }

    #[test]
    fn a_second_save_of_the_same_text_joins_the_run_in_flight() {
        let mut saves = PendingSaves::default();
        started(&mut saves, 1, "x");
        assert_eq!(saves.begin(TAB, 1, "x"), Begin::AlreadyFormatting);
    }

    #[test]
    fn a_save_of_newer_text_supersedes_the_older_run() {
        let mut saves = PendingSaves::default();
        let old = started(&mut saves, 1, "x");
        let new = started(&mut saves, 2, "xy");
        assert!(
            !saves.formatted(TAB, old, Ok(Some("X".into()))),
            "stale answer dropped"
        );
        assert!(saves.take(TAB, 2, "xy").is_none());
        assert!(saves.formatted(TAB, new, Ok(Some("XY".into()))));
        assert_eq!(
            saves.take(TAB, 2, "xy").unwrap().formatted.as_deref(),
            Some("XY")
        );
    }

    #[test]
    fn a_cancelled_save_ignores_its_late_answer() {
        let mut saves = PendingSaves::default();
        let ticket = started(&mut saves, 1, "x");
        saves.cancel(TAB);
        assert!(!saves.formatted(TAB, ticket, Ok(Some("X".into()))));
        assert!(saves.take(TAB, 1, "x").is_none());
        assert!(!saves.is_pending(TAB));
    }

    #[test]
    fn an_answer_is_written_once() {
        let mut saves = PendingSaves::default();
        let ticket = started(&mut saves, 1, "x");
        saves.formatted(TAB, ticket, Ok(None));
        assert!(
            !saves.formatted(TAB, ticket, Ok(Some("X".into()))),
            "no second answer"
        );
        assert_eq!(saves.take(TAB, 1, "x").unwrap().formatted, None);
        assert!(saves.take(TAB, 1, "x").is_none());
    }

    #[test]
    fn a_new_save_after_a_ready_answer_starts_fresh() {
        let mut saves = PendingSaves::default();
        let first = started(&mut saves, 1, "x");
        saves.formatted(TAB, first, Ok(Some("X".into())));
        // The view has not taken it yet; Ctrl+S again on the same text.
        let second = started(&mut saves, 1, "x");
        assert_ne!(first, second);
        assert!(saves.take(TAB, 1, "x").is_none(), "waiting for the new run");
    }
}
