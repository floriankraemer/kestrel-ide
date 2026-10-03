//! The one place a server on another host than the project (ADR-0067)
//! sees and answers in its own paths: every URI crossing the wire is
//! rewritten here, so nothing above `Server::send` or below `dispatch`
//! needs to know a container's paths differ from the local ones.

use std::borrow::Cow;

use process_exec::host::ExecHost;
use serde_json::Value;

use super::{path_for, uri_for, Server};

/// `uri`, spelled by host `from`, as host `to` spells it: through the local
/// path both agree on. A URI `from` cannot turn into a path (not `file://`)
/// is returned unchanged, as is one outside a container's mount — the path
/// of a file only the container has.
pub(super) fn translate_uri(from: &ExecHost, to: &ExecHost, uri: &str) -> String {
    if from == to {
        return uri.to_string();
    }
    match path_for(from, uri) {
        Some(path) => uri_for(to, &path),
        None => uri.to_string(),
    }
}

/// The `processId` to send in `initialize`. A server watches that pid to
/// exit with its parent, and a container has no such process: tell it there
/// is none.
pub(super) fn parent_process_id(host: &ExecHost) -> Value {
    match host {
        ExecHost::Container(_) => Value::Null,
        _ => Value::from(std::process::id()),
    }
}

/// What to call the place a missing server was looked for.
pub(super) fn host_noun(host: &ExecHost) -> &'static str {
    match host {
        ExecHost::Container(_) => "container",
        _ => "WSL distro",
    }
}

impl Server {
    /// `message` as this server's host should read it: every URI in the
    /// project host's spelling becomes the server's. Unchanged (and not
    /// copied) for a server on the project's own host.
    pub(super) fn message_to_wire<'a>(&self, message: &'a Value) -> Cow<'a, Value> {
        if self.host == self.project_host {
            return Cow::Borrowed(message);
        }
        let mut copy = message.clone();
        map_uris(&mut copy, &|uri| {
            translate_uri(&self.project_host, &self.host, uri)
        });
        Cow::Owned(copy)
    }

    /// The inverse of [`Self::message_to_wire`] for what the server sent.
    pub(super) fn message_from_wire(&self, mut message: Value) -> Value {
        if self.host != self.project_host {
            map_uris(&mut message, &|uri| {
                translate_uri(&self.host, &self.project_host, uri)
            });
        }
        message
    }
}

/// Object keys whose string value is a document or folder URI. Text
/// content (`text`, `newText`, markdown) never sits under these, so a
/// document that happens to start with `file://` is left alone.
const URI_KEYS: [&str; 6] = [
    "uri",
    "targetUri",
    "rootUri",
    "oldUri",
    "newUri",
    "scopeUri",
];

/// Rewrite every URI in `message` with `map`: the string values under
/// [`URI_KEYS`] and the keys of a `changes` map (a legacy `WorkspaceEdit`
/// keys its edits by document URI).
pub(super) fn map_uris(message: &mut Value, map: &dyn Fn(&str) -> String) {
    match message {
        Value::Object(object) => {
            for key in URI_KEYS {
                if let Some(Value::String(uri)) = object.get_mut(key) {
                    *uri = map(uri);
                }
            }
            if let Some(Value::Object(changes)) = object.get_mut("changes") {
                let remapped: serde_json::Map<String, Value> = std::mem::take(changes)
                    .into_iter()
                    .map(|(uri, edits)| (map(&uri), edits))
                    .collect();
                *changes = remapped;
            }
            for value in object.values_mut() {
                map_uris(value, map);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| map_uris(item, map)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn upper(uri: &str) -> String {
        uri.replace("/local/", "/remote/")
    }

    #[test]
    fn uri_fields_are_rewritten_at_any_depth() {
        let mut message = json!({"params": {
            "textDocument": {"uri": "file:///local/a.php"},
            "locations": [{"targetUri": "file:///local/b.php", "range": {}}],
            "documentChanges": [{"kind": "rename", "oldUri": "file:///local/c", "newUri": "file:///local/d"}],
        }});
        map_uris(&mut message, &upper);
        assert_eq!(
            message,
            json!({"params": {
                "textDocument": {"uri": "file:///remote/a.php"},
                "locations": [{"targetUri": "file:///remote/b.php", "range": {}}],
                "documentChanges": [{"kind": "rename", "oldUri": "file:///remote/c", "newUri": "file:///remote/d"}],
            }})
        );
    }

    #[test]
    fn the_keys_of_a_changes_map_are_rewritten() {
        let mut message = json!({"edit": {"changes": {"file:///local/a.php": [{"newText": "x"}]}}});
        map_uris(&mut message, &upper);
        assert_eq!(
            message,
            json!({"edit": {"changes": {"file:///remote/a.php": [{"newText": "x"}]}}})
        );
    }

    #[test]
    fn document_text_that_looks_like_a_uri_is_left_alone() {
        let mut message = json!({"text": "file:///local/a.php", "newText": "file:///local/b.php"});
        let before = message.clone();
        map_uris(&mut message, &upper);
        assert_eq!(message, before);
    }
}
