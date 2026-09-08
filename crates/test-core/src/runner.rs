//! Running a test framework's process and turning its streamed output into
//! tree events (D2/D4).
//!
//! Mirrors `build_core::runner`'s shape — blocking by design, the caller
//! (`ui-shell`'s `TestServiceRust`, D4) gives it a thread and turns its
//! callbacks into Qt signals — but spawns over pipes
//! (`process_exec::spawn`) rather than `run_core::Supervisor`'s PTY, for
//! the plan's stated reason: a tty hard-wraps output, and a TeamCity
//! service message split mid-line by an 80-column wrap parses as garbage.
//!
//! `process_exec::run` (collect-to-completion) is the wrong primitive
//! here — a caller that wants "the tree fills while the run is in flight"
//! cannot use a function that returns after the process has already
//! exited, which is exactly why `process_exec::spawn` exists.

use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::teamcity::{TeamCityEvent, TeamCityParser};

/// What a caller wants to hear while a run is in flight.
pub trait TestSink {
    /// A chunk of raw stdout, exactly as the process wrote it (colours and
    /// all — `run_core::AnsiStripper` is applied by the view layer per the
    /// plan's "Reuse, not reinvention" section, not here).
    fn output(&mut self, text: &str);
    /// One parsed TeamCity service message, as it streams in.
    fn event(&mut self, event: TeamCityEvent);
}

/// A handle another thread can use to stop a running test process.
#[derive(Clone, Default)]
pub struct TestRunHandle {
    spawned: Arc<Mutex<Option<process_exec::Spawned>>>,
}

impl TestRunHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Kill the running process, if any. Stopping a run that has already
    /// finished (or never started) is not an error.
    pub fn stop(&self) {
        if let Ok(guard) = self.spawned.lock() {
            if let Some(spawned) = guard.as_ref() {
                spawned.kill();
            }
        }
    }
}

/// Why a run could not be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunFailure {
    NotFound,
    Io(String),
}

/// Spawn `program args` in `work_dir`, streaming stdout through a
/// [`TeamCityParser`] and reporting every event and every raw chunk to
/// `sink` as they arrive. Blocks until the process exits or
/// [`TestRunHandle::stop`] kills it.
///
/// Returns the exit code, or `None` when the process could not be waited
/// on (most commonly: it was just stopped).
pub fn run(
    handle: &TestRunHandle,
    program: &str,
    args: &[String],
    work_dir: &Path,
    sink: &mut dyn TestSink,
) -> Result<Option<i32>, RunFailure> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let spawned = process_exec::spawn(program, &arg_refs, work_dir).map_err(|e| match e {
        process_exec::Failure::NotFound => RunFailure::NotFound,
        process_exec::Failure::Io(msg) => RunFailure::Io(msg),
        // `spawn` never blocks waiting for exit, so it has no timeout to
        // report; kept as an arm rather than matched away so a future
        // change to `process_exec::Failure` is a compile error here, not a
        // silent gap.
        process_exec::Failure::TimedOut => RunFailure::Io("unexpected timeout".into()),
    })?;
    *handle
        .spawned
        .lock()
        .map_err(|_| RunFailure::Io("run handle lock poisoned".into()))? = Some(spawned.clone());

    let mut stdout = spawned.take_stdout().expect("stdout was piped");
    let mut stderr = spawned.take_stderr().expect("stderr was piped");

    // Drained on its own thread purely to avoid the pipe-fills-and-blocks
    // deadlock `process_exec::run` avoids the same way. A test framework's
    // own diagnostic chatter belongs on stdout in every format this plan
    // supports, so stderr is captured wholesale rather than streamed to
    // `sink` — an acceptable v1 simplification, not a silent data loss:
    // nothing observed on stdout is affected by it.
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    let mut parser = TeamCityParser::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = match stdout.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let chunk = String::from_utf8_lossy(&buffer[..read]).into_owned();
        for event in parser.feed(&chunk) {
            sink.event(event);
        }
        sink.output(&chunk);
    }
    for event in parser.finish() {
        sink.event(event);
    }
    let _ = stderr_reader.join();

    *handle
        .spawned
        .lock()
        .map_err(|_| RunFailure::Io("run handle lock poisoned".into()))? = None;
    Ok(spawned.wait().ok().and_then(|status| status.code()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Collected {
        output: String,
        events: Vec<TeamCityEvent>,
    }

    impl TestSink for Collected {
        fn output(&mut self, text: &str) {
            self.output.push_str(text);
        }
        fn event(&mut self, event: TeamCityEvent) {
            self.events.push(event);
        }
    }

    #[test]
    fn events_arrive_as_the_process_writes_them() {
        let dir = tempfile::tempdir().unwrap();
        let handle = TestRunHandle::new();
        let mut collected = Collected::default();
        let script = "echo \"##teamcity[testStarted name='t']\"; \
                       echo \"##teamcity[testFinished name='t' duration='1']\"";
        let code = run(
            &handle,
            "sh",
            &["-c".into(), script.into()],
            dir.path(),
            &mut collected,
        )
        .unwrap();
        assert_eq!(code, Some(0));
        assert_eq!(collected.events.len(), 2);
        assert!(collected.output.contains("teamcity"));
    }

    #[test]
    fn a_missing_program_is_reported_as_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let handle = TestRunHandle::new();
        let mut collected = Collected::default();
        let err = run(
            &handle,
            "this-binary-does-not-exist-anywhere",
            &[],
            dir.path(),
            &mut collected,
        )
        .unwrap_err();
        assert!(matches!(err, RunFailure::NotFound));
    }

    #[test]
    fn stopping_a_run_ends_it_before_completion() {
        let dir = tempfile::tempdir().unwrap();
        let handle = TestRunHandle::new();
        let stopper = handle.clone();
        let stop_thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            stopper.stop();
        });
        let mut collected = Collected::default();
        let code = run(
            &handle,
            "sh",
            &["-c".into(), "sleep 5".into()],
            dir.path(),
            &mut collected,
        )
        .unwrap();
        stop_thread.join().unwrap();
        assert_ne!(code, Some(0));
    }
}
