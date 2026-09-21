//! `RusshTunnel` (F7.5): the in-process SSH tunnel `db_core::tunnel::select`
//! falls back to when `ssh` is not on `PATH`, or the auth mode is a
//! password (`ssh -o BatchMode=yes` cannot supply one interactively).
//!
//! Host key verification against `~/.ssh/known_hosts` never auto-accepts
//! an unknown or changed key: [`DbErrorCode::HostKeyUnknown`]/
//! [`DbErrorCode::HostKeyMismatch`] surface to the caller instead of
//! silently trusting it — consenting to a new key is a later phase's UI
//! (the plan's F7b), not this module's job.
//!
//! Every async call runs through `crate::runtime()` (`block_on`), the same
//! private-runtime pattern `postgres.rs`/`mongodb.rs` use — `RusshTunnel`
//! itself is an ordinary blocking constructor from every other crate's
//! point of view.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use russh::client::{self, Handle};
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::{
    known_hosts, load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use russh::{ChannelMsg, Disconnect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use db_core::error::{DbError, DbErrorCode};
use db_core::tunnel::{SshAuthMode, SshConfig, Tunnel};

fn tunnel_err(message: impl std::fmt::Display) -> DbError {
    DbError::new(DbErrorCode::TunnelFailed, message.to_string())
}

/// What went wrong with the server's host key, recorded by
/// [`HostKeyRecorder::check_server_key`] for [`RusshTunnel::open`] to turn
/// into a typed [`DbError`] once `client::connect` reports the handshake
/// failed (aborting from inside the callback by returning `Ok(false)`
/// surfaces to the caller as a generic connection error, so the actual
/// reason is threaded out through this side channel instead).
#[derive(Clone, Default)]
struct HostKeyOutcome(Arc<Mutex<Option<HostKeyProblem>>>);

#[derive(Clone)]
enum HostKeyProblem {
    Unknown { fingerprint: String },
    Mismatch { fingerprint: String },
}

impl HostKeyOutcome {
    fn record(&self, problem: HostKeyProblem) {
        // SAFETY-equivalent invariant: never panics — a poisoned mutex here
        // would mean a previous check already panicked, which `unwrap_or_else`
        // recovers from by just overwriting the stale value.
        let mut guard = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        *guard = Some(problem);
    }

    fn into_error(self) -> Option<DbError> {
        let problem = self
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        problem.map(|problem| match problem {
            HostKeyProblem::Unknown { fingerprint } => DbError::new(
                DbErrorCode::HostKeyUnknown,
                format!("the server's host key ({fingerprint}) is not in ~/.ssh/known_hosts"),
            ),
            HostKeyProblem::Mismatch { fingerprint } => DbError::new(
                DbErrorCode::HostKeyMismatch,
                format!(
                    "the server's host key ({fingerprint}) does not match the one recorded in ~/.ssh/known_hosts"
                ),
            ),
        })
    }
}

struct HostKeyRecorder {
    host: String,
    port: u16,
    outcome: HostKeyOutcome,
}

impl client::Handler for HostKeyRecorder {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_public_key else {
            // A certificate-based host key: F7.5 does not model CA trust —
            // silently accepting one would be worse than refusing it.
            // ponytail: revisit once a real data source presents one.
            self.outcome.record(HostKeyProblem::Unknown {
                fingerprint: "<openssh certificate, unsupported>".to_string(),
            });
            return Ok(false);
        };
        let fingerprint = key.fingerprint(Default::default()).to_string();
        match known_hosts::check_known_hosts(&self.host, self.port, key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                self.outcome.record(HostKeyProblem::Unknown { fingerprint });
                Ok(false)
            }
            Err(_) => {
                self.outcome
                    .record(HostKeyProblem::Mismatch { fingerprint });
                Ok(false)
            }
        }
    }
}

async fn authenticate(
    handle: &mut Handle<HostKeyRecorder>,
    user: &str,
    ssh: &SshConfig,
) -> Result<(), DbError> {
    let result = match ssh.auth {
        SshAuthMode::Agent | SshAuthMode::SshConfig => authenticate_via_agent(handle, user).await?,
        SshAuthMode::KeyFile => {
            let key_file = ssh.key_file.as_deref().ok_or_else(|| {
                tunnel_err("SSH key-file auth selected but no key file is configured")
            })?;
            let key_pair = load_secret_key(key_file, ssh.password.as_deref()).map_err(|error| {
                tunnel_err(format!("could not load SSH key {key_file}: {error}"))
            })?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|error| tunnel_err(format!("SSH handshake failed: {error}")))?
                .flatten();
            handle
                .authenticate_publickey(
                    user,
                    PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash_alg),
                )
                .await
                .map_err(|error| tunnel_err(format!("SSH publickey auth failed: {error}")))?
        }
        SshAuthMode::Password => {
            let password = ssh.password.as_deref().ok_or_else(|| {
                tunnel_err("SSH password auth selected but no password is configured")
            })?;
            handle
                .authenticate_password(user, password)
                .await
                .map_err(|error| tunnel_err(format!("SSH password auth failed: {error}")))?
        }
    };
    if result.success() {
        Ok(())
    } else {
        Err(tunnel_err(
            "the SSH server rejected every offered credential",
        ))
    }
}

async fn authenticate_via_agent(
    handle: &mut Handle<HostKeyRecorder>,
    user: &str,
) -> Result<russh::client::AuthResult, DbError> {
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|error| tunnel_err(format!("could not reach ssh-agent: {error}")))?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|error| tunnel_err(format!("ssh-agent has no usable identities: {error}")))?;
    let hash_alg: Option<HashAlg> = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|error| tunnel_err(format!("SSH handshake failed: {error}")))?
        .flatten();
    for identity in identities {
        let AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        let result = handle
            .authenticate_publickey_with(user, key, hash_alg, &mut agent)
            .await
            .map_err(|error| tunnel_err(format!("ssh-agent auth failed: {error:?}")))?;
        if result.success() {
            return Ok(result);
        }
    }
    Err(tunnel_err("ssh-agent had no identity the server accepted"))
}

/// Copies bytes between one accepted local TCP connection and one
/// `direct-tcpip` channel until either side closes — the same shape
/// russh's own `client_open_direct_tcpip` example uses.
async fn forward_one_connection(
    mut socket: tokio::net::TcpStream,
    mut channel: russh::Channel<russh::client::Msg>,
) {
    let mut buf = vec![0u8; 65536];
    let mut socket_closed = false;
    loop {
        tokio::select! {
            read = socket.read(&mut buf), if !socket_closed => {
                match read {
                    Ok(0) => {
                        socket_closed = true;
                        let _ = channel.eof().await;
                    }
                    Ok(n) => {
                        if channel.data(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { data }) => {
                        if socket.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// An SSH tunnel opened in-process (no `ssh` binary involved).
pub struct RusshTunnel {
    local_port: u16,
    accept_task: tokio::task::JoinHandle<()>,
    handle: Arc<tokio::sync::Mutex<Handle<HostKeyRecorder>>>,
}

impl RusshTunnel {
    /// Connects to `ssh`, authenticates per `ssh.auth`, binds a local
    /// `127.0.0.1:0` listener, and starts forwarding every connection
    /// accepted on it to `remote_host:remote_port` over the SSH session.
    pub fn open(ssh: &SshConfig, remote_host: &str, remote_port: u16) -> Result<Self, DbError> {
        let ssh = ssh.clone();
        let remote_host = remote_host.to_string();
        crate::runtime().block_on(Self::open_async(ssh, remote_host, remote_port))
    }

    async fn open_async(
        ssh: SshConfig,
        remote_host: String,
        remote_port: u16,
    ) -> Result<Self, DbError> {
        let outcome = HostKeyOutcome::default();
        let recorder = HostKeyRecorder {
            host: ssh.host.clone(),
            port: ssh.port,
            outcome: outcome.clone(),
        };
        let config = Arc::new(client::Config::default());
        let mut handle = client::connect(config, (ssh.host.as_str(), ssh.port), recorder)
            .await
            .map_err(|error| {
                outcome
                    .clone()
                    .into_error()
                    .unwrap_or_else(|| tunnel_err(format!("SSH connection failed: {error}")))
            })?;

        authenticate(&mut handle, &ssh.user, &ssh).await?;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| tunnel_err(format!("could not reserve a local port: {error}")))?;
        let local_port = listener
            .local_addr()
            .map_err(|error| tunnel_err(format!("could not read the local port: {error}")))?
            .port();

        let handle = Arc::new(tokio::sync::Mutex::new(handle));
        let accept_task = crate::runtime().spawn(accept_loop(
            listener,
            handle.clone(),
            remote_host,
            remote_port,
        ));

        Ok(Self {
            local_port,
            accept_task,
            handle,
        })
    }
}

async fn accept_loop(
    listener: TcpListener,
    handle: Arc<tokio::sync::Mutex<Handle<HostKeyRecorder>>>,
    remote_host: String,
    remote_port: u16,
) {
    loop {
        let (socket, originator): (_, SocketAddr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(_) => return,
        };
        let channel = {
            let handle = handle.lock().await;
            handle
                .channel_open_direct_tcpip(
                    remote_host.clone(),
                    remote_port.into(),
                    originator.ip().to_string(),
                    originator.port().into(),
                )
                .await
        };
        if let Ok(channel) = channel {
            tokio::spawn(forward_one_connection(socket, channel));
        }
    }
}

impl Tunnel for RusshTunnel {
    fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for RusshTunnel {
    fn drop(&mut self) {
        // ponytail: an abrupt abort + drop rather than a graceful
        // `Disconnect::ByApplication` handshake — acceptable for a tunnel
        // whose failure mode is "the forwarded connections stop working",
        // never data loss, since `Connection::close` (F1) already owns the
        // graceful shutdown of whatever was tunnelled through it.
        self.accept_task.abort();
        let handle = self.handle.clone();
        crate::runtime().spawn(async move {
            let _ = handle
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "", "")
                .await;
        });
    }
}

#[cfg(all(test, feature = "db-integration"))]
mod integration_tests {
    use super::*;
    use db_core::datasource::SslMode;

    /// Compiles a real connect-through-a-tunnel path against the `sshd`
    /// service `docker/db-compose.yml` adds (env `IDE_DB_SSH_HOST` etc.) —
    /// not run in this sandbox (no network services available here), only
    /// proven to compile and type-check.
    #[test]
    #[ignore = "needs docker/db-compose.yml's sshd + postgres services"]
    fn tunnels_through_the_compose_sshd_to_postgres() {
        let host = std::env::var("IDE_DB_SSH_HOST").expect("IDE_DB_SSH_HOST");
        let port: u16 = std::env::var("IDE_DB_SSH_PORT")
            .unwrap_or_else(|_| "22".to_string())
            .parse()
            .expect("IDE_DB_SSH_PORT");
        let user = std::env::var("IDE_DB_SSH_USER").expect("IDE_DB_SSH_USER");
        let password = std::env::var("IDE_DB_SSH_PASSWORD").expect("IDE_DB_SSH_PASSWORD");
        let _ = SslMode::Disable; // keep the import meaningful once this test grows a real assertion
        let ssh = SshConfig {
            host,
            port,
            user,
            auth: SshAuthMode::Password,
            key_file: None,
            password: Some(password),
        };
        let tunnel = RusshTunnel::open(&ssh, "postgres", 5432).expect("tunnel open");
        assert!(tunnel.local_port() > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_outcome_starts_empty() {
        let outcome = HostKeyOutcome::default();
        assert!(outcome.into_error().is_none());
    }

    #[test]
    fn an_unknown_host_key_becomes_host_key_unknown() {
        let outcome = HostKeyOutcome::default();
        outcome.record(HostKeyProblem::Unknown {
            fingerprint: "SHA256:abc".to_string(),
        });
        let error = outcome.into_error().expect("an error");
        assert_eq!(error.code, DbErrorCode::HostKeyUnknown);
        assert!(error.message.contains("SHA256:abc"));
    }

    #[test]
    fn a_changed_host_key_becomes_host_key_mismatch() {
        let outcome = HostKeyOutcome::default();
        outcome.record(HostKeyProblem::Mismatch {
            fingerprint: "SHA256:def".to_string(),
        });
        let error = outcome.into_error().expect("an error");
        assert_eq!(error.code, DbErrorCode::HostKeyMismatch);
        assert!(error.message.contains("SHA256:def"));
    }
}
