//! Run configurations and console (F4-9): `RunService`, the `Threading`
//! QObject that owns one `run_core::Supervisor`.
//!
//! # Threading
//!
//! One worker thread owns the `Supervisor` — a launch, a `kill_tree()`, and
//! reading a batcher's pending bytes must not run on the UI thread, the same
//! reason `VcsService` owns its `Repository` on a worker thread
//! (`crate::bridge::vcs`). That alone would only get one console read at a
//! time, though: `Supervisor::read_output` needs a chunk the caller has
//! already read, and `PtySession::read` blocks until bytes arrive, so a
//! second running console would sit unread for as long as the first one's
//! shell stays quiet.
//!
//! So each launch also spawns a **dedicated reader thread for that console**
//! (`docs/architecture/next-five-features-plan.md`'s "the same \[shape\],
//! plus one reader thread per console") that owns the raw PTY read half —
//! obtained via `Supervisor::take_reader`, exactly `PtySession::take_reader`
//! passed one level up — and does nothing but block on `read()` in a loop.
//! Each chunk it reads is sent as a new job into the **same** channel the Qt
//! thread's `run`/`stop`/`detectConfigurations` calls already use, so every
//! operation against the `Supervisor` — including feeding it a chunk through
//! `read_output` — is still serialized through the one worker thread that
//! owns it exclusively. `run_core::Supervisor`'s own doc comment asks for
//! exactly this: "single-threaded... expects its caller to serialize
//! access". Concurrency lives in *how many threads produce jobs*, never in
//! shared mutable access to the `Supervisor` itself.
//!
//! `pty_core::PtySession`'s fields are all `Send` trait objects, so
//! `run_core::Supervisor` (a `BTreeMap` of PTY sessions, batchers and
//! strings) is `Send` as a whole — confirmed by the compiler accepting the
//! `move` into the worker thread's closure below, not asserted separately.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::mpsc::Sender;

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use run_core::{AnsiResolver, RunConfigExt};

use crate::bridge::errors;
use crate::bridge::ffi;

/// `FfiContainerOptions` <-> `RunConfig`'s container-kind sub-tables (C5,
/// ADR-0056): structured, no JSON.
mod container_form;
mod editor;
pub use editor::RunConfigEditorRust;

/// One unit of work for the worker thread that owns the `Supervisor`.
/// Mirrors `VcsJob` (`crate::bridge::vcs`): every call against the
/// `Supervisor` — a launch, a stop, or a console's reader thread handing
/// over a freshly read chunk — is one closure through this queue, so nothing
/// ever touches the `Supervisor` from two threads at once.
type RunJob = Box<dyn FnOnce(&mut RunWorker) + Send>;

/// What the worker thread owns.
struct RunWorker {
    supervisor: run_core::Supervisor,
    /// Kept so a job (the launch job, specifically) can hand a clone to a
    /// new console's reader thread — the one thing a reader thread needs to
    /// feed bytes back into this same queue.
    jobs_tx: Sender<RunJob>,
}

/// What the Qt thread remembers about one active console: enough to answer
/// `resolveLink` with no worker round trip, and to know which configuration
/// `rerun` should relaunch. `output` mirrors what `consoleOutput` has
/// already emitted (not a second copy of `run-core`'s own ring buffer,
/// which is private to `Supervisor`) and is capped at the same
/// `run_core::batching::MAX_RING_BYTES` bound for the same reason.
struct ConsoleState {
    config_id: String,
    cwd: PathBuf,
    output: String,
    /// Escape sequences are resolved here, once: `output` (what
    /// `resolveLink` byte-offsets index into) and what `consoleOutput`
    /// sends the view are then the same visible text, so link resolution
    /// stays correct even though the stream it is computed from is not the
    /// raw one `run-core` batched.
    ///
    /// The styling that resolution recovers rides beside the text in
    /// `last_runs` rather than being thrown away (R2-1).
    ansi: AnsiResolver,
    /// The styled runs covering the text of the most recent
    /// `consoleOutput` signal, in UTF-16 units, waiting for the view's
    /// `consoleStyleRuns` call. One chunk deep on purpose: a run is only
    /// ever needed by the slot handling the signal that produced it, and
    /// the document the view appends to keeps the formatting afterwards.
    last_runs: Vec<ffi::FfiStyledRun>,
    /// Set once, by whichever of `finish_console`/`stop` first observes this
    /// console has exited, so the other does not also emit `consoleFinished`
    /// (see both call sites). The entry itself is kept around afterwards,
    /// not removed: `resolveLink` must still answer for a finished console's
    /// scrollback — F4-11's dock deliberately leaves a finished console's
    /// tab open for exactly that review — so the cache it reads from has to
    /// outlive the process it was collected from.
    finished: bool,
}

/// Rust side of the `RunService` QObject.
#[derive(Default)]
pub struct RunServiceRust {
    jobs: RefCell<Option<Sender<RunJob>>>,
    consoles: RefCell<std::collections::HashMap<u64, ConsoleState>>,
}

/// The project root `AppSession` currently has open, or `None` with no
/// project open. `RunService` needs no `openProject` of its own the way
/// `VcsService` does: `Supervisor` has no per-project state to reset (a
/// launched console is just a process, indifferent to which project is
/// open), so the root is read fresh from the one shared session on every
/// call that needs it — the same handle `ProjectTreeModel`/`DocumentManager`
/// already share (`crate::bridge::registry`).
fn current_project_root() -> Option<PathBuf> {
    crate::bridge::convert::current_project_root()
}

/// `err`, as the typed code + message that crosses the FFI seam (ADR-0003).
/// `run-core`'s own codes (800-899, `error.rs`) are already stable, so this
/// is a field copy, not a translation.
fn to_ffi_result(err: &run_core::RunError) -> ffi::FfiResult {
    ffi::FfiResult {
        code: err.code(),
        message: QString::from(err.to_string().as_str()),
    }
}

fn env_to_string(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The inverse of [`env_to_string`]. A line with no `=` is dropped rather
/// than guessed at — better an env var silently missing from the draft than
/// one invented with an empty value from a stray line.
fn env_from_string(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            line.split_once('=')
                .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

/// `\n`-separated text into a trimmed, non-empty-lines `Vec<String>` — the
/// same convention `before_launch`/compose-files fields already use.
fn lines_of(text: &QString) -> Vec<String> {
    text.to_string()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn no_project() -> ffi::FfiResult {
    ffi::FfiResult {
        code: errors::CODE_NO_PROJECT,
        message: QString::from("no project is open"),
    }
}

fn unknown_run_config(message: &str) -> ffi::FfiResult {
    ffi::FfiResult {
        code: errors::CODE_UNKNOWN_RUN_CONFIG,
        message: QString::from(message),
    }
}

/// The before-launch list as the dialog shows and accepts it: one task per
/// line. Parsing it back is `tasks_from_string`; both live here rather than
/// in `run-core` because the *text* is a dialog affordance, not a rule.
fn tasks_to_string(config: &run_core::RunConfig) -> String {
    run_core::before_launch::tasks_of(config)
        .iter()
        .map(|task| match task {
            run_core::BeforeLaunchTask::Build => "build".to_string(),
            run_core::BeforeLaunchTask::RunConfiguration(id) => format!("run {id}"),
            run_core::BeforeLaunchTask::ExternalTool { program, args } => {
                let mut line = format!("tool {program}");
                for arg in args {
                    line.push(' ');
                    line.push_str(arg);
                }
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The inverse. A line this version cannot read is dropped rather than
/// guessed at, the same rule the persisted form follows.
fn tasks_from_string(text: &str) -> Vec<app_config::BeforeLaunchSetting> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let task = match (words.next()?, words) {
                ("build", _) => run_core::BeforeLaunchTask::Build,
                ("run", mut rest) => {
                    run_core::BeforeLaunchTask::RunConfiguration(rest.next()?.to_string())
                }
                ("tool", mut rest) => run_core::BeforeLaunchTask::ExternalTool {
                    program: rest.next()?.to_string(),
                    args: rest.map(str::to_string).collect(),
                },
                _ => return None,
            };
            Some(task.to_setting())
        })
        .collect()
}

fn to_ffi_run_config(config: &run_core::RunConfig) -> ffi::FfiRunConfig {
    ffi::FfiRunConfig {
        id: QString::from(config.id.as_str()),
        name: QString::from(config.name.as_str()),
        program: QString::from(config.program.as_str()),
        args: QString::from(config.args.join(" ").as_str()),
        cwd: QString::from(config.cwd.clone().unwrap_or_default().as_str()),
        env: QString::from(env_to_string(&config.env).as_str()),
        toolchain: QString::from(config.toolchain.clone().unwrap_or_default().as_str()),
        target: QString::from(config.target.clone().unwrap_or_default().as_str()),
        temporary: config.temporary,
        allow_parallel: config.allow_parallel,
        before_launch: QString::from(tasks_to_string(config).as_str()),
        kind: QString::from(config.kind.clone().unwrap_or_default().as_str()),
        container: container_form::to_ffi_options(config),
    }
}

/// The `[containers]` section actually in force — the global layer with the
/// open project's override applied (ADR-0022), the same rule every other
/// project-scoped setting resolves through
/// (`crate::bridge::convert::load_resolved_settings`). Used wherever a
/// container-kind configuration's `connection_id` needs resolving: launching
/// it, previewing its command, and its Services picker.
pub(super) fn effective_container_settings() -> app_config::ContainerSettings {
    crate::bridge::convert::load_resolved_settings().containers
}

/// Trim `output` down to `max_bytes` from the front, on a UTF-8 char
/// boundary — never a raw byte cut, which could split a multi-byte
/// character and produce invalid UTF-8 in a `String`.
/// Drop the oldest cached output once it passes `max_bytes`, returning how
/// many **UTF-16 code units** went with it.
///
/// The count is the point. `resolveLink` and `findInConsole` answer in
/// offsets into this string, and the view resolves those against its own
/// document — so the two must trim the same text at the same moment. They
/// used not to: this cache dropped bytes at 16 MiB while the widget dropped
/// blocks at five thousand, and after either fired a Ctrl+Click landed on
/// whatever text had moved into that offset. The view no longer trims on
/// its own; it mirrors this, through `consoleTrimmed`.
fn trim_cached_output(output: &mut String, max_bytes: usize) -> usize {
    if output.len() <= max_bytes {
        return 0;
    }
    let mut boundary = output.len() - max_bytes;
    while boundary < output.len() && !output.is_char_boundary(boundary) {
        boundary += 1;
    }
    let dropped = output[..boundary].encode_utf16().count();
    output.drain(..boundary);
    dropped
}

/// What a console shows where `run-core`'s ring dropped its oldest lines.
///
/// Lives here rather than in `run_console_panel.cpp` because it is part of
/// the console's text: everything the view displays has to be in the cache
/// the offsets are measured against (see [`trim_cached_output`]).
const TRUNCATION_NOTICE: &str = "\n--- output truncated: earlier lines were dropped ---\n";

/// One chunk's styled runs, in the units the view counts in.
///
/// `run_core` measures a run in bytes of UTF-8; `QTextCursor` moves in
/// UTF-16 code units, so the offsets are converted here — at the seam that
/// owns the difference — rather than in either the domain crate or the
/// view. A default-styled run is dropped: the console's own palette is
/// already what an unformatted range paints with, so sending it would only
/// ask C++ to apply an empty format.
fn to_ffi_runs(styled: &run_core::StyledText) -> Vec<ffi::FfiStyledRun> {
    let mut runs = Vec::new();
    let mut utf16_start = 0usize;
    let mut byte_cursor = 0usize;

    for run in &styled.runs {
        // Runs arrive in order and cover the text end to end, so the gap
        // between the last run's end and this one's start (if any) is
        // plain text whose UTF-16 length still has to be counted.
        utf16_start += styled.text[byte_cursor..run.start].encode_utf16().count();
        let length = styled.text[run.start..run.start + run.len]
            .encode_utf16()
            .count();
        byte_cursor = run.start + run.len;

        if !run.style.is_default() {
            let fg = run.style.fg.unwrap_or_default();
            let bg = run.style.bg.unwrap_or_default();
            runs.push(ffi::FfiStyledRun {
                start: utf16_start as u32,
                length: length as u32,
                has_fg: run.style.fg.is_some(),
                fg_r: fg.r,
                fg_g: fg.g,
                fg_b: fg.b,
                has_bg: run.style.bg.is_some(),
                bg_r: bg.r,
                bg_g: bg.g,
                bg_b: bg.b,
                bold: run.style.attrs.bold,
                italic: run.style.attrs.italic,
                underline: run.style.attrs.underline,
                inverse: run.style.attrs.inverse,
            });
        }
        utf16_start += length;
    }
    runs
}

/// Queue every event a batch produced onto the Qt thread: text as
/// `consoleOutput` (appended to this console's cached output for
/// `resolveLink`), a dropped-history notice as one more line of that same
/// output.
fn emit_events(
    console_id: u64,
    events: Vec<run_core::BatchedOutput>,
    qt_thread: &CxxQtThread<ffi::RunService>,
) {
    for event in events {
        match event {
            run_core::BatchedOutput::Output(text) => {
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
                    let (visible, trimmed) = {
                        let mut consoles = service.consoles.borrow_mut();
                        let Some(state) = consoles.get_mut(&console_id) else {
                            return;
                        };
                        let styled = state.ansi.feed(&text);
                        state.last_runs = to_ffi_runs(&styled);
                        state.output.push_str(&styled.text);
                        let trimmed = trim_cached_output(
                            &mut state.output,
                            run_core::batching::MAX_RING_BYTES,
                        );
                        (styled.text, trimmed)
                    };
                    service
                        .as_mut()
                        .console_output(console_id, QString::from(visible.as_str()));
                    // After the append, never before: the view has just
                    // added this chunk, and trimming its front is what
                    // keeps its document the same text this cache holds.
                    if trimmed > 0 {
                        service.as_mut().console_trimmed(console_id, trimmed as u32);
                    }
                });
            }
            run_core::BatchedOutput::Truncated => {
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
                    // Cached as well as emitted. Text the view shows but
                    // this cache does not hold shifts every later offset
                    // `resolveLink` and `findInConsole` answer with, so the
                    // notice is part of the console's text rather than
                    // something C++ inserts alongside it.
                    {
                        let mut consoles = service.consoles.borrow_mut();
                        let Some(state) = consoles.get_mut(&console_id) else {
                            return;
                        };
                        state.last_runs.clear();
                        state.output.push_str(TRUNCATION_NOTICE);
                    }
                    service
                        .as_mut()
                        .console_output(console_id, QString::from(TRUNCATION_NOTICE));
                });
            }
        }
    }
}

/// Spawn the dedicated reader thread for a freshly launched console: block
/// on `read()` in a loop, feeding every chunk back into the job queue as a
/// `read_output` call, until EOF (the process exited) or the channel is
/// gone (the app is shutting down).
/// Run one configuration's before-launch tasks, in order, stopping at the
/// first failure. Returns whether the launch may proceed.
///
/// Every task is a process run to completion, which is exactly what
/// `build_core::runner` does — including a Build task, whose steps come from
/// the same `BuildSpec` the Build dock uses. There is no second way to run a
/// build in this codebase (ADR-0040).
fn run_before_launch(
    tasks: &[run_core::BeforeLaunchTask],
    config_id: &str,
    root: &Path,
    configs: &[run_core::RunConfig],
    qt_thread: &CxxQtThread<ffi::RunService>,
) -> bool {
    for task in tasks {
        let (label, steps, toolchain) = match resolve_task(task, root, configs) {
            Ok(resolved) => resolved,
            Err(message) => {
                fail_before_launch(qt_thread, config_id, &message);
                return false;
            }
        };

        let started_id = config_id.to_string();
        let started_label = label.clone();
        let started_thread = qt_thread.clone();
        let _ = started_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
            service.as_mut().before_launch_started(
                QString::from(started_id.as_str()),
                QString::from(started_label.as_str()),
            );
        });

        let mut sink = BeforeLaunchSink {
            config_id: config_id.to_string(),
            qt_thread: qt_thread.clone(),
        };
        let handle = build_core::BuildHandle::new();
        let outcome = build_core::runner::run(&handle, &steps, toolchain, root, &mut sink);
        if !outcome.succeeded() {
            fail_before_launch(
                qt_thread,
                config_id,
                &format!("{label} failed (exit {})", outcome.exit_code),
            );
            return false;
        }
    }
    true
}

/// What one task actually runs: a label for the dock, the steps, and the
/// toolchain whose diagnostics the output is read as.
type ResolvedTask = (String, Vec<run_core::LaunchSpec>, run_core::ToolchainId);

fn resolve_task(
    task: &run_core::BeforeLaunchTask,
    root: &Path,
    configs: &[run_core::RunConfig],
) -> Result<ResolvedTask, String> {
    match task {
        run_core::BeforeLaunchTask::Build => {
            let toolchain = build_core::buildable_toolchain(root).map_err(|err| err.to_string())?;
            let steps = build_core::BuildSpec::new(toolchain, build_core::BuildKind::Build, root)
                .steps()
                .map_err(|err| err.to_string())?;
            Ok(("Build".to_string(), steps, toolchain))
        }
        run_core::BeforeLaunchTask::RunConfiguration(id) => {
            let config = configs
                .iter()
                .find(|c| &c.id == id)
                .ok_or_else(|| format!("unknown run configuration \"{id}\""))?;
            let mut spec = config.to_launch_spec(root);
            spec.cwd = Some(spec.cwd.clone().unwrap_or_else(|| root.to_path_buf()));
            Ok((config.name.clone(), vec![spec], tool_for_output(root)))
        }
        run_core::BeforeLaunchTask::ExternalTool { program, args } => {
            let spec = run_core::LaunchSpec {
                program: program.clone(),
                args: args.clone(),
                cwd: Some(root.to_path_buf()),
                env: Vec::new(),
                console: run_core::ConsoleKind::Pty,
            };
            Ok((program.clone(), vec![spec], tool_for_output(root)))
        }
    }
}

/// Which parser a non-build task's output is read with. The project's own
/// toolchain, so a script that runs the compiler still produces recognisable
/// diagnostics; `Make` (the text table) when the project has no build tool,
/// because guessing Cargo's JSON for arbitrary output would find nothing.
fn tool_for_output(root: &Path) -> run_core::ToolchainId {
    build_core::buildable_toolchain(root).unwrap_or(run_core::ToolchainId::Make)
}

fn fail_before_launch(qt_thread: &CxxQtThread<ffi::RunService>, config_id: &str, message: &str) {
    let config_id = config_id.to_string();
    let message = message.to_string();
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
        service.as_mut().before_launch_failed(
            QString::from(config_id.as_str()),
            ffi::FfiResult {
                code: errors::CODE_BEFORE_LAUNCH,
                message: QString::from(message.as_str()),
            },
        );
    });
}

/// Streams a before-launch task's output to the Build dock. Diagnostics are
/// deliberately dropped: the Problems dock is fed by `BuildService`, and a
/// second writer to it from here would leave rows nothing clears. The
/// errors themselves are still visible — they are in the output.
struct BeforeLaunchSink {
    config_id: String,
    qt_thread: CxxQtThread<ffi::RunService>,
}

impl build_core::BuildSink for BeforeLaunchSink {
    fn output(&mut self, text: &str) {
        let config_id = self.config_id.clone();
        let text = text.to_string();
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::RunService>| {
                service.as_mut().before_launch_output(
                    QString::from(config_id.as_str()),
                    QString::from(text.as_str()),
                );
            });
    }

    fn diagnostics(&mut self, _diagnostics: Vec<build_core::BuildDiagnostic>) {}
}

/// Actually launch `spec` on the worker thread and start tracking it as a
/// console, under `config_id` — the common tail of a full `launch()` (after
/// its before-launch tasks pass) and [`ffi::RunService::spawn_ad_hoc_console`]
/// (which has none to run).
fn spawn_console_on_worker(
    worker: &mut RunWorker,
    config_id: String,
    spec: run_core::LaunchSpec,
    qt_thread: &CxxQtThread<ffi::RunService>,
) {
    let cwd = spec.cwd.clone().unwrap_or_default();
    match worker.supervisor.launch(config_id.clone(), &spec) {
        Ok(id) => {
            let reader = worker.supervisor.take_reader(id);
            let console_id = id.0;
            let started_config_id = config_id.clone();
            let qt_thread_for_started = qt_thread.clone();
            let _ = qt_thread_for_started.queue(move |mut service: Pin<&mut ffi::RunService>| {
                service.consoles.borrow_mut().insert(
                    console_id,
                    ConsoleState {
                        config_id: started_config_id.clone(),
                        cwd,
                        output: String::new(),
                        ansi: AnsiResolver::default(),
                        last_runs: Vec::new(),
                        finished: false,
                    },
                );
                service
                    .as_mut()
                    .console_started(console_id, QString::from(started_config_id.as_str()));
            });
            if let Ok(reader) = reader {
                spawn_reader_thread(
                    console_id,
                    reader,
                    worker.jobs_tx.clone(),
                    qt_thread.clone(),
                );
            }
        }
        Err(err) => {
            let result = to_ffi_result(&err);
            let qt_thread = qt_thread.clone();
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
                service
                    .as_mut()
                    .run_failed(QString::from(config_id.as_str()), result);
            });
        }
    }
}

fn spawn_reader_thread(
    console_id: u64,
    mut reader: Box<dyn std::io::Read + Send>,
    jobs_tx: Sender<RunJob>,
    qt_thread: CxxQtThread<ffi::RunService>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let qt_thread = qt_thread.clone();
                    let _ = jobs_tx.send(Box::new(move |worker: &mut RunWorker| {
                        finish_console(worker, run_core::ConsoleId(console_id), &qt_thread);
                    }));
                    break;
                }
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    let qt_thread = qt_thread.clone();
                    let sent = jobs_tx.send(Box::new(move |worker: &mut RunWorker| {
                        let now = std::time::Instant::now();
                        if let Ok(events) = worker.supervisor.read_output(
                            run_core::ConsoleId(console_id),
                            &chunk,
                            now,
                        ) {
                            emit_events(console_id, events, &qt_thread);
                        }
                    }));
                    if sent.is_err() {
                        break; // The worker thread is gone (app shutdown).
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// Kill `id`'s tree and report it finished, once — the second half of
/// `stop`'s escalation and the whole of `kill`.
fn hard_stop(
    worker: &mut RunWorker,
    id: run_core::ConsoleId,
    qt_thread: &CxxQtThread<ffi::RunService>,
) {
    let Ok(outcome) = worker.supervisor.stop(id) else {
        return;
    };
    let escaped = matches!(outcome, pty_core::KillOutcome::Escaped);
    let console_id = id.0;
    let qt_thread = qt_thread.clone();
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
        if mark_finished(&mut service.consoles.borrow_mut(), console_id) {
            service.as_mut().console_finished(console_id, -1, escaped);
        }
    });
}

/// A console's process exited on its own (its reader thread hit EOF): flush
/// whatever output was still pending, best-effort recover its exit code,
/// reap it via `Supervisor::stop` (a no-op kill on an already-dead process
/// — see `pty_core::platform::kill_tree`'s `ESRCH` case — so this is really
/// just "stop tracking it"), and report it finished.
fn finish_console(
    worker: &mut RunWorker,
    id: run_core::ConsoleId,
    qt_thread: &CxxQtThread<ffi::RunService>,
) {
    let now = std::time::Instant::now();
    if let Ok(events) = worker.supervisor.flush_remaining(id, now) {
        emit_events(id.0, events, qt_thread);
    }

    // The exit code is usually available the instant EOF is seen, but not
    // guaranteed to be — give the OS a brief moment to reap the child.
    let mut exit_code = None;
    for _ in 0..20 {
        match worker.supervisor.exit_code(id) {
            Ok(Some(code)) => {
                exit_code = Some(code);
                break;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(5)),
            Err(_) => break,
        }
    }

    let escaped = matches!(
        worker.supervisor.stop(id),
        Ok(pty_core::KillOutcome::Escaped)
    );
    let exit_code = exit_code.map_or(-1, |code| code as i32);
    let console_id = id.0;
    let qt_thread = qt_thread.clone();
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
        // The console may already be marked finished if an explicit
        // `stop()` won the race with this natural-exit path — both call
        // `Supervisor::stop`, and only the first to run here should emit.
        // The entry itself is never removed (see `ConsoleState::finished`'s
        // doc comment): `resolveLink` still has to answer for it.
        if mark_finished(&mut service.consoles.borrow_mut(), console_id) {
            service
                .as_mut()
                .console_finished(console_id, exit_code, escaped);
        }
    });
}

/// Marks `console_id` finished and returns whether this call was the one
/// that did it — `false` if it was already finished (or is unknown), so a
/// caller emits `consoleFinished` at most once per console. Shared by
/// `finish_console` and `RunService::stop`, the two paths that can each
/// observe the same exit.
fn mark_finished(
    consoles: &mut std::collections::HashMap<u64, ConsoleState>,
    console_id: u64,
) -> bool {
    match consoles.get_mut(&console_id) {
        Some(state) if !state.finished => {
            state.finished = true;
            true
        }
        _ => false,
    }
}

impl ffi::RunService {
    /// The `Sender` jobs are pushed through, starting the worker thread on
    /// first use — there is no per-project reset to hang this on (see this
    /// module's doc comment), so lazy start is simpler than an `openProject`
    /// nothing else needs.
    fn ensure_worker(self: Pin<&mut Self>) -> Sender<RunJob> {
        if let Some(tx) = self.jobs.borrow().clone() {
            return tx;
        }
        let (tx, rx) = std::sync::mpsc::channel::<RunJob>();
        let mut worker = RunWorker {
            supervisor: run_core::Supervisor::new(),
            jobs_tx: tx.clone(),
        };
        std::thread::spawn(move || {
            for job in rx {
                job(&mut worker);
            }
        });
        *self.jobs.borrow_mut() = Some(tx.clone());
        tx
    }

    pub fn configurations(&self) -> Vec<ffi::FfiRunConfig> {
        let Some(root) = current_project_root() else {
            return Vec::new();
        };
        app_config::project_settings::load(&root)
            .unwrap_or_default()
            .run_configs
            .unwrap_or_default()
            .iter()
            .map(to_ffi_run_config)
            .collect()
    }

    pub fn detect_configurations(mut self: Pin<&mut Self>) {
        let Some(root) = current_project_root() else {
            return;
        };
        let tx = self.as_mut().ensure_worker();
        let qt_thread = self.as_mut().qt_thread();
        let _ = tx.send(Box::new(move |_worker: &mut RunWorker| {
            let result = app_config::project_settings::update(&root, |settings| {
                let existing = settings.run_configs.clone().unwrap_or_default();
                let detected = run_core::detect(&root);
                settings.run_configs = Some(run_core::merge_detected(&existing, detected));
            });
            if result.is_ok() {
                let _ = qt_thread.queue(|mut service: Pin<&mut ffi::RunService>| {
                    service.as_mut().configurations_changed();
                });
            }
        }));
    }

    pub fn run(mut self: Pin<&mut Self>, config_id: &QString) -> ffi::FfiResult {
        let config_id = config_id.to_string();
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let configs = app_config::project_settings::load(&root)
            .unwrap_or_default()
            .run_configs
            .unwrap_or_default();
        let Some(config) = configs.into_iter().find(|c| c.id == config_id) else {
            return unknown_run_config("unknown run configuration");
        };

        let context = run_core::MacroContext::for_project(&root);
        self.as_mut().launch(config, &root, &context)
    }

    pub fn can_run_file(&self, path: &QString) -> bool {
        let Some(root) = current_project_root() else {
            return false;
        };
        run_core::config_for_file(&root, Path::new(&path.to_string())).is_some()
    }

    pub fn run_context(mut self: Pin<&mut Self>, path: &QString) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let Some(config) = run_core::config_for_file(&root, &file) else {
            return unknown_run_config("this file has no run target");
        };

        // Persist before launching: the toolbar's combo, the console tab and
        // a later rerun all key on a configuration the settings file knows
        // about, so a temporary one that never reached disk would run once
        // and then be unrerunnable.
        let remembered = {
            let config = config.clone();
            app_config::project_settings::update(&root, move |settings| {
                let mut configs = settings.run_configs.clone().unwrap_or_default();
                run_core::remember_temporary(&mut configs, config.clone());
                settings.run_configs = Some(configs);
            })
        };
        if remembered.is_ok() {
            self.as_mut().configurations_changed();
        }

        let context = run_core::MacroContext::for_file(&root, &file);
        self.as_mut().launch(config, &root, &context)
    }

    /// Whether `path`'s gutter should show the Dockerfile/Containerfile
    /// popup (C5, ADR-0056) — `syntax_core`'s own `dockerfile` language
    /// entry already covers `Dockerfile`, `Containerfile`, `*.dockerfile`,
    /// `*.containerfile` and `Dockerfile.<stage>` (ADR-0018: one detection
    /// table), so this reuses it rather than a second file-name rule.
    pub fn can_run_containerfile(&self, path: &QString) -> bool {
        syntax_core::language_for_path(Path::new(&path.to_string())).id() == "dockerfile"
    }

    /// Whether `path`'s gutter should show the compose popup — a file-name
    /// rule (compose is a YAML *flavor*, not a language, ADR-0018), the
    /// same one [`crate::bridge::run::detect_compose`] would suggest it
    /// from.
    pub fn can_run_compose_file(&self, path: &QString) -> bool {
        run_core::is_compose_file_name(Path::new(&path.to_string()))
    }

    /// The Dockerfile gutter's "Build image": `docker build` only, as a
    /// tracked console — no run configuration created or remembered, since
    /// this is a one-off build, not something to reappear in the run
    /// toolbar's picker.
    pub fn build_containerfile(mut self: Pin<&mut Self>, path: &QString) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let Some(mut config) = run_core::containerfile_config(&root, &file) else {
            return unknown_run_config("this file is outside the project");
        };
        if let Some(setting) = config.containerfile.as_mut() {
            setting.run_built_image = false; // build only — see `to_launch_spec_in`'s dispatch.
        }
        let containers = effective_container_settings();
        let context = run_core::MacroContext::for_file(&root, &file).with_containers(containers);
        let spec = {
            use run_core::RunConfigExt as _;
            config.to_launch_spec_in(&context)
        };
        self.as_mut()
            .spawn_ad_hoc_console(format!("{}-build", config.id), spec);
        ffi::FfiResult::default()
    }

    /// The Dockerfile gutter's "Run container": build then run, remembered
    /// as a temporary configuration (so it also appears in the run
    /// toolbar's picker and can be rerun), same as [`Self::run_context`].
    pub fn run_containerfile(mut self: Pin<&mut Self>, path: &QString) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let Some(config) = run_core::containerfile_config(&root, &file) else {
            return unknown_run_config("this file is outside the project");
        };
        self.as_mut().remember_and_launch(config, &root, &file)
    }

    /// The Dockerfile gutter's "New configuration...": remembers the
    /// temporary configuration without launching it, returning its id so
    /// the caller (`containers_panel.cpp`/the gutter popup) can open the
    /// run-config dialog already pointed at it.
    pub fn new_containerfile_configuration(mut self: Pin<&mut Self>, path: &QString) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let file = PathBuf::from(path.to_string());
        let Some(config) = run_core::containerfile_config(&root, &file) else {
            return QString::default();
        };
        self.as_mut().remember_only(config)
    }

    /// The compose file gutter's "Run": the whole file, every service,
    /// remembered as a temporary configuration and launched.
    pub fn run_compose_file(mut self: Pin<&mut Self>, path: &QString) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let file = PathBuf::from(path.to_string());
        let containers = effective_container_settings();
        let connection_id = containers
            .connections
            .first()
            .map(|c| c.id.as_str())
            .unwrap_or("");
        let Some(config) = run_core::compose_config(&root, connection_id, &file) else {
            return unknown_run_config("this file is outside the project");
        };
        self.as_mut().remember_and_launch(config, &root, &file)
    }

    /// The compose file gutter's "New configuration...".
    pub fn new_compose_file_configuration(mut self: Pin<&mut Self>, path: &QString) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let file = PathBuf::from(path.to_string());
        let containers = effective_container_settings();
        let connection_id = containers
            .connections
            .first()
            .map(|c| c.id.as_str())
            .unwrap_or("");
        let Some(config) = run_core::compose_config(&root, connection_id, &file) else {
            return QString::default();
        };
        self.as_mut().remember_only(config)
    }

    /// Remember `config` as a temporary configuration (persisting it) and
    /// launch it from `file`'s context — the shared tail of
    /// [`Self::run_containerfile`]/[`Self::run_compose_file`], mirroring
    /// [`Self::run_context`]'s own remember-then-launch shape.
    fn remember_and_launch(
        mut self: Pin<&mut Self>,
        config: run_core::RunConfig,
        root: &Path,
        file: &Path,
    ) -> ffi::FfiResult {
        let id = self.as_mut().remember_only(config.clone());
        let _ = id; // configurations_changed already emitted by remember_only.
        let context = run_core::MacroContext::for_file(root, file);
        self.as_mut().launch(config, root, &context)
    }

    /// Remember `config` as a temporary configuration without launching it,
    /// returning its id.
    fn remember_only(mut self: Pin<&mut Self>, config: run_core::RunConfig) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let id = config.id.clone();
        let result = app_config::project_settings::update(&root, move |settings| {
            let mut configs = settings.run_configs.clone().unwrap_or_default();
            run_core::remember_temporary(&mut configs, config.clone());
            settings.run_configs = Some(configs);
        });
        if result.is_ok() {
            self.as_mut().configurations_changed();
        }
        QString::from(id.as_str())
    }

    /// Launch `config`, honouring its parallel-run policy: unless the
    /// configuration allows parallel runs, its still-running consoles are
    /// stopped first, which is IntelliJ's default ("Allow multiple
    /// instances" off) and the reason a second Run does not quietly leave
    /// two servers holding the same port.
    fn launch(
        mut self: Pin<&mut Self>,
        config: run_core::RunConfig,
        root: &Path,
        context: &run_core::MacroContext,
    ) -> ffi::FfiResult {
        let config_id = config.id.clone();
        if !config.allow_parallel {
            let running: Vec<u64> = self
                .consoles
                .borrow()
                .iter()
                .filter(|(_, state)| !state.finished && state.config_id == config_id)
                .map(|(id, _)| *id)
                .collect();
            for console_id in running {
                self.as_mut().stop(console_id);
            }
        }

        let root = root.to_path_buf();
        let containers = effective_container_settings();
        let context = context.clone().with_containers(containers.clone());
        let mut spec = config.to_launch_spec_in(&context);
        let cwd = spec.cwd.clone().unwrap_or_else(|| root.clone());
        // `to_launch_spec` leaves `cwd` as `None` for a configuration with
        // no explicit working directory (`run_core::config::to_launch_spec`
        // makes no project-root assumption of its own — that is this
        // bridge's job, per its own `cwd` field doc comment). Filling it in
        // here, rather than trusting `pty_core`'s spawn to fall back to
        // something sane, is what actually launches the process in the
        // project root instead of wherever the IDE process itself happened
        // to start from.
        spec.cwd = Some(cwd.clone());

        // Before-launch tasks are validated here, on the Qt thread, before
        // anything runs: a cycle discovered halfway through would already
        // have started processes the user then has to kill one at a time
        // (B2-3).
        let configs = app_config::project_settings::load(&root)
            .unwrap_or_default()
            .run_configs
            .unwrap_or_default();
        if let Err(err) = run_core::before_launch::validate(&config_id, &configs) {
            return ffi::FfiResult {
                code: errors::CODE_BEFORE_LAUNCH,
                message: QString::from(err.to_string().as_str()),
            };
        }
        let tasks = run_core::before_launch::tasks_of_with_containers(&config, &containers);

        let tx = self.as_mut().ensure_worker();
        let qt_thread = self.qt_thread();
        let launch_config_id = config_id.clone();
        let task_root = root.clone();
        let _ = tx.send(Box::new(move |worker: &mut RunWorker| {
            // Sequential and fail-fast, on the worker thread: a run whose
            // build failed must not start, and the user finds out in the
            // Build dock rather than by reading a program's output for
            // errors that are not its own (B2-2).
            if !run_before_launch(&tasks, &launch_config_id, &task_root, &configs, &qt_thread) {
                return;
            }
            spawn_console_on_worker(worker, launch_config_id, spec, &qt_thread);
        }));
        ffi::FfiResult::default()
    }

    /// Launch `spec` as a tracked console with no before-launch tasks and no
    /// parallel-run policy — the tail of [`RunService::launch`], reused by
    /// the Containers dock's compose project actions (C5, ADR-0056: Start
    /// All/Stop/Down/Scale, and the console's own "Down") so every one of
    /// them shows up as a real console with real output, exactly like any
    /// other launch, rather than a fire-and-forget process.
    fn spawn_ad_hoc_console(
        mut self: Pin<&mut Self>,
        config_id: String,
        spec: run_core::LaunchSpec,
    ) {
        let tx = self.as_mut().ensure_worker();
        let qt_thread = self.qt_thread();
        let _ = tx.send(Box::new(move |worker: &mut RunWorker| {
            spawn_console_on_worker(worker, config_id, spec, &qt_thread);
        }));
    }

    /// Stop a console the way IntelliJ's Stop does: ask first, insist
    /// afterwards (R2-4).
    ///
    /// A TERM goes to the whole tree and the console keeps running for up
    /// to `run_core::TERMINATION_GRACE`, so a program with a signal handler
    /// gets to flush its output — which arrives through the console's own
    /// reader thread, and is why the console is not forgotten immediately.
    /// If it is still alive when the grace runs out, it is killed.
    ///
    /// The wait happens on a thread of its own rather than on the worker:
    /// the worker is also what feeds every console's output through its
    /// batcher, and blocking it for two seconds would freeze every other
    /// console's output along with this one's.
    pub fn stop(mut self: Pin<&mut Self>, console_id: u64) {
        let Some(tx) = self.jobs.borrow().clone() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        let _ = tx.send(Box::new(move |worker: &mut RunWorker| {
            let id = run_core::ConsoleId(console_id);
            let now = std::time::Instant::now();
            if let Ok(events) = worker.supervisor.flush_remaining(id, now) {
                emit_events(console_id, events, &qt_thread);
            }
            if worker.supervisor.terminate(id).is_err() {
                return;
            }

            let jobs_tx = worker.jobs_tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(run_core::TERMINATION_GRACE);
                let _ = jobs_tx.send(Box::new(move |worker: &mut RunWorker| {
                    // Gone already: the TERM was enough, and the console's
                    // own EOF path has reported it finished.
                    if worker.supervisor.is_running(id) {
                        hard_stop(worker, id, &qt_thread);
                    }
                }));
            });
        }));
    }

    /// Kill a console outright, with no grace period — the escape hatch for
    /// a process that ignores TERM (R2-4).
    pub fn kill(mut self: Pin<&mut Self>, console_id: u64) {
        let Some(tx) = self.jobs.borrow().clone() else {
            return;
        };
        let qt_thread = self.as_mut().qt_thread();
        let _ = tx.send(Box::new(move |worker: &mut RunWorker| {
            let id = run_core::ConsoleId(console_id);
            let now = std::time::Instant::now();
            if let Ok(events) = worker.supervisor.flush_remaining(id, now) {
                emit_events(console_id, events, &qt_thread);
            }
            hard_stop(worker, id, &qt_thread);
        }));
    }

    pub fn rerun(mut self: Pin<&mut Self>, console_id: u64) -> ffi::FfiResult {
        let config_id = match self.consoles.borrow().get(&console_id) {
            Some(state) => state.config_id.clone(),
            None => {
                return ffi::FfiResult {
                    code: errors::CODE_UNKNOWN_CONSOLE,
                    message: QString::from("unknown console"),
                }
            }
        };
        self.as_mut().stop(console_id);
        self.as_mut().run(&QString::from(config_id.as_str()))
    }

    fn config_for_console(&self, console_id: u64) -> Option<run_core::RunConfig> {
        let config_id = self.consoles.borrow().get(&console_id)?.config_id.clone();
        let root = current_project_root()?;
        app_config::project_settings::load(&root)
            .ok()?
            .run_configs
            .unwrap_or_default()
            .into_iter()
            .find(|c| c.id == config_id)
    }

    /// Whether `console_id`'s configuration is a compose configuration
    /// (C5, ADR-0056) — the console's "Down" button is shown only then.
    pub fn is_compose_console(&self, console_id: u64) -> bool {
        self.config_for_console(console_id)
            .is_some_and(|config| config.kind.as_deref() == Some("compose"))
    }

    /// `compose down`, with the configuration's own remove flags — the
    /// console's "Down" action for a compose configuration.
    /// `compose down` as a tracked console — a real console the run dock
    /// shows output in, not a fire-and-forget process (C5, ADR-0056 §6):
    /// `Supervisor` tracks consoles by an arbitrary label, so a second
    /// command against the same configuration is exactly the same
    /// mechanism as any other launch, via [`Self::spawn_ad_hoc_console`].
    pub fn compose_down(mut self: Pin<&mut Self>, console_id: u64) -> ffi::FfiResult {
        let Some(config) = self.config_for_console(console_id) else {
            return unknown_run_config("unknown console");
        };
        let containers = effective_container_settings();
        let Some(command) = run_core::down_command(&config, &containers) else {
            return unknown_run_config("not a compose configuration");
        };
        let root = current_project_root().unwrap_or_default();
        let spec = run_core::LaunchSpec {
            program: command.program,
            args: command.args,
            cwd: Some(root),
            env: Vec::new(),
            console: run_core::ConsoleKind::Pty,
        };
        self.as_mut()
            .spawn_ad_hoc_console(format!("{}:down", config.id), spec);
        ffi::FfiResult::default()
    }

    /// The Containers dock compose project node's "Start All": `compose up
    /// -d`, tracked as a console like any other launch.
    pub fn run_compose_project(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        files: &QString,
        project_name: &QString,
    ) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let containers = effective_container_settings();
        let files = lines_of(files);
        let spec = run_core::compose_project_up_spec(
            &containers,
            &connection_id.to_string(),
            &files,
            &project_name.to_string(),
            &root,
        );
        self.as_mut()
            .spawn_ad_hoc_console(format!("compose:{project_name}:up"), spec);
        ffi::FfiResult::default()
    }

    /// The compose project node's "Stop": `compose stop`, tracked.
    pub fn stop_compose_project(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        files: &QString,
        project_name: &QString,
    ) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let containers = effective_container_settings();
        let files = lines_of(files);
        let command = run_core::compose_project_stop_command(
            &containers,
            &connection_id.to_string(),
            &files,
            &project_name.to_string(),
        );
        let spec = run_core::LaunchSpec {
            program: command.program,
            args: command.args,
            cwd: Some(root),
            env: Vec::new(),
            console: run_core::ConsoleKind::Pty,
        };
        self.as_mut()
            .spawn_ad_hoc_console(format!("compose:{project_name}:stop"), spec);
        ffi::FfiResult::default()
    }

    /// The compose project node's "Down": `compose down`, tracked.
    pub fn down_compose_project(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        files: &QString,
        project_name: &QString,
    ) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let containers = effective_container_settings();
        let files = lines_of(files);
        let command = run_core::compose_project_down_command(
            &containers,
            &connection_id.to_string(),
            &files,
            &project_name.to_string(),
        );
        let spec = run_core::LaunchSpec {
            program: command.program,
            args: command.args,
            cwd: Some(root),
            env: Vec::new(),
            console: run_core::ConsoleKind::Pty,
        };
        self.as_mut()
            .spawn_ad_hoc_console(format!("compose:{project_name}:down"), spec);
        ffi::FfiResult::default()
    }

    /// The compose service node's "Scale...": `compose up -d --scale
    /// service=n --no-recreate`, tracked.
    pub fn scale_compose_service(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        files: &QString,
        service: &QString,
        count: u32,
    ) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let containers = effective_container_settings();
        let files = lines_of(files);
        let command = run_core::compose_project_scale_command(
            &containers,
            &connection_id.to_string(),
            &files,
            &service.to_string(),
            count,
        );
        let spec = run_core::LaunchSpec {
            program: command.program,
            args: command.args,
            cwd: Some(root),
            env: Vec::new(),
            console: run_core::ConsoleKind::Pty,
        };
        let label = format!("compose:{service}:scale");
        self.as_mut().spawn_ad_hoc_console(label, spec);
        ffi::FfiResult::default()
    }

    /// Every match of `pattern` in this console's text, in document order
    /// and in UTF-16 units — the same offsets `QTextCursor` counts in.
    ///
    /// The matcher is `editor_core::search`, the one Find in Files and the
    /// editor's own find bar already use (R2-3): a console is a buffer of
    /// text, and a second matching implementation would be a second answer
    /// to "does this pattern match here".
    pub fn find_in_console(
        &self,
        console_id: u64,
        pattern: &QString,
        case_sensitive: bool,
    ) -> Vec<ffi::FfiTextMatch> {
        let consoles = self.consoles.borrow();
        let Some(state) = consoles.get(&console_id) else {
            return Vec::new();
        };
        let options = editor_core::search::SearchOptions {
            regex: false,
            case_sensitive,
        };
        editor_core::search::find_matches(&state.output, &pattern.to_string(), options)
            .unwrap_or_default()
            .into_iter()
            .map(|found| ffi::FfiTextMatch {
                start: found.start as u32,
                end: found.end as u32,
            })
            .collect()
    }

    /// Forget a console's scrollback, so the view's document and this cache
    /// stay the same text (R2-3). The process, if any, keeps running: this
    /// clears what was printed, not what is printing.
    pub fn clear_console(&self, console_id: u64) {
        if let Some(state) = self.consoles.borrow_mut().get_mut(&console_id) {
            state.output.clear();
            state.last_runs.clear();
        }
    }

    /// Drop a finished console entirely — the tab is going away, so the
    /// scrollback `resolveLink` was keeping for it has nothing left to
    /// answer for.
    ///
    /// A console that is still running is left alone: the view stops it
    /// first and closes the tab when `consoleFinished` arrives, so that
    /// "close" never silently orphans a process is a view sequencing
    /// question, not a rule this can enforce halfway.
    pub fn close_console(&self, console_id: u64) {
        let mut consoles = self.consoles.borrow_mut();
        if consoles
            .get(&console_id)
            .is_some_and(|state| state.finished)
        {
            consoles.remove(&console_id);
        }
    }

    /// Every console this session has, newest first, with the ones still
    /// running first of all (R2-5).
    ///
    /// Read from the Qt thread's own map rather than round-tripping to
    /// `Supervisor::active_ids`: the popup opens on a click and must fill
    /// immediately, and `finished` here is set by the same code that makes
    /// the supervisor forget a console, so the two cannot disagree.
    pub fn active_consoles(&self) -> Vec<ffi::FfiRunningConsole> {
        let consoles = self.consoles.borrow();
        let mut rows: Vec<_> = consoles
            .iter()
            .map(|(id, state)| ffi::FfiRunningConsole {
                console_id: *id,
                config_id: QString::from(state.config_id.as_str()),
                running: !state.finished,
            })
            .collect();
        rows.sort_by_key(|row| (!row.running, std::cmp::Reverse(row.console_id)));
        rows
    }

    pub fn console_style_runs(&self, console_id: u64) -> Vec<ffi::FfiStyledRun> {
        self.consoles
            .borrow()
            .get(&console_id)
            .map(|state| state.last_runs.clone())
            .unwrap_or_default()
    }

    pub fn resolve_link(&self, console_id: u64, byte_offset: u32) -> ffi::FfiResolvedLink {
        let consoles = self.consoles.borrow();
        let Some(state) = consoles.get(&console_id) else {
            return ffi::FfiResolvedLink::default();
        };
        match run_core::resolve_link(&state.output, byte_offset as usize, &state.cwd) {
            Some(link) => ffi::FfiResolvedLink {
                found: true,
                path: QString::from(link.path.display().to_string().as_str()),
                line: link.line,
                has_column: link.col.is_some(),
                column: link.col.unwrap_or(0),
            },
            None => ffi::FfiResolvedLink::default(),
        }
    }
}

#[cfg(test)]
mod styled_run_tests {
    use super::to_ffi_runs;

    fn runs_of(text: &str) -> Vec<crate::bridge::ffi::FfiStyledRun> {
        to_ffi_runs(&run_core::AnsiResolver::default().feed(text))
    }

    #[test]
    fn a_colored_span_becomes_one_run_at_its_offset() {
        let runs = runs_of("plain \x1b[31mred\x1b[0m tail");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start, "plain ".len() as u32);
        assert_eq!(runs[0].length, "red".len() as u32);
        assert!(runs[0].has_fg && !runs[0].has_bg);
        assert_eq!((runs[0].fg_r, runs[0].fg_g, runs[0].fg_b), (205, 0, 0));
    }

    #[test]
    fn offsets_are_counted_in_utf16_units_not_bytes() {
        // The emoji is 4 bytes of UTF-8 but 2 UTF-16 code units, which is
        // what `QTextCursor::setPosition` counts. Getting this wrong paints
        // the format over the wrong characters, and only for non-ASCII
        // output — exactly the bug a byte offset would hide in ASCII tests.
        let runs = runs_of("\u{1F600}\x1b[32mok");
        assert_eq!(runs[0].start, 2);
        assert_eq!(runs[0].length, 2);
    }

    #[test]
    fn plain_text_produces_no_runs_at_all() {
        assert!(runs_of("nothing to style").is_empty());
    }

    #[test]
    fn attributes_survive_without_a_color() {
        let runs = runs_of("\x1b[1mbold");
        assert_eq!(runs.len(), 1);
        assert!(runs[0].bold && !runs[0].has_fg);
    }
}

#[cfg(test)]
mod trim_tests {
    use super::trim_cached_output;

    #[test]
    fn output_under_the_cap_is_left_alone() {
        let mut output = String::from("short");
        assert_eq!(trim_cached_output(&mut output, 1024), 0);
        assert_eq!(output, "short");
    }

    #[test]
    fn the_dropped_prefix_is_reported_in_utf16_units() {
        // Four bytes of emoji are two UTF-16 code units, and the view
        // deletes what this number says — counting bytes here would leave
        // its document one character longer than the cache and shift every
        // offset `resolveLink` answers with.
        let mut output = String::from("\u{1F600}tail");
        let dropped = trim_cached_output(&mut output, "tail".len());
        assert_eq!(dropped, 2);
        assert_eq!(output, "tail");
    }

    #[test]
    fn trimming_never_splits_a_character() {
        let mut output = String::from("a\u{1F600}b");
        let dropped = trim_cached_output(&mut output, 2);
        assert!(output.starts_with('b'));
        assert_eq!(dropped, output_dropped_units("a\u{1F600}"));
    }

    fn output_dropped_units(prefix: &str) -> usize {
        prefix.encode_utf16().count()
    }
}
