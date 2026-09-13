//! Volumes (C4, ADR-0055): create/remove/prune argv builders and
//! containers-using-a-volume, derived from each container's own mounts
//! (already parsed by [`crate::model::Container`]) rather than a second
//! `inspect` round trip.

use crate::model::Container;

/// Everything "Create Volume..." collects, argv-ready.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VolumeSpec {
    pub name: String,
    /// Empty means "let the engine pick" (its own default, `local`).
    pub driver: String,
    pub labels: Vec<(String, String)>,
    pub options: Vec<(String, String)>,
}

/// `volume create [--driver d] [--label k=v]... [--opt k=v]... <name>`.
pub fn create_args(spec: &VolumeSpec) -> Vec<String> {
    let mut args = vec!["volume".to_string(), "create".to_string()];
    if !spec.driver.is_empty() {
        args.push("--driver".to_string());
        args.push(spec.driver.clone());
    }
    for (key, value) in &spec.labels {
        args.push("--label".to_string());
        args.push(format!("{key}={value}"));
    }
    for (key, value) in &spec.options {
        args.push("--opt".to_string());
        args.push(format!("{key}={value}"));
    }
    args.push(spec.name.clone());
    args
}

/// `volume rm [-f] <name>`.
pub fn remove_args(name: &str, force: bool) -> Vec<String> {
    let mut args = vec!["volume".to_string(), "rm".to_string()];
    if force {
        args.push("-f".to_string());
    }
    args.push(name.to_string());
    args
}

/// `volume prune -f`.
pub fn prune_args() -> Vec<String> {
    vec!["volume".to_string(), "prune".to_string(), "-f".to_string()]
}

/// Containers that mount `volume_name` — the Dashboard's "containers using
/// it" list, read straight off each container's own `Mounts` rather than a
/// second engine call.
pub fn containers_using<'a>(volume_name: &str, containers: &'a [Container]) -> Vec<&'a Container> {
    containers
        .iter()
        .filter(|container| {
            container
                .mounts
                .iter()
                .any(|mount| mount.kind == "volume" && mount.name == volume_name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn create_argv_with_every_flag() {
        let spec = VolumeSpec {
            name: "pgdata".to_string(),
            driver: "local".to_string(),
            labels: vec![("app".to_string(), "shop".to_string())],
            options: vec![("type".to_string(), "tmpfs".to_string())],
        };
        assert_eq!(
            create_args(&spec),
            vec![
                "volume",
                "create",
                "--driver",
                "local",
                "--label",
                "app=shop",
                "--opt",
                "type=tmpfs",
                "pgdata",
            ]
        );
    }

    #[test]
    fn create_argv_with_nothing_but_a_name() {
        let spec = VolumeSpec {
            name: "plain".to_string(),
            ..VolumeSpec::default()
        };
        assert_eq!(create_args(&spec), vec!["volume", "create", "plain"]);
    }

    #[test]
    fn remove_argv_adds_force_flag_only_when_asked() {
        assert_eq!(remove_args("pgdata", false), vec!["volume", "rm", "pgdata"]);
        assert_eq!(
            remove_args("pgdata", true),
            vec!["volume", "rm", "-f", "pgdata"]
        );
    }

    #[test]
    fn prune_argv() {
        assert_eq!(prune_args(), vec!["volume", "prune", "-f"]);
    }

    #[test]
    fn containers_using_matches_by_mount_name_not_path() {
        let with_volume = Container::from_value(json!({
            "Id": "c1",
            "Mounts": [{"Type": "volume", "Name": "pgdata", "Destination": "/var/lib/postgresql/data"}]
        }))
        .unwrap();
        let bind_mount_same_dest = Container::from_value(json!({
            "Id": "c2",
            "Mounts": [{"Type": "bind", "Name": "", "Source": "/host/pgdata", "Destination": "/var/lib/postgresql/data"}]
        }))
        .unwrap();
        let containers = vec![with_volume, bind_mount_same_dest];
        let using = containers_using("pgdata", &containers);
        assert_eq!(using.len(), 1);
        assert_eq!(using[0].id, "c1");
    }
}
