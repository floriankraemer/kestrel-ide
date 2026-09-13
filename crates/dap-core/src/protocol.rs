//! The Debug Adapter Protocol's message envelope and the few bodies this
//! client reads structurally (D1-2).
//!
//! Deliberately partial. DAP has around sixty request types and this crate
//! types the ones whose fields are actually read — a stack frame's line, a
//! variable's value, whether the adapter supports a capability — and leaves
//! the rest as `serde_json::Value`, which is what the adapter sent anyway.
//! A full mirror of the specification would be a thousand lines of structs
//! that only get read back out as JSON.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One message on the wire. DAP's envelope is *not* JSON-RPC: it has a
/// monotonic `seq`, and the discriminator is `type`, not the presence of an
/// `id`.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request {
        seq: i64,
        command: String,
        arguments: Value,
    },
    Response {
        request_seq: i64,
        success: bool,
        command: String,
        message: Option<String>,
        body: Value,
    },
    Event {
        event: String,
        body: Value,
    },
}

impl Message {
    /// Parse one framed payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Message, String> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|err| format!("not JSON: {err}"))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "message without a type".to_string())?;
        match kind {
            "request" => Ok(Message::Request {
                seq: value.get("seq").and_then(Value::as_i64).unwrap_or(0),
                command: string_at(&value, "command"),
                arguments: value.get("arguments").cloned().unwrap_or(Value::Null),
            }),
            "response" => Ok(Message::Response {
                request_seq: value
                    .get("request_seq")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                success: value
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                command: string_at(&value, "command"),
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                body: value.get("body").cloned().unwrap_or(Value::Null),
            }),
            "event" => Ok(Message::Event {
                event: string_at(&value, "event"),
                body: value.get("body").cloned().unwrap_or(Value::Null),
            }),
            other => Err(format!("unknown message type {other:?}")),
        }
    }

    /// The bytes for a request with this sequence number.
    pub fn request_bytes(seq: i64, command: &str, arguments: &Value) -> Vec<u8> {
        let mut message = serde_json::json!({
            "seq": seq,
            "type": "request",
            "command": command,
        });
        // An adapter that receives `"arguments": null` where it expected an
        // object is entitled to reject the request, so an argument-less
        // command sends no `arguments` key at all.
        if !arguments.is_null() {
            message["arguments"] = arguments.clone();
        }
        serde_json::to_vec(&message).expect("a json! value serialises")
    }

    /// The bytes for a response to a request the *adapter* sent us — DAP is
    /// bidirectional, and `runInTerminal` is the one that matters here.
    pub fn response_bytes(seq: i64, request_seq: i64, command: &str, body: &Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "seq": seq,
            "type": "response",
            "request_seq": request_seq,
            "success": true,
            "command": command,
            "body": body,
        }))
        .expect("a json! value serialises")
    }
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What an adapter said it can do, from its `initialize` response.
///
/// Every field defaults to "no": DAP's own rule is that an absent capability
/// is unsupported, and the view disables an action because the adapter said
/// so rather than because we guessed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
    pub supports_function_breakpoints: bool,
    pub supports_conditional_breakpoints: bool,
    pub supports_hit_conditional_breakpoints: bool,
    pub supports_log_points: bool,
    pub supports_evaluate_for_hovers: bool,
    pub supports_set_variable: bool,
    pub supports_restart_frame: bool,
    pub supports_step_in_targets_request: bool,
    pub supports_step_back: bool,
    pub supports_terminate_request: bool,
    pub supports_data_breakpoints: bool,
    pub supports_exception_options: bool,
    pub supports_exception_filter_options: bool,
    /// Filters the adapter offers for exception breakpoints, as
    /// `(filter id, label)`. Kept as pairs rather than a struct because the
    /// view shows exactly these two fields.
    #[serde(skip)]
    pub exception_filters: Vec<(String, String)>,
}

impl Capabilities {
    /// Read them out of an `initialize` response body, including the
    /// exception filters, which `serde` cannot map onto the pair list.
    pub fn from_body(body: &Value) -> Capabilities {
        let mut capabilities: Capabilities =
            serde_json::from_value(body.clone()).unwrap_or_default();
        if let Some(filters) = body
            .get("exceptionBreakpointFilters")
            .and_then(Value::as_array)
        {
            capabilities.exception_filters = filters
                .iter()
                .filter_map(|filter| {
                    Some((
                        filter.get("filter")?.as_str()?.to_string(),
                        filter.get("label")?.as_str()?.to_string(),
                    ))
                })
                .collect();
        }
        capabilities
    }
}

/// One frame of a stopped thread's stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    /// Empty when the frame has no source — a runtime-internal frame, or one
    /// the adapter only knows by address.
    pub path: String,
    pub line: u32,
    pub column: u32,
}

/// One scope of a frame: locals, arguments, globals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    /// The adapter's hint that this scope is expensive to expand — the view
    /// leaves such a scope collapsed rather than fetching it eagerly.
    pub expensive: bool,
}

/// One variable, or one child of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: String,
    /// Non-zero when this variable has children to fetch on expansion.
    pub variables_reference: i64,
}

/// One of the debuggee's threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

/// Why the debuggee stopped, from a `stopped` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    pub thread_id: i64,
    /// `breakpoint`, `step`, `exception`, `pause`, … reported as the adapter
    /// spelled it: the view shows it and never branches on it.
    pub reason: String,
    pub description: String,
    /// Whether every thread stopped, not just `thread_id`.
    pub all_threads_stopped: bool,
}

/// Read a `stackTrace` response body.
pub fn stack_frames(body: &Value) -> Vec<StackFrame> {
    array(body, "stackFrames")
        .iter()
        .filter_map(|frame| {
            Some(StackFrame {
                id: frame.get("id")?.as_i64()?,
                name: string_at(frame, "name"),
                path: frame
                    .get("source")
                    .and_then(|source| source.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                line: frame.get("line").and_then(Value::as_u64).unwrap_or(0) as u32,
                column: frame.get("column").and_then(Value::as_u64).unwrap_or(0) as u32,
            })
        })
        .collect()
}

/// Read a `scopes` response body.
pub fn scopes(body: &Value) -> Vec<Scope> {
    array(body, "scopes")
        .iter()
        .filter_map(|scope| {
            Some(Scope {
                name: string_at(scope, "name"),
                variables_reference: scope.get("variablesReference")?.as_i64()?,
                expensive: scope
                    .get("expensive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Read a `variables` response body.
pub fn variables(body: &Value) -> Vec<Variable> {
    array(body, "variables")
        .iter()
        .map(|variable| Variable {
            name: string_at(variable, "name"),
            value: string_at(variable, "value"),
            type_name: string_at(variable, "type"),
            variables_reference: variable
                .get("variablesReference")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        })
        .collect()
}

/// Read a `threads` response body.
pub fn threads(body: &Value) -> Vec<Thread> {
    array(body, "threads")
        .iter()
        .filter_map(|thread| {
            Some(Thread {
                id: thread.get("id")?.as_i64()?,
                name: string_at(thread, "name"),
            })
        })
        .collect()
}

/// Read a `stopped` event body.
pub fn stopped(body: &Value) -> Stopped {
    Stopped {
        thread_id: body.get("threadId").and_then(Value::as_i64).unwrap_or(0),
        reason: string_at(body, "reason"),
        description: string_at(body, "description"),
        all_threads_stopped: body
            .get("allThreadsStopped")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// Read an `evaluate` response body as the same shape a `variables` row is
/// (R5): `result`/`type`/`variablesReference` line up one-to-one with
/// `value`/`typeName`/`variablesReference`, so an evaluated expression or a
/// watch expands through the identical `variables` request the Variables
/// view already uses — one tree, not two.
pub fn evaluate_result(expression: &str, body: &Value) -> Variable {
    Variable {
        name: expression.to_string(),
        value: string_at(body, "result"),
        type_name: string_at(body, "type"),
        variables_reference: body
            .get("variablesReference")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    }
}

fn array<'a>(body: &'a Value, key: &str) -> &'a [Value] {
    body.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_message_type_parses() {
        let request = Message::from_bytes(
            br#"{"seq":1,"type":"request","command":"runInTerminal","arguments":{"args":["x"]}}"#,
        )
        .unwrap();
        assert!(matches!(request, Message::Request { seq: 1, .. }));

        let response = Message::from_bytes(
            br#"{"seq":2,"type":"response","request_seq":1,"success":true,"command":"initialize","body":{}}"#,
        )
        .unwrap();
        assert!(matches!(response, Message::Response { success: true, .. }));

        let event = Message::from_bytes(br#"{"seq":3,"type":"event","event":"stopped","body":{}}"#)
            .unwrap();
        assert!(matches!(event, Message::Event { .. }));
    }

    #[test]
    fn a_message_that_is_not_dap_is_an_error_rather_than_a_panic() {
        assert!(Message::from_bytes(b"not json").is_err());
        assert!(Message::from_bytes(br#"{"no":"type"}"#).is_err());
        assert!(Message::from_bytes(br#"{"type":"telepathy"}"#).is_err());
    }

    #[test]
    fn a_failed_response_carries_its_message() {
        let response = Message::from_bytes(
            br#"{"seq":2,"type":"response","request_seq":1,"success":false,"command":"launch","message":"no such file"}"#,
        )
        .unwrap();
        match response {
            Message::Response {
                success, message, ..
            } => {
                assert!(!success);
                assert_eq!(message.as_deref(), Some("no such file"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_argument_less_request_sends_no_arguments_key() {
        let bytes = Message::request_bytes(7, "configurationDone", &Value::Null);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("arguments").is_none());
        assert_eq!(value["seq"], 7);
        assert_eq!(value["type"], "request");
    }

    #[test]
    fn absent_capabilities_are_unsupported_rather_than_assumed() {
        let capabilities = Capabilities::from_body(&json!({}));
        assert!(!capabilities.supports_configuration_done_request);
        assert!(!capabilities.supports_set_variable);
        assert!(capabilities.exception_filters.is_empty());
    }

    #[test]
    fn capabilities_and_exception_filters_are_read_from_the_initialize_body() {
        let capabilities = Capabilities::from_body(&json!({
            "supportsConfigurationDoneRequest": true,
            "supportsSetVariable": true,
            "exceptionBreakpointFilters": [
                {"filter": "raised", "label": "Raised Exceptions"},
                {"filter": "uncaught", "label": "Uncaught Exceptions"},
            ],
        }));
        assert!(capabilities.supports_configuration_done_request);
        assert!(capabilities.supports_set_variable);
        assert_eq!(
            capabilities.exception_filters,
            vec![
                ("raised".to_string(), "Raised Exceptions".to_string()),
                ("uncaught".to_string(), "Uncaught Exceptions".to_string()),
            ]
        );
    }

    #[test]
    fn a_frame_without_a_source_still_parses() {
        let frames = stack_frames(&json!({
            "stackFrames": [
                {"id": 1, "name": "main", "source": {"path": "/p/src/main.rs"}, "line": 4, "column": 9},
                {"id": 2, "name": "__libc_start", "line": 0, "column": 0},
            ]
        }));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].path, "/p/src/main.rs");
        assert_eq!(
            frames[1].path, "",
            "a frame with no source is still a frame"
        );
    }

    #[test]
    fn variables_scopes_and_threads_parse() {
        assert_eq!(
            variables(&json!({"variables": [
                {"name": "answer", "value": "42", "type": "i32", "variablesReference": 0}
            ]}))[0],
            Variable {
                name: "answer".into(),
                value: "42".into(),
                type_name: "i32".into(),
                variables_reference: 0,
            }
        );
        assert_eq!(
            scopes(&json!({"scopes": [
                {"name": "Locals", "variablesReference": 3, "expensive": false}
            ]}))[0]
                .variables_reference,
            3
        );
        assert_eq!(
            threads(&json!({"threads": [{"id": 1, "name": "main"}]}))[0].name,
            "main"
        );
    }

    #[test]
    fn a_body_missing_its_array_yields_nothing_rather_than_panicking() {
        assert!(stack_frames(&json!({})).is_empty());
        assert!(variables(&json!({"variables": "not an array"})).is_empty());
    }

    #[test]
    fn a_stopped_event_is_read_as_the_adapter_spelled_it() {
        let event = stopped(&json!({
            "reason": "breakpoint", "threadId": 1, "allThreadsStopped": true
        }));
        assert_eq!(event.reason, "breakpoint");
        assert_eq!(event.thread_id, 1);
        assert!(event.all_threads_stopped);
    }

    #[test]
    fn an_evaluate_result_reads_as_a_variable_row() {
        let variable = evaluate_result(
            "items",
            &json!({"result": "[1, 2, 3]", "type": "list", "variablesReference": 7}),
        );
        assert_eq!(variable.name, "items");
        assert_eq!(variable.value, "[1, 2, 3]");
        assert_eq!(variable.type_name, "list");
        assert_eq!(variable.variables_reference, 7);
    }

    #[test]
    fn an_evaluate_result_with_no_children_carries_a_zero_reference() {
        let variable = evaluate_result("1 + 1", &json!({"result": "2", "type": "int"}));
        assert_eq!(variable.variables_reference, 0);
    }
}
