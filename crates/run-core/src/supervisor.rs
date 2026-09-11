//! Process supervision (F4-6): launch a [`LaunchSpec`], track every console
//! currently running, batch its output, and stop it on request.
//!
//! This is domain logic only — no threads, no channels, no cxx-qt. A future
//! `ui-shell::RunService` owns the job/event thread pair around this struct,
//! exactly the way `vcs_core::Repository` is a plain struct that
//! `VcsServiceRust` drives from behind a `Sender<VcsJob>` (see
//! `crates/ui-shell/src/bridge/vcs/mod.rs`) and `TerminalSessionRust` drives
//! a PTY reader thread around `pty_core::PtySession` (see
//! `crates/ui-shell/src/bridge/terminal.rs`). `Supervisor` itself is
//! single-threaded and expects its caller to serialize access (e.g. from one
//! job thread), the same expectation `Repository` makes.

use pty_core::{KillOutcome, PtySession, PtySize, ShellSpec};

use crate::batching::{BatchedOutput, OutputBatcher};
use crate::config::LaunchSpec;
use crate::error::RunError;

/// Identifies one launched console. Distinct per `launch()` call, even for
/// two runs of the same [`crate::RunConfig`] — that is the whole point: two
/// runs of "cargo test" must not share a console or a pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConsoleId(pub u64);

/// The default PTY size for a launched run. Run consoles do not resize
/// interactively the way a terminal does (no widget behind them yet), so
/// this is a fixed, generous size rather than a parameter nothing sets.
const DEFAULT_PTY_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 120,
};

/// One console currently tracked by the [`Supervisor`].
struct RunningConsole {
    pty_session: PtySession,
    batcher: OutputBatcher,
    #[allow(dead_code)] // read by a future RunService for display/grouping.
    config_id: String,
}

/// Tracks every console the IDE has launched and not yet reaped.
#[derive(Default)]
pub struct Supervisor {
    consoles: std::collections::BTreeMap<ConsoleId, RunningConsole>,
    next_id: u64,
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate `spec`, spawn it attached to a PTY, and start tracking it
    /// under a freshly assigned [`ConsoleId`].
    ///
    /// Validation happens before `pty-core` is ever touched, so a bad
    /// config fails with a specific reason (`InvalidConfig`/`CwdNotFound`)
    /// instead of an opaque spawn error.
    pub fn launch(
        &mut self,
        config_id: impl Into<String>,
        spec: &LaunchSpec,
    ) -> Result<ConsoleId, RunError> {
        if spec.program.trim().is_empty() {
            return Err(RunError::InvalidConfig(
                "program must not be empty".to_string(),
            ));
        }
        // A remote (WSL UNC) cwd is the distro's to check, not ours: the
        // share only resolves while the distro is up, so `is_dir()` here
        // answers "is the distro running", not "does the directory exist",
        // and `wsl.exe --cd` reports a missing one itself. Same reason
        // #255 stopped launching `wsl.exe` *from* the UNC path.
        if let Some(cwd) = &spec.cwd {
            if !process_exec::host::ExecHost::for_path(cwd).is_remote() && !cwd.is_dir() {
                return Err(RunError::CwdNotFound(cwd.display().to_string()));
            }
        }

        // W4-1: a PTY spawn (`pty-core`) never touches `process_exec::run`/
        // `spawn`, so it gets none of their automatic WSL wrapping (W1-7) —
        // `LaunchSpec`'s argv is translated by hand here, the same
        // `ExecHost::argv` every other seam uses. A local `cwd` (or none)
        // makes `ExecHost::for_path` answer `Local` and this a no-op.
        let cwd_for_host = spec
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let host = process_exec::host::ExecHost::for_path(&cwd_for_host);
        let resolved_program =
            process_exec::host::resolve_program(&host, &spec.program, &cwd_for_host)
                .unwrap_or_else(|| spec.program.clone());
        let arg_refs: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        let (program, args) = host.argv(&resolved_program, &arg_refs, &cwd_for_host);

        let mut shell = ShellSpec::new(program, args).with_env(spec.env.clone());
        // Only a local cwd reaches the spawn. A remote one is already
        // carried by `--cd` inside the distro, and handing the UNC path to
        // the PTY as a working directory is the failure #255 removed from
        // `ExecHost::command`: the spawn needs the share to resolve, which
        // it does not when the distro is stopped.
        if let Some(cwd) = &spec.cwd {
            if !host.is_remote() {
                shell = shell.with_cwd(cwd.clone());
            }
        }

        let pty_session = PtySession::spawn(&shell, DEFAULT_PTY_SIZE)?;

        let id = ConsoleId(self.next_id);
        self.next_id += 1;
        self.consoles.insert(
            id,
            RunningConsole {
                pty_session,
                batcher: OutputBatcher::new(),
                config_id: config_id.into(),
            },
        );
        Ok(id)
    }

    /// Kill `id`'s whole process tree and stop tracking it on success.
    ///
    /// [`KillOutcome::Escaped`] is a successful signal that honestly reports
    /// a double-forked descendant that could not be reached — not an error,
    /// and still removes the console: the IDE cannot follow an escaped
    /// process, so nothing is served by continuing to track it.
    /// Ask `id`'s process tree to exit, and keep tracking it.
    ///
    /// The console stays: a program given a TERM is expected to still be
    /// there for a moment, printing whatever it flushes on the way out, and
    /// forgetting it here would throw that output away. Whoever asked for
    /// this escalates to [`Supervisor::stop`] if the process is still
    /// running after [`TERMINATION_GRACE`].
    pub fn terminate(&mut self, id: ConsoleId) -> Result<KillOutcome, RunError> {
        let console = self.consoles.get_mut(&id).ok_or(RunError::UnknownConsole)?;
        Ok(console.pty_session.terminate_tree()?)
    }

    pub fn stop(&mut self, id: ConsoleId) -> Result<KillOutcome, RunError> {
        let console = self.consoles.get_mut(&id).ok_or(RunError::UnknownConsole)?;
        let outcome = console.pty_session.kill_tree()?;
        self.consoles.remove(&id);
        Ok(outcome)
    }

    /// Move `id`'s PTY read half out for a dedicated reader thread to own —
    /// exactly [`pty_core::PtySession::take_reader`]'s own contract, exposed
    /// here because [`RunningConsole`]'s fields are private to this module
    /// and a `ui-shell::RunService` reader thread (one per console,
    /// mirroring `TerminalSessionRust`'s) needs the raw byte source to do
    /// the blocking `read()` loop `read_output`'s doc comment describes.
    /// Returns [`RunError::Io`] if already taken.
    pub fn take_reader(
        &mut self,
        id: ConsoleId,
    ) -> Result<Box<dyn std::io::Read + Send>, RunError> {
        let console = self.consoles.get_mut(&id).ok_or(RunError::UnknownConsole)?;
        console
            .pty_session
            .take_reader()
            .ok_or_else(|| RunError::Io("PTY read half already taken".to_string()))
    }

    /// Feed a chunk read from `id`'s PTY through its output batcher.
    /// The blocking `read()` loop itself belongs to the caller's reader
    /// thread, exactly as `TerminalSessionRust`'s reader thread calls
    /// `session.read()` then hands the bytes onward — obtained via
    /// [`Supervisor::take_reader`].
    pub fn read_output(
        &mut self,
        id: ConsoleId,
        chunk: &[u8],
        now: std::time::Instant,
    ) -> Result<Vec<BatchedOutput>, RunError> {
        let console = self.consoles.get_mut(&id).ok_or(RunError::UnknownConsole)?;
        Ok(console.batcher.push(chunk, now))
    }

    /// Flush whatever a console's batcher is holding once it has exited, so
    /// a short-lived run's final bytes are not lost waiting for a batch
    /// trigger that will never come.
    pub fn flush_remaining(
        &mut self,
        id: ConsoleId,
        now: std::time::Instant,
    ) -> Result<Vec<BatchedOutput>, RunError> {
        let console = self.consoles.get_mut(&id).ok_or(RunError::UnknownConsole)?;
        Ok(console.batcher.flush_all(now))
    }

    /// `Some(exit_code)` once `id`'s process has exited, `None` while still
    /// running.
    pub fn exit_code(&mut self, id: ConsoleId) -> Result<Option<u32>, RunError> {
        let console = self.consoles.get_mut(&id).ok_or(RunError::UnknownConsole)?;
        Ok(console.pty_session.try_wait()?)
    }

    /// Whether `id` is still tracked (launched and not yet stopped/reaped).
    /// The operating system's id for `id`'s process.
    ///
    /// Needed by exactly one caller: a debug adapter that asked the client
    /// to start the debuggee (`runInTerminal`) has to be told which process
    /// to attach to, and the answer is the pid (D1-6).
    pub fn process_id(&self, id: ConsoleId) -> Option<u32> {
        self.consoles.get(&id)?.pty_session.process_id()
    }

    /// Block until `id`'s process has exited, and report its exit code.
    ///
    /// [`Supervisor::exit_code`] asks without waiting, which is what a
    /// console poll wants; a caller that has just read the process's output
    /// to EOF needs this instead. The two are a hair apart in time and a
    /// world apart in meaning: right after EOF the child is exiting but
    /// usually not yet reaped, so `exit_code` answers `None` there — and a
    /// caller that read that as "no failure" would report a failed build as
    /// a successful one.
    pub fn wait(&mut self, id: ConsoleId) -> Result<u32, RunError> {
        let console = self.consoles.get_mut(&id).ok_or(RunError::UnknownConsole)?;
        console
            .pty_session
            .wait()
            .map_err(|err| RunError::Io(err.to_string()))
    }

    pub fn is_running(&self, id: ConsoleId) -> bool {
        self.consoles.contains_key(&id)
    }

    /// Every console currently tracked, oldest first.
    pub fn active_ids(&self) -> Vec<ConsoleId> {
        self.consoles.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConsoleKind;

    fn spec(program: &str, args: Vec<&str>) -> LaunchSpec {
        LaunchSpec {
            program: program.to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            cwd: None,
            env: Vec::new(),
            console: ConsoleKind::Pty,
        }
    }

    fn wait_for_exit(supervisor: &mut Supervisor, id: ConsoleId) -> u32 {
        loop {
            if let Some(code) = supervisor.exit_code(id).expect("exit_code") {
                return code;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn empty_program_is_rejected_before_spawning() {
        let mut supervisor = Supervisor::new();
        let result = supervisor.launch("cfg", &spec("", vec![]));
        assert!(matches!(result, Err(RunError::InvalidConfig(_))));
    }

    #[test]
    fn missing_cwd_is_rejected_before_spawning() {
        let mut supervisor = Supervisor::new();
        let mut launch_spec = spec("/bin/sh", vec!["-c", "echo hi"]);
        launch_spec.cwd = Some(std::path::PathBuf::from("/no/such/directory/anywhere"));
        let result = supervisor.launch("cfg", &launch_spec);
        assert!(matches!(result, Err(RunError::CwdNotFound(_))));
    }

    #[test]
    fn a_program_that_does_not_exist_reports_spawn_error_not_panic() {
        let mut supervisor = Supervisor::new();
        let result = supervisor.launch("cfg", &spec("/no/such/binary-xyz", vec![]));
        assert!(matches!(result, Err(RunError::Spawn(_))));
        assert!(supervisor.active_ids().is_empty());
    }

    #[test]
    fn two_launches_get_distinct_consoles_and_stopping_one_leaves_the_other_running() {
        let mut supervisor = Supervisor::new();
        let a = supervisor
            .launch("cfg-a", &spec("/bin/sh", vec!["-c", "sleep 5"]))
            .expect("launch a");
        let b = supervisor
            .launch("cfg-b", &spec("/bin/sh", vec!["-c", "sleep 5"]))
            .expect("launch b");
        assert_ne!(a, b);
        assert!(supervisor.is_running(a));
        assert!(supervisor.is_running(b));

        supervisor.stop(a).expect("stop a");
        assert!(!supervisor.is_running(a));
        assert!(supervisor.is_running(b), "stopping A must not affect B");

        supervisor.stop(b).expect("stop b");
        assert!(!supervisor.is_running(b));
    }

    #[test]
    fn terminate_asks_and_a_handler_gets_to_run_before_the_process_exits() {
        // The point of a TERM: this shell traps it, prints, and only then
        // leaves. A kill would lose the line — which is the whole reason
        // Stop stopped sending one (R2-4).
        let mut supervisor = Supervisor::new();
        let id = supervisor
            .launch(
                "cfg",
                &spec(
                    "/bin/sh",
                    vec!["-c", "trap 'echo caught-term; exit 0' TERM; sleep 5"],
                ),
            )
            .expect("launch");

        // The trap has to be installed before the signal arrives, or the
        // shell's default disposition takes it and nothing is printed.
        std::thread::sleep(std::time::Duration::from_millis(200));
        supervisor.terminate(id).expect("terminate");
        assert_eq!(wait_for_exit(&mut supervisor, id), 0);

        let mut buf = [0u8; 4096];
        let text = read_all_text(&mut supervisor, id, &mut buf, std::time::Instant::now());
        assert!(text.contains("caught-term"), "got: {text:?}");
    }

    #[test]
    fn a_process_that_ignores_terminate_survives_it_and_dies_to_stop() {
        // The other half of the pair: `terminate` keeps tracking the
        // console precisely because the process may still be there, and
        // `stop` is what the caller escalates to.
        let mut supervisor = Supervisor::new();
        let id = supervisor
            .launch("cfg", &spec("/bin/sh", vec!["-c", "trap '' TERM; sleep 5"]))
            .expect("launch");
        std::thread::sleep(std::time::Duration::from_millis(200));

        supervisor.terminate(id).expect("terminate");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            supervisor.is_running(id),
            "a TERM the process ignores must not be reported as a stop"
        );

        supervisor.stop(id).expect("stop");
        assert!(!supervisor.is_running(id));
    }

    #[test]
    fn output_from_one_console_never_appears_in_another() {
        let mut supervisor = Supervisor::new();
        let a = supervisor
            .launch("cfg-a", &spec("/bin/sh", vec!["-c", "echo from-a"]))
            .expect("launch a");
        let b = supervisor
            .launch("cfg-b", &spec("/bin/sh", vec!["-c", "echo from-b"]))
            .expect("launch b");

        wait_for_exit(&mut supervisor, a);
        wait_for_exit(&mut supervisor, b);

        let mut buf = [0u8; 4096];
        let now = std::time::Instant::now();
        let a_text = read_all_text(&mut supervisor, a, &mut buf, now);
        let b_text = read_all_text(&mut supervisor, b, &mut buf, now);

        assert!(a_text.contains("from-a"));
        assert!(!a_text.contains("from-b"));
        assert!(b_text.contains("from-b"));
        assert!(!b_text.contains("from-a"));
    }

    fn read_all_text(
        supervisor: &mut Supervisor,
        id: ConsoleId,
        buf: &mut [u8],
        now: std::time::Instant,
    ) -> String {
        // The console's own PTY reader is what a future reader thread would
        // own; the test drives it directly since there is no thread here.
        let mut text = String::new();
        // Read via a short-lived borrow of the console's session by going
        // through the supervisor's internal map is not exposed publicly, so
        // reuse read_output's batching path against bytes we read here.
        let bytes = read_raw(supervisor, id, buf);
        for event in supervisor
            .read_output(id, &bytes, now)
            .expect("read_output")
        {
            if let BatchedOutput::Output(s) = event {
                text.push_str(&s);
            }
        }
        for event in supervisor
            .flush_remaining(id, now)
            .expect("flush_remaining")
        {
            if let BatchedOutput::Output(s) = event {
                text.push_str(&s);
            }
        }
        text
    }

    /// Reads whatever is currently available on `id`'s PTY. Only reachable
    /// from within this test module, via a private accessor kept next to
    /// its only caller rather than exposed on `Supervisor`'s public API.
    fn read_raw(supervisor: &mut Supervisor, id: ConsoleId, buf: &mut [u8]) -> Vec<u8> {
        let console = supervisor.consoles.get_mut(&id).expect("console");
        let mut collected = Vec::new();
        // The child already exited by the time this runs (tests call
        // wait_for_exit first), so a handful of reads reaches EOF quickly.
        for _ in 0..8 {
            match console.pty_session.read(buf) {
                Ok(0) => break,
                Ok(n) => collected.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        collected
    }

    #[test]
    fn take_reader_hands_over_the_pty_read_half_exactly_once() {
        let mut supervisor = Supervisor::new();
        let id = supervisor
            .launch("cfg", &spec("/bin/sh", vec!["-c", "echo hi"]))
            .expect("launch");

        let mut reader = supervisor.take_reader(id).expect("take_reader");
        let mut buf = [0u8; 64];
        let mut collected = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => collected.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        assert!(String::from_utf8_lossy(&collected).contains("hi"));

        // Taking it again fails rather than panicking — the read half moved.
        assert!(matches!(supervisor.take_reader(id), Err(RunError::Io(_))));
    }

    #[test]
    fn take_reader_on_an_unknown_console_is_reported_not_panicked() {
        let mut supervisor = Supervisor::new();
        assert!(matches!(
            supervisor.take_reader(ConsoleId(999)),
            Err(RunError::UnknownConsole)
        ));
    }

    #[test]
    fn stopping_an_unknown_console_is_reported_not_panicked() {
        let mut supervisor = Supervisor::new();
        let result = supervisor.stop(ConsoleId(999));
        assert!(matches!(result, Err(RunError::UnknownConsole)));
    }

    #[test]
    fn a_running_console_reports_a_process_id_and_an_unknown_one_reports_none() {
        let mut supervisor = Supervisor::new();
        let spec = LaunchSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "sleep 1".into()],
            cwd: Some(std::env::temp_dir()),
            env: Vec::new(),
            console: ConsoleKind::Pty,
        };
        let id = supervisor.launch("pid", &spec).unwrap();
        assert!(supervisor.process_id(id).is_some());
        assert!(supervisor.process_id(ConsoleId(99)).is_none());
        let _ = supervisor.stop(id);
    }

    #[test]
    fn wait_reports_the_exit_code_a_poll_right_after_eof_would_miss() {
        let mut supervisor = Supervisor::new();
        let spec = LaunchSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "exit 3".into()],
            cwd: Some(std::env::temp_dir()),
            env: Vec::new(),
            console: ConsoleKind::Pty,
        };
        let id = supervisor.launch("waiter", &spec).unwrap();
        assert_eq!(supervisor.wait(id).unwrap(), 3);
    }

    #[test]
    fn waiting_on_an_unknown_console_is_reported_not_panicked() {
        let mut supervisor = Supervisor::new();
        assert!(matches!(
            supervisor.wait(ConsoleId(99)),
            Err(RunError::UnknownConsole)
        ));
    }

    // W4-1: `launch` translates `LaunchSpec`'s argv through `ExecHost`
    // itself — a PTY spawn never goes through `process_exec::run`/`spawn`'s
    // automatic wrapping. Proven with the same fake-`wsl.exe`-on-`PATH`
    // trick `process-exec`'s own tests use.
    #[test]
    fn launch_wraps_a_remote_cwd_through_wsl_exe() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;
        static PATH_LOCK: Mutex<()> = Mutex::new(());
        let _guard = PATH_LOCK.lock().unwrap();

        let bin_dir = tempfile::tempdir().unwrap();
        let script_path = bin_dir.path().join("wsl.exe");
        {
            let mut script = std::fs::File::create(&script_path).unwrap();
            write!(
                script,
                "#!/bin/sh\n\
                 for arg in \"$@\"; do\n\
                 \x20\x20if [ \"$arg\" = \"-lc\" ]; then echo /bin/sh; exit 0; fi\n\
                 done\n\
                 echo \"$@\"\n"
            )
            .unwrap();
        }
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        // Not created on disk, and deliberately so: a remote cwd is
        // classified by its UNC spelling, never opened — neither the
        // existence check nor the PTY spawn touches it. Creating it would
        // write `/wsl.localhost` at the filesystem root, which only
        // succeeds when the test runs as root (see #251, #252, #255).
        let cwd = std::path::PathBuf::from("//wsl.localhost/Ubuntu/tmp/run-core-e2e");

        let original_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: serialized by PATH_LOCK.
        unsafe {
            std::env::set_var(
                "PATH",
                format!("{}:{original_path}", bin_dir.path().display()),
            );
        }
        let mut supervisor = Supervisor::new();
        let mut launch_spec = spec("/bin/sh", vec!["-c", "true"]);
        launch_spec.cwd = Some(cwd);
        let id = supervisor.launch("cfg", &launch_spec).expect("launch");
        unsafe {
            std::env::set_var("PATH", original_path);
        }

        let mut reader = supervisor.take_reader(id).expect("take_reader");
        let mut buf = [0u8; 256];
        let mut collected = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => collected.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        let output = String::from_utf8_lossy(&collected);
        assert!(
            output.contains("-d Ubuntu --cd /tmp/run-core-e2e -e /bin/sh -c true"),
            "expected the wrapped argv on the PTY, got: {output:?}"
        );
    }
}
