//! Debugging (D3-1): `DebugService`, the QObject that owns the breakpoints
//! and whatever debug sessions are running.
//!
//! # Threading
//!
//! Every DAP request blocks — an adapter evaluating an expression is running
//! the debuggee's own code — so no invokable here talks to an adapter on the
//! Qt thread. Each one hands the work to a short-lived thread and answers
//! through a signal, the same shape `BuildService` uses. `DapSession` is
//! `Sync`, so the thread only needs an `Arc` of it.
//!
//! The Qt thread keeps a cache of what the last `stopped` produced — threads,
//! frames, the variables fetched so far — so the views can be filled
//! synchronously from a signal without a round trip each.
//!
//! One QObject for N sessions, the ADR-0032 precedent: cxx-qt registers a
//! `#[qobject]` type's `QMetaObject` once at build time, so per-session
//! QObjects are mechanically unavailable, not merely undesirable.
//!
//! Translation only: what a breakpoint is, which adapter a project uses, and
//! what a launch body looks like are all `dap-core`'s (ADR-0041).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use dap_core::breakpoints::{Breakpoint, BreakpointStore};
use dap_core::{DapError, DapSession, SessionListener};
use serde_json::{json, Value};

use crate::bridge::errors;
use crate::bridge::ffi;

/// What the Qt thread keeps about one running session.
struct SessionState {
    session: Arc<DapSession>,
    /// The thread the adapter last reported stopped, and the frame the views
    /// are showing. Both are needed by every stepping request, so they are
    /// remembered rather than passed in from C++ each time.
    stopped_thread: i64,
    current_frame: i64,
    frames: Vec<dap_core::StackFrame>,
    threads: Vec<dap_core::Thread>,
    /// Variables already fetched, keyed by the reference they were fetched
    /// with. Cleared on every stop: a value from the previous suspension is
    /// worse than no value, because it looks current.
    variables: HashMap<i64, Vec<dap_core::Variable>>,
    /// The references of the current frame's own scopes, in the order the
    /// adapter listed them. `variables` is keyed globally, so without this
    /// there is no way to ask "what is in scope *here*" — which is exactly
    /// what inline values need (D3-7).
    scope_references: Vec<i64>,
}

/// What a session is attaching to (D4-1, D4-2).
enum AttachTo {
    /// A process on this machine, by id.
    Pid(u32),
    /// A debug server reachable over the network.
    Remote(dap_core::launch::RemoteTarget),
}

impl AttachTo {
    fn label(&self) -> String {
        match self {
            AttachTo::Pid(pid) => format!("attach {pid}"),
            AttachTo::Remote(target) => format!("attach {}:{}", target.host, target.port),
        }
    }

    /// The `attach` body, as a closure the session thread applies once it
    /// knows which adapter answered — every adapter spells attaching
    /// differently, and neither this type nor the caller should have to
    /// know which (`dap_core::launch` does).
    fn arguments(&self) -> Box<dyn FnOnce(&str) -> serde_json::Value + Send> {
        match self {
            AttachTo::Pid(pid) => {
                let pid = *pid;
                Box::new(move |adapter_id: &str| {
                    dap_core::launch::attach_arguments(adapter_id, pid)
                })
            }
            AttachTo::Remote(target) => {
                let target = target.clone();
                Box::new(move |adapter_id: &str| {
                    dap_core::launch::remote_attach_arguments(adapter_id, &target)
                })
            }
        }
    }
}

/// Rust side of the `DebugService` QObject.
#[derive(Default)]
pub struct DebugServiceRust {
    next_id: Cell<u64>,
    sessions: RefCell<HashMap<u64, SessionState>>,
    /// The breakpoints, which exist with no session at all — setting one
    /// before starting the debugger is the normal case.
    breakpoints: RefCell<BreakpointStore>,
    /// Watch expressions, evaluated again on every stop.
    watches: RefCell<Vec<String>>,
    /// The last watch results, in the same order as `watches`.
    watch_values: RefCell<Vec<String>>,
}

fn current_project_root() -> Option<PathBuf> {
    crate::bridge::convert::current_project_root()
}

fn no_project() -> ffi::FfiResult {
    ffi::FfiResult {
        code: errors::CODE_NO_PROJECT,
        message: QString::from("no project is open"),
    }
}

fn to_ffi_result(err: &DapError) -> ffi::FfiResult {
    ffi::FfiResult {
        code: err.code(),
        message: QString::from(err.to_string().as_str()),
    }
}

/// The listener that turns a session's events into signals. Every callback
/// runs on the session's reader thread, so everything is queued.
struct QtListener {
    session_id: u64,
    qt_thread: CxxQtThread<ffi::DebugService>,
}

impl SessionListener for QtListener {
    fn event(&mut self, event: &str, body: &Value) {
        let session_id = self.session_id;
        let event = event.to_string();
        let body = body.clone();
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::DebugService>| {
                service.as_mut().handle_event(session_id, &event, &body);
            });
    }

    fn reverse_request(&mut self, command: &str, arguments: &Value) -> Option<Value> {
        // D1-6: `runInTerminal` means the adapter cannot start the debuggee
        // itself and is asking us to. Started through `run-core`'s
        // supervisor — the same PTY a plain Run uses — so a debugged
        // program's output looks like a run's, and so this client can
        // honestly claim the capability (ADR-0041).
        if command != "runInTerminal" {
            return None;
        }
        let argv: Vec<String> = arguments
            .get("args")?
            .as_array()?
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        let (program, args) = argv.split_first()?;
        let spec = run_core::LaunchSpec {
            program: program.clone(),
            args: args.to_vec(),
            cwd: arguments
                .get("cwd")
                .and_then(Value::as_str)
                .map(PathBuf::from),
            env: arguments
                .get("env")
                .and_then(Value::as_object)
                .map(|env| {
                    env.iter()
                        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            console: run_core::ConsoleKind::Pty,
        };

        let session_id = self.session_id;
        let qt_thread = self.qt_thread.clone();
        let mut supervisor = run_core::Supervisor::new();
        let id = supervisor
            .launch(format!("debug-{session_id}"), &spec)
            .ok()?;
        let process_id = supervisor.process_id(id);
        if let Ok(mut reader) = supervisor.take_reader(id) {
            // The debuggee's own output, streamed into the debugger console
            // where the adapter's `output` events already go.
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                let mut ansi = run_core::AnsiStripper::default();
                while let Ok(read) = std::io::Read::read(&mut reader, &mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let text = ansi.feed(&String::from_utf8_lossy(&buffer[..read]));
                    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                        service.as_mut().debug_output(
                            session_id,
                            QString::from("stdout"),
                            QString::from(text.as_str()),
                        );
                    });
                }
                // Held until the process ends: dropping the supervisor kills
                // the PTY, and the debuggee with it.
                drop(supervisor);
            });
        }

        // The adapter attaches to this process, so it needs the id.
        Some(match process_id {
            Some(pid) => json!({ "processId": pid }),
            None => json!({}),
        })
    }

    fn disconnected(&mut self) {
        let session_id = self.session_id;
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::DebugService>| {
                service.as_mut().finish_session(session_id, 0);
            });
    }
}

impl ffi::DebugService {
    /// Start debugging `config_id`, the same configuration Run would launch.
    pub fn debug(mut self: Pin<&mut Self>, config_id: &QString) -> ffi::FfiResult {
        let config_id = config_id.to_string();
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let settings = app_config::project_settings::load(&root).unwrap_or_default();
        let Some(config) = settings
            .run_configs
            .unwrap_or_default()
            .into_iter()
            .find(|c| c.id == config_id)
        else {
            return ffi::FfiResult {
                code: errors::CODE_UNKNOWN_RUN_CONFIG,
                message: QString::from("unknown run configuration"),
            };
        };

        // Which adapter: the configuration's own toolchain if it has one,
        // otherwise whatever the project is built with. Both answers come
        // from the one toolchain table (ADR-0039).
        use run_core::RunConfigExt;
        let toolchain = config
            .toolchain()
            .or_else(|| run_core::detect_toolchains(&root).first().copied());
        let Some(adapter) = toolchain.and_then(|toolchain| {
            dap_core::catalog::for_toolchain(
                toolchain,
                &settings.debug_adapters.unwrap_or_default(),
            )
        }) else {
            let language = toolchain
                .map(|t| t.as_str().to_string())
                .unwrap_or_else(|| "this project".to_string());
            return to_ffi_result(&DapError::NoAdapter(language));
        };

        let mut spec = config.to_launch_spec(&root);
        spec.cwd = Some(spec.cwd.clone().unwrap_or_else(|| root.clone()));

        let session_id = self.next_id.get() + 1;
        self.next_id.set(session_id);
        let qt_thread = self.as_mut().qt_thread();
        let listener = Box::new(QtListener {
            session_id,
            qt_thread: qt_thread.clone(),
        });

        let session = match DapSession::start(&adapter, Some(&root), listener) {
            Ok(session) => session,
            Err(err) => return to_ffi_result(&err),
        };

        self.sessions.borrow_mut().insert(
            session_id,
            SessionState {
                session: Arc::clone(&session),
                stopped_thread: 0,
                current_frame: 0,
                frames: Vec::new(),
                scope_references: Vec::new(),
                threads: Vec::new(),
                variables: HashMap::new(),
            },
        );
        self.as_mut()
            .debug_started(session_id, QString::from(config_id.as_str()));

        // The handshake is three blocking requests with the breakpoints in
        // the middle, which is exactly what `configurationDone` exists for.
        let adapter_id = adapter.id.clone();
        let breakpoints = self.breakpoints.borrow().clone();
        std::thread::spawn(move || {
            let result = handshake(&session, &adapter_id, &spec, &breakpoints);
            if let Err(err) = result {
                let failure = (err.code(), err.to_string());
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                    service.as_mut().debug_failed(
                        session_id,
                        ffi::FfiResult {
                            code: failure.0,
                            message: QString::from(failure.1.as_str()),
                        },
                    );
                    service.as_mut().finish_session(session_id, -1);
                });
            }
        });
        ffi::FfiResult::default()
    }

    /// Attach to a process that is already running (D4-1).
    ///
    /// Which adapter: the project's toolchain, the same answer `debug` uses.
    /// The process id is the user's — this IDE does not enumerate processes,
    /// because doing it portably means three implementations and a
    /// permissions story, and every debugger's attach dialog is a list the
    /// user searches for a number they already know.
    pub fn attach(self: Pin<&mut Self>, pid: u32) -> ffi::FfiResult {
        self.attach_to(&AttachTo::Pid(pid))
    }

    /// Attach to a debuggee already running elsewhere — a debug server on a
    /// host and port (D4-2).
    ///
    /// The target is remembered in the project's settings, path mappings
    /// included: which server a project attaches to, and how a container's
    /// paths line up with the checkout, is the same for everyone working on
    /// it. The mappings are only editable in `.ide/settings.toml`, because
    /// a dialog for them would be a schema editor for something most
    /// projects leave empty.
    pub fn attach_remote(mut self: Pin<&mut Self>, host: &QString, port: u32) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let host = host.to_string();
        if host.is_empty() || port == 0 || port > u32::from(u16::MAX) {
            return ffi::FfiResult {
                code: errors::CODE_INVALID_ARGUMENT,
                message: QString::from("a remote target needs a host and a port"),
            };
        }

        let mut settings = app_config::project_settings::load(&root).unwrap_or_default();
        let mappings = settings
            .remote_attach
            .as_ref()
            .map(|previous| previous.path_mappings.clone())
            .unwrap_or_default();
        settings.remote_attach = Some(app_config::project_settings::RemoteAttachSetting {
            host: host.clone(),
            port: port as u16,
            path_mappings: mappings.clone(),
        });
        let _ = app_config::project_settings::save(&root, &settings);

        self.as_mut()
            .attach_to(&AttachTo::Remote(dap_core::launch::RemoteTarget {
                host,
                port: port as u16,
                mappings: mappings
                    .iter()
                    .filter_map(|mapping| mapping.split_once('='))
                    .map(|(local, remote)| (local.trim().to_string(), remote.trim().to_string()))
                    .collect(),
            }))
    }

    /// The remote target this project last attached to, as `host:port`, or
    /// empty if it never has — what the Attach to Remote dialog offers.
    pub fn last_remote_target(&self) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let settings = app_config::project_settings::load(&root).unwrap_or_default();
        match settings.remote_attach {
            Some(target) => QString::from(format!("{}:{}", target.host, target.port).as_str()),
            None => QString::default(),
        }
    }

    /// Start a session against something already running — a local pid or a
    /// remote server. The two differ only in what the `attach` body says
    /// and what the session is called; everything else — which adapter,
    /// which breakpoints, the initialize/attach/configurationDone order —
    /// is one path deliberately (D4-1, D4-2).
    fn attach_to(mut self: Pin<&mut Self>, to: &AttachTo) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let settings = app_config::project_settings::load(&root).unwrap_or_default();
        let toolchain = run_core::detect_toolchains(&root).first().copied();
        let Some(adapter) = toolchain.and_then(|toolchain| {
            dap_core::catalog::for_toolchain(
                toolchain,
                &settings.debug_adapters.unwrap_or_default(),
            )
        }) else {
            return to_ffi_result(&DapError::NoAdapter("this project".to_string()));
        };

        let session_id = self.next_id.get() + 1;
        self.next_id.set(session_id);
        let qt_thread = self.as_mut().qt_thread();
        let session = match DapSession::start(
            &adapter,
            Some(&root),
            Box::new(QtListener {
                session_id,
                qt_thread: qt_thread.clone(),
            }),
        ) {
            Ok(session) => session,
            Err(err) => return to_ffi_result(&err),
        };

        self.sessions.borrow_mut().insert(
            session_id,
            SessionState {
                session: Arc::clone(&session),
                stopped_thread: 0,
                current_frame: 0,
                frames: Vec::new(),
                scope_references: Vec::new(),
                threads: Vec::new(),
                variables: HashMap::new(),
            },
        );
        self.as_mut()
            .debug_started(session_id, QString::from(to.label().as_str()));

        let adapter_id = adapter.id.clone();
        let breakpoints = self.breakpoints.borrow().clone();
        let arguments = to.arguments();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), DapError> {
                session.initialize()?;
                session.attach(arguments(&adapter_id))?;
                session.wait_for_initialized(std::time::Duration::from_secs(10))?;
                send_configuration(&session, &breakpoints);
                session.configuration_done()
            })();
            if let Err(err) = result {
                let failure = (err.code(), err.to_string());
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                    service.as_mut().debug_failed(
                        session_id,
                        ffi::FfiResult {
                            code: failure.0,
                            message: QString::from(failure.1.as_str()),
                        },
                    );
                    service.as_mut().finish_session(session_id, -1);
                });
            }
        });
        ffi::FfiResult::default()
    }

    /// Whether this session's adapter can reload changed classes (D4-4).
    ///
    /// The answer is the adapter's, through `dap_core::catalog` — the view
    /// greys the action out because the debugger said it cannot do this,
    /// never because C++ recognised a language.
    pub fn can_reload_classes(&self, session_id: u64) -> bool {
        self.sessions
            .borrow()
            .get(&session_id)
            .is_some_and(|state| {
                dap_core::catalog::supports_class_reload(state.session.adapter_id())
            })
    }

    /// Redefine the running program's classes from what is now on disk
    /// (D4-4). A rebuild has to have happened first — this reloads what the
    /// build produced, it does not produce it.
    pub fn reload_classes(mut self: Pin<&mut Self>, session_id: u64) {
        if !self.can_reload_classes(session_id) {
            return;
        }
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let result = session.request(dap_core::catalog::CLASS_RELOAD_REQUEST, json!({}));
            if let Err(err) = result {
                let failure = (err.code(), err.to_string());
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                    service.as_mut().debug_failed(
                        session_id,
                        ffi::FfiResult {
                            code: failure.0,
                            message: QString::from(failure.1.as_str()),
                        },
                    );
                });
            }
        });
    }

    /// The exception filters this session's adapter offers, as
    /// `id\tlabel\tenabled` lines (D4-3).
    ///
    /// Per adapter, not per language: which exceptions can be broken on is
    /// something only the adapter knows, and it says so in its `initialize`
    /// response. A view that hard-coded "caught" and "uncaught" would be
    /// wrong for at least one of the three adapters shipped here.
    pub fn exception_filters(&self, session_id: u64) -> QString {
        let sessions = self.sessions.borrow();
        let Some(state) = sessions.get(&session_id) else {
            return QString::from("");
        };
        let enabled = self.breakpoints.borrow();
        let lines: Vec<String> = state
            .session
            .capabilities()
            .exception_filters
            .iter()
            .map(|(id, label)| {
                let on = enabled.exception_filters().iter().any(|f| f == id);
                format!("{id}\t{label}\t{on}")
            })
            .collect();
        QString::from(lines.join("\n").as_str())
    }

    /// Break on this class of exception, or stop doing so (D4-3).
    pub fn set_exception_filter(mut self: Pin<&mut Self>, filter: &QString, enabled: bool) {
        self.breakpoints
            .borrow_mut()
            .set_exception_filter(&filter.to_string(), enabled);
        self.as_mut().persist_breakpoints();

        let filters = self.breakpoints.borrow().exception_arguments();
        let sessions: Vec<Arc<DapSession>> = self
            .sessions
            .borrow()
            .values()
            .map(|state| Arc::clone(&state.session))
            .collect();
        for session in sessions {
            let filters = filters.clone();
            std::thread::spawn(move || {
                let _ = session.request("setExceptionBreakpoints", json!({ "filters": filters }));
            });
        }
        self.as_mut().breakpoints_changed();
    }

    /// Every running session, as `id\tlabel` lines — what the Debug dock's
    /// session picker lists (D4-5).
    pub fn sessions(&self) -> QString {
        let lines: Vec<String> = self
            .sessions
            .borrow()
            .keys()
            .map(|id| format!("{id}\tSession {id}"))
            .collect();
        QString::from(lines.join("\n").as_str())
    }

    /// End a session: ask the adapter to stop the debuggee, then make sure
    /// the adapter itself is gone.
    pub fn stop(self: Pin<&mut Self>, session_id: u64) {
        let session = self.session_handle(session_id);
        if let Some(session) = session {
            std::thread::spawn(move || session.shutdown());
        }
    }

    pub fn resume(self: Pin<&mut Self>, session_id: u64) {
        self.step(session_id, "continue");
    }

    pub fn pause(self: Pin<&mut Self>, session_id: u64) {
        self.step(session_id, "pause");
    }

    pub fn step_over(self: Pin<&mut Self>, session_id: u64) {
        self.step(session_id, "next");
    }

    pub fn step_into(self: Pin<&mut Self>, session_id: u64) {
        self.step(session_id, "stepIn");
    }

    pub fn step_out(self: Pin<&mut Self>, session_id: u64) {
        self.step(session_id, "stepOut");
    }

    /// Every stepping request is the same shape: one command against the
    /// stopped thread, answered by a `stopped` event rather than by the
    /// response.
    fn step(mut self: Pin<&mut Self>, session_id: u64, command: &str) {
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let thread_id = self
            .sessions
            .borrow()
            .get(&session_id)
            .map(|state| state.stopped_thread)
            .unwrap_or(0);
        let command = command.to_string();
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let result = session.request(&command, json!({ "threadId": thread_id }));
            if let Err(err) = result {
                let failure = (err.code(), err.to_string());
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                    service.as_mut().debug_failed(
                        session_id,
                        ffi::FfiResult {
                            code: failure.0,
                            message: QString::from(failure.1.as_str()),
                        },
                    );
                });
            }
        });
        // The debuggee is running again until the next `stopped`; the views
        // grey out on this rather than waiting for the response.
        let _ = command;
    }

    /// Run to a line without setting a permanent breakpoint there — a
    /// temporary breakpoint plus a resume, which is what every adapter that
    /// lacks `gotoTargets` supports.
    pub fn run_to_cursor(mut self: Pin<&mut Self>, session_id: u64, path: &QString, line: u32) {
        let path = PathBuf::from(path.to_string());
        {
            let mut breakpoints = self.breakpoints.borrow_mut();
            breakpoints.set(
                &path,
                Breakpoint {
                    line,
                    temporary: true,
                    ..Breakpoint::default()
                },
            );
        }
        self.as_mut().send_breakpoints_for(session_id, &path);
        self.resume(session_id);
    }

    /// The frames of the stopped thread, from the cache the last `stopped`
    /// filled.
    pub fn frames(&self) -> Vec<ffi::FfiStackFrame> {
        self.with_current(|state| {
            state
                .frames
                .iter()
                .map(|frame| ffi::FfiStackFrame {
                    id: frame.id,
                    name: QString::from(frame.name.as_str()),
                    path: QString::from(frame.path.as_str()),
                    line: frame.line,
                    column: frame.column,
                })
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn threads(&self) -> Vec<ffi::FfiDebugThread> {
        self.with_current(|state| {
            state
                .threads
                .iter()
                .map(|thread| ffi::FfiDebugThread {
                    id: thread.id,
                    name: QString::from(thread.name.as_str()),
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Variables already fetched for `reference`. Empty means "not fetched
    /// yet" — the view asks for them with `expand`, which answers through
    /// `variablesChanged`.
    pub fn variables(&self, reference: i64) -> Vec<ffi::FfiVariable> {
        self.with_current(|state| {
            state
                .variables
                .get(&reference)
                .map(|variables| {
                    variables
                        .iter()
                        .map(|variable| ffi::FfiVariable {
                            name: QString::from(variable.name.as_str()),
                            value: QString::from(variable.value.as_str()),
                            type_name: QString::from(variable.type_name.as_str()),
                            variables_reference: variable.variables_reference,
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
    }

    /// The values to paint at the end of the lines of `path`, given the
    /// buffer's current `text` (D3-7).
    ///
    /// The text comes from the view because the view owns it: a file being
    /// debugged may have unsaved edits, and reading it from disk would
    /// place values against lines the user is no longer looking at. Which
    /// value belongs on which line is `dap_core::inline_values`' rule;
    /// this only supplies the frame's variables and the stopped line.
    pub fn inline_values(&self, path: &QString, text: &QString) -> Vec<ffi::FfiInlineValue> {
        let path = path.to_string();
        self.with_current(|state| {
            let Some(frame) = state
                .frames
                .iter()
                .find(|frame| frame.id == state.current_frame)
            else {
                return Vec::new();
            };
            // Values belong to the file execution is in, not to whichever
            // file happens to be open.
            if frame.path != path {
                return Vec::new();
            }

            let variables: Vec<(String, String)> = state
                .scope_references
                .iter()
                .filter_map(|reference| state.variables.get(reference))
                .flatten()
                .map(|variable| (variable.name.clone(), variable.value.clone()))
                .collect();

            dap_core::inline_values(&text.to_string(), &variables, frame.line)
                .into_iter()
                .map(|value| ffi::FfiInlineValue {
                    line: value.line,
                    text: QString::from(value.text.as_str()),
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Fetch a frame's scopes, then the variables of each — one round trip
    /// per level, which is what makes a deep object cheap to show and
    /// expensive only where the user expands it.
    pub fn expand(mut self: Pin<&mut Self>, session_id: u64, reference: i64) {
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let Ok(body) = session.request("variables", json!({ "variablesReference": reference }))
            else {
                return;
            };
            let variables = dap_core::protocol::variables(&body);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                if let Some(state) = service.sessions.borrow_mut().get_mut(&session_id) {
                    state.variables.insert(reference, variables);
                }
                service.as_mut().variables_changed(session_id, reference);
            });
        });
    }

    /// The scopes of a frame, fetched and cached under the frame's own id so
    /// the view can ask for them like any other level.
    pub fn select_frame(mut self: Pin<&mut Self>, session_id: u64, frame_id: i64) {
        if let Some(state) = self.as_mut().sessions.borrow_mut().get_mut(&session_id) {
            state.current_frame = frame_id;
        }
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let Ok(body) = session.request("scopes", json!({ "frameId": frame_id })) else {
                return;
            };
            let scopes = dap_core::protocol::scopes(&body);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                let references: Vec<i64> = scopes
                    .iter()
                    .map(|scope| scope.variables_reference)
                    .collect();
                let names: Vec<String> = scopes.iter().map(|scope| scope.name.clone()).collect();
                if let Some(state) = service.sessions.borrow_mut().get_mut(&session_id) {
                    state.scope_references = references.clone();
                }
                service
                    .as_mut()
                    .scopes_changed(session_id, QString::from(names.join("\n").as_str()));
                for reference in references {
                    service.as_mut().expand(session_id, reference);
                }
            });
        });
    }

    /// Evaluate an expression in the selected frame — the Evaluate dialog,
    /// and the debugger console's input line.
    pub fn evaluate(mut self: Pin<&mut Self>, session_id: u64, expression: &QString) {
        let expression = expression.to_string();
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let frame_id = self
            .sessions
            .borrow()
            .get(&session_id)
            .map(|state| state.current_frame)
            .unwrap_or(0);
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let answer = evaluate_expression(&session, &expression, frame_id);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                service.as_mut().evaluated(
                    session_id,
                    QString::from(expression.as_str()),
                    QString::from(answer.as_str()),
                );
            });
        });
    }

    /// Change a variable's value where the adapter allows it. Refused
    /// locally when the adapter said it cannot, rather than sent and failed.
    pub fn set_variable(
        mut self: Pin<&mut Self>,
        session_id: u64,
        reference: i64,
        name: &QString,
        value: &QString,
    ) -> ffi::FfiResult {
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return to_ffi_result(&DapError::Disconnected("setVariable".into()));
        };
        if !session.capabilities().supports_set_variable {
            return to_ffi_result(&DapError::Unsupported("changing a variable".into()));
        }
        let (name, value) = (name.to_string(), value.to_string());
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let _ = session.request(
                "setVariable",
                json!({ "variablesReference": reference, "name": name, "value": value }),
            );
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                service.as_mut().expand(session_id, reference);
            });
        });
        ffi::FfiResult::default()
    }

    /// Whether the adapter of the given session allows changing a variable —
    /// what the Variables view's context menu is enabled from.
    pub fn can_set_variable(&self, session_id: u64) -> bool {
        self.sessions
            .borrow()
            .get(&session_id)
            .map(|state| state.session.capabilities().supports_set_variable)
            .unwrap_or(false)
    }

    pub fn watches(&self) -> QString {
        QString::from(self.watches.borrow().join("\n").as_str())
    }

    pub fn watch_values(&self) -> QString {
        QString::from(self.watch_values.borrow().join("\n").as_str())
    }

    pub fn add_watch(mut self: Pin<&mut Self>, expression: &QString) {
        let expression = expression.to_string();
        if expression.trim().is_empty() {
            return;
        }
        self.watches.borrow_mut().push(expression);
        self.watch_values.borrow_mut().push(String::new());
        self.as_mut().watches_changed();
        let session_id = self.current_session_id();
        if session_id != 0 {
            self.refresh_watches(session_id);
        }
    }

    pub fn remove_watch(mut self: Pin<&mut Self>, index: u32) {
        let index = index as usize;
        if index < self.watches.borrow().len() {
            self.watches.borrow_mut().remove(index);
            self.watch_values.borrow_mut().remove(index);
            self.as_mut().watches_changed();
        }
    }

    // -- Breakpoints ------------------------------------------------------

    /// Toggle a line breakpoint, and tell every running session about it.
    pub fn toggle_breakpoint(mut self: Pin<&mut Self>, path: &QString, line: u32) -> bool {
        let path = PathBuf::from(path.to_string());
        let now_set = self.breakpoints.borrow_mut().toggle(&path, line);
        self.as_mut().persist_breakpoints();
        let ids: Vec<u64> = self.sessions.borrow().keys().copied().collect();
        for session_id in ids {
            self.as_mut().send_breakpoints_for(session_id, &path);
        }
        self.as_mut().breakpoints_changed();
        now_set
    }

    /// The lines of `path` that have a breakpoint, newline-separated — the
    /// gutter asks for the whole file at once rather than line by line.
    pub fn breakpoint_lines(&self, path: &QString) -> QString {
        let path = PathBuf::from(path.to_string());
        let lines: Vec<String> = self
            .breakpoints
            .borrow()
            .in_file(&path)
            .iter()
            .map(|breakpoint| breakpoint.line.to_string())
            .collect();
        QString::from(lines.join("\n").as_str())
    }

    /// Give a breakpoint a condition, a hit condition or a log message, or
    /// enable/disable it — the breakpoints dialog's whole job.
    pub fn configure_breakpoint(
        mut self: Pin<&mut Self>,
        path: &QString,
        line: u32,
        enabled: bool,
        condition: &QString,
        log_message: &QString,
    ) {
        let path = PathBuf::from(path.to_string());
        {
            let mut breakpoints = self.breakpoints.borrow_mut();
            let existing = breakpoints.get(&path, line).cloned();
            breakpoints.set(
                &path,
                Breakpoint {
                    line,
                    enabled,
                    condition: condition.to_string(),
                    log_message: log_message.to_string(),
                    ..existing.unwrap_or_default()
                },
            );
        }
        self.as_mut().persist_breakpoints();
        let ids: Vec<u64> = self.sessions.borrow().keys().copied().collect();
        for session_id in ids {
            self.as_mut().send_breakpoints_for(session_id, &path);
        }
        self.as_mut().breakpoints_changed();
    }

    pub fn muted(&self) -> bool {
        self.breakpoints.borrow().muted()
    }

    /// Mute or unmute every breakpoint at once, and re-send them: muting
    /// means the adapter is told there are none, not that ours are gone.
    pub fn set_muted(mut self: Pin<&mut Self>, muted: bool) {
        self.breakpoints.borrow_mut().set_muted(muted);
        self.as_mut().persist_breakpoints();
        let files: Vec<PathBuf> = self
            .breakpoints
            .borrow()
            .files()
            .iter()
            .map(|path| path.to_path_buf())
            .collect();
        let ids: Vec<u64> = self.sessions.borrow().keys().copied().collect();
        for session_id in ids {
            for path in &files {
                self.as_mut().send_breakpoints_for(session_id, path);
            }
        }
        self.as_mut().breakpoints_changed();
    }

    /// An edit moved lines in `path`: `delta` lines were inserted at (or
    /// removed from) `from`. Driven from the buffer-edit seam, not from a
    /// hook of the debugger's own (ADR-0041).
    pub fn shift_breakpoints(mut self: Pin<&mut Self>, path: &QString, from: u32, delta: i64) {
        let path = PathBuf::from(path.to_string());
        let before = self.breakpoints.borrow().in_file(&path).to_vec();
        self.breakpoints
            .borrow_mut()
            .shift_lines(&path, from, delta);
        if self.breakpoints.borrow().in_file(&path) == before.as_slice() {
            return;
        }
        self.as_mut().persist_breakpoints();
        self.as_mut().breakpoints_changed();
    }

    /// Load this project's breakpoints. Called when a project opens.
    pub fn load_breakpoints(mut self: Pin<&mut Self>) {
        let Some(root) = current_project_root() else {
            return;
        };
        let settings = app_config::breakpoint_settings::load(&root).unwrap_or_default();
        *self.breakpoints.borrow_mut() =
            dap_core::breakpoints::persistence::from_settings(&settings);
        self.as_mut().breakpoints_changed();
    }

    // -- internals --------------------------------------------------------

    fn persist_breakpoints(self: Pin<&mut Self>) {
        let Some(root) = current_project_root() else {
            return;
        };
        let settings = dap_core::breakpoints::persistence::to_settings(&self.breakpoints.borrow());
        let _ = app_config::breakpoint_settings::update(&root, |stored| *stored = settings);
    }

    fn session_handle(self: Pin<&mut Self>, session_id: u64) -> Option<Arc<DapSession>> {
        self.sessions
            .borrow()
            .get(&session_id)
            .map(|state| Arc::clone(&state.session))
    }

    /// The session whose views are on screen. With one session it is that
    /// one; with several, the lowest id — a deterministic answer until D4
    /// gives the view session tabs to choose with.
    fn current_session_id(&self) -> u64 {
        self.sessions.borrow().keys().min().copied().unwrap_or(0)
    }

    fn with_current<T>(&self, read: impl FnOnce(&SessionState) -> T) -> Option<T> {
        let sessions = self.sessions.borrow();
        let id = sessions.keys().min().copied()?;
        sessions.get(&id).map(read)
    }

    fn send_breakpoints_for(mut self: Pin<&mut Self>, session_id: u64, path: &Path) {
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let arguments = json!({
            "source": { "path": dap_core::source_path(session.host(), path) },
            "breakpoints": self.breakpoints.borrow().source_breakpoints(path),
        });
        std::thread::spawn(move || {
            let _ = session.request("setBreakpoints", arguments);
        });
    }

    /// Re-evaluate every watch expression against the selected frame.
    fn refresh_watches(mut self: Pin<&mut Self>, session_id: u64) {
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let expressions = self.watches.borrow().clone();
        let frame_id = self
            .sessions
            .borrow()
            .get(&session_id)
            .map(|state| state.current_frame)
            .unwrap_or(0);
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let values: Vec<String> = expressions
                .iter()
                .map(|expression| evaluate_expression(&session, expression, frame_id))
                .collect();
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                *service.watch_values.borrow_mut() = values;
                service.as_mut().watches_changed();
            });
        });
    }

    /// One `stopped` event: fetch the threads and the stopped thread's
    /// frames, publish them, and re-evaluate the watches.
    fn on_stopped(mut self: Pin<&mut Self>, session_id: u64, stopped: dap_core::Stopped) {
        if let Some(state) = self.as_mut().sessions.borrow_mut().get_mut(&session_id) {
            state.stopped_thread = stopped.thread_id;
            state.variables.clear();
        }
        let Some(session) = self.as_mut().session_handle(session_id) else {
            return;
        };
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let threads = session
                .request("threads", Value::Null)
                .map(|body| dap_core::protocol::threads(&body))
                .unwrap_or_default();
            let mut frames = session
                .request("stackTrace", json!({ "threadId": stopped.thread_id }))
                .map(|body| dap_core::protocol::stack_frames(&body))
                .unwrap_or_default();
            // W4-5: each frame's `Source.path` came back from the adapter
            // as a Linux path on a WSL project root — translate it to the
            // UNC path the share serves before anything downstream (the
            // frame list, the caret jump) treats it as a local path.
            for frame in &mut frames {
                frame.path = dap_core::local_path(session.host(), &frame.path);
            }
            let top = frames.first().cloned();

            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                let top_id = top.as_ref().map(|frame| frame.id).unwrap_or(0);
                if let Some(state) = service.sessions.borrow_mut().get_mut(&session_id) {
                    state.threads = threads;
                    state.frames = frames;
                    state.current_frame = top_id;
                }
                let (path, line) = top
                    .map(|frame| (frame.path, frame.line))
                    .unwrap_or_default();
                service.as_mut().debug_stopped(
                    session_id,
                    QString::from(stopped.reason.as_str()),
                    QString::from(path.as_str()),
                    line,
                );
                if top_id != 0 {
                    service.as_mut().select_frame(session_id, top_id);
                }
                service.as_mut().refresh_watches(session_id);
            });
        });
    }

    /// Every event the adapter sends. Public to the bridge only because the
    /// listener queues a call to it from the reader thread.
    pub(crate) fn handle_event(
        mut self: Pin<&mut Self>,
        session_id: u64,
        event: &str,
        body: &Value,
    ) {
        match event {
            "stopped" => {
                let stopped = dap_core::protocol::stopped(body);
                self.on_stopped(session_id, stopped);
            }
            "continued" => self.as_mut().debug_resumed(session_id),
            "output" => {
                let category = body
                    .get("category")
                    .and_then(Value::as_str)
                    .unwrap_or("console");
                let text = body.get("output").and_then(Value::as_str).unwrap_or("");
                self.as_mut().debug_output(
                    session_id,
                    QString::from(category),
                    QString::from(text),
                );
            }
            "terminated" | "exited" => {
                let exit_code = body.get("exitCode").and_then(Value::as_i64).unwrap_or(0) as i32;
                self.as_mut().finish_session(session_id, exit_code);
            }
            _ => {}
        }
    }

    /// Forget a session and tell the view. Called both when the adapter
    /// reports the debuggee ended and when the adapter itself goes away, so
    /// it must be safe twice.
    pub(crate) fn finish_session(mut self: Pin<&mut Self>, session_id: u64, exit_code: i32) {
        if self.sessions.borrow_mut().remove(&session_id).is_none() {
            return;
        }
        self.as_mut().debug_terminated(session_id, exit_code);
    }
}

/// The three blocking requests a session starts with, with the breakpoints
/// sent in between — which is what `configurationDone` exists to bracket.
fn handshake(
    session: &Arc<DapSession>,
    adapter_id: &str,
    spec: &run_core::LaunchSpec,
    breakpoints: &BreakpointStore,
) -> Result<(), DapError> {
    session.initialize()?;
    // Launch is not awaited, and the breakpoints wait for the adapter's
    // `initialized` event: an adapter may hold the launch response until
    // `configurationDone`, which a client blocked on launch can never send
    // (see `DapSession::launch`).
    session.launch(dap_core::launch::arguments(adapter_id, spec))?;
    session.wait_for_initialized(std::time::Duration::from_secs(10))?;
    send_configuration(session, breakpoints);
    session.configuration_done()
}

/// The breakpoints and exception filters a session starts with, sent between
/// `initialized` and `configurationDone` — which is the window DAP gives for
/// exactly this.
fn send_configuration(session: &Arc<DapSession>, breakpoints: &BreakpointStore) {
    for path in breakpoints.files() {
        let _ = session.request(
            "setBreakpoints",
            json!({
                "source": { "path": dap_core::source_path(session.host(), path) },
                "breakpoints": breakpoints.source_breakpoints(path),
            }),
        );
    }
    if !breakpoints.exception_arguments().is_empty() {
        let _ = session.request(
            "setExceptionBreakpoints",
            json!({ "filters": breakpoints.exception_arguments() }),
        );
    }
}

/// One evaluation, reduced to the string the view shows. A failed
/// evaluation shows its own message rather than nothing: "no such variable"
/// is the answer to the question that was asked.
fn evaluate_expression(session: &Arc<DapSession>, expression: &str, frame_id: i64) -> String {
    let mut arguments = json!({ "expression": expression, "context": "watch" });
    if frame_id != 0 {
        arguments["frameId"] = json!(frame_id);
    }
    match session.request("evaluate", arguments) {
        Ok(body) => body
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Err(err) => err.to_string(),
    }
}
