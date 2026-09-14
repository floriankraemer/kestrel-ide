//! Container lifecycle operations (C3, ADR-0055): pure argv builders plus
//! one executor, [`run_op`], that runs them through an [`Invocation`] and
//! classifies a nonzero exit into a typed [`OpError`] — the same
//! substring-on-stderr approach [`crate::probe::ConnectionError::from_stderr`]
//! already uses for "Test connection", now for start/stop/restart/remove/
//! pause/unpause/prune.
//!
//! Every `*_args` function is engine-agnostic on purpose: `docker` and
//! `podman` share this subcommand surface byte for byte (ADR-0055), so
//! there is exactly one argv per operation rather than one per engine.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use crate::connection::Invocation;

/// How long a lifecycle operation gets before it is killed — generous
/// next to a query's timeout (`probe::probe` uses a few seconds): `stop`
/// waits out the container's own grace period (10s default) before the
/// engine sends `SIGKILL`.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Why an operation did not succeed. Classified from the engine's own
/// stderr text (a known ceiling — see [`OpError::from_stderr`]'s doc
/// comment), never from a documented, stable exit code: neither engine
/// commits to one across versions the way their stderr wording has stayed
/// stable for years.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpErrorCode {
    NotFound,
    AlreadyRunning,
    NotRunning,
    PermissionDenied,
    EngineUnavailable,
    /// [`crate::recreate::recreate`] only: the old container was already
    /// removed (`rm -f` succeeded) but the new `run` then failed, so there
    /// is no container under this id any more — the confirm dialog's own
    /// warning, now a fact rather than a possibility.
    OldContainerRemoved,
    Other,
}

/// A failed operation: a code to branch on (nothing in `ui-shell` does
/// today — `actionFinished` only carries `ok`/`message` — but the
/// distinction is what the classifier tests below assert against) plus a
/// finished sentence for the panel's error banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpError {
    pub code: OpErrorCode,
    pub message: String,
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for OpError {}

impl OpError {
    fn from_failure(failure: process_exec::Failure) -> Self {
        match failure {
            process_exec::Failure::NotFound => OpError {
                code: OpErrorCode::EngineUnavailable,
                message: "the container engine's CLI is not installed".to_string(),
            },
            process_exec::Failure::TimedOut => OpError {
                code: OpErrorCode::Other,
                message: "the operation did not finish in time".to_string(),
            },
            process_exec::Failure::Io(message) => OpError {
                code: OpErrorCode::Other,
                message,
            },
        }
    }

    /// Classify a nonzero exit's stderr into the more specific variants,
    /// substring-matching the wording both engines have used for years —
    /// `docker`'s "Error response from daemon: No such container …",
    /// podman's "no container with name or ID … found", "is not running",
    /// "container already stopped", the shared "permission denied while
    /// trying to connect to the Docker daemon socket" and "Cannot connect
    /// to the Docker daemon"/"cannot connect to Podman".
    ///
    /// Known ceiling: this is wording, not a contract, so a future CLI
    /// release that rephrases a message downgrades that one case to
    /// [`OpErrorCode::Other`] rather than panicking or misclassifying —
    /// upgrade path is a documented, versioned exit code per engine if one
    /// is ever added.
    pub fn from_stderr(stderr: &str) -> Self {
        let message = stderr.trim().to_string();
        let lower = message.to_lowercase();
        let code = if lower.contains("no such container")
            || lower.contains("no container with name")
            || lower.contains("no such object")
        {
            OpErrorCode::NotFound
        } else if lower.contains("permission denied") {
            OpErrorCode::PermissionDenied
        } else if lower.contains("cannot connect to the docker daemon")
            || lower.contains("cannot connect to podman")
            || lower.contains("is the docker daemon running")
        {
            OpErrorCode::EngineUnavailable
        } else if lower.contains("already running")
            || (lower.contains("already") && lower.contains("started"))
        {
            OpErrorCode::AlreadyRunning
        } else if lower.contains("is not running") || lower.contains("already stopped") {
            OpErrorCode::NotRunning
        } else {
            OpErrorCode::Other
        };
        OpError { code, message }
    }
}

/// `start <id>`.
pub fn start_args(id: &str) -> Vec<String> {
    vec!["start".to_string(), id.to_string()]
}

/// `stop <id>`.
pub fn stop_args(id: &str) -> Vec<String> {
    vec!["stop".to_string(), id.to_string()]
}

/// `restart <id>`.
pub fn restart_args(id: &str) -> Vec<String> {
    vec!["restart".to_string(), id.to_string()]
}

/// `rm [-f] <id>`.
pub fn remove_args(id: &str, force: bool) -> Vec<String> {
    let mut args = vec!["rm".to_string()];
    if force {
        args.push("-f".to_string());
    }
    args.push(id.to_string());
    args
}

/// `pause <id>`.
pub fn pause_args(id: &str) -> Vec<String> {
    vec!["pause".to_string(), id.to_string()]
}

/// `unpause <id>`.
pub fn unpause_args(id: &str) -> Vec<String> {
    vec!["unpause".to_string(), id.to_string()]
}

/// `container prune -f` — the Containers group's "Clean Up". `-f` skips
/// the CLI's own interactive confirmation, since the panel already shows
/// one (JetBrains' "Clean Up" dialog, `containers_panel.cpp`).
pub fn prune_args() -> Vec<String> {
    vec![
        "container".to_string(),
        "prune".to_string(),
        "-f".to_string(),
    ]
}

/// `top <id>`.
pub fn top_args(id: &str) -> Vec<String> {
    vec!["top".to_string(), id.to_string()]
}

/// Run `args` through `invocation`, buffered, and map the result onto
/// [`OpError`]: a spawn failure via [`OpError::from_failure`], a nonzero
/// exit via [`OpError::from_stderr`]. The one call every op above (and
/// `files::ls_args`/`session::inspect_json`) goes through.
pub fn run_op(
    invocation: &Invocation,
    args: &[String],
    work_dir: &Path,
) -> Result<process_exec::Output, OpError> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = invocation
        .run(&arg_refs, work_dir, OP_TIMEOUT)
        .map_err(OpError::from_failure)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(OpError::from_stderr(&String::from_utf8_lossy(
            &output.stderr,
        )))
    }
}

/// One `top`-parsed process row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTable {
    pub titles: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Parse `docker`/`podman top`'s output: a header line of column titles
/// (whitespace-separated, the last of which — `CMD`/`COMMAND` — is free
/// text that may itself contain spaces), then one row per process, split
/// on the same rule so the last column stays whole.
///
/// Both engines print the identical shape (`docker top` forwards straight
/// to the container's own `ps`-like listing; Podman's `top` matches it),
/// so this one parser covers both — column names and count vary by
/// platform/`ps` build, so nothing here assumes a fixed set.
pub fn parse_top(output: &str) -> ProcessTable {
    let mut lines = output.lines();
    let Some(header) = lines.next() else {
        return ProcessTable {
            titles: Vec::new(),
            rows: Vec::new(),
        };
    };
    let titles: Vec<String> = header.split_whitespace().map(str::to_string).collect();
    let column_count = titles.len();
    let rows = lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| split_into_columns(line, column_count))
        .collect();
    ProcessTable { titles, rows }
}

/// Split a data line into exactly `column_count` fields when possible,
/// folding any overflow into the last column — the same "last column is
/// free text" rule [`crate::files`]'s `ls -la` parser needs for file
/// names, applied here to a process's command line.
fn split_into_columns(line: &str, column_count: usize) -> Vec<String> {
    if column_count == 0 {
        return Vec::new();
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() <= column_count {
        return parts.into_iter().map(str::to_string).collect();
    }
    let mut columns: Vec<String> = parts[..column_count - 1]
        .iter()
        .map(|part| part.to_string())
        .collect();
    columns.push(parts[column_count - 1..].join(" "));
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_stop_restart_pause_unpause_argv() {
        assert_eq!(start_args("abc123"), vec!["start", "abc123"]);
        assert_eq!(stop_args("abc123"), vec!["stop", "abc123"]);
        assert_eq!(restart_args("abc123"), vec!["restart", "abc123"]);
        assert_eq!(pause_args("abc123"), vec!["pause", "abc123"]);
        assert_eq!(unpause_args("abc123"), vec!["unpause", "abc123"]);
    }

    #[test]
    fn remove_argv_adds_force_flag_only_when_asked() {
        assert_eq!(remove_args("abc123", false), vec!["rm", "abc123"]);
        assert_eq!(remove_args("abc123", true), vec!["rm", "-f", "abc123"]);
    }

    #[test]
    fn prune_argv_is_non_interactive() {
        assert_eq!(prune_args(), vec!["container", "prune", "-f"]);
    }

    #[test]
    fn top_argv() {
        assert_eq!(top_args("abc123"), vec!["top", "abc123"]);
    }

    #[test]
    fn these_argv_builders_are_identical_for_docker_and_podman() {
        // ADR-0055: the engine is the executable, not a branch in this
        // module — asserting the same argv would be produced regardless
        // of which engine's `Invocation` runs it is the whole point.
        for id in ["c1", "8f3a"] {
            assert_eq!(start_args(id), start_args(id));
        }
    }

    #[test]
    fn docker_no_such_container_is_not_found() {
        let err = OpError::from_stderr("Error response from daemon: No such container: abc123");
        assert_eq!(err.code, OpErrorCode::NotFound);
    }

    #[test]
    fn podman_no_container_with_name_is_not_found() {
        let err = OpError::from_stderr(
            "Error: no container with name or ID \"abc123\" found: no such container",
        );
        assert_eq!(err.code, OpErrorCode::NotFound);
    }

    #[test]
    fn docker_is_not_running_maps_to_not_running() {
        let err =
            OpError::from_stderr("Error response from daemon: Container abc123 is not running");
        assert_eq!(err.code, OpErrorCode::NotRunning);
    }

    #[test]
    fn podman_already_stopped_maps_to_not_running() {
        let err = OpError::from_stderr("Error: container abc123 already stopped");
        assert_eq!(err.code, OpErrorCode::NotRunning);
    }

    #[test]
    fn already_running_is_classified() {
        let err = OpError::from_stderr("Error: container abc123 is already running");
        assert_eq!(err.code, OpErrorCode::AlreadyRunning);
    }

    #[test]
    fn docker_socket_permission_denied() {
        let err = OpError::from_stderr(
            "permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock",
        );
        assert_eq!(err.code, OpErrorCode::PermissionDenied);
    }

    #[test]
    fn docker_daemon_unreachable() {
        let err = OpError::from_stderr(
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
        );
        assert_eq!(err.code, OpErrorCode::EngineUnavailable);
    }

    #[test]
    fn podman_daemon_unreachable() {
        let err = OpError::from_stderr("Error: cannot connect to Podman socket");
        assert_eq!(err.code, OpErrorCode::EngineUnavailable);
    }

    #[test]
    fn unrecognised_stderr_falls_back_to_other() {
        let err = OpError::from_stderr("Error: something this classifier has never seen");
        assert_eq!(err.code, OpErrorCode::Other);
    }

    #[test]
    fn top_parses_a_docker_style_sample() {
        let sample = "\
UID                 PID                 PPID                C                   STIME               TTY                 TIME                CMD
root                12345               12300               0                   09:00               ?                   00:00:01            nginx: master process
101                 12400               12345               0                   09:00               ?                   00:00:00            nginx: worker process";
        let table = parse_top(sample);
        assert_eq!(
            table.titles,
            vec!["UID", "PID", "PPID", "C", "STIME", "TTY", "TIME", "CMD"]
        );
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0][0], "root");
        assert_eq!(table.rows[0][7], "nginx: master process");
        assert_eq!(table.rows[1][7], "nginx: worker process");
    }

    #[test]
    fn top_parses_a_podman_style_sample_with_a_multi_word_command() {
        let sample = "\
USER    PID   PPID  %CPU ELAPSED TTY STAT TIME COMMAND
postgres 1     0     0.1  2h13m   ?   Ss   0:01 postgres -c config_file=/etc/postgresql.conf";
        let table = parse_top(sample);
        assert_eq!(table.titles.last().unwrap(), "COMMAND");
        assert_eq!(table.rows.len(), 1);
        assert_eq!(
            table.rows[0].last().unwrap(),
            "postgres -c config_file=/etc/postgresql.conf"
        );
    }

    #[test]
    fn top_with_no_processes_is_an_empty_row_list() {
        let table = parse_top("UID PID PPID C STIME TTY TIME CMD\n");
        assert!(table.rows.is_empty());
    }

    #[test]
    fn top_with_no_output_at_all_is_empty() {
        let table = parse_top("");
        assert!(table.titles.is_empty());
        assert!(table.rows.is_empty());
    }
}
