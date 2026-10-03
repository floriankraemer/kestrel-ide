//! A listen session against the scripted stand-in for vscode-php-debug
//! (PHP parity D0): launch without `program`, connections arriving as
//! threads, a stop, the Variables path, and the second connection.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use dap_core::catalog::Adapter;
use dap_core::{DapSession, SessionListener};
use serde_json::{json, Value};

struct Events(Sender<(String, Value)>);

impl SessionListener for Events {
    fn event(&mut self, event: &str, body: &Value) {
        let _ = self.0.send((event.to_string(), body.clone()));
    }
    fn reverse_request(&mut self, _command: &str, _arguments: &Value) -> Option<Value> {
        None
    }
    fn disconnected(&mut self) {}
}

fn next(events: &Receiver<(String, Value)>, name: &str, thread: i64) -> Value {
    loop {
        let (event, body) = events
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("no {name} event for thread {thread}"));
        if event == name && body["threadId"] == thread {
            return body;
        }
    }
}

#[test]
fn a_listen_session_sees_each_connection_as_a_thread_and_stops_on_it() {
    let adapter = Adapter {
        id: "php-debug".into(),
        program: env!("CARGO_BIN_EXE_stub_adapter").into(),
        args: Vec::new(),
        install_hint: String::new(),
    };
    let (sender, events) = channel();
    let session = DapSession::start(&adapter, None, Box::new(Events(sender))).expect("starts");
    session.initialize().expect("initialize");
    // Listen mode: no `program`.
    session
        .launch(json!({"port": 9003, "pathMappings": {"/app": "/home/me/app"}}))
        .expect("launch");
    session
        .wait_for_initialized(Duration::from_secs(10))
        .expect("initialized");
    session
        .request(
            "setBreakpoints",
            json!({"source": {"path": "/app/index.php"}, "breakpoints": [{"line": 7}]}),
        )
        .expect("setBreakpoints");
    session.configuration_done().expect("configurationDone");

    let (_, output) = loop {
        let event = events
            .recv_timeout(Duration::from_secs(10))
            .expect("output");
        if event.0 == "output" {
            break event;
        }
    };
    assert!(!output["output"].as_str().unwrap().contains("program"));

    next(&events, "thread", 1);
    next(&events, "stopped", 1);
    let frames = session
        .request("stackTrace", json!({"threadId": 1}))
        .map(|body| dap_core::protocol::stack_frames(&body))
        .expect("stackTrace");
    assert_eq!(
        (frames[0].path.as_str(), frames[0].line),
        ("/app/index.php", 7)
    );
    let scopes = session
        .request("scopes", json!({"frameId": frames[0].id}))
        .map(|body| dap_core::protocol::scopes(&body))
        .expect("scopes");
    let variables = session
        .request(
            "variables",
            json!({"variablesReference": scopes[0].variables_reference}),
        )
        .map(|body| dap_core::protocol::variables(&body))
        .expect("variables");
    assert_eq!(variables[0].name, "$answer");

    // The first connection ends; the second arrives as another thread.
    session
        .request("continue", json!({"threadId": 1}))
        .expect("continue");
    next(&events, "stopped", 2);
    session.shutdown();
}

/// A run (or `[php]` interpreter) in a container makes the adapter listen on
/// every interface — Xdebug dials in from the container — and map the
/// container's mount back to the project. A loopback-only listener or an
/// empty mapping would silently never hit a breakpoint.
#[test]
fn a_container_target_listens_on_every_interface_with_path_mappings() {
    use app_config::{ContainerSettings, ContainerTargetSetting};
    use process_exec::host::ExecHost;

    let containers = ContainerSettings {
        targets: vec![ContainerTargetSetting {
            id: "t1".into(),
            name: "php".into(),
            source: "image".into(),
            image: Some("php:8.3-cli".into()),
            workdir: "/var/www".into(),
            ..ContainerTargetSetting::default()
        }],
        ..ContainerSettings::default()
    };
    let root = std::path::Path::new("/home/me/app");
    let ExecHost::Container(container) =
        run_core::container_target::run_host(Some("container:t1"), &containers, root)
    else {
        panic!("a known target runs in its container");
    };
    let plan = dap_core::xdebug::plan_for(&ExecHost::Local, Some(&container.path_map), 9003);
    assert_eq!(plan.listen_arguments["hostname"], "0.0.0.0");
    assert_eq!(
        plan.listen_arguments["pathMappings"],
        json!({"/var/www": "/home/me/app"})
    );
}
