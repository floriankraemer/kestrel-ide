//! Server-agnostic conformance-suite plumbing shared by
//! `real_server_conformance.rs` (rust-analyzer) and `csharp_conformance.rs`
//! (csharp-ls).
//!
//! Nothing here knows about a specific server or fixture — that stays in
//! each test file, which is the only place a divergent fixture shape or
//! expectations section belongs.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use lsp_core::manager::LspEvent;

/// Drain events until one matches, or fail naming what we waited for.
/// Non-matching events are skipped: a real server emits progress and log
/// notifications we do not care about here.
pub fn wait_for<T>(
    rx: &Receiver<LspEvent>,
    what: &str,
    timeout: Duration,
    mut pick: impl FnMut(&LspEvent) -> Option<T>,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out after {timeout:?} waiting for {what}");
        }
        match rx.recv_timeout(remaining) {
            Ok(event) => {
                if let LspEvent::ServerFailed { message, .. } = &event {
                    panic!("server failed while waiting for {what}: {message}");
                }
                if let Some(value) = pick(&event) {
                    return value;
                }
            }
            Err(e) => panic!("waiting for {what}: {e}"),
        }
    }
}

/// Poll `attempt` until it yields a value, or the deadline passes.
///
/// Real servers answer nothing usable until they finish indexing, and that
/// takes a variable amount of wall-clock time no event tells us about
/// precisely enough to await instead of poll. Returns how long it took, so
/// callers keep reporting the real cost rather than hiding it.
pub fn retry_until<T>(
    what: &str,
    timeout: Duration,
    mut attempt: impl FnMut() -> Option<T>,
) -> (T, Duration) {
    let started = Instant::now();
    let deadline = started + timeout;
    loop {
        if let Some(value) = attempt() {
            return (value, started.elapsed());
        }
        if Instant::now() >= deadline {
            panic!(
                "{what} still had no answer after {timeout:?} — the server never \
                 finished indexing, or the request is genuinely unsupported"
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
