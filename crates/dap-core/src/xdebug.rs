//! Xdebug through vscode-php-debug (ADR-0069): the environment a PHP
//! process needs to connect back, and where the adapter listens for it.
//!
//! Every PHP debug case is the same: a listen session on `port`, the
//! program started with [`env`], and each Xdebug connection arriving as a
//! DAP thread. What differs is only where PHP runs, which decides the
//! address it must dial and the one the adapter must listen on.

use process_exec::host::PathMap;
use serde_json::Value;

use crate::launch::php_listen_arguments;

/// Xdebug 3's default client port.
pub const DEFAULT_PORT: u16 = 9003;
/// The adapter's listen address when only this machine connects.
pub const LOOPBACK: &str = "127.0.0.1";
/// What a container calls its host. Docker Desktop and Podman resolve it;
/// on Linux Docker the container needs `host.docker.internal:host-gateway`
/// in `extra_hosts` (the debug start error says so).
pub const CONTAINER_HOST: &str = "host.docker.internal";
/// The session key every launch from this IDE uses.
pub const IDEKEY: &str = "kestrel-ide";

/// How the run is armed: Xdebug's `XDEBUG_SESSION` (start with the
/// request, the CLI way) or `XDEBUG_TRIGGER` (only when the trigger
/// matches, the web way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Session,
    Trigger,
}

/// Where PHP runs relative to the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// This machine; also a WSL distro, where the adapter runs beside PHP.
    Local,
    Container,
}

/// The environment that makes an Xdebug-enabled PHP connect to
/// `client_host:port`.
pub fn env(trigger: Trigger, client_host: &str, port: u16, idekey: &str) -> Vec<(String, String)> {
    let key = match trigger {
        Trigger::Session => "XDEBUG_SESSION",
        Trigger::Trigger => "XDEBUG_TRIGGER",
    };
    vec![
        ("XDEBUG_MODE".to_string(), "debug".to_string()),
        (key.to_string(), idekey.to_string()),
        (
            "XDEBUG_CONFIG".to_string(),
            format!("client_host={client_host} client_port={port}"),
        ),
    ]
}

/// What a PHP debug launch needs from the adapter's side and the program's.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The adapter's `launch` body (listen mode).
    pub listen_arguments: Value,
    /// Added to the program's environment.
    pub env: Vec<(String, String)>,
}

/// The plan for PHP running on `host`. `path_map` is the container's mount
/// (adapter-visible local root <-> server root); it becomes the
/// `pathMappings` object, server path to local path.
pub fn plan(host: HostKind, path_map: Option<&PathMap>, port: u16) -> Plan {
    let (listen_on, client_host) = match host {
        // Listening on every interface is what lets a container reach the
        // adapter through the host gateway.
        HostKind::Container => ("0.0.0.0", CONTAINER_HOST),
        HostKind::Local => (LOOPBACK, LOOPBACK),
    };
    let mappings: Vec<(String, String)> = path_map
        .into_iter()
        .map(|map| {
            (
                map.remote_root.clone(),
                map.local_root.display().to_string(),
            )
        })
        .collect();
    Plan {
        listen_arguments: php_listen_arguments(port, listen_on, &mappings),
        env: env(Trigger::Session, client_host, port, IDEKEY),
    }
}

/// What a request to listen with some adapter arguments does, given what is
/// running — the rule the bridge follows so replacing a listener never
/// waits for the old adapter on the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenDecision {
    /// The running listener already listens that way.
    Reuse,
    /// Nothing is running or stopping: start now.
    Start,
    /// A listener with other arguments runs: stop it off-thread, then start.
    Replace,
    /// An old adapter is still shutting down (it holds the port): the
    /// request replaces whatever was queued behind it.
    Queue,
}

/// Decide how to satisfy a request for `wanted` arguments.
pub fn decide_listen(
    running: Option<&Value>,
    shutdown_in_flight: bool,
    wanted: &Value,
) -> ListenDecision {
    if shutdown_in_flight {
        return ListenDecision::Queue;
    }
    match running {
        Some(current) if current == wanted => ListenDecision::Reuse,
        Some(_) => ListenDecision::Replace,
        None => ListenDecision::Start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn env_arms_debug_mode_and_names_where_to_connect() {
        let env = env(Trigger::Session, "127.0.0.1", 9003, "k");
        assert_eq!(value(&env, "XDEBUG_MODE"), Some("debug"));
        assert_eq!(value(&env, "XDEBUG_SESSION"), Some("k"));
        assert_eq!(value(&env, "XDEBUG_TRIGGER"), None);
        assert_eq!(
            value(&env, "XDEBUG_CONFIG"),
            Some("client_host=127.0.0.1 client_port=9003")
        );
    }

    #[test]
    fn the_trigger_mode_uses_xdebug_trigger() {
        let env = env(Trigger::Trigger, "h", 1, "k");
        assert_eq!(value(&env, "XDEBUG_TRIGGER"), Some("k"));
        assert_eq!(value(&env, "XDEBUG_SESSION"), None);
    }

    #[test]
    fn a_local_run_listens_on_loopback_without_mappings() {
        let plan = plan(HostKind::Local, None, 9100);
        assert_eq!(plan.listen_arguments["hostname"], LOOPBACK);
        assert_eq!(plan.listen_arguments["port"], 9100);
        assert_eq!(plan.listen_arguments["pathMappings"], serde_json::json!({}));
        assert_eq!(
            value(&plan.env, "XDEBUG_CONFIG"),
            Some("client_host=127.0.0.1 client_port=9100")
        );
    }

    #[test]
    fn a_container_run_dials_the_host_and_maps_its_mount() {
        let map = PathMap::new("/home/me/app", "/var/www");
        let plan = plan(HostKind::Container, Some(&map), DEFAULT_PORT);
        assert_eq!(plan.listen_arguments["hostname"], "0.0.0.0");
        assert_eq!(
            plan.listen_arguments["pathMappings"]["/var/www"],
            "/home/me/app"
        );
        assert_eq!(
            value(&plan.env, "XDEBUG_CONFIG"),
            Some("client_host=host.docker.internal client_port=9003")
        );
    }

    #[test]
    fn replacing_a_listener_never_starts_before_the_old_adapter_is_gone() {
        use serde_json::json;
        let (a, b) = (json!({"port": 1}), json!({"port": 2}));
        assert_eq!(decide_listen(None, false, &a), ListenDecision::Start);
        assert_eq!(decide_listen(Some(&a), false, &a), ListenDecision::Reuse);
        assert_eq!(decide_listen(Some(&a), false, &b), ListenDecision::Replace);
        // The old adapter still holds the port: queue, whatever was asked.
        assert_eq!(decide_listen(None, true, &b), ListenDecision::Queue);
        assert_eq!(decide_listen(None, true, &a), ListenDecision::Queue);
    }
}
