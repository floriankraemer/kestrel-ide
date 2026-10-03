//! The canned answers and small request readers the stub server's `main` loop
//! shares; split out to keep that file under the size ceiling.

use serde_json::{json, Value};

/// One `$/progress` notification for `token`.
pub(crate) fn progress(token: &str, value: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": "$/progress",
           "params": {"token": token, "value": value}})
}

/// The `textDocument.uri` of a request, or a stand-in when it has none.
pub(crate) fn uri_of(params: &Value) -> String {
    params
        .pointer("/textDocument/uri")
        .and_then(Value::as_str)
        .unwrap_or("file:///stub/main.rs")
        .to_string()
}

/// The 0-based line of a request's `position`, which is what every canned
/// answer above is keyed by.
pub(crate) fn position_line(params: &Value) -> u64 {
    params
        .pointer("/position/line")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// A `DocumentHighlight` on a 0-based line, optionally with a kind.
pub(crate) fn highlight(line: u64, kind: Option<u64>) -> Value {
    let mut value = json!({"range": {
        "start": {"line": line, "character": 4},
        "end": {"line": line, "character": 8},
    }});
    if let Some(kind) = kind {
        value["kind"] = json!(kind);
    }
    value
}

/// A `Location` in `uri` at a 0-based line/character.
pub(crate) fn location(uri: &str, line: u64, character: u64) -> Value {
    json!({"uri": uri, "range": {
        "start": {"line": line, "character": character},
        "end": {"line": line, "character": character + 4},
    }})
}

/// C11: a `CallHierarchyItem`/`TypeHierarchyItem` — the two are the same
/// shape on the wire, so one builder covers both stub features.
pub(crate) fn hierarchy_item(uri: &str, name: &str, kind: u64, line: u64) -> Value {
    json!({
        "name": name,
        "kind": kind,
        "uri": uri,
        "range": {"start": {"line": line, "character": 0},
                  "end": {"line": line, "character": 10}},
        "selectionRange": {"start": {"line": line, "character": 4},
                           "end": {"line": line, "character": 4 + name.len() as u64}},
    })
}

/// The one diagnostic this server ever reports, on line 1.
pub(crate) fn canned_diagnostic() -> Value {
    json!({
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 4}},
        "severity": 1,
        "source": "stub_server",
        "message": "canned diagnostic",
    })
}

/// The optional second problem (`STUB_LSP_GREET_DIAGNOSTIC`): a warning on the
/// fixture's `greet` call.
pub(crate) fn greet_problem() -> Value {
    json!({
        "range": {"start": {"line": 3, "character": 30}, "end": {"line": 3, "character": 35}},
        "severity": 2,
        "source": "stub_server",
        "code": "W0042",
        "message": "greet is deprecated",
    })
}

/// The `textDocument/rangeFormatting` reply: one edit naming the range it was
/// asked about for tab size 3, `MethodNotFound` otherwise.
pub(crate) fn range_formatting(id: Value, params: &Value) -> Value {
    let tab_size = params
        .pointer("/options/tabSize")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if tab_size != 3 {
        return json!({"jsonrpc": "2.0", "id": id, "error": {
            "code": -32601,
            "message": "textDocument/rangeFormatting is not implemented",
        }});
    }
    let at = |end: &str| {
        let get = |field: &str| {
            params
                .pointer(&format!("/range/{end}/{field}"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        (get("line"), get("character"))
    };
    let ((sl, sc), (el, ec)) = (at("start"), at("end"));
    let range = json!({"start": {"line": sl, "character": sc},
                       "end": {"line": el, "character": ec}});
    json!({"jsonrpc": "2.0", "id": id, "result": [
        {"range": range, "newText": format!("range {sl}:{sc}-{el}:{ec}")}
    ]})
}
