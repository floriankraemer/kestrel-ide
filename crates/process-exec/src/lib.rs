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
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(work_dir)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }

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

    Ok(Output {
        status,
        stdout: join(stdout_reader)?,
        stderr: join(stderr_reader)?,
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
}
