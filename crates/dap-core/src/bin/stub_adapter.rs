//! A scripted stand-in for vscode-php-debug, so listen-session tests run
//! with no PHP, Node or Xdebug installed (PHP parity D0).
//!
//! Same precedent as `lsp-core`'s `stub_server`: a `[[bin]]`, found by tests
//! through `env!("CARGO_BIN_EXE_stub_adapter")`.
//!
//! Script: `initialize` answers capabilities and sends `initialized`;
//! `launch` (with or without `program` — without is listen mode) echoes its
//! arguments as an `output` event so a test can inspect them; after
//! `configurationDone` the first "connection" arrives as thread 1 and stops
//! at the first breakpoint set (if any); `continue` ends that connection
//! and a second arrives as thread 2, also stopped; a second `continue`
//! ends it. `stackTrace`, `scopes`, `variables` and `evaluate` answer
//! canned data (`$answer = 42`); `disconnect` exits.

use std::io::{self, BufReader, Write};

use serde_json::{json, Value};

struct Stub {
    out: io::Stdout,
    seq: i64,
    breakpoint: Option<(String, i64)>,
}

impl Stub {
    fn send(&mut self, mut message: Value) {
        self.seq += 1;
        message["seq"] = json!(self.seq);
        let bytes = serde_json::to_vec(&message).expect("json");
        let mut out = self.out.lock();
        stdio_framing::write_message(&mut out, &bytes).expect("write");
        out.flush().expect("flush");
    }

    fn event(&mut self, event: &str, body: Value) {
        self.send(json!({"type": "event", "event": event, "body": body}));
    }

    fn respond(&mut self, request: &Value, body: Value) {
        self.send(json!({
            "type": "response", "request_seq": request["seq"], "success": true,
            "command": request["command"], "body": body,
        }));
    }

    fn connection(&mut self, thread: i64) {
        self.event("thread", json!({"reason": "started", "threadId": thread}));
        if self.breakpoint.is_some() {
            self.event(
                "stopped",
                json!({"reason": "breakpoint", "threadId": thread, "allThreadsStopped": false}),
            );
        }
    }

    fn handle(&mut self, request: &Value) -> bool {
        let args = &request["arguments"];
        match request["command"].as_str().unwrap_or_default() {
            "initialize" => {
                self.respond(request, json!({"supportsConfigurationDoneRequest": true}));
                self.event("initialized", json!({}));
            }
            "launch" => {
                self.respond(request, json!({}));
                self.event("output", json!({"category": "console", "output": args.to_string()}));
            }
            "setBreakpoints" => {
                let path = args["source"]["path"].as_str().unwrap_or_default().to_string();
                let lines: Vec<i64> = args["breakpoints"]
                    .as_array()
                    .map(|bps| bps.iter().filter_map(|bp| bp["line"].as_i64()).collect())
                    .unwrap_or_default();
                self.breakpoint = lines.first().map(|line| (path, *line));
                let verified: Vec<Value> =
                    lines.iter().map(|l| json!({"verified": true, "line": l})).collect();
                self.respond(request, json!({"breakpoints": verified}));
            }
            "configurationDone" => {
                self.respond(request, json!({}));
                self.connection(1);
            }
            "continue" => {
                let thread = args["threadId"].as_i64().unwrap_or(1);
                self.respond(request, json!({"allThreadsContinued": false}));
                self.event("thread", json!({"reason": "exited", "threadId": thread}));
                if thread == 1 {
                    self.connection(2);
                }
            }
            "threads" => self.respond(
                request,
                json!({"threads": [{"id": 1, "name": "Request 1 (stub)"}]}),
            ),
            "stackTrace" => {
                let (path, line) = self.breakpoint.clone().unwrap_or_default();
                self.respond(
                    request,
                    json!({"stackFrames": [{"id": 1, "name": "{main}", "line": line, "column": 1,
                        "source": {"path": path}}], "totalFrames": 1}),
                );
            }
            "scopes" => self.respond(
                request,
                json!({"scopes": [{"name": "Locals", "variablesReference": 1, "expensive": false}]}),
            ),
            "variables" => self.respond(
                request,
                json!({"variables": [{"name": "$answer", "value": "42", "type": "int",
                    "variablesReference": 0}]}),
            ),
            "evaluate" => self.respond(
                request,
                json!({"result": "42", "type": "int", "variablesReference": 0}),
            ),
            "disconnect" => {
                self.respond(request, json!({}));
                return false;
            }
            _ => self.respond(request, json!({})),
        }
        true
    }
}

fn main() {
    let mut input = BufReader::new(io::stdin());
    let mut stub = Stub {
        out: io::stdout(),
        seq: 0,
        breakpoint: None,
    };
    while let Ok(Some(bytes)) = stdio_framing::read_message(&mut input) {
        let Ok(request) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if !stub.handle(&request) {
            break;
        }
    }
}
