//! Dashboard editing -> recreate (C9): a container's `inspect` JSON turned
//! into an editable [`RunSpec`], a `run` argv compiled back from it, and
//! [`recreate`] itself — `rm -f <id>` then `run …` — the same "no
//! `container update`-only edit path" tradeoff JetBrains' own Docker plugin
//! makes: Docker/Podman have no general "change this container's config in
//! place" command, so editing *is* remove-and-recreate.
//!
//! Compose-managed containers are refused before any of this runs
//! ([`is_compose_managed`]) — a compose file is that container's real
//! source of truth, and a one-off `recreate` would immediately drift from
//! it on the next `compose up`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::connection::Invocation;
use crate::model::{null_default, Container, Mount, PortBinding};
use crate::ops::{run_op, OpError, OpErrorCode};

/// One network a container is (or should be, after recreate) attached to,
/// with its per-network aliases (`docker run --network-alias`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkSpec {
    pub name: String,
    pub aliases: Vec<String>,
}

/// Everything [`recreate_argv`] needs to reproduce (or, once edited,
/// change) a container's `run` invocation. Built from `inspect` JSON by
/// [`RunSpec::from_inspect`]; the Dashboard's Add/Edit/Remove tables mutate
/// a clone through [`RunSpec::with_env`]/[`RunSpec::with_ports`]/
/// [`RunSpec::with_mounts`] and compare it against the original for the
/// "Recreate with changes" button's dirty state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunSpec {
    pub image: String,
    pub name: String,
    pub env: Vec<(String, String)>,
    pub ports: Vec<PortBinding>,
    /// Bind mounts and named volumes both — [`Mount::kind`] is `"bind"` or
    /// `"volume"`, [`Mount::name`] the volume name for the latter.
    pub mounts: Vec<Mount>,
    pub cmd: Vec<String>,
    pub entrypoint: Vec<String>,
    pub working_dir: String,
    pub user: String,
    pub networks: Vec<NetworkSpec>,
    /// `""` (none/`no`), `"always"`, `"unless-stopped"`, `"on-failure"` or
    /// `"on-failure:<count>"` — [`app_config::container_run`]'s own
    /// convention would work here too, but this crate cannot depend back
    /// on `app-config`'s run-config types for a struct that is not a
    /// persisted setting, so the policy stays this one plain string.
    pub restart_policy: String,
    /// Compose (`com.docker.compose.*`/`io.podman.compose.*`) and any
    /// `ide.*` label already dropped by [`RunSpec::from_inspect`] — a
    /// recreated container starts label-clean of markers that describe a
    /// different management path.
    pub labels: BTreeMap<String, String>,
    pub hostname: String,
    pub privileged: bool,
    /// `host:ip` entries, `docker run --add-host`'s own format.
    pub extra_hosts: Vec<String>,
    pub publish_all: bool,
}

impl RunSpec {
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    pub fn with_ports(mut self, ports: Vec<PortBinding>) -> Self {
        self.ports = ports;
        self
    }

    pub fn with_mounts(mut self, mounts: Vec<Mount>) -> Self {
        self.mounts = mounts;
        self
    }

    /// Read every field [`recreate_argv`] needs off a container's own
    /// `inspect` JSON. [`Container`] itself already carries image/name/env/
    /// cmd/entrypoint/mounts/ports/labels; the rest
    /// (`HostConfig`/`Config.WorkingDir`/`Config.User`/`Config.Hostname`/
    /// `NetworkSettings.Networks`) is not modeled on [`Container`] — nothing
    /// else needs it — so it is read straight off [`Container::raw`], the
    /// field [`crate::model`]'s own doc comment says exists for exactly
    /// this: "the Inspect tab a later task adds" (this is that later task).
    pub fn from_inspect(container: &Container) -> Self {
        let extra: RawExtra = serde_json::from_value(container.raw.clone()).unwrap_or_default();
        let labels = container
            .labels
            .iter()
            .filter(|(key, _)| !is_managed_label(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let networks = extra
            .network_settings
            .networks
            .into_iter()
            .map(|(name, endpoint)| NetworkSpec {
                name,
                aliases: endpoint.aliases,
            })
            .collect();
        RunSpec {
            image: container.image.clone(),
            name: container.name.clone(),
            env: container
                .env
                .iter()
                .map(|entry| match entry.split_once('=') {
                    Some((key, value)) => (key.to_string(), value.to_string()),
                    None => (entry.clone(), String::new()),
                })
                .collect(),
            ports: container.ports.clone(),
            mounts: container.mounts.clone(),
            cmd: container.cmd.clone(),
            entrypoint: container.entrypoint.clone(),
            working_dir: extra.config.working_dir,
            user: extra.config.user,
            networks,
            restart_policy: restart_policy_string(&extra.host_config.restart_policy),
            labels,
            hostname: extra.config.hostname,
            privileged: extra.host_config.privileged,
            extra_hosts: extra.host_config.extra_hosts,
            publish_all: extra.host_config.publish_all_ports,
        }
    }
}

/// A label a recreated container never keeps: it names a different
/// management path (a compose project, or a future `ide.*` marker this
/// integration itself might write) that a one-off `recreate` cannot stay
/// truthful to.
fn is_managed_label(key: &str) -> bool {
    key.starts_with("com.docker.compose.")
        || key.starts_with("io.podman.compose.")
        || key.starts_with("ide.")
}

/// Whether `container` is managed by a compose project — [`recreate`] must
/// never run on one of these; the compose file is its real source of
/// truth, and `recreate` would immediately drift from it. Bridged to
/// `nodeActions.canRecreate` (`false` here).
pub fn is_compose_managed(container: &Container) -> bool {
    container.labels.keys().any(|key| is_managed_label(key))
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawRestartPolicy {
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "MaximumRetryCount")]
    maximum_retry_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawHostConfig {
    #[serde(default, rename = "RestartPolicy")]
    restart_policy: RawRestartPolicy,
    #[serde(default, rename = "Privileged")]
    privileged: bool,
    #[serde(default, rename = "ExtraHosts", deserialize_with = "null_default")]
    extra_hosts: Vec<String>,
    #[serde(default, rename = "PublishAllPorts")]
    publish_all_ports: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawConfig {
    #[serde(default, rename = "WorkingDir")]
    working_dir: String,
    #[serde(default, rename = "User")]
    user: String,
    #[serde(default, rename = "Hostname")]
    hostname: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawNetworkEndpoint {
    #[serde(default, rename = "Aliases", deserialize_with = "null_default")]
    aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawNetworkSettings {
    #[serde(default, rename = "Networks", deserialize_with = "null_default")]
    networks: BTreeMap<String, RawNetworkEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawExtra {
    #[serde(default, rename = "HostConfig")]
    host_config: RawHostConfig,
    #[serde(default, rename = "Config")]
    config: RawConfig,
    #[serde(default, rename = "NetworkSettings")]
    network_settings: RawNetworkSettings,
}

/// `{Name: "no"}`/`{}` -> `""`; `{Name: "on-failure", MaximumRetryCount: 3}`
/// -> `"on-failure:3"`; anything else -> the bare policy name.
fn restart_policy_string(policy: &RawRestartPolicy) -> String {
    match policy.name.as_str() {
        "" | "no" => String::new(),
        "on-failure" if policy.maximum_retry_count > 0 => {
            format!("on-failure:{}", policy.maximum_retry_count)
        }
        name => name.to_string(),
    }
}

/// One `-p`/`--publish` argument from an `inspect`-derived [`PortBinding`]
/// (its own `protocol` is never empty the way
/// `app_config::container_run::PortBinding`'s can be — `inspect` always
/// prints one).
fn port_arg(binding: &PortBinding) -> String {
    let mut spec = String::new();
    if !binding.host_ip.is_empty() {
        spec.push_str(&binding.host_ip);
        spec.push(':');
    }
    spec.push_str(&binding.host_port);
    spec.push(':');
    spec.push_str(&binding.container_port);
    if !binding.protocol.is_empty() && binding.protocol != "tcp" {
        spec.push('/');
        spec.push_str(&binding.protocol);
    }
    spec
}

/// One `-v` argument for `mount` — a bind mount goes through
/// [`crate::run_config::mount_arg`] (so the SELinux `:z` rule stays the
/// one function [`crate::selinux::relabel_suffix`] backs); a named volume
/// never takes `:z` (it is not a host path SELinux labels apply to), only
/// `:ro`.
fn mount_argument(mount: &Mount, selinux_relabel: bool) -> String {
    if mount.kind == "volume" && !mount.name.is_empty() {
        if mount.read_write {
            format!("{}:{}", mount.name, mount.destination)
        } else {
            format!("{}:{}:ro", mount.name, mount.destination)
        }
    } else {
        crate::run_config::mount_arg(
            &mount.source,
            &mount.destination,
            !mount.read_write,
            selinux_relabel,
        )
    }
}

/// `run -d --name … <flags from every RunSpec field> <image> <entrypoint
/// tail + cmd>`.
///
/// Known ceiling: `docker run`/`podman run` accept exactly one `--network`
/// (attaching a second and later network needs a separate `network
/// connect` call this function does not make) — only `spec.networks`'s
/// first entry becomes `--network`/`--network-alias`; a container attached
/// to more than one network at recreate time keeps only the first here.
/// Upgrade path: `recreate` growing a follow-up `network connect` call per
/// extra network once the Dashboard actually offers editing more than one.
pub fn recreate_argv(spec: &RunSpec, selinux_relabel: bool) -> Vec<String> {
    let mut args = vec!["run".to_string(), "-d".to_string()];
    if !spec.name.is_empty() {
        args.push("--name".to_string());
        args.push(spec.name.clone());
    }
    if !spec.hostname.is_empty() {
        args.push("--hostname".to_string());
        args.push(spec.hostname.clone());
    }
    if !spec.user.is_empty() {
        args.push("-u".to_string());
        args.push(spec.user.clone());
    }
    if !spec.working_dir.is_empty() {
        args.push("-w".to_string());
        args.push(spec.working_dir.clone());
    }
    if spec.privileged {
        args.push("--privileged".to_string());
    }
    if spec.publish_all {
        args.push("-P".to_string());
    }
    for port in &spec.ports {
        args.push("-p".to_string());
        args.push(port_arg(port));
    }
    for mount in &spec.mounts {
        args.push("-v".to_string());
        args.push(mount_argument(mount, selinux_relabel));
    }
    for (key, value) in &spec.env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    for host in &spec.extra_hosts {
        args.push("--add-host".to_string());
        args.push(host.clone());
    }
    if let Some(network) = spec.networks.first() {
        if !network.name.is_empty() {
            args.push("--network".to_string());
            args.push(network.name.clone());
            for alias in &network.aliases {
                args.push("--network-alias".to_string());
                args.push(alias.clone());
            }
        }
    }
    if !spec.restart_policy.is_empty() {
        args.push("--restart".to_string());
        args.push(spec.restart_policy.clone());
    }
    for (key, value) in &spec.labels {
        args.push("--label".to_string());
        args.push(format!("{key}={value}"));
    }
    let mut command = spec.cmd.clone();
    if let Some((first, rest)) = spec.entrypoint.split_first() {
        args.push("--entrypoint".to_string());
        args.push(first.clone());
        let mut full = rest.to_vec();
        full.extend(command);
        command = full;
    }
    args.push(spec.image.clone());
    args.extend(command);
    args
}

/// Remove the container at `id` (`rm -f`) and `run` a new one from `spec`.
/// The confirm dialog states this ahead of time; the two-step shape means a
/// `run` failure leaves the old container already gone, which
/// [`OpErrorCode::OldContainerRemoved`] reports explicitly rather than as
/// an ordinary [`OpErrorCode::Other`].
///
/// `Ok(new_id)` — the id `docker run -d`/`podman run -d` prints on stdout.
pub fn recreate(
    invocation: &Invocation,
    id: &str,
    spec: &RunSpec,
    selinux_relabel: bool,
    work_dir: &Path,
) -> Result<String, OpError> {
    run_op(invocation, &crate::ops::remove_args(id, true), work_dir)?;
    let argv = recreate_argv(spec, selinux_relabel);
    match run_op(invocation, &argv, work_dir) {
        Ok(output) => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        Err(run_err) => Err(OpError {
            code: OpErrorCode::OldContainerRemoved,
            message: format!(
                "the old container was removed, but the recreated container failed to start: {run_err}"
            ),
        }),
    }
}

/// [`RunSpec::env`] as `"KEY=VALUE"` lines — the Dashboard's Env table
/// crosses the FFI seam as one `\n`-joined `QString`, the same convention
/// every other multi-row Dashboard field in this integration uses
/// (`FfiNetworkDashboard::labels`, ...).
pub fn format_env_lines(env: &[(String, String)]) -> Vec<String> {
    env.iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// The inverse of [`format_env_lines`]: a line with no `=` becomes a
/// key with an empty value rather than being dropped — the Dashboard's
/// table already validates its own rows before it lets one through with
/// no `=`; this stays total either way.
pub fn parse_env_lines(lines: &[String]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| match line.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (line.clone(), String::new()),
        })
        .collect()
}

/// [`RunSpec::ports`] as `"host_ip:host_port:container_port/protocol"`
/// lines (protocol omitted when `tcp`, matching [`port_arg`]'s own
/// shorthand).
pub fn format_port_lines(ports: &[PortBinding]) -> Vec<String> {
    ports.iter().map(port_arg).collect()
}

/// The inverse of [`format_port_lines`]. A line missing its `host_port:
/// container_port` shape is skipped rather than erroring — same "the
/// table validates, this stays total" rule as [`parse_env_lines`].
pub fn parse_port_lines(lines: &[String]) -> Vec<PortBinding> {
    lines
        .iter()
        .filter_map(|line| {
            let (host, container_and_proto) = line.rsplit_once(':')?;
            let (container_port, protocol) = match container_and_proto.split_once('/') {
                Some((port, proto)) => (port, proto),
                None => (container_and_proto, "tcp"),
            };
            let (host_ip, host_port) = match host.rsplit_once(':') {
                Some((ip, port)) => (ip.to_string(), port.to_string()),
                None => (String::new(), host.to_string()),
            };
            if container_port.is_empty() || host_port.is_empty() {
                return None;
            }
            Some(PortBinding {
                host_ip,
                host_port,
                container_port: container_port.to_string(),
                protocol: protocol.to_string(),
            })
        })
        .collect()
}

/// [`RunSpec::mounts`] as `"<kind>|<source>|<target>|<ro>"` lines
/// (`kind` is `bind`/`volume`, `ro` is `1`/`0`) — `|` rather than `:`
/// because a Windows bind-mount source (`C:\...`) already contains `:`.
pub fn format_mount_lines(mounts: &[Mount]) -> Vec<String> {
    mounts
        .iter()
        .map(|mount| {
            let kind = if mount.kind == "volume" {
                "volume"
            } else {
                "bind"
            };
            let source = if kind == "volume" {
                &mount.name
            } else {
                &mount.source
            };
            let ro = if mount.read_write { "0" } else { "1" };
            format!("{kind}|{source}|{}|{ro}", mount.destination)
        })
        .collect()
}

/// The inverse of [`format_mount_lines`]. A malformed line (not exactly
/// four `|`-separated fields) is skipped.
pub fn parse_mount_lines(lines: &[String]) -> Vec<Mount> {
    lines
        .iter()
        .filter_map(|line| {
            let fields: Vec<&str> = line.splitn(4, '|').collect();
            let [kind, source, target, ro] = fields.as_slice() else {
                return None;
            };
            let read_write = *ro != "1";
            Some(if *kind == "volume" {
                Mount {
                    kind: "volume".to_string(),
                    name: source.to_string(),
                    source: String::new(),
                    destination: target.to_string(),
                    read_write,
                }
            } else {
                Mount {
                    kind: "bind".to_string(),
                    name: String::new(),
                    source: source.to_string(),
                    destination: target.to_string(),
                    read_write,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Container;

    fn container_from(json: &str) -> Container {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        Container::from_value(value).unwrap()
    }

    fn fixture(name: &str) -> Vec<Container> {
        let path = format!(
            "{}/testdata/inspect/{name}/containers.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let json = std::fs::read_to_string(path).unwrap();
        crate::model::parse_list(&json, Container::from_value).unwrap()
    }

    #[test]
    fn docker_fixture_round_trips_image_env_mounts_and_network() {
        let containers = fixture("docker");
        let redis = containers.iter().find(|c| c.name == "redis").unwrap();
        let spec = RunSpec::from_inspect(redis);
        assert_eq!(spec.image, "redis:7.2-alpine");
        assert!(spec
            .env
            .contains(&("REDIS_VERSION".to_string(), "7.2.15".to_string())));
        assert_eq!(spec.working_dir, "/data");
        assert_eq!(spec.restart_policy, "always");
        assert_eq!(spec.mounts.len(), 1);
        assert!(spec.mounts[0].read_write);
        assert_eq!(spec.networks.len(), 1);
        assert!(spec.networks[0].aliases.contains(&"redis".to_string()));

        let argv = recreate_argv(&spec, false);
        assert!(argv.contains(&"redis:7.2-alpine".to_string()));
        assert!(argv.windows(2).any(|w| w == ["--restart", "always"]));
        assert!(argv.windows(2).any(|w| w == ["-w", "/data"]));
        assert!(argv
            .windows(2)
            .any(|w| w == ["--network", "event-sourcing-chess-example_default"]));
        assert!(argv.windows(2).any(|w| w == ["--network-alias", "redis"]));
        assert!(argv
            .iter()
            .any(|a| a.starts_with("/home/user/projects") && a.ends_with(":/data")));
    }

    #[test]
    fn named_volume_mounts_use_the_volume_name_as_the_source() {
        let containers = fixture("docker");
        let web = containers
            .iter()
            .find(|c| c.name.contains("web-1"))
            .unwrap();
        let spec = RunSpec::from_inspect(web);
        let argv = recreate_argv(&spec, false);
        assert!(argv
            .iter()
            .any(|a| a == "jagdonline-2d80272f_web_node_modules:/app/apps/web/node_modules"));
        assert!(argv
            .iter()
            .any(|a| a == "/app:/app" || a.ends_with(":/app")));
    }

    #[test]
    fn selinux_relabel_applies_only_to_bind_mounts() {
        let containers = fixture("docker");
        let web = containers
            .iter()
            .find(|c| c.name.contains("web-1"))
            .unwrap();
        let spec = RunSpec::from_inspect(web);
        let argv = recreate_argv(&spec, true);
        assert!(
            argv.iter()
                .any(|a| a == "jagdonline-2d80272f_web_node_modules:/app/apps/web/node_modules"),
            "named volume mount must not gain :z: {argv:?}"
        );
        let bind_mount = argv
            .iter()
            .find(|a| a.ends_with(":/app:z"))
            .expect("bind mount should gain :z");
        assert!(bind_mount.starts_with('/'));
    }

    #[test]
    fn podman_fixture_round_trips_labels_minus_compose_markers() {
        let containers = fixture("podman");
        let container = &containers[0];
        let spec = RunSpec::from_inspect(container);
        assert!(!is_managed_label_present(&spec.labels));
        assert!(!is_compose_managed(container) || spec.labels.is_empty());
    }

    fn is_managed_label_present(labels: &BTreeMap<String, String>) -> bool {
        labels.keys().any(|key| is_managed_label(key))
    }

    #[test]
    fn on_failure_restart_policy_carries_its_retry_count() {
        let raw = RawRestartPolicy {
            name: "on-failure".to_string(),
            maximum_retry_count: 3,
        };
        assert_eq!(restart_policy_string(&raw), "on-failure:3");
        let argv_spec = RunSpec {
            restart_policy: "on-failure:3".to_string(),
            image: "x".to_string(),
            ..RunSpec::default()
        };
        let argv = recreate_argv(&argv_spec, false);
        assert!(argv.windows(2).any(|w| w == ["--restart", "on-failure:3"]));
    }

    #[test]
    fn no_restart_policy_omits_the_flag() {
        let raw = RawRestartPolicy {
            name: "no".to_string(),
            maximum_retry_count: 0,
        };
        assert_eq!(restart_policy_string(&raw), "");
    }

    #[test]
    fn entrypoint_override_moves_the_command_after_it() {
        let spec = RunSpec {
            image: "img".to_string(),
            entrypoint: vec!["/bin/wrapper".to_string(), "--flag".to_string()],
            cmd: vec!["arg1".to_string()],
            ..RunSpec::default()
        };
        let argv = recreate_argv(&spec, false);
        let entrypoint_index = argv.iter().position(|a| a == "--entrypoint").unwrap();
        assert_eq!(argv[entrypoint_index + 1], "/bin/wrapper");
        let image_index = argv.iter().position(|a| a == "img").unwrap();
        assert_eq!(&argv[image_index + 1..], &["--flag", "arg1"]);
    }

    #[test]
    fn compose_managed_containers_are_flagged() {
        let containers = fixture("docker");
        let web = containers
            .iter()
            .find(|c| c.name.contains("web-1"))
            .unwrap();
        assert!(is_compose_managed(web));
        let standalone = container_from(r#"{"Id":"abc","Name":"/x","Config":{"Image":"img"}}"#);
        assert!(!is_compose_managed(&standalone));
    }

    #[test]
    fn missing_extra_fields_degrade_to_empty_rather_than_failing() {
        let container = container_from(r#"{"Id":"abc","Name":"/x","Config":{"Image":"img"}}"#);
        let spec = RunSpec::from_inspect(&container);
        assert_eq!(spec.working_dir, "");
        assert_eq!(spec.restart_policy, "");
        assert!(spec.networks.is_empty());
        assert!(!spec.privileged);
    }

    #[test]
    fn publish_all_and_privileged_and_extra_hosts_round_trip() {
        let container = container_from(
            r#"{"Id":"abc","Name":"/x","Config":{"Image":"img"},"HostConfig":{"Privileged":true,"PublishAllPorts":true,"ExtraHosts":["db:10.0.0.1"]}}"#,
        );
        let spec = RunSpec::from_inspect(&container);
        assert!(spec.privileged);
        assert!(spec.publish_all);
        assert_eq!(spec.extra_hosts, vec!["db:10.0.0.1".to_string()]);
        let argv = recreate_argv(&spec, false);
        assert!(argv.contains(&"--privileged".to_string()));
        assert!(argv.contains(&"-P".to_string()));
        assert!(argv.windows(2).any(|w| w == ["--add-host", "db:10.0.0.1"]));
    }

    // ------------------------------------------------- FFI line formats --

    #[test]
    fn env_lines_round_trip() {
        let env = vec![
            ("FOO".to_string(), "bar".to_string()),
            ("EMPTY".to_string(), String::new()),
        ];
        let lines = format_env_lines(&env);
        assert_eq!(lines, vec!["FOO=bar".to_string(), "EMPTY=".to_string()]);
        assert_eq!(parse_env_lines(&lines), env);
    }

    #[test]
    fn port_lines_round_trip_including_host_ip_and_udp() {
        let ports = vec![
            PortBinding {
                host_ip: String::new(),
                host_port: "8080".to_string(),
                container_port: "80".to_string(),
                protocol: "tcp".to_string(),
            },
            PortBinding {
                host_ip: "127.0.0.1".to_string(),
                host_port: "53".to_string(),
                container_port: "53".to_string(),
                protocol: "udp".to_string(),
            },
        ];
        let lines = format_port_lines(&ports);
        assert_eq!(
            lines,
            vec!["8080:80".to_string(), "127.0.0.1:53:53/udp".to_string()]
        );
        assert_eq!(parse_port_lines(&lines), ports);
    }

    #[test]
    fn mount_lines_round_trip_bind_and_named_volume() {
        let mounts = vec![
            Mount {
                kind: "bind".to_string(),
                name: String::new(),
                source: "/home/f/data".to_string(),
                destination: "/data".to_string(),
                read_write: false,
            },
            Mount {
                kind: "volume".to_string(),
                name: "app_node_modules".to_string(),
                source: "/var/lib/docker/volumes/app_node_modules/_data".to_string(),
                destination: "/app/node_modules".to_string(),
                read_write: true,
            },
        ];
        let lines = format_mount_lines(&mounts);
        assert_eq!(
            lines,
            vec![
                "bind|/home/f/data|/data|1".to_string(),
                "volume|app_node_modules|/app/node_modules|0".to_string(),
            ]
        );
        let parsed = parse_mount_lines(&lines);
        assert_eq!(parsed[0].source, "/home/f/data");
        assert_eq!(parsed[0].destination, "/data");
        assert!(!parsed[0].read_write);
        assert_eq!(parsed[1].name, "app_node_modules");
        assert_eq!(parsed[1].destination, "/app/node_modules");
        assert!(parsed[1].read_write);
    }

    #[test]
    fn malformed_lines_are_skipped_rather_than_erroring() {
        assert!(parse_port_lines(&["nonsense".to_string()]).is_empty());
        assert!(parse_mount_lines(&["only|two".to_string()]).is_empty());
    }
}
