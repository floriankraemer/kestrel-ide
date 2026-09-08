//! B5 — when an analyzer runs.
//!
//! Three triggers ([`crate::Trigger`]): `OnType` (debounced), `OnSave`,
//! `Manual`. This module owns none of the argv construction or output
//! parsing — it is purely "when", handed a program and arguments to spawn
//! and a callback to hand the result to. `ui-shell` (B8) is the one thing
//! that ever constructs a [`Scheduler`], on a background thread, so results
//! reach it through an ordinary `Send` closure it can forward to
//! `CxxQtThread::queue()` — nothing here knows Qt exists.
//!
//! No tokio: every run happens on its own `std::thread`, per the layering
//! table's rule for every background-work crate that isn't `ai-chat-core`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A finished run's raw output, before any output-format parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Why a run produced no [`RunOutput`]. Mirrors `process_exec::Failure`
/// rather than re-exporting it, so a caller that only has this crate in
/// scope (a settings page, say) need not also depend on `process-exec` to
/// match on the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunFailure {
    NotFound,
    TimedOut,
    Io(String),
}

pub type RunResult = Result<RunOutput, RunFailure>;

fn run_process(
    program: &Path,
    args: &[String],
    project_root: &Path,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> RunResult {
    let program_str = program.to_string_lossy();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    process_exec::run(&program_str, &arg_refs, project_root, stdin, timeout, &[])
        .map(|out| RunOutput {
            stdout: out.stdout,
            stderr: out.stderr,
        })
        .map_err(|e| match e {
            process_exec::Failure::NotFound => RunFailure::NotFound,
            process_exec::Failure::TimedOut => RunFailure::TimedOut,
            process_exec::Failure::Io(msg) => RunFailure::Io(msg),
        })
}

/// One (analyzer, file) key's generation counter.
///
/// `schedule_file_run` bumps this on every call; a debounce timer or a
/// finished process that finds the counter has moved on knows a newer
/// keystroke superseded it and drops its own result instead of delivering
/// a stale one. This is cooperative rather than a `Child::kill()` — the
/// process itself keeps running to completion or its own timeout — which
/// is a real cost for a slow analyzer typed over repeatedly and the
/// deliberate simplification here.
/// ponytail: cooperative-cancel-only, upgrade to killing the in-flight
/// `Child` (would need `process_exec::run` to hand one back) if a slow
/// analyzer run piling up under fast typing ever measurably matters.
type Generation = Arc<AtomicU64>;

/// Runs analyzer jobs on worker threads per the plan's trigger and
/// concurrency rules.
pub struct Scheduler {
    file_generations: Mutex<HashMap<(String, PathBuf), Generation>>,
    manual_running: Arc<AtomicBool>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            file_generations: Mutex::new(HashMap::new()),
            manual_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Schedule one analyzer against one file, debounced by `delay` (pass
    /// [`Duration::ZERO`] for `OnSave`, where there is nothing to debounce).
    ///
    /// One in-flight run per `(analyzer_id, file)`: calling this again for
    /// the same pair before the previous call's process has finished (or
    /// even started, if it is still inside `delay`) means the previous
    /// call's `on_result` is never invoked — the new call's generation has
    /// already superseded it. `on_result` runs on a worker thread, never on
    /// the caller's.
    #[allow(clippy::too_many_arguments)]
    pub fn schedule_file_run<F>(
        &self,
        analyzer_id: &str,
        program: PathBuf,
        args: Vec<String>,
        project_root: &Path,
        file: &Path,
        stdin: Option<Vec<u8>>,
        delay: Duration,
        timeout: Duration,
        on_result: F,
    ) where
        F: FnOnce(RunResult) + Send + 'static,
    {
        let key = (analyzer_id.to_string(), file.to_path_buf());
        let generation = {
            let mut generations = self.file_generations.lock().expect("not poisoned");
            let counter = generations
                .entry(key)
                .or_insert_with(|| Arc::new(AtomicU64::new(0)));
            counter.fetch_add(1, Ordering::SeqCst);
            counter.clone()
        };
        let my_generation = generation.load(Ordering::SeqCst);
        let project_root = project_root.to_path_buf();

        thread::spawn(move || {
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            if generation.load(Ordering::SeqCst) != my_generation {
                return; // superseded before the run even started
            }
            let result = run_process(&program, &args, &project_root, stdin.as_deref(), timeout);
            if generation.load(Ordering::SeqCst) != my_generation {
                return; // superseded while the process was running
            }
            on_result(result);
        });
    }

    /// Start a project-wide (`Manual`) run, or refuse to when one is
    /// already in flight — manual runs are serialized, never concurrent
    /// with each other. Returns whether this call actually started one;
    /// the caller (B9's "Inspect Project" action) uses that to decide
    /// whether to show "a project run is already running" rather than
    /// queueing a second one silently.
    pub fn run_manual<F>(
        &self,
        program: PathBuf,
        args: Vec<String>,
        project_root: &Path,
        timeout: Duration,
        on_result: F,
    ) -> bool
    where
        F: FnOnce(RunResult) + Send + 'static,
    {
        if self.manual_running.swap(true, Ordering::SeqCst) {
            return false;
        }
        let running = self.manual_running.clone();
        let project_root = project_root.to_path_buf();
        thread::spawn(move || {
            let result = run_process(&program, &args, &project_root, None, timeout);
            on_result(result);
            running.store(false, Ordering::SeqCst);
        });
        true
    }

    pub fn is_manual_run_in_progress(&self) -> bool {
        self.manual_running.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_file_run_delivers_its_result() {
        let scheduler = Scheduler::new();
        let (tx, rx) = mpsc::channel();
        scheduler.schedule_file_run(
            "phpstan",
            PathBuf::from("this-program-does-not-exist-anywhere"),
            vec![],
            Path::new("."),
            Path::new("a.php"),
            None,
            Duration::ZERO,
            Duration::from_secs(5),
            move |result| tx.send(result).unwrap(),
        );
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result, Err(RunFailure::NotFound));
    }

    #[test]
    fn a_second_schedule_for_the_same_key_supersedes_the_first() {
        let scheduler = Scheduler::new();
        let (tx, rx) = mpsc::channel::<&'static str>();
        let tx1 = tx.clone();
        scheduler.schedule_file_run(
            "phpstan",
            PathBuf::from("nope"),
            vec![],
            Path::new("."),
            Path::new("a.php"),
            None,
            Duration::from_millis(200),
            Duration::from_secs(5),
            move |_| tx1.send("first").unwrap(),
        );
        // Supersede before the first call's debounce elapses.
        scheduler.schedule_file_run(
            "phpstan",
            PathBuf::from("nope"),
            vec![],
            Path::new("."),
            Path::new("a.php"),
            None,
            Duration::ZERO,
            Duration::from_secs(5),
            move |_| tx.send("second").unwrap(),
        );
        let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(first, "second");
        // The first call's `on_result` must never fire.
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());
    }

    #[test]
    fn a_different_file_is_an_independent_key() {
        let scheduler = Scheduler::new();
        let (tx, rx) = mpsc::channel();
        scheduler.schedule_file_run(
            "phpstan",
            PathBuf::from("nope"),
            vec![],
            Path::new("."),
            Path::new("a.php"),
            None,
            Duration::ZERO,
            Duration::from_secs(5),
            {
                let tx = tx.clone();
                move |_| tx.send("a").unwrap()
            },
        );
        scheduler.schedule_file_run(
            "phpstan",
            PathBuf::from("nope"),
            vec![],
            Path::new("."),
            Path::new("b.php"),
            None,
            Duration::ZERO,
            Duration::from_secs(5),
            move |_| tx.send("b").unwrap(),
        );
        let mut seen = vec![
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        ];
        seen.sort_unstable();
        assert_eq!(seen, vec!["a", "b"]);
    }

    #[test]
    fn a_second_manual_run_is_refused_while_one_is_in_flight() {
        let scheduler = Scheduler::new();
        let (tx, rx) = mpsc::channel();
        let started = scheduler.run_manual(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_string(), "sleep 0.3".to_string()],
            Path::new("."),
            Duration::from_secs(5),
            move |result| tx.send(result).unwrap(),
        );
        assert!(started);
        assert!(scheduler.is_manual_run_in_progress());
        let refused = scheduler.run_manual(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_string(), "true".to_string()],
            Path::new("."),
            Duration::from_secs(5),
            |_| {},
        );
        assert!(!refused, "a manual run must not overlap another");

        let _ = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!scheduler.is_manual_run_in_progress());
    }
}
