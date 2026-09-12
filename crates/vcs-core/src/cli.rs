//! The `git` subprocess layer (F3-5).
//!
//! Everything that honours the user's configuration, credentials, hooks or
//! signing shells out here rather than going through `gix` — see ADR-0031.
//! This module is the one place `vcs-core` spawns a process; `staging`,
//! `commit`, `branch` and `remote` build argv with [`argv`] and run it with
//! [`run`], never `std::process::Command` directly, so `GIT_TERMINAL_PROMPT`,
//! the timeout and the stderr-to-sentence conversion apply everywhere.

use std::path::Path;
use std::time::Duration;

use process_exec::Failure;

use crate::error::VcsError;

/// How long a `git` subprocess gets before this crate gives up waiting on
/// it. Generous: `fetch`/`push` are network calls, not local reads.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// Run `git <args>` in `work_dir` and return its stdout as text.
///
/// `GIT_TERMINAL_PROMPT=0` is always set, so a missing credential fails
/// fast with a message on stderr instead of blocking forever on a prompt
/// nothing can answer. A missing `git` binary is
/// [`VcsError::GitNotInstalled`], never folded into some other failure.
pub fn run(work_dir: &Path, args: &[&str]) -> Result<String, VcsError> {
    run_internal(work_dir, args, None, TIMEOUT)
}

/// [`run`], but writing `stdin_text` to the child's stdin first — the shape
/// `staging::stage_hunk` needs to feed a generated patch to
/// `git apply --cached -`.
pub fn run_with_stdin(
    work_dir: &Path,
    args: &[&str],
    stdin_text: &str,
) -> Result<String, VcsError> {
    run_internal(work_dir, args, Some(stdin_text), TIMEOUT)
}

/// [`run`] with an explicit timeout, so tests can exercise the timeout path
/// without waiting out the real [`TIMEOUT`].
#[cfg(test)]
fn run_with_timeout(work_dir: &Path, args: &[&str], timeout: Duration) -> Result<String, VcsError> {
    run_internal(work_dir, args, None, timeout)
}

fn run_internal(
    work_dir: &Path,
    args: &[&str],
    stdin_text: Option<&str>,
    timeout: Duration,
) -> Result<String, VcsError> {
    let command_str = display_command(args);

    // `GIT_TERMINAL_PROMPT=0` and the git-specific error mapping stay here;
    // `process_exec::run` is just the spawn/pipe/drain/wait/kill mechanism
    // (B1 of the PHP tooling plan — the same mechanism an analyzer or test
    // runner needs, extracted rather than copied).
    let output = process_exec::run(
        "git",
        args,
        work_dir,
        stdin_text.map(str::as_bytes),
        timeout,
        &[("GIT_TERMINAL_PROMPT", "0")],
    )
    .map_err(|failure| match failure {
        Failure::NotFound => VcsError::GitNotInstalled,
        Failure::TimedOut => VcsError::GitTimedOut {
            command: command_str.clone(),
        },
        Failure::Io(message) => VcsError::Read(message),
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if let Some(path) = dubious_ownership_path(&stderr) {
            return Err(VcsError::DubiousOwnership { path });
        }
        return Err(VcsError::GitFailed {
            command: command_str,
            stderr,
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Extract the repository path from git's own "dubious ownership" message,
/// e.g.:
/// ```text
/// fatal: detected dubious ownership in repository at '/wsl.localhost/Ubuntu/home/florian/projects/ide'
/// ```
/// `git` always wraps the path in quotes here (single on Unix, and it has
/// used double quotes on some Windows builds), so this looks for the marker
/// phrase and takes whatever is between the quote character that follows it.
/// Returns `None` for any stderr that doesn't match — the caller falls back
/// to the generic [`VcsError::GitFailed`].
fn dubious_ownership_path(stderr: &str) -> Option<std::path::PathBuf> {
    const MARKER: &str = "detected dubious ownership in repository at ";
    let after_marker = &stderr[stderr.find(MARKER)? + MARKER.len()..];
    let quote = after_marker.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &after_marker[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(std::path::PathBuf::from(&rest[..end]))
}

fn display_command(args: &[&str]) -> String {
    let mut s = String::from("git");
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// Argv construction for the write/network operations, kept separate from
/// [`run`] so the shape of a command can be asserted on without a real
/// `git` binary. `staging`, `commit`, `branch` and `remote` build on these.
pub mod argv {
    /// `git --no-optional-locks status --porcelain=v2 -z --branch
    /// --untracked-files=all --renames` — one command yielding
    /// branch/upstream/ahead-behind plus every changed path, renames
    /// included, that `crate::status`'s `parse_porcelain_v2` expects.
    ///
    /// `--no-optional-locks` (a global flag, so it precedes the `status`
    /// subcommand) stops this read from taking the index lock at all —
    /// `git status` refreshes the on-disk index by default, which writes
    /// `.git/index.lock` then `.git/index`. Those writes fire the project
    /// watcher on `.git`, which used to re-trigger this same refresh: issue
    /// #285's ~8/s rebuild loop on every opened repository. A stat-only
    /// status without the refresh is what every other IDE's polling status
    /// check does for the same reason.
    pub fn status() -> Vec<&'static str> {
        vec![
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
            "--renames",
        ]
    }

    /// `git add -- <paths>`.
    pub fn add<'a>(paths: &'a [&str]) -> Vec<&'a str> {
        let mut args = vec!["add", "--"];
        args.extend_from_slice(paths);
        args
    }

    /// `git reset -- <paths>` — unstage without touching the working tree.
    pub fn reset<'a>(paths: &'a [&str]) -> Vec<&'a str> {
        let mut args = vec!["reset", "--"];
        args.extend_from_slice(paths);
        args
    }

    /// `git add -A` — stage every change in the repository, tracked or not.
    pub fn add_all() -> Vec<&'static str> {
        vec!["add", "-A"]
    }

    /// `git reset` — unstage everything, leaving the working tree and
    /// `HEAD` untouched. The whole-repository counterpart to [`reset`].
    pub fn reset_all() -> Vec<&'static str> {
        vec!["reset"]
    }

    /// `git apply --cached [--reverse] -` (patch text goes on stdin).
    pub fn apply_cached(reverse: bool) -> Vec<&'static str> {
        if reverse {
            vec!["apply", "--cached", "--reverse", "-"]
        } else {
            vec!["apply", "--cached", "-"]
        }
    }

    /// `git commit -m <message> [--amend]`.
    pub fn commit(message: &str, amend: bool) -> Vec<&str> {
        let mut args = vec!["commit", "-m", message];
        if amend {
            args.push("--amend");
        }
        args
    }

    /// `git branch <name> [<start-point>]`.
    pub fn branch_create<'a>(name: &'a str, start_point: Option<&'a str>) -> Vec<&'a str> {
        let mut args = vec!["branch", name];
        if let Some(start) = start_point {
            args.push(start);
        }
        args
    }

    /// `git checkout <name>`.
    pub fn checkout(name: &str) -> Vec<&str> {
        vec!["checkout", name]
    }

    /// `git checkout HEAD -- <paths>` — discard a path's changes in both the
    /// index and the working tree.
    ///
    /// `HEAD` rather than a bare `git checkout -- <paths>`: without a tree
    /// argument `git` restores from the *index*, which leaves a staged
    /// change in place and makes "revert this file" mean something different
    /// depending on whether the user had staged it. Naming `HEAD` says what
    /// the caller means — put this file back the way the last commit has it.
    pub fn checkout_paths<'a>(paths: &'a [&str]) -> Vec<&'a str> {
        let mut args = vec!["checkout", "HEAD", "--"];
        args.extend_from_slice(paths);
        args
    }

    /// `git branch -d|-D <name>`.
    pub fn branch_delete(name: &str, force: bool) -> Vec<&str> {
        vec!["branch", if force { "-D" } else { "-d" }, name]
    }

    /// `git fetch <remote>`.
    pub fn fetch(remote: &str) -> Vec<&str> {
        vec!["fetch", remote]
    }

    /// `git pull <remote> <branch>`.
    pub fn pull<'a>(remote: &'a str, branch: &'a str) -> Vec<&'a str> {
        vec!["pull", remote, branch]
    }

    /// `git push [-u] <remote> <branch>`.
    pub fn push<'a>(remote: &'a str, branch: &'a str, set_upstream: bool) -> Vec<&'a str> {
        if set_upstream {
            vec!["push", "-u", remote, branch]
        } else {
            vec!["push", remote, branch]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn init_repo(dir: &Path) {
        let status = StdCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir)
            .status()
            .expect("git must be on PATH for these tests");
        assert!(status.success());
    }

    // -----------------------------------------------------------------
    // argv construction — no git binary needed.
    // -----------------------------------------------------------------

    #[test]
    fn status_argv() {
        assert_eq!(
            argv::status(),
            vec![
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--untracked-files=all",
                "--renames",
            ]
        );
    }

    #[test]
    fn add_argv() {
        assert_eq!(
            argv::add(&["a.txt", "b.txt"]),
            vec!["add", "--", "a.txt", "b.txt"]
        );
    }

    #[test]
    fn reset_argv() {
        assert_eq!(
            argv::reset(&["a.txt", "b.txt"]),
            vec!["reset", "--", "a.txt", "b.txt"]
        );
    }

    #[test]
    fn add_all_argv() {
        assert_eq!(argv::add_all(), vec!["add", "-A"]);
    }

    #[test]
    fn reset_all_argv() {
        assert_eq!(argv::reset_all(), vec!["reset"]);
    }

    #[test]
    fn commit_argv_with_and_without_amend() {
        assert_eq!(argv::commit("msg", false), vec!["commit", "-m", "msg"]);
        assert_eq!(
            argv::commit("msg", true),
            vec!["commit", "-m", "msg", "--amend"]
        );
    }

    #[test]
    fn apply_cached_argv_reverse_toggles_unstage() {
        assert_eq!(argv::apply_cached(false), vec!["apply", "--cached", "-"]);
        assert_eq!(
            argv::apply_cached(true),
            vec!["apply", "--cached", "--reverse", "-"]
        );
    }

    #[test]
    fn branch_delete_argv_force_flag() {
        assert_eq!(
            argv::branch_delete("feature", false),
            vec!["branch", "-d", "feature"]
        );
        assert_eq!(
            argv::branch_delete("feature", true),
            vec!["branch", "-D", "feature"]
        );
    }

    #[test]
    fn push_argv_set_upstream() {
        assert_eq!(
            argv::push("origin", "main", false),
            vec!["push", "origin", "main"]
        );
        assert_eq!(
            argv::push("origin", "main", true),
            vec!["push", "-u", "origin", "main"]
        );
    }

    // -----------------------------------------------------------------
    // dubious-ownership stderr parsing — no git binary needed.
    // -----------------------------------------------------------------

    #[test]
    fn dubious_ownership_path_extracts_the_quoted_path() {
        let stderr = "fatal: detected dubious ownership in repository at \
            '/wsl.localhost/Ubuntu/home/florian/projects/ide'\n\
            To add an exception for this directory, call:\n\n\
            \tgit config --global --add safe.directory /wsl.localhost/Ubuntu/home/florian/projects/ide";
        assert_eq!(
            dubious_ownership_path(stderr),
            Some(std::path::PathBuf::from(
                "/wsl.localhost/Ubuntu/home/florian/projects/ide"
            ))
        );
    }

    #[test]
    fn dubious_ownership_path_is_none_for_unrelated_stderr() {
        assert_eq!(dubious_ownership_path("fatal: not a git repository"), None);
    }

    // -----------------------------------------------------------------
    // `run` against a real git binary.
    // -----------------------------------------------------------------

    #[test]
    fn run_captures_stdout_on_success() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let out = run(dir.path(), &["status", "--porcelain"]).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn run_turns_a_nonzero_exit_into_a_readable_error() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let err = run(dir.path(), &["this-is-not-a-git-command"]).unwrap_err();
        match err {
            VcsError::GitFailed { command, stderr } => {
                assert!(command.contains("this-is-not-a-git-command"));
                assert!(!stderr.is_empty());
            }
            other => panic!("expected GitFailed, got {other:?}"),
        }
    }

    #[test]
    fn run_times_out_on_a_command_that_hangs() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        // `git`'s own `-c` accepts arbitrary config, so this is a real
        // `git` invocation, not a shell trick — `sleep` isn't a git
        // subcommand, so route through `GIT_SSH_COMMAND`-style indirection
        // is unnecessary: a nonexistent pager that blocks would work too,
        // but the simplest hang is a real subprocess that outlives the
        // test's patience. `git`'s `hook` runner will happily exec anything
        // on PATH, so a hook script that sleeps proves the same timeout
        // path a real stuck hook would hit.
        let hooks_dir = dir.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\nsleep 5\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        run(dir.path(), &["config", "user.email", "test@example.com"]).unwrap();
        run(dir.path(), &["config", "user.name", "Test"]).unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(dir.path(), &["add", "a.txt"]).unwrap();

        // A 60s TIMEOUT would make this test itself hang for a minute, so
        // this asserts the mechanism using a short local override instead
        // of the real constant.
        let short_timeout = Duration::from_millis(200);
        let result = run_with_timeout(dir.path(), &["commit", "-m", "x"], short_timeout);
        assert!(matches!(result, Err(VcsError::GitTimedOut { .. })));
    }
}
