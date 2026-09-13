//! Finding connections the user has not typed in by hand yet: the CLI's own
//! contexts/connections/machines, plus a couple of well-known socket
//! presets. Feeds the Settings page's "Add from contexts..." menu (C1) and,
//! later, the Containers dock's "+ Add" menu.
//!
//! Every parser here is pure — takes the CLI's already-captured stdout and
//! returns candidates — so it is tested against fixture JSON under
//! `testdata/discovery/` with no process spawned. The one call that
//! actually runs a CLI lives in `bridge::containers` (ui-shell), not here.

use serde::Deserialize;

use crate::connection::{ConnectionKind, Engine};

/// One connection this build found on its own, offered to "Add from
/// contexts..." (or a socket-preset menu entry) rather than added silently
/// — discovery only ever suggests, the user still decides whether it
/// becomes a saved [`app_config::ContainerConnectionSetting`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConnection {
    /// What the menu entry shows, e.g. `"default (unix:///var/run/docker.sock)"`.
    pub label: String,
    pub engine: Engine,
    pub kind: ConnectionKind,
}

/// `docker context ls --format json`'s output: **line-delimited** JSON —
/// unlike `podman`'s single JSON array — one object per line, each shaped
/// like `{"Name":"default","Current":true,"DockerEndpoint":"unix:///var/run/docker.sock"}`.
/// A blank line (trailing newline) and a line this build cannot parse are
/// both skipped rather than failing the whole list — one malformed context
/// should not hide every other one.
#[derive(Deserialize)]
struct DockerContextLine {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "DockerEndpoint", default)]
    endpoint: String,
}

pub fn parse_docker_contexts(stdout: &str) -> Vec<DiscoveredConnection> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<DockerContextLine>(line).ok())
        .map(|context| DiscoveredConnection {
            label: if context.endpoint.is_empty() {
                context.name.clone()
            } else {
                format!("{} ({})", context.name, context.endpoint)
            },
            engine: Engine::Docker,
            kind: ConnectionKind::Context { name: context.name },
        })
        .collect()
}

/// `podman system connection list --format json`: a single JSON array,
/// e.g. `[{"Name":"remote","URI":"ssh://user@host/run/podman/podman.sock","Identity":"~/.ssh/id"}]`.
#[derive(Deserialize)]
struct PodmanConnectionEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "URI", default)]
    uri: String,
}

pub fn parse_podman_connections(stdout: &str) -> Vec<DiscoveredConnection> {
    let entries: Vec<PodmanConnectionEntry> = serde_json::from_str(stdout).unwrap_or_default();
    entries
        .into_iter()
        .map(|entry| DiscoveredConnection {
            label: if entry.uri.is_empty() {
                entry.name.clone()
            } else {
                format!("{} ({})", entry.name, entry.uri)
            },
            engine: Engine::Podman,
            kind: ConnectionKind::Context { name: entry.name },
        })
        .collect()
}

/// `podman machine list --format json`: a single JSON array, e.g.
/// `[{"Name":"podman-machine-default","Running":true,"VMType":"qemu"}]`.
#[derive(Deserialize)]
struct PodmanMachineEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Running", default)]
    running: bool,
}

pub fn parse_podman_machines(stdout: &str) -> Vec<DiscoveredConnection> {
    let entries: Vec<PodmanMachineEntry> = serde_json::from_str(stdout).unwrap_or_default();
    entries
        .into_iter()
        .map(|entry| DiscoveredConnection {
            label: format!(
                "{} ({})",
                entry.name,
                if entry.running { "running" } else { "stopped" }
            ),
            engine: Engine::Podman,
            kind: ConnectionKind::PodmanMachine { name: entry.name },
        })
        .collect()
}

/// Colima (<https://github.com/abiosoft/colima>) runs a real Docker daemon
/// behind a Unix socket at a fixed, profile-named path — `"default"` unless
/// the user named their instance something else, which this preset does not
/// try to detect (a second preset for a named profile is a one-line follow
/// up if anyone asks for it).
pub fn colima_socket_preset(home_dir: &str) -> DiscoveredConnection {
    let path = format!("{home_dir}/.colima/default/docker.sock");
    DiscoveredConnection {
        label: format!("Colima ({path})"),
        engine: Engine::Docker,
        kind: ConnectionKind::UnixSocket { path },
    }
}

/// Rancher Desktop's Docker-compatible socket, same fixed-path idea as
/// Colima's above.
pub fn rancher_desktop_socket_preset(home_dir: &str) -> DiscoveredConnection {
    let path = format!("{home_dir}/.rd/docker.sock");
    DiscoveredConnection {
        label: format!("Rancher Desktop ({path})"),
        engine: Engine::Docker,
        kind: ConnectionKind::UnixSocket { path },
    }
}

/// The default rootless Podman socket path, `/run/user/<uid>/podman/podman.sock`
/// — what `systemctl --user start podman.socket` publishes, and the address
/// [`ConnectionError::PermissionDenied`](crate::probe::ConnectionError)'s
/// Podman hint tells the user to start.
pub fn default_rootless_podman_socket(uid: u32) -> DiscoveredConnection {
    let path = format!("/run/user/{uid}/podman/podman.sock");
    DiscoveredConnection {
        label: format!("Podman (rootless, {path})"),
        engine: Engine::Podman,
        kind: ConnectionKind::UnixSocket { path },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_context_ls_line_delimited_json() {
        let stdout = include_str!("../testdata/discovery/docker_context_ls.jsonl");
        let contexts = parse_docker_contexts(stdout);
        assert_eq!(contexts.len(), 2);
        assert_eq!(
            contexts[0].kind,
            ConnectionKind::Context {
                name: "default".to_string()
            }
        );
        assert_eq!(contexts[0].engine, Engine::Docker);
        assert_eq!(
            contexts[1].kind,
            ConnectionKind::Context {
                name: "desktop-linux".to_string()
            }
        );
    }

    #[test]
    fn a_blank_trailing_line_is_skipped_not_a_parse_failure() {
        let stdout =
            "{\"Name\":\"default\",\"DockerEndpoint\":\"unix:///var/run/docker.sock\"}\n\n";
        assert_eq!(parse_docker_contexts(stdout).len(), 1);
    }

    #[test]
    fn parses_podman_system_connection_list_json_array() {
        let stdout = include_str!("../testdata/discovery/podman_connection_list.json");
        let connections = parse_podman_connections(stdout);
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].engine, Engine::Podman);
        assert_eq!(
            connections[0].kind,
            ConnectionKind::Context {
                name: "remote".to_string()
            }
        );
    }

    #[test]
    fn parses_podman_machine_list_json_array() {
        let stdout = include_str!("../testdata/discovery/podman_machine_list.json");
        let machines = parse_podman_machines(stdout);
        assert_eq!(machines.len(), 1);
        assert_eq!(
            machines[0].kind,
            ConnectionKind::PodmanMachine {
                name: "podman-machine-default".to_string()
            }
        );
        assert!(machines[0].label.contains("running"));
    }

    #[test]
    fn socket_presets_use_the_documented_fixed_paths() {
        assert_eq!(
            colima_socket_preset("/home/f").kind,
            ConnectionKind::UnixSocket {
                path: "/home/f/.colima/default/docker.sock".to_string()
            }
        );
        assert_eq!(
            rancher_desktop_socket_preset("/home/f").kind,
            ConnectionKind::UnixSocket {
                path: "/home/f/.rd/docker.sock".to_string()
            }
        );
        assert_eq!(
            default_rootless_podman_socket(1000).kind,
            ConnectionKind::UnixSocket {
                path: "/run/user/1000/podman/podman.sock".to_string()
            }
        );
    }
}
