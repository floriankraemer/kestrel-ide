//! Merging the answers of several servers into the one the caller expects
//! (ADR-0066). Pure functions over JSON, so the rules are testable without a
//! process. Servers are given in answer order and the first to offer a thing
//! wins, so the order the user configured is also the order of preference.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::routing::tag_origin;

/// Merge `answers` (server id, result) for a [`crate::routing::Route::Merge`]
/// method into one result of the shape a single server would have returned.
pub fn merge(method: &str, answers: Vec<(String, Value)>) -> Value {
    match method {
        "textDocument/completion" => merge_completion(answers),
        "textDocument/codeAction" => merge_code_actions(answers),
        "textDocument/references" => merge_unique(answers, location_key),
        "workspace/symbol" => merge_unique(answers, symbol_key),
        // Not a merge method: the first answer is the answer.
        _ => answers.into_iter().next().map_or(Value::Null, |(_, v)| v),
    }
}

/// The entries of an array-or-null answer.
fn entries(answer: Value) -> Vec<Value> {
    match answer {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

/// Union of `CompletionItem[]` / `CompletionList` answers as one list, an
/// item once per `(label, kind)`, each tagged with its server so
/// `completionItem/resolve` goes back to it. The list is incomplete if any
/// part of it is.
fn merge_completion(answers: Vec<(String, Value)>) -> Value {
    let mut incomplete = false;
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for (server, answer) in answers {
        let items = match answer {
            Value::Object(mut list) => {
                incomplete |= list
                    .get("isIncomplete")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                match list.remove("items") {
                    Some(Value::Array(items)) => items,
                    _ => Vec::new(),
                }
            }
            other => entries(other),
        };
        for mut item in items {
            let key = (
                item.get("label")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                item.get("kind").and_then(Value::as_u64),
            );
            if seen.insert(key) {
                tag_origin(&mut item, &server);
                merged.push(item);
            }
        }
    }
    json!({"isIncomplete": incomplete, "items": merged})
}

/// Union of code actions, an action once per `(title, kind)`, each tagged
/// with its server so `codeAction/resolve` goes back to it.
fn merge_code_actions(answers: Vec<(String, Value)>) -> Value {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for (server, answer) in answers {
        for mut action in entries(answer) {
            let key = (
                action
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                action
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
            if seen.insert(key) {
                tag_origin(&mut action, &server);
                merged.push(action);
            }
        }
    }
    Value::Array(merged)
}

/// Union of arrays of entries, an entry once per `key`.
fn merge_unique(answers: Vec<(String, Value)>, key: fn(&Value) -> String) -> Value {
    let mut seen = HashSet::new();
    let merged: Vec<Value> = answers
        .into_iter()
        .flat_map(|(_, answer)| entries(answer))
        .filter(|entry| seen.insert(key(entry)))
        .collect();
    Value::Array(merged)
}

/// `Location` and `LocationLink` both reduce to a uri and a range.
fn location_key(location: &Value) -> String {
    let uri = location.get("uri").or_else(|| location.get("targetUri"));
    let range = location
        .get("range")
        .or_else(|| location.get("targetSelectionRange"));
    format!("{uri:?}{range:?}")
}

/// `SymbolInformation` and `WorkspaceSymbol` (whose range may be absent).
fn symbol_key(symbol: &Value) -> String {
    let location = symbol.get("location").unwrap_or(&Value::Null);
    format!(
        "{:?}{:?}{}",
        symbol.get("name"),
        symbol.get("kind"),
        location_key(location)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::ORIGIN_KEY;

    fn pair(id: &str, v: Value) -> (String, Value) {
        (id.to_string(), v)
    }

    #[test]
    fn completion_is_deduped_by_label_and_kind_with_the_first_server_winning() {
        let merged = merge(
            "textDocument/completion",
            vec![
                pair(
                    "a",
                    json!([{"label": "foo", "kind": 2, "detail": "from a"}]),
                ),
                pair(
                    "b",
                    json!({"isIncomplete": false, "items": [
                        {"label": "foo", "kind": 2, "detail": "from b"},
                        {"label": "foo", "kind": 7},
                        {"label": "bar", "kind": 2},
                    ]}),
                ),
            ],
        );
        let items = merged["items"].as_array().unwrap();
        let summary: Vec<_> = items
            .iter()
            .map(|i| {
                (
                    i["label"].as_str().unwrap(),
                    i[ORIGIN_KEY].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(summary, [("foo", "a"), ("foo", "b"), ("bar", "b")]);
        assert_eq!(items[0]["detail"], "from a");
    }

    #[test]
    fn a_completion_list_is_incomplete_if_any_part_is() {
        let merged = merge(
            "textDocument/completion",
            vec![
                pair("a", json!({"isIncomplete": true, "items": []})),
                pair("b", json!(null)),
            ],
        );
        assert_eq!(merged["isIncomplete"], true);
        assert_eq!(merged["items"], json!([]));
    }

    #[test]
    fn code_actions_are_unioned_deduped_by_title_and_kind_and_tagged() {
        let merged = merge(
            "textDocument/codeAction",
            vec![
                pair("a", json!([{"title": "Import X", "kind": "quickfix"}])),
                pair(
                    "b",
                    json!([
                        {"title": "Import X", "kind": "quickfix"},
                        {"title": "Import X", "kind": "refactor"},
                        {"title": "Run", "command": "b.run"},
                    ]),
                ),
            ],
        );
        let tags: Vec<_> = merged
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                (
                    a["title"].as_str().unwrap(),
                    a[ORIGIN_KEY].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(tags, [("Import X", "a"), ("Import X", "b"), ("Run", "b")]);
    }

    #[test]
    fn references_are_deduped_by_uri_and_range() {
        let at = |uri: &str, line: u64| {
            json!({"uri": uri, "range": {"start": {"line": line, "character": 0},
                                         "end": {"line": line, "character": 3}}})
        };
        let merged = merge(
            "textDocument/references",
            vec![
                pair("a", json!([at("file:///x", 1), at("file:///y", 2)])),
                pair("b", json!([at("file:///x", 1), at("file:///x", 5)])),
            ],
        );
        assert_eq!(merged.as_array().unwrap().len(), 3);
    }

    #[test]
    fn a_location_link_and_a_location_to_the_same_place_are_one_reference() {
        let range =
            json!({"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}});
        let merged = merge(
            "textDocument/references",
            vec![
                pair("a", json!([{"uri": "file:///x", "range": range}])),
                pair(
                    "b",
                    json!([{"targetUri": "file:///x", "targetSelectionRange": range,
                            "targetRange": range}]),
                ),
            ],
        );
        assert_eq!(merged.as_array().unwrap().len(), 1);
    }

    #[test]
    fn workspace_symbols_are_deduped_by_name_kind_and_place() {
        let sym = |name: &str, line: u64| {
            json!({"name": name, "kind": 5, "location": {"uri": "file:///x",
                "range": {"start": {"line": line, "character": 0},
                          "end": {"line": line, "character": 1}}}})
        };
        let merged = merge(
            "workspace/symbol",
            vec![
                pair("a", json!([sym("Foo", 1)])),
                pair("b", json!([sym("Foo", 1), sym("Foo", 9), sym("Bar", 1)])),
            ],
        );
        assert_eq!(merged.as_array().unwrap().len(), 3);
    }

    #[test]
    fn null_answers_merge_to_empty_lists() {
        assert_eq!(
            merge("textDocument/references", vec![pair("a", Value::Null)]),
            json!([])
        );
    }
}
