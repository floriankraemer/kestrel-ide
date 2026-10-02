//! E1: a second personality for the stub, so a test can run two differently
//! behaving servers for one language (ADR-0066).
//!
//! `STUB_LSP_TAG=<tag>` turns it on. Every answer below names the tag, so a
//! test can tell which server a merged or routed result came from, and
//! `STUB_LSP_CAPS=<a,b,…>` picks which `…Provider` capabilities `initialize`
//! advertises. The answers do not depend on the advertised set: a server the
//! client should not have asked still answers, and the test then sees it did.

use std::sync::Mutex;

use serde_json::{json, Map, Value};

/// What the client sent as `initialize.params`, for `stub/initializeParams`.
static INITIALIZE_PARAMS: Mutex<Value> = Mutex::new(Value::Null);

pub fn tag() -> Option<String> {
    std::env::var("STUB_LSP_TAG").ok().filter(|t| !t.is_empty())
}

pub fn remember_initialize(params: &Value) {
    *INITIALIZE_PARAMS.lock().expect("init lock") = params.clone();
}

/// The capabilities this profile adds on top of the stub's defaults.
pub fn capabilities(tag: &str) -> Map<String, Value> {
    let mut caps = Map::new();
    for name in std::env::var("STUB_LSP_CAPS")
        .unwrap_or_default()
        .split(',')
    {
        match name {
            "" => {}
            "completion" => {
                caps.insert(
                    "completionProvider".into(),
                    json!({"triggerCharacters": [format!("t{tag}")], "resolveProvider": true}),
                );
            }
            "codeAction" => {
                caps.insert(
                    "codeActionProvider".into(),
                    json!({"resolveProvider": true}),
                );
            }
            "executeCommand" => {
                caps.insert(
                    "executeCommandProvider".into(),
                    json!({"commands": [format!("cmd.{tag}")]}),
                );
            }
            other => {
                caps.insert(format!("{other}Provider"), json!(true));
            }
        }
    }
    caps
}

fn loc(uri: &str, line: u64) -> Value {
    json!({"uri": uri, "range": {"start": {"line": line, "character": 0},
                                 "end": {"line": line, "character": 1}}})
}

/// The profile's answer to `method`, or `None` to fall through to the stub's
/// own handlers.
pub fn answer(tag: &str, method: &str, params: &Value) -> Option<Value> {
    Some(match method {
        "stub/tag" => json!(tag),
        "stub/initializeParams" => INITIALIZE_PARAMS.lock().expect("init lock").clone(),
        "textDocument/completion" => json!({"isIncomplete": tag == "a", "items": [
            {"label": "shared", "kind": 2},
            {"label": format!("only_{tag}"), "kind": 2},
        ]}),
        "completionItem/resolve" => {
            let mut item = params.clone();
            item["detail"] = json!(format!("resolved by {tag}"));
            item
        }
        "textDocument/definition" => json!([loc(&format!("file:///{tag}.rs"), 0)]),
        "textDocument/hover" => {
            json!({"contents": {"kind": "plaintext", "value": format!("hover {tag}")}})
        }
        "textDocument/references" => {
            json!([
                loc("file:///shared.rs", 1),
                loc(&format!("file:///{tag}.rs"), 2)
            ])
        }
        "workspace/symbol" => json!([
            {"name": "Shared", "kind": 5, "location": loc("file:///shared.rs", 1)},
            {"name": format!("Sym{tag}"), "kind": 5, "location": loc("file:///s.rs", 3)},
        ]),
        "textDocument/codeAction" => json!([
            {"title": format!("fix from {tag}"), "kind": "quickfix", "data": {"from": tag}},
            {"title": format!("run {tag}"), "command": {
                "title": format!("run {tag}"), "command": format!("cmd.{tag}"), "arguments": []}},
        ]),
        "codeAction/resolve" => {
            let mut action = params.clone();
            action["edit"] = json!({"changes": {}});
            action["title"] = json!(format!("{} (resolved by {tag})", params["title"].as_str()?));
            action
        }
        "workspace/executeCommand" => json!({"ranOn": tag, "command": params["command"]}),
        _ => return None,
    })
}
