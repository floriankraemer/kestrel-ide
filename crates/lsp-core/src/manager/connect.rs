//! The `initialize`/`initialized` handshake for one server process, split
//! out of `manager.rs` to keep it under the file-size ceiling.

use super::*;

/// Spawn the child and run the `initialize`/`initialized` handshake, leaving
/// the connection published and the reader positioned at the next message.
pub(super) fn connect(
    server: &Server,
    cfg: &ServerConfig,
    root_uri: &str,
    root_path: &str,
    host: &ExecHost,
) -> Result<
    (
        BufReader<std::process::ChildStdout>,
        Vec<String>,
        SignatureTriggers,
        bool,
    ),
    LspError,
> {
    // W3-1/W3-3: `ExecHost::command` (ADR-0052) replaces a bare
    // `Command::new`, which gains this crate `current_dir` and
    // `CREATE_NO_WINDOW` it never had, and runs the server inside the
    // distro for a WSL project root. `resolve_program` is what makes a
    // missing server say so plainly instead of a generic spawn failure —
    // `Local` never probes, so this is free on every project that isn't one.
    let resolved_command =
        process_exec::host::resolve_program(host, &cfg.command, Path::new(root_path)).ok_or_else(
            || LspError::Spawn {
                command: cfg.command.clone(),
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "{} not found inside the {}",
                        cfg.command,
                        wire_uris::host_noun(host)
                    ),
                ),
            },
        )?;
    let args: Vec<&str> = cfg.args.iter().map(String::as_str).collect();
    let mut command = host.command(&resolved_command, &args, Path::new(root_path), &[]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Servers are chatty on stderr and nothing reads it; a full pipe
        // would deadlock the child.
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| LspError::Spawn {
            command: cfg.command.clone(),
            source,
        })?;

    let mut stdin = child.stdin.take().expect("stdin was piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));

    // The handshake is done inline, before the connection is published, so
    // nothing else can be in flight and no dispatch table is needed yet.
    let mut init_params = json!({
        "processId": wire_uris::parent_process_id(host),
        "rootUri": root_uri,
        "capabilities": capabilities::client_capabilities(),
        "workspaceFolders": Value::Null,
    });
    if !cfg.initialization_options.is_null() {
        init_params["initializationOptions"] = cfg.initialization_options.clone();
    }
    let init = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": init_params,
    });
    write_message(
        &mut stdin,
        &serde_json::to_vec(&init).map_err(io::Error::from)?,
    )?;

    let (trigger_characters, signature_triggers, completion_resolve_supported) = loop {
        let Some(body) = read_message(&mut stdout)? else {
            return Err(LspError::Disconnected {
                method: "initialize".into(),
            });
        };
        let message: Value = serde_json::from_slice(&body).map_err(io::Error::from)?;
        if message.get("id").and_then(Value::as_i64) == Some(0) && message.get("method").is_none() {
            if let Some(error) = message.get("error") {
                return Err(response_error(error));
            }
            // What the server can do is read here, once, and published with
            // `ServerReady` — nothing else ever sees the raw result.
            let result = message.get("result").unwrap_or(&Value::Null);
            *server.capabilities.lock().unwrap() =
                result.get("capabilities").cloned().unwrap_or(Value::Null);
            // C9: read once, here, same as the other capabilities above —
            // but stored on `server` rather than threaded through the
            // return tuple, because a server may instead only tell us via a
            // *later* `client/registerCapability` (`dispatch` sets the same
            // field), and `semantic_tokens_legend` needs to answer
            // correctly either way.
            *server.semantic_tokens_legend.lock().unwrap() = semantic_tokens::parse_legend(result);
            // C10: same read-once-here convention, for the same reason —
            // csharp-ls may instead only register `textDocument/codeLens`
            // dynamically, which `code_lenses_supported` also checks.
            *server.code_lens_supported.lock().unwrap() = code_lens::is_offered(result);
            // C11: same read-once-here convention — presence of either
            // capability is the whole answer, same reasoning
            // `code_lens::is_offered` gives for its own capability.
            *server.call_hierarchy_supported.lock().unwrap() = result
                .pointer("/capabilities/callHierarchyProvider")
                .is_some();
            *server.type_hierarchy_supported.lock().unwrap() = result
                .pointer("/capabilities/typeHierarchyProvider")
                .is_some();
            break (
                parse_trigger_characters(result),
                parse_signature_triggers(result),
                parse_resolve_provider(result),
            );
        }
        // Anything else before the response (log messages, server requests)
        // is dropped: the client isn't observable yet.
    };

    let initialized = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
    write_message(
        &mut stdin,
        &serde_json::to_vec(&initialized).map_err(io::Error::from)?,
    )?;
    stdin.flush()?;

    // A respawned process has seen none of the documents the last one was
    // sent: forget them, so the next `did_open` per document reaches it (a
    // `didOpen` that failed while no connection existed was un-marked by
    // `notify_servers_where`).
    server.opened.lock().unwrap().clear();
    *server.conn.lock().unwrap() = Some(Conn { stdin, child });
    Ok((
        stdout,
        trigger_characters,
        signature_triggers,
        completion_resolve_supported,
    ))
}
