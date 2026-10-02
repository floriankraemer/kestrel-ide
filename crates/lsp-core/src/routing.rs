//! Who answers a request when a language has several servers (ADR-0066).
//!
//! Pure rules only — which servers a method may go to, and how to read an
//! origin back out of a tagged payload. Sending, threading and merging the
//! answers live in [`crate::manager`] and [`crate::merge`].

use serde_json::Value;

use crate::signature_help::SignatureTriggers;

/// How a method is answered when more than one server could.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The first capable server whose answer is not empty. Definition, hover,
    /// rename, formatting, implementation — one authoritative answer.
    First,
    /// Every capable server, answers merged. Completion, code actions,
    /// references and workspace symbols — each server knows things the
    /// other does not.
    Merge,
    /// Back to the server that produced the thing being resolved or run.
    Origin,
}

/// The routing rule for `method`. Anything not named is [`Route::First`].
pub fn route_of(method: &str) -> Route {
    match method {
        "textDocument/completion"
        | "textDocument/codeAction"
        | "textDocument/references"
        | "workspace/symbol" => Route::Merge,
        "completionItem/resolve"
        | "codeAction/resolve"
        | "codeLens/resolve"
        | "workspace/executeCommand" => Route::Origin,
        _ => Route::First,
    }
}

/// The `ServerCapabilities` field a server must declare to be sent `method`,
/// or `None` for a method this table does not know — those are never filtered,
/// because a vendor extension (`csharp/metadata`) has no standard field.
pub fn required_capability(method: &str) -> Option<&'static str> {
    Some(match method {
        "textDocument/completion" | "completionItem/resolve" => "completionProvider",
        "textDocument/hover" => "hoverProvider",
        "textDocument/signatureHelp" => "signatureHelpProvider",
        "textDocument/definition" => "definitionProvider",
        "textDocument/declaration" => "declarationProvider",
        "textDocument/typeDefinition" => "typeDefinitionProvider",
        "textDocument/implementation" => "implementationProvider",
        "textDocument/references" => "referencesProvider",
        "textDocument/documentHighlight" => "documentHighlightProvider",
        "textDocument/documentSymbol" => "documentSymbolProvider",
        "textDocument/codeAction" | "codeAction/resolve" => "codeActionProvider",
        "textDocument/codeLens" | "codeLens/resolve" => "codeLensProvider",
        "textDocument/formatting" => "documentFormattingProvider",
        "textDocument/rangeFormatting" => "documentRangeFormattingProvider",
        "textDocument/onTypeFormatting" => "documentOnTypeFormattingProvider",
        "textDocument/rename" | "textDocument/prepareRename" => "renameProvider",
        "textDocument/inlayHint" => "inlayHintProvider",
        "textDocument/foldingRange" => "foldingRangeProvider",
        "textDocument/selectionRange" => "selectionRangeProvider",
        "textDocument/prepareCallHierarchy" => "callHierarchyProvider",
        "textDocument/prepareTypeHierarchy" => "typeHierarchyProvider",
        "textDocument/semanticTokens/full" | "textDocument/semanticTokens/range" => {
            "semanticTokensProvider"
        }
        "workspace/symbol" => "workspaceSymbolProvider",
        "workspace/executeCommand" => "executeCommandProvider",
        _ => return None,
    })
}

/// Does a server with `capabilities` accept `method`?
///
/// `registered` answers whether the server dynamically registered a method
/// (`client/registerCapability`); csharp-ls declares nothing statically.
/// Dynamic registrations are made under the base method, so
/// `semanticTokens/full` is also looked up as `semanticTokens`.
pub fn is_capable(method: &str, capabilities: &Value, registered: impl Fn(&str) -> bool) -> bool {
    let Some(field) = required_capability(method) else {
        return true;
    };
    let declared = capabilities
        .get(field)
        .is_some_and(|v| !v.is_null() && *v != Value::Bool(false));
    declared
        || registered(method)
        || method
            .rsplit_once('/')
            .is_some_and(|(base, leaf)| matches!(leaf, "full" | "range") && registered(base))
}

/// The key a payload carries to say which server produced it.
pub const ORIGIN_KEY: &str = "x-ide-server";

/// Mark `value` (a completion item, a code action or a command) as coming
/// from `server_id`, so resolving or running it can go back there.
pub fn tag_origin(value: &mut Value, server_id: &str) {
    if let Value::Object(map) = value {
        map.insert(ORIGIN_KEY.into(), Value::String(server_id.into()));
    }
}

/// Remove and return the origin tag, leaving the payload exactly as the
/// server wrote it — the protocol's contract for resolve is "the item you
/// handed me".
pub fn take_origin(value: &mut Value) -> Option<String> {
    match value.as_object_mut()?.remove(ORIGIN_KEY)? {
        Value::String(id) => Some(id),
        _ => None,
    }
}

/// Which server owns `command`: the first whose
/// `executeCommandProvider.commands` lists it. `commands_by_server` pairs a
/// server id with its capabilities, in answer order.
pub fn command_owner<'a>(
    command: &str,
    capabilities_by_server: impl IntoIterator<Item = (&'a str, &'a Value)>,
) -> Option<&'a str> {
    capabilities_by_server.into_iter().find_map(|(id, caps)| {
        caps.pointer("/executeCommandProvider/commands")
            .and_then(Value::as_array)
            .is_some_and(|commands| commands.iter().any(|c| c.as_str() == Some(command)))
            .then_some(id)
    })
}

/// Is a `First` answer worth stopping at? `null`, an empty array and an empty
/// `{items: []}` all mean "nothing here" — the next server may know better.
pub fn is_empty_answer(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty),
        _ => false,
    }
}

/// What one server said it wants, from its `initialize` result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerAdvert {
    pub trigger_characters: Vec<String>,
    pub signature_triggers: SignatureTriggers,
    pub completion_resolve: bool,
    /// `documentOnTypeFormattingProvider`'s first and further trigger
    /// characters.
    pub on_type_triggers: Vec<String>,
}

/// What every server of one language advertised, merged for the language:
/// the editor asks for completion after a character any of them wants.
#[derive(Debug, Clone, Default)]
pub struct Advertised {
    servers: Vec<(String, ServerAdvert)>,
}

impl Advertised {
    /// Record (or replace, after a restart) one server's advertisement.
    pub fn set(&mut self, server_id: &str, advert: ServerAdvert) {
        match self.servers.iter_mut().find(|(id, _)| id == server_id) {
            Some((_, existing)) => *existing = advert,
            None => self.servers.push((server_id.to_string(), advert)),
        }
    }

    /// Forget one server's advertisement (it was stopped).
    pub fn remove(&mut self, server_id: &str) {
        self.servers.retain(|(id, _)| id != server_id);
    }

    /// Completion trigger characters: the union, first occurrence first.
    pub fn trigger_characters(&self) -> Vec<String> {
        union(self.servers.iter().map(|(_, a)| &a.trigger_characters))
    }

    /// Signature help is on when any server offers it, on every character
    /// any of them names.
    pub fn signature_triggers(&self) -> SignatureTriggers {
        SignatureTriggers {
            supported: self
                .servers
                .iter()
                .any(|(_, a)| a.signature_triggers.supported),
            trigger: union(
                self.servers
                    .iter()
                    .map(|(_, a)| &a.signature_triggers.trigger),
            ),
            retrigger: union(
                self.servers
                    .iter()
                    .map(|(_, a)| &a.signature_triggers.retrigger),
            ),
        }
    }

    /// Characters after which any server wants `onTypeFormatting`: the union,
    /// first occurrence first.
    pub fn on_type_triggers(&self) -> Vec<String> {
        union(self.servers.iter().map(|(_, a)| &a.on_type_triggers))
    }

    /// Whether any server offers `completionItem/resolve`; the request itself
    /// goes to the item's origin, which answers or says it cannot.
    pub fn completion_resolve_supported(&self) -> bool {
        self.servers.iter().any(|(_, a)| a.completion_resolve)
    }
}

fn union<'a>(lists: impl Iterator<Item = &'a Vec<String>>) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for item in lists.flatten() {
        if !merged.contains(item) {
            merged.push(item.clone());
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_method_has_the_route_the_adr_names() {
        for m in [
            "textDocument/completion",
            "textDocument/codeAction",
            "textDocument/references",
            "workspace/symbol",
        ] {
            assert_eq!(route_of(m), Route::Merge, "{m}");
        }
        for m in [
            "textDocument/definition",
            "textDocument/hover",
            "textDocument/rename",
            "textDocument/formatting",
            "textDocument/implementation",
            "csharp/metadata",
        ] {
            assert_eq!(route_of(m), Route::First, "{m}");
        }
        for m in [
            "completionItem/resolve",
            "codeAction/resolve",
            "workspace/executeCommand",
        ] {
            assert_eq!(route_of(m), Route::Origin, "{m}");
        }
    }

    #[test]
    fn a_server_is_capable_when_it_declares_the_provider() {
        let caps = json!({"hoverProvider": true, "renameProvider": {"prepareProvider": true},
                          "definitionProvider": false});
        let none = |_: &str| false;
        assert!(is_capable("textDocument/hover", &caps, none));
        assert!(is_capable("textDocument/prepareRename", &caps, none));
        assert!(!is_capable("textDocument/definition", &caps, none));
        assert!(!is_capable("textDocument/references", &caps, none));
    }

    #[test]
    fn a_dynamic_registration_counts_including_under_the_base_method() {
        let caps = Value::Null;
        let reg = |m: &str| m == "textDocument/semanticTokens" || m == "textDocument/codeLens";
        assert!(is_capable("textDocument/semanticTokens/full", &caps, reg));
        assert!(is_capable("textDocument/codeLens", &caps, reg));
        assert!(!is_capable("textDocument/hover", &caps, reg));
    }

    #[test]
    fn an_unknown_method_is_never_filtered() {
        assert!(is_capable("csharp/metadata", &Value::Null, |_| false));
    }

    #[test]
    fn the_origin_tag_round_trips_and_is_removed_before_sending() {
        let mut item = json!({"label": "x", "data": 1});
        tag_origin(&mut item, "phpactor");
        assert_eq!(item[ORIGIN_KEY], "phpactor");
        assert_eq!(take_origin(&mut item).as_deref(), Some("phpactor"));
        assert_eq!(item, json!({"label": "x", "data": 1}));
        assert_eq!(take_origin(&mut item), None);
    }

    #[test]
    fn a_command_goes_to_the_server_that_lists_it() {
        let a = json!({"executeCommandProvider": {"commands": ["a.run"]}});
        let b = json!({"executeCommandProvider": {"commands": ["b.run", "shared"]}});
        let c = json!({});
        let servers = [("a", &a), ("b", &b), ("c", &c)];
        assert_eq!(command_owner("b.run", servers), Some("b"));
        assert_eq!(command_owner("a.run", servers), Some("a"));
        assert_eq!(command_owner("unknown", servers), None);
    }

    #[test]
    fn nothing_is_an_empty_answer_and_a_populated_one_is_not() {
        assert!(is_empty_answer(&Value::Null));
        assert!(is_empty_answer(&json!([])));
        assert!(is_empty_answer(&json!({"items": []})));
        assert!(!is_empty_answer(&json!([1])));
        assert!(!is_empty_answer(&json!({"contents": "x"})));
    }

    #[test]
    fn advertisements_merge_as_a_union_and_a_restart_replaces_its_own() {
        let advert = |chars: &[&str], sig: bool, resolve: bool| ServerAdvert {
            trigger_characters: chars.iter().map(|c| c.to_string()).collect(),
            signature_triggers: SignatureTriggers {
                supported: sig,
                trigger: chars.iter().map(|c| c.to_string()).collect(),
                retrigger: vec![],
            },
            completion_resolve: resolve,
            on_type_triggers: chars.iter().map(|c| c.to_string()).collect(),
        };
        let mut merged = Advertised::default();
        assert!(!merged.completion_resolve_supported());
        merged.set("a", advert(&[".", ":"], false, false));
        merged.set("b", advert(&[":", ">"], true, true));
        assert_eq!(merged.trigger_characters(), [".", ":", ">"]);
        assert!(merged.signature_triggers().supported);
        assert_eq!(merged.signature_triggers().trigger, [".", ":", ">"]);
        assert!(merged.completion_resolve_supported());
        assert_eq!(merged.on_type_triggers(), [".", ":", ">"]);

        merged.set("b", advert(&["$"], false, false));
        merged.set("c", advert(&["@"], false, false));
        merged.remove("c");
        assert_eq!(merged.trigger_characters(), [".", ":", "$"]);
        assert!(!merged.signature_triggers().supported);
        assert!(!merged.completion_resolve_supported());
    }

    #[test]
    fn the_navigation_and_formatting_requests_have_their_adr_route_and_capability() {
        for (method, route, capability) in [
            (
                "textDocument/implementation",
                Route::First,
                "implementationProvider",
            ),
            (
                "textDocument/typeDefinition",
                Route::First,
                "typeDefinitionProvider",
            ),
            (
                "textDocument/declaration",
                Route::First,
                "declarationProvider",
            ),
            (
                "textDocument/onTypeFormatting",
                Route::First,
                "documentOnTypeFormattingProvider",
            ),
            ("workspace/symbol", Route::Merge, "workspaceSymbolProvider"),
        ] {
            assert_eq!(route_of(method), route, "{method}");
            assert_eq!(required_capability(method), Some(capability), "{method}");
        }
    }
}
