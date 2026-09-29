//! The IDE's MCP server: local Streamable-HTTP JSON-RPC on 127.0.0.1, per
//! decision A4 and ADR-0004. Qt-free by design — mirrors
//! `editor-core`/`project-model`/`syntax-core`, no dependency on `ui-shell`.
//!
//! Two surfaces sit on one dispatcher:
//!
//! - **MCP proper** — `initialize`, `tools/list`, `tools/call` — which is
//!   what an off-the-shelf agent speaks.
//! - **The flat method names** (`ping`, `search_text`, …), callable
//!   directly, which is what `curl` and this crate's own tests use.
//!   `tools/call` is a thin wrapper over the same [`dispatch_method`], so
//!   the two surfaces cannot drift apart.
//!
//! Editor state is reached only through [`EditorCommand`]s that `ui-shell`
//! answers on the Qt thread. The project index is reached directly: an
//! [`IndexHandle`] is already an `Arc<RwLock<…>>` because the UI's own
//! searches run off the Qt thread, and every query takes `&self`, so an MCP
//! tool is one more concurrent reader on a blocking worker.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

mod cursor;
pub use cursor::CursorPosition;

/// The MCP revision this server implements. A client that asks for a
/// different one still gets told this one — the spec's prescribed answer
/// for a version we don't speak.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How many hits an index tool returns unless the caller asks for a
/// different ceiling. The consumer is a model reading results into a
/// context window, so the default is deliberately small.
const DEFAULT_RESULT_LIMIT: usize = 100;

/// One open editor tab, as the MCP `list_open_buffers` tool reports it
/// (M3/M4). Deliberately generic (no `app_core::TabId`) — this crate stays
/// independent of `app-core`, per the plan's crate boundaries.
#[derive(Debug, Clone, Serialize)]
pub struct BufferInfo {
    pub tab_id: u64,
    pub title: String,
}

/// One project-tree entry, as the MCP `list_project_tree` tool reports it
/// (M4). Deliberately generic (a bare path string, not `project_model`'s
/// arena `TreeNode`) — same independence reasoning as `BufferInfo`.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectTreeEntry {
    pub path: String,
    pub is_dir: bool,
}

/// Commands the MCP transport sends to the running editor. `mcp-server`
/// never touches editor state itself — it only defines the message shape;
/// `ui-shell` is the (only) consumer, dispatching each command onto the
/// relevant QObject's `CxxQtThread` (M3), reusing the exact cross-thread
/// pattern `bridge.rs`'s filesystem-watcher relay already established. Each
/// variant carries its own `oneshot::Sender` so the HTTP handler that sent
/// it can `.await` the one reply it's waiting for, no correlation ids
/// needed.
pub enum EditorCommand {
    ListOpenBuffers(oneshot::Sender<Vec<BufferInfo>>),
    ListProjectTree(oneshot::Sender<Vec<ProjectTreeEntry>>),
    ReadBuffer {
        tab_id: u64,
        respond: oneshot::Sender<Option<String>>,
    },
    GetCursorPosition {
        tab_id: u64,
        respond: oneshot::Sender<Option<CursorPosition>>,
    },
    /// The live buffer text for `path` if some tab holds it, unsaved edits
    /// included; `None` when the file is not open. `resolve_declaration`
    /// needs it so navigation resolves against what the user is looking at
    /// instead of a stale copy on disk.
    BufferContentForPath {
        path: String,
        respond: oneshot::Sender<Option<String>>,
    },
    /// Open `path` as a new tab, or focus its existing tab if already open
    /// — same semantics as the UI's own "Open File" (M5). `Ok` carries the
    /// tab id; `Err` the same user-facing message an `AppError` would
    /// display.
    OpenFile {
        path: String,
        respond: oneshot::Sender<Result<u64, String>>,
    },
    /// Replace the tab's in-memory content, same as a human typing — does
    /// not write to disk (M5; see `save_buffer` for that).
    EditBuffer {
        tab_id: u64,
        content: String,
        respond: oneshot::Sender<Result<(), String>>,
    },
    /// Write the tab's current in-memory content to disk (M5).
    SaveBuffer {
        tab_id: u64,
        respond: oneshot::Sender<Result<(), String>>,
    },
}

/// The channel half `mcp-server` holds and sends on; the caller (`ui-shell`)
/// creates the `(sender, receiver)` pair, passes the sender to [`start`],
/// and keeps the `mpsc::UnboundedReceiver<EditorCommand>` for itself to
/// consume on its own listener thread.
pub type EditorCommandSender = mpsc::UnboundedSender<EditorCommand>;

/// The project index, shared with `ui-shell`'s `SearchModel` rather than
/// rebuilt here — one index per process, one lock, one set of queries.
pub type IndexHandle = Arc<RwLock<index_core::IndexSlot>>;

/// `{port, token}` written to disk so an MCP client can discover a running
/// instance without a fixed port. Lives at `<config_dir>/mcp-discovery.json`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Discovery {
    pub port: u16,
    pub token: String,
}

pub fn discovery_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("mcp-discovery.json")
}

// ponytail: token is time+counter mixed, not a CSPRNG. Fine for a
// loopback-only, per-launch, short-lived dev credential; swap for the `rand`
// crate if this ever needs to resist a determined local attacker.
fn generate_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}{:016x}", nanos, counter ^ 0x5bd1_e995)
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    /// Absent for a JSON-RPC *notification* — `notifications/initialized`
    /// is the one every MCP client sends right after the handshake. A
    /// notification must never be answered with a response body, so this
    /// cannot be a bare `Value`.
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcError {
    /// A malformed call: the caller has to change the request itself.
    fn invalid_params(message: impl Into<String>) -> Self {
        RpcError {
            code: -32602,
            message: message.into(),
        }
    }

    fn method_not_found(method: &str) -> Self {
        RpcError {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }

    /// The UI-thread listener isn't running yet, or has already shut down —
    /// either way the command couldn't reach (or come back from) the
    /// editor. Not a JSON-RPC framing problem, so this is a server-defined
    /// code in the reserved application range rather than one of the
    /// JSON-RPC spec's own (which top out at -32000).
    fn editor_unavailable() -> Self {
        RpcError {
            code: -32000,
            message: "editor is not available".into(),
        }
    }

    /// The call was well-formed but the editor or the index refused it —
    /// the message is the one a user would see in the UI.
    fn operation_failed(message: impl Into<String>) -> Self {
        RpcError {
            code: -32001,
            message: message.into(),
        }
    }

    /// Whether this is the caller's protocol mistake — reported by
    /// `tools/call` as a JSON-RPC error — rather than a tool that ran and
    /// failed, which MCP wants reported inside the result so the model can
    /// read it and react.
    fn is_protocol_error(&self) -> bool {
        self.code == -32601 || self.code == -32602
    }
}

/// Pull `params.tab_id` out as a `u64`, or an "invalid params" error.
fn required_tab_id(params: &Value) -> Result<u64, RpcError> {
    params
        .get("tab_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::invalid_params("params.tab_id must be a non-negative integer"))
}

/// Pull `params.<key>` out as a `String`, or an "invalid params" error.
fn required_string(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params(format!("params.{key} must be a string")))
}

fn required_usize(params: &Value, key: &str) -> Result<usize, RpcError> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .ok_or_else(|| {
            RpcError::invalid_params(format!("params.{key} must be a non-negative integer"))
        })
}

fn optional_bool(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn optional_limit(params: &Value) -> usize {
    params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_RESULT_LIMIT)
}

fn search_options(params: &Value) -> editor_core::SearchOptions {
    editor_core::SearchOptions {
        regex: optional_bool(params, "is_regex", false),
        case_sensitive: optional_bool(params, "case_sensitive", false),
    }
}

#[derive(Clone)]
struct AppState {
    token: String,
    commands: EditorCommandSender,
    index: IndexHandle,
}

impl AppState {
    /// Send one `EditorCommand` and await its reply, mapping both a dead
    /// channel and a dropped responder to "editor is not available".
    async fn ask_editor<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> EditorCommand,
    ) -> Result<T, RpcError> {
        let (respond, receive) = oneshot::channel();
        self.commands
            .send(make(respond))
            .map_err(|_| RpcError::editor_unavailable())?;
        receive.await.map_err(|_| RpcError::editor_unavailable())
    }

    /// Run `query` against the index on a blocking worker. The lock and the
    /// tantivy/ripgrep work underneath both block, so doing this inline
    /// would stall the whole async runtime for the length of a
    /// project-wide scan.
    async fn with_index<T, F>(&self, query: F) -> Result<T, RpcError>
    where
        T: Send + 'static,
        F: FnOnce(&index_core::TextIndex) -> Result<T, RpcError> + Send + 'static,
    {
        let index = Arc::clone(&self.index);
        blocking(move || {
            let guard = index.read().expect("index lock poisoned");
            let Some(ready) = guard.ready() else {
                // `IndexSlot` owns the wording for "no project" vs. "still
                // building" vs. "build failed" — the UI reads the same
                // strings, so an agent and a human get the same answer.
                return Err(RpcError::operation_failed(
                    guard.unavailable_reason().unwrap_or_default(),
                ));
            };
            query(ready)
        })
        .await
    }

    /// [`Self::with_index`] for the one tool that writes
    /// (`replace_in_files`), which needs `&mut TextIndex` to re-index what
    /// it rewrote.
    async fn with_index_mut<T, F>(&self, query: F) -> Result<T, RpcError>
    where
        T: Send + 'static,
        F: FnOnce(&mut index_core::TextIndex) -> Result<T, RpcError> + Send + 'static,
    {
        let index = Arc::clone(&self.index);
        blocking(move || {
            let mut guard = index.write().expect("index lock poisoned");
            if guard.ready().is_none() {
                return Err(RpcError::operation_failed(
                    guard.unavailable_reason().unwrap_or_default(),
                ));
            }
            query(guard.ready_mut().expect("checked immediately above"))
        })
        .await
    }
}

/// `spawn_blocking` with the join error folded into our own error type — a
/// panicking worker is a bug, but it must not take the server down with it.
async fn blocking<T, F>(work: F) -> Result<T, RpcError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, RpcError> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(_) => Err(RpcError::operation_failed("the index task did not finish")),
    }
}

fn index_error(err: index_core::IndexError) -> RpcError {
    RpcError::operation_failed(err.to_string())
}

// --- index result serialization -----------------------------------------
//
// `index-core` deliberately carries no serde dependency, so the JSON shape
// of a hit is decided here, at the boundary that publishes it.

fn search_match_json(m: &index_core::SearchMatch) -> Value {
    json!({
        "path": m.path.to_string_lossy(),
        "line": m.line,
        "start": m.start,
        "end": m.end,
        "text": m.line_text,
    })
}

fn file_match_json(m: &index_core::FileMatch) -> Value {
    json!({
        "path": m.path.to_string_lossy(),
        "relative": m.relative,
        "score": m.score,
    })
}

fn symbol_match_json(m: &index_core::SymbolMatch) -> Value {
    json!({
        "name": m.name,
        "kind": m.kind.map(|k| format!("{k:?}")),
        "path": m.path.to_string_lossy(),
        "line": m.line,
        "column": m.col,
        "is_definition": m.is_definition,
        "container": m.container,
    })
}

fn resolution_json(r: &index_core::Resolution) -> Value {
    json!({
        "name": r.name,
        "tier": match r.tier {
            index_core::ResolutionTier::LocalFile => "local_file",
            index_core::ResolutionTier::Project => "project",
            index_core::ResolutionTier::None => "none",
        },
        "candidates": r.candidates.iter().map(symbol_match_json).collect::<Vec<_>>(),
    })
}

// --- MCP protocol surface ------------------------------------------------

/// The tool catalogue `tools/list` publishes. Schemas are hand-written
/// literals rather than derived: there are a dozen of them, they are the
/// prompt an agent reads before choosing a tool, and the wording matters
/// more than the saved keystrokes.
fn tool_catalogue() -> Value {
    json!([
        tool("ping", "Check that the IDE's MCP server is responding.", json!({})),
        tool(
            "list_open_buffers",
            "List the tabs currently open in the editor, with the tab ids the other buffer tools take.",
            json!({}),
        ),
        tool(
            "list_project_tree",
            "List every file and directory in the open project.",
            json!({}),
        ),
        tool(
            "read_buffer",
            "Read a tab's current text, including edits the user has not saved yet.",
            json!({"tab_id": {"type": "integer", "description": "Tab id from list_open_buffers."}}),
        ),
        tool(
            "get_cursor_position",
            "Where the caret sits in a tab (1-based line, 0-based column).",
            json!({"tab_id": {"type": "integer"}}),
        ),
        tool(
            "open_file",
            "Open a file in the editor, or focus it if it is already open. Returns its tab id.",
            json!({"path": {"type": "string", "description": "Absolute path."}}),
        ),
        tool(
            "edit_buffer",
            "Replace a tab's text in memory, exactly as typing would. Does not write to disk — call save_buffer for that.",
            json!({"tab_id": {"type": "integer"}, "content": {"type": "string"}}),
        ),
        tool(
            "save_buffer",
            "Write a tab's current text to disk.",
            json!({"tab_id": {"type": "integer"}}),
        ),
        tool(
            "index_status",
            "Whether the project index can answer queries, and how many files it holds.",
            json!({}),
        ),
        tool(
            "search_text",
            "Search the project's text. Returns each match with its file, 1-based line, byte span within the line, and the line's text.",
            json!({
                "pattern": {"type": "string"},
                "is_regex": {"type": "boolean", "description": "Treat pattern as a regex. Default false."},
                "case_sensitive": {"type": "boolean", "description": "Default false."},
                "limit": {"type": "integer", "description": "Maximum matches to return. Default 100."}
            }),
        ),
        tool(
            "find_files",
            "Fuzzy-match a path fragment against every file in the project.",
            json!({
                "query": {"type": "string"},
                "limit": {"type": "integer", "description": "Default 100."}
            }),
        ),
        tool(
            "find_definitions",
            "Find where symbols matching a name are defined, best match first.",
            json!({
                "query": {"type": "string"},
                "limit": {"type": "integer", "description": "Default 100."}
            }),
        ),
        tool(
            "find_usages",
            "Find every occurrence of an exact symbol name, definitions included. Name-based, not type-resolved: two unrelated methods with the same name both match.",
            json!({"name": {"type": "string"}}),
        ),
        tool(
            "find_implementations",
            "Find the types that extend or implement a given type.",
            json!({"supertype": {"type": "string"}}),
        ),
        tool(
            "find_supertypes",
            "Find the types a given type extends or implements.",
            json!({"type_name": {"type": "string"}}),
        ),
        tool(
            "resolve_declaration",
            "Resolve what the identifier at a byte offset refers to, preferring a binding in the same file and falling back to project-wide definitions. Uses the open buffer's unsaved text when the file is open.",
            json!({
                "path": {"type": "string", "description": "Absolute path."},
                "byte_offset": {"type": "integer", "description": "Byte offset of the identifier within the file."}
            }),
        ),
        tool(
            "replace_in_files",
            "Replace every match of a pattern across the project, on disk. Regex captures ($1, …) expand against each match. Returns how many files and matches were rewritten, and how many files were skipped because they changed since the search.",
            json!({
                "pattern": {"type": "string"},
                "replacement": {"type": "string"},
                "is_regex": {"type": "boolean", "description": "Default false."},
                "case_sensitive": {"type": "boolean", "description": "Default false."}
            }),
        ),
    ])
}

/// One `tools/list` entry. Every property listed in `properties` that has no
/// documented default is required — the tools here have no optional-vs-
/// required subtlety beyond that.
fn tool(name: &str, description: &str, properties: Value) -> Value {
    let required: Vec<&str> = properties
        .as_object()
        .map(|props| {
            props
                .iter()
                .filter(|(_, schema)| {
                    !schema
                        .get("description")
                        .and_then(Value::as_str)
                        .is_some_and(|d| d.contains("Default"))
                })
                .map(|(key, _)| key.as_str())
                .collect()
        })
        .unwrap_or_default();
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        },
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "ide", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// `tools/call`: run the named tool through the same dispatcher the flat
/// method names use, then package the outcome the way MCP expects — a tool
/// that ran and failed reports `isError` inside the result, so the model
/// sees the message; only a malformed call is a JSON-RPC error.
async fn call_tool(state: &AppState, params: &Value) -> Result<Value, RpcError> {
    let name = required_string(params, "name")?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    match dispatch_method(state, &name, &arguments).await {
        Ok(value) => Ok(json!({
            "content": [{"type": "text", "text": text_content(&value)}],
            "structuredContent": value,
            "isError": false,
        })),
        Err(err) if err.is_protocol_error() => Err(err),
        Err(err) => Ok(json!({
            "content": [{"type": "text", "text": err.message}],
            "isError": true,
        })),
    }
}

/// MCP tool results are text blocks. A bare JSON string is passed through
/// as-is (`"pong"` reads better than `"\"pong\""`); anything structured is
/// pretty-printed so it stays readable in a transcript.
fn text_content(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Every tool, by name. `tools/call` and the flat method surface both land
/// here, which is what keeps them identical.
async fn dispatch_method(
    state: &AppState,
    method: &str,
    params: &Value,
) -> Result<Value, RpcError> {
    match method {
        "ping" => Ok(Value::String("pong".into())),

        // --- editor -----------------------------------------------------
        "list_open_buffers" => {
            let buffers = state.ask_editor(EditorCommand::ListOpenBuffers).await?;
            Ok(serde_json::to_value(buffers).expect("Vec<BufferInfo> serializes"))
        }
        "list_project_tree" => {
            let entries = state.ask_editor(EditorCommand::ListProjectTree).await?;
            Ok(serde_json::to_value(entries).expect("Vec<ProjectTreeEntry> serializes"))
        }
        "read_buffer" => {
            let tab_id = required_tab_id(params)?;
            let content = state
                .ask_editor(|respond| EditorCommand::ReadBuffer { tab_id, respond })
                .await?;
            Ok(json!({ "content": content }))
        }
        "get_cursor_position" => {
            let tab_id = required_tab_id(params)?;
            let position = state
                .ask_editor(|respond| EditorCommand::GetCursorPosition { tab_id, respond })
                .await?;
            Ok(serde_json::to_value(position).expect("Option<CursorPosition> serializes"))
        }
        "open_file" => {
            let path = required_string(params, "path")?;
            let tab_id = state
                .ask_editor(|respond| EditorCommand::OpenFile { path, respond })
                .await?
                .map_err(RpcError::operation_failed)?;
            Ok(json!({ "tab_id": tab_id }))
        }
        "edit_buffer" => {
            let tab_id = required_tab_id(params)?;
            let content = required_string(params, "content")?;
            state
                .ask_editor(|respond| EditorCommand::EditBuffer {
                    tab_id,
                    content,
                    respond,
                })
                .await?
                .map_err(RpcError::operation_failed)?;
            Ok(Value::Null)
        }
        "save_buffer" => {
            let tab_id = required_tab_id(params)?;
            state
                .ask_editor(|respond| EditorCommand::SaveBuffer { tab_id, respond })
                .await?
                .map_err(RpcError::operation_failed)?;
            Ok(Value::Null)
        }

        // --- index ------------------------------------------------------
        "index_status" => {
            let index = Arc::clone(&state.index);
            blocking(move || {
                let guard = index.read().expect("index lock poisoned");
                Ok(match guard.ready() {
                    Some(ready) => json!({
                        "ready": true,
                        "root": ready.root().to_string_lossy(),
                        "indexed_file_count": ready.indexed_file_count(),
                    }),
                    None => json!({
                        "ready": false,
                        "reason": guard.unavailable_reason(),
                    }),
                })
            })
            .await
        }
        "search_text" => {
            let pattern = required_string(params, "pattern")?;
            let opts = search_options(params);
            let limit = optional_limit(params);
            state
                .with_index(move |index| {
                    let matches = index
                        .search_with(
                            &pattern,
                            opts.regex,
                            opts.case_sensitive,
                            limit,
                            &AtomicBool::new(false),
                        )
                        .map_err(index_error)?;
                    Ok(json!({
                        "matches": matches.iter().map(search_match_json).collect::<Vec<_>>(),
                    }))
                })
                .await
        }
        "find_files" => {
            let query = required_string(params, "query")?;
            let limit = optional_limit(params);
            state
                .with_index(move |index| {
                    let matches = index.find_files(&query, limit);
                    Ok(json!({
                        "files": matches.iter().map(file_match_json).collect::<Vec<_>>(),
                    }))
                })
                .await
        }
        "find_definitions" => {
            let query = required_string(params, "query")?;
            let limit = optional_limit(params);
            state
                .with_index(move |index| {
                    let matches = index
                        .find_definitions_ranked(&query, limit)
                        .map_err(index_error)?;
                    Ok(json!({
                        "symbols": matches.iter().map(symbol_match_json).collect::<Vec<_>>(),
                    }))
                })
                .await
        }
        "find_usages" => {
            let name = required_string(params, "name")?;
            state
                .with_index(move |index| {
                    let matches = index.find_usages(&name).map_err(index_error)?;
                    Ok(json!({
                        "symbols": matches.iter().map(symbol_match_json).collect::<Vec<_>>(),
                    }))
                })
                .await
        }
        "find_implementations" => {
            let supertype = required_string(params, "supertype")?;
            state
                .with_index(move |index| {
                    let matches = index
                        .find_implementations(&supertype)
                        .map_err(index_error)?;
                    Ok(json!({
                        "symbols": matches.iter().map(symbol_match_json).collect::<Vec<_>>(),
                    }))
                })
                .await
        }
        "find_supertypes" => {
            let type_name = required_string(params, "type_name")?;
            state
                .with_index(move |index| {
                    let matches = index.find_supertypes(&type_name).map_err(index_error)?;
                    Ok(json!({
                        "symbols": matches.iter().map(symbol_match_json).collect::<Vec<_>>(),
                    }))
                })
                .await
        }
        "resolve_declaration" => {
            let path = required_string(params, "path")?;
            let byte_offset = required_usize(params, "byte_offset")?;
            // The open buffer wins over the file: the user may be sitting
            // on unsaved edits, and resolving against disk would answer
            // about text that is no longer on screen.
            let open_content = state
                .ask_editor(|respond| EditorCommand::BufferContentForPath {
                    path: path.clone(),
                    respond,
                })
                .await?;
            state
                .with_index(move |index| {
                    let path = PathBuf::from(path);
                    let content = match open_content {
                        Some(content) => content,
                        None => std::fs::read_to_string(&path)
                            .map_err(|e| RpcError::operation_failed(e.to_string()))?,
                    };
                    let resolution = index
                        .resolve_declaration(&path, &content, byte_offset)
                        .map_err(index_error)?;
                    Ok(resolution_json(&resolution))
                })
                .await
        }
        "replace_in_files" => {
            let pattern = required_string(params, "pattern")?;
            let replacement = required_string(params, "replacement")?;
            let opts = search_options(params);
            state
                .with_index_mut(move |index| {
                    let matches = index
                        .search_with(
                            &pattern,
                            opts.regex,
                            opts.case_sensitive,
                            usize::MAX,
                            &AtomicBool::new(false),
                        )
                        .map_err(index_error)?;
                    let spans: Vec<_> = matches
                        .into_iter()
                        .map(|m| (m.path, m.line, m.start, m.end))
                        .collect();
                    let resolved =
                        index_core::resolve_replacements(&spans, &pattern, &replacement, opts)
                            .map_err(RpcError::operation_failed)?;
                    let report = index.replace_in_files(&resolved).map_err(index_error)?;
                    Ok(json!({
                        "files": report.files,
                        "matches": report.matches,
                        "skipped_files": report.skipped_files,
                    }))
                })
                .await
        }

        other => Err(RpcError::method_not_found(other)),
    }
}

async fn rpc_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RpcRequest>,
) -> Response {
    let auth_ok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {}", state.token))
        .unwrap_or(false);
    if !auth_ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    if req.jsonrpc != "2.0" {
        return respond(
            req.id,
            Err(RpcError {
                code: -32600,
                message: "invalid jsonrpc version".into(),
            }),
        );
    }

    let outcome = match req.method.as_str() {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(json!({ "tools": tool_catalogue() })),
        "tools/call" => call_tool(&state, &req.params).await,
        // Client-to-server notifications carry no result and, having no
        // `id`, get no response body either — accepting them silently is
        // the whole contract.
        method if method.starts_with("notifications/") => Ok(Value::Null),
        method => dispatch_method(&state, method, &req.params).await,
    };

    respond(req.id, outcome)
}

/// Frame one outcome. A request (`id` present) gets a JSON-RPC response; a
/// notification (`id` absent) gets `202 Accepted` and no body, which is
/// what the Streamable-HTTP transport prescribes.
fn respond(id: Option<Value>, outcome: Result<Value, RpcError>) -> Response {
    let Some(id) = id else {
        return StatusCode::ACCEPTED.into_response();
    };
    let response = match outcome {
        Ok(result) => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        },
    };
    Json(response).into_response()
}

/// A running server. Dropping this without calling [`Self::shutdown`]
/// leaves the server task running detached — always shut it down.
pub struct ServerHandle {
    pub port: u16,
    pub token: String,
    config_dir: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl ServerHandle {
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Abort rather than wait out the graceful shutdown. An in-flight
        // tool call can be parked waiting for the editor thread to answer
        // its `EditorCommand`, and the caller of `shutdown` is very often
        // that same editor thread — waiting for those requests to finish
        // would deadlock the two against each other. A client mid-call
        // sees its connection drop, which is the honest answer for "the
        // server was just switched off".
        self.join.abort();
        let _ = self.join.await;
        // A discovery file that outlives its server points every client at
        // a dead port, so it goes when the server does.
        let _ = std::fs::remove_file(discovery_file_path(&self.config_dir));
    }
}

/// Start the MCP server on `127.0.0.1:port` (`0` = OS-assigned, ADR-0004's
/// default, which is what keeps two IDE instances from colliding), write
/// the discovery file into `config_dir`, and return a handle to it.
///
/// `commands` is where editor-touching tool calls (`list_open_buffers`, …)
/// send their `EditorCommand`s — the caller owns the matching receiver and
/// runs the editor side of each command (M3). `index` is the same project
/// index the UI's own search uses; index tools query it directly.
///
/// A non-zero `port` that is already taken fails here rather than falling
/// back to an arbitrary one: a client configured for a fixed port should
/// hear about the collision, not silently talk to nothing.
pub async fn start(
    config_dir: &Path,
    commands: EditorCommandSender,
    index: IndexHandle,
    port: u16,
) -> io::Result<ServerHandle> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    let token = generate_token();

    std::fs::create_dir_all(config_dir)?;
    let discovery = Discovery {
        port,
        token: token.clone(),
    };
    std::fs::write(
        discovery_file_path(config_dir),
        serde_json::to_string_pretty(&discovery).expect("Discovery serializes"),
    )?;

    let state = AppState {
        token,
        commands,
        index,
    };
    let app = Router::new()
        // `/rpc` is what this server has always answered on; `/mcp` is what
        // people expect to paste into an MCP client's config. Same handler.
        .route("/rpc", post(rpc_handler))
        .route("/mcp", post(rpc_handler))
        .with_state(state.clone());

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("mcp-server: serve failed");
    });

    Ok(ServerHandle {
        port,
        token: state.token,
        config_dir: config_dir.to_path_buf(),
        shutdown_tx: Some(shutdown_tx),
        join,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot with no project in it — enough for every editor-side test,
    /// which never touches the index.
    fn empty_index() -> IndexHandle {
        Arc::new(RwLock::new(index_core::IndexSlot::NoProject))
    }

    #[tokio::test]
    async fn ping_round_trips_over_http_with_valid_token() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"], "pong");

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth("not-the-token")
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 401);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn list_open_buffers_round_trips_through_the_command_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        // Stands in for ui-shell's real listener thread (M3): answers the
        // one command this test sends with a canned buffer list.
        tokio::spawn(async move {
            if let Some(EditorCommand::ListOpenBuffers(respond)) = rx.recv().await {
                let _ = respond.send(vec![BufferInfo {
                    tab_id: 1,
                    title: "a.rs".to_string(),
                }]);
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "list_open_buffers"}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"][0]["tab_id"], 1);
        assert_eq!(body["result"][0]["title"], "a.rs");

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn list_open_buffers_with_no_listener_returns_editor_unavailable_error() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx); // No listener thread running — send() will fail.
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "list_open_buffers"}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32000);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn list_project_tree_round_trips_through_the_command_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        tokio::spawn(async move {
            if let Some(EditorCommand::ListProjectTree(respond)) = rx.recv().await {
                let _ = respond.send(vec![ProjectTreeEntry {
                    path: "/proj/a.txt".to_string(),
                    is_dir: false,
                }]);
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "list_project_tree"}))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"][0]["path"], "/proj/a.txt");
        assert_eq!(body["result"][0]["is_dir"], false);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn read_buffer_round_trips_through_the_command_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        tokio::spawn(async move {
            if let Some(EditorCommand::ReadBuffer { tab_id, respond }) = rx.recv().await {
                assert_eq!(tab_id, 7);
                let _ = respond.send(Some("fn main() {}".to_string()));
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "read_buffer", "params": {"tab_id": 7}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"]["content"], "fn main() {}");

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn read_buffer_without_tab_id_is_invalid_params() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "read_buffer"}))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32602);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn get_cursor_position_round_trips_through_the_command_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        tokio::spawn(async move {
            if let Some(EditorCommand::GetCursorPosition { tab_id, respond }) = rx.recv().await {
                assert_eq!(tab_id, 3);
                let _ = respond.send(Some(CursorPosition { line: 2, column: 5 }));
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "get_cursor_position", "params": {"tab_id": 3}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"]["line"], 2);
        assert_eq!(body["result"]["column"], 5);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn open_file_round_trips_through_the_command_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        tokio::spawn(async move {
            if let Some(EditorCommand::OpenFile { path, respond }) = rx.recv().await {
                assert_eq!(path, "/proj/a.txt");
                let _ = respond.send(Ok(42));
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "open_file", "params": {"path": "/proj/a.txt"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"]["tab_id"], 42);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn open_file_failure_surfaces_the_editor_error_message() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        tokio::spawn(async move {
            if let Some(EditorCommand::OpenFile { respond, .. }) = rx.recv().await {
                let _ = respond.send(Err("file looks binary".to_string()));
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "open_file", "params": {"path": "/proj/bin"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32001);
        assert_eq!(body["error"]["message"], "file looks binary");

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn edit_buffer_round_trips_through_the_command_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        tokio::spawn(async move {
            if let Some(EditorCommand::EditBuffer {
                tab_id,
                content,
                respond,
            }) = rx.recv().await
            {
                assert_eq!(tab_id, 5);
                assert_eq!(content, "new content");
                let _ = respond.send(Ok(()));
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "edit_buffer",
                "params": {"tab_id": 5, "content": "new content"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].is_null());

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn save_buffer_round_trips_through_the_command_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        tokio::spawn(async move {
            if let Some(EditorCommand::SaveBuffer { tab_id, respond }) = rx.recv().await {
                assert_eq!(tab_id, 9);
                let _ = respond.send(Ok(()));
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "save_buffer", "params": {"tab_id": 9}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].is_null());

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn unknown_method_returns_rpc_error_not_http_error() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/rpc", handle.port))
            .bearer_auth(&handle.token)
            .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "nonexistent"}))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32601);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn discovery_file_is_written_with_matching_port_and_token() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();

        let contents = std::fs::read_to_string(discovery_file_path(dir.path())).unwrap();
        let discovery: Discovery = serde_json::from_str(&contents).unwrap();
        assert_eq!(discovery.port, handle.port);
        assert_eq!(discovery.token, handle.token);

        handle.shutdown().await;
    }
    // --- MCP protocol ---------------------------------------------------

    /// One server plus an HTTP client already pointed at it, since every
    /// protocol test needs the same four lines otherwise.
    struct Harness {
        handle: ServerHandle,
        client: reqwest::Client,
    }

    impl Harness {
        async fn call(&self, body: serde_json::Value) -> reqwest::Response {
            self.client
                .post(format!("http://127.0.0.1:{}/mcp", self.handle.port))
                .bearer_auth(&self.handle.token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }

        /// `tools/call` a tool and return the result object.
        async fn tool(&self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
            let resp = self
                .call(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments}
                }))
                .await;
            assert_eq!(resp.status(), 200);
            resp.json::<serde_json::Value>().await.unwrap()["result"].clone()
        }
    }

    #[tokio::test]
    async fn initialize_advertises_the_tools_capability() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        let resp = harness
            .call(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}
            }))
            .await;

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(body["result"]["capabilities"]["tools"].is_object());
        assert_eq!(body["result"]["serverInfo"]["name"], "ide");

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn a_notification_is_accepted_without_a_response_body() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        // No "id": this is a notification, and answering it would break
        // clients that are not expecting a reply.
        let resp = harness
            .call(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;

        assert_eq!(resp.status(), 202);
        assert!(resp.text().await.unwrap().is_empty());

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn tools_list_describes_every_tool_with_a_schema() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        let resp = harness
            .call(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .await;

        let body: serde_json::Value = resp.json().await.unwrap();
        let tools = body["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 17, "got {} tools", tools.len());
        for entry in tools {
            assert!(entry["name"].as_str().is_some_and(|n| !n.is_empty()));
            assert!(entry["description"].as_str().is_some_and(|d| !d.is_empty()));
            assert_eq!(entry["inputSchema"]["type"], "object");
        }
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "read_buffer",
            "search_text",
            "find_definitions",
            "find_usages",
            "resolve_declaration",
            "replace_in_files",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from tools/list"
            );
        }

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn tools_call_reaches_the_same_editor_commands_as_the_flat_method() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        tokio::spawn(async move {
            if let Some(EditorCommand::ReadBuffer { tab_id, respond }) = rx.recv().await {
                assert_eq!(tab_id, 7);
                let _ = respond.send(Some("fn main() {}".to_string()));
            }
        });

        let result = harness
            .tool("read_buffer", serde_json::json!({"tab_id": 7}))
            .await;

        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["content"], "fn main() {}");
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("fn main() {}"));

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn a_tool_that_runs_and_fails_reports_is_error_not_a_protocol_error() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        tokio::spawn(async move {
            if let Some(EditorCommand::OpenFile { respond, .. }) = rx.recv().await {
                let _ = respond.send(Err("file looks binary".to_string()));
            }
        });

        let result = harness
            .tool("open_file", serde_json::json!({"path": "/proj/bin"}))
            .await;

        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "file looks binary");

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn calling_an_unknown_tool_is_a_protocol_error() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        let resp = harness
            .call(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "no_such_tool", "arguments": {}}
            }))
            .await;

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32601);

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn a_fixed_port_is_bound_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();

        // Let the OS pick a free port, then hand that same number back as a
        // fixed request — a hardcoded constant would collide on a busy CI box.
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let wanted = probe.local_addr().unwrap().port();
        drop(probe);

        let handle = start(dir.path(), tx, empty_index(), wanted).await.unwrap();

        assert_eq!(handle.port, wanted);
        let discovery: Discovery = serde_json::from_str(
            &std::fs::read_to_string(discovery_file_path(dir.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(discovery.port, wanted);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_does_not_wait_for_a_request_parked_on_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let port = handle.port;
        let token = handle.token.clone();

        // A listener that takes the command and never answers — the shape
        // of a Qt thread that is itself blocked calling shutdown.
        let parked = tokio::spawn(async move {
            let command = rx.recv().await;
            std::future::pending::<()>().await;
            drop(command);
        });
        tokio::spawn(async move {
            let _ = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{port}/rpc"))
                .bearer_auth(token)
                .json(&serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "read_buffer",
                    "params": {"tab_id": 1}
                }))
                .send()
                .await;
        });
        // Give the request time to reach the handler and park there.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        tokio::time::timeout(std::time::Duration::from_secs(5), handle.shutdown())
            .await
            .expect("shutdown must not wait for an unanswerable in-flight request");

        parked.abort();
    }

    #[tokio::test]
    async fn shutdown_removes_the_discovery_file() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        assert!(discovery_file_path(dir.path()).exists());

        handle.shutdown().await;

        // A discovery file pointing at a dead port is worse than none.
        assert!(!discovery_file_path(dir.path()).exists());
    }

    // --- index tools ------------------------------------------------------

    /// A real index over a small fixture project — index tools are tested
    /// against `index-core` itself, never a stand-in.
    fn indexed_fixture() -> (tempfile::TempDir, IndexHandle) {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("alpha.rs"),
            "struct Widget;\n\nfn build_widget() -> Widget {\n    Widget\n}\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("beta.rs"),
            "fn main() {\n    let w = build_widget();\n}\n",
        )
        .unwrap();
        let index = index_core::TextIndex::build(project.path()).unwrap();
        (
            project,
            Arc::new(RwLock::new(index_core::IndexSlot::Ready(Box::new(index)))),
        )
    }

    async fn indexed_harness() -> (tempfile::TempDir, tempfile::TempDir, Harness) {
        let config = tempfile::tempdir().unwrap();
        let (project, index) = indexed_fixture();
        let (tx, rx) = mpsc::unbounded_channel();
        // No editor listener: index tools must not need one.
        drop(rx);
        let handle = start(config.path(), tx, index, 0).await.unwrap();
        (
            project,
            config,
            Harness {
                handle,
                client: reqwest::Client::new(),
            },
        )
    }

    #[tokio::test]
    async fn index_status_reports_the_root_and_file_count() {
        let (project, _config, harness) = indexed_harness().await;

        let result = harness.tool("index_status", serde_json::json!({})).await;

        assert_eq!(result["isError"], false);
        let status = &result["structuredContent"];
        assert_eq!(status["ready"], true);
        assert_eq!(status["root"], project.path().to_string_lossy().as_ref());
        assert_eq!(status["indexed_file_count"], 2);

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn search_text_returns_matches_with_file_line_and_span() {
        let (_project, _config, harness) = indexed_harness().await;

        let result = harness
            .tool(
                "search_text",
                serde_json::json!({"pattern": "build_widget"}),
            )
            .await;

        let matches = result["structuredContent"]["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2, "{matches:?}");
        for m in matches {
            assert!(m["path"].as_str().unwrap().ends_with(".rs"));
            assert!(m["line"].as_u64().unwrap() >= 1);
            assert!(m["text"].as_str().unwrap().contains("build_widget"));
        }

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn find_files_fuzzy_matches_a_path_fragment() {
        let (_project, _config, harness) = indexed_harness().await;

        let result = harness
            .tool("find_files", serde_json::json!({"query": "beta"}))
            .await;

        let files = result["structuredContent"]["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["relative"], "beta.rs");

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn find_definitions_and_find_usages_agree_about_a_symbol() {
        let (_project, _config, harness) = indexed_harness().await;

        let defs = harness
            .tool(
                "find_definitions",
                serde_json::json!({"query": "build_widget"}),
            )
            .await;
        let defs = defs["structuredContent"]["symbols"].as_array().unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs[0]["path"].as_str().unwrap().ends_with("alpha.rs"));
        assert_eq!(defs[0]["is_definition"], true);

        let usages = harness
            .tool("find_usages", serde_json::json!({"name": "build_widget"}))
            .await;
        let usages = usages["structuredContent"]["symbols"].as_array().unwrap();
        // Definition plus the one call site.
        assert_eq!(usages.len(), 2);

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn search_text_without_a_project_reports_why_not() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let handle = start(dir.path(), tx, empty_index(), 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        let result = harness
            .tool("search_text", serde_json::json!({"pattern": "anything"}))
            .await;

        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "No project is open yet.");

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn resolve_declaration_prefers_the_unsaved_buffer_over_the_file() {
        let config = tempfile::tempdir().unwrap();
        let (project, index) = indexed_fixture();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = start(config.path(), tx, index, 0).await.unwrap();
        let harness = Harness {
            handle,
            client: reqwest::Client::new(),
        };

        // The editor holds a version of beta.rs that defines its own local
        // `build_widget`; disk does not. Resolving must follow the buffer.
        let buffer = "fn build_widget() {}\nfn main() {\n    build_widget();\n}\n";
        tokio::spawn(async move {
            if let Some(EditorCommand::BufferContentForPath { respond, .. }) = rx.recv().await {
                let _ = respond.send(Some(buffer.to_string()));
            }
        });

        let offset = buffer.rfind("build_widget").unwrap();
        let result = harness
            .tool(
                "resolve_declaration",
                serde_json::json!({
                    "path": project.path().join("beta.rs").to_string_lossy(),
                    "byte_offset": offset
                }),
            )
            .await;

        let resolution = &result["structuredContent"];
        assert_eq!(resolution["name"], "build_widget");
        assert_eq!(resolution["tier"], "local_file");
        assert_eq!(resolution["candidates"][0]["line"], 1);

        harness.handle.shutdown().await;
    }

    #[tokio::test]
    async fn replace_in_files_rewrites_the_project_and_reindexes() {
        let (project, _config, harness) = indexed_harness().await;

        let result = harness
            .tool(
                "replace_in_files",
                serde_json::json!({"pattern": "Widget", "replacement": "Gadget", "case_sensitive": true}),
            )
            .await;

        assert_eq!(result["isError"], false);
        let report = &result["structuredContent"];
        assert_eq!(report["files"], 1);
        assert_eq!(report["skipped_files"], 0);

        let rewritten = std::fs::read_to_string(project.path().join("alpha.rs")).unwrap();
        assert!(rewritten.contains("struct Gadget;"));
        assert!(!rewritten.contains("Widget"));

        // The index was updated in the same pass, so a follow-up search
        // sees the new name and not the old one.
        let after = harness
            .tool("search_text", serde_json::json!({"pattern": "Gadget"}))
            .await;
        assert!(!after["structuredContent"]["matches"]
            .as_array()
            .unwrap()
            .is_empty());

        harness.handle.shutdown().await;
    }
}
