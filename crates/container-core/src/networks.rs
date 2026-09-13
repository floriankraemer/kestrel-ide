//! Networks (C4, ADR-0055): create/remove/prune argv builders. Connected
//! containers and labels need no new parsing — [`crate::model::Network`]
//! already carries both out of `network inspect`.

/// Everything "Create Network..." collects, argv-ready.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkSpec {
    pub name: String,
    /// Empty means "let the engine pick" (its own default, `bridge`).
    pub driver: String,
    pub subnet: String,
    pub gateway: String,
    pub internal: bool,
    pub attachable: bool,
    pub labels: Vec<(String, String)>,
}

/// `network create [--driver d] [--subnet s] [--gateway g] [--internal]
/// [--attachable] [--label k=v]... <name>`.
pub fn create_args(spec: &NetworkSpec) -> Vec<String> {
    let mut args = vec!["network".to_string(), "create".to_string()];
    if !spec.driver.is_empty() {
        args.push("--driver".to_string());
        args.push(spec.driver.clone());
    }
    if !spec.subnet.is_empty() {
        args.push("--subnet".to_string());
        args.push(spec.subnet.clone());
    }
    if !spec.gateway.is_empty() {
        args.push("--gateway".to_string());
        args.push(spec.gateway.clone());
    }
    if spec.internal {
        args.push("--internal".to_string());
    }
    if spec.attachable {
        args.push("--attachable".to_string());
    }
    for (key, value) in &spec.labels {
        args.push("--label".to_string());
        args.push(format!("{key}={value}"));
    }
    args.push(spec.name.clone());
    args
}

/// `network rm <id>`.
pub fn remove_args(id: &str) -> Vec<String> {
    vec!["network".to_string(), "rm".to_string(), id.to_string()]
}

/// `network prune -f`.
pub fn prune_args() -> Vec<String> {
    vec!["network".to_string(), "prune".to_string(), "-f".to_string()]
}

/// A network's configured subnets, out of its `inspect` JSON — the
/// Dashboard's "Subnets" line. Docker nests them at `IPAM.Config[].Subnet`;
/// Podman's `network inspect` prints a top-level `subnets[].subnet` instead
/// (the same lowercase-keys difference [`crate::model::Network`] already
/// aliases for its own fields).
pub fn subnets(raw: &serde_json::Value) -> Vec<String> {
    let docker = raw
        .get("IPAM")
        .and_then(|ipam| ipam.get("Config"))
        .and_then(|config| config.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("Subnet").and_then(|v| v.as_str()));
    let podman = raw
        .get("subnets")
        .and_then(|subnets| subnets.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("subnet").and_then(|v| v.as_str()));
    docker.chain(podman).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_argv_with_every_flag() {
        let spec = NetworkSpec {
            name: "shop".to_string(),
            driver: "bridge".to_string(),
            subnet: "172.20.0.0/16".to_string(),
            gateway: "172.20.0.1".to_string(),
            internal: true,
            attachable: true,
            labels: vec![("team".to_string(), "checkout".to_string())],
        };
        assert_eq!(
            create_args(&spec),
            vec![
                "network",
                "create",
                "--driver",
                "bridge",
                "--subnet",
                "172.20.0.0/16",
                "--gateway",
                "172.20.0.1",
                "--internal",
                "--attachable",
                "--label",
                "team=checkout",
                "shop",
            ]
        );
    }

    #[test]
    fn create_argv_with_nothing_but_a_name() {
        let spec = NetworkSpec {
            name: "plain".to_string(),
            ..NetworkSpec::default()
        };
        assert_eq!(create_args(&spec), vec!["network", "create", "plain"]);
    }

    #[test]
    fn remove_and_prune_argv() {
        assert_eq!(remove_args("n1"), vec!["network", "rm", "n1"]);
        assert_eq!(prune_args(), vec!["network", "prune", "-f"]);
    }

    #[test]
    fn subnets_reads_dockers_nested_ipam_config() {
        let raw = serde_json::json!({
            "IPAM": {"Config": [{"Subnet": "172.17.0.0/16", "Gateway": "172.17.0.1"}]}
        });
        assert_eq!(subnets(&raw), vec!["172.17.0.0/16".to_string()]);
    }

    #[test]
    fn subnets_reads_podmans_top_level_subnets() {
        let raw = serde_json::json!({
            "subnets": [{"subnet": "10.88.0.0/16", "gateway": "10.88.0.1"}]
        });
        assert_eq!(subnets(&raw), vec!["10.88.0.0/16".to_string()]);
    }

    #[test]
    fn subnets_is_empty_when_neither_shape_is_present() {
        assert!(subnets(&serde_json::json!({})).is_empty());
    }
}
