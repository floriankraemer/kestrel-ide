//! One thread per connection: probe, snapshot, then follow the engine's
//! `events` stream and re-snapshot when something changed.
//!
//! The engine's event stream is bursty — one `compose up` is a dozen
//! events in a few milliseconds — so events are not applied individually:
//! any state-changing line marks the snapshot stale, and [`debounce_loop`]
//! re-snapshots once the stream has been quiet for [`DEBOUNCE`]. That loop
//! is fed by a plain `mpsc` channel, so the tests drive it with hand-timed
//! [`Signal`]s and never spawn a CLI.
//!
//! No tokio: blocking `std::thread`s and `std::sync::mpsc`, forwarded to
//! Qt by `ui-shell` through `qt_thread().queue(...)`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::connection::{ConnectionConfig, ConnectionKind, Engine, Invocation};
use crate::probe::{self, EngineInfo};
use crate::snapshot::EngineSnapshot;

/// How long the event stream has to stay quiet before a re-snapshot.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub enum ContainerEventKind {
    /// The engine answered the probe; a `Snapshot` follows.
    Connected(EngineInfo),
    Snapshot(EngineSnapshot),
    /// [`WatcherHandle::stop`] was honoured. Nothing follows.
    Disconnected,
    /// A probe, snapshot or event stream failed. The watcher keeps
    /// retrying with backoff; a later `Connected` supersedes this.
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerEvent {
    pub connection_id: String,
    pub kind: ContainerEventKind,
}

/// What wakes the watcher thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// A state-changing line arrived on the event stream.
    Change,
    /// The user asked for a snapshot now.
    Refresh,
    /// The event stream hit EOF — the child died or the daemon went away.
    SourceEnded,
    Stop,
}

/// Why [`debounce_loop`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopExit {
    Stopped,
    SourceEnded,
}

/// Block on `signals`, calling `on_snapshot` once per burst of `Change`s
/// (after `debounce` of quiet) and once per `Refresh`. A closed channel
/// counts as `Stop`.
pub fn debounce_loop(
    signals: &Receiver<Signal>,
    debounce: Duration,
    mut on_snapshot: impl FnMut(),
) -> LoopExit {
    loop {
        match signals.recv() {
            Err(_) | Ok(Signal::Stop) => return LoopExit::Stopped,
            Ok(Signal::SourceEnded) => return LoopExit::SourceEnded,
            Ok(Signal::Refresh) => on_snapshot(),
            Ok(Signal::Change) => loop {
                match signals.recv_timeout(debounce) {
                    Ok(Signal::Change) | Ok(Signal::Refresh) => continue,
                    Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => {
                        return LoopExit::Stopped
                    }
                    Ok(Signal::SourceEnded) => {
                        on_snapshot();
                        return LoopExit::SourceEnded;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        on_snapshot();
                        break;
                    }
                }
            },
        }
    }
}

/// Whether an `events` line describes a change the tree could show.
/// `exec_*`, `top`, `stats`, `attach`/`detach`, `resize` and the archive
/// copies fire on every Terminal/Files interaction without changing any
/// state — re-snapshotting on them would make the dock flicker while the
/// user types into a container shell. Docker calls the field `Action`,
/// Podman `Status`; an unparsable line is treated as a change (better one
/// spare snapshot than a missed one).
pub fn is_state_change(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return !line.trim().is_empty();
    };
    let action = value
        .get("Action")
        .or_else(|| value.get("Status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    const NOISE: [&str; 9] = [
        "exec",
        "top",
        "stats",
        "attach",
        "detach",
        "resize",
        "archive-path",
        "extract-to-dir",
        "copy",
    ];
    !NOISE
        .iter()
        .any(|noise| action == *noise || action.starts_with(&format!("{noise}_")))
}

/// Read `reader` to EOF, sending `Change` for each state-changing line and
/// `SourceEnded` at the end. Runs on its own thread — `BufRead::lines`
/// blocks.
pub fn pump_lines(reader: impl BufRead, signals: &Sender<Signal>) {
    for line in reader.lines().map_while(Result::ok) {
        if is_state_change(&line) && signals.send(Signal::Change).is_err() {
            return;
        }
    }
    let _ = signals.send(Signal::SourceEnded);
}

/// The caller's side of a running watcher. Cheap to clone.
#[derive(Clone)]
pub struct WatcherHandle {
    signals: Sender<Signal>,
    child: Arc<Mutex<Option<Child>>>,
}

impl WatcherHandle {
    /// Re-snapshot now. During a backoff wait this retries the connection
    /// immediately instead.
    pub fn refresh(&self) {
        let _ = self.signals.send(Signal::Refresh);
    }

    /// Stop watching. The events child is killed so its reader thread
    /// unblocks; the watcher thread emits `Disconnected` and exits.
    pub fn stop(&self) {
        let _ = self.signals.send(Signal::Stop);
        self.kill_child();
    }

    fn kill_child(&self) {
        if let Ok(mut slot) = self.child.lock() {
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn set_child(&self, child: Child) {
        if let Ok(mut slot) = self.child.lock() {
            *slot = Some(child);
        }
    }
}

/// Start watching `config` under `connection_id`, reporting through
/// `events`. Returns immediately; the thread runs until
/// [`WatcherHandle::stop`] or until `events`'s receiver is dropped.
pub fn spawn(
    connection_id: String,
    config: ConnectionConfig,
    work_dir: PathBuf,
    events: Sender<ContainerEvent>,
) -> WatcherHandle {
    let (signal_tx, signal_rx) = mpsc::channel();
    let handle = WatcherHandle {
        signals: signal_tx,
        child: Arc::new(Mutex::new(None)),
    };
    let thread_handle = handle.clone();
    std::thread::Builder::new()
        .name(format!("container-watcher/{connection_id}"))
        .spawn(move || {
            Watcher {
                connection_id,
                config,
                work_dir,
                events,
                signals: signal_rx,
                handle: thread_handle,
            }
            .run();
        })
        .expect("spawn container watcher thread");
    handle
}

struct Watcher {
    connection_id: String,
    config: ConnectionConfig,
    work_dir: PathBuf,
    events: Sender<ContainerEvent>,
    signals: Receiver<Signal>,
    handle: WatcherHandle,
}

enum Wait {
    Retry,
    Stop,
}

impl Watcher {
    /// `false` once nobody listens any more.
    fn emit(&self, kind: ContainerEventKind) -> bool {
        self.events
            .send(ContainerEvent {
                connection_id: self.connection_id.clone(),
                kind,
            })
            .is_ok()
    }

    fn run(self) {
        let engine = self.config.engine;
        let mut backoff = INITIAL_BACKOFF;
        loop {
            let session = self.connect_and_snapshot(engine);
            let invocation = match session {
                Ok(invocation) => invocation,
                Err(Wait::Stop) => break,
                Err(Wait::Retry) => {
                    if let Wait::Stop = self.wait_for_retry(backoff) {
                        break;
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };
            backoff = INITIAL_BACKOFF;

            let signal_tx = self.handle.signals.clone();
            match spawn_events_child(&invocation, engine, &self.work_dir) {
                Ok((child, stdout)) => {
                    self.handle.set_child(child);
                    std::thread::spawn(move || pump_lines(BufReader::new(stdout), &signal_tx));
                }
                Err(error) => {
                    self.emit(ContainerEventKind::Error(format!(
                        "could not follow the engine's events: {error}"
                    )));
                    if let Wait::Stop = self.wait_for_retry(backoff) {
                        break;
                    }
                    continue;
                }
            }

            let exit = debounce_loop(&self.signals, DEBOUNCE, || {
                match EngineSnapshot::fetch(&invocation, engine, &self.work_dir) {
                    Ok(snapshot) => self.emit(ContainerEventKind::Snapshot(snapshot)),
                    Err(error) => self.emit(ContainerEventKind::Error(error.to_string())),
                };
            });
            self.handle.kill_child();
            match exit {
                LoopExit::Stopped => break,
                LoopExit::SourceEnded => {
                    self.emit(ContainerEventKind::Error(
                        "the engine's event stream ended; reconnecting".to_string(),
                    ));
                    if let Wait::Stop = self.wait_for_retry(backoff) {
                        break;
                    }
                }
            }
        }
        self.emit(ContainerEventKind::Disconnected);
    }

    /// Probe, then snapshot. `Err(Wait::Retry)` after reporting the error;
    /// `Err(Wait::Stop)` when the listener is gone.
    fn connect_and_snapshot(&self, engine: Engine) -> Result<Invocation, Wait> {
        let mut invocation = self.config.invocation();
        if self.config.kind == ConnectionKind::Minikube {
            match crate::connection::minikube_docker_env(&self.work_dir) {
                Ok(env) => invocation = invocation.with_extra_env(env),
                Err(error) => return self.report(error.to_string()),
            }
        }
        let info = match probe::probe(&invocation, engine, &self.work_dir) {
            Ok(info) => info,
            Err(error) => return self.report(error.to_string()),
        };
        if !self.emit(ContainerEventKind::Connected(info)) {
            return Err(Wait::Stop);
        }
        match EngineSnapshot::fetch(&invocation, engine, &self.work_dir) {
            Ok(snapshot) => {
                if !self.emit(ContainerEventKind::Snapshot(snapshot)) {
                    return Err(Wait::Stop);
                }
            }
            Err(error) => return self.report(error.to_string()),
        }
        Ok(invocation)
    }

    fn report<T>(&self, message: String) -> Result<T, Wait> {
        if self.emit(ContainerEventKind::Error(message)) {
            Err(Wait::Retry)
        } else {
            Err(Wait::Stop)
        }
    }

    /// Sleep out a backoff, but wake early on `Refresh` (retry now) or
    /// `Stop`.
    fn wait_for_retry(&self, backoff: Duration) -> Wait {
        let deadline = std::time::Instant::now() + backoff;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.signals.recv_timeout(remaining) {
                Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => return Wait::Stop,
                Ok(Signal::Refresh) | Err(RecvTimeoutError::Timeout) => return Wait::Retry,
                Ok(Signal::Change) | Ok(Signal::SourceEnded) => continue,
            }
        }
    }
}

/// `<cli> events --format ...` as a long-running child with piped stdout.
/// Docker takes a Go template; Podman's own `json` keyword is the
/// documented spelling there.
fn spawn_events_child(
    invocation: &Invocation,
    engine: Engine,
    work_dir: &Path,
) -> std::io::Result<(Child, std::process::ChildStdout)> {
    let format = match engine {
        Engine::Docker => "{{json .}}",
        Engine::Podman => "json",
    };
    // `Invocation::command`, not a bare `Command::new`: it is the one path
    // that applies `CREATE_NO_WINDOW`, and this child lives as long as the
    // connection — a bare spawn parks a visible console on Windows for the
    // whole session, not the instant a one-shot `inspect` would.
    let mut command = invocation.command(work_dir, &["events", "--format", format]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("events child spawned without a stdout pipe"))?;
    Ok((child, stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    const FAST: Duration = Duration::from_millis(60);

    fn counting_loop(
        rx: Receiver<Signal>,
    ) -> (Arc<AtomicUsize>, std::thread::JoinHandle<LoopExit>) {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let join = std::thread::spawn(move || {
            debounce_loop(&rx, FAST, || {
                seen.fetch_add(1, Ordering::SeqCst);
            })
        });
        (count, join)
    }

    #[test]
    fn a_burst_within_the_window_collapses_into_one_snapshot() {
        let (tx, rx) = mpsc::channel();
        let (count, join) = counting_loop(rx);
        for _ in 0..25 {
            tx.send(Signal::Change).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(FAST * 3);
        tx.send(Signal::Stop).unwrap();
        assert_eq!(join.join().unwrap(), LoopExit::Stopped);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn bursts_spaced_apart_snapshot_separately() {
        let (tx, rx) = mpsc::channel();
        let (count, join) = counting_loop(rx);
        for _ in 0..3 {
            tx.send(Signal::Change).unwrap();
            tx.send(Signal::Change).unwrap();
            std::thread::sleep(FAST * 3);
        }
        tx.send(Signal::Stop).unwrap();
        join.join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn refresh_snapshots_immediately_without_waiting_for_quiet() {
        let (tx, rx) = mpsc::channel();
        let (count, join) = counting_loop(rx);
        let started = Instant::now();
        tx.send(Signal::Refresh).unwrap();
        while count.load(Ordering::SeqCst) == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "refresh never ran"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            started.elapsed() < FAST,
            "refresh must not sit out the debounce"
        );
        tx.send(Signal::Stop).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn source_ending_mid_burst_snapshots_once_then_reports_it() {
        let (tx, rx) = mpsc::channel();
        let (count, join) = counting_loop(rx);
        tx.send(Signal::Change).unwrap();
        tx.send(Signal::SourceEnded).unwrap();
        assert_eq!(join.join().unwrap(), LoopExit::SourceEnded);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn source_ending_while_quiet_returns_without_a_snapshot() {
        let (tx, rx) = mpsc::channel();
        let (count, join) = counting_loop(rx);
        tx.send(Signal::SourceEnded).unwrap();
        assert_eq!(join.join().unwrap(), LoopExit::SourceEnded);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_dropped_sender_stops_the_loop() {
        let (tx, rx) = mpsc::channel::<Signal>();
        let (count, join) = counting_loop(rx);
        drop(tx);
        assert_eq!(join.join().unwrap(), LoopExit::Stopped);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stop_during_a_burst_skips_the_pending_snapshot() {
        let (tx, rx) = mpsc::channel();
        let (count, join) = counting_loop(rx);
        tx.send(Signal::Change).unwrap();
        tx.send(Signal::Stop).unwrap();
        assert_eq!(join.join().unwrap(), LoopExit::Stopped);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn noise_actions_are_not_state_changes() {
        assert!(is_state_change(
            r#"{"Type":"container","Action":"start","Actor":{"ID":"abc"}}"#
        ));
        assert!(is_state_change(r#"{"Type":"container","Action":"die"}"#));
        assert!(is_state_change(r#"{"Type":"image","Action":"pull"}"#));
        assert!(!is_state_change(
            r#"{"Type":"container","Action":"exec_create: sh -c ls"}"#
        ));
        assert!(!is_state_change(
            r#"{"Type":"container","Action":"exec_start: sh"}"#
        ));
        assert!(!is_state_change(
            r#"{"Type":"container","Action":"exec_die"}"#
        ));
        assert!(!is_state_change(r#"{"Type":"container","Action":"top"}"#));
        assert!(!is_state_change(
            r#"{"Type":"container","Action":"resize"}"#
        ));
        // Podman spells the field `Status`.
        assert!(is_state_change(
            r#"{"Type":"container","Status":"start","Name":"web"}"#
        ));
        assert!(!is_state_change(
            r#"{"Type":"container","Status":"exec","Name":"web"}"#
        ));
        // Unparsable lines count, blank ones do not.
        assert!(is_state_change("garbage"));
        assert!(!is_state_change("   "));
    }

    #[test]
    fn fixture_event_lines_classify_as_expected() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/inspect");
        let docker = std::fs::read_to_string(root.join("docker/events.jsonl")).unwrap();
        let (changes, noise): (Vec<&str>, Vec<&str>) =
            docker.lines().partition(|line| is_state_change(line));
        assert!(changes
            .iter()
            .any(|line| line.contains("\"Action\":\"start\"")));
        assert!(
            noise
                .iter()
                .all(|line| line.contains("\"Action\":\"attach\"")),
            "{noise:?}"
        );
        assert!(!noise.is_empty(), "the capture includes an attach line");
        let podman = std::fs::read_to_string(root.join("podman/events.jsonl")).unwrap();
        let changes = podman.lines().filter(|line| is_state_change(line)).count();
        assert_eq!(changes, 3, "pull, create, start — not exec");
    }

    #[test]
    fn pump_lines_sends_one_change_per_real_line_then_source_ended() {
        let input = concat!(
            "{\"Action\":\"start\"}\n",
            "{\"Action\":\"exec_start: sh\"}\n",
            "{\"Action\":\"die\"}\n",
        );
        let (tx, rx) = mpsc::channel();
        pump_lines(Cursor::new(input), &tx);
        drop(tx);
        let received: Vec<Signal> = rx.iter().collect();
        assert_eq!(
            received,
            vec![Signal::Change, Signal::Change, Signal::SourceEnded]
        );
    }

    /// `sh` standing in for the CLI: the script echoes its argv and one
    /// environment variable, so this proves the child receives the
    /// connection's prefix, the `events` argv and its `env` — the three
    /// things `spawn_events_child` is responsible for handing over.
    #[cfg(unix)]
    #[test]
    fn events_child_runs_the_invocation_with_its_argv_and_env() {
        use std::io::Read;
        let invocation = Invocation {
            program: "sh".to_string(),
            prefix_args: vec![
                "-c".to_string(),
                "printf '%s\\n' \"$STUB_MARK\" \"$@\"".to_string(),
                "events-stub".to_string(),
            ],
            env: vec![("STUB_MARK".to_string(), "marked".to_string())],
            host: process_exec::host::ExecHost::Local,
        };
        let (mut child, mut stdout) =
            spawn_events_child(&invocation, Engine::Docker, &std::env::temp_dir())
                .expect("sh spawns");
        let mut output = String::new();
        stdout.read_to_string(&mut output).expect("stdout is piped");
        assert!(child.wait().expect("child exits").success());
        assert_eq!(
            output.lines().collect::<Vec<_>>(),
            ["marked", "events", "--format", "{{json .}}"]
        );
    }

    #[test]
    fn a_watcher_against_a_missing_cli_reports_an_error_and_stops_cleanly() {
        let (tx, rx) = mpsc::channel();
        let config = ConnectionConfig {
            engine: Engine::Docker,
            kind: ConnectionKind::Auto,
            executable: Some("definitely-not-a-container-cli-xyz".to_string()),
            compose_executable: None,
        };
        let handle = spawn("c1".to_string(), config, std::env::temp_dir(), tx);
        let first = rx.recv_timeout(Duration::from_secs(10)).expect("an event");
        assert_eq!(first.connection_id, "c1");
        assert!(
            matches!(first.kind, ContainerEventKind::Error(ref message) if message.contains("not found")),
            "{first:?}"
        );
        handle.stop();
        let rest: Vec<ContainerEvent> = rx.iter().collect();
        assert_eq!(
            rest.last().map(|event| &event.kind),
            Some(&ContainerEventKind::Disconnected)
        );
    }

    #[test]
    fn refresh_during_backoff_retries_at_once() {
        let (tx, rx) = mpsc::channel();
        let config = ConnectionConfig {
            engine: Engine::Podman,
            kind: ConnectionKind::Auto,
            executable: Some("definitely-not-a-container-cli-xyz".to_string()),
            compose_executable: None,
        };
        let handle = spawn("c2".to_string(), config, std::env::temp_dir(), tx);
        rx.recv_timeout(Duration::from_secs(10))
            .expect("first error");
        let started = Instant::now();
        handle.refresh();
        let second = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("second error");
        assert!(matches!(second.kind, ContainerEventKind::Error(_)));
        assert!(
            started.elapsed() < INITIAL_BACKOFF,
            "refresh should preempt the {INITIAL_BACKOFF:?} backoff"
        );
        handle.stop();
        let _: Vec<ContainerEvent> = rx.iter().collect();
    }
}
