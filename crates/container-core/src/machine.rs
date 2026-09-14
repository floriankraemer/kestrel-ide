//! Podman machines (C9): `podman machine list --format json` and
//! start/stop argv for the connection node's "Start machine"/"Stop machine"
//! actions (offered only for a [`crate::connection::ConnectionKind::PodmanMachine`]
//! connection). Docker has no equivalent — nothing here is ever called on a
//! Docker connection.
//!
//! Fixture note: `podman` is not installed on this repo's build host, so
//! `testdata/machine/list.json` is hand-authored from the documented
//! `podman-machine-list(1)` JSON schema rather than captured from a real
//! `podman machine list --format json` run — flagged the same way the other
//! hand-authored Podman fixtures under `testdata/inspect/podman/` are (see
//! `testdata/inspect/README.md`).

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
struct RawMachine {
    #[serde(default, rename = "Name")]
    name: String,
    #[serde(default, rename = "Default")]
    default: bool,
    #[serde(default, rename = "Running")]
    running: bool,
    #[serde(default, rename = "CPUs")]
    cpus: u64,
    /// Bytes, per `podman-machine-list(1)`.
    #[serde(default, rename = "Memory")]
    memory: u64,
    /// Bytes, per `podman-machine-list(1)`.
    #[serde(default, rename = "DiskSize")]
    disk: u64,
}

/// One `podman machine` — the connection node's "machine state" line and
/// the Start/Stop machine actions' target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub name: String,
    pub running: bool,
    pub default: bool,
    pub cpus: u64,
    /// Bytes.
    pub memory: u64,
    /// Bytes.
    pub disk: u64,
}

impl Machine {
    pub fn from_value(raw: serde_json::Value) -> Result<Self, serde_json::Error> {
        let parsed: RawMachine = serde_json::from_value(raw)?;
        Ok(Machine {
            name: parsed.name,
            running: parsed.running,
            default: parsed.default,
            cpus: parsed.cpus,
            memory: parsed.memory,
            disk: parsed.disk,
        })
    }
}

/// `machine list --format json` -> every machine, parsed with
/// [`crate::model::parse_list`]'s same "one bad element fails the whole
/// call" rule.
pub fn parse_list(json: &str) -> Result<Vec<Machine>, serde_json::Error> {
    crate::model::parse_list(json, Machine::from_value)
}

/// `machine list --format json`.
pub fn list_args() -> Vec<String> {
    vec![
        "machine".to_string(),
        "list".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]
}

/// `machine start [<name>]` — no name targets the default machine, which
/// the connection node's "Start machine" action never needs (a
/// [`crate::connection::ConnectionKind::PodmanMachine`] always names one),
/// but the empty-name shape stays valid rather than special-cased away.
pub fn start_args(name: &str) -> Vec<String> {
    let mut args = vec!["machine".to_string(), "start".to_string()];
    if !name.is_empty() {
        args.push(name.to_string());
    }
    args
}

/// `machine stop [<name>]`.
pub fn stop_args(name: &str) -> Vec<String> {
    let mut args = vec!["machine".to_string(), "stop".to_string()];
    if !name.is_empty() {
        args.push(name.to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../testdata/machine/list.json");

    #[test]
    fn parses_the_hand_authored_machine_list_fixture() {
        let machines = parse_list(FIXTURE).unwrap();
        assert_eq!(machines.len(), 2);
        let default = machines.iter().find(|m| m.default).unwrap();
        assert_eq!(default.name, "podman-machine-default");
        assert!(default.running);
        assert_eq!(default.cpus, 4);
        assert_eq!(default.memory, 2_147_483_648);
        let other = machines.iter().find(|m| !m.default).unwrap();
        assert!(!other.running);
    }

    #[test]
    fn list_argv() {
        assert_eq!(list_args(), vec!["machine", "list", "--format", "json"]);
    }

    #[test]
    fn start_stop_argv() {
        assert_eq!(start_args("dev"), vec!["machine", "start", "dev"]);
        assert_eq!(stop_args("dev"), vec!["machine", "stop", "dev"]);
        assert_eq!(start_args(""), vec!["machine", "start"]);
    }

    #[test]
    fn missing_fields_degrade_to_defaults_rather_than_failing() {
        let machines = parse_list(r#"[{"Name":"m1"}]"#).unwrap();
        assert_eq!(
            machines[0],
            Machine {
                name: "m1".to_string(),
                running: false,
                default: false,
                cpus: 0,
                memory: 0,
                disk: 0,
            }
        );
    }
}
