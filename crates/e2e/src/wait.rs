//! The one waiting primitive.
//!
//! Modelled on `lsp-core/tests/stub_server/support.rs`'s `wait_for`: a
//! deadline, a poll, a predicate, and a panic naming what was waited for.
//!
//! A test that passes because 200 ms happened to be enough is worse than no
//! test: it goes green in CI and red on a loaded laptop, which teaches
//! whoever sees it to re-run rather than debug. So nothing in this suite ever
//! waits for a duration — it waits for a transition, and `POLL_INTERVAL`
//! below is the only `sleep` in the whole test tree.

use std::time::{Duration, Instant};

/// How often a wait re-checks. The only `sleep` in the test tree.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Ceiling on any one wait. Generous on purpose: this bound exists to turn a
/// hang into a readable failure, never to assert that something was fast.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll `probe` until it yields, or panic naming `what`.
pub fn wait_for<T>(what: &str, probe: impl FnMut() -> Option<T>) -> T {
    wait_for_within(what, DEFAULT_TIMEOUT, probe)
}

/// `wait_for` with an explicit ceiling, for a wait that is genuinely
/// expected to be short (a window mapping) or genuinely long (an index).
pub fn wait_for_within<T>(
    what: &str,
    timeout: Duration,
    mut probe: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("timed out after {timeout:?} waiting for {what}");
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
