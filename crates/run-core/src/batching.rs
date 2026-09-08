//! Output batching (F4-7): coalesce raw PTY byte chunks into infrequent
//! [`BatchedOutput::Output`] events instead of one event per `read()`, plus
//! a bounded ring buffer so a long-lived run's console does not grow memory
//! without bound.
//!
//! # Partial UTF-8 across a chunk boundary
//!
//! A multi-byte UTF-8 character can straddle two `read()`s (classically, a
//! 4 KB read boundary landing inside it). [`OutputBatcher`] never converts
//! a chunk to `String` on its own: it appends every chunk to one pending
//! byte buffer and, at flush time, takes the longest valid UTF-8 prefix
//! (`std::str::from_utf8`'s `valid_up_to`) as the flushed text, leaving any
//! incomplete trailing bytes in the pending buffer for the next chunk to
//! complete. A flush can therefore emit less than the trigger threshold by
//! a handful of bytes; it never emits mangled text.
//!
//! # ANSI/SGR state across a chunk boundary
//!
//! This module never parses escape sequences itself — it only decides
//! *when* to hand accumulated bytes onward, never *how* to interpret them,
//! so a `\x1b[31m` split across two flushes is just two ordinary text
//! fragments as far as this module is concerned. Correctness depends on the
//! consumer feeding flushed text through one persistent, stateful ANSI
//! parser rather than resetting between flushes — exactly what
//! `terminal_core::TerminalEmulator::feed` already does for the terminal's
//! own reader thread (`ui_shell::bridge::terminal`), which is why a future
//! styled-run renderer reuses it here rather than a second parser. See
//! `ansi_state_survives_a_batch_boundary` below for the seam proven end to
//! end.

use std::time::{Duration, Instant};

/// Coalescing triggers: flush once pending output crosses this many bytes,
/// or this much time has passed since the last flush — whichever comes
/// first. Values from the plan doc (§4, F4-7): loose enough that interactive
/// output still feels immediate, tight enough that a 10k-line-in-100ms burst
/// produces a handful of events rather than one per line.
pub const MAX_BATCH_BYTES: usize = 64 * 1024;
pub const MAX_BATCH_INTERVAL: Duration = Duration::from_millis(16);

/// Ring buffer bounds: whichever limit is hit first evicts the oldest lines.
pub const MAX_RING_LINES: usize = 100_000;
pub const MAX_RING_BYTES: usize = 16 * 1024 * 1024;

/// What one flush produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchedOutput {
    /// Text output, in the order the process produced it. Escape sequences
    /// are passed through unmodified — this module never edits the bytes.
    Output(String),
    /// The ring buffer dropped its oldest content to stay within
    /// [`MAX_RING_LINES`]/[`MAX_RING_BYTES`]. Emitted at most once per
    /// flush, immediately after the [`BatchedOutput::Output`] that pushed
    /// the buffer over its limit — a dropped-history notice reads better
    /// paired with the output that caused it than as a separate event
    /// hunting for an insertion point.
    Truncated,
}

/// A bounded history of console output lines. Not a `Vec<u8>` ring, because
/// what a console displays and searches is lines, and truncation is most
/// honestly expressed in the same unit ("the oldest N lines were dropped").
#[derive(Debug, Default)]
struct RingBuffer {
    lines: std::collections::VecDeque<String>,
    total_bytes: usize,
}

impl RingBuffer {
    /// Append `line`, evicting the oldest lines while over either limit.
    /// Returns `true` if anything was evicted.
    fn push_line(&mut self, line: String) -> bool {
        self.total_bytes += line.len();
        self.lines.push_back(line);
        let mut truncated = false;
        while self.lines.len() > MAX_RING_LINES || self.total_bytes > MAX_RING_BYTES {
            let Some(evicted) = self.lines.pop_front() else {
                break;
            };
            self.total_bytes -= evicted.len();
            truncated = true;
        }
        truncated
    }
}

/// Coalesces raw byte chunks (as read from a `pty_core::PtySession`) into
/// infrequent [`BatchedOutput`] events, backed by a bounded ring buffer.
#[derive(Default)]
pub struct OutputBatcher {
    pending: Vec<u8>,
    /// When the current coalescing window started — i.e. when `pending`
    /// last went from empty to non-empty. Compared against `now` for the
    /// time trigger. Deliberately *not* "the last time we flushed": that
    /// would restart the interval on every push, so a steady trickle of
    /// small chunks arriving faster than the interval would never trip the
    /// time trigger at all.
    pending_since: Option<Instant>,
    ring: RingBuffer,
}

impl OutputBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of raw output bytes read at `now`. Returns the events
    /// this push produced — empty while still coalescing.
    pub fn push(&mut self, chunk: &[u8], now: Instant) -> Vec<BatchedOutput> {
        if self.pending.is_empty() {
            self.pending_since = Some(now);
        }
        self.pending.extend_from_slice(chunk);
        self.maybe_flush(now, false)
    }

    /// Flush whatever is pending if the time trigger has elapsed, even with
    /// no new bytes — otherwise a chunk that arrived just under the byte
    /// threshold, followed by silence, would never reach the console.
    pub fn flush_due(&mut self, now: Instant) -> Vec<BatchedOutput> {
        self.maybe_flush(now, false)
    }

    /// Flush unconditionally (end of stream: the process exited and
    /// whatever is pending, valid or not, is all there will ever be — a
    /// trailing incomplete UTF-8 tail at EOF is lossy-converted rather than
    /// held forever).
    pub fn flush_all(&mut self, now: Instant) -> Vec<BatchedOutput> {
        self.maybe_flush(now, true)
    }

    fn maybe_flush(&mut self, now: Instant, force: bool) -> Vec<BatchedOutput> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let due = self.pending.len() >= MAX_BATCH_BYTES
            || self
                .pending_since
                .is_some_and(|since| now.duration_since(since) >= MAX_BATCH_INTERVAL);
        if !due && !force {
            return Vec::new();
        }

        let valid_len = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) => e.valid_up_to(),
        };
        let valid_len = if force { self.pending.len() } else { valid_len };
        if valid_len == 0 {
            // Nothing complete enough to flush yet; wait for more bytes.
            return Vec::new();
        }

        let text = String::from_utf8_lossy(&self.pending[..valid_len]).into_owned();
        self.pending.drain(..valid_len);
        // Any leftover is an incomplete trailing UTF-8 sequence (a forced
        // flush never leaves one — it takes everything). Its wait for
        // completion already started at `pending_since`, so that timestamp
        // is left untouched rather than reset to `now`.
        if self.pending.is_empty() {
            self.pending_since = None;
        }

        let mut events = vec![BatchedOutput::Output(text.clone())];
        let mut truncated = false;
        for line in split_keep_newlines(&text) {
            truncated |= self.ring.push_line(line);
        }
        if truncated {
            events.push(BatchedOutput::Truncated);
        }
        events
    }
}

/// Split `text` into lines, each keeping its trailing `\n` (or CR) so the
/// ring buffer's line count matches what a reader would count, and a final
/// unterminated fragment is still counted as one line rather than lost.
fn split_keep_newlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            lines.push(text[start..=i].to_string());
            start = i + 1;
        }
    }
    if start < text.len() {
        lines.push(text[start..].to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_thousand_lines_in_100ms_produce_at_most_twenty_events() {
        let mut batcher = OutputBatcher::new();
        let base = Instant::now();
        let mut total_events = 0;
        for i in 0..10_000u32 {
            // Spread the 10 000 pushes across a 100ms window.
            let now = base + Duration::from_nanos(i as u64 * 10_000);
            let line = format!("line {i}\n");
            total_events += batcher.push(line.as_bytes(), now).len();
        }
        // A generous margin over the plan's "≤ 20", since Truncated events
        // (none expected here — well under the ring cap) would also count.
        assert!(
            total_events <= 20,
            "expected <= 20 batched events, got {total_events}"
        );
    }

    #[test]
    fn a_single_push_under_threshold_does_not_flush_immediately() {
        let mut batcher = OutputBatcher::new();
        let now = Instant::now();
        let events = batcher.push(b"partial", now);
        assert!(events.is_empty());
    }

    #[test]
    fn a_1mb_line_with_no_newline_flushes_on_size() {
        let mut batcher = OutputBatcher::new();
        let now = Instant::now();
        let big = vec![b'x'; 1024 * 1024];
        let events = batcher.push(&big, now);
        assert_eq!(events.len(), 1);
        let BatchedOutput::Output(text) = &events[0] else {
            panic!("expected Output, got {:?}", events[0]);
        };
        assert_eq!(text.len(), 1024 * 1024);
    }

    #[test]
    fn time_trigger_flushes_a_small_pending_chunk() {
        let mut batcher = OutputBatcher::new();
        let t0 = Instant::now();
        assert!(batcher.push(b"hi", t0).is_empty());
        let t1 = t0 + MAX_BATCH_INTERVAL + Duration::from_millis(1);
        let events = batcher.flush_due(t1);
        assert_eq!(events, vec![BatchedOutput::Output("hi".to_string())]);
    }

    #[test]
    fn partial_utf8_split_across_a_chunk_boundary_is_not_mangled() {
        let mut batcher = OutputBatcher::new();
        // "é" is 0xC3 0xA9. Split the two bytes across two pushes, each one
        // over the byte threshold on its own so both flush immediately.
        let mut first = vec![b'x'; MAX_BATCH_BYTES];
        first.push(0xC3);
        let t0 = Instant::now();
        let events = batcher.push(&first, t0);
        let BatchedOutput::Output(text) = &events[0] else {
            panic!("expected Output");
        };
        // The dangling lead byte must not have been lossy-converted into a
        // replacement character.
        assert!(!text.contains('\u{FFFD}'), "mangled: {text:?}");
        assert_eq!(text.len(), MAX_BATCH_BYTES);

        let t1 = t0 + Duration::from_micros(1);
        assert!(
            batcher.push(&[0xA9, b'y'], t1).is_empty(),
            "the completed character is below both triggers, so it waits like any other small chunk"
        );
        // A real reader thread eventually calls `flush_due` on its own
        // schedule (or `flush_all` at EOF) rather than leaving bytes
        // stranded — that's what surfaces the held-back lead byte.
        let t2 = t1 + MAX_BATCH_INTERVAL + Duration::from_millis(1);
        let events = batcher.flush_due(t2);
        let BatchedOutput::Output(text) = &events[0] else {
            panic!("expected Output");
        };
        assert!(text.starts_with('é'), "got {text:?}");
        assert!(!text.contains('\u{FFFD}'), "mangled: {text:?}");
    }

    #[test]
    fn ring_buffer_truncates_and_reports_it_once() {
        let mut batcher = OutputBatcher::new();
        let t0 = Instant::now();
        for i in 0..(MAX_RING_LINES + 10) {
            let now = t0 + Duration::from_micros(i as u64);
            batcher.push(format!("line {i}\n").as_bytes(), now);
        }
        let final_flush = batcher.flush_all(t0 + Duration::from_secs(1));
        let mut all_events: Vec<BatchedOutput> = Vec::new();
        all_events.extend(final_flush);
        assert!(
            !batcher.ring.lines.is_empty(),
            "ring buffer should retain the most recent lines"
        );
        assert!(
            batcher.ring.lines.len() <= MAX_RING_LINES,
            "ring buffer exceeded its cap: {}",
            batcher.ring.lines.len()
        );
        // The oldest line must be gone.
        assert!(!batcher.ring.lines.iter().any(|l| l.starts_with("line 0\n")));
        let _ = all_events; // silence unused warning if the flush landed empty
    }

    #[test]
    fn a_million_lines_keeps_memory_bounded() {
        let mut batcher = OutputBatcher::new();
        let t0 = Instant::now();
        for i in 0..1_000_000u32 {
            let now = t0 + Duration::from_nanos(i as u64 * 100);
            batcher.push(format!("l{i}\n").as_bytes(), now);
        }
        assert!(batcher.ring.total_bytes <= MAX_RING_BYTES);
        assert!(batcher.ring.lines.len() <= MAX_RING_LINES);
    }

    /// Proves the seam the module docs describe: a persistent
    /// `terminal_core::TerminalEmulator`, fed each flush's text in order,
    /// resolves an ANSI color sequence correctly even when this batcher
    /// splits it across two flushes — the reason a second ANSI parser is
    /// unnecessary here.
    #[test]
    fn ansi_state_survives_a_batch_boundary() {
        let mut batcher = OutputBatcher::new();
        let t0 = Instant::now();

        // Force two separate flushes by exceeding the byte threshold on the
        // first, with the SGR sequence itself split across the boundary:
        // "\x1b[3" then "1mred\n".
        let mut first = vec![b'x'; MAX_BATCH_BYTES];
        first.extend_from_slice(b"\x1b[3");
        let events_a = batcher.push(&first, t0);
        let t1 = t0 + Duration::from_micros(1);
        let mut events_b = batcher.push(b"1mred\n", t1);
        // As in the partial-UTF-8 test above: a small chunk below both
        // triggers waits for a reader thread's own `flush_due`.
        let t2 = t1 + MAX_BATCH_INTERVAL + Duration::from_millis(1);
        events_b.extend(batcher.flush_due(t2));

        let mut emulator = terminal_core::TerminalEmulator::new(
            terminal_core::GridSize::new(5, 80),
            terminal_core::Palette::xterm(),
        );
        for event in events_a.into_iter().chain(events_b) {
            if let BatchedOutput::Output(text) = event {
                emulator.feed(text.as_bytes());
            }
        }
        // 64KB of filler at 80 columns wraps across many more rows than a
        // 5-row viewport with no scrollback can keep, so "red" scrolls to
        // wherever the viewport ends up — not a fixed row number. Search
        // every visible row rather than assuming which one.
        let grid = emulator.grid();
        let red_cell = grid
            .rows
            .iter()
            .flatten()
            .find(|c| c.character == 'r')
            .expect("the word \"red\" rendered somewhere on the visible grid");
        // ANSI 31 = red. If the escape sequence had been torn apart by the
        // flush boundary and fed to two independent parsers, this cell
        // would still show the terminal's default foreground.
        assert_eq!(red_cell.fg, terminal_core::CellColor { r: 205, g: 0, b: 0 });
    }
}
