//! SSH tunnelling (ADR-0061 §5): the CLI first — `CliTunnel` shells out to
//! the user's own `ssh`, for free config/agent/`ProxyJump` support — with
//! `russh` reserved as a future in-process fallback (not built in F1; see
//! the plan's F7 row) for the two cases the CLI cannot cover.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::error::{DbError, DbErrorCode};

/// What the tunnel is asked to forward: a bastion to go through, and the
/// backend host/port on the far side of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelSpec {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub remote_host: String,
    pub remote_port: u16,
}

/// A tunnel mechanism: opens a local port that forwards to
/// `spec.remote_host:remote_port` through `spec.ssh_host`. Implementations
/// close the tunnel on `Drop` (`CliTunnel` kills its `ssh` child;
/// `db_drivers::ssh::RusshTunnel` drops its forwarding task and channel) —
/// never a separate `close` method a caller could forget to call.
pub trait Tunnel: Send {
    /// The local port traffic should be sent to once open.
    fn local_port(&self) -> u16;
}

/// How an SSH tunnel authenticates — `app_config::database::SshSetting`'s
/// own free-form `auth` string, typed (F7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAuthMode {
    /// Ask a running `ssh-agent` for a signature — never touches key
    /// material directly.
    Agent,
    /// A private key file on disk, passphrase (if any) from [`crate::
    /// datasource::Secrets::ssh_password`].
    KeyFile,
    /// A plain password, from the same `Secrets` field.
    Password,
    /// No explicit choice: let `ssh`'s own config/agent/default-identity
    /// resolution decide — only meaningful for [`SelectedTunnel::Cli`],
    /// since there is no "ssh config" for `RusshTunnel` to defer to.
    SshConfig,
}

impl SshAuthMode {
    pub fn from_id(id: &str) -> Self {
        match id {
            "password" => SshAuthMode::Password,
            "key" => SshAuthMode::KeyFile,
            "config" => SshAuthMode::SshConfig,
            _ => SshAuthMode::Agent,
        }
    }
}

/// A typed view over `app_config::database::SshSetting` plus the secret
/// [`crate::datasource::Secrets::ssh_password`] resolved (a passphrase for
/// [`SshAuthMode::KeyFile`], a password for [`SshAuthMode::Password`]) —
/// what [`select`] and `db_drivers::ssh::RusshTunnel::open` actually need.
#[derive(Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuthMode,
    pub key_file: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for SshConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("auth", &self.auth)
            .field("key_file", &self.key_file)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Which tunnel mechanism [`select`] picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedTunnel {
    /// Shell out to the user's own `ssh` (`CliTunnel`) — free config,
    /// agent and `ProxyJump` support.
    Cli,
    /// Connect in-process (`db_drivers::ssh::RusshTunnel`) — the only
    /// option once a password is the credential, since `ssh -o
    /// BatchMode=yes` cannot supply one interactively, and the only option
    /// when no `ssh` binary is on `PATH` at all.
    Russh,
}

/// Whether a binary named `name` exists on `PATH` — a plain file-exists
/// check, not a spawn-and-see: cheap, and avoids the interactive-hang risk
/// a real invocation could carry on a misconfigured `PATH`.
fn binary_on_path(name: &str, path_var: Option<&std::ffi::OsStr>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    std::env::split_paths(path_var)
        .any(|dir| dir.join(name).is_file() || dir.join(format!("{name}.exe")).is_file())
}

/// [`select`]'s decision rule, parameterised on whether `ssh` is on `PATH`
/// so it is testable without a real binary or `$PATH` (ADR-0061 §5's
/// "CLI first" default, narrowed to the auth modes it can actually
/// carry).
fn select_with(auth: SshAuthMode, ssh_on_path: bool) -> SelectedTunnel {
    let cli_capable_auth = matches!(
        auth,
        SshAuthMode::Agent | SshAuthMode::KeyFile | SshAuthMode::SshConfig
    );
    if ssh_on_path && cli_capable_auth {
        SelectedTunnel::Cli
    } else {
        SelectedTunnel::Russh
    }
}

/// Pick a tunnel mechanism for `auth`: the CLI (`ssh` on `PATH`) when it
/// can carry this auth mode, `RusshTunnel` otherwise — a password auth
/// mode always selects `Russh` (`ssh -o BatchMode=yes` cannot prompt for
/// one), and so does a missing `ssh` binary.
pub fn select(auth: SshAuthMode) -> SelectedTunnel {
    select_with(
        auth,
        binary_on_path("ssh", std::env::var_os("PATH").as_deref()),
    )
}

/// Ask the OS for an ephemeral port, then release it immediately — the
/// standard "pick a free port" trick: the window between releasing the
/// listener and `ssh` binding the same port is a real (if narrow) race,
/// accepted the same way every other free-port picker in this ecosystem
/// accepts it, since a bind failure here degrades to a typed
/// [`DbErrorCode::TunnelFailed`] rather than a hang.
fn pick_free_port() -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// The exact `ssh` argv [`CliTunnel::open`] runs — a pure function so the
/// byte-for-byte shape is testable without a real `ssh` binary or a real
/// bastion.
///
/// `-N`: no remote command, just forward. `-o BatchMode=yes`: a tunnel that
/// would need interactive input fails fast rather than hanging the connect
/// flow. `-o ExitOnForwardFailure=yes`: a forward that could not bind exits
/// immediately instead of leaving a useless session running. The forwarded
/// port binds to `127.0.0.1` only, never a wildcard address (ADR-0061 §5).
pub fn build_argv(local_port: u16, spec: &TunnelSpec) -> Vec<String> {
    vec![
        "-N".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-p".to_string(),
        spec.ssh_port.to_string(),
        "-L".to_string(),
        format!(
            "127.0.0.1:{local_port}:{}:{}",
            spec.remote_host, spec.remote_port
        ),
        format!("{}@{}", spec.ssh_user, spec.ssh_host),
    ]
}

/// Poll `127.0.0.1:port` until a TCP connect succeeds or `timeout` elapses.
pub fn wait_until_ready(port: u16, timeout: Duration) -> Result<(), DbError> {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(DbError::new(
                DbErrorCode::TunnelFailed,
                format!("SSH tunnel did not become ready on port {port} within {timeout:?}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A tunnel opened by shelling out to the user's own `ssh` binary
/// (`process_exec::spawn`). Kills the child on `Drop`, so a session that
/// closes its connection (or panics) never leaves a forwarding `ssh`
/// running past it.
pub struct CliTunnel {
    local_port: u16,
    child: process_exec::Spawned,
}

impl CliTunnel {
    /// Picks a free local port, spawns `ssh` to forward it, and waits up
    /// to `ready_timeout` for the forwarded port to accept a connection.
    pub fn open(spec: &TunnelSpec, ready_timeout: Duration) -> Result<Self, DbError> {
        let local_port = pick_free_port().map_err(|error| {
            DbError::new(
                DbErrorCode::TunnelFailed,
                format!("could not reserve a local port: {error}"),
            )
        })?;
        let argv = build_argv(local_port, spec);
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let work_dir = std::env::temp_dir();
        let child = process_exec::spawn("ssh", &args, &work_dir).map_err(|failure| {
            DbError::new(
                DbErrorCode::TunnelFailed,
                format!("could not start ssh: {failure:?}"),
            )
        })?;
        wait_until_ready(local_port, ready_timeout)?;
        Ok(Self { local_port, child })
    }
}

impl Tunnel for CliTunnel {
    fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for CliTunnel {
    fn drop(&mut self) {
        self.child.kill();
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn agent_key_file_and_ssh_config_prefer_cli_when_ssh_is_on_path() {
        assert_eq!(select_with(SshAuthMode::Agent, true), SelectedTunnel::Cli);
        assert_eq!(select_with(SshAuthMode::KeyFile, true), SelectedTunnel::Cli);
        assert_eq!(
            select_with(SshAuthMode::SshConfig, true),
            SelectedTunnel::Cli
        );
    }

    #[test]
    fn password_auth_always_selects_russh_even_with_ssh_on_path() {
        assert_eq!(
            select_with(SshAuthMode::Password, true),
            SelectedTunnel::Russh
        );
    }

    #[test]
    fn a_missing_ssh_binary_always_selects_russh() {
        assert_eq!(
            select_with(SshAuthMode::Agent, false),
            SelectedTunnel::Russh
        );
        assert_eq!(
            select_with(SshAuthMode::KeyFile, false),
            SelectedTunnel::Russh
        );
    }

    #[test]
    fn auth_mode_from_id_maps_the_known_strings_and_defaults_to_agent() {
        assert_eq!(SshAuthMode::from_id("password"), SshAuthMode::Password);
        assert_eq!(SshAuthMode::from_id("key"), SshAuthMode::KeyFile);
        assert_eq!(SshAuthMode::from_id("config"), SshAuthMode::SshConfig);
        assert_eq!(SshAuthMode::from_id("agent"), SshAuthMode::Agent);
        assert_eq!(SshAuthMode::from_id("anything-else"), SshAuthMode::Agent);
    }

    #[test]
    fn ssh_config_debug_never_prints_the_password() {
        let config = SshConfig {
            host: "bastion".to_string(),
            port: 22,
            user: "florian".to_string(),
            auth: SshAuthMode::Password,
            key_file: None,
            password: Some("hunter2".to_string()),
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn binary_on_path_finds_a_real_binary_and_misses_a_fake_one() {
        assert!(binary_on_path(
            "sh",
            Some(std::ffi::OsStr::new("/bin:/usr/bin"))
        ));
        assert!(!binary_on_path(
            "not-a-real-binary-xyz",
            Some(std::ffi::OsStr::new("/bin:/usr/bin"))
        ));
    }

    #[test]
    fn binary_on_path_is_false_with_no_path_variable() {
        assert!(!binary_on_path("sh", None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TunnelSpec {
        TunnelSpec {
            ssh_host: "bastion.example".to_string(),
            ssh_port: 22,
            ssh_user: "florian".to_string(),
            remote_host: "db.internal".to_string(),
            remote_port: 5432,
        }
    }

    #[test]
    fn build_argv_is_byte_for_byte_the_documented_shape() {
        let argv = build_argv(54321, &spec());
        assert_eq!(
            argv,
            vec![
                "-N",
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-p",
                "22",
                "-L",
                "127.0.0.1:54321:db.internal:5432",
                "florian@bastion.example",
            ]
        );
    }

    #[test]
    fn the_forwarded_port_always_binds_to_loopback_only() {
        let argv = build_argv(1, &spec());
        let forward = argv
            .iter()
            .find(|arg| arg.contains(":db.internal:"))
            .unwrap();
        assert!(forward.starts_with("127.0.0.1:"));
    }

    #[test]
    fn pick_free_port_returns_a_port_that_can_be_bound_again() {
        let port = pick_free_port().expect("a free port");
        assert!(port > 0);
        // Provably free: binding it again immediately succeeds.
        assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    #[test]
    fn wait_until_ready_times_out_on_a_port_nothing_listens_on() {
        // A free port that nothing is bound to right now.
        let port = pick_free_port().expect("a free port");
        let result = wait_until_ready(port, Duration::from_millis(150));
        assert_eq!(result.unwrap_err().code, DbErrorCode::TunnelFailed);
    }

    #[test]
    fn wait_until_ready_succeeds_once_something_listens() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        // Accept in the background so the connect this test drives
        // actually completes rather than sitting in the OS backlog only.
        let handle = std::thread::spawn(move || {
            let _ = listener.accept();
        });
        wait_until_ready(port, Duration::from_secs(2)).expect("ready");
        handle.join().ok();
    }
}
