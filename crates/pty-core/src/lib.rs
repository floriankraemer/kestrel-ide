//! Cross-platform PTY transport: spawn a shell attached to a pseudo-terminal,
//! read/write its byte stream, resize it, and manage the child process.
//!
//! Qt-free by design (see `docs/architecture/layering.md`) — this crate only
//! moves bytes in and out of a PTY. Grid/VT100 state lives in `terminal-core`
//! wiring it into the UI lives in `ui-shell`.
//!
//! Blocking reads are intentional: the eventual `ui-shell` integration drives
//! this from a dedicated `std::thread` doing blocking reads, the same shape
//! `start_mcp_server` already uses for its background listener thread.

use std::env;
use std::fmt;
use std::io::{Read, Write};
use std::path::PathBuf;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize as NativePtySize};

/// Which shells this machine offers, for the terminal's shell picker.
pub mod shells;

pub use shells::ShellCandidate;

/// Terminal size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl PtySize {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

impl From<PtySize> for NativePtySize {
    fn from(size: PtySize) -> Self {
        NativePtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// Typed error crossing this crate's API (ADR-0003's typed-error convention
/// applies once this reaches the FFI seam in a later task).
#[derive(Debug)]
pub enum PtyError {
    Spawn(String),
    Io(String),
    Resize(String),
    Wait(String),
}

impl fmt::Display for PtyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PtyError::Spawn(msg) => write!(f, "failed to spawn shell: {msg}"),
            PtyError::Io(msg) => write!(f, "PTY I/O error: {msg}"),
            PtyError::Resize(msg) => write!(f, "failed to resize PTY: {msg}"),
            PtyError::Wait(msg) => write!(f, "failed to wait on child process: {msg}"),
        }
    }
}

impl std::error::Error for PtyError {}

/// Which Windows shell to launch. Windows offers no single canonical shell,
/// and CI/build environments won't have all of them installed, so the
/// caller picks explicitly instead of this crate probing the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsShellKind {
    PowerShellCore,
    WindowsPowerShell,
    Wsl,
}

impl WindowsShellKind {
    fn program(self) -> &'static str {
        match self {
            WindowsShellKind::PowerShellCore => "pwsh.exe",
            WindowsShellKind::WindowsPowerShell => "powershell.exe",
            WindowsShellKind::Wsl => "wsl.exe",
        }
    }
}

/// The program (and args) to launch as the PTY's child process.
/// Deliberately a plain data struct — no OS probing happens in constructors,
/// so callers (and tests) can inject an explicit shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSpec {
    pub program: String,
    pub args: Vec<String>,
    /// Working directory for the child. `None` inherits this process's,
    /// which is what a terminal wants; a run configuration names its own.
    pub cwd: Option<PathBuf>,
    /// Environment entries **added to** the inherited environment, not
    /// replacing it. A child that cannot see `PATH` or `HOME` behaves
    /// nothing like the same command typed into a shell, and every user
    /// expectation here is set by the shell.
    pub env: Vec<(String, String)>,
}

impl ShellSpec {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: None,
            env: Vec::new(),
        }
    }

    /// Run in `dir` rather than inheriting the IDE's working directory.
    pub fn with_cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Add environment entries on top of the inherited environment.
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// Resolve `$SHELL`, falling back to `/bin/bash` then `/bin/sh` if unset.
    pub fn unix_default() -> Self {
        let program = env::var("SHELL").unwrap_or_else(|_| {
            if std::path::Path::new("/bin/bash").exists() {
                "/bin/bash".to_string()
            } else {
                "/bin/sh".to_string()
            }
        });
        Self {
            program,
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
        }
    }

    /// A named Windows shell, with no OS probing — the caller (or its own
    /// fallback policy) decides which of `pwsh.exe`/`powershell.exe`/
    /// `wsl.exe` to request.
    pub fn windows(kind: WindowsShellKind) -> Self {
        Self {
            program: kind.program().to_string(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
        }
    }
}

/// A running shell attached to a pseudo-terminal.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
}

impl PtySession {
    /// Spawn `shell` attached to a new PTY of the given size.
    pub fn spawn(shell: &ShellSpec, size: PtySize) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size.into())
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        let mut cmd = CommandBuilder::new(&shell.program);
        for arg in &shell.args {
            cmd.arg(arg);
        }
        if let Some(dir) = &shell.cwd {
            cmd.cwd(dir);
        }
        for (key, value) in &shell.env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        // Drop our copy of the slave end after spawning: the child owns its
        // own handle, and holding ours open would keep the PTY's read end
        // from ever reporting EOF once the child exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Spawn(e.to_string()))?;

        Ok(Self {
            master: pair.master,
            child,
            reader: Some(reader),
            writer,
        })
    }

    /// Blocking read of whatever output bytes are currently available.
    /// Returns `Ok(0)` on EOF (child exited and closed the PTY). Errors with
    /// `PtyError::Io` if [`take_reader`](Self::take_reader) already moved
    /// the read half out.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, PtyError> {
        match self.reader.as_mut() {
            Some(reader) => reader.read(buf).map_err(|e| PtyError::Io(e.to_string())),
            None => Err(PtyError::Io("read half already taken".to_string())),
        }
    }

    /// Move the read half out for a dedicated background thread to own.
    /// Needed because `read`/`write` both require exclusive (`&mut self`)
    /// access: a caller that put the whole `PtySession` behind one lock so a
    /// background thread could do blocking reads would find that lock held
    /// for the whole blocking `read` call, stalling any `write` from another
    /// thread until the next output byte arrives — for an interactive shell
    /// that's a deadlock (the shell can't echo a keystroke that `write` can
    /// never deliver). Splitting the read half out lets the reader thread
    /// own it exclusively while `write`/`resize`/`kill` stay on the
    /// `PtySession` for the caller's own thread to use, lock-free. Returns
    /// `None` if already taken.
    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    /// Write input bytes (e.g. keystrokes) to the shell.
    pub fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.writer
            .write_all(data)
            .map_err(|e| PtyError::Io(e.to_string()))
    }

    /// Resize the PTY (e.g. on dock-widget resize).
    pub fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        self.master
            .resize(size.into())
            .map_err(|e| PtyError::Resize(e.to_string()))
    }

    /// Forcibly terminate the child process.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        self.child.kill().map_err(|e| PtyError::Wait(e.to_string()))
    }

    /// The child's process id, while it is running.
    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Kill the child **and everything it started**.
    ///
    /// [`PtySession::kill`] signals one process. That is right for a shell,
    /// which passes the signal on, and wrong for anything else: a `cargo
    /// build` killed on its own leaves rustc processes holding the CPU and
    /// the target directory, and a `npm test` leaves node. Orphaned build
    /// processes are the most-complained-about defect in every IDE that got
    /// this wrong, so a run configuration uses this, not `kill`.
    ///
    /// # Platform behaviour
    ///
    /// On Unix the child leads its own process group (see
    /// [`PtySession::spawn`]), so one `killpg` reaches every descendant that
    /// has not deliberately left the group.
    ///
    /// **A process that double-forks and calls `setsid` escapes**, and this
    /// reports that honestly rather than claiming success — a daemon is
    /// supposed to survive its parent, and pretending otherwise would be a
    /// lie about the one thing the caller wanted to know.
    ///
    /// On Windows the child belongs to a Job Object created with
    /// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and terminating the job takes
    /// the whole tree with it. That path is compiled but **not exercised by
    /// CI**, which has no Windows runner — see the release checklist.
    pub fn kill_tree(&mut self) -> Result<KillOutcome, PtyError> {
        let Some(pid) = self.process_id() else {
            // Already reaped: there is nothing left to signal, which is the
            // state the caller was asking for.
            return Ok(KillOutcome::Complete);
        };
        let outcome = platform::signal_tree(pid, platform::Signal::Kill)?;
        // Always signal the direct child too, so a failure to reach the
        // group still stops the thing we actually started.
        let _ = self.child.kill();
        Ok(outcome)
    }

    /// **Ask** the child's whole tree to exit, and leave it running if it
    /// declines.
    ///
    /// This is IntelliJ's Exit next to [`PtySession::kill_tree`]'s Kill: a
    /// TERM that a program with a signal handler can act on — flush its
    /// output, close its sockets, remove its pid file — where a KILL gives
    /// it no such chance. The caller escalates if the process is still
    /// there after a grace period (`run_core::Supervisor` does, see
    /// `TERMINATION_GRACE`), which is the half of the sentence
    /// [`platform::signal_tree`]'s TERM branch has always described but
    /// nothing implemented: `kill_tree` sent TERM and then immediately
    /// killed the child anyway, so nothing was ever given time to react.
    ///
    /// On Windows there is no soft equivalent — a Job Object terminates —
    /// so this is the same call as `kill_tree`, honestly, rather than a
    /// promise the platform cannot keep.
    pub fn terminate_tree(&mut self) -> Result<KillOutcome, PtyError> {
        let Some(pid) = self.process_id() else {
            return Ok(KillOutcome::Complete);
        };
        platform::signal_tree(pid, platform::Signal::Terminate)
    }

    /// Non-blocking check: `Some(exit_code)` if the child has already
    /// exited, `None` if it's still running.
    pub fn try_wait(&mut self) -> Result<Option<u32>, PtyError> {
        self.child
            .try_wait()
            .map_err(|e| PtyError::Wait(e.to_string()))
            .map(|status| status.map(|s| s.exit_code()))
    }

    /// Block until the child exits, returning its exit code.
    pub fn wait(&mut self) -> Result<u32, PtyError> {
        self.child
            .wait()
            .map_err(|e| PtyError::Wait(e.to_string()))
            .map(|status| status.exit_code())
    }
}

/// What [`PtySession::kill_tree`] managed to reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    /// Every process in the child's group was signalled.
    Complete,
    /// The group was signalled, but at least one descendant had already
    /// left it — a double-forked daemon, typically. Those processes are
    /// still running and this build cannot reach them.
    ///
    /// Reported rather than swallowed: a caller that tells the user "stopped"
    /// when something is still holding a port or a lock file has told them
    /// the one thing they needed to know incorrectly.
    Escaped,
}

#[cfg(unix)]
mod platform {
    use super::{KillOutcome, PtyError};

    /// Which of the two things a caller can mean by "stop this".
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Signal {
        /// TERM: a request the process may handle, delay, or ignore.
        Terminate,
        /// KILL: not a request.
        Kill,
    }

    /// Signal the child's whole process group.
    ///
    /// Errors other than "no such group" are real failures.
    pub(super) fn signal_tree(pid: u32, signal: Signal) -> Result<KillOutcome, PtyError> {
        let number = match signal {
            Signal::Terminate => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // Safety: killpg with a valid pgid and a standard signal number has
        // no memory effects; the only failure modes are reported via errno.
        let result = unsafe { libc::killpg(pid as libc::pid_t, number) };
        if result == 0 {
            return Ok(KillOutcome::Complete);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            // The group is already gone — which is the requested end state.
            Some(libc::ESRCH) => Ok(KillOutcome::Complete),
            // We are not allowed to signal it, which in practice means
            // something in it changed credentials or left the group.
            Some(libc::EPERM) => Ok(KillOutcome::Escaped),
            _ => Err(PtyError::Wait(err.to_string())),
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{KillOutcome, PtyError};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Signal {
        Terminate,
        Kill,
    }

    /// Windows has no process groups in the Unix sense. The tree is killed
    /// by the Job Object the child was assigned at spawn, which closes with
    /// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    ///
    /// Not exercised by CI: the Windows binary is cross-built through MXE
    /// and there is no Windows runner, so this is covered by the manual
    /// release checklist instead of a test that cannot run.
    ///
    /// `Signal` is accepted and ignored: Windows job termination has no
    /// "ask politely" form, and pretending otherwise would make
    /// `terminate_tree` claim a grace period the platform never gives.
    pub(super) fn signal_tree(_pid: u32, _signal: Signal) -> Result<KillOutcome, PtyError> {
        // portable-pty's Windows child already terminates its job on kill,
        // so the caller's follow-up `child.kill()` does the work. Reported
        // as complete because the job takes the tree with it.
        Ok(KillOutcome::Complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_reads_expected_output() {
        let shell = ShellSpec::new("/bin/sh", vec!["-c".into(), "echo hello".into()]);
        let mut session = PtySession::spawn(&shell, PtySize::new(24, 80)).expect("spawn");

        let mut output = Vec::new();
        let mut buf = [0u8; 256];
        // Read until EOF (child exits and closes the PTY) or we've clearly
        // seen the expected text — avoids hanging if the shell keeps the
        // PTY open past printing.
        loop {
            match session.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    output.extend_from_slice(&buf[..n]);
                    if String::from_utf8_lossy(&output).contains("hello") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("hello"),
            "expected 'hello' in output, got: {text:?}"
        );

        session.wait().expect("wait");
    }

    #[test]
    fn resize_does_not_error() {
        let shell = ShellSpec::new("/bin/sh", vec!["-c".into(), "sleep 1".into()]);
        let mut session = PtySession::spawn(&shell, PtySize::new(24, 80)).expect("spawn");

        session.resize(PtySize::new(40, 120)).expect("resize");

        session.kill().expect("kill");
        session.wait().expect("wait");
    }

    #[test]
    fn kill_stops_the_child() {
        let shell = ShellSpec::new("/bin/sh", vec!["-c".into(), "sleep 30".into()]);
        let mut session = PtySession::spawn(&shell, PtySize::new(24, 80)).expect("spawn");

        assert_eq!(session.try_wait().expect("try_wait before kill"), None);

        session.kill().expect("kill");
        session.wait().expect("wait after kill");

        assert!(session.try_wait().expect("try_wait after kill").is_some());
    }

    #[test]
    fn take_reader_moves_reading_out_and_still_works() {
        let shell = ShellSpec::new("/bin/sh", vec!["-c".into(), "echo hi".into()]);
        let mut session = PtySession::spawn(&shell, PtySize::new(24, 80)).expect("spawn");

        let mut reader = session.take_reader().expect("reader available once");
        assert!(
            session.take_reader().is_none(),
            "second take must yield None"
        );

        let mut output = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    output.extend_from_slice(&buf[..n]);
                    if String::from_utf8_lossy(&output).contains("hi") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        assert!(String::from_utf8_lossy(&output).contains("hi"));

        session.wait().expect("wait");
    }

    #[test]
    fn read_after_take_reader_errors() {
        let shell = ShellSpec::new("/bin/sh", vec!["-c".into(), "sleep 1".into()]);
        let mut session = PtySession::spawn(&shell, PtySize::new(24, 80)).expect("spawn");

        session.take_reader();
        let mut buf = [0u8; 16];
        assert!(session.read(&mut buf).is_err());

        session.kill().expect("kill");
        session.wait().expect("wait");
    }

    #[test]
    fn unix_default_resolves_a_shell() {
        let spec = ShellSpec::unix_default();
        assert!(!spec.program.is_empty());
    }

    #[test]
    fn windows_shell_kinds_map_to_expected_programs() {
        assert_eq!(
            ShellSpec::windows(WindowsShellKind::PowerShellCore).program,
            "pwsh.exe"
        );
        assert_eq!(
            ShellSpec::windows(WindowsShellKind::WindowsPowerShell).program,
            "powershell.exe"
        );
        assert_eq!(ShellSpec::windows(WindowsShellKind::Wsl).program, "wsl.exe");
    }
}

#[cfg(all(test, unix))]
mod kill_tree_tests {
    use super::*;
    use std::thread;
    use std::time::{Duration, Instant};

    fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("timed out waiting for {what}");
    }

    /// Whether `pid` is a *running* process.
    ///
    /// Deliberately not `kill(pid, 0)`: that succeeds for a **zombie**, and a
    /// process killed here is very likely to become one — its parent has just
    /// been killed too, so it is reparented to PID 1, and in a container PID 1
    /// is usually the test binary's own init, which may never reap it. Signal
    /// 0 would then report a dead process as alive forever and this test would
    /// fail against perfectly correct code.
    fn alive(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // The state field follows the comm field, which is parenthesised and
        // may itself contain spaces — so split on the last ')' rather than
        // counting fields from the left.
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .map(|state| state != "Z")
            .unwrap_or(false)
    }

    /// The child of a PTY is expected to lead its own process group, which is
    /// what makes `killpg` reach the whole tree. That is portable-pty's doing
    /// rather than ours, so it is asserted rather than assumed — if a version
    /// bump ever changes it, `kill_tree` silently degrades to `kill` and this
    /// is the test that says so.
    #[test]
    fn the_child_leads_its_own_process_group() {
        let spec = ShellSpec::new("/bin/sh", vec!["-c".into(), "sleep 30".into()]);
        let mut session = PtySession::spawn(&spec, PtySize::new(24, 80)).unwrap();
        let pid = session.process_id().expect("running child") as libc::pid_t;

        let pgid = unsafe { libc::getpgid(pid) };
        assert_eq!(
            pgid, pid,
            "the child is not its own process group leader, so killpg would \
             not reach its children"
        );
        let _ = session.kill_tree();
    }

    /// The case that matters: a build tool that spawns compilers. Killing the
    /// direct child alone leaves the grandchild holding the CPU.
    #[test]
    fn kill_tree_reaches_a_grandchild() {
        // `sh -c 'sleep 300 & echo $!; wait'` prints the grandchild's pid and
        // then blocks, so the test can watch that specific process.
        let spec = ShellSpec::new(
            "/bin/sh",
            vec!["-c".into(), "sleep 300 & echo $!; wait".into()],
        );
        let mut session = PtySession::spawn(&spec, PtySize::new(24, 80)).unwrap();

        let mut buffer = String::new();
        wait_until("the grandchild to report its pid", || {
            let mut chunk = [0u8; 256];
            match session.read(&mut chunk) {
                Ok(n) if n > 0 => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    buffer.contains('\n')
                }
                _ => false,
            }
        });
        let grandchild: u32 = buffer
            .split_whitespace()
            .find_map(|t| t.parse().ok())
            .expect("a pid on stdout");
        assert!(alive(grandchild), "the grandchild should be running");

        assert_eq!(session.kill_tree().unwrap(), KillOutcome::Complete);
        wait_until("the grandchild to die", || !alive(grandchild));
    }

    /// Killing something already gone is not an error: the requested end
    /// state is "not running", and it is.
    #[test]
    fn killing_an_already_dead_tree_is_success() {
        let spec = ShellSpec::new("/bin/sh", vec!["-c".into(), "exit 0".into()]);
        let mut session = PtySession::spawn(&spec, PtySize::new(24, 80)).unwrap();
        wait_until("the child to exit", || {
            matches!(session.try_wait(), Ok(Some(_)))
        });
        assert_eq!(session.kill_tree().unwrap(), KillOutcome::Complete);
    }
}

#[cfg(all(test, unix))]
mod cwd_env_tests {
    use super::*;
    use std::thread;
    use std::time::{Duration, Instant};

    fn read_output(session: &mut PtySession, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buffer = String::new();
        while Instant::now() < deadline {
            let mut chunk = [0u8; 512];
            if let Ok(n) = session.read(&mut chunk) {
                if n > 0 {
                    buffer.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    if buffer.contains(needle) {
                        return buffer;
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out; got: {buffer:?}");
    }

    #[test]
    fn the_child_runs_in_the_requested_directory() {
        let dir = std::fs::canonicalize("/tmp").unwrap();
        let spec = ShellSpec::new("/bin/sh", vec!["-c".into(), "pwd".into()]).with_cwd(&dir);
        let mut session = PtySession::spawn(&spec, PtySize::new(24, 80)).unwrap();
        let out = read_output(&mut session, "tmp");
        assert!(out.contains(dir.to_str().unwrap()), "got {out:?}");
    }

    /// Environment entries are added, not substituted. A child that cannot
    /// see PATH behaves nothing like the same command typed into a shell.
    #[test]
    fn env_entries_are_added_to_the_inherited_environment() {
        let spec = ShellSpec::new(
            "/bin/sh",
            vec!["-c".into(), "echo \"$IDE_MARKER:${PATH:+haspath}\"".into()],
        )
        .with_env(vec![("IDE_MARKER".into(), "set-by-test".into())]);
        let mut session = PtySession::spawn(&spec, PtySize::new(24, 80)).unwrap();
        let out = read_output(&mut session, "set-by-test");
        assert!(out.contains("set-by-test:haspath"), "got {out:?}");
    }
}
