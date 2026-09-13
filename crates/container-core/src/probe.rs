//! "Test connection": run `<cli> version --format json` through an
//! [`Invocation`] and turn the result into an [`EngineInfo`], or a
//! [`ConnectionError`] whose [`std::fmt::Display`] carries a
//! JetBrains-troubleshooting-page-style hint — the text the Settings page's
//! red result label shows verbatim.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::connection::{Engine, Invocation};

/// A successful probe: what `Test connection` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInfo {
    pub engine: Engine,
    pub client_version: String,
    pub server_version: String,
    pub api_version: String,
}

impl fmt::Display for EngineInfo {
    /// "Docker Engine 29.6.2" / "Podman Engine 5.2.3" — the success line
    /// the plan's mockup shows in the settings page.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.engine {
            Engine::Docker => "Docker",
            Engine::Podman => "Podman",
        };
        write!(f, "{name} Engine {}", self.server_version)
    }
}

/// Why a probe failed to produce an [`EngineInfo`].
///
/// Each variant's [`Display`](fmt::Display) is written for the settings
/// page's error label, not a log — JetBrains' own Docker troubleshooting
/// page is the model: name what's wrong, then say what to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionError {
    /// The CLI itself is not on `PATH` (or not executable at the resolved
    /// path).
    CliNotFound { program: String },
    /// The CLI ran but could not reach the daemon at all.
    DaemonUnreachable { engine: Engine, detail: String },
    /// The daemon answered but refused the request — most often a
    /// permission-denied on the Unix socket.
    PermissionDenied { engine: Engine, detail: String },
    /// A TLS handshake failed against a `tcp://` connection.
    TlsFailure { detail: String },
    /// The CLI answered but its `version --format json` output did not
    /// parse as the shape this crate expects.
    UnparsableOutput { detail: String },
    /// `minikube docker-env` reported the cluster is not running.
    MinikubeNotRunning(String),
    /// Anything else: a nonzero exit with stderr text, or an I/O failure
    /// running the process.
    Other { detail: String },
}

impl ConnectionError {
    fn from_process_failure(program: &str, failure: process_exec::Failure) -> Self {
        match failure {
            process_exec::Failure::NotFound => ConnectionError::CliNotFound {
                program: program.to_string(),
            },
            process_exec::Failure::TimedOut => ConnectionError::Other {
                detail: format!("{program} did not respond within the timeout"),
            },
            process_exec::Failure::Io(message) => ConnectionError::Other { detail: message },
        }
    }

    pub(crate) fn from_minikube_failure(failure: process_exec::Failure) -> Self {
        Self::from_process_failure("minikube", failure)
    }

    /// Classify a nonzero exit's stderr into the more specific variants
    /// above, falling back to [`ConnectionError::Other`]. Substring
    /// matching on the CLI's own English messages is a known ceiling —
    /// upgrade path is matching on a specific, documented exit code per
    /// engine if one ever proves unreliable.
    fn from_stderr(engine: Engine, stderr: &str) -> Self {
        let lower = stderr.to_lowercase();
        if lower.contains("permission denied") {
            ConnectionError::PermissionDenied {
                engine,
                detail: stderr.trim().to_string(),
            }
        } else if lower.contains("tls") || lower.contains("certificate") {
            ConnectionError::TlsFailure {
                detail: stderr.trim().to_string(),
            }
        } else if lower.contains("cannot connect")
            || lower.contains("connection refused")
            || lower.contains("no such host")
            || lower.contains("is the docker daemon running")
        {
            ConnectionError::DaemonUnreachable {
                engine,
                detail: stderr.trim().to_string(),
            }
        } else {
            ConnectionError::Other {
                detail: stderr.trim().to_string(),
            }
        }
    }
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionError::CliNotFound { program } => write!(
                f,
                "'{program}' was not found on PATH. Install the {program} CLI, or set an \
                 executable override for this connection."
            ),
            ConnectionError::DaemonUnreachable { engine, detail } => {
                let hint = match engine {
                    Engine::Docker => {
                        "Is the Docker daemon running? On Linux, check `systemctl status \
                         docker`; on Windows/Mac, start Docker Desktop."
                    }
                    Engine::Podman => {
                        "Is the Podman socket active? Run `systemctl --user start \
                         podman.socket` for a rootless connection, or `podman machine start` \
                         if you connect through a machine."
                    }
                };
                write!(f, "Could not connect: {detail}\n{hint}")
            }
            ConnectionError::PermissionDenied { engine, detail } => {
                let hint = match engine {
                    Engine::Docker => {
                        "Add your user to the 'docker' group (`sudo usermod -aG docker $USER`) \
                         and start a new session, or run this IDE with access to the socket."
                    }
                    Engine::Podman => {
                        "Rootless Podman's socket is per-user; check that this IDE runs as the \
                         same user the socket belongs to, and that `systemctl --user start \
                         podman.socket` has been run for that user."
                    }
                };
                write!(f, "Permission denied: {detail}\n{hint}")
            }
            ConnectionError::TlsFailure { detail } => write!(
                f,
                "TLS handshake failed: {detail}\nCheck the certificate directory — it must \
                 contain ca.pem, cert.pem and key.pem for this daemon."
            ),
            ConnectionError::UnparsableOutput { detail } => write!(
                f,
                "The engine responded, but its version output could not be read: {detail}"
            ),
            ConnectionError::MinikubeNotRunning(detail) => write!(
                f,
                "minikube docker-env failed: {detail}\nStart the cluster with `minikube start`."
            ),
            ConnectionError::Other { detail } => write!(f, "{detail}"),
        }
    }
}

/// Run `<cli> version --format json` through `invocation` and parse the
/// result. `work_dir` is only where the process is launched from — no
/// container-core operation is project-relative, so callers typically pass
/// the current directory or the config directory.
pub fn probe(
    invocation: &Invocation,
    engine: Engine,
    work_dir: &Path,
) -> Result<EngineInfo, ConnectionError> {
    let argv = invocation.argv(&["version", "--format", "json"]);
    let arg_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let env_refs: Vec<(&str, &str)> = invocation
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let output = process_exec::run(
        &invocation.program,
        &arg_refs,
        work_dir,
        None,
        Duration::from_secs(10),
        &env_refs,
    )
    .map_err(|failure| ConnectionError::from_process_failure(&invocation.program, failure))?;

    if !output.status.success() {
        return Err(ConnectionError::from_stderr(
            engine,
            &String::from_utf8_lossy(&output.stderr),
        ));
    }

    parse_version_output(engine, &String::from_utf8_lossy(&output.stdout))
}

/// Both `docker version --format json` and `podman version --format json`
/// nest client/server blocks, but under different shapes:
///
/// Docker: `{"Client":{"Version":"27.3.1","ApiVersion":"1.47"},"Server":{"Version":"27.3.1"}}`
/// Podman: `{"Client":{"Version":"5.2.3","APIVersion":"5.2.3"},"Server":{"Version":"5.2.3"}}`
///   (podman machine/remote setups may omit `"Server"` entirely — the
///   client *is* talking to a server, but only the client block is
///   guaranteed.)
///
/// One lenient struct with both casings covers both, `#[serde(default)]`
/// throughout so a field either engine omits just comes back empty rather
/// than failing the whole parse.
#[derive(Deserialize, Default)]
struct VersionOutput {
    #[serde(default, rename = "Client")]
    client: VersionBlock,
    #[serde(default, rename = "Server")]
    server: Option<VersionBlock>,
}

#[derive(Deserialize, Default)]
struct VersionBlock {
    #[serde(default, rename = "Version")]
    version: String,
    #[serde(default, rename = "ApiVersion")]
    api_version: String,
    #[serde(default, rename = "APIVersion")]
    api_version_podman: String,
}

impl VersionBlock {
    fn api_version(&self) -> String {
        if !self.api_version.is_empty() {
            self.api_version.clone()
        } else {
            self.api_version_podman.clone()
        }
    }
}

/// Parse `version --format json`'s stdout, the mechanism [`probe`] calls
/// after a successful run — split out so it is also directly testable
/// against captured/hand-authored fixture JSON with no process involved.
pub fn parse_version_output(engine: Engine, stdout: &str) -> Result<EngineInfo, ConnectionError> {
    let parsed: VersionOutput =
        serde_json::from_str(stdout).map_err(|error| ConnectionError::UnparsableOutput {
            detail: error.to_string(),
        })?;

    let server_version = parsed
        .server
        .as_ref()
        .map(|server| server.version.clone())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| parsed.client.version.clone());

    let api_version = parsed.client.api_version();
    Ok(EngineInfo {
        engine,
        client_version: parsed.client.version,
        server_version,
        api_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_version_json() {
        let stdout = include_str!("../testdata/probe/docker_version.json");
        let info = parse_version_output(Engine::Docker, stdout).expect("parses");
        assert_eq!(info.engine, Engine::Docker);
        assert!(!info.client_version.is_empty());
        assert!(!info.server_version.is_empty());
        assert!(!info.api_version.is_empty());
    }

    #[test]
    fn parses_podman_version_json() {
        let stdout = include_str!("../testdata/probe/podman_version.json");
        let info = parse_version_output(Engine::Podman, stdout).expect("parses");
        assert_eq!(info.engine, Engine::Podman);
        assert_eq!(info.client_version, "5.2.3");
        assert_eq!(info.server_version, "5.2.3");
        assert_eq!(info.api_version, "5.2.3");
    }

    #[test]
    fn podman_version_without_a_server_block_falls_back_to_the_client_version() {
        let stdout = r#"{"Client":{"APIVersion":"5.2.3","Version":"5.2.3"}}"#;
        let info = parse_version_output(Engine::Podman, stdout).expect("parses");
        assert_eq!(info.server_version, "5.2.3");
    }

    #[test]
    fn garbage_output_is_unparsable_not_a_panic() {
        let error = parse_version_output(Engine::Docker, "not json").unwrap_err();
        assert!(matches!(error, ConnectionError::UnparsableOutput { .. }));
    }

    #[test]
    fn engine_info_display_matches_the_settings_page_success_line() {
        let info = EngineInfo {
            engine: Engine::Docker,
            client_version: "27.3.1".to_string(),
            server_version: "29.6.2".to_string(),
            api_version: "1.47".to_string(),
        };
        assert_eq!(info.to_string(), "Docker Engine 29.6.2");
    }

    #[test]
    fn daemon_unreachable_carries_the_engine_specific_hint() {
        let error = ConnectionError::from_stderr(
            Engine::Docker,
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
        );
        assert!(matches!(error, ConnectionError::DaemonUnreachable { .. }));
        assert!(error.to_string().contains("systemctl status docker"));
    }

    #[test]
    fn podman_permission_denied_names_the_rootless_socket() {
        let error = ConnectionError::from_stderr(Engine::Podman, "permission denied");
        assert!(matches!(error, ConnectionError::PermissionDenied { .. }));
        assert!(error.to_string().contains("podman.socket"));
    }

    #[test]
    fn tls_failure_is_classified_from_stderr() {
        let error = ConnectionError::from_stderr(
            Engine::Docker,
            "error during connect: Get \"https://10.0.0.5:2376/v1.47/version\": remote error: tls: bad certificate",
        );
        assert!(matches!(error, ConnectionError::TlsFailure { .. }));
        assert!(error.to_string().contains("ca.pem"));
    }
}
