//! Turning a `run_core::LaunchSpec` into a DAP `launch` body (D3).
//!
//! This is the seam ADR-0032 designed `LaunchSpec` for: "the shared,
//! debugger-agnostic half of what would eventually become a DAP `launch`
//! request body". One configuration, launched by Run or by Debug, is the
//! same program with the same arguments in the same directory — the only
//! difference is who starts it.
//!
//! Adapters do not agree on the schema, so the mapping is per adapter. It
//! lives here rather than in the bridge because "what codelldb calls the
//! program" is a fact about codelldb, not about Qt.

use run_core::LaunchSpec;
use serde_json::{json, Map, Value};

/// The `launch` arguments for `adapter_id`, from what the run configuration
/// says.
///
/// An adapter this function does not know gets the common shape — `program`,
/// `args`, `cwd`, `env` — which is what most adapters accept, rather than
/// nothing at all.
pub fn arguments(adapter_id: &str, spec: &LaunchSpec) -> Value {
    let mut arguments = common(spec);
    match adapter_id {
        // codelldb wants the debuggee's environment as a map and calls the
        // session type `lldb`.
        "codelldb" => {
            arguments.insert("type".into(), json!("lldb"));
            arguments.insert("request".into(), json!("launch"));
            // Same reason as debugpy's `console`: the adapter starts the
            // debuggee, and its output arrives as `output` events.
            arguments.insert("terminal".into(), json!("console"));
        }
        // debugpy runs a *module or file*, and calls the working directory
        // `cwd` like everyone else. `console` decides where the debuggee's
        // own output goes; the integrated terminal is what a run console is.
        "debugpy" => {
            arguments.insert("type".into(), json!("python"));
            arguments.insert("request".into(), json!("launch"));
            // `internalConsole`: the adapter starts the debuggee itself and
            // reports its output as DAP `output` events, which is what the
            // debugger console shows. `integratedTerminal` would ask the
            // client to start it — see `session::initialize`.
            arguments.insert("console".into(), json!("internalConsole"));
            // `program` for debugpy is the script, which for a Python run
            // configuration is the first argument rather than the
            // interpreter this crate was handed.
            if let Some(script) = spec.args.first() {
                arguments.insert("program".into(), json!(script));
                arguments.insert("args".into(), json!(&spec.args[1..]));
            }
        }
        // java-debug launches a main class with a classpath, neither of
        // which a run configuration carries. Passing the common shape at
        // least gives the adapter the working directory and environment;
        // the rest has to come from the user's own launch settings.
        "java-debug" => {
            arguments.insert("type".into(), json!("java"));
            arguments.insert("request".into(), json!("launch"));
        }
        _ => {}
    }
    Value::Object(arguments)
}

/// The `attach` arguments for joining a process that is already running.
pub fn attach_arguments(adapter_id: &str, pid: u32) -> Value {
    match adapter_id {
        "codelldb" => json!({"type": "lldb", "request": "attach", "pid": pid}),
        "debugpy" => json!({"type": "python", "request": "attach", "processId": pid}),
        _ => json!({"request": "attach", "processId": pid}),
    }
}

/// A debuggee that is already running somewhere else (D4-2): a debug
/// server listening on a socket, and how its paths line up with this
/// checkout's.
///
/// `mappings` is `(local, remote)` root pairs. Empty means the two trees
/// are laid out identically, which is the common case when the remote is a
/// container mounting the same source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteTarget {
    pub host: String,
    pub port: u16,
    pub mappings: Vec<(String, String)>,
}

/// The `attach` arguments for a debuggee reachable over the network.
///
/// Every adapter spells this differently, and the differences are not
/// cosmetic — debugpy connects to a socket and maps paths, codelldb drives
/// LLDB commands and maps sources, java-debug names a host and a port.
/// There is no common schema to invent here (which is exactly why this task
/// waited for a second case), so this is a table of what each adapter
/// documents, and an unknown adapter gets the plainest reading of the
/// specification rather than one of the three dialects.
pub fn remote_attach_arguments(adapter_id: &str, target: &RemoteTarget) -> Value {
    match adapter_id {
        "debugpy" => {
            let mappings: Vec<Value> = target
                .mappings
                .iter()
                .map(|(local, remote)| json!({"localRoot": local, "remoteRoot": remote}))
                .collect();
            json!({
                "type": "python",
                "request": "attach",
                "connect": {"host": target.host, "port": target.port},
                "pathMappings": mappings,
            })
        }
        "codelldb" => {
            let source_map: Map<String, Value> = target
                .mappings
                .iter()
                .map(|(local, remote)| (remote.clone(), json!(local)))
                .collect();
            json!({
                "type": "lldb",
                "request": "custom",
                "targetCreateCommands": [],
                "processCreateCommands": [
                    format!("gdb-remote {}:{}", target.host, target.port)
                ],
                "sourceMap": Value::Object(source_map),
            })
        }
        "java-debug" => json!({
            "type": "java",
            "request": "attach",
            "hostName": target.host,
            "port": target.port,
        }),
        _ => json!({
            "request": "attach",
            "host": target.host,
            "port": target.port,
        }),
    }
}

fn common(spec: &LaunchSpec) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("program".into(), json!(spec.program));
    arguments.insert("args".into(), json!(spec.args));
    if let Some(cwd) = &spec.cwd {
        arguments.insert("cwd".into(), json!(cwd.display().to_string()));
    }
    if !spec.env.is_empty() {
        let env: Map<String, Value> = spec
            .env
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect();
        arguments.insert("env".into(), Value::Object(env));
    }
    arguments
}

#[cfg(test)]
mod remote_tests {
    use super::*;

    fn target() -> RemoteTarget {
        RemoteTarget {
            host: "10.0.0.4".to_string(),
            port: 5678,
            mappings: vec![("/home/me/app".to_string(), "/srv/app".to_string())],
        }
    }

    #[test]
    fn debugpy_connects_to_the_socket_and_maps_the_paths() {
        let args = remote_attach_arguments("debugpy", &target());
        assert_eq!(args["connect"]["host"], "10.0.0.4");
        assert_eq!(args["connect"]["port"], 5678);
        assert_eq!(args["pathMappings"][0]["localRoot"], "/home/me/app");
        assert_eq!(args["pathMappings"][0]["remoteRoot"], "/srv/app");
    }

    #[test]
    fn codelldb_gets_a_gdb_remote_command_and_a_source_map() {
        // codelldb has no remote-attach request of its own: connecting is
        // an LLDB command, which is why this is a `custom` request rather
        // than an `attach` one with different key names.
        let args = remote_attach_arguments("codelldb", &target());
        assert_eq!(args["request"], "custom");
        assert_eq!(args["processCreateCommands"][0], "gdb-remote 10.0.0.4:5678");
        // The map is keyed by the *remote* root, which is the direction
        // LLDB reads it in — the opposite of debugpy's.
        assert_eq!(args["sourceMap"]["/srv/app"], "/home/me/app");
    }

    #[test]
    fn java_debug_names_a_host_and_a_port() {
        let args = remote_attach_arguments("java-debug", &target());
        assert_eq!(args["hostName"], "10.0.0.4");
        assert_eq!(args["port"], 5678);
    }

    #[test]
    fn an_unknown_adapter_gets_the_plainest_reading_of_the_spec() {
        let args = remote_attach_arguments("something-else", &target());
        assert_eq!(args["request"], "attach");
        assert_eq!(args["host"], "10.0.0.4");
    }

    #[test]
    fn no_mappings_means_the_trees_match() {
        let mut plain = target();
        plain.mappings.clear();
        let args = remote_attach_arguments("debugpy", &plain);
        assert_eq!(args["pathMappings"].as_array().map(Vec::len), Some(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            program: "python3".into(),
            args: vec!["scripts/etl.py".into(), "--verbose".into()],
            cwd: Some(PathBuf::from("/p")),
            env: vec![("RUST_LOG".into(), "debug".into())],
            console: run_core::ConsoleKind::Pty,
            path_map: None,
        }
    }

    #[test]
    fn the_common_shape_carries_program_args_cwd_and_env() {
        let arguments = arguments("something-new", &spec());
        assert_eq!(arguments["program"], "python3");
        assert_eq!(arguments["args"][0], "scripts/etl.py");
        assert_eq!(arguments["cwd"], "/p");
        assert_eq!(arguments["env"]["RUST_LOG"], "debug");
    }

    #[test]
    fn codelldb_gets_its_own_type_and_a_launch_request() {
        let arguments = arguments("codelldb", &spec());
        assert_eq!(arguments["type"], "lldb");
        assert_eq!(arguments["request"], "launch");
    }

    #[test]
    fn debugpy_debugs_the_script_rather_than_the_interpreter() {
        // The run configuration launches `python3 scripts/etl.py`; debugpy
        // is the interpreter itself, so what it needs is the script.
        let arguments = arguments("debugpy", &spec());
        assert_eq!(arguments["program"], "scripts/etl.py");
        assert_eq!(arguments["args"], json!(["--verbose"]));
    }

    #[test]
    fn an_env_less_configuration_sends_no_env_key() {
        let arguments = arguments(
            "codelldb",
            &LaunchSpec {
                env: Vec::new(),
                ..spec()
            },
        );
        assert!(arguments.get("env").is_none());
    }

    #[test]
    fn attach_names_the_process_the_way_each_adapter_expects() {
        assert_eq!(attach_arguments("codelldb", 42)["pid"], 42);
        assert_eq!(attach_arguments("debugpy", 42)["processId"], 42);
        assert_eq!(attach_arguments("delve", 42)["processId"], 42);
    }
}
