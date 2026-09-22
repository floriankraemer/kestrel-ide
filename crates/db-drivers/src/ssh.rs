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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use russh::client::{self, Handle};
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::{
    known_hosts, load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate,
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

/// Every host key a connection attempt has seen rejected, by `(host,
/// port)`, so [`accept_host_key`] (F7b's host-key prompt) can look the
/// actual key material back up once the user consents — a [`DbError`]
/// only carries a code plus a message (ADR-0003), never a key, so the
/// fingerprint in [`HostKeyOutcome::into_error`]'s message cannot itself
/// be turned back into a `known_hosts` line. A process-wide table rather
/// than a per-tunnel-attempt handle: the failed [`RusshTunnel::open`] call
/// that recorded it has already returned by the time a UI prompt resolves
/// (database-tools.md §4's host-key-prompt flow spans a user decision in
/// between).
/// ponytail: entries are never evicted — a host that is never retried
/// leaves one small `(String, PublicKey)` behind; revisit with a TTL if a
/// long-running session accumulates many distinct unreachable hosts.
fn pending_host_keys() -> &'static Mutex<HashMap<(String, u16), PublicKey>> {
    static PENDING: OnceLock<Mutex<HashMap<(String, u16), PublicKey>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
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
                pending_host_keys()
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert((self.host.clone(), self.port), key.clone());
                Ok(false)
            }
            Err(_) => {
                self.outcome
                    .record(HostKeyProblem::Mismatch { fingerprint });
                pending_host_keys()
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert((self.host.clone(), self.port), key.clone());
                Ok(false)
            }
        }
    }
}

/// Appends `host`/`port`'s public `key` to the OpenSSH `known_hosts` file
/// at `path`, in the same format `ssh`/`ssh-keyscan` write (F7b) — a thin
/// wrapper over `russh`'s own `learn_known_hosts_path` (the crate already
/// used for the read half, `check_known_hosts`, above) rather than a
/// hand-rolled writer.
pub fn append_known_host(
    host: &str,
    port: u16,
    key: &PublicKey,
    path: &Path,
) -> Result<(), DbError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            tunnel_err(format!("could not create {}: {error}", parent.display()))
        })?;
    }
    known_hosts::learn_known_hosts_path(host, port, key, path)
        .map_err(|error| tunnel_err(format!("could not update known_hosts: {error}")))
}

/// The default `known_hosts` path a real (non-test) caller writes to —
/// `~/.ssh/known_hosts`, the same file `ssh`'s own CLI fallback
/// (`tunnel::select`) and every OpenSSH client read from.
pub fn default_known_hosts_path() -> Option<PathBuf> {
    Some(dirs_home()?.join(".ssh").join("known_hosts"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The host-key prompt's "Accept and add to known_hosts" answer (F7b):
/// looks up the key [`HostKeyRecorder::check_server_key`] stashed for
/// `host`/`port` the last time a connect attempt rejected it, appends it
/// to `path`, and removes the pending entry either way (a stale key is
/// never worth retrying silently a second time). The caller retries the
/// connect afterwards — this only fixes `known_hosts`, it does not itself
/// reopen a tunnel. Split from [`accept_host_key`] so a test exercises the
/// whole "consume the pending entry, then write" path against a temp file
/// instead of the real `~/.ssh/known_hosts`.
pub fn accept_host_key_at(host: &str, port: u16, path: &Path) -> Result<(), DbError> {
    let key = pending_host_keys()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .remove(&(host.to_string(), port))
        .ok_or_else(|| {
            tunnel_err("no pending host key for this host — try connecting again first")
        })?;
    append_known_host(host, port, &key, path)
}

/// [`accept_host_key_at`] against the real `~/.ssh/known_hosts` — what the
/// host-key prompt's UI actually calls.
pub fn accept_host_key(host: &str, port: u16) -> Result<(), DbError> {
    let path = default_known_hosts_path()
        .ok_or_else(|| tunnel_err("could not resolve the home directory for known_hosts"))?;
    accept_host_key_at(host, port, &path)
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

    /// A real, fixed Ed25519 test key in OpenSSH format — parsed rather
    /// than freshly generated: `append_known_host`/`accept_host_key` only
    /// need *a* valid `PublicKey`, and parsing one is deterministic where
    /// generating one needs a CSPRNG dependency this crate has no other
    /// use for.
    fn test_public_key() -> PublicKey {
        // A real Ed25519 public key (russh's own `known_hosts` test
        // fixture upstream), not a hand-typed base64 string — a made-up
        // one can parse as syntactically well-formed and still fail the
        // exact-bytes equality `check_known_hosts_path` does.
        PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ",
        )
        .expect("a valid test Ed25519 public key")
    }

    #[test]
    fn append_known_host_writes_a_line_the_check_recognises() {
        let dir = std::env::temp_dir().join(format!("ide-known-hosts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("known_hosts");
        let key = test_public_key();

        append_known_host("example.test", 22, &key, &path).expect("append succeeds");

        let contents = std::fs::read_to_string(&path).expect("file was written");
        assert!(contents.contains("example.test"));
        assert!(
            known_hosts::check_known_hosts_path("example.test", 22, &key, &path).unwrap_or(false)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_known_host_creates_missing_parent_directories() {
        let dir =
            std::env::temp_dir().join(format!("ide-known-hosts-nested-{}", std::process::id()));
        let path = dir.join(".ssh").join("known_hosts");
        let key = test_public_key();

        append_known_host("example.test", 22, &key, &path).expect("append creates parents");
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accept_host_key_at_with_nothing_pending_is_refused() {
        let dir = std::env::temp_dir().join(format!("ide-known-hosts-none-{}", std::process::id()));
        let path = dir.join("known_hosts");
        let error = accept_host_key_at("no-such-host.invalid", 2222, &path).unwrap_err();
        assert_eq!(error.code, DbErrorCode::TunnelFailed);
    }

    #[test]
    fn accept_host_key_at_writes_the_pending_key_and_consumes_it_once() {
        let host = format!("pending-test-host-{}.invalid", std::process::id());
        let key = test_public_key();
        pending_host_keys()
            .lock()
            .unwrap()
            .insert((host.clone(), 22), key.clone());
        let dir =
            std::env::temp_dir().join(format!("ide-known-hosts-accept-{}", std::process::id()));
        let path = dir.join("known_hosts");

        accept_host_key_at(&host, 22, &path).expect("accept succeeds");
        assert!(known_hosts::check_known_hosts_path(&host, 22, &key, &path).unwrap_or(false));

        // Consumed: a second accept with nothing pending is refused.
        assert!(pending_host_keys()
            .lock()
            .unwrap()
            .get(&(host.clone(), 22))
            .is_none());
        assert!(accept_host_key_at(&host, 22, &path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
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
