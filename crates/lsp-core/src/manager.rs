//! Server lifecycle, request routing, restart policy and document-version
//! tracking — the rules of talking to a language server.
//!
//! Blocking threads only, no async runtime (plan decision 9): one child
//! process per language, one writer behind a mutex, one supervisor thread per
//! server that both reads that server's stdout and owns its restart loop.
//! Everything the UI needs to see arrives on a single `Receiver<LspEvent>`.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::apply_edit::{
    await_verdict, ApplyEditGate, RefactorSession, RefactorSessions, UNSOLICITED_REASON,
};
use crate::catalog::ServerConfig;
use crate::code_lens;
use crate::completion::{parse_resolve_provider, parse_trigger_characters};
use crate::configuration;
use crate::framing::{read_message, write_message};
use crate::progress::{ProgressTracker, ServerActivity};
use crate::registration::{Registration, Registrations};
use crate::semantic_tokens::{self, SemanticTokensLegend};
use crate::signature_help::{parse_signature_triggers, SignatureTriggers};
use crate::watched_files::{FileChangeKind, WatchedFiles};
use process_exec::host::ExecHost;

/// Windows path -> Linux path (if `host` is remote) -> `file://` URI.
///
/// The one direction every outbound message needs: `rootUri`, `didOpen`'s
/// `textDocument.uri`, a watched-file change's `uri`. `diagnostics_core`'s
/// `uri_from_path` stays exactly as it is (ADR-0046) — this only decides
/// *which* path string reaches it.
pub fn uri_for(host: &ExecHost, path: &str) -> String {
    if host.is_remote() {
        crate::diagnostics::uri_from_path(&host.to_remote(Path::new(path)))
    } else {
        crate::diagnostics::uri_from_path(path)
    }
}

/// `file://` URI -> Linux path (if `host` is remote) -> Windows path.
///
/// The reverse of [`uri_for`]: a definition target, a workspace-edit
/// document, a resource operation's path — anything a server hands back
/// that this crate returns to its own caller as a path rather than a URI.
pub fn path_for(host: &ExecHost, uri: &str) -> Option<String> {
    let raw = crate::diagnostics::path_from_uri(uri)?;
    if host.is_remote() {
        Some(host.to_local(&raw).to_string_lossy().into_owned())
    } else {
        Some(raw)
    }
}

/// How long a request waits for its response before it is cancelled.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Hover is speculative and mouse-driven: an answer that arrives seconds
/// after the dwell is not worth showing, so it gets its own short deadline.
pub const HOVER_TIMEOUT: Duration = Duration::from_secs(2);
/// Completion is asked for per keystroke and the popup is only useful while
/// the word it describes is still being typed, so it gets the shortest
/// deadline of all: a list that lands later is discarded anyway.
pub const COMPLETION_TIMEOUT: Duration = Duration::from_secs(3);
/// Refactoring — code actions, rename, and the commands they run. Generous
/// next to the others because a server may have to analyse a whole project
/// to answer, and an Extract on a large file legitimately takes seconds; the
/// user asked for this one and is waiting on it, unlike a hover they may
/// already have moved past.
pub const REFACTOR_TIMEOUT: Duration = Duration::from_secs(30);
/// Go to Definition is an explicit gesture and the user is waiting for it,
/// but a jump that lands half a minute later is a bug, not a jump.
pub const DEFINITION_TIMEOUT: Duration = Duration::from_secs(5);
/// C12: `csharp/metadata` decompiles or reconstructs source for a framework
/// symbol server-side — real work, and the user is one navigation gesture
/// past [`DEFINITION_TIMEOUT`] already having decided to wait, so this is
/// closer to [`FORMATTING_TIMEOUT`] than to a caret-driven request.
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(15);

/// Reformatting a whole file is real work — rustfmt on a large file, or a
/// formatter that shells out — so this is generous compared with hover or
/// completion. It is still bounded: an editor that hangs on Ctrl+Alt+L is
/// worse than one that says the formatter took too long.
pub const FORMATTING_TIMEOUT: Duration = Duration::from_secs(15);

/// JSON-RPC's "method not found". A server answers this for a request it
/// does not implement, which is how an unsupported capability is discovered
/// without having read every field of its `initialize` result.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
/// Signature help is retriggered on `(` and every `,` while an argument
/// list is being typed, so it is as speculative as hover and gets the same
/// deadline: a tip for an argument the user has finished typing is noise.
pub const SIGNATURE_HELP_TIMEOUT: Duration = Duration::from_secs(2);
/// Document highlights fire on every caret move and are purely decorative.
/// One second is deliberately the shortest deadline in this file: an answer
/// slower than that describes a caret position the user has already left,
/// and painting it would highlight the wrong word.
pub const DOCUMENT_HIGHLIGHT_TIMEOUT: Duration = Duration::from_secs(1);
/// Inlay hints cost the server a real inference pass over the viewport, and
/// unlike the caret-driven requests their answer does not go stale — a hint
/// is anchored to a line, so it is still correct when it lands late. Hence
/// longer than hover, and still far short of a refactoring.
pub const INLAY_HINT_TIMEOUT: Duration = Duration::from_secs(5);
/// Alt+Enter, which opens a popup that is not drawn until the list arrives.
/// The user is waiting with nothing on screen, so this cannot be
/// [`REFACTOR_TIMEOUT`]; *applying* the action they then choose still is,
/// because by then they have committed to waiting.
pub const INTENTION_TIMEOUT: Duration = Duration::from_secs(5);
/// C9: `textDocument/semanticTokens/full` re-analyses the whole open
/// document, not a viewport or a keystroke, so this is generous like
/// [`FORMATTING_TIMEOUT`] rather than short like hover or completion — but
/// it is still fired in the background on document open, never blocking
/// typing, so a slow answer costs nothing but a delayed repaint.
pub const SEMANTIC_TOKENS_TIMEOUT: Duration = Duration::from_secs(10);
/// C10: `textDocument/codeLens` re-analyses the whole open document, same as
/// semantic tokens, and is likewise fired in the background on document
/// open rather than blocking typing — so it gets the same generous, non-UI-
/// blocking deadline as [`SEMANTIC_TOKENS_TIMEOUT`].
pub const CODE_LENS_TIMEOUT: Duration = Duration::from_secs(10);
/// C11: call hierarchy and type hierarchy each re-analyse the caret's whole
/// call/type graph, not a keystroke — an explicit gesture (Ctrl+Alt+H and
/// friends) the user is waiting on, so this matches [`DEFINITION_TIMEOUT`]
/// rather than the background-refresh features above.
pub const HIERARCHY_TIMEOUT: Duration = Duration::from_secs(5);
/// Delay before the first respawn attempt; doubles per consecutive failure.
const RESTART_BACKOFF_INITIAL: Duration = Duration::from_millis(200);
/// Ceiling for the exponential backoff.
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(10);
/// Consecutive crashes tolerated before the server is given up on. A server
/// that dies this often is broken, and respawning forever would just burn CPU.
const MAX_RESTARTS: u32 = 5;
/// How long a stopping server is given to exit on its own before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// A session that lasted at least this long counts as healthy, so its exit
/// resets the restart budget and the backoff.
const HEALTHY_SESSION: Duration = Duration::from_secs(30);

/// Something the UI wants to know about. Delivered in arrival order on the
/// manager's single event channel; a dropped receiver silently discards.
#[derive(Debug, Clone)]
pub enum LspEvent {
    /// The server finished `initialize`/`initialized` and accepts requests.
    /// `restarts` is 0 for the first launch and counts respawns after that.
    ServerReady {
        language_id: String,
        restarts: u32,
        /// The characters this server wants completion requested after, from
        /// its `initialize` result — `.` in most languages, `:` in Rust.
        trigger_characters: Vec<String>,
        /// F2-9: whether this server offers signature help at all, and which
        /// characters trigger and retrigger it — from the same `initialize`
        /// result, read once for the same reason `trigger_characters` is.
        signature_triggers: SignatureTriggers,
        /// C7: whether this server offers `completionItem/resolve`
        /// (`completionProvider.resolveProvider`), from the same
        /// `initialize` result — kept so an accept or a preview knows
        /// whether the second round trip is worth making at all, rather
        /// than sending it to every server and reading `MethodNotFound`
        /// back one keystroke at a time.
        completion_resolve_supported: bool,
    },
    /// The server's stdout hit EOF or errored, i.e. it died. A respawn follows
    /// after `retry_in` unless the restart budget is used up.
    ServerExited {
        language_id: String,
        restarts: u32,
        retry_in: Duration,
    },
    /// The server could not be launched or handshaken, or crashed past its
    /// restart budget. No further events will arrive for this language.
    ServerFailed {
        language_id: String,
        message: String,
    },
    /// F0-16: what the server is working on, from its `$/progress`
    /// notifications. `activity` is `None` when it has no work open, i.e.
    /// it is idle and its answers can be trusted.
    ///
    /// Emitted only when the visible activity changes, and only by servers
    /// that report progress at all. A server that never sends `$/progress`
    /// never sends this event and is idle from [`LspEvent::ServerReady`]
    /// onwards — nothing waits on it, the state is advisory.
    ServerBusy {
        language_id: String,
        activity: Option<ServerActivity>,
    },
    /// `textDocument/publishDiagnostics`.
    Diagnostics {
        language_id: String,
        uri: String,
        /// The document version the server diagnosed, when it reports one.
        version: Option<i32>,
        diagnostics: Vec<lsp_types::Diagnostic>,
    },
    /// The server is asking the editor to apply a `WorkspaceEdit`
    /// (`workspace/applyEdit`) — the shape command-driven refactorings take.
    ///
    /// The server is blocked until `gate` is answered, so the receiver must
    /// claim or refuse it rather than dropping it on the floor. `edit` is the
    /// raw `WorkspaceEdit`, parsed by `crate::workspace_edit` on the UI side
    /// where the set of open documents is known.
    ApplyEdit {
        language_id: String,
        /// What the server calls this change, for the preview's title.
        label: Option<String>,
        edit: Value,
        gate: ApplyEditGate,
    },
    /// Any other server-to-client notification, unparsed.
    Notification {
        language_id: String,
        method: String,
        params: Value,
    },
}

/// Why an LSP operation failed.
#[derive(Debug)]
pub enum LspError {
    /// No server is configured/started for that language id.
    NoServer(String),
    /// The server is not currently connected (crashed, or still restarting).
    NotRunning(String),
    /// Spawning the child process failed — usually a missing executable.
    Spawn { command: String, source: io::Error },
    /// Writing to or reading from the server's pipes failed.
    Io(io::Error),
    /// The server answered with a JSON-RPC error.
    Response { code: i64, message: String },
    /// No response within the timeout; a `$/cancelRequest` was sent.
    Timeout { method: String },
    /// The server died while the request was in flight.
    Disconnected { method: String },
    /// The server's payload was not the shape the protocol requires.
    Protocol(String),
}

impl LspError {
    pub const CODE_NO_SERVER: i32 = 600;
    pub const CODE_NOT_RUNNING: i32 = 601;
    pub const CODE_SPAWN: i32 = 602;
    pub const CODE_IO: i32 = 603;
    pub const CODE_RESPONSE: i32 = 604;
    pub const CODE_TIMEOUT: i32 = 605;
    pub const CODE_DISCONNECTED: i32 = 606;
    pub const CODE_PROTOCOL: i32 = 607;

    /// The variant's stable numeric code (ADR-0003 §4: 600–699 is
    /// `lsp-core`'s range, shared with [`crate::workspace_edit::EditError`]).
    /// Append-only.
    ///
    /// Note that [`LspError::Response`] carries the *server's* JSON-RPC
    /// code, which is a different numbering entirely and stays in the
    /// message; this code says only "the server answered with an error".
    pub fn code(&self) -> i32 {
        match self {
            LspError::NoServer(_) => Self::CODE_NO_SERVER,
            LspError::NotRunning(_) => Self::CODE_NOT_RUNNING,
            LspError::Spawn { .. } => Self::CODE_SPAWN,
            LspError::Io(_) => Self::CODE_IO,
            LspError::Response { .. } => Self::CODE_RESPONSE,
            LspError::Timeout { .. } => Self::CODE_TIMEOUT,
            LspError::Disconnected { .. } => Self::CODE_DISCONNECTED,
            LspError::Protocol(_) => Self::CODE_PROTOCOL,
        }
    }
}

impl fmt::Display for LspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LspError::NoServer(lang) => write!(f, "no language server configured for {lang}"),
            LspError::NotRunning(lang) => write!(f, "the {lang} language server is not running"),
            LspError::Spawn { command, source } => {
                write!(f, "could not start {command}: {source}")
            }
            LspError::Io(e) => write!(f, "language server I/O failed: {e}"),
            LspError::Response { code, message } => {
                write!(f, "language server error {code}: {message}")
            }
            LspError::Timeout { method } => write!(f, "{method} timed out"),
            LspError::Disconnected { method } => {
                write!(f, "the language server exited during {method}")
            }
            LspError::Protocol(what) => write!(f, "malformed language server message: {what}"),
        }
    }
}

impl std::error::Error for LspError {}

impl From<io::Error> for LspError {
    fn from(e: io::Error) -> Self {
        LspError::Io(e)
    }
}

/// The live child process: its stdin (the one writer) and the handle we reap.
struct Conn {
    stdin: ChildStdin,
    child: Child,
}

/// One language's server: its config, its connection, and the requests
/// currently awaiting a response.
struct Server {
    language_id: String,
    conn: Mutex<Option<Conn>>,
    pending: Mutex<HashMap<i64, Sender<Result<Value, LspError>>>>,
    next_id: AtomicI64,
    stopping: AtomicBool,
    /// F0-16: the `$/progress` work this server currently has open. Owned
    /// per server because a token is only unique within one server, and
    /// touched only from that server's reader thread and its supervisor.
    progress: Mutex<ProgressTracker>,
    /// Whether the editor currently has a refactoring in flight — read by
    /// `dispatch` to decide whether an inbound `workspace/applyEdit` was
    /// asked for. Shared with the manager, not owned here.
    sessions: Arc<RefactorSessions>,
    /// C4: what this server has asked us to watch for it via
    /// `client/registerCapability`, keyed by registration id like the
    /// protocol keys it. Per server, like `progress`, because a
    /// registration id is only unique within one server's session.
    registrations: Registrations,
    /// C6: the `workspace/configuration` section this server pulls its
    /// settings from, from the `ServerConfig` it was launched with. Fixed
    /// for the server's lifetime — changing it means relaunching with a
    /// different config, same as `command`/`args`.
    settings_section: Option<String>,
    /// C6: the settings blob answered for `settings_section`, mutable via
    /// [`LspManager::update_settings`] without a relaunch.
    settings: Mutex<Value>,
    /// C9: this server's semantic-tokens legend, if it offers the request at
    /// all — set from `initialize`'s static `semanticTokensProvider`
    /// capability at connect time, or from a dynamic
    /// `client/registerCapability` registration if one arrives later
    /// (csharp-ls's path; see `dispatch`'s `client/registerCapability` arm).
    /// `None` means "not known to support it yet", checked generically by
    /// [`LspManager::semantic_tokens_legend`] rather than gating on one path
    /// or the other.
    semantic_tokens_legend: Mutex<Option<SemanticTokensLegend>>,
    /// C10: whether this server statically advertised `codeLensProvider` in
    /// its `initialize` result. The dynamic half of the same dual path —
    /// a `client/registerCapability` for `textDocument/codeLens` — needs no
    /// mirror of this field: unlike a semantic-tokens legend, a
    /// `CodeLensOptions` carries nothing this client reads back out, so
    /// `LspManager::code_lenses_supported` checks `registrations` directly
    /// (see `code_lens::is_offered`'s own doc comment).
    code_lens_supported: Mutex<bool>,
    /// C11: whether this server statically advertised `callHierarchyProvider`
    /// in its `initialize` result. Same dual-path convention as
    /// `code_lens_supported` — the dynamic half needs no mirror field here
    /// either, since a `CallHierarchyOptions` carries nothing this client
    /// reads back out; `LspManager::call_hierarchy_supported` checks
    /// `registrations` directly for that half.
    call_hierarchy_supported: Mutex<bool>,
    /// C11: the type-hierarchy twin of `call_hierarchy_supported`, for
    /// `typeHierarchyProvider`.
    type_hierarchy_supported: Mutex<bool>,
}

impl Server {
    fn send(&self, message: &Value) -> Result<(), LspError> {
        let payload = serde_json::to_vec(message).map_err(io::Error::from)?;
        let mut guard = self.conn.lock().unwrap();
        let conn = guard
            .as_mut()
            .ok_or_else(|| LspError::NotRunning(self.language_id.clone()))?;
        write_message(&mut conn.stdin, &payload)?;
        Ok(())
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, LspError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);

        let sent = self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }));
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }

        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                // Best-effort: the server may already be gone.
                let _ = self.notify("$/cancelRequest", json!({ "id": id }));
                Err(LspError::Timeout {
                    method: method.to_string(),
                })
            }
            // The sender is dropped when the connection dies.
            Err(RecvTimeoutError::Disconnected) => Err(LspError::Disconnected {
                method: method.to_string(),
            }),
        }
    }

    /// Fail every in-flight request; called when the connection dies so no
    /// caller waits for a response that can never arrive.
    fn drop_pending(&self) {
        self.pending.lock().unwrap().clear();
    }
}

/// What the manager knows about one open document.
struct DocState {
    language_id: String,
    version: i32,
}

/// Owns the running language servers and the rules around them.
///
/// Deliberately not a `bridge.rs` concern: lifecycle, restart backoff, request
/// correlation and document versions are rules, and the adapter is allowed
/// none (`docs/architecture/layering.md`).
pub struct LspManager {
    /// Workspace root as a `file://` URI, sent in `initialize` — already
    /// translated through `host` if the root is a WSL UNC path.
    root_uri: String,
    /// The Windows-side workspace root path this manager was constructed
    /// with, unmodified. Only [`Self::start`]'s subprocess spawn needs a
    /// filesystem `current_dir`; everything else works from `root_uri`.
    root_path: String,
    /// Where this project's tooling runs — derived once, at construction,
    /// from `root_path` (ADR-0052). Never recomputed: a project's host does
    /// not change without a new `LspManager`.
    host: ExecHost,
    servers: Mutex<HashMap<String, Arc<Server>>>,
    supervisors: Mutex<HashMap<String, JoinHandle<()>>>,
    documents: Mutex<HashMap<String, DocState>>,
    events: Sender<LspEvent>,
    /// Refactorings the editor has in flight, which is what makes an inbound
    /// `workspace/applyEdit` legitimate (`crate::apply_edit`).
    sessions: Arc<RefactorSessions>,
}

impl LspManager {
    /// Create a manager for a workspace root, plus the channel every event is
    /// delivered on. The caller owns the receiver — typically a listener
    /// thread that forwards onto the UI thread.
    ///
    /// Takes the same naive `file://` URI callers already built with
    /// `uri_from_path` before this plan — no caller needs to change. The
    /// Windows path is recovered from it once, here, to derive [`ExecHost`]
    /// (ADR-0052) and re-translate `root_uri` if the root is a WSL UNC path;
    /// every other ingest/egress site in this crate normalizes off that one
    /// `host`.
    pub fn new(root_uri: impl Into<String>) -> (Self, Receiver<LspEvent>) {
        let naive_uri = root_uri.into();
        let root_path =
            crate::diagnostics::path_from_uri(&naive_uri).unwrap_or_else(|| naive_uri.clone());
        let host = ExecHost::for_path(Path::new(&root_path));
        let (events, rx) = channel();
        let manager = LspManager {
            root_uri: uri_for(&host, &root_path),
            root_path,
            host,
            servers: Mutex::new(HashMap::new()),
            supervisors: Mutex::new(HashMap::new()),
            documents: Mutex::new(HashMap::new()),
            events,
            sessions: Arc::new(RefactorSessions::default()),
        };
        (manager, rx)
    }

    /// Recover a caller-built naive URI (from the *pre-translation*
    /// `uri_from_path` this crate's own callers still use, ADR-0052's
    /// "ui-shell's URI users are untouched: translation happened at
    /// ingest") back to the Windows path it encoded, then translate it
    /// through this manager's `host`. A no-op on `ExecHost::Local`.
    pub(crate) fn normalize_uri(&self, uri: &str) -> String {
        if !self.host.is_remote() {
            return uri.to_string();
        }
        match crate::diagnostics::path_from_uri(uri) {
            Some(windows_path) => uri_for(&self.host, &windows_path),
            None => uri.to_string(),
        }
    }

    /// Launch a server and complete its `initialize`/`initialized` handshake.
    ///
    /// Blocks until the server is ready, so a bad command surfaces here as an
    /// error rather than as a silent no-op later. Starting a language that is
    /// already running is a no-op.
    pub fn start(&self, cfg: &ServerConfig) -> Result<(), LspError> {
        if self.servers.lock().unwrap().contains_key(&cfg.language_id) {
            return Ok(());
        }
        let server = Arc::new(Server {
            language_id: cfg.language_id.clone(),
            conn: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            stopping: AtomicBool::new(false),
            progress: Mutex::new(ProgressTracker::default()),
            sessions: Arc::clone(&self.sessions),
            registrations: Registrations::default(),
            settings_section: cfg.settings_section.clone(),
            settings: Mutex::new(cfg.settings.clone()),
            semantic_tokens_legend: Mutex::new(None),
            code_lens_supported: Mutex::new(false),
            call_hierarchy_supported: Mutex::new(false),
            type_hierarchy_supported: Mutex::new(false),
        });

        let (ready_tx, ready_rx) = channel();
        let handle = spawn_supervisor(
            Arc::clone(&server),
            cfg.clone(),
            self.root_uri.clone(),
            self.root_path.clone(),
            self.host.clone(),
            self.events.clone(),
            ready_tx,
        );

        // The handshake runs in the supervisor thread (it must own the reader);
        // the first attempt's outcome comes back here.
        match ready_rx.recv() {
            Ok(Ok(())) => {
                self.servers
                    .lock()
                    .unwrap()
                    .insert(cfg.language_id.clone(), server);
                self.supervisors
                    .lock()
                    .unwrap()
                    .insert(cfg.language_id.clone(), handle);
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err(LspError::NotRunning(cfg.language_id.clone()))
            }
        }
    }

    /// Is a server currently connected for this language?
    pub fn is_running(&self, language_id: &str) -> bool {
        self.server(language_id)
            .map(|s| s.conn.lock().unwrap().is_some())
            .unwrap_or(false)
    }

    /// Send a request and wait for its response ([`DEFAULT_REQUEST_TIMEOUT`]).
    pub fn request(
        &self,
        language_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, LspError> {
        self.request_with_timeout(language_id, method, params, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Send a request, cancelling it with `$/cancelRequest` if `timeout`
    /// elapses first.
    pub fn request_with_timeout(
        &self,
        language_id: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let server = self
            .server(language_id)
            .ok_or_else(|| LspError::NoServer(language_id.to_string()))?;
        server.request(method, params, timeout)
    }

    /// Send a notification (fire and forget by protocol definition).
    pub fn notify(&self, language_id: &str, method: &str, params: Value) -> Result<(), LspError> {
        let server = self
            .server(language_id)
            .ok_or_else(|| LspError::NoServer(language_id.to_string()))?;
        server.notify(method, params)
    }

    /// Tell the server a document is open. The manager owns the version
    /// counter: versions start at 1 and only ever increase, per document.
    ///
    /// `uri` is the naive `file://` URI a caller already built with
    /// `uri_from_path` — [`Self::normalize_uri`] retranslates it through
    /// this manager's host before it ever reaches the wire, or a lookup key.
    pub fn did_open(&self, uri: &str, language_id: &str, text: &str) -> Result<(), LspError> {
        let uri = self.normalize_uri(uri);
        self.documents.lock().unwrap().insert(
            uri.clone(),
            DocState {
                language_id: language_id.to_string(),
                version: 1,
            },
        );
        self.notify(
            language_id,
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri, "languageId": language_id, "version": 1, "text": text
            }}),
        )
    }

    /// Tell the server a document changed, as a full-text sync. Returns the
    /// new version.
    pub fn did_change(&self, uri: &str, text: &str) -> Result<i32, LspError> {
        let uri = self.normalize_uri(uri);
        let (language_id, version) = {
            let mut docs = self.documents.lock().unwrap();
            let doc = docs
                .get_mut(&uri)
                .ok_or_else(|| LspError::Protocol(format!("{uri} was never opened")))?;
            doc.version += 1;
            (doc.language_id.clone(), doc.version)
        };
        self.notify(
            &language_id,
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        )?;
        Ok(version)
    }

    /// Tell the server a document was saved.
    pub fn did_save(&self, uri: &str) -> Result<(), LspError> {
        let uri = self.normalize_uri(uri);
        let language_id = self.language_of(&uri)?;
        self.notify(
            &language_id,
            "textDocument/didSave",
            json!({"textDocument": {"uri": uri}}),
        )
    }

    /// Tell the server a document is closed and forget its version.
    pub fn did_close(&self, uri: &str) -> Result<(), LspError> {
        let uri = self.normalize_uri(uri);
        let language_id = self.language_of(&uri)?;
        self.documents.lock().unwrap().remove(&uri);
        self.notify(
            &language_id,
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        )
    }

    /// Tell a server about filesystem changes it asked to watch
    /// (`client/registerCapability` → `workspace/didChangeWatchedFiles`,
    /// C4/C5), off `project_model::ProjectWatcher` rather than a second
    /// watcher of this client's own. `changes` is filtered down to what
    /// this server's current registrations actually cover and sent as one
    /// batched notification — LSP's `changes` param is already an array,
    /// so this is one notification per call, not one per file. A server
    /// with no matching registration, or no server running for
    /// `language_id` at all, gets nothing: this never wakes a server that
    /// asked for no watches.
    pub fn did_change_watched_files(
        &self,
        language_id: &str,
        changes: &[(PathBuf, FileChangeKind)],
    ) -> Result<(), LspError> {
        let Some(server) = self.server(language_id) else {
            return Ok(());
        };
        // Registration is rare (once per server session, typically), so
        // recompiling on every call — rather than caching the compiled
        // `GlobSet` on `Server` and invalidating it on register/unregister
        // — is the simpler correct choice here.
        let watched = WatchedFiles::compile(&server.registrations.watchers());
        if watched.is_empty() {
            return Ok(());
        }
        let interesting: Vec<Value> = changes
            .iter()
            .filter(|(path, kind)| watched.interested(path, *kind))
            .map(|(path, kind)| {
                json!({
                    "uri": uri_for(&self.host, &path.to_string_lossy()),
                    "type": *kind as u8,
                })
            })
            .collect();
        if interesting.is_empty() {
            return Ok(());
        }
        server.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes": interesting}),
        )
    }

    /// Whether this server offers code lenses at all — from `initialize`'s
    /// static `codeLensProvider` capability, or a dynamic
    /// `client/registerCapability` registration for `textDocument/codeLens`
    /// (csharp-ls's suspected path, the same dual path C9 checks for
    /// semantic tokens). `false` for a server that is not running.
    pub fn code_lenses_supported(&self, language_id: &str) -> bool {
        self.server(language_id).is_some_and(|server| {
            *server.code_lens_supported.lock().unwrap()
                || server
                    .registrations
                    .method_registered("textDocument/codeLens")
        })
    }

    /// Whether this server offers call hierarchy at all — from `initialize`'s
    /// static `callHierarchyProvider` capability, or a dynamic
    /// `client/registerCapability` registration for
    /// `textDocument/prepareCallHierarchy`. `false` for a server that is not
    /// running.
    pub fn call_hierarchy_supported(&self, language_id: &str) -> bool {
        self.server(language_id).is_some_and(|server| {
            *server.call_hierarchy_supported.lock().unwrap()
                || server
                    .registrations
                    .method_registered("textDocument/prepareCallHierarchy")
        })
    }

    /// Whether this server offers type hierarchy at all — from
    /// `initialize`'s static `typeHierarchyProvider` capability, or a
    /// dynamic `client/registerCapability` registration for
    /// `textDocument/prepareTypeHierarchy`. `false` for a server that is not
    /// running.
    pub fn type_hierarchy_supported(&self, language_id: &str) -> bool {
        self.server(language_id).is_some_and(|server| {
            *server.type_hierarchy_supported.lock().unwrap()
                || server
                    .registrations
                    .method_registered("textDocument/prepareTypeHierarchy")
        })
    }

    /// This server's semantic-tokens legend, if it has told us about one —
    /// via `initialize`'s static capability (set at connect time, `connect`
    /// below) or a dynamic `client/registerCapability` registration
    /// (`dispatch`'s `client/registerCapability` arm) — whichever arrived.
    /// `None` for a server that has done neither yet, which is this
    /// method's answer to "is `semantic_tokens` worth calling right now" —
    /// checked generically across both paths rather than the caller having
    /// to know which one a given server uses (C9's plan explicitly calls
    /// out that csharp-ls's path is unconfirmed, so both are handled).
    pub fn semantic_tokens_legend(&self, language_id: &str) -> Option<SemanticTokensLegend> {
        self.server(language_id)?
            .semantic_tokens_legend
            .lock()
            .unwrap()
            .clone()
    }

    /// The version last sent for a document, if it is open.
    pub fn document_version(&self, uri: &str) -> Option<i32> {
        let uri = self.normalize_uri(uri);
        self.documents.lock().unwrap().get(&uri).map(|d| d.version)
    }

    /// Whether the running server has dynamically registered `method` via
    /// `client/registerCapability`. `false` for a server that is not
    /// running at all, same as "it never registered anything".
    pub fn method_registered(&self, language_id: &str, method: &str) -> bool {
        self.server(language_id)
            .is_some_and(|server| server.registrations.method_registered(method))
    }

    /// C6: update the settings a running server pulls via
    /// `workspace/configuration` and tell it to re-pull them.
    ///
    /// The notification's `settings` is deliberately `null`, not `settings`
    /// itself — that is what tells a client-supports-pull server (csharp-ls
    /// included) to re-issue `workspace/configuration` rather than treat the
    /// notification as the new value pushed inline.
    pub fn update_settings(&self, language_id: &str, settings: Value) -> Result<(), LspError> {
        let server = self
            .server(language_id)
            .ok_or_else(|| LspError::NoServer(language_id.to_string()))?;
        *server.settings.lock().unwrap() = settings;
        server.notify(
            "workspace/didChangeConfiguration",
            json!({"settings": Value::Null}),
        )
    }

    /// Shut one server down: `shutdown`, `exit`, then kill if it lingers.
    pub fn stop(&self, language_id: &str) {
        let server = self.servers.lock().unwrap().remove(language_id);
        let handle = self.supervisors.lock().unwrap().remove(language_id);
        let Some(server) = server else { return };

        server.stopping.store(true, Ordering::SeqCst);
        let _ = server.request("shutdown", Value::Null, SHUTDOWN_GRACE);
        let _ = server.notify("exit", Value::Null);

        // Give it the grace period to close its stdout on its own; the
        // supervisor takes the connection when it sees EOF.
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            if server.conn.lock().unwrap().is_none() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        if let Some(conn) = server.conn.lock().unwrap().as_mut() {
            let _ = conn.child.kill();
        }
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// Shut every running server down.
    pub fn stop_all(&self) {
        let languages: Vec<String> = self.servers.lock().unwrap().keys().cloned().collect();
        for language_id in languages {
            self.stop(&language_id);
        }
    }

    fn server(&self, language_id: &str) -> Option<Arc<Server>> {
        self.servers.lock().unwrap().get(language_id).cloned()
    }

    /// Mark a refactoring as in flight for as long as the returned guard
    /// lives.
    ///
    /// This is what makes an inbound `workspace/applyEdit` legitimate: a
    /// server may only rewrite the user's files while the user is asking it
    /// to. Callers hold the guard across the whole gesture — including the
    /// `workspace/executeCommand` that provokes the edit — and dropping it
    /// closes the door again.
    pub fn begin_refactor(&self) -> RefactorSession {
        self.sessions.begin()
    }

    /// Whether a refactoring this client started is in flight right now.
    pub fn refactor_active(&self) -> bool {
        self.sessions.active()
    }

    /// Where this project's tooling runs (ADR-0052) — `ExecHost::Local`
    /// unless the workspace root is a WSL UNC path. Exposed so a feature
    /// module (`rename`, and `LspManager::parse_workspace_changes` below)
    /// can retranslate a server's response paths without duplicating
    /// [`ExecHost::for_path`]'s classification.
    pub fn host(&self) -> &ExecHost {
        &self.host
    }

    /// [`crate::workspace_edit::parse_workspace_changes`], with every
    /// step's path retranslated through this manager's host (W3-2) — the
    /// method callers use instead of the free function, so a WSL project's
    /// `workspace/applyEdit` and refactor-preview paths are the UNC path
    /// `ui-shell` opened, not the Linux path the server actually sent.
    pub fn parse_workspace_changes(
        &self,
        value: &Value,
    ) -> Result<crate::workspace_edit::WorkspaceChanges, crate::workspace_edit::EditError> {
        let mut changes = crate::workspace_edit::parse_workspace_changes(value)?;
        changes.retranslate_paths(&self.host);
        Ok(changes)
    }

    pub(crate) fn language_of(&self, uri: &str) -> Result<String, LspError> {
        self.documents
            .lock()
            .unwrap()
            .get(uri)
            .map(|d| d.language_id.clone())
            .ok_or_else(|| LspError::Protocol(format!("{uri} was never opened")))
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// The per-server supervisor: spawn, handshake, read until EOF, respawn with
/// capped exponential backoff.
fn spawn_supervisor(
    server: Arc<Server>,
    cfg: ServerConfig,
    root_uri: String,
    root_path: String,
    host: ExecHost,
    events: Sender<LspEvent>,
    ready: Sender<Result<(), LspError>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut restarts: u32 = 0;
        let mut ready = Some(ready);
        let mut backoff = RESTART_BACKOFF_INITIAL;

        loop {
            match connect(&server, &cfg, &root_uri, &root_path, &host) {
                Ok((
                    stdout,
                    trigger_characters,
                    signature_triggers,
                    completion_resolve_supported,
                )) => {
                    if let Some(tx) = ready.take() {
                        let _ = tx.send(Ok(()));
                    }
                    let _ = events.send(LspEvent::ServerReady {
                        language_id: cfg.language_id.clone(),
                        restarts,
                        trigger_characters,
                        signature_triggers,
                        completion_resolve_supported,
                    });
                    let started = Instant::now();
                    read_loop(&server, &cfg.language_id, stdout, &events);
                    // A session that ran for a while was healthy: its exit
                    // starts a fresh restart budget rather than counting
                    // towards the crash loop the backoff exists to damp.
                    if started.elapsed() >= HEALTHY_SESSION {
                        restarts = 0;
                        backoff = RESTART_BACKOFF_INITIAL;
                    }
                }
                Err(e) => {
                    let message = e.to_string();
                    if let Some(tx) = ready.take() {
                        let _ = tx.send(Err(e));
                        return;
                    }
                    let _ = events.send(LspEvent::ServerFailed {
                        language_id: cfg.language_id.clone(),
                        message,
                    });
                    return;
                }
            }

            // The connection is gone: reap the child and release waiters.
            if let Some(mut conn) = server.conn.lock().unwrap().take() {
                let _ = conn.child.kill();
                let _ = conn.child.wait();
            }
            server.drop_pending();
            // Work a dead server left open can never end, and a status bar
            // stuck on its last percentage would outlive the server itself.
            if server.progress.lock().unwrap().clear() {
                let _ = events.send(LspEvent::ServerBusy {
                    language_id: cfg.language_id.clone(),
                    activity: None,
                });
            }

            if server.stopping.load(Ordering::SeqCst) {
                return;
            }
            restarts += 1;
            if restarts > MAX_RESTARTS {
                let _ = events.send(LspEvent::ServerFailed {
                    language_id: cfg.language_id.clone(),
                    message: format!("gave up after {MAX_RESTARTS} restarts"),
                });
                return;
            }
            let _ = events.send(LspEvent::ServerExited {
                language_id: cfg.language_id.clone(),
                restarts,
                retry_in: backoff,
            });
            thread::sleep(backoff);
            backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
        }
    })
}

/// Spawn the child and run the `initialize`/`initialized` handshake, leaving
/// the connection published and the reader positioned at the next message.
fn connect(
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
                    format!("{} not found inside the WSL distro", cfg.command),
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
    let init = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": client_capabilities(),
            "workspaceFolders": Value::Null,
        }
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

    *server.conn.lock().unwrap() = Some(Conn { stdin, child });
    Ok((
        stdout,
        trigger_characters,
        signature_triggers,
        completion_resolve_supported,
    ))
}

/// Read and dispatch until the server's stdout ends (i.e. it died).
fn read_loop(
    server: &Arc<Server>,
    language_id: &str,
    mut stdout: BufReader<std::process::ChildStdout>,
    events: &Sender<LspEvent>,
) {
    loop {
        match read_message(&mut stdout) {
            Ok(Some(body)) => match serde_json::from_slice::<Value>(&body) {
                Ok(message) => dispatch(server, language_id, message, events),
                // A single unparsable message is not worth killing the
                // session over; the framing is still in sync.
                Err(_) => continue,
            },
            Ok(None) | Err(_) => return,
        }
    }
}

fn dispatch(server: &Arc<Server>, language_id: &str, message: Value, events: &Sender<LspEvent>) {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").and_then(Value::as_i64);

    match (method, id) {
        // Response to one of our requests.
        (None, Some(id)) => {
            let Some(tx) = server.pending.lock().unwrap().remove(&id) else {
                return;
            };
            let result = match message.get("error") {
                Some(error) => Err(response_error(error)),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(result);
        }
        // `workspace/applyEdit` is the one server-to-client request we
        // implement, and the only one that cannot be answered here: applying
        // an edit needs the UI thread, and this is the thread that reads
        // every message from this server — blocking it would stall
        // diagnostics and every in-flight response behind one dialog.
        //
        // So the answer is made elsewhere and this arm only routes. An edit
        // nobody asked for is refused immediately, without a thread and
        // without troubling the UI: that is a server rewriting the user's
        // files unprompted. A wanted one is handed to a short-lived thread
        // that publishes it and waits on the gate, which is bounded — see
        // `crate::apply_edit`. There is at most one such thread per
        // refactoring gesture, because a gesture is what makes the request
        // legitimate in the first place.
        (Some("workspace/applyEdit"), Some(id)) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            if !server.sessions.active() {
                let _ = server.send(&apply_edit_response(id, false, Some(UNSOLICITED_REASON)));
                return;
            }
            let (gate, rx) = ApplyEditGate::new();
            let _ = events.send(LspEvent::ApplyEdit {
                language_id: language_id.to_string(),
                label: params
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                edit: params.get("edit").cloned().unwrap_or(Value::Null),
                gate: gate.clone(),
            });
            let server = Arc::clone(server);
            thread::spawn(move || {
                let verdict = await_verdict(rx, &gate);
                let _ = server.send(&apply_edit_response(
                    id,
                    verdict.applied(),
                    verdict.reason(),
                ));
            });
        }
        // F0-16: the server asking permission to open a progress token.
        // There is nothing to decide — the client advertised
        // `window.workDoneProgress`, so the answer is always yes, and the
        // token itself arrives with the `$/progress` that follows. Answered
        // here rather than falling through to "not implemented" below,
        // which is what would otherwise make a server stop reporting.
        (Some("window/workDoneProgress/create"), Some(id)) => {
            let _ = server.send(&json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}));
        }
        // C4: csharp-ls (and any server that leans on dynamic registration
        // rather than declaring capabilities in `initialize`) sends this
        // right after `initialized`. There is nothing to decide — this
        // client always accepts a registration, same reasoning as
        // `window/workDoneProgress/create` above — and no blocking work, so
        // it is answered inline here rather than dispatched elsewhere.
        // Malformed params (missing/wrong-shaped fields) are treated as an
        // empty registration list rather than crashing this reader thread;
        // refusing a registration the server would only retry is worse than
        // ignoring one this client could not parse.
        (Some("client/registerCapability"), Some(id)) => {
            let registrations = message
                .get("params")
                .and_then(|p| p.get("registrations"))
                .cloned()
                .and_then(|v| serde_json::from_value::<Vec<Registration>>(v).ok())
                .unwrap_or_default();
            // C9: csharp-ls's suspected path — a server that declares
            // `textDocument/semanticTokens` dynamically rather than in
            // `initialize`'s static result carries the same
            // `SemanticTokensOptions` shape in its `registerOptions`
            // (`semantic_tokens::parse_provider` reads either). The last
            // registration for the method wins, matching `Registrations::register`'s
            // own "replacing a reused id is the safer read" reasoning.
            if let Some(legend) = registrations
                .iter()
                .filter(|r| r.method == "textDocument/semanticTokens")
                .find_map(|r| semantic_tokens::parse_provider(&r.register_options))
            {
                *server.semantic_tokens_legend.lock().unwrap() = Some(legend);
            }
            server.registrations.register(registrations);
            let _ = server.send(&json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}));
        }
        // The spec really does spell this "unregisterations".
        (Some("client/unregisterCapability"), Some(id)) => {
            let ids: Vec<String> = message
                .get("params")
                .and_then(|p| p.get("unregisterations"))
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|e| e.get("id").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            server.registrations.unregister(&ids);
            let _ = server.send(&json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}));
        }
        // C6: csharp-ls pulls its settings rather than taking them pushed,
        // so it sends this right after `initialized` (and again after
        // `workspace/didChangeConfiguration`). A pure lookup against this
        // server's own configured section — nothing to decide, nothing to
        // block on — so it is answered inline here, same reasoning as
        // `window/workDoneProgress/create` and `client/registerCapability`
        // above. Answers are returned in request order, `null` for any
        // section this client has no opinion on (ADR-0016: single-root, so
        // `scopeUri` is ignorable).
        (Some("workspace/configuration"), Some(id)) => {
            let items = message
                .get("params")
                .and_then(|p| p.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let settings = server.settings.lock().unwrap().clone();
            let result: Vec<Value> = items
                .iter()
                .map(|item| {
                    let section = item.get("section").and_then(Value::as_str).unwrap_or("");
                    configuration::resolve(server.settings_section.as_deref(), &settings, section)
                })
                .collect();
            let _ = server.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
        }
        // Every other server-to-client request. The server blocks until it
        // gets an answer, so answer honestly rather than not at all.
        (Some(method), Some(id)) => {
            let _ = server.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("{method} is not implemented")},
            }));
        }
        // Notification.
        (Some(method), None) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let event = if method == "textDocument/publishDiagnostics" {
                publish_diagnostics(language_id, &params)
            } else if method == "$/progress" {
                // Handled on the reader thread like any other notification:
                // the tracker is a `Mutex` around a `Vec`, so this costs
                // nothing and adds no thread. `apply` says whether anything
                // visible changed, which is what keeps a server reporting
                // every percent from flooding the channel with no-ops.
                let mut progress = server.progress.lock().unwrap();
                if !progress.apply(&params) {
                    return;
                }
                Some(LspEvent::ServerBusy {
                    language_id: language_id.to_string(),
                    activity: progress.current(),
                })
            } else {
                None
            };
            let _ = events.send(event.unwrap_or(LspEvent::Notification {
                language_id: language_id.to_string(),
                method: method.to_string(),
                params,
            }));
        }
        (None, None) => {}
    }
}

/// The `ApplyWorkspaceEditResult` the protocol expects: `applied`, plus a
/// `failureReason` whenever it is false, so a server can tell the user why
/// its refactoring did not happen.
fn apply_edit_response(id: i64, applied: bool, reason: Option<&str>) -> Value {
    let mut result = json!({"applied": applied});
    if let Some(reason) = reason {
        result["failureReason"] = Value::String(reason.to_string());
    }
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn publish_diagnostics(language_id: &str, params: &Value) -> Option<LspEvent> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let diagnostics =
        serde_json::from_value::<Vec<lsp_types::Diagnostic>>(params.get("diagnostics")?.clone())
            .ok()?;
    Some(LspEvent::Diagnostics {
        language_id: language_id.to_string(),
        uri,
        version: params
            .get("version")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        diagnostics,
    })
}

/// `TextDocumentPositionParams`: `line` and `character` are 0-based and
/// counted in UTF-16 code units, per the encoding `initialize` negotiates.
pub(crate) fn position_params(uri: &str, line: u32, character: u32) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": {"line": line, "character": character},
    })
}

fn response_error(error: &Value) -> LspError {
    LspError::Response {
        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string(),
    }
}

/// What this client can do. Kept deliberately small — capabilities are added
/// by the feature tasks that implement them (L2-L5), not speculatively.
fn client_capabilities() -> Value {
    json!({
        "textDocument": {
            "synchronization": {"dynamicRegistration": false},
            "publishDiagnostics": {"relatedInformation": true},
            // L3/L4: advertised because they are implemented — `contentFormat`
            // lists markdown first because that is what the tooltip renders,
            // and `linkSupport` opts into the richer `LocationLink` reply.
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "definition": {"linkSupport": true},
            // L5: `snippetSupport: false` is the honest answer — snippet
            // items are inserted as their plain text, with no tabstops (see
            // `completion::strip_snippet`), so a server that would send
            // placeholder-heavy items is told to prefer plain ones.
            "completion": {
                "completionItem": {
                    "snippetSupport": false,
                    "documentationFormat": ["plaintext", "markdown"],
                    // C7: which fields are worth a `completionItem/resolve`
                    // round trip for — `additionalTextEdits` is the `using`
                    // csharp-ls adds for an unimported type; `documentation`
                    // and `detail` are the two fields most servers only
                    // fill in on resolve, to keep the initial list cheap.
                    "resolveSupport": {
                        "properties": ["documentation", "detail", "additionalTextEdits"],
                    },
                },
                "contextSupport": false,
            },
            // RF6: code actions as literals rather than bare commands, so an
            // action can carry its own edit; `resolveSupport` names `edit`
            // only, because that is the one field we ask a server to fill in
            // later. The kind list is the families the UI offers — servers
            // may answer with any kind, and `code_action::kind_matches`
            // classifies what arrives, so this list narrows requests without
            // limiting what can come back.
            "codeAction": {
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": [
                    "", "quickfix", "refactor", "refactor.extract",
                    "refactor.inline", "refactor.rewrite", "source",
                    // F2: Organize Imports is offered in its own right and
                    // as a quick fix for an unresolved symbol, so the kind
                    // is named rather than left to the `source` family.
                    "source.organizeImports",
                ]}},
                "resolveSupport": {"properties": ["edit"]},
                "dataSupport": true,
                "isPreferredSupport": true,
                "disabledSupport": true,
            },
            "rename": {"prepareSupport": true},
            // F1: advertised because reformat is implemented. `dynamicRegistration`
            // is false throughout this client — a server that wants to register
            // capabilities later has nowhere to send them.
            "formatting": {"dynamicRegistration": false},
            "rangeFormatting": {"dynamicRegistration": false},
            // F2: parameter hints. `labelOffsetSupport` says we prefer the
            // unambiguous `[start, end]` parameter label — a substring has
            // to be searched for in the signature and can match the wrong
            // occurrence — but both shapes are handled either way
            // (`signature_help::parse_signature_help`).
            // `activeParameterSupport` opts into the per-signature index,
            // which is the only way an overload set can say that *this*
            // overload takes fewer arguments.
            "signatureHelp": {
                "signatureInformation": {
                    "documentationFormat": ["plaintext", "markdown"],
                    "parameterInformation": {"labelOffsetSupport": true},
                    "activeParameterSupport": true,
                },
                "contextSupport": false,
            },
            "documentHighlight": {"dynamicRegistration": false},
            // No `resolveSupport`: hints are requested for a viewport and
            // painted whole, so there is no second round trip to opt into.
            // The `InlayHintLabelPart[]` label form needs no capability and
            // is parsed regardless.
            "inlayHint": {"dynamicRegistration": false},
            // C9: `dynamicRegistration: true` — unlike every other entry in
            // this block — because csharp-ls is believed to declare this
            // one dynamically rather than statically (see
            // `semantic_tokens` module docs); `formats: ["relative"]` is
            // the only encoding LSP 3.17 defines, so it is the only value
            // that could go here. `tokenTypes`/`tokenModifiers` are the
            // full LSP standard vocabulary this client's mapping
            // understands (`semantic_tokens::base_scope_name`); a server is
            // free to define fewer, and any it defines that this list omits
            // still decodes correctly; `requests.full: true` and no `range`
            // entry is what makes only the whole-document request offered.
            "semanticTokens": {
                "dynamicRegistration": true,
                "requests": {"full": true},
                "tokenTypes": crate::semantic_tokens::STANDARD_TOKEN_TYPES,
                "tokenModifiers": crate::semantic_tokens::STANDARD_TOKEN_MODIFIERS,
                "formats": ["relative"],
            },
            // C10: dynamic, because csharp-ls is believed to register this
            // one dynamically too, same reasoning as `semanticTokens` above.
            // No `resolveSupport`-shaped field exists for code lens in the
            // spec — a lens without a `command` always needs
            // `codeLens/resolve`, decided per item
            // (`code_lens::CodeLensItem::needs_resolve`), not by a
            // capability this client would advertise.
            "codeLens": {"dynamicRegistration": true},
            // C11: dynamic, on the same suspicion as `semanticTokens` and
            // `codeLens` above — csharp-ls is not confirmed to declare
            // either hierarchy capability statically. Neither carries a
            // resolve-style sub-capability worth advertising: an item's
            // `data` always round-trips through `incomingCalls`/
            // `outgoingCalls`/`supertypes`/`subtypes` verbatim, with no
            // separate resolve request in the spec.
            "callHierarchy": {"dynamicRegistration": true},
            "typeHierarchy": {"dynamicRegistration": true},
        },
        "workspace": {
            // RF5: we answer `workspace/applyEdit`, which is how the
            // command-driven refactorings reach us at all.
            "applyEdit": true,
            "executeCommand": {"dynamicRegistration": false},
            "workspaceEdit": {
                // Versions let a stale edit be caught before it is applied.
                "documentChanges": true,
                // F2: create, rename and delete are performed by
                // `app_core::AppSession::apply_file_ops` (F2). Without
                // these advertised, rust-analyzer's "move to submodule" and
                // every extract-to-new-file refactoring is refused whole —
                // the user sees "unsupported" for a correct edit.
                "resourceOperations": ["create", "rename", "delete"],
                // We apply all of an edit or none of it.
                "failureHandling": "abort",
                "normalizesLineEndings": false,
            },
            // C4: the one capability this client dynamically registers for
            // — csharp-ls and others declare their watched-file globs this
            // way rather than up front. `relativePatternSupport: false`
            // because `Registrations::watchers` hands `globPattern` on
            // untouched to C5, which does not yet resolve a `RelativePattern`
            // against a base URI.
            "didChangeWatchedFiles": {
                "dynamicRegistration": true,
                "relativePatternSupport": false,
            },
            // C6: we answer `workspace/configuration`, which is how
            // csharp-ls (and any server that pulls rather than takes pushed
            // settings) gets its config at all.
            "configuration": true,
        },
        // F0-16: without this a server has no permission to open a progress
        // token, and rust-analyzer stays silent while it indexes — which is
        // exactly the window in which it answers every request with nothing.
        "window": {"workDoneProgress": true},
        "general": {"positionEncodings": ["utf-16"]},
    })
}

#[cfg(test)]
mod host_translation_tests {
    use super::*;

    fn wsl_host() -> ExecHost {
        ExecHost::for_path(Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj"))
    }

    #[test]
    fn uri_for_is_unchanged_on_a_local_host() {
        assert_eq!(
            uri_for(&ExecHost::Local, "/home/f/proj/src/main.rs"),
            crate::diagnostics::uri_from_path("/home/f/proj/src/main.rs")
        );
    }

    #[test]
    fn uri_for_translates_a_windows_unc_path_to_a_linux_file_uri() {
        let host = wsl_host();
        assert_eq!(
            uri_for(&host, r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs"),
            "file:///home/f/proj/src/main.rs"
        );
    }

    #[test]
    fn path_for_translates_a_linux_file_uri_back_to_the_unc_path() {
        let host = wsl_host();
        assert_eq!(
            path_for(&host, "file:///home/f/proj/src/main.rs"),
            Some(r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs".to_string())
        );
    }

    #[test]
    fn uri_for_then_path_for_round_trips_to_identity() {
        let host = wsl_host();
        let original = r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs";
        let uri = uri_for(&host, original);
        assert_eq!(path_for(&host, &uri).as_deref(), Some(original));
    }

    #[test]
    fn new_translates_root_uri_for_a_wsl_root() {
        let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path(
            "//wsl.localhost/Ubuntu/home/f/proj",
        ));
        assert_eq!(manager.root_uri, "file:///home/f/proj");
        assert!(manager.host.is_remote());
    }

    #[test]
    fn new_leaves_root_uri_unchanged_for_a_local_root() {
        let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path("/home/f/proj"));
        assert_eq!(manager.root_uri, "file:///home/f/proj");
        assert!(!manager.host.is_remote());
    }

    /// normalize_uri applied twice must be a no-op — `format_range` falling
    /// back to `self.format(uri, options)` and every other re-entrant call
    /// in this crate depends on it.
    #[test]
    fn normalize_uri_is_idempotent() {
        let (manager, _rx) = LspManager::new(crate::diagnostics::uri_from_path(
            "//wsl.localhost/Ubuntu/home/f/proj",
        ));
        let naive =
            crate::diagnostics::uri_from_path("//wsl.localhost/Ubuntu/home/f/proj/src/main.rs");
        let once = manager.normalize_uri(&naive);
        let twice = manager.normalize_uri(&once);
        assert_eq!(once, "file:///home/f/proj/src/main.rs");
        assert_eq!(once, twice);
    }

    /// W3-3: a server binary absent from the distro is reported plainly,
    /// not as a generic spawn failure — proven with the same fake-`wsl.exe`
    /// trick `process-exec`'s own tests use, since `command -v` (and
    /// therefore `resolve_program`) genuinely finds nothing for it.
    #[test]
    fn starting_a_server_missing_from_the_distro_says_so() {
        use std::sync::Mutex;
        static PATH_LOCK: Mutex<()> = Mutex::new(());
        let _guard = PATH_LOCK.lock().unwrap();

        let bin_dir = tempfile::tempdir().unwrap();
        let script_path = bin_dir.path().join("wsl.exe");
        std::fs::write(&script_path, "#!/bin/sh\nexit 1\n").unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        std::fs::set_permissions(&script_path, perms).unwrap();

        // Not created on disk, and deliberately so: the WSL root is only
        // ever parsed for its UNC spelling — nothing here touches the
        // filesystem under it. Creating it would write `/wsl.localhost` at
        // the filesystem root, which only succeeds when the test runs as
        // root (see #251 for the same trap in analysis-core).
        let root = PathBuf::from("//wsl.localhost/Ubuntu/tmp/lsp-core-e2e");

        let original_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: serialized by PATH_LOCK.
        unsafe {
            std::env::set_var(
                "PATH",
                format!("{}:{original_path}", bin_dir.path().display()),
            );
        }
        let (manager, _rx) =
            LspManager::new(crate::diagnostics::uri_from_path(&root.to_string_lossy()));
        let result = manager.start(&ServerConfig {
            language_id: "rust".to_string(),
            name: "rust-analyzer".to_string(),
            command: "rust-analyzer".to_string(),
            args: Vec::new(),
            enabled: true,
            settings_section: None,
            settings: Value::Null,
            source: crate::catalog::ServerSource::Builtin,
        });
        unsafe {
            std::env::set_var("PATH", original_path);
        }

        let err = result.unwrap_err();
        assert!(
            matches!(&err, LspError::Spawn { source, .. } if source.to_string().contains("not found inside the WSL distro")),
            "expected a WSL-specific not-found message, got {err:?}"
        );
    }
}
