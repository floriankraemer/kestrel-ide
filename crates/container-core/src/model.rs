//! Typed views over `inspect` JSON — containers, images, volumes, networks
//! and (Podman) pods.
//!
//! Lenient by design: Docker and Podman agree on the shape of `inspect`
//! output but not on every field, and both evolve. Every field is
//! `#[serde(default)]` and tolerates an explicit `null`, so a missing or
//! renamed key degrades to an empty value instead of failing the whole
//! snapshot. Each struct keeps the untouched [`serde_json::Value`] it was
//! read from (`raw`) for the Inspect tab a later task adds.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// `#[serde(default)]` alone rejects an explicit `null`; Docker writes
/// `"Labels": null` and `"Entrypoint": null` routinely.
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// Where a container stands, derived from `State` — the six states the
/// dock tree paints and the Dashboard names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerStatus {
    Running,
    Paused,
    Restarting,
    Exited(i64),
    Created,
    Dead,
    /// A `Status` string neither engine documents today; kept verbatim
    /// rather than mis-filed.
    Other(String),
}

impl ContainerStatus {
    /// The status word the tree shows: `running`, `paused`, `restarting`,
    /// `exited (1)`, `created`, `dead`.
    pub fn label(&self) -> String {
        match self {
            ContainerStatus::Running => "running".to_string(),
            ContainerStatus::Paused => "paused".to_string(),
            ContainerStatus::Restarting => "restarting".to_string(),
            ContainerStatus::Exited(code) => format!("exited ({code})"),
            ContainerStatus::Created => "created".to_string(),
            ContainerStatus::Dead => "dead".to_string(),
            ContainerStatus::Other(status) => status.clone(),
        }
    }

    /// Whether the container's process is alive (running or paused) — the
    /// "stopped containers" filter's complement.
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            ContainerStatus::Running | ContainerStatus::Paused | ContainerStatus::Restarting
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct ContainerState {
    #[serde(default, rename = "Status")]
    pub status: String,
    #[serde(default, rename = "Running")]
    pub running: bool,
    #[serde(default, rename = "Paused")]
    pub paused: bool,
    #[serde(default, rename = "Restarting")]
    pub restarting: bool,
    #[serde(default, rename = "Dead")]
    pub dead: bool,
    #[serde(default, rename = "ExitCode")]
    pub exit_code: i64,
    #[serde(default, rename = "StartedAt")]
    pub started_at: String,
    #[serde(default, rename = "FinishedAt")]
    pub finished_at: String,
}

impl ContainerState {
    /// The boolean flags win over the `Status` string where both are
    /// present — `Status` is display text, the flags are what the engine
    /// itself branches on.
    pub fn status(&self) -> ContainerStatus {
        if self.paused {
            ContainerStatus::Paused
        } else if self.restarting {
            ContainerStatus::Restarting
        } else if self.running {
            ContainerStatus::Running
        } else if self.dead {
            ContainerStatus::Dead
        } else {
            match self.status.as_str() {
                "exited" | "stopped" => ContainerStatus::Exited(self.exit_code),
                "created" | "configured" | "initialized" => ContainerStatus::Created,
                "dead" => ContainerStatus::Dead,
                "running" => ContainerStatus::Running,
                "paused" => ContainerStatus::Paused,
                "restarting" => ContainerStatus::Restarting,
                other => ContainerStatus::Other(other.to_string()),
            }
        }
    }
}

/// One published port: `host_ip:host_port -> container_port/protocol`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortBinding {
    pub host_ip: String,
    pub host_port: String,
    pub container_port: String,
    pub protocol: String,
}

impl PortBinding {
    /// `0.0.0.0:8080 -> 80/tcp`, the Dashboard's line for it.
    pub fn display(&self) -> String {
        format!(
            "{}:{} -> {}/{}",
            self.host_ip, self.host_port, self.container_port, self.protocol
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Mount {
    #[serde(default, rename = "Type")]
    pub kind: String,
    /// The volume name for a `volume` mount, empty for a bind mount.
    #[serde(default, rename = "Name")]
    pub name: String,
    #[serde(default, rename = "Source")]
    pub source: String,
    #[serde(default, rename = "Destination")]
    pub destination: String,
    #[serde(default, rename = "RW")]
    pub read_write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawPortHost {
    #[serde(default, rename = "HostIp")]
    host_ip: String,
    #[serde(default, rename = "HostPort")]
    host_port: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawContainerConfig {
    #[serde(default, rename = "Env", deserialize_with = "null_default")]
    env: Vec<String>,
    #[serde(default, rename = "Cmd", deserialize_with = "null_default")]
    cmd: Vec<String>,
    #[serde(default, rename = "Entrypoint", deserialize_with = "null_default")]
    entrypoint: Vec<String>,
    #[serde(default, rename = "Labels", deserialize_with = "null_default")]
    labels: BTreeMap<String, String>,
    #[serde(default, rename = "Image")]
    image: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawNetworkSettings {
    #[serde(default, rename = "Ports", deserialize_with = "null_default")]
    ports: BTreeMap<String, Option<Vec<RawPortHost>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawContainer {
    #[serde(default, rename = "Id")]
    id: String,
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "Created")]
    created: String,
    #[serde(default, rename = "Image")]
    image_id: String,
    /// Podman only: the image reference the container was created from.
    #[serde(default, rename = "ImageName")]
    image_name: String,
    /// Podman only: the pod this container belongs to, empty otherwise.
    #[serde(default, rename = "Pod")]
    pod: String,
    #[serde(default, rename = "State", deserialize_with = "null_default")]
    state: ContainerState,
    #[serde(default, rename = "Mounts", deserialize_with = "null_default")]
    mounts: Vec<Mount>,
    #[serde(default, rename = "Config", deserialize_with = "null_default")]
    config: RawContainerConfig,
    #[serde(default, rename = "NetworkSettings", deserialize_with = "null_default")]
    network_settings: RawNetworkSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub id: String,
    /// Without Docker's leading `/`.
    pub name: String,
    /// The image reference the container was created from (`Config.Image`;
    /// Podman's `ImageName` when that is empty), or the image id.
    pub image: String,
    pub image_id: String,
    pub created: String,
    pub state: ContainerState,
    pub labels: BTreeMap<String, String>,
    pub ports: Vec<PortBinding>,
    pub mounts: Vec<Mount>,
    pub env: Vec<String>,
    pub cmd: Vec<String>,
    pub entrypoint: Vec<String>,
    /// Podman: the owning pod's id, empty when not in a pod.
    pub pod: String,
    pub raw: Value,
}

impl Container {
    pub fn from_value(raw: Value) -> Result<Self, serde_json::Error> {
        let parsed: RawContainer = serde_json::from_value(raw.clone())?;
        let mut ports: Vec<PortBinding> = parsed
            .network_settings
            .ports
            .iter()
            .flat_map(|(port, hosts)| {
                let (container_port, protocol) = port.split_once('/').unwrap_or((port, "tcp"));
                hosts.iter().flatten().map(move |host| PortBinding {
                    host_ip: host.host_ip.clone(),
                    host_port: host.host_port.clone(),
                    container_port: container_port.to_string(),
                    protocol: protocol.to_string(),
                })
            })
            .collect();
        ports.dedup();
        let image = [parsed.config.image.as_str(), parsed.image_name.as_str()]
            .into_iter()
            .find(|candidate| !candidate.is_empty())
            .unwrap_or(parsed.image_id.as_str())
            .to_string();
        Ok(Container {
            id: parsed.id,
            name: parsed.name.trim_start_matches('/').to_string(),
            image,
            image_id: parsed.image_id,
            created: parsed.created,
            state: parsed.state,
            labels: parsed.config.labels,
            ports,
            mounts: parsed.mounts,
            env: parsed.config.env,
            cmd: parsed.config.cmd,
            entrypoint: parsed.config.entrypoint,
            pod: parsed.pod,
            raw,
        })
    }

    pub fn status(&self) -> ContainerStatus {
        self.state.status()
    }

    /// The label under either compose implementation's prefix:
    /// `com.docker.compose.<key>` first, then `io.podman.compose.<key>`.
    pub fn compose_label(&self, key: &str) -> Option<&str> {
        self.labels
            .get(&format!("com.docker.compose.{key}"))
            .or_else(|| self.labels.get(&format!("io.podman.compose.{key}")))
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawImageConfig {
    #[serde(default, rename = "Labels", deserialize_with = "null_default")]
    labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawImage {
    #[serde(default, rename = "Id")]
    id: String,
    #[serde(default, rename = "RepoTags", deserialize_with = "null_default")]
    repo_tags: Vec<String>,
    #[serde(default, rename = "RepoDigests", deserialize_with = "null_default")]
    repo_digests: Vec<String>,
    #[serde(default, rename = "Created")]
    created: String,
    #[serde(default, rename = "Size")]
    size: u64,
    #[serde(default, rename = "Architecture")]
    architecture: String,
    #[serde(default, rename = "Os")]
    os: String,
    #[serde(default, rename = "Config", deserialize_with = "null_default")]
    config: RawImageConfig,
    /// Podman repeats the labels at the top level; Docker only nests them.
    #[serde(default, rename = "Labels", deserialize_with = "null_default")]
    labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// `sha256:...` as the engine prints it.
    pub id: String,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    pub created: String,
    pub size: u64,
    pub architecture: String,
    pub os: String,
    pub labels: BTreeMap<String, String>,
    pub raw: Value,
}

impl Image {
    pub fn from_value(raw: Value) -> Result<Self, serde_json::Error> {
        let parsed: RawImage = serde_json::from_value(raw.clone())?;
        let labels = if parsed.config.labels.is_empty() {
            parsed.labels
        } else {
            parsed.config.labels
        };
        Ok(Image {
            id: parsed.id,
            repo_tags: parsed
                .repo_tags
                .into_iter()
                .filter(|tag| tag != "<none>:<none>")
                .collect(),
            repo_digests: parsed.repo_digests,
            created: parsed.created,
            size: parsed.size,
            architecture: parsed.architecture,
            os: parsed.os,
            labels,
            raw,
        })
    }

    /// A dangling image: no tag points at it.
    pub fn is_untagged(&self) -> bool {
        self.repo_tags.is_empty()
    }

    /// The first tag, or the short id for an untagged image.
    pub fn display_name(&self) -> String {
        self.repo_tags
            .first()
            .cloned()
            .unwrap_or_else(|| short_id(&self.id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawVolume {
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "Driver")]
    driver: String,
    #[serde(default, rename = "Mountpoint")]
    mountpoint: String,
    #[serde(default, rename = "CreatedAt")]
    created_at: String,
    #[serde(default, rename = "Scope")]
    scope: String,
    #[serde(default, rename = "Labels", deserialize_with = "null_default")]
    labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub created_at: String,
    pub scope: String,
    pub labels: BTreeMap<String, String>,
    pub raw: Value,
}

impl Volume {
    pub fn from_value(raw: Value) -> Result<Self, serde_json::Error> {
        let parsed: RawVolume = serde_json::from_value(raw.clone())?;
        Ok(Volume {
            name: parsed.name,
            driver: parsed.driver,
            mountpoint: parsed.mountpoint,
            created_at: parsed.created_at,
            scope: parsed.scope,
            labels: parsed.labels,
            raw,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawNetworkContainer {
    #[serde(default, rename = "Name", alias = "name")]
    name: String,
}

/// Docker capitalises these keys; Podman's `network inspect` writes them
/// in lowercase — hence the aliases.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawNetwork {
    #[serde(default, rename = "Id", alias = "id")]
    id: String,
    #[serde(default, rename = "Name", alias = "name")]
    name: String,
    #[serde(default, rename = "Driver", alias = "driver")]
    driver: String,
    #[serde(default, rename = "Scope", alias = "scope")]
    scope: String,
    #[serde(default, rename = "Created", alias = "created")]
    created: String,
    #[serde(default, rename = "Internal", alias = "internal")]
    internal: bool,
    #[serde(
        default,
        rename = "Labels",
        alias = "labels",
        deserialize_with = "null_default"
    )]
    labels: BTreeMap<String, String>,
    #[serde(
        default,
        rename = "Containers",
        alias = "containers",
        deserialize_with = "null_default"
    )]
    containers: BTreeMap<String, RawNetworkContainer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub created: String,
    pub internal: bool,
    pub labels: BTreeMap<String, String>,
    /// Connected containers: id -> name.
    pub containers: BTreeMap<String, String>,
    pub raw: Value,
}

impl Network {
    pub fn from_value(raw: Value) -> Result<Self, serde_json::Error> {
        let parsed: RawNetwork = serde_json::from_value(raw.clone())?;
        Ok(Network {
            id: parsed.id,
            name: parsed.name,
            driver: parsed.driver,
            scope: parsed.scope,
            created: parsed.created,
            internal: parsed.internal,
            labels: parsed.labels,
            containers: parsed
                .containers
                .into_iter()
                .map(|(id, container)| (id, container.name))
                .collect(),
            raw,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct PodContainer {
    #[serde(default, rename = "Id")]
    pub id: String,
    #[serde(default, rename = "Names")]
    pub name: String,
    #[serde(default, rename = "Status")]
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawPod {
    #[serde(default, rename = "Id")]
    id: String,
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "Status")]
    status: String,
    #[serde(default, rename = "Created")]
    created: String,
    #[serde(default, rename = "InfraId")]
    infra_id: String,
    #[serde(default, rename = "Labels", deserialize_with = "null_default")]
    labels: BTreeMap<String, String>,
    #[serde(default, rename = "Containers", deserialize_with = "null_default")]
    containers: Vec<PodContainer>,
}

/// A Podman pod, from `podman pod ls --format json` (there is no
/// Docker equivalent).
#[derive(Debug, Clone, PartialEq)]
pub struct Pod {
    pub id: String,
    pub name: String,
    /// `Running`, `Exited`, `Created`, `Degraded`, ... as podman prints it.
    pub status: String,
    pub created: String,
    pub infra_id: String,
    pub labels: BTreeMap<String, String>,
    pub containers: Vec<PodContainer>,
    pub raw: Value,
}

impl Pod {
    pub fn from_value(raw: Value) -> Result<Self, serde_json::Error> {
        let parsed: RawPod = serde_json::from_value(raw.clone())?;
        Ok(Pod {
            id: parsed.id,
            name: parsed.name,
            status: parsed.status,
            created: parsed.created,
            infra_id: parsed.infra_id,
            labels: parsed.labels,
            containers: parsed.containers,
            raw,
        })
    }
}

/// Parse an `inspect` array (or a single object) into typed items with
/// `from_value`. A non-array, non-object document is an error; an element
/// that fails to parse fails the whole call — `inspect` output is one
/// engine's one answer, not a stream to be partially salvaged.
pub fn parse_list<T>(
    json: &str,
    from_value: fn(Value) -> Result<T, serde_json::Error>,
) -> Result<Vec<T>, serde_json::Error> {
    let document: Value = serde_json::from_str(json)?;
    match document {
        Value::Array(items) => items.into_iter().map(from_value).collect(),
        Value::Null => Ok(Vec::new()),
        object => from_value(object).map(|item| vec![item]),
    }
}

/// The 12-hex-character prefix both CLIs print, with any `sha256:` prefix
/// dropped.
pub fn short_id(id: &str) -> String {
    let bare = id.strip_prefix("sha256:").unwrap_or(id);
    bare.chars().take(12).collect()
}

/// Parse an RFC 3339 timestamp as both engines print them
/// (`2026-09-11T06:55:35.295366787Z`, `2026-09-12T08:15:02+02:00`) into
/// seconds since the Unix epoch. `None` for anything unparsable and for
/// Go's zero time (`0001-01-01T00:00:00Z`), which Docker writes for
/// "never".
pub fn parse_timestamp(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if year < 1970 {
        return None;
    }

    let offset_index = rest.find(['Z', '+', '-'])?;
    let (time, zone) = rest.split_at(offset_index);
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.split('.').next()?.parse().ok()?;

    let offset_seconds: i64 = match zone {
        "Z" | "z" => 0,
        signed => {
            let sign = if signed.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = signed[1..].split_once(':')?;
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
    };

    let days = days_from_civil(year, month, day);
    let unix = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    u64::try_from(unix)
        .ok()
        .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds))
}

/// Howard Hinnant's `days_from_civil`: days since 1970-01-01 for a
/// proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `human_age(then, now)`: the short relative age the tree shows —
/// `"5 s"`, `"12 min"`, `"2 h"`, `"3 d"`, `"6 mo"`, `"2 y"`. Empty when
/// `then` does not parse or lies in the future.
pub fn human_age(then: &str, now: SystemTime) -> String {
    let Some(then) = parse_timestamp(then) else {
        return String::new();
    };
    let Ok(elapsed) = now.duration_since(then) else {
        return String::new();
    };
    let seconds = elapsed.as_secs();
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    match seconds {
        s if s < MINUTE => format!("{s} s"),
        s if s < HOUR => format!("{} min", s / MINUTE),
        s if s < DAY => format!("{} h", s / HOUR),
        s if s < MONTH => format!("{} d", s / DAY),
        s if s < YEAR => format!("{} mo", s / MONTH),
        s => format!("{} y", s / YEAR),
    }
}

/// `1.2 GB` / `275 MB` / `8.4 MB` / `512 kB` / `12 B` — decimal units, as
/// both CLIs print sizes.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(json: &str) -> ContainerState {
        serde_json::from_str(json).expect("state json")
    }

    #[test]
    fn status_is_derived_from_the_state_flags_first() {
        assert_eq!(
            state(r#"{"Status":"running","Running":true}"#).status(),
            ContainerStatus::Running
        );
        assert_eq!(
            state(r#"{"Status":"paused","Running":true,"Paused":true}"#).status(),
            ContainerStatus::Paused
        );
        assert_eq!(
            state(r#"{"Status":"restarting","Running":true,"Restarting":true}"#).status(),
            ContainerStatus::Restarting
        );
        assert_eq!(
            state(r#"{"Status":"exited","ExitCode":137}"#).status(),
            ContainerStatus::Exited(137)
        );
        assert_eq!(
            state(r#"{"Status":"created"}"#).status(),
            ContainerStatus::Created
        );
        assert_eq!(
            state(r#"{"Status":"dead","Dead":true}"#).status(),
            ContainerStatus::Dead
        );
        assert_eq!(
            state(r#"{"Status":"removing"}"#).status(),
            ContainerStatus::Other("removing".to_string())
        );
    }

    #[test]
    fn status_labels_and_liveness() {
        assert_eq!(ContainerStatus::Exited(3).label(), "exited (3)");
        assert_eq!(ContainerStatus::Running.label(), "running");
        assert!(ContainerStatus::Paused.is_live());
        assert!(!ContainerStatus::Exited(0).is_live());
        assert!(!ContainerStatus::Created.is_live());
        assert!(!ContainerStatus::Dead.is_live());
    }

    #[test]
    fn container_tolerates_nulls_and_missing_fields() {
        let raw: Value = serde_json::from_str(
            r#"{"Id":"abc","Name":"/x","Config":{"Labels":null,"Entrypoint":null,"Cmd":null,"Env":null},"NetworkSettings":{"Ports":{"80/tcp":null}}}"#,
        )
        .unwrap();
        let container = Container::from_value(raw).expect("lenient parse");
        assert_eq!(container.name, "x");
        assert!(container.labels.is_empty());
        assert!(container.entrypoint.is_empty());
        assert!(container.ports.is_empty());
        assert_eq!(container.status(), ContainerStatus::Other(String::new()));
    }

    #[test]
    fn container_flattens_ports_and_prefers_the_image_reference() {
        let raw: Value = serde_json::from_str(
            r#"{"Id":"abc","Name":"/api","Image":"sha256:deadbeef","Config":{"Image":"nginx:1.27"},
                "NetworkSettings":{"Ports":{"80/tcp":[{"HostIp":"0.0.0.0","HostPort":"8080"},{"HostIp":"::","HostPort":"8080"}],"443/tcp":null}}}"#,
        )
        .unwrap();
        let container = Container::from_value(raw).unwrap();
        assert_eq!(container.image, "nginx:1.27");
        assert_eq!(container.image_id, "sha256:deadbeef");
        let displayed: Vec<String> = container.ports.iter().map(PortBinding::display).collect();
        assert_eq!(
            displayed,
            vec!["0.0.0.0:8080 -> 80/tcp", ":::8080 -> 80/tcp"]
        );
    }

    #[test]
    fn compose_label_reads_either_prefix() {
        let raw: Value = serde_json::from_str(
            r#"{"Config":{"Labels":{"io.podman.compose.project":"shop","com.docker.compose.service":""}}}"#,
        )
        .unwrap();
        let container = Container::from_value(raw).unwrap();
        assert_eq!(container.compose_label("project"), Some("shop"));
        assert_eq!(container.compose_label("service"), None);
    }

    #[test]
    fn image_untagged_detection_and_display_name() {
        let tagged = Image::from_value(
            serde_json::from_str(
                r#"{"Id":"sha256:abcdef1234567890","RepoTags":["alpine:latest"]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(!tagged.is_untagged());
        assert_eq!(tagged.display_name(), "alpine:latest");

        let dangling = Image::from_value(
            serde_json::from_str(
                r#"{"Id":"sha256:abcdef1234567890","RepoTags":["<none>:<none>"],"Config":null,"Labels":null}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(dangling.is_untagged());
        assert_eq!(dangling.display_name(), "abcdef123456");
    }

    #[test]
    fn network_accepts_docker_and_podman_casing() {
        let docker = Network::from_value(
            serde_json::from_str(
                r#"{"Name":"bridge","Id":"n1","Driver":"bridge","Containers":{"c1":{"Name":"web"}}}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(docker.name, "bridge");
        assert_eq!(docker.containers.get("c1").map(String::as_str), Some("web"));

        let podman = Network::from_value(
            serde_json::from_str(
                r#"{"name":"podman","id":"n2","driver":"bridge","internal":true,"labels":{"a":"b"}}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(podman.name, "podman");
        assert!(podman.internal);
        assert_eq!(podman.labels.get("a").map(String::as_str), Some("b"));
    }

    #[test]
    fn parse_list_accepts_array_single_object_and_null() {
        let many = parse_list(r#"[{"Name":"a"},{"Name":"b"}]"#, Volume::from_value).unwrap();
        assert_eq!(many.len(), 2);
        let one = parse_list(r#"{"Name":"a"}"#, Volume::from_value).unwrap();
        assert_eq!(one[0].name, "a");
        assert!(parse_list("null", Volume::from_value).unwrap().is_empty());
        assert!(parse_list("not json", Volume::from_value).is_err());
    }

    #[test]
    fn short_id_drops_the_digest_prefix() {
        assert_eq!(short_id("sha256:83a1b4068e0f983d6084c4a6"), "83a1b4068e0f");
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn timestamps_parse_with_zulu_and_offsets_and_reject_go_zero_time() {
        let zulu = parse_timestamp("1970-01-02T00:00:00.123456789Z").unwrap();
        assert_eq!(zulu.duration_since(UNIX_EPOCH).unwrap().as_secs(), 86_400);
        let plus = parse_timestamp("1970-01-01T02:00:00+02:00").unwrap();
        assert_eq!(plus, UNIX_EPOCH);
        let minus = parse_timestamp("1970-01-01T00:00:00-01:30").unwrap();
        assert_eq!(minus.duration_since(UNIX_EPOCH).unwrap().as_secs(), 5_400);
        assert!(parse_timestamp("0001-01-01T00:00:00Z").is_none());
        assert!(parse_timestamp("").is_none());
        assert!(parse_timestamp("yesterday").is_none());
        // A known date: 2026-09-11 is 20_707 days after the epoch.
        let known = parse_timestamp("2026-09-11T06:55:35Z").unwrap();
        assert_eq!(
            known.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            20_707 * 86_400 + 6 * 3600 + 55 * 60 + 35
        );
    }

    #[test]
    fn human_age_buckets() {
        let now = UNIX_EPOCH + Duration::from_secs(20_707 * 86_400);
        let then = |text: &str| human_age(text, now);
        assert_eq!(then("2026-09-10T23:59:30Z"), "30 s");
        assert_eq!(then("2026-09-10T23:48:00Z"), "12 min");
        assert_eq!(then("2026-09-10T21:00:00Z"), "3 h");
        assert_eq!(then("2026-09-08T00:00:00Z"), "3 d");
        assert_eq!(then("2026-07-01T00:00:00Z"), "2 mo");
        assert_eq!(then("2024-01-01T00:00:00Z"), "2 y");
        assert_eq!(then("2026-09-12T00:00:00Z"), "", "future is blank");
        assert_eq!(then("0001-01-01T00:00:00Z"), "");
    }

    #[test]
    fn human_size_uses_decimal_units() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(512_000), "512 kB");
        assert_eq!(human_size(8_420_000), "8.4 MB");
        assert_eq!(human_size(274_852_063), "275 MB");
        assert_eq!(human_size(2_260_000_000), "2.3 GB");
    }
}
