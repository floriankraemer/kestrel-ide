//! A minimal stub language server, used as a test fixture so `lsp-core`'s
//! tests run offline with no real language server installed (task X2).
//!
//! It is a `[[bin]]` of this crate rather than an example so that integration
//! tests can locate it with `env!("CARGO_BIN_EXE_stub_server")` — a path Cargo
//! guarantees, instead of guessing at `target/debug` layout, which breaks
//! under a custom `CARGO_TARGET_DIR`, `--release`, or cross-compilation.
//!
//! Supported: framing, `initialize`/`initialized`/`shutdown`/`exit`, a canned
//! diagnostic on `textDocument/didOpen`, canned `textDocument/hover`,
//! `textDocument/definition` and `textDocument/completion` replies that vary by the requested position so
//! every response shape the protocol allows is reachable, and two test
//! affordances —
//! `stub/echo` (with an optional `delay_ms`, answered on its own thread so
//! responses can come back out of order) and the `STUB_LSP_DIE_ON_DIDOPEN`
//! environment variable, which makes the server die mid-session.
//!
//! RF3 added the refactoring half: `textDocument/codeAction`,
//! `codeAction/resolve`, `textDocument/prepareRename`, `textDocument/rename`
//! and `workspace/executeCommand` — the last of which can send a
//! `workspace/applyEdit` request *back* to the client and block on the
//! answer, which is how the command-driven refactorings that jdtls and
//! intelephense use are exercised offline.
//!
//! F2 added the Alt+Enter surfaces — `textDocument/signatureHelp`,
//! `documentHighlight` and `inlayHint` — and, more to the point, the failure
//! paths a real language server will not produce on demand (ADR-0024's
//! division of labour): `stub/silence` never answers at all, `stub/garbage`
//! sends a well-framed message that is not a JSON-RPC response, and
//! `stub/lateDuplicate` answers the same id twice so a reply arriving after
//! its request was superseded can be shown to go nowhere. Malformed replies
//! are reachable per method too — signature help on line 3, a document
//! highlight with no range on line 3, an inlay hint whose label is a number
//! on line 900. A `codeAction` request at line 5 answers with a resource
//! operation ahead of its text edits (F2-3), for the one thing none of the
//! above exercises: creating a file as part of an edit.
//!
//! C4 added `stub/registerCapabilityRun`, which sends a
//! `client/registerCapability` for `workspace/didChangeWatchedFiles`
//! followed by a `client/unregisterCapability` for the same id — the
//! sequence csharp-ls actually runs, since it declares most of its
//! capabilities dynamically rather than in `initialize`.
//!
//! C6 added `stub/configurationRun`, which sends a `workspace/configuration`
//! request for two sections — one the test's server was configured under
//! and one it was not — the way csharp-ls pulls its settings after
//! `initialized`.
//!
//! C7 added `STUB_LSP_COMPLETION_RESOLVE`, which makes `initialize`
//! advertise `completionProvider.resolveProvider: true` (unset, the default,
//! advertises the provider with no `resolveProvider` at all — a server that
//! never offers the second round trip). `completionItem/resolve` itself is
//! always answered regardless of the flag — the flag only controls what a
//! test can observe was *advertised*, matching real servers that answer a
//! request whether or not a client bothered to check the capability first.
//! The reply echoes the item back with an `additionalTextEdits` entry added,
//! simulating the `using` an unimported type's completion brings with it in
//! csharp-ls; a `label` of `"stub/silence"` gets no reply at all, the same
//! never-answer affordance `stub/silence` is elsewhere, so the client's own
//! timeout path is reachable for this method too.
//!
//! C9 added `textDocument/semanticTokens/full`, answered with canned
//! delta-encoded tokens covering the same-line-multiple-tokens and
//! line-advance decoding cases, plus two ways to advertise the capability:
//! `STUB_LSP_SEMANTIC_TOKENS_STATIC` puts `semanticTokensProvider` in
//! `initialize`'s result the way rust-analyzer does, and
//! `stub/semanticTokensRegisterRun` sends a `client/registerCapability`
//! for it instead — the path csharp-ls is believed to use, so a client that
//! only reads the static capability never sees it.
//!
//! C10 added `textDocument/codeLens` and `codeLens/resolve`, plus
//! `STUB_LSP_CODE_LENS_STATIC`/`stub/codeLensRegisterRun` for the same
//! static/dynamic split. The resolved lens's command is `stub.applyEdit` —
//! the same command `workspace/executeCommand`'s handler already blocks on a
//! `workspace/applyEdit` answer for — so running a lens exercises the
//! session gate through the existing path rather than a second one.
//!
//! C11 added the call- and type-hierarchy methods —
//! `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`,
//! `callHierarchy/outgoingCalls`, `textDocument/prepareTypeHierarchy`,
//! `typeHierarchy/supertypes` and `typeHierarchy/subtypes` — with the same
//! static/dynamic split (`STUB_LSP_CALL_HIERARCHY_STATIC`/
//! `stub/callHierarchyRegisterRun`, and the type-hierarchy twins).
//! `outgoingCalls` answers an empty array for the item at line 9, so a test
//! can tell a leaf function's real answer apart from "the server had
//! nothing"; `supertypes` answers `null` for the item at line 99, the case
//! `type_hierarchy_outcome`'s index fallback exists for.

use std::collections::HashMap;
use std::io::{self, BufReader, Stdout, Write};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use lsp_core::framing::{read_message, write_message};
use serde_json::{json, Value};

/// Set to `1` to exit(1) right after answering the first `textDocument/didOpen`,
/// so respawn and backoff can be exercised.
const DIE_ON_DIDOPEN: &str = "STUB_LSP_DIE_ON_DIDOPEN";
/// C7: set to `1` to advertise `completionProvider.resolveProvider: true`
/// in `initialize`'s result.
const COMPLETION_RESOLVE: &str = "STUB_LSP_COMPLETION_RESOLVE";
/// C9: set to `1` to advertise `semanticTokensProvider` statically in
/// `initialize`'s result, the way rust-analyzer does. Unset, a test instead
/// reaches `stub/semanticTokensRegisterRun` for the dynamic-registration
/// path csharp-ls is believed to use.
const SEMANTIC_TOKENS_STATIC: &str = "STUB_LSP_SEMANTIC_TOKENS_STATIC";
/// C10: set to `1` to advertise `codeLensProvider` statically in
/// `initialize`'s result. Unset, a test instead reaches
/// `stub/codeLensRegisterRun` for the dynamic-registration path csharp-ls is
/// believed to use — the same static/dynamic split C9 exercises.
const CODE_LENS_STATIC: &str = "STUB_LSP_CODE_LENS_STATIC";
/// C11: set to `1` to advertise `callHierarchyProvider` statically in
/// `initialize`'s result. Unset, a test instead reaches
/// `stub/callHierarchyRegisterRun` for the dynamic-registration path, the
/// same static/dynamic split C9/C10 exercise for their own features.
const CALL_HIERARCHY_STATIC: &str = "STUB_LSP_CALL_HIERARCHY_STATIC";
/// C11: the type-hierarchy twin of [`CALL_HIERARCHY_STATIC`].
const TYPE_HIERARCHY_STATIC: &str = "STUB_LSP_TYPE_HIERARCHY_STATIC";
/// C9: the legend both the static and dynamic paths advertise — the LSP
/// standard vocabulary, same order `lsp_core::semantic_tokens::STANDARD_TOKEN_TYPES`/
/// `STANDARD_TOKEN_MODIFIERS` define, so a test can index into it by name.
fn semantic_tokens_legend() -> Value {
    json!({
        "tokenTypes": [
            "namespace", "type", "class", "enum", "interface", "struct", "typeParameter",
            "parameter", "variable", "property", "enumMember", "event", "function", "method",
            "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator",
            "decorator",
        ],
        "tokenModifiers": [
            "declaration", "definition", "readonly", "static", "deprecated", "abstract",
            "async", "modification", "documentation", "defaultLibrary",
        ],
    })
}

type Out = Arc<Mutex<Stdout>>;

/// Responses to requests *this server* sent to the client, waiting to be
/// handed back to the thread that is blocked on them.
type Pending = Arc<Mutex<HashMap<i64, Sender<Value>>>>;

/// How long a server-to-client request waits before giving up. Only a
/// deadlocked client ever reaches it, and a test that hangs is worse than a
/// test that fails.
const CLIENT_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

fn send(out: &Out, message: Value) {
    let payload = serde_json::to_vec(&message).expect("serializable");
    let mut guard = out.lock().expect("stdout lock");
    write_message(&mut *guard, &payload).expect("write to stdout");
}

fn main() {
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    // What the client said it can do, kept so a test can assert the
    // advertisement end to end rather than against a private function.
    let client_capabilities: Arc<Mutex<Value>> = Arc::new(Mutex::new(Value::Null));
    // C5: the last `workspace/didChangeWatchedFiles` notification this
    // server was sent, so a test can assert what actually crossed the wire
    // rather than only that `LspManager::did_change_watched_files` returned
    // `Ok`.
    let last_watched_files_change: Arc<Mutex<Value>> = Arc::new(Mutex::new(Value::Null));
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let mut next_request_id = 9100i64;
    let mut input = BufReader::new(io::stdin());
    let die_on_didopen = std::env::var(DIE_ON_DIDOPEN).is_ok_and(|v| v == "1");
    let completion_resolve = std::env::var(COMPLETION_RESOLVE).is_ok_and(|v| v == "1");
    let semantic_tokens_static = std::env::var(SEMANTIC_TOKENS_STATIC).is_ok_and(|v| v == "1");
    let code_lens_static = std::env::var(CODE_LENS_STATIC).is_ok_and(|v| v == "1");
    let call_hierarchy_static = std::env::var(CALL_HIERARCHY_STATIC).is_ok_and(|v| v == "1");
    let type_hierarchy_static = std::env::var(TYPE_HIERARCHY_STATIC).is_ok_and(|v| v == "1");

    while let Some(body) = read_message(&mut input).expect("read from stdin") {
        let message: Value = match serde_json::from_slice(&body) {
            Ok(m) => m,
            Err(_) => continue,
        };
        // A message with an id and no method is the client answering
        // something we asked, not a request: hand it to whoever is waiting.
        if message.get("method").is_none() {
            if let Some(id) = message.get("id").and_then(Value::as_i64) {
                if let Some(tx) = pending.lock().expect("pending lock").remove(&id) {
                    // The whole message, not just its `result`: a client
                    // that answers with an *error* is a different thing
                    // from one that answers `null`, and F0-16's progress
                    // run has to tell them apart.
                    let _ = tx.send(message.clone());
                }
            }
            continue;
        }

        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match (method, id) {
            ("initialize", Some(id)) => {
                *client_capabilities.lock().expect("capabilities lock") =
                    params.get("capabilities").cloned().unwrap_or(Value::Null);
                let mut completion_provider = json!({
                    // L5: the same pair rust-analyzer advertises, so `.` and
                    // (twice over) `::` are covered.
                    "triggerCharacters": [".", ":"],
                });
                if completion_resolve {
                    completion_provider["resolveProvider"] = json!(true);
                }
                let mut capabilities = json!({
                    "textDocumentSync": 1,
                    "completionProvider": completion_provider,
                    // F2: the pair every language agrees on, plus
                    // the closing paren as a retrigger so the nested
                    // -call case is reachable offline.
                    "signatureHelpProvider": {
                        "triggerCharacters": ["(", ","],
                        "retriggerCharacters": [")"],
                    },
                    "documentHighlightProvider": true,
                    "inlayHintProvider": true,
                });
                if semantic_tokens_static {
                    capabilities["semanticTokensProvider"] = json!({
                        "legend": semantic_tokens_legend(),
                        "full": true,
                    });
                }
                if code_lens_static {
                    capabilities["codeLensProvider"] = json!({"resolveProvider": true});
                }
                if call_hierarchy_static {
                    capabilities["callHierarchyProvider"] = json!(true);
                }
                if type_hierarchy_static {
                    capabilities["typeHierarchyProvider"] = json!(true);
                }
                send(
                    &out,
                    json!({"jsonrpc": "2.0", "id": id, "result": {
                        "capabilities": capabilities,
                        "serverInfo": {"name": "stub_server", "version": "0.1.0"},
                    }}),
                )
            }
            // What this client advertised in `initialize`, handed back so a
            // test can assert it.
            ("stub/clientCapabilities", Some(id)) => {
                let capabilities = client_capabilities
                    .lock()
                    .expect("capabilities lock")
                    .clone();
                send(
                    &out,
                    json!({"jsonrpc": "2.0", "id": id, "result": capabilities}),
                );
            }
            ("initialized", _) => {}
            ("workspace/didChangeWatchedFiles", _) => {
                *last_watched_files_change
                    .lock()
                    .expect("watched files lock") = params;
            }
            // What the client last sent via `workspace/didChangeWatchedFiles`,
            // so a test can assert its `changes` array end to end.
            ("stub/lastWatchedFilesChange", Some(id)) => {
                let change = last_watched_files_change
                    .lock()
                    .expect("watched files lock")
                    .clone();
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": change}));
            }
            ("shutdown", Some(id)) => {
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": null}))
            }
            ("exit", _) => return,
            ("textDocument/didOpen", _) => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let version = params.pointer("/textDocument/version").cloned();
                send(
                    &out,
                    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                           "params": {
                        "uri": uri,
                        "version": version,
                        "diagnostics": [canned_diagnostic()],
                    }}),
                );
                if die_on_didopen {
                    // Die mid-session, without an `exit`: the client must see
                    // EOF and respawn us.
                    std::process::exit(1);
                }
            }
            // Echo the params back, optionally after a delay, on its own
            // thread — so a slow request cannot hold up a later fast one and
            // response correlation is actually tested.
            ("stub/echo", Some(id)) => {
                let out = Arc::clone(&out);
                let delay = params.get("delay_ms").and_then(Value::as_u64).unwrap_or(0);
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(delay));
                    send(&out, json!({"jsonrpc": "2.0", "id": id, "result": params}));
                });
            }
            // L3: a hover in every shape a real server might pick, chosen by
            // the requested line so one stub covers the parsing matrix:
            // line 0 -> MarkupContent, line 1 -> a {language, value}
            // MarkedString, line 2 -> an array, anything else -> no hover.
            ("textDocument/hover", Some(id)) => {
                let line = params
                    .pointer("/position/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let contents = match line {
                    0 => {
                        json!({"kind": "markdown", "value": "```rust\nfn main()\n```\nThe **entry** point."})
                    }
                    1 => json!({"language": "rust", "value": "fn main()"}),
                    2 => json!(["plain hover", {"language": "rust", "value": "fn main()"}]),
                    _ => Value::Null,
                };
                let result = if contents.is_null() {
                    Value::Null
                } else {
                    json!({ "contents": contents })
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // L4: one target per requested character, so the single-Location,
            // Location-array and LocationLink-array replies are all reachable.
            ("textDocument/definition", Some(id)) => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let character = params
                    .pointer("/position/character")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let result = match character {
                    0 => location(&uri, 3, 4),
                    1 => json!([location(&uri, 3, 4), location(&uri, 9, 2)]),
                    2 => json!([{
                        "targetUri": uri,
                        "targetRange": {"start": {"line": 3, "character": 0},
                                        "end": {"line": 5, "character": 1}},
                        "targetSelectionRange": {"start": {"line": 3, "character": 4},
                                                 "end": {"line": 3, "character": 8}},
                    }]),
                    _ => Value::Null,
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // L5: both response shapes, chosen by the requested line —
            // line 0 -> a bare CompletionItem[], line 1 -> a CompletionList
            // that is incomplete and carries a snippet item and a textEdit
            // item, line 6 -> R2's CamelHumps fixture, anything else ->
            // null (nothing to complete).
            ("textDocument/completion", Some(id)) => {
                let line = params
                    .pointer("/position/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let result = match line {
                    0 => json!([
                        {"label": "push", "kind": 2, "detail": "fn push(&mut self, value: T)",
                         "sortText": "0001"},
                        {"label": "pop", "kind": 2, "sortText": "0000",
                         "documentation": {"kind": "markdown", "value": "Removes the last element."}},
                        {"label": "#[allow]", "kind": 15, "filterText": "allow",
                         "insertText": "#[allow(dead_code)]"},
                    ]),
                    1 => json!({"isIncomplete": true, "items": [
                        {"label": "map", "kind": 3, "insertTextFormat": 2,
                         "insertText": "map(${1:f})$0"},
                        {"label": "max", "kind": 3, "textEdit": {
                            "newText": "max()",
                            "range": {"start": {"line": 1, "character": 2},
                                      "end": {"line": 1, "character": 4}},
                        }},
                    ]}),
                    // R2: "fooBar" has a hump starting with 'f' ("Bar") for
                    // "fBr" to CamelHumps-match; "objBarrel" has none (its
                    // only hump is "Barrel", and 'f' does not appear in it
                    // at all), so it fails every tier.
                    6 => json!([
                        {"label": "fooBar", "kind": 6},
                        {"label": "objBarrel", "kind": 6},
                    ]),
                    _ => Value::Null,
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C7: echo the item back with `additionalTextEdits` added — the
            // `using` an unimported type's completion brings with it,
            // csharp-ls's reason for this request existing at all. A label
            // of `"stub/silence"` gets no reply, so the client's own
            // timeout/`$/cancelRequest` path is reachable for this method.
            ("completionItem/resolve", Some(id)) => {
                if params.get("label").and_then(Value::as_str) == Some("stub/silence") {
                    continue;
                }
                let mut item = params.clone();
                if let Value::Object(map) = &mut item {
                    map.insert(
                        "additionalTextEdits".into(),
                        json!([{
                            "newText": "using System.Collections.Generic;\n",
                            "range": {"start": {"line": 0, "character": 0},
                                      "end": {"line": 0, "character": 0}},
                        }]),
                    );
                }
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": item}));
            }
            // C12: `csharp/metadata` is not a real LSP method — this stub
            // proves `LspManager::fetch_metadata`'s request/response shape
            // against *something*, since it has never been round-tripped
            // against a real csharp-ls (see that method's doc comment). A
            // uri containing "missing" answers with no "source" field at
            // all, exercising the clean-refusal path.
            ("csharp/metadata", Some(id)) => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let result = if uri.contains("missing") {
                    json!({})
                } else {
                    json!({"source": format!("// decompiled from {uri}\nclass Stub {{}}\n")})
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // F2-5: one signature-help shape per requested line —
            // line 0 -> offset parameter labels, line 1 -> substring labels,
            // line 2 -> an overload set whose second signature carries its
            // own `activeParameter` (the case where the two indices
            // disagree), line 3 -> a *malformed* reply: `signatures` is an
            // object, which no server should send and the client must
            // survive, line 4 -> an active index past the end of the
            // signature's own parameters, anything else -> null.
            ("textDocument/signatureHelp", Some(id)) => {
                let result = match position_line(&params) {
                    0 => json!({
                        "signatures": [{
                            "label": "fn push(&mut self, value: T)",
                            "documentation": "Appends an element.",
                            "parameters": [{"label": [8, 17]}, {"label": [19, 27]}],
                        }],
                        "activeSignature": 0,
                        "activeParameter": 1,
                    }),
                    1 => json!({
                        "signatures": [{
                            "label": "fn insert(index: usize, value: T)",
                            "parameters": [{"label": "index: usize"}, {"label": "value: T"}],
                        }],
                        "activeParameter": 0,
                    }),
                    2 => json!({
                        "signatures": [
                            {"label": "f(a, b)",
                             "parameters": [{"label": "a"}, {"label": "b"}]},
                            {"label": "f(a)", "parameters": [{"label": "a"}],
                             "activeParameter": 0},
                        ],
                        "activeSignature": 1,
                        "activeParameter": 1,
                    }),
                    3 => json!({"signatures": {"label": "not an array"}}),
                    4 => json!({
                        "signatures": [{"label": "f(a)", "parameters": [{"label": "a"}]}],
                        "activeParameter": 9,
                    }),
                    _ => Value::Null,
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // F2-6: highlights, one kind per requested line, plus a
            // kind-less entry (line 2) and an entry with no range at all
            // (line 3) — the malformed shape a real server will not produce
            // on demand.
            ("textDocument/documentHighlight", Some(id)) => {
                let result = match position_line(&params) {
                    0 => json!([
                        highlight(1, Some(1)),
                        highlight(4, Some(2)),
                        highlight(7, Some(3))
                    ]),
                    1 => json!([highlight(2, Some(9))]),
                    2 => json!([highlight(3, None)]),
                    3 => json!([{"kind": 2}]),
                    _ => Value::Null,
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // F2-6: inlay hints for the requested line range only — the
            // reply echoes the range's own lines, so a test can prove the
            // client asked for a viewport and not for the whole file. A
            // request whose range starts on line 900 gets a malformed hint
            // (a label of the wrong type) instead.
            ("textDocument/inlayHint", Some(id)) => {
                let first = params
                    .pointer("/range/start/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let last = params
                    .pointer("/range/end/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(first);
                let result = if first == 900 {
                    json!([{"position": {"line": 900, "character": 0}, "label": 42}])
                } else {
                    json!([
                        {"position": {"line": first, "character": 9}, "label": ": i32",
                         "kind": 1, "paddingLeft": true},
                        {"position": {"line": last, "character": 4},
                         "label": [{"value": "value"}, {"value": ":"}],
                         "kind": 2, "paddingRight": true},
                    ])
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C9: canned tokens covering the delta-decoding cases that
            // matter — same-line multiple tokens and a line advance —
            // against the legend `semantic_tokens_legend()` advertises:
            // (line 0, char 0, len 3, type "type" idx 1) then
            // (line 0, char +4, len 6, type "function" idx 12, modifier
            // "defaultLibrary" bit 9) then (line 1, char 0, len 4, type
            // "keyword" idx 15). `uri == "file:///stub/empty.rs"` answers
            // `null`, so a server with nothing to say for a document is
            // reachable too.
            ("textDocument/semanticTokens/full", Some(id)) => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let result = if uri.ends_with("empty.rs") {
                    Value::Null
                } else {
                    json!({"resultId": "1", "data": [
                        0, 0, 3, 1, 0,
                        0, 4, 6, 12, 1 << 9,
                        1, 0, 4, 15, 0,
                    ]})
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C9: the dynamic-registration path a client must also handle —
            // csharp-ls is believed to declare `textDocument/semanticTokens`
            // this way rather than in `initialize`'s static result. Answers
            // once the registration round-trips, the same shape
            // `stub/registerCapabilityRun` uses.
            ("stub/semanticTokensRegisterRun", Some(id)) => {
                let register_id = next_request_id;
                next_request_id += 1;
                let (register_tx, register_rx) = channel();
                pending
                    .lock()
                    .expect("pending lock")
                    .insert(register_id, register_tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": register_id,
                               "method": "client/registerCapability", "params": {
                            "registrations": [{
                                "id": "stub-semantic-tokens",
                                "method": "textDocument/semanticTokens",
                                "registerOptions": {
                                    "legend": semantic_tokens_legend(),
                                    "full": true,
                                },
                            }],
                        }}),
                    );
                    let register_answer = register_rx
                        .recv_timeout(CLIENT_REPLY_TIMEOUT)
                        .unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&register_id);
                    let registered = register_answer.get("id").is_some()
                        && register_answer.get("error").is_none();
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"registered": registered}}),
                    );
                });
            }
            // C10: two lenses — one already carries its `command` (a plain
            // `workspace/executeCommand`, `stub.plain`, which the generic
            // handler below answers with `null`), the other only `data`,
            // csharp-ls's lazy-resolve path, which `codeLens/resolve` below
            // fills in with `stub.applyEdit` — the same command
            // `workspace/executeCommand`'s handler already blocks on a
            // client answer for, so a lens click exercises the applyEdit
            // gate through the same path a code action's command does,
            // rather than a second command of its own.
            ("textDocument/codeLens", Some(id)) => {
                let result = json!([
                    {"range": {"start": {"line": 0, "character": 0},
                               "end": {"line": 0, "character": 3}},
                     "command": {"title": "1 reference", "command": "stub.plain",
                                 "arguments": [1]}},
                    {"range": {"start": {"line": 5, "character": 0},
                               "end": {"line": 5, "character": 1}},
                     "data": {"token": "resolve-me"}},
                ]);
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // The unresolved lens from line 5 above, with its command filled
            // in. The item comes back whole, same convention
            // `codeAction/resolve` follows.
            ("codeLens/resolve", Some(id)) => {
                let mut lens = params.clone();
                lens["command"] = json!({"title": "Run", "command": "stub.applyEdit",
                                          "arguments": ["file:///workspace/main.rs"]});
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": lens}));
            }
            // C10: the dynamic-registration path — csharp-ls is believed to
            // declare `textDocument/codeLens` this way rather than in
            // `initialize`'s static result. Same shape
            // `stub/semanticTokensRegisterRun` uses.
            ("stub/codeLensRegisterRun", Some(id)) => {
                let register_id = next_request_id;
                next_request_id += 1;
                let (register_tx, register_rx) = channel();
                pending
                    .lock()
                    .expect("pending lock")
                    .insert(register_id, register_tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": register_id,
                               "method": "client/registerCapability", "params": {
                            "registrations": [{
                                "id": "stub-code-lens",
                                "method": "textDocument/codeLens",
                                "registerOptions": {"resolveProvider": true},
                            }],
                        }}),
                    );
                    let register_answer = register_rx
                        .recv_timeout(CLIENT_REPLY_TIMEOUT)
                        .unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&register_id);
                    let registered = register_answer.get("id").is_some()
                        && register_answer.get("error").is_none();
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"registered": registered}}),
                    );
                });
            }
            // C11: line 99 answers `null` — "no call hierarchy here", a
            // valid answer for a position that is not a callable symbol.
            // Any other line answers with one item, `DoWork` in
            // `main.cs`/`main.rs` (whichever `uri` was asked about).
            ("textDocument/prepareCallHierarchy", Some(id)) => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let line = params
                    .pointer("/position/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let result = match line {
                    99 => Value::Null,
                    // The leaf function `outgoingCalls` reports as calling
                    // nothing further — see that handler below.
                    9 => json!([hierarchy_item(uri, "Helper", 12, 9)]),
                    _ => json!([hierarchy_item(uri, "DoWork", 12, 3)]),
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C11: one caller, `Main`, with one call site — `fromRanges` is
            // never empty here (the empty case is `outgoingCalls`' below,
            // which is the one that matters for the fallback-vs-real-
            // answer distinction).
            ("callHierarchy/incomingCalls", Some(id)) => {
                let uri = params
                    .pointer("/item/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let result = json!([{
                    "from": hierarchy_item(uri, "Main", 12, 9),
                    "fromRanges": [{"start": {"line": 9, "character": 4},
                                    "end": {"line": 9, "character": 10}}],
                }]);
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C11: `DoWork` (line 3) calls one thing, `Helper`; asking about
            // `Helper` itself (line 9, `stub/outgoingLeaf`'s target) answers
            // with an empty array — a leaf function's real, non-fallback
            // answer, per the plan's own distinction.
            ("callHierarchy/outgoingCalls", Some(id)) => {
                let uri = params
                    .pointer("/item/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let line = params
                    .pointer("/item/range/start/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let result = if line == 9 {
                    json!([])
                } else {
                    json!([{
                        "to": hierarchy_item(uri, "Helper", 12, 9),
                        "fromRanges": [{"start": {"line": 3, "character": 8},
                                        "end": {"line": 3, "character": 14}}],
                    }])
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C11: the dynamic-registration path for call hierarchy — same
            // shape as `stub/codeLensRegisterRun`.
            ("stub/callHierarchyRegisterRun", Some(id)) => {
                let register_id = next_request_id;
                next_request_id += 1;
                let (register_tx, register_rx) = channel();
                pending
                    .lock()
                    .expect("pending lock")
                    .insert(register_id, register_tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": register_id,
                               "method": "client/registerCapability", "params": {
                            "registrations": [{
                                "id": "stub-call-hierarchy",
                                "method": "textDocument/prepareCallHierarchy",
                                "registerOptions": {},
                            }],
                        }}),
                    );
                    let register_answer = register_rx
                        .recv_timeout(CLIENT_REPLY_TIMEOUT)
                        .unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&register_id);
                    let registered = register_answer.get("id").is_some()
                        && register_answer.get("error").is_none();
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"registered": registered}}),
                    );
                });
            }
            // C11: line 99 answers `null`, same convention as
            // `prepareCallHierarchy`. Any other line answers with `Circle`.
            ("textDocument/prepareTypeHierarchy", Some(id)) => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let line = params
                    .pointer("/position/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let result = if line == 99 {
                    Value::Null
                } else {
                    json!([hierarchy_item(uri, "Circle", 5, 3)])
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C11: `Circle`'s one supertype, `Shape` — line 99 answers
            // `null`, the case `type_hierarchy_outcome`'s index fallback
            // reaches for.
            ("typeHierarchy/supertypes", Some(id)) => {
                let uri = params
                    .pointer("/item/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let line = params
                    .pointer("/item/range/start/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let result = if line == 99 {
                    Value::Null
                } else {
                    json!([hierarchy_item(uri, "Shape", 5, 0)])
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // C11: `Circle`'s subtypes — empty, since the stub gives it none.
            ("typeHierarchy/subtypes", Some(id)) => {
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": []}));
            }
            // C11: the dynamic-registration path for type hierarchy — same
            // shape as `stub/callHierarchyRegisterRun`.
            ("stub/typeHierarchyRegisterRun", Some(id)) => {
                let register_id = next_request_id;
                next_request_id += 1;
                let (register_tx, register_rx) = channel();
                pending
                    .lock()
                    .expect("pending lock")
                    .insert(register_id, register_tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": register_id,
                               "method": "client/registerCapability", "params": {
                            "registrations": [{
                                "id": "stub-type-hierarchy",
                                "method": "textDocument/prepareTypeHierarchy",
                                "registerOptions": {},
                            }],
                        }}),
                    );
                    let register_answer = register_rx
                        .recv_timeout(CLIENT_REPLY_TIMEOUT)
                        .unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&register_id);
                    let registered = register_answer.get("id").is_some()
                        && register_answer.get("error").is_none();
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"registered": registered}}),
                    );
                });
            }
            // F2: a request that is simply never answered, so the client's
            // timeout and its `$/cancelRequest` are reachable offline. No
            // real server can be asked to do this on demand.
            ("stub/silence", Some(_id)) => {}
            // F2: a reply that arrives *after* the request that asked for it
            // was superseded — the same id answered twice, late. A client
            // that correlates by id must ignore the second one rather than
            // hand it to whoever asks next.
            ("stub/lateDuplicate", Some(id)) => {
                let out = Arc::clone(&out);
                let delay = params.get("delay_ms").and_then(Value::as_u64).unwrap_or(50);
                thread::spawn(move || {
                    send(&out, json!({"jsonrpc": "2.0", "id": id, "result": "first"}));
                    thread::sleep(Duration::from_millis(delay));
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": "superseded"}),
                    );
                });
            }
            // F2: a well-framed message that is not JSON-RPC at all, sent
            // unsolicited. The client must skip it and keep serving the
            // request that follows.
            ("stub/garbage", Some(id)) => {
                send(
                    &out,
                    json!({"jsonrpc": "2.0", "result": "no id, no method"}),
                );
                send(
                    &out,
                    json!({"jsonrpc": "2.0", "id": id, "result": "still alive"}),
                );
            }
            // F0-16: the whole sequence a real indexing server sends — a
            // `window/workDoneProgress/create` request, then begin / report
            // / end on the token it created, and only then the answer to
            // this request, so a test that has the answer knows every
            // notification is already through the client's reader.
            //
            // `created` reports whether the client accepted the create.
            // A client that answers it with "not implemented" is what makes
            // a real server give up on reporting at all, so that regression
            // fails here rather than silently reappearing.
            ("stub/indexingRun", Some(id)) => {
                let token = "stub-index";
                // `{"finish": false}` leaves the token open, which is what a
                // server that dies mid-index leaves behind.
                let finish = params
                    .get("finish")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let request_id = next_request_id;
                next_request_id += 1;
                let (tx, rx) = channel();
                pending.lock().expect("pending lock").insert(request_id, tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": request_id,
                               "method": "window/workDoneProgress/create",
                               "params": {"token": token}}),
                    );
                    let answer = rx.recv_timeout(CLIENT_REPLY_TIMEOUT).unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&request_id);
                    let created = answer.get("id").is_some() && answer.get("error").is_none();

                    send(
                        &out,
                        progress(
                            token,
                            json!({"kind": "begin", "title": "Indexing",
                                               "percentage": 0}),
                        ),
                    );
                    send(
                        &out,
                        progress(token, json!({"kind": "report", "percentage": 60})),
                    );
                    if finish {
                        send(&out, progress(token, json!({"kind": "end"})));
                    }
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"created": created}}),
                    );
                });
            }
            // C4: the sequence csharp-ls actually runs — register
            // `workspace/didChangeWatchedFiles` right after `initialized`,
            // then later unregister the same id. Answered only once both
            // requests have gone out and come back, so a test holding the
            // answer knows the whole round trip already went through the
            // client's reader thread.
            ("stub/registerCapabilityRun", Some(id)) => {
                let register_id = next_request_id;
                let unregister_id = next_request_id + 1;
                next_request_id += 2;

                let (register_tx, register_rx) = channel();
                let (unregister_tx, unregister_rx) = channel();
                pending
                    .lock()
                    .expect("pending lock")
                    .insert(register_id, register_tx);
                pending
                    .lock()
                    .expect("pending lock")
                    .insert(unregister_id, unregister_tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                let params = params.clone();
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": register_id,
                               "method": "client/registerCapability", "params": {
                            "registrations": [{
                                "id": "stub-watch",
                                "method": "workspace/didChangeWatchedFiles",
                                "registerOptions": {"watchers": [
                                    {"globPattern": "**/*.rs", "kind": 7},
                                ]},
                            }],
                        }}),
                    );
                    let register_answer = register_rx
                        .recv_timeout(CLIENT_REPLY_TIMEOUT)
                        .unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&register_id);
                    let registered = register_answer.get("id").is_some()
                        && register_answer.get("error").is_none();

                    // A test hunting for the window in which the
                    // registration is live gets to choose how wide it is.
                    let pause_ms = params.get("pause_ms").and_then(Value::as_u64).unwrap_or(0);
                    thread::sleep(Duration::from_millis(pause_ms));

                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": unregister_id,
                               "method": "client/unregisterCapability", "params": {
                            "unregisterations": [
                                {"id": "stub-watch", "method": "workspace/didChangeWatchedFiles"},
                            ],
                        }}),
                    );
                    let unregister_answer = unregister_rx
                        .recv_timeout(CLIENT_REPLY_TIMEOUT)
                        .unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&unregister_id);
                    let unregistered = unregister_answer.get("id").is_some()
                        && unregister_answer.get("error").is_none();

                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {
                            "registered": registered,
                            "unregistered": unregistered,
                        }}),
                    );
                });
            }
            // C6: pull configuration for two sections — `"csharp"`, which a
            // test's `ServerConfig` is configured under, and `"other"`,
            // which nothing is — so both the matched and null-fallback
            // paths of `workspace/configuration` are exercised through the
            // real reader thread. Answered only once the pull's reply is
            // back, so a test holding the answer knows the round trip
            // already completed.
            ("stub/configurationRun", Some(id)) => {
                let request_id = next_request_id;
                next_request_id += 1;

                let (tx, rx) = channel();
                pending.lock().expect("pending lock").insert(request_id, tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": request_id,
                               "method": "workspace/configuration", "params": {
                            "items": [
                                {"scopeUri": Value::Null, "section": "csharp"},
                                {"scopeUri": Value::Null, "section": "other"},
                            ],
                        }}),
                    );
                    let answer = rx.recv_timeout(CLIENT_REPLY_TIMEOUT).unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&request_id);
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id,
                               "result": answer.get("result").cloned().unwrap_or(Value::Null)}),
                    );
                });
            }
            // Ask the client something it does not implement, to check we
            // answer server-to-client requests instead of hanging.
            ("stub/askClient", Some(id)) => {
                let out = Arc::clone(&out);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": 9001,
                                      "method": "workspace/configuration", "params": {}}),
                    );
                    send(&out, json!({"jsonrpc": "2.0", "id": id, "result": "asked"}));
                });
            }
            // RF3: code actions, chosen by the requested range's start line so
            // one stub covers the shapes the parser has to handle —
            // line 0 -> a CodeAction carrying its own edit,
            // line 1 -> a bare Command and a CodeAction carrying a command,
            // line 2 -> an unresolved item (no edit, no command, has `data`),
            // line 3 -> a disabled item,
            // anything else -> no actions here.
            ("textDocument/codeAction", Some(id)) => {
                let uri = uri_of(&params);
                let line = params
                    .pointer("/range/start/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let end_line = params
                    .pointer("/range/end/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(line);
                // A filter for a kind this server does not recognise, which
                // it answers by saying nothing at all — the behaviour
                // `needs_unfiltered_retry` exists for. A filter it does
                // recognise (`refactor.extract`) is honoured as before.
                let unrecognised_filter = params
                    .pointer("/context/only")
                    .and_then(Value::as_array)
                    .is_some_and(|only| only.contains(&json!("source.organizeImports")));
                let with_diagnostics = params
                    .pointer("/context/diagnostics")
                    .and_then(Value::as_array)
                    .is_some_and(|d| !d.is_empty());
                if unrecognised_filter {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": Value::Null}),
                    );
                    io::stdout().flush().ok();
                    continue;
                }
                if end_line > line {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": [
                            {"title": "Organize imports", "kind": "source.organizeImports",
                             "edit": {"documentChanges": []}},
                            {"title": "Extract into function",
                             "kind": "refactor.extract.function"},
                        ]}),
                    );
                    io::stdout().flush().ok();
                    continue;
                }
                // F2-4: line 4 answers the two intention requests
                // differently — the diagnostic-scoped one offers a preferred
                // quick fix, both offer the same refactoring, and the merged
                // list must contain that refactoring exactly once.
                if line == 4 {
                    let result = if with_diagnostics {
                        json!([
                            {"title": "Import `HashMap`", "kind": "quickfix",
                             "isPreferred": true, "edit": {"documentChanges": []}},
                            {"title": "Extract into function",
                             "kind": "refactor.extract.function",
                             "data": {"scope": "diagnostic"}},
                        ])
                    } else {
                        json!([
                            {"title": "Extract into function",
                             "kind": "refactor.extract.function",
                             "data": {"scope": "range"}},
                            {"title": "Organize imports", "kind": "source.organizeImports"},
                        ])
                    };
                    send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
                    io::stdout().flush().ok();
                    continue;
                }
                let result = match line {
                    0 => json!([{
                        "title": "Extract into function",
                        "kind": "refactor.extract.function",
                        "edit": {"documentChanges": [{
                            "textDocument": {"uri": uri, "version": 1},
                            "edits": [{
                                "range": {"start": {"line": 0, "character": 0},
                                          "end": {"line": 0, "character": 3}},
                                "newText": "extracted()",
                            }],
                        }]},
                    }]),
                    1 => json!([
                        {"title": "Run the command directly",
                         "command": "stub.plain", "arguments": [1]},
                        {"title": "Extract class",
                         "kind": "refactor.extract.class",
                         "command": {"title": "Extract class",
                                     "command": "stub.applyEdit", "arguments": [uri]}},
                    ]),
                    2 => json!([{
                        "title": "Needs resolving",
                        "kind": "refactor.inline",
                        "data": {"token": 42},
                    }]),
                    3 => json!([{
                        "title": "Not available here",
                        "kind": "refactor.extract",
                        "disabled": {"reason": "selection is not an expression"},
                    }]),
                    // F2-3: a resource operation ahead of its text edits, in
                    // one `documentChanges` array — "move to a new file"
                    // creates the sibling, writes its content, and trims
                    // what moved out of the original, the shape a real
                    // extract-to-module refactoring takes.
                    5 => {
                        let new_uri = format!(
                            "{}/extracted.rs",
                            &uri[..uri.rfind('/').unwrap_or(uri.len())]
                        );
                        json!([{
                            "title": "Move to new file",
                            "kind": "refactor.move",
                            "edit": {"documentChanges": [
                                {"kind": "create", "uri": new_uri},
                                {"textDocument": {"uri": new_uri, "version": Value::Null},
                                 "edits": [{
                                     "range": {"start": {"line": 0, "character": 0},
                                               "end": {"line": 0, "character": 0}},
                                     "newText": "// moved here\n",
                                 }]},
                                {"textDocument": {"uri": uri, "version": 1},
                                 "edits": [{
                                     "range": {"start": {"line": 5, "character": 0},
                                               "end": {"line": 5, "character": 1}},
                                     "newText": "moved",
                                 }]},
                            ]},
                        }])
                    }
                    _ => Value::Null,
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // The unresolved item from line 2 above, with its edit filled in.
            ("codeAction/resolve", Some(id)) => {
                let mut action = params.clone();
                action["edit"] = json!({"changes": {
                    "file:///stub/resolved.rs": [{
                        "range": {"start": {"line": 2, "character": 0},
                                  "end": {"line": 2, "character": 1}},
                        "newText": "resolved",
                    }],
                }});
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": action}));
            }
            // RF3: prepareRename, one shape per requested line —
            // 0 -> a bare Range, 1 -> {range, placeholder},
            // 2 -> {defaultBehavior}, anything else -> null (cannot rename).
            ("textDocument/prepareRename", Some(id)) => {
                let line = position_line(&params);
                let range = json!({"start": {"line": line, "character": 4},
                                   "end": {"line": line, "character": 8}});
                let result = match line {
                    0 => range,
                    1 => json!({"range": range, "placeholder": "old_name"}),
                    2 => json!({"defaultBehavior": true}),
                    _ => Value::Null,
                };
                send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            // F1: formatting, keyed by the tab size so a test can pick a
            // behaviour without a second document. 2 -> edits, 4 -> an empty
            // list (already formatted), 8 -> null, anything else -> a
            // MethodNotFound error, which is what a server that does not
            // implement formatting actually sends. A real server will not
            // produce these four on demand, which is the whole reason the
            // stub exists.
            ("textDocument/formatting", Some(id)) => {
                let tab_size = params
                    .get("options")
                    .and_then(|o| o.get("tabSize"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let range = json!({"start": {"line": 0, "character": 0},
                                   "end": {"line": 0, "character": 4}});
                match tab_size {
                    2 => send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id,
                               "result": [{"range": range, "newText": "  "}]}),
                    ),
                    4 => send(&out, json!({"jsonrpc": "2.0", "id": id, "result": []})),
                    8 => send(&out, json!({"jsonrpc": "2.0", "id": id, "result": null})),
                    _ => send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "error": {
                            "code": -32601,
                            "message": "textDocument/formatting is not implemented",
                        }}),
                    ),
                }
            }
            // F1: range formatting is never implemented by the stub, so the
            // client's fall back to whole-document formatting is exercised.
            ("textDocument/rangeFormatting", Some(id)) => {
                send(
                    &out,
                    json!({"jsonrpc": "2.0", "id": id, "error": {
                        "code": -32601,
                        "message": "textDocument/rangeFormatting is not implemented",
                    }}),
                );
            }
            // RF3: rename, again by line — 0 -> a versioned documentChanges
            // edit, 1 -> a legacy `changes` edit, 2 -> null (nothing to do),
            // anything else -> an error response.
            ("textDocument/rename", Some(id)) => {
                let uri = uri_of(&params);
                let new_name = params
                    .get("newName")
                    .and_then(Value::as_str)
                    .unwrap_or("new_name")
                    .to_string();
                let range = json!({"start": {"line": 0, "character": 4},
                                   "end": {"line": 0, "character": 8}});
                match position_line(&params) {
                    0 => send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"documentChanges": [{
                            "textDocument": {"uri": uri, "version": 1},
                            "edits": [{"range": range, "newText": new_name}],
                        }]}}),
                    ),
                    1 => send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"changes": {
                            uri: [{"range": range, "newText": new_name}],
                        }}}),
                    ),
                    2 => send(&out, json!({"jsonrpc": "2.0", "id": id, "result": null})),
                    _ => send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "error": {
                            "code": -32602, "message": "cannot rename this element",
                        }}),
                    ),
                }
            }
            // RF3: the command-driven refactoring shape. `stub.applyEdit`
            // asks the client to apply an edit and blocks on the answer
            // before completing, which is exactly what jdtls, csharp-ls and
            // intelephense do for Extract — and the reason the client may
            // not answer server requests on its read thread.
            ("workspace/executeCommand", Some(id)) => {
                let command = params
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if command != "stub.applyEdit" {
                    send(&out, json!({"jsonrpc": "2.0", "id": id, "result": null}));
                    io::stdout().flush().ok();
                    continue;
                }
                let uri = params
                    .pointer("/arguments/0")
                    .and_then(Value::as_str)
                    .unwrap_or("file:///stub/commanded.rs")
                    .to_string();
                let request_id = next_request_id;
                next_request_id += 1;

                let (tx, rx) = channel();
                pending.lock().expect("pending lock").insert(request_id, tx);

                let out = Arc::clone(&out);
                let pending = Arc::clone(&pending);
                thread::spawn(move || {
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": request_id,
                               "method": "workspace/applyEdit", "params": {
                            "label": "Extract class",
                            "edit": {"documentChanges": [{
                                "textDocument": {"uri": uri, "version": 1},
                                "edits": [{
                                    "range": {"start": {"line": 0, "character": 0},
                                              "end": {"line": 0, "character": 0}},
                                    "newText": "class Extracted {}\n",
                                }],
                            }]},
                        }}),
                    );
                    let answer = rx.recv_timeout(CLIENT_REPLY_TIMEOUT).unwrap_or(Value::Null);
                    pending.lock().expect("pending lock").remove(&request_id);
                    send(
                        &out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {
                            "clientApplied": answer.pointer("/result/applied").and_then(Value::as_bool),
                        }}),
                    );
                });
            }
            (_, Some(id)) => send(
                &out,
                json!({"jsonrpc": "2.0", "id": id, "error": {
                    "code": -32601, "message": format!("{method} is not implemented"),
                }}),
            ),
            (_, None) => {}
        }
        io::stdout().flush().ok();
    }
}

/// One `$/progress` notification for `token`.
fn progress(token: &str, value: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": "$/progress",
           "params": {"token": token, "value": value}})
}

/// The `textDocument.uri` of a request, or a stand-in when it has none.
fn uri_of(params: &Value) -> String {
    params
        .pointer("/textDocument/uri")
        .and_then(Value::as_str)
        .unwrap_or("file:///stub/main.rs")
        .to_string()
}

/// The 0-based line of a request's `position`, which is what every canned
/// answer above is keyed by.
fn position_line(params: &Value) -> u64 {
    params
        .pointer("/position/line")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// A `DocumentHighlight` on a 0-based line, optionally with a kind.
fn highlight(line: u64, kind: Option<u64>) -> Value {
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
fn location(uri: &str, line: u64, character: u64) -> Value {
    json!({"uri": uri, "range": {
        "start": {"line": line, "character": character},
        "end": {"line": line, "character": character + 4},
    }})
}

/// C11: a `CallHierarchyItem`/`TypeHierarchyItem` — the two are the same
/// shape on the wire, so one builder covers both stub features.
fn hierarchy_item(uri: &str, name: &str, kind: u64, line: u64) -> Value {
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
fn canned_diagnostic() -> Value {
    json!({
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 4}},
        "severity": 1,
        "source": "stub_server",
        "message": "canned diagnostic",
    })
}
