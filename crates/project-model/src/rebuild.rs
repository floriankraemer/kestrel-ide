//! Collapsing a burst of filesystem-watcher events into a bounded number of
//! tree rebuilds.
//!
//! A rebuild re-walks the whole project from disk ([`crate::rebuild_tree_sorted`]),
//! so its cost scales with the project and it is emphatically not something to
//! run once per event: a `git checkout`, a `cargo build` or the initial
//! watch-registration burst produce thousands of structural events in a few
//! seconds, and every one of them asks the same question — "what does the tree
//! look like now?" — whose answer only the last one needs.
//!
//! Running them concurrently is worse than wasteful, it is unsurvivable. Each
//! in-flight rebuild holds a complete second copy of the tree while it builds,
//! and the walk covers build output as well as source: on this repository's own
//! checkout that is 424 000 entries and roughly 130 MB per rebuild. Two hundred
//! of them at once is thirty gigabytes, which is exactly how the app came to be
//! OOM-killed moments after opening its own repository.
//!
//! This is the same judgement the VCS status relay records for its own
//! watcher-driven refresh (ADR-0031 §7): one request in flight, one folded
//! behind it, and a request that arrives during a walk is answered by re-running
//! once at the end rather than by starting a second walk beside it.

/// Decides, for one project, whether a watcher event should start a tree
/// rebuild now, be folded into the rebuild already running, or trigger one
/// final catch-up rebuild once that one lands.
///
/// Holds no tree and does no I/O — it is the rule, not the work, so it lives
/// here rather than in the adapter that owns the worker thread.
#[derive(Debug, Default)]
pub struct RebuildCoalescer {
    /// A rebuild has been started and has not reported back yet.
    running: bool,
    /// At least one event arrived while that rebuild was walking, so its
    /// answer is already stale and one more rebuild is owed.
    queued: bool,
}

impl RebuildCoalescer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A structural watcher event arrived.
    ///
    /// `true` means "start a rebuild now". `false` means one is already
    /// running and this event has been folded into the single catch-up
    /// rebuild [`Self::finished`] will ask for — the caller must not spawn.
    pub fn request(&mut self) -> bool {
        if self.running {
            self.queued = true;
            return false;
        }
        self.running = true;
        true
    }

    /// The rebuild that was running has landed (or failed, or was abandoned
    /// because no project is open any more — every exit counts, or the flag
    /// stays set and every later event is silently dropped forever).
    ///
    /// `true` means events arrived while it was walking, so start exactly one
    /// more; the coalescer stays "running" across that hand-off rather than
    /// briefly opening a window for a third.
    pub fn finished(&mut self) -> bool {
        if self.queued {
            self.queued = false;
            return true;
        }
        self.running = false;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_event_starts_a_rebuild() {
        let mut coalescer = RebuildCoalescer::new();
        assert!(coalescer.request());
    }

    #[test]
    fn a_burst_during_a_walk_starts_no_second_rebuild() {
        let mut coalescer = RebuildCoalescer::new();
        assert!(coalescer.request());
        for _ in 0..1000 {
            assert!(
                !coalescer.request(),
                "a rebuild is already walking; nothing else may spawn beside it"
            );
        }
    }

    #[test]
    fn a_burst_is_answered_by_exactly_one_catch_up_rebuild() {
        let mut coalescer = RebuildCoalescer::new();
        coalescer.request();
        for _ in 0..1000 {
            coalescer.request();
        }
        assert!(coalescer.finished(), "the folded events are owed a rebuild");
        assert!(
            !coalescer.finished(),
            "and only one — the catch-up covers all of them"
        );
    }

    #[test]
    fn a_quiet_rebuild_asks_for_no_follow_up() {
        let mut coalescer = RebuildCoalescer::new();
        coalescer.request();
        assert!(!coalescer.finished());
    }

    #[test]
    fn the_next_event_after_a_quiet_rebuild_starts_a_fresh_one() {
        let mut coalescer = RebuildCoalescer::new();
        coalescer.request();
        coalescer.finished();
        assert!(
            coalescer.request(),
            "the coalescer must not latch: a later change still rebuilds"
        );
    }

    /// The failure mode this type exists to prevent, stated as a count: a
    /// thousand events must never put more than one walk on the machine at a
    /// time, and must cost two walks in total rather than a thousand.
    #[test]
    fn a_thousand_events_cost_two_walks_and_never_overlap() {
        let mut coalescer = RebuildCoalescer::new();
        let mut in_flight = 0;
        let mut walks = 0;

        for _ in 0..1000 {
            if coalescer.request() {
                in_flight += 1;
                walks += 1;
            }
            assert!(in_flight <= 1, "two rebuilds walked the tree at once");
        }
        // The walk lands; drain the catch-up chain the same way the adapter does.
        loop {
            in_flight -= 1;
            if !coalescer.finished() {
                break;
            }
            in_flight += 1;
            walks += 1;
            assert!(in_flight <= 1, "two rebuilds walked the tree at once");
        }

        assert_eq!(walks, 2, "one for the first event, one to catch up");
        assert_eq!(in_flight, 0);
    }
}
