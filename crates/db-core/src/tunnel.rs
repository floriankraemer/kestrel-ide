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
/// `spec.remote_host:remote_port` through `spec.ssh_host`.
pub trait Tunnel: Send {
    /// The local port traffic should be sent to once open.
    fn local_port(&self) -> u16;
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
