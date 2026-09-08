//! One debug session: the adapter process, the request/response pairing,
//! and the handshake (D1-3, D1-5).
//!
//! # Threading
//!
//! Blocking, like `lsp-core` and for the same reason: a reader thread owns
//! the adapter's stdout and does nothing but read framed messages; every
//! request is sent from the caller's thread and waits on a channel the
//! reader fulfils. No runtime, no async — the adapter's caller marshals
//! results back to Qt through `CxxQtThread::queue()` (ADR-0007).
//!
//! Events do not go through that pairing: they arrive unsolicited, so the
//! reader hands them to a listener the session was built with. A stopped
//! event that arrived while a `stackTrace` was in flight must not be lost
//! behind it.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::catalog::Adapter;
use crate::error::DapError;
use crate::protocol::{Capabilities, Message};

/// How long a request may take before it is given up on. Generous: an
/// adapter attaching to a large process, or evaluating an expression that
/// runs code, can legitimately take seconds.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// What the session hands its owner as it happens: every event the adapter
/// sent, plus the fact that the adapter went away.
///
/// A trait rather than a channel of one enum because the caller is the Qt
/// adapter, which turns each of these into a different signal; a channel
/// would only move the match somewhere else.
pub trait SessionListener: Send {
    /// One DAP event, by name and body — `stopped`, `output`, `terminated`,
    /// `exited`, `thread`, and whatever else this adapter sends.
    fn event(&mut self, event: &str, body: &Value);
    /// A request *from* the adapter. `runInTerminal` is the one that
    /// matters: an adapter that cannot launch the debuggee itself asks the
    /// client to. Returning a body accepts it; returning `None` refuses.
    fn reverse_request(&mut self, command: &str, arguments: &Value) -> Option<Value>;
    /// The adapter's stdout reached EOF: it exited, cleanly or not.
    fn disconnected(&mut self);
}

struct Pending {
    waiters: HashMap<i64, Sender<Result<Value, DapError>>>,
}

/// A live debug adapter.
pub struct DapSession {
    adapter_id: String,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    pending: Arc<Mutex<Pending>>,
    next_seq: AtomicI64,
    capabilities: Mutex<Capabilities>,
    /// Set when the adapter's `initialized` event arrives. Waited on rather
    /// than assumed: the event is what says the adapter is ready for
    /// breakpoints, and DAP puts no ordering guarantee on it beyond that.
    initialized: Arc<(Mutex<bool>, Condvar)>,
    /// Where this session's adapter runs (ADR-0052) — derived once, from
    /// `start`'s `cwd`, the same way `LspManager` derives its own.
    host: process_exec::host::ExecHost,
}

/// A `Source.path` this client sends to the adapter — `setBreakpoints`'s
/// `source.path`, most commonly. Windows path in, translated through `host`
/// (a no-op unless `host` is remote): the adapter runs inside the distro on
/// a WSL project root and only ever understands its own Linux paths.
pub fn source_path(host: &process_exec::host::ExecHost, path: &Path) -> String {
    if host.is_remote() {
        host.to_remote(path)
    } else {
        path.display().to_string()
    }
}

/// The reverse of [`source_path`]: a `Source.path` the adapter sent back
/// (`StackFrame::path`, most commonly) translated to the Windows path the
/// share actually serves.
pub fn local_path(host: &process_exec::host::ExecHost, path: &str) -> String {
    if host.is_remote() {
        host.to_local(path).to_string_lossy().into_owned()
    } else {
        path.to_string()
    }
}

impl DapSession {
    /// Start `adapter`'s program and begin reading its output.
    ///
    /// This is where `ConsoleKind::Pipes` finally means something: a debug
    /// adapter speaks a protocol over stdio, so unlike a run or a build it
    /// must *not* be given a terminal — a PTY would echo, translate newlines
    /// and let the adapter think it is talking to a human (ADR-0032's
    /// reservation, spent here).
    pub fn start(
        adapter: &Adapter,
        cwd: Option<&std::path::Path>,
        mut listener: Box<dyn SessionListener>,
    ) -> Result<Arc<DapSession>, DapError> {
        // W4-5: `ExecHost::command` (ADR-0052) replaces a bare `Command::new`
        // — it runs the adapter inside the distro for a WSL project root,
        // and `resolve_program` reports a missing adapter binary plainly
        // rather than as a generic spawn failure, the same rule `lsp-core`'s
        // `connect` follows. `Local` never probes, so this costs nothing on
        // every other project.
        let host_cwd = cwd
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let host = process_exec::host::ExecHost::for_path(&host_cwd);
        let resolved_program =
            process_exec::host::resolve_program(&host, &adapter.program, &host_cwd)
                .unwrap_or_else(|| adapter.program.clone());
        let arg_refs: Vec<&str> = adapter.args.iter().map(String::as_str).collect();
        let mut command = host.command(&resolved_program, &arg_refs, &host_cwd, &[]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command.spawn().map_err(|err| DapError::AdapterNotStarted {
            adapter: adapter.id.clone(),
            reason: format!("{err}. {}", adapter.install_hint),
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DapError::AdapterNotStarted {
                adapter: adapter.id.clone(),
                reason: "the adapter has no stdin".into(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DapError::AdapterNotStarted {
                adapter: adapter.id.clone(),
                reason: "the adapter has no stdout".into(),
            })?;

        let session = Arc::new(DapSession {
            adapter_id: adapter.id.clone(),
            stdin: Mutex::new(Some(stdin)),
            child: Mutex::new(Some(child)),
            pending: Arc::new(Mutex::new(Pending {
                waiters: HashMap::new(),
            })),
            next_seq: AtomicI64::new(1),
            capabilities: Mutex::new(Capabilities::default()),
            initialized: Arc::new((Mutex::new(false), Condvar::new())),
            host,
        });

        let reader_session = Arc::clone(&session);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            // A message this client cannot parse is skipped rather than
            // ending the session: the next one may be the `stopped` event
            // the user is waiting for. EOF or an I/O error does end it.
            while let Ok(Some(bytes)) = stdio_framing::read_message(&mut reader) {
                if let Ok(message) = Message::from_bytes(&bytes) {
                    reader_session.dispatch(message, listener.as_mut());
                }
            }
            reader_session.fail_pending("the adapter exited");
            listener.disconnected();
        });

        Ok(session)
    }

    /// The adapter's declared capabilities, empty until `initialize` has
    /// answered. Every action the view offers is gated on one of these.
    pub fn capabilities(&self) -> Capabilities {
        self.capabilities.lock().expect("capabilities").clone()
    }

    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }

    /// `initialize`, then `launch` or `attach`, then the breakpoints the
    /// caller wants, then `configurationDone` — DAP's fixed order.
    ///
    /// Split so the caller can set breakpoints between the two halves, which
    /// is the whole reason the protocol has a `configurationDone` at all.
    pub fn initialize(&self) -> Result<Capabilities, DapError> {
        let body = self.request(
            "initialize",
            json!({
                "clientID": "kestrel-ide",
                "adapterID": self.adapter_id,
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                // Claimed, and answered: `SessionListener::reverse_request`
                // starts the program the adapter asks for and reports its
                // process id (D1-6). The claim and the implementation have
                // to move together — advertising this while answering with
                // an empty body is what made debugpy's launcher time out.
                "supportsRunInTerminalRequest": true,
            }),
        )?;
        let capabilities = Capabilities::from_body(&body);
        *self.capabilities.lock().expect("capabilities") = capabilities.clone();
        Ok(capabilities)
    }

    /// `launch` (start the program) or `attach` (join a running one). The
    /// arguments are the adapter's own — every adapter documents its own
    /// launch schema, and inventing a common one would mean translating into
    /// something no adapter accepts.
    /// Send `launch` **without waiting for its response**, which is the only
    /// order that works.
    ///
    /// An adapter is entitled to hold the launch response until the
    /// debuggee is actually up, and the debuggee does not start until
    /// `configurationDone` — which the client cannot send if it is still
    /// blocked on `launch`. debugpy does exactly this, and a client that
    /// waits deadlocks for its whole timeout and then reports "the adapter
    /// did not answer launch in time", which is a true statement about the
    /// wrong thing.
    ///
    /// A launch that genuinely fails is reported by the adapter as an
    /// `output` event and a `terminated`, which the caller is listening for
    /// anyway.
    /// Where this session's adapter runs (ADR-0052) — `ExecHost::Local`
    /// unless the debugged project's root is a WSL UNC path. Callers use
    /// [`source_path`]/[`local_path`] against this to translate a
    /// `Source.path` at the seam.
    pub fn host(&self) -> &process_exec::host::ExecHost {
        &self.host
    }

    pub fn launch(&self, arguments: Value) -> Result<(), DapError> {
        self.send_request("launch", arguments)
    }

    /// The same, for joining a process that is already running.
    pub fn attach(&self, arguments: Value) -> Result<(), DapError> {
        self.send_request("attach", arguments)
    }

    /// Send a request and do not wait for it. Its response is matched
    /// against no waiter and dropped, which is what "fire and forget" means
    /// on a protocol that answers everything.
    pub fn send_request(&self, command: &str, arguments: Value) -> Result<(), DapError> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.send(&Message::request_bytes(seq, command, &arguments))
    }

    /// Block until the adapter's `initialized` event arrives.
    ///
    /// That event is the adapter saying it is ready for breakpoints. Sending
    /// them before it is the other way to get this handshake wrong.
    pub fn wait_for_initialized(&self, timeout: Duration) -> Result<(), DapError> {
        let (lock, condvar) = &*self.initialized;
        let mut ready = lock
            .lock()
            .map_err(|_| DapError::Protocol("the initialized flag was poisoned".to_string()))?;
        while !*ready {
            let (guard, wait) = condvar
                .wait_timeout(ready, timeout)
                .map_err(|_| DapError::Protocol("the initialized flag was poisoned".to_string()))?;
            ready = guard;
            if wait.timed_out() && !*ready {
                return Err(DapError::Timeout("initialized".to_string()));
            }
        }
        Ok(())
    }

    /// End of the configuration phase. Skipped for an adapter that does not
    /// support it, which the specification explicitly allows.
    pub fn configuration_done(&self) -> Result<(), DapError> {
        if !self.capabilities().supports_configuration_done_request {
            return Ok(());
        }
        self.request("configurationDone", Value::Null).map(|_| ())
    }

    /// Send a request and wait for its response.
    pub fn request(&self, command: &str, arguments: Value) -> Result<Value, DapError> {
        self.request_with_timeout(command, arguments, DEFAULT_TIMEOUT)
    }

    pub fn request_with_timeout(
        &self,
        command: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, DapError> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.pending
            .lock()
            .expect("pending")
            .waiters
            .insert(seq, tx);

        if let Err(err) = self.send(&Message::request_bytes(seq, command, &arguments)) {
            self.pending.lock().expect("pending").waiters.remove(&seq);
            return Err(err);
        }

        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().expect("pending").waiters.remove(&seq);
                Err(DapError::Timeout(command.to_string()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(DapError::Disconnected(command.to_string())),
        }
    }

    /// Ask the adapter to end the session, then make sure the process is
    /// gone. `disconnect` is best-effort: an adapter that has already died
    /// cannot answer, and that is not a failure worth reporting.
    pub fn shutdown(&self) {
        let _ = self.request_with_timeout(
            "disconnect",
            json!({ "terminateDebuggee": true }),
            Duration::from_secs(2),
        );
        // Dropping stdin is what tells an adapter that ignored `disconnect`
        // that there is nothing more coming.
        *self.stdin.lock().expect("stdin") = None;
        if let Some(mut child) = self.child.lock().expect("child").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.fail_pending("the session was stopped");
    }

    fn send(&self, payload: &[u8]) -> Result<(), DapError> {
        let mut guard = self.stdin.lock().expect("stdin");
        let stdin = guard
            .as_mut()
            .ok_or_else(|| DapError::Disconnected("send".to_string()))?;
        stdio_framing::write_message(stdin, payload)
            .map_err(|err| DapError::Disconnected(err.to_string()))?;
        stdin
            .flush()
            .map_err(|err| DapError::Disconnected(err.to_string()))
    }

    fn dispatch(&self, message: Message, listener: &mut dyn SessionListener) {
        match message {
            Message::Response {
                request_seq,
                success,
                command,
                message,
                body,
            } => {
                let waiter = self
                    .pending
                    .lock()
                    .expect("pending")
                    .waiters
                    .remove(&request_seq);
                if let Some(waiter) = waiter {
                    let result = if success {
                        Ok(body)
                    } else {
                        Err(DapError::Request {
                            command,
                            message: message.unwrap_or_else(|| "no reason given".to_string()),
                        })
                    };
                    let _ = waiter.send(result);
                }
            }
            Message::Event { event, body } => {
                if event == "initialized" {
                    let (lock, condvar) = &*self.initialized;
                    if let Ok(mut ready) = lock.lock() {
                        *ready = true;
                        condvar.notify_all();
                    }
                }
                listener.event(&event, &body)
            }
            Message::Request {
                seq,
                command,
                arguments,
            } => {
                // DAP is bidirectional. An adapter that cannot start the
                // debuggee itself asks the client to, and an unanswered
                // reverse request leaves the adapter waiting forever.
                if let Some(body) = listener.reverse_request(&command, &arguments) {
                    let response_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                    let _ = self.send(&Message::response_bytes(response_seq, seq, &command, &body));
                }
            }
        }
    }

    /// Fail every in-flight request, so no caller waits for a response that
    /// can never arrive.
    fn fail_pending(&self, what: &str) {
        let waiters = std::mem::take(&mut self.pending.lock().expect("pending").waiters);
        for (_, waiter) in waiters {
            let _ = waiter.send(Err(DapError::Disconnected(what.to_string())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// A stand-in adapter: `sh` reading framed requests and answering them.
    /// Real-adapter coverage is the manual matrix in the plan (D4-6); what
    /// is worth testing here is this client's own framing, pairing and
    /// failure handling.
    fn echo_adapter(script: &str) -> Adapter {
        Adapter {
            id: "test".into(),
            program: "sh".into(),
            args: vec!["-c".into(), script.to_string()],
            install_hint: "install the test adapter".into(),
        }
    }

    // W4-5: `source_path`/`local_path` translate a `Source.path` at the
    // seam, a no-op on `ExecHost::Local` and both directions of the same
    // rule on a WSL root.
    #[test]
    fn source_path_is_unchanged_on_a_local_host() {
        let host = process_exec::host::ExecHost::Local;
        assert_eq!(
            source_path(&host, Path::new("/home/f/proj/src/main.rs")),
            "/home/f/proj/src/main.rs"
        );
    }

    #[test]
    fn source_path_translates_a_unc_path_to_linux_on_a_wsl_host() {
        let host =
            process_exec::host::ExecHost::for_path(Path::new("//wsl.localhost/Ubuntu/home/f/proj"));
        assert_eq!(
            source_path(
                &host,
                Path::new("//wsl.localhost/Ubuntu/home/f/proj/src/main.rs")
            ),
            "/home/f/proj/src/main.rs"
        );
    }

    #[test]
    fn local_path_translates_a_linux_path_back_to_the_unc_path() {
        let host =
            process_exec::host::ExecHost::for_path(Path::new("//wsl.localhost/Ubuntu/home/f/proj"));
        assert_eq!(
            local_path(&host, "/home/f/proj/src/main.rs"),
            "//wsl.localhost/Ubuntu/home/f/proj/src/main.rs"
        );
    }

    #[derive(Default)]
    struct Recorder {
        events: Arc<Mutex<Vec<String>>>,
        disconnected: Arc<AtomicBool>,
        reverse: Arc<Mutex<Vec<String>>>,
    }

    impl SessionListener for Recorder {
        fn event(&mut self, event: &str, _body: &Value) {
            self.events.lock().unwrap().push(event.to_string());
        }
        fn reverse_request(&mut self, command: &str, _arguments: &Value) -> Option<Value> {
            self.reverse.lock().unwrap().push(command.to_string());
            Some(json!({}))
        }
        fn disconnected(&mut self) {
            self.disconnected.store(true, Ordering::Relaxed);
        }
    }

    /// One framed message, as a shell `printf` argument.
    fn framed(body: &str) -> String {
        format!(
            "printf 'Content-Length: {}\\r\\n\\r\\n%s' '{}'",
            body.len(),
            body
        )
    }

    #[test]
    fn an_adapter_that_is_not_installed_reports_the_install_hint() {
        let adapter = Adapter {
            program: "definitely-not-installed-adapter".into(),
            ..echo_adapter("true")
        };
        let err = match DapSession::start(&adapter, None, Box::<Recorder>::default()) {
            Err(err) => err,
            Ok(_) => panic!("an adapter that is not installed must not start"),
        };
        assert_eq!(err.code(), DapError::CODE_ADAPTER_NOT_STARTED);
        assert!(
            err.to_string().contains("install the test adapter"),
            "{err}"
        );
    }

    #[test]
    fn initialize_pairs_its_response_and_records_the_capabilities() {
        let response = r#"{"seq":1,"type":"response","request_seq":1,"success":true,"command":"initialize","body":{"supportsConfigurationDoneRequest":true}}"#;
        // Read the request first so the adapter does not answer before it is
        // asked, then answer and stay alive briefly.
        let script = format!("head -c 1 > /dev/null; {}; sleep 1", framed(response));
        let session =
            DapSession::start(&echo_adapter(&script), None, Box::<Recorder>::default()).unwrap();
        let capabilities = session.initialize().unwrap();
        assert!(capabilities.supports_configuration_done_request);
        assert!(session.capabilities().supports_configuration_done_request);
        session.shutdown();
    }

    #[test]
    fn a_failed_response_becomes_a_typed_error_naming_the_command() {
        let response = r#"{"seq":1,"type":"response","request_seq":1,"success":false,"command":"stackTrace","message":"no such thread"}"#;
        let script = format!("head -c 1 > /dev/null; {}; sleep 1", framed(response));
        let session =
            DapSession::start(&echo_adapter(&script), None, Box::<Recorder>::default()).unwrap();
        let err = session
            .request("stackTrace", json!({ "threadId": 1 }))
            .unwrap_err();
        assert_eq!(err.code(), DapError::CODE_REQUEST);
        assert!(err.to_string().contains("no such thread"), "{err}");
        session.shutdown();
    }

    #[test]
    fn an_adapter_that_exits_fails_the_request_in_flight_rather_than_hanging() {
        let session =
            DapSession::start(&echo_adapter("exit 0"), None, Box::<Recorder>::default()).unwrap();
        let err = session.initialize().unwrap_err();
        assert_eq!(err.code(), DapError::CODE_DISCONNECTED);
    }

    #[test]
    fn events_reach_the_listener_and_eof_reports_the_disconnect() {
        let recorder = Recorder::default();
        let events = Arc::clone(&recorder.events);
        let disconnected = Arc::clone(&recorder.disconnected);
        let stopped = r#"{"seq":1,"type":"event","event":"stopped","body":{"reason":"breakpoint","threadId":1}}"#;
        let session = DapSession::start(
            &echo_adapter(&format!("{}; exit 0", framed(stopped))),
            None,
            Box::new(recorder),
        )
        .unwrap();

        for _ in 0..100 {
            if disconnected.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(events.lock().unwrap().as_slice(), ["stopped"]);
        assert!(disconnected.load(Ordering::Relaxed));
        session.shutdown();
    }

    #[test]
    fn a_reverse_request_is_answered_rather_than_left_waiting() {
        let recorder = Recorder::default();
        let reverse = Arc::clone(&recorder.reverse);
        let request =
            r#"{"seq":1,"type":"request","command":"runInTerminal","arguments":{"args":["a"]}}"#;
        let session = DapSession::start(
            &echo_adapter(&format!("{}; sleep 1", framed(request))),
            None,
            Box::new(recorder),
        )
        .unwrap();

        for _ in 0..100 {
            if !reverse.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(reverse.lock().unwrap().as_slice(), ["runInTerminal"]);
        session.shutdown();
    }

    #[test]
    fn launch_does_not_wait_for_its_response() {
        // An adapter that holds the launch response until `configurationDone`
        // is not misbehaving — debugpy does it — so `launch` must return
        // immediately or the handshake deadlocks.
        let session =
            DapSession::start(&echo_adapter("sleep 2"), None, Box::<Recorder>::default()).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(session.launch(json!({})), Ok(()));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "launch waited for a response"
        );
        session.shutdown();
    }

    #[test]
    fn waiting_for_initialized_ends_when_the_event_arrives() {
        let event = r#"{"seq":1,"type":"event","event":"initialized"}"#;
        let script = format!("{}; sleep 1", framed(event));
        let session =
            DapSession::start(&echo_adapter(&script), None, Box::<Recorder>::default()).unwrap();
        assert_eq!(session.wait_for_initialized(Duration::from_secs(5)), Ok(()));
        session.shutdown();
    }

    #[test]
    fn waiting_for_initialized_times_out_rather_than_hanging() {
        let session =
            DapSession::start(&echo_adapter("sleep 2"), None, Box::<Recorder>::default()).unwrap();
        assert_eq!(
            session.wait_for_initialized(Duration::from_millis(200)),
            Err(DapError::Timeout("initialized".to_string()))
        );
        session.shutdown();
    }

    #[test]
    fn configuration_done_is_skipped_when_the_adapter_does_not_support_it() {
        // No script at all: if this sent a request it would block until the
        // timeout, so returning promptly *is* the assertion.
        let session =
            DapSession::start(&echo_adapter("sleep 1"), None, Box::<Recorder>::default()).unwrap();
        assert_eq!(session.configuration_done(), Ok(()));
        session.shutdown();
    }
}
