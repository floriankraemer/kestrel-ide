//! Piped subprocess execution: spawn, drain stdout/stderr concurrently,
//! optional stdin, a wall-clock timeout, and killing the child on timeout.
//!
//! Extracted from `vcs-core/src/cli.rs` (B1 of the PHP tooling plan) — that
//! was the only process path in the repo with a wall-clock timeout and
//! per-pipe drain threads, which is exactly what an analyzer or test
//! runner needs too. This crate is the mechanism only: spawn/pipe/drain/
//! wait/kill. It has no error type of its own to speak of a program's
//! *meaning* — a missing `git` binary and a missing `phpstan` binary are
//! both just [`Failure::NotFound`], and the caller (which knows whether
//! that's `VcsError::GitNotInstalled` or an analyzer's "not installed"
//! status) maps it onto its own vocabulary, the same split `stdio-framing`
//! draws between framing bytes and LSP/DAP message meaning.

use std::io;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub mod host;

use host::ExecHost;

/// On Windows, stop a spawned child from briefly flashing its own console
/// window — `Command::new` otherwise allocates one for every subprocess,
/// visible for the instant it takes to run something as quick as `git
/// status`. No effect (and no `windows-sys`/`winapi` dependency) on other
/// platforms. `pub(crate)`: `host::ExecHost::command` applies this too, so
/// `wsl.exe` itself — the only Windows-side process a remote run spawns
/// directly — never flashes a console either.
pub(crate) fn suppress_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// A finished process's captured output.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Why [`run`] did not return an [`Output`].
#[derive(Debug)]
pub enum Failure {
    /// `program` is not on `PATH` (or not executable).
    NotFound,
    /// The process did not finish within the given timeout; it has been
    /// killed.
    TimedOut,
    /// Spawning, writing stdin, draining a pipe, or waiting failed for a
    /// reason that isn't "not found" or "timed out" — the message is
    /// `io::Error::to_string()`.
    Io(String),
}

/// Run `program args` in `work_dir`, optionally feeding `stdin` first,
/// waiting at most `timeout` before killing it.
///
/// `env` is extra environment variables set on top of the inherited
/// environment (e.g. `vcs-core` sets `GIT_TERMINAL_PROMPT=0`).
///
/// Draining stdout and stderr each on their own thread is not optional:
/// the child can fill either pipe's OS buffer and block on a write before
/// this function looks at it, so anything that does not drain both
/// concurrently has a deadlock built in. Draining here, rather than in a
/// thread that owns the whole child, is what lets this function keep the
/// `Child` and therefore kill it on timeout instead of leaving it running
/// after returning [`Failure::TimedOut`].
pub fn run(
    program: &str,
    args: &[&str],
    work_dir: &Path,
    stdin: Option<&[u8]>,
    timeout: Duration,
    env: &[(&str, &str)],
) -> Result<Output, Failure> {
    let host = ExecHost::for_path(work_dir);
    let resolved_program = match host::resolve_program(&host, program, work_dir) {
        Some(resolved) => resolved,
        // Not found in the distro: fall through with the bare name so the
        // spawn below still happens and fails the normal way (`wsl.exe`
        // exits with `-e`'s "no such file or directory", which is not
        // exit 127 — it is exit 2 — so this does not double-report; the
        // caller simply gets git/analyzer-shaped stderr instead of an
        // early `Failure::NotFound`. Acceptable: the common case, a
        // present binary, never takes this branch.)
        None => program.to_string(),
    };

    let mut command = host.command(&resolved_program, args, work_dir, env);
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Failure::NotFound),
        Err(e) => return Err(Failure::Io(e.to_string())),
    };

    if let Some(bytes) = stdin {
        use std::io::Write;
        // Written synchronously before the drain threads below start, then
        // dropped to close the pipe. Callers that hand this a payload
        // large enough to fill the stdin pipe buffer before the child
        // starts reading its own stdout/stderr accept that deadlock risk
        // themselves (none of today's callers do: `git apply`'s patches
        // and an analyzer's stdin-fed source file are both read to EOF
        // before the tool produces output).
        let mut pipe = child.stdin.take().expect("stdin was piped");
        if let Err(e) = pipe.write_all(bytes) {
            return Err(Failure::Io(e.to_string()));
        }
        drop(pipe);
    }

    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let stdout_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        io::Read::read_to_end(&mut stdout_pipe, &mut buffer).map(|_| buffer)
    });
    let stderr_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        io::Read::read_to_end(&mut stderr_pipe, &mut buffer).map(|_| buffer)
    });

    let Some(status) =
        wait_with_timeout(&mut child, timeout).map_err(|e| Failure::Io(e.to_string()))?
    else {
        // The pipes are still owned by the reader threads; killing the
        // child closes its ends, so they finish rather than blocking
        // forever on a process nobody is waiting for any more.
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::TimedOut);
    };

    let join = |reader: thread::JoinHandle<io::Result<Vec<u8>>>| match reader.join() {
        Ok(Ok(buffer)) => Ok(buffer),
        Ok(Err(e)) => Err(Failure::Io(e.to_string())),
        Err(_) => Err(Failure::Io("reading the process's output failed".into())),
    };

    let stdout = join(stdout_reader)?;
    let stderr = join(stderr_reader)?;

    // `wsl.exe` spawns fine even when the *Linux* program is missing, so a
    // missing binary shows up as an ordinary nonzero exit rather than
    // `io::ErrorKind::NotFound` above. Remap the two ways that happens
    // (exit 127, or `wsl.exe`'s own "no such distro" stderr) onto the same
    // `Failure::NotFound` a missing local binary already reports, so
    // `VcsError::GitNotInstalled` and friends keep working unmodified.
    if host.is_remote() && host::is_missing_program(status.code(), &stderr) {
        return Err(Failure::NotFound);
    }

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Wait for `child` for at most `timeout`, returning `None` if it outlives
/// that.
///
/// Backs off from a tenth of a millisecond so a process that finishes in
/// single-digit milliseconds is not held up by the poll interval, while one
/// sitting on a long-running call is not woken thousands of times a second.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    const MIN_POLL: Duration = Duration::from_micros(100);
    const MAX_POLL: Duration = Duration::from_millis(20);

    let deadline = Instant::now() + timeout;
    let mut poll = MIN_POLL;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = match deadline.checked_duration_since(Instant::now()) {
            Some(remaining) if !remaining.is_zero() => remaining,
            _ => return Ok(None),
        };
        thread::sleep(poll.min(remaining));
        poll = (poll * 2).min(MAX_POLL);
    }
}

/// A spawned child whose output is read incrementally rather than
/// collected to completion, and which can be killed from another thread —
/// what a test runner needs and [`run`] cannot give it, since [`run`] does
/// not return until the child has already exited.
///
/// Cloneable and `Send` for the same reason `build_core::BuildHandle`
/// wraps its supervisor in an `Arc<Mutex<_>>`: one thread reads the pipes
/// to completion while another (the Qt thread, in `test-core`'s caller)
/// holds a handle only to call [`Spawned::kill`].
#[derive(Debug, Clone)]
pub struct Spawned {
    child: Arc<Mutex<Child>>,
}

impl Spawned {
    /// Take stdout's read end. `None` if already taken — a caller only
    /// ever takes it once, right after spawning.
    pub fn take_stdout(&self) -> Option<ChildStdout> {
        self.child.lock().ok()?.stdout.take()
    }

    /// Take stderr's read end, same rule as [`Self::take_stdout`].
    pub fn take_stderr(&self) -> Option<ChildStderr> {
        self.child.lock().ok()?.stderr.take()
    }

    /// Kill the child. Not a process-tree kill (see [`run`]'s doc comment
    /// on the same limitation) — sound for a test framework's own process,
    /// which is what every caller today spawns directly rather than
    /// through a shell wrapper.
    pub fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    /// Block until the child exits, reaping it. Call only after both pipes
    /// have been read to EOF — same deadlock risk [`run`] avoids by
    /// draining concurrently, which this type leaves to its caller since
    /// it hands the pipes out rather than draining them itself.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        self.child.lock().expect("process lock poisoned").wait()
    }
}

/// Spawn `program args` in `work_dir` with piped stdout/stderr, without
/// waiting for it to finish. The caller reads [`Spawned::take_stdout`] (and
/// optionally stderr) as bytes arrive, which is the whole difference from
/// [`run`]: this is for a caller that wants to act on output as it streams
/// in — a test runner's TeamCity service messages — rather than parse a
/// batch report after the process has already exited.
pub fn spawn(program: &str, args: &[&str], work_dir: &Path) -> Result<Spawned, Failure> {
    let host = ExecHost::for_path(work_dir);
    let resolved_program =
        host::resolve_program(&host, program, work_dir).unwrap_or_else(|| program.to_string());

    let mut command = host.command(&resolved_program, args, work_dir, &[]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn();
    match child {
        Ok(child) => Ok(Spawned {
            child: Arc::new(Mutex::new(child)),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(Failure::NotFound),
        Err(e) => Err(Failure::Io(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            "echo",
            &["hello"],
            dir.path(),
            None,
            Duration::from_secs(5),
            &[],
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hello\n");
    }

    #[test]
    fn feeds_stdin_to_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            "cat",
            &[],
            dir.path(),
            Some(b"piped in"),
            Duration::from_secs(5),
            &[],
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"piped in");
    }

    #[test]
    fn nonzero_exit_is_reported_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            "sh",
            &["-c", "echo oops >&2; exit 1"],
            dir.path(),
            None,
            Duration::from_secs(5),
            &[],
        )
        .unwrap();
        assert!(!out.status.success());
        assert_eq!(out.stderr, b"oops\n");
    }

    #[test]
    fn missing_binary_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(
            "this-binary-does-not-exist-anywhere",
            &[],
            dir.path(),
            None,
            Duration::from_secs(5),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, Failure::NotFound));
    }

    #[test]
    fn times_out_on_a_command_that_hangs() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(
            "sh",
            &["-c", "sleep 5"],
            dir.path(),
            None,
            Duration::from_millis(200),
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, Failure::TimedOut));
    }

    #[test]
    fn extra_env_reaches_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            "sh",
            &["-c", "echo $PROCESS_EXEC_TEST_VAR"],
            dir.path(),
            None,
            Duration::from_secs(5),
            &[("PROCESS_EXEC_TEST_VAR", "set")],
        )
        .unwrap();
        assert_eq!(out.stdout, b"set\n");
    }

    #[test]
    fn spawn_streams_stdout_before_the_child_exits() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let spawned = spawn("sh", &["-c", "echo one; sleep 5; echo two"], dir.path()).unwrap();
        let mut stdout = spawned.take_stdout().unwrap();
        let mut buffer = [0u8; 16];
        let read = stdout.read(&mut buffer).unwrap();
        // The child is still sleeping when this returns: proof the caller
        // sees "one" without waiting for "two" or exit, which `run` could
        // never give it.
        assert_eq!(&buffer[..read], b"one\n");
        spawned.kill();
        let _ = spawned.wait();
    }

    #[test]
    fn spawn_reports_a_missing_binary_as_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = spawn("this-binary-does-not-exist-anywhere", &[], dir.path()).unwrap_err();
        assert!(matches!(err, Failure::NotFound));
    }

    #[test]
    fn spawned_can_be_killed_from_another_thread() {
        let dir = tempfile::tempdir().unwrap();
        let spawned = spawn("sh", &["-c", "sleep 5"], dir.path()).unwrap();
        let killer = spawned.clone();
        let handle = thread::spawn(move || killer.kill());
        handle.join().unwrap();
        let status = spawned.wait().unwrap();
        assert!(!status.success());
    }
}
