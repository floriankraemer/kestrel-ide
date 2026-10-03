//! Xdebug through vscode-php-debug (ADR-0069): the environment a PHP
//! process needs to connect back, and where the adapter listens for it.
//!
//! Every PHP debug case is the same: a listen session on `port`, the
//! program started with [`env`], and each Xdebug connection arriving as a
//! DAP thread. What differs is only where PHP runs, which decides the
//! address it must dial and the one the adapter must listen on.

use std::path::{Path, PathBuf};

use process_exec::host::{ExecHost, PathMap};
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

/// The plan for PHP whose container mount is `container_map` (`None` for
/// PHP running beside the project). `adapter_host` is where the adapter
/// runs: the mapping's local side must be a path it can read (the distro
/// path for a WSL root).
pub fn plan_for(adapter_host: &ExecHost, container_map: Option<&PathMap>, port: u16) -> Plan {
    let path_map = container_map.map(|map| PathMap {
        local_root: PathBuf::from(crate::source_path(adapter_host, &map.local_root)),
        remote_root: map.remote_root.clone(),
    });
    let kind = if path_map.is_some() {
        HostKind::Container
    } else {
        HostKind::Local
    };
    plan(kind, path_map.as_ref(), port)
}

/// A request that waited for the old adapter to shut down.
#[derive(Debug, PartialEq)]
pub struct PendingListen<R> {
    pub root: PathBuf,
    pub arguments: Value,
    pub run: Option<R>,
}

/// What the owner of a [`ListenSession`] must do for a request.
#[derive(Debug, PartialEq)]
pub enum ListenAction<R> {
    /// Nothing more (queued, or reused with no run to launch).
    Nothing,
    /// Start the listener now, then launch `run` once it is ready.
    Start(PendingListen<R>),
    /// Launch `run` on the running, handshake-complete listener `session_id`.
    Launch { session_id: u64, run: R },
    /// Stop the running adapter off-thread, then `take_pending` and start.
    Replace,
}

/// The one PHP listen session's lifecycle, independent of how sessions are
/// started: what runs, what waits behind a shutdown, and the run to launch
/// once the listener is up. `R` is the owner's run payload.
#[derive(Debug)]
pub struct ListenSession<R> {
    running: Option<(u64, Value)>,
    /// The running adapter finished its DAP handshake and accepts connections.
    ready: bool,
    /// Runs requested while the running adapter was still handshaking.
    awaiting_ready: Vec<R>,
    pending: Option<PendingListen<R>>,
}

impl<R> Default for ListenSession<R> {
    fn default() -> Self {
        Self {
            running: None,
            ready: false,
            awaiting_ready: Vec::new(),
            pending: None,
        }
    }
}

impl<R> ListenSession<R> {
    pub fn is_listening(&self) -> bool {
        self.running.is_some()
    }

    pub fn session_id(&self) -> Option<u64> {
        self.running.as_ref().map(|(id, _)| *id)
    }

    /// Ask to listen with `arguments`, optionally followed by `run`.
    pub fn request(&mut self, root: &Path, arguments: Value, run: Option<R>) -> ListenAction<R> {
        let decision = decide_listen(
            self.running.as_ref().map(|(_, args)| args),
            self.pending.is_some(),
            &arguments,
        );
        let request = PendingListen {
            root: root.to_path_buf(),
            arguments,
            run,
        };
        match decision {
            ListenDecision::Reuse => match (request.run, self.session_id()) {
                (Some(run), Some(session_id)) if self.ready => {
                    ListenAction::Launch { session_id, run }
                }
                // Xdebug does not retry: wait for `handshake_done`.
                (Some(run), Some(_)) => {
                    self.awaiting_ready.push(run);
                    ListenAction::Nothing
                }
                _ => ListenAction::Nothing,
            },
            ListenDecision::Start => ListenAction::Start(request),
            ListenDecision::Queue => {
                self.pending = Some(request);
                ListenAction::Nothing
            }
            ListenDecision::Replace => {
                self.pending = Some(request);
                ListenAction::Replace
            }
        }
    }

    /// The request waiting behind the old adapter, if the user has not
    /// stopped listening meanwhile.
    pub fn take_pending(&mut self) -> Option<PendingListen<R>> {
        self.pending.take()
    }

    /// The adapter for `session_id` is up and listens with `arguments`.
    pub fn started(&mut self, session_id: u64, arguments: Value) {
        self.running = Some((session_id, arguments));
        self.ready = false;
        self.awaiting_ready.clear();
    }

    /// The adapter for `session_id` finished its handshake. Returns the
    /// runs that were requested meanwhile, to launch now.
    pub fn handshake_done(&mut self, session_id: u64) -> Vec<R> {
        if self.session_id() != Some(session_id) {
            return Vec::new();
        }
        self.ready = true;
        std::mem::take(&mut self.awaiting_ready)
    }

    /// The user turned listening off: drop what was queued. Returns the
    /// session to shut down, if any.
    pub fn stop(&mut self) -> Option<u64> {
        self.pending = None;
        self.awaiting_ready.clear();
        self.session_id()
    }

    /// A session ended; true when it was the listener.
    pub fn ended(&mut self, session_id: u64) -> bool {
        let was_listener = self.session_id() == Some(session_id);
        if was_listener {
            self.running = None;
            self.ready = false;
            self.awaiting_ready.clear();
        }
        was_listener
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

    #[test]
    fn plan_for_a_container_map_listens_on_every_interface() {
        let map = PathMap::new("/home/me/app", "/var/www");
        let local = ExecHost::Local;
        let plan = plan_for(&local, Some(&map), DEFAULT_PORT);
        assert_eq!(plan.listen_arguments["hostname"], "0.0.0.0");
        assert_eq!(
            plan.listen_arguments["pathMappings"]["/var/www"],
            "/home/me/app"
        );
        let plan = plan_for(&local, None, DEFAULT_PORT);
        assert_eq!(plan.listen_arguments["hostname"], LOOPBACK);
    }

    fn up(session: &mut ListenSession<&'static str>, id: u64, args: &Value) {
        session.started(id, args.clone());
        session.handshake_done(id);
    }

    #[test]
    fn a_run_during_the_handshake_waits_until_it_completes() {
        let args = serde_json::json!({"port": 1});
        let root = Path::new("/p");
        let mut s = ListenSession::default();
        s.started(7, args.clone());
        assert_eq!(
            s.request(root, args.clone(), Some("early")),
            ListenAction::Nothing
        );
        assert!(
            s.handshake_done(8).is_empty(),
            "another session's handshake"
        );
        assert_eq!(s.handshake_done(7), vec!["early"]);
        assert!(s.handshake_done(7).is_empty(), "launched once");
        assert_eq!(
            s.request(root, args, Some("late")),
            ListenAction::Launch {
                session_id: 7,
                run: "late"
            }
        );
    }

    #[test]
    fn a_failed_handshake_drops_the_waiting_runs() {
        let args = serde_json::json!({"port": 1});
        let mut s = ListenSession::default();
        s.started(7, args.clone());
        s.request(Path::new("/p"), args, Some("early"));
        assert!(s.ended(7));
        s.started(8, serde_json::json!({"port": 1}));
        assert!(s.handshake_done(8).is_empty());
    }

    #[test]
    fn a_run_reuses_the_running_listener() {
        let args = serde_json::json!({"port": 1});
        let mut s = ListenSession::default();
        assert!(!s.is_listening());
        up(&mut s, 7, &args);
        assert!(s.is_listening());
        let root = Path::new("/p");
        assert_eq!(
            s.request(root, args.clone(), Some("run")),
            ListenAction::Launch {
                session_id: 7,
                run: "run"
            }
        );
        // A bare toggle on an already listening session does nothing.
        assert_eq!(s.request(root, args, None), ListenAction::Nothing);
    }

    #[test]
    fn a_run_waits_for_the_listener_it_needs_and_launches_once() {
        let (a, b) = (
            serde_json::json!({"port": 1}),
            serde_json::json!({"port": 2}),
        );
        let root = Path::new("/p");
        let mut s = ListenSession::default();
        let ListenAction::Start(first) = s.request(root, a.clone(), Some("one")) else {
            panic!("nothing runs: start");
        };
        assert_eq!(first.run, Some("one"));
        s.started(1, a);
        // Different arguments replace the listener; the run is queued.
        assert_eq!(s.request(root, b, Some("two")), ListenAction::Replace);
        assert!(s.ended(1));
        let pending = s.take_pending().expect("queued run");
        assert_eq!(pending.run, Some("two"));
        assert!(s.take_pending().is_none(), "the queued run starts once");
    }

    #[test]
    fn requests_during_shutdown_replace_the_queue() {
        let (a, b) = (
            serde_json::json!({"port": 1}),
            serde_json::json!({"port": 2}),
        );
        let root = Path::new("/p");
        let mut s = ListenSession::default();
        s.started(1, a.clone());
        assert_eq!(s.request(root, b, Some("two")), ListenAction::Replace);
        s.ended(1);
        assert_eq!(s.request(root, a, Some("three")), ListenAction::Nothing);
        assert_eq!(s.take_pending().unwrap().run, Some("three"));
    }

    #[test]
    fn turning_listening_off_drops_the_queue_and_session_end_clears_state() {
        let a = serde_json::json!({"port": 1});
        let root = Path::new("/p");
        let mut s = ListenSession::default();
        s.started(3, a.clone());
        s.request(root, serde_json::json!({"port": 2}), Some("r"));
        assert_eq!(s.stop(), Some(3));
        assert!(s.take_pending().is_none());
        assert!(!s.ended(99), "another session's end is not the listener's");
        assert!(s.is_listening());
        assert!(s.ended(3));
        assert!(!s.is_listening());
        assert_eq!(s.stop(), None);
    }
}
