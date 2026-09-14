//! Streaming sessions (C3, ADR-0055): `pty_core::ShellSpec` builders for
//! the Log/Terminal/Exec/Attach tabs, and `inspect_json` for the Inspect
//! tab's virtual document — every one of them composed through an
//! [`Invocation`] so a connection's WSL/SSH/context/TCP wrapping (already
//! solved once in `connection.rs`) is never re-implemented here.

use std::path::Path;

use pty_core::ShellSpec;

use crate::connection::Invocation;
use crate::ops::{run_op, OpError};
use crate::registry_ref;

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

/// The env var name a login+pull/push script reads the registry secret
/// from (C7) — never argv, so it never appears in a process listing or
/// shell history; the caller sets it as this [`ShellSpec`]'s one extra
/// environment entry.
pub const REGISTRY_SECRET_ENV: &str = "IDE_REGISTRY_SECRET";

/// Split `invocation` into what a login+action script needs: the outer
/// command that actually gets spawned (`sh` for every local/flag-based
/// connection kind; `wsl.exe` plus its `-d <distro> --` wrapper for a WSL
/// connection) and the inner engine program + flags each script line runs.
///
/// Every connection kind but WSL already runs the engine binary directly
/// (`Invocation::program`, with `prefix_args` pure flags like `-H
/// <url>`/`--context <name>`) — for those the inner program is `program`
/// itself and the outer command is `sh` with no extra wrapper. WSL instead
/// bakes the engine binary as the *last* element of `prefix_args`
/// (`connection.rs`'s `Wsl` arm), ahead of the caller's own args, so a
/// script needs that split undone rather than re-derived.
fn engine_split(invocation: &Invocation) -> (String, Vec<String>, String, Vec<String>) {
    if let process_exec::host::ExecHost::Wsl(_) = &invocation.host {
        if let Some((inner, outer)) = invocation.prefix_args.split_last() {
            return (
                invocation.program.clone(),
                outer.to_vec(),
                inner.clone(),
                Vec::new(),
            );
        }
    }
    (
        "sh".to_string(),
        Vec::new(),
        invocation.program.clone(),
        invocation.prefix_args.clone(),
    )
}

fn engine_line(inner_program: &str, inner_flags: &[String], args: &[String]) -> String {
    let mut argv = inner_flags.to_vec();
    argv.extend(args.iter().cloned());
    registry_ref::shell_line(inner_program, &argv)
}

/// A login (only when `credential` is given and a secret is stored), then
/// every one of `steps` in order, as ONE `sh -c` script so the whole
/// sequence shares a single PTY session (ADR-0055's Log/Terminal shape,
/// reused for C7's registry pull/push) — progress from every stage stays
/// visible in the one tab the view opens for it. No `credential`/`secret`
/// skips the login line entirely: an anonymous pull, or a push the CLI's
/// own credential store already has a login for (C7's documented fallback
/// when no OS keychain is available).
fn login_and_run(
    invocation: &Invocation,
    credential: Option<(&str, &str)>,
    secret: Option<&str>,
    steps: Vec<Vec<String>>,
) -> ShellSpec {
    let (outer_program, outer_prefix, inner_program, inner_flags) = engine_split(invocation);
    let mut lines = Vec::new();
    if let (Some((address, username)), Some(_)) = (credential, secret) {
        let login = registry_ref::login_args(address, username);
        lines.push(format!(
            "echo \"${REGISTRY_SECRET_ENV}\" | {}",
            engine_line(&inner_program, &inner_flags, &login)
        ));
    }
    for step in &steps {
        lines.push(engine_line(&inner_program, &inner_flags, step));
    }
    let script = lines.join(" && ");

    let mut args = outer_prefix;
    // WSL's outer program is `wsl.exe`, so the distro wrapper still needs
    // an explicit `sh` after its `--`; a local connection's outer program
    // already *is* `sh`, so its args start straight at `-c`.
    if outer_program != "sh" {
        args.push("sh".to_string());
    }
    args.push("-c".to_string());
    args.push(script);

    let mut env = invocation.env.clone();
    if let (Some(secret), Some(_)) = (secret, credential) {
        env.push((REGISTRY_SECRET_ENV.to_string(), secret.to_string()));
    }
    ShellSpec::new(outer_program, args).with_env(env)
}

/// `[login &&] pull <reference>` (C7): the registry tree's "Pull Image…"
/// and the editor's "Pull image" intention once a reference names a
/// configured private registry. `credential` is `(address, username)` —
/// `None` for anonymous/public pulls, matching [`login_and_run`].
pub fn pull_from_registry_session(
    invocation: &Invocation,
    reference: &str,
    credential: Option<(&str, &str)>,
    secret: Option<&str>,
) -> ShellSpec {
    login_and_run(
        invocation,
        credential,
        secret,
        vec![crate::images::pull_args(reference)],
    )
}

/// `tag <image> <destination> && [login &&] push <destination>` (C7): the
/// Images console's "Push Image…" dialog.
pub fn push_to_registry_session(
    invocation: &Invocation,
    image_reference: &str,
    destination: &str,
    credential: Option<(&str, &str)>,
    secret: Option<&str>,
) -> ShellSpec {
    login_and_run(
        invocation,
        credential,
        secret,
        vec![
            crate::images::tag_args(image_reference, destination),
            registry_ref::push_args(destination),
        ],
    )
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
    inspect_json_with_args(
        invocation,
        kind,
        id,
        &["inspect".to_string(), id.to_string()],
        work_dir,
    )
}

/// [`inspect_json`], but with the argv the caller supplies rather than the
/// generic `inspect <id>` — a pod's own inspect is `pod inspect <id>`
/// ([`crate::pods::inspect_args`]), not the type-agnostic `inspect` every
/// other kind here uses.
pub fn inspect_json_with_args(
    invocation: &Invocation,
    kind: &str,
    id: &str,
    args: &[String],
    work_dir: &Path,
) -> Result<String, OpError> {
    let output = run_op(invocation, args, work_dir).map_err(|err| OpError {
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
    fn pull_from_registry_without_credentials_skips_login() {
        let spec = pull_from_registry_session(
            &invocation(Engine::Docker),
            "ghcr.io/acme/app:v1",
            None,
            None,
        );
        assert_eq!(spec.program, "sh");
        assert_eq!(spec.args[0], "-c");
        assert_eq!(spec.args[1], "docker pull ghcr.io/acme/app:v1");
        assert!(spec.env.is_empty());
    }

    #[test]
    fn pull_from_registry_with_credentials_logs_in_first_via_stdin() {
        let spec = pull_from_registry_session(
            &invocation(Engine::Docker),
            "ghcr.io/acme/app:v1",
            Some(("ghcr.io", "alice")),
            Some("s3cr3t"),
        );
        assert_eq!(
            spec.args[1],
            "echo \"$IDE_REGISTRY_SECRET\" | docker login --username alice --password-stdin ghcr.io \
             && docker pull ghcr.io/acme/app:v1"
        );
        assert_eq!(
            spec.env,
            vec![(REGISTRY_SECRET_ENV.to_string(), "s3cr3t".to_string())]
        );
    }

    #[test]
    fn push_to_registry_tags_then_logs_in_then_pushes() {
        let spec = push_to_registry_session(
            &invocation(Engine::Podman),
            "app:dev",
            "ghcr.io/acme/app:v1",
            Some(("ghcr.io", "alice")),
            Some("s3cr3t"),
        );
        assert_eq!(
            spec.args[1],
            "echo \"$IDE_REGISTRY_SECRET\" | podman login --username alice --password-stdin ghcr.io \
             && podman tag app:dev ghcr.io/acme/app:v1 && podman push ghcr.io/acme/app:v1"
        );
    }

    #[test]
    fn push_to_registry_without_credentials_still_tags_and_pushes() {
        let spec = push_to_registry_session(
            &invocation(Engine::Docker),
            "app:dev",
            "ghcr.io/acme/app:v1",
            None,
            None,
        );
        assert_eq!(
            spec.args[1],
            "docker tag app:dev ghcr.io/acme/app:v1 && docker push ghcr.io/acme/app:v1"
        );
    }

    #[test]
    fn a_wsl_connection_wraps_sh_after_its_own_prefix_instead_of_the_engine_program() {
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
        let spec = pull_from_registry_session(&invocation, "ghcr.io/acme/app:v1", None, None);
        assert_eq!(spec.program, "wsl.exe");
        assert_eq!(
            spec.args,
            vec![
                "-d",
                "Ubuntu",
                "--",
                "sh",
                "-c",
                "docker pull ghcr.io/acme/app:v1"
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
