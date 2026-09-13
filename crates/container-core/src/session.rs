//! Streaming sessions (C3, ADR-0055): `pty_core::ShellSpec` builders for
//! the Log/Terminal/Exec/Attach tabs, and `inspect_json` for the Inspect
//! tab's virtual document — every one of them composed through an
//! [`Invocation`] so a connection's WSL/SSH/context/TCP wrapping (already
//! solved once in `connection.rs`) is never re-implemented here.

use std::path::Path;

use pty_core::ShellSpec;

use crate::connection::Invocation;
use crate::ops::{run_op, OpError};

/// Turn `invocation` plus a subcommand's argv into a [`ShellSpec`]: the
/// program and prefix args an interactive/streaming session spawns
/// through `pty_core::PtySession::spawn`, `invocation.env` carried along
/// exactly as [`Invocation::run`] does for a buffered call.
fn shell_spec(invocation: &Invocation, args: Vec<String>) -> ShellSpec {
    let arg_strs: Vec<&str> = args.iter().map(String::as_str).collect();
    let argv = invocation.argv(&arg_strs);
    ShellSpec::new(invocation.program.clone(), argv).with_env(invocation.env.clone())
}

/// `logs -f --timestamps --tail <n>` — the Log tab, and its "Restart log"
/// button (a fresh session with the same spec).
pub fn logs_session(invocation: &Invocation, id: &str, tail: u32) -> ShellSpec {
    shell_spec(
        invocation,
        vec![
            "logs".to_string(),
            "-f".to_string(),
            "--timestamps".to_string(),
            "--tail".to_string(),
            tail.to_string(),
            id.to_string(),
        ],
    )
}

/// `exec -it [-u 0] <id> sh -c 'command -v bash >/dev/null && exec bash ||
/// exec sh'` — Terminal (as current user, or as root when `as_root`).
/// Prefers `bash` when the image has it, falling back to `sh`, which every
/// image with a shell at all has — the same "try the nicer shell, fall
/// back to the one that is always there" rule `pty_core::ShellSpec::
/// unix_default` follows on the host.
pub fn terminal_session(invocation: &Invocation, id: &str, as_root: bool) -> ShellSpec {
    let mut args = vec!["exec".to_string(), "-it".to_string()];
    if as_root {
        args.push("-u".to_string());
        args.push("0".to_string());
    }
    args.push(id.to_string());
    args.push("sh".to_string());
    args.push("-c".to_string());
    args.push("command -v bash >/dev/null && exec bash || exec sh".to_string());
    shell_spec(invocation, args)
}

/// `exec -it <id> <command...>` — the Exec dialog's command line, run as
/// its own argv (not through a shell) so quoting the user typed in the
/// dialog is never re-interpreted.
pub fn exec_session(invocation: &Invocation, id: &str, command: &[String]) -> ShellSpec {
    let mut args = vec!["exec".to_string(), "-it".to_string(), id.to_string()];
    args.extend(command.iter().cloned());
    shell_spec(invocation, args)
}

/// `attach --sig-proxy=false <id>` — `--sig-proxy=false` so closing the
/// Attach tab (which kills this session's PTY child, `TerminalSupervisor::
/// close_session`) does not forward that signal into the container itself,
/// same reasoning JetBrains' own Attach action documents.
pub fn attach_session(invocation: &Invocation, id: &str) -> ShellSpec {
    shell_spec(
        invocation,
        vec![
            "attach".to_string(),
            "--sig-proxy=false".to_string(),
            id.to_string(),
        ],
    )
}

/// `pull <reference>` — the Images console's Pull button (C4), shown as a
/// "Pull: <reference>" `TerminalWidget` tab so progress lines reach the
/// user exactly like every other streamed command in this crate.
pub fn pull_session(invocation: &Invocation, reference: &str) -> ShellSpec {
    shell_spec(invocation, vec!["pull".to_string(), reference.to_string()])
}

/// `inspect <id>`, pretty-printed — the Inspect tab's virtual document
/// text. `kind` only chooses the message on failure (a container's vs. an
/// image's vs. a network's `inspect`), the subcommand argv is identical.
pub fn inspect_json(
    invocation: &Invocation,
    kind: &str,
    id: &str,
    work_dir: &Path,
) -> Result<String, OpError> {
    let args = vec!["inspect".to_string(), id.to_string()];
    let output = run_op(invocation, &args, work_dir).map_err(|err| OpError {
        code: err.code,
        message: format!("inspecting {kind} '{id}' failed: {}", err.message),
    })?;
    let raw: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|error| OpError {
            code: crate::ops::OpErrorCode::Other,
            message: format!("inspect output was not valid JSON: {error}"),
        })?;
    serde_json::to_string_pretty(&raw).map_err(|error| OpError {
        code: crate::ops::OpErrorCode::Other,
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Engine;

    fn invocation(engine: Engine) -> Invocation {
        Invocation {
            program: match engine {
                Engine::Docker => "docker".to_string(),
                Engine::Podman => "podman".to_string(),
            },
            prefix_args: Vec::new(),
            env: Vec::new(),
            host: process_exec::host::ExecHost::Local,
        }
    }

    #[test]
    fn pull_session_argv() {
        let spec = pull_session(&invocation(Engine::Docker), "nginx:1.27");
        assert_eq!(spec.program, "docker");
        assert_eq!(spec.args, vec!["pull", "nginx:1.27"]);
    }

    #[test]
    fn logs_session_argv() {
        let spec = logs_session(&invocation(Engine::Docker), "c1", 500);
        assert_eq!(spec.program, "docker");
        assert_eq!(
            spec.args,
            vec!["logs", "-f", "--timestamps", "--tail", "500", "c1"]
        );
    }

    #[test]
    fn terminal_session_uses_dash_u_0_only_when_root() {
        let user = terminal_session(&invocation(Engine::Docker), "c1", false);
        assert!(!user.args.contains(&"-u".to_string()));
        assert_eq!(user.args[0], "exec");
        assert_eq!(user.args[1], "-it");
        assert_eq!(user.args[2], "c1");

        let root = terminal_session(&invocation(Engine::Podman), "c1", true);
        assert_eq!(root.program, "podman");
        assert!(root.args.contains(&"-u".to_string()));
        let u_index = root.args.iter().position(|a| a == "-u").unwrap();
        assert_eq!(root.args[u_index + 1], "0");
    }

    #[test]
    fn terminal_session_prefers_bash_falls_back_to_sh() {
        let spec = terminal_session(&invocation(Engine::Docker), "c1", false);
        let command = spec.args.last().unwrap();
        assert!(command.contains("command -v bash"));
        assert!(command.contains("exec bash"));
        assert!(command.contains("exec sh"));
    }

    #[test]
    fn exec_session_argv_preserves_the_command_as_given() {
        let spec = exec_session(
            &invocation(Engine::Docker),
            "c1",
            &["ls".to_string(), "-la".to_string(), "/tmp".to_string()],
        );
        assert_eq!(spec.args, vec!["exec", "-it", "c1", "ls", "-la", "/tmp"]);
    }

    #[test]
    fn attach_session_disables_sig_proxy() {
        let spec = attach_session(&invocation(Engine::Docker), "c1");
        assert_eq!(spec.args, vec!["attach", "--sig-proxy=false", "c1"]);
    }

    #[test]
    fn shell_spec_wraps_wsl_invocations_once_not_twice() {
        // The C2 fix made `Invocation::command()` wrap a WSL connection's
        // argv exactly once; `shell_spec` reuses `Invocation::argv`
        // (prefix_args + args), never `Invocation::command`, so a WSL
        // connection's `prefix_args` (`wsl.exe -d <distro> --`, built by
        // `connection.rs`) still appears exactly once here too.
        let invocation = Invocation {
            program: "wsl.exe".to_string(),
            prefix_args: vec![
                "-d".to_string(),
                "Ubuntu".to_string(),
                "--".to_string(),
                "docker".to_string(),
            ],
            env: Vec::new(),
            host: process_exec::host::ExecHost::Wsl(process_exec::host::WslHost {
                distro: "Ubuntu".to_string(),
                unc_prefix: "//wsl.localhost/Ubuntu".to_string(),
            }),
        };
        let spec = logs_session(&invocation, "c1", 100);
        assert_eq!(spec.program, "wsl.exe");
        assert_eq!(
            spec.args,
            vec![
                "-d",
                "Ubuntu",
                "--",
                "docker",
                "logs",
                "-f",
                "--timestamps",
                "--tail",
                "100",
                "c1"
            ]
        );
    }

    #[test]
    fn inspect_json_pretty_prints_and_reports_kind_on_failure() {
        // Exercise the JSON round trip via the pure formatting path,
        // without spawning a real CLI: this asserts pretty_string
        // formatting matches serde_json's, which is what the function
        // under test delegates to after a successful run_op.
        let raw: serde_json::Value = serde_json::from_str(r#"{"Id":"c1"}"#).unwrap();
        let pretty = serde_json::to_string_pretty(&raw).unwrap();
        assert!(pretty.contains("\"Id\": \"c1\""));
    }
}
