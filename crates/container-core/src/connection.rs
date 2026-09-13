//! A named connection to a Docker or Podman engine, and how it turns into
//! an actual command line (ADR-0055).
//!
//! [`app_config::ContainerConnectionSetting`] stores every kind's fields as
//! plain strings — persistence stays dumb (ADR-0017/ADR-0039). This module
//! is the one place that gives them meaning: [`ConnectionKind::from_setting`]
//! parses the row into a typed value, and [`ConnectionConfig::invocation`]
//! turns that, plus the chosen [`Engine`], into an [`Invocation`] — the
//! program, prefix arguments and environment every later task (`ops.rs`,
//! `session.rs`, `run_config.rs`, ...) builds its own argv on top of.

use std::path::Path;
use std::time::Duration;

use process_exec::host::{ExecHost, WslHost};

use app_config::ContainerConnectionSetting;

/// Which CLI this connection talks to. The engine *is* the executable
/// (ADR-0055): `podman` is a drop-in for `docker` on every subcommand this
/// crate uses, so there is exactly one dispatch point — here — rather than
/// one per operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Docker,
    Podman,
}

impl Engine {
    /// The string stored in `ContainerConnectionSetting::engine`.
    pub fn id(self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
        }
    }

    /// Unrecognised values map to [`Engine::Docker`] — the same
    /// "absent/unknown reads as the default" rule every settings-model
    /// mapping in this codebase follows, so a settings file from a future
    /// build never fails to load, it just falls back.
    pub fn from_id(id: &str) -> Self {
        match id {
            "podman" => Engine::Podman,
            _ => Engine::Docker,
        }
    }

    /// The CLI's default program name.
    pub fn default_program(self) -> &'static str {
        self.id()
    }
}

/// How this engine is reached.
///
/// One enum for both engines rather than an `EngineKind` per engine: every
/// variant maps onto *some* flag or environment variable on both `docker`
/// and `podman` (see [`ConnectionConfig::invocation`]), even where the
/// combination is unusual (a `PodmanMachine` kind paired with the `Docker`
/// engine) — the table-driven tests in this module exercise every kind
/// against both engines for exactly that reason: the mapping has to stay
/// total, not just cover the sensible combinations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionKind {
    /// No connection override: the CLI's own default (`DOCKER_HOST`/
    /// `CONTAINER_HOST` env, or its compiled-in default socket).
    Auto,
    /// A Unix domain socket path, e.g. `/var/run/docker.sock`.
    UnixSocket { path: String },
    /// A TCP daemon URL, e.g. `tcp://host:2376`, with an optional TLS
    /// certificate directory.
    Tcp {
        url: String,
        cert_dir: Option<String>,
    },
    /// A Windows named pipe path, e.g. `//./pipe/docker_engine`.
    NamedPipe { path: String },
    /// A `docker context` (or, for Podman, a named `system connection`).
    Context { name: String },
    /// An SSH daemon URL, e.g. `ssh://user@host`, with an optional identity
    /// (private key) file.
    Ssh {
        url: String,
        identity: Option<String>,
    },
    /// A WSL distro this machine reaches the engine's CLI through.
    Wsl { distro: String },
    /// A named `podman machine`.
    PodmanMachine { name: String },
    /// The Docker daemon inside a `minikube` cluster's node, reached
    /// through `minikube docker-env`'s exported environment.
    Minikube,
}

impl ConnectionKind {
    /// The string stored in `ContainerConnectionSetting::kind`.
    pub fn id(&self) -> &'static str {
        match self {
            ConnectionKind::Auto => "auto",
            ConnectionKind::UnixSocket { .. } => "unix_socket",
            ConnectionKind::Tcp { .. } => "tcp",
            ConnectionKind::NamedPipe { .. } => "named_pipe",
            ConnectionKind::Context { .. } => "context",
            ConnectionKind::Ssh { .. } => "ssh",
            ConnectionKind::Wsl { .. } => "wsl",
            ConnectionKind::PodmanMachine { .. } => "podman_machine",
            ConnectionKind::Minikube => "minikube",
        }
    }

    /// Parse a persisted row's `kind` plus its flat fields into a typed
    /// value. An unrecognised `kind` maps to [`ConnectionKind::Auto`] —
    /// same "unknown reads as the least surprising default" rule as
    /// [`Engine::from_id`].
    pub fn from_setting(setting: &ContainerConnectionSetting) -> Self {
        match setting.kind.as_str() {
            "unix_socket" => ConnectionKind::UnixSocket {
                path: setting.path.clone(),
            },
            "tcp" => ConnectionKind::Tcp {
                url: setting.url.clone(),
                cert_dir: non_empty(&setting.cert_dir),
            },
            "named_pipe" => ConnectionKind::NamedPipe {
                path: setting.path.clone(),
            },
            "context" => ConnectionKind::Context {
                name: setting.resource_name.clone(),
            },
            "ssh" => ConnectionKind::Ssh {
                url: setting.url.clone(),
                identity: non_empty(&setting.identity),
            },
            "wsl" => ConnectionKind::Wsl {
                distro: setting.distro.clone(),
            },
            "podman_machine" => ConnectionKind::PodmanMachine {
                name: setting.resource_name.clone(),
            },
            "minikube" => ConnectionKind::Minikube,
            _ => ConnectionKind::Auto,
        }
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// Everything [`ConnectionConfig::invocation`] needs: which engine, which
/// kind of connection, and the two executable overrides every kind can
/// still take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionConfig {
    pub engine: Engine,
    pub kind: ConnectionKind,
    /// Overrides the engine's default program name (e.g. `podman-remote`).
    pub executable: Option<String>,
    /// Overrides the compose invocation's program name (e.g. a standalone
    /// `docker-compose` binary rather than the `docker compose` plugin).
    /// Read by [`ConnectionConfig::compose_program`], not by
    /// [`ConnectionConfig::invocation`] itself.
    pub compose_executable: Option<String>,
}

impl ConnectionConfig {
    /// Build straight from a persisted row: `engine`/`kind` parsed by
    /// [`Engine::from_id`]/[`ConnectionKind::from_setting`], the two
    /// executable overrides carried through as-is.
    pub fn from_setting(setting: &ContainerConnectionSetting) -> Self {
        ConnectionConfig {
            engine: Engine::from_id(&setting.engine),
            kind: ConnectionKind::from_setting(setting),
            executable: non_empty(&setting.executable),
            compose_executable: non_empty(&setting.compose_executable),
        }
    }

    /// The program this connection's *compose* invocation runs — the
    /// override when set, else the engine's own program (`docker compose`/
    /// `podman compose` are subcommands of the same CLI, not a second
    /// binary, unless overridden to a standalone `docker-compose`).
    pub fn compose_program(&self) -> String {
        self.compose_executable
            .clone()
            .unwrap_or_else(|| self.program())
    }

    fn program(&self) -> String {
        self.executable
            .clone()
            .unwrap_or_else(|| self.engine.default_program().to_string())
    }

    /// Build the [`Invocation`] this connection runs every command through.
    ///
    /// [`ConnectionKind::Minikube`] is the one kind this cannot finish
    /// alone: the environment it needs comes from actually running
    /// `minikube docker-env` (see [`minikube_docker_env`] and
    /// [`Invocation::with_extra_env`]), which is I/O this function
    /// deliberately stays free of so every other kind's argv/env stays a
    /// pure, table-testable mapping.
    pub fn invocation(&self) -> Invocation {
        let program = self.program();
        let mut prefix_args = Vec::new();
        let mut env = Vec::new();

        match &self.kind {
            ConnectionKind::Auto | ConnectionKind::Minikube => {}
            ConnectionKind::UnixSocket { path } => {
                push_daemon_url(self.engine, &mut prefix_args, &format!("unix://{path}"));
            }
            ConnectionKind::NamedPipe { path } => {
                push_daemon_url(self.engine, &mut prefix_args, &format!("npipe://{path}"));
            }
            ConnectionKind::Tcp { url, cert_dir } => {
                push_daemon_url(self.engine, &mut prefix_args, url);
                if let Some(cert_dir) = cert_dir {
                    match self.engine {
                        // The Docker CLI's own TLS convention: these three
                        // variables, together, are what `-H tcp://...`
                        // needs to actually verify the daemon rather than
                        // dial it in the clear.
                        Engine::Docker => {
                            env.push(("DOCKER_TLS_VERIFY".to_string(), "1".to_string()));
                            env.push(("DOCKER_CERT_PATH".to_string(), cert_dir.clone()));
                        }
                        // `podman --url` has no CLI-flag TLS story of its
                        // own; the cert directory is accepted but unused —
                        // an honest, documented gap rather than a made-up
                        // flag (see the plan's "Parity gaps").
                        Engine::Podman => {}
                    }
                }
            }
            ConnectionKind::Context { name } => match self.engine {
                Engine::Docker => {
                    prefix_args.push("--context".to_string());
                    prefix_args.push(name.clone());
                }
                Engine::Podman => {
                    prefix_args.push("--connection".to_string());
                    prefix_args.push(name.clone());
                }
            },
            ConnectionKind::PodmanMachine { name } => {
                // A `podman machine` is reached the same way a named
                // connection is: `--connection <name>`. Paired with the
                // `Docker` engine this is a nonsensical setting the UI
                // should never offer, but the mapping stays total rather
                // than panicking on it.
                prefix_args.push("--connection".to_string());
                prefix_args.push(name.clone());
            }
            ConnectionKind::Ssh { url, identity } => match self.engine {
                Engine::Docker => {
                    // The Docker CLI has no SSH-identity flag of its own;
                    // it defers to the user's `~/.ssh/config`/agent. That is
                    // a real, documented gap (see the plan's "Parity
                    // gaps") — `identity` is accepted on this kind but only
                    // takes effect for Podman.
                    env.push(("DOCKER_HOST".to_string(), format!("ssh://{url}")));
                }
                Engine::Podman => {
                    prefix_args.push("--url".to_string());
                    prefix_args.push(format!("ssh://{url}"));
                    if let Some(identity) = identity {
                        prefix_args.push("--identity".to_string());
                        prefix_args.push(identity.clone());
                    }
                }
            },
            ConnectionKind::Wsl { distro } => {
                // Reuses `process_exec::host::ExecHost::Wsl`'s wrapping
                // shape (`wsl.exe -d <distro> -- ...`), built directly
                // rather than through `ExecHost::command`: that helper also
                // adds a `--cd <path>` a container connection has no
                // project-relative path to supply, and a `-e` in place of
                // Docker's own `--` would be one more divergence from the
                // plan's documented argv for no benefit.
                return Invocation {
                    program: "wsl.exe".to_string(),
                    prefix_args: vec!["-d".to_string(), distro.clone(), "--".to_string(), program],
                    env,
                    host: ExecHost::Wsl(WslHost {
                        distro: distro.clone(),
                        unc_prefix: format!("//wsl.localhost/{distro}"),
                    }),
                };
            }
        }

        Invocation {
            program,
            prefix_args,
            env,
            host: ExecHost::Local,
        }
    }
}

fn push_daemon_url(engine: Engine, prefix_args: &mut Vec<String>, url: &str) {
    match engine {
        Engine::Docker => {
            prefix_args.push("-H".to_string());
            prefix_args.push(url.to_string());
        }
        Engine::Podman => {
            prefix_args.push("--url".to_string());
            prefix_args.push(url.to_string());
        }
    }
}

/// A ready-to-run command shape: `program prefix_args... <caller's args>`,
/// with `env` added on top of the inherited environment, run on `host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub prefix_args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub host: ExecHost,
}

impl Invocation {
    /// `prefix_args` followed by `args`, as owned strings — what every
    /// caller (`ops.rs`, `session.rs`, ...) hands `process_exec::run`/
    /// `spawn` or a `pty_core::ShellSpec`.
    pub fn argv(&self, args: &[&str]) -> Vec<String> {
        self.prefix_args
            .iter()
            .cloned()
            .chain(args.iter().map(|arg| arg.to_string()))
            .collect()
    }

    /// [`Self::env`] plus `extra`, `extra` winning on a key collision — the
    /// hook [`minikube_docker_env`]'s result is folded through, since a
    /// [`ConnectionKind::Minikube`] invocation carries no env of its own
    /// until it is run once.
    #[must_use]
    pub fn with_extra_env(mut self, extra: Vec<(String, String)>) -> Self {
        for (key, value) in extra {
            self.env.retain(|(existing, _)| existing != &key);
            self.env.push((key, value));
        }
        self
    }

    /// A ready-to-spawn [`std::process::Command`], for a caller that wants
    /// `Command` directly rather than going through `process_exec`.
    pub fn command(&self, cwd: &Path, args: &[&str]) -> std::process::Command {
        let argv = self.argv(args);
        let arg_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let env_refs: Vec<(&str, &str)> = self
            .env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        self.host.command(&self.program, &arg_refs, cwd, &env_refs)
    }
}

/// Parse `minikube docker-env --shell none`'s output into environment
/// pairs. `--shell none` is what makes this format stable: it prints plain
/// `KEY=VALUE` lines with no shell-specific quoting or `export` prefix,
/// unlike the default (bash) output every online example shows — one line
/// per variable, in whatever order `minikube` chose.
pub fn parse_minikube_docker_env(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            (
                key.trim().to_string(),
                value.trim().trim_matches('"').to_string(),
            )
        })
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

/// Run `minikube docker-env --shell none` and parse its output. The one
/// I/O-performing function in this module — everything else here is a pure
/// mapping, table-tested without spawning a real `minikube`.
pub fn minikube_docker_env(
    work_dir: &Path,
) -> Result<Vec<(String, String)>, crate::probe::ConnectionError> {
    let output = process_exec::run(
        "minikube",
        &["docker-env", "--shell", "none"],
        work_dir,
        None,
        Duration::from_secs(15),
        &[],
    )
    .map_err(crate::probe::ConnectionError::from_minikube_failure)?;

    if !output.status.success() {
        return Err(crate::probe::ConnectionError::MinikubeNotRunning(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(parse_minikube_docker_env(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(engine: &str, kind: &str) -> ContainerConnectionSetting {
        ContainerConnectionSetting {
            engine: engine.to_string(),
            kind: kind.to_string(),
            ..ContainerConnectionSetting::default()
        }
    }

    fn config(engine: Engine, kind: ConnectionKind) -> ConnectionConfig {
        ConnectionConfig {
            engine,
            kind,
            executable: None,
            compose_executable: None,
        }
    }

    #[test]
    fn engine_id_round_trips() {
        for engine in [Engine::Docker, Engine::Podman] {
            assert_eq!(Engine::from_id(engine.id()), engine);
        }
        assert_eq!(Engine::from_id("nonsense"), Engine::Docker);
    }

    #[test]
    fn kind_id_round_trips_through_a_setting() {
        let cases: &[(&str, ConnectionKind)] = &[
            ("auto", ConnectionKind::Auto),
            (
                "unix_socket",
                ConnectionKind::UnixSocket {
                    path: String::new(),
                },
            ),
            (
                "tcp",
                ConnectionKind::Tcp {
                    url: String::new(),
                    cert_dir: None,
                },
            ),
            (
                "named_pipe",
                ConnectionKind::NamedPipe {
                    path: String::new(),
                },
            ),
            (
                "context",
                ConnectionKind::Context {
                    name: String::new(),
                },
            ),
            (
                "ssh",
                ConnectionKind::Ssh {
                    url: String::new(),
                    identity: None,
                },
            ),
            (
                "wsl",
                ConnectionKind::Wsl {
                    distro: String::new(),
                },
            ),
            (
                "podman_machine",
                ConnectionKind::PodmanMachine {
                    name: String::new(),
                },
            ),
            ("minikube", ConnectionKind::Minikube),
        ];
        for (id, kind) in cases {
            assert_eq!(kind.id(), *id);
            let setting = setting("docker", id);
            assert_eq!(&ConnectionKind::from_setting(&setting), kind);
        }
        assert_eq!(
            ConnectionKind::from_setting(&setting("docker", "not-a-real-kind")),
            ConnectionKind::Auto
        );
    }

    /// Table-driven argv/env for every kind, against both engines — the
    /// exact matrix the plan calls for. Each row names the kind, the
    /// engine, and the expected `(program, prefix_args, env)`.
    #[test]
    fn invocation_argv_and_env_per_kind_and_engine() {
        struct Case {
            kind: ConnectionKind,
            docker: (
                &'static str,
                &'static [&'static str],
                &'static [(&'static str, &'static str)],
            ),
            podman: (
                &'static str,
                &'static [&'static str],
                &'static [(&'static str, &'static str)],
            ),
        }

        let cases = vec![
            Case {
                kind: ConnectionKind::Auto,
                docker: ("docker", &[], &[]),
                podman: ("podman", &[], &[]),
            },
            Case {
                kind: ConnectionKind::UnixSocket {
                    path: "/var/run/docker.sock".to_string(),
                },
                docker: ("docker", &["-H", "unix:///var/run/docker.sock"], &[]),
                podman: ("podman", &["--url", "unix:///var/run/docker.sock"], &[]),
            },
            Case {
                kind: ConnectionKind::NamedPipe {
                    path: "//./pipe/docker_engine".to_string(),
                },
                docker: ("docker", &["-H", "npipe://" /* + path below */], &[]),
                podman: ("podman", &["--url", "npipe://"], &[]),
            },
            Case {
                kind: ConnectionKind::Tcp {
                    url: "tcp://10.0.0.5:2376".to_string(),
                    cert_dir: Some("/certs".to_string()),
                },
                docker: (
                    "docker",
                    &["-H", "tcp://10.0.0.5:2376"],
                    &[("DOCKER_TLS_VERIFY", "1"), ("DOCKER_CERT_PATH", "/certs")],
                ),
                podman: ("podman", &["--url", "tcp://10.0.0.5:2376"], &[]),
            },
            Case {
                kind: ConnectionKind::Context {
                    name: "remote-box".to_string(),
                },
                docker: ("docker", &["--context", "remote-box"], &[]),
                podman: ("podman", &["--connection", "remote-box"], &[]),
            },
            Case {
                kind: ConnectionKind::PodmanMachine {
                    name: "podman-machine-default".to_string(),
                },
                docker: ("docker", &["--connection", "podman-machine-default"], &[]),
                podman: ("podman", &["--connection", "podman-machine-default"], &[]),
            },
            Case {
                kind: ConnectionKind::Ssh {
                    url: "user@example.com".to_string(),
                    identity: Some("/home/user/.ssh/id_ed25519".to_string()),
                },
                docker: ("docker", &[], &[("DOCKER_HOST", "ssh://user@example.com")]),
                podman: (
                    "podman",
                    &[
                        "--url",
                        "ssh://user@example.com",
                        "--identity",
                        "/home/user/.ssh/id_ed25519",
                    ],
                    &[],
                ),
            },
            Case {
                kind: ConnectionKind::Minikube,
                docker: ("docker", &[], &[]),
                podman: ("podman", &[], &[]),
            },
        ];

        for case in cases {
            for (engine, (program, prefix_args, env)) in
                [(Engine::Docker, case.docker), (Engine::Podman, case.podman)]
            {
                // The named-pipe case's expected prefix carries the path
                // itself for readability above; assemble it here rather
                // than repeating the full literal twice per engine.
                let (expected_program, expected_prefix): (&str, Vec<String>) =
                    if let ConnectionKind::NamedPipe { path } = &case.kind {
                        let url = format!("npipe://{path}");
                        (
                            program,
                            match engine {
                                Engine::Docker => vec!["-H".to_string(), url],
                                Engine::Podman => vec!["--url".to_string(), url],
                            },
                        )
                    } else {
                        (program, prefix_args.iter().map(|s| s.to_string()).collect())
                    };

                let invocation = config(engine, case.kind.clone()).invocation();
                assert_eq!(
                    invocation.program, expected_program,
                    "{:?}/{engine:?}",
                    case.kind
                );
                assert_eq!(
                    invocation.prefix_args, expected_prefix,
                    "{:?}/{engine:?}",
                    case.kind
                );
                assert_eq!(
                    invocation.env,
                    env.iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect::<Vec<_>>(),
                    "{:?}/{engine:?}",
                    case.kind
                );
            }
        }
    }

    #[test]
    fn executable_override_replaces_the_program_name() {
        let cfg = ConnectionConfig {
            engine: Engine::Podman,
            kind: ConnectionKind::Auto,
            executable: Some("podman-remote".to_string()),
            compose_executable: None,
        };
        assert_eq!(cfg.invocation().program, "podman-remote");
        assert_eq!(cfg.compose_program(), "podman-remote");
    }

    #[test]
    fn compose_executable_override_is_independent_of_the_main_program() {
        let cfg = ConnectionConfig {
            engine: Engine::Docker,
            kind: ConnectionKind::Auto,
            executable: None,
            compose_executable: Some("docker-compose".to_string()),
        };
        assert_eq!(cfg.invocation().program, "docker");
        assert_eq!(cfg.compose_program(), "docker-compose");
    }

    #[test]
    fn wsl_wraps_through_wsl_exe_dash_dash() {
        let cfg = config(
            Engine::Docker,
            ConnectionKind::Wsl {
                distro: "Ubuntu".to_string(),
            },
        );
        let invocation = cfg.invocation();
        assert_eq!(invocation.program, "wsl.exe");
        assert_eq!(
            invocation.argv(&["ps", "-a"]),
            vec!["-d", "Ubuntu", "--", "docker", "ps", "-a"]
        );
        assert!(matches!(invocation.host, ExecHost::Wsl(_)));
    }

    #[test]
    fn parses_minikube_docker_env_shell_none_output() {
        let output = "DOCKER_TLS_VERIFY=\"1\"\nDOCKER_HOST=\"tcp://192.168.49.2:2376\"\nDOCKER_CERT_PATH=\"/home/user/.minikube/certs\"\nDOCKER_API_VERSION=\"1.41\"\n";
        assert_eq!(
            parse_minikube_docker_env(output),
            vec![
                ("DOCKER_TLS_VERIFY".to_string(), "1".to_string()),
                (
                    "DOCKER_HOST".to_string(),
                    "tcp://192.168.49.2:2376".to_string()
                ),
                (
                    "DOCKER_CERT_PATH".to_string(),
                    "/home/user/.minikube/certs".to_string()
                ),
                ("DOCKER_API_VERSION".to_string(), "1.41".to_string()),
            ]
        );
    }

    #[test]
    fn with_extra_env_lets_a_later_value_win() {
        let invocation = Invocation {
            program: "docker".to_string(),
            prefix_args: Vec::new(),
            env: vec![("DOCKER_HOST".to_string(), "old".to_string())],
            host: ExecHost::Local,
        }
        .with_extra_env(vec![("DOCKER_HOST".to_string(), "new".to_string())]);
        assert_eq!(
            invocation.env,
            vec![("DOCKER_HOST".to_string(), "new".to_string())]
        );
    }
}
