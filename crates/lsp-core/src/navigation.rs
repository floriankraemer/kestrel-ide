//! Go-to-definition: what a `textDocument/definition` response means, and
//! when the server's answer is used instead of the name-based index.
//!
//! Both are rules (ADR-0016, ADR-0011), so neither may live in `bridge.rs` or
//! `cpp/`: the response has three legal shapes, and "LSP first, index as the
//! fallback" is a decision with edge cases — no server, a dead server, a
//! server that answers with nothing — that the view must not re-litigate.

use serde_json::Value;

use serde_json::json;

use crate::diagnostics::path_from_uri;
use crate::manager::{position_params, LspError, DEFINITION_TIMEOUT, METADATA_TIMEOUT};

/// One place a definition was found, addressed the way the editor jumps:
/// `line` 1-based, `column` 0-based, both in UTF-16 code units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionTarget {
    pub uri: String,
    pub path: String,
    pub line: u32,
    pub column: u32,
}

/// Parse a `textDocument/definition` result across the three shapes the
/// protocol allows: a single `Location`, an array of `Location`, and an array
/// of `LocationLink` (which names the target `targetUri`/`targetSelectionRange`
/// instead). Anything unparsable yields no targets, which the caller treats
/// as "the server had no answer".
pub fn parse_definition(result: &Value) -> Vec<DefinitionTarget> {
    match result {
        Value::Array(items) => items.iter().filter_map(target).collect(),
        Value::Object(_) => target(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn target(item: &Value) -> Option<DefinitionTarget> {
    // A LocationLink prefers targetSelectionRange (the name itself) over
    // targetRange (the whole declaration), so the caret lands on the symbol.
    let (uri, range) = match item.get("targetUri") {
        Some(uri) => (
            uri.as_str()?,
            item.get("targetSelectionRange")
                .or_else(|| item.get("targetRange"))?,
        ),
        None => (item.get("uri")?.as_str()?, item.get("range")?),
    };
    let start = range.get("start")?;
    Some(DefinitionTarget {
        uri: uri.to_string(),
        path: path_from_uri(uri).unwrap_or_else(|| uri.to_string()),
        line: start.get("line")?.as_u64()? as u32 + 1,
        column: start.get("character")?.as_u64()? as u32,
    })
}

/// Who answers a go-to-definition gesture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionOutcome {
    /// The language server answered with one or more `file:` targets;
    /// these are the answer.
    Lsp(Vec<DefinitionTarget>),
    /// Nobody asked a server, or it had nothing — ADR-0011's name-based
    /// `index_core::resolve_declaration` answers instead.
    Index,
    /// The server's answer is a non-`file:` URI (C12) — csharp-ls's
    /// `csharp:/metadata/...` for decompiled/generated framework source,
    /// when `useMetadataUris` is on. There is no local path to jump to
    /// directly: the caller must fetch the URI's text first
    /// ([`crate::manager::LspManager::fetch_metadata`]) and open it as a
    /// read-only virtual document, or refuse cleanly if that fails — never
    /// treat the raw URI as a path (ADR-0003 amendment).
    NeedsMetadataFetch(String),
}

/// Split a [`DefinitionOutcome::NeedsMetadataFetch`] URI
/// (`"csharp:/metadata/Projects/x/Console.cs"`) into the `(scheme, key)`
/// pair [`app_core::AppSession::open_virtual_document`] takes — the inverse
/// of csharp-ls's own `scheme:/key` convention for this URI, not a general
/// URI parser. `None` for anything that does not have this shape, which
/// should not happen for a URI that already passed `definition_outcome`'s
/// "not `file://`" check, but a caller fetching over the network is exactly
/// where "should not happen" still needs a typed answer instead of a panic.
pub fn virtual_doc_key(uri: &str) -> Option<(String, String)> {
    let (scheme, rest) = uri.split_once(':')?;
    let key = rest.strip_prefix('/').unwrap_or(rest);
    if scheme.is_empty() || key.is_empty() {
        return None;
    }
    Some((scheme.to_string(), key.to_string()))
}

/// The precedence rule of ADR-0016: a running server's answer wins, and the
/// index is the fallback for everything else — no server configured for the
/// language, a server still starting or inside its restart backoff, a request
/// that timed out or errored, and a server that simply knows nothing about
/// the symbol.
///
/// `None` means no request was made at all.
///
/// A server that answers with a non-`file:` URI (C12) is a third case, not a
/// flavour of "the server answered" — [`DefinitionTarget::path`] falls back
/// to the raw URI string for any URI `path_from_uri` cannot parse, and
/// treating that string as a local path is exactly the "every tab is a
/// file" assumption this function exists to not make.
pub fn definition_outcome(
    response: Option<Result<Vec<DefinitionTarget>, LspError>>,
) -> DefinitionOutcome {
    match response {
        Some(Ok(targets)) if !targets.is_empty() => {
            match targets.iter().find(|t| !t.uri.starts_with("file://")) {
                Some(non_file) => DefinitionOutcome::NeedsMetadataFetch(non_file.uri.clone()),
                None => DefinitionOutcome::Lsp(targets),
            }
        }
        _ => DefinitionOutcome::Index,
    }
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/definition` for a position in an open document. An empty
    /// vector means the server had no answer; whether that falls back to the
    /// index is [`crate::navigation::definition_outcome`]'s decision.
    pub fn definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<DefinitionTarget>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/definition",
            position_params(uri, line, character),
            DEFINITION_TIMEOUT,
        )?;
        let mut targets = parse_definition(&result);
        // W3-2: `target()` computed `.path` with the plain (host-unaware)
        // `path_from_uri` — correct on `ExecHost::Local`, a bare Linux path
        // (e.g. inside `~/.cargo/registry`) on a WSL root. Recompute it
        // through this manager's host, so Go to Definition opens the UNC
        // path the share actually serves rather than a path that only
        // resolves inside the distro.
        for target in &mut targets {
            if let Some(path) = crate::manager::path_for(self.host(), &target.uri) {
                target.path = path;
            }
        }
        Ok(targets)
    }
    /// C12: csharp-ls's custom `csharp/metadata` request — fetches the
    /// decompiled/generated source text a `csharp:/...` definition target
    /// points at (see [`crate::navigation::DefinitionOutcome::NeedsMetadataFetch`]),
    /// so it can be opened as a read-only virtual document instead of a
    /// broken tab.
    ///
    /// `language_id` is passed explicitly rather than derived from `uri` via
    /// `language_of` (as [`LspManager::definition`] does): `uri` here is a
    /// definition *target* the client has never opened, so it is not in the
    /// open-documents table `language_of` reads.
    ///
    /// This is not a method in the LSP base specification — csharp-ls
    /// documents it (`Custom Request: csharp/metadata`) but publishes no
    /// formal schema, so the request/response shape below is a best-effort
    /// reconstruction from the common `{textDocument: {uri}}` request /
    /// `{source: string, ...}` response shape other LSP metadata extensions
    /// (e.g. OmniSharp's own predecessor of this request) use, and is
    /// **unverified against a real csharp-ls process** — this repo's Docker
    /// verification budget did not extend to a live decompiled-symbol round
    /// trip. `stub_server`'s `csharp/metadata` mode exercises the wire
    /// shape this code expects, not what a real server actually sends.
    pub fn fetch_metadata(&self, language_id: &str, uri: &str) -> Result<String, LspError> {
        let result = self.request_with_timeout(
            language_id,
            "csharp/metadata",
            json!({"textDocument": {"uri": uri}}),
            METADATA_TIMEOUT,
        )?;
        result
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                LspError::Protocol(format!("csharp/metadata for {uri} had no \"source\" field"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn location(uri: &str, line: u64, character: u64) -> Value {
        json!({"uri": uri, "range": {
            "start": {"line": line, "character": character},
            "end": {"line": line, "character": character + 4},
        }})
    }

    #[test]
    fn a_single_location_is_one_target() {
        let targets = parse_definition(&location("file:///a/main.rs", 4, 8));
        assert_eq!(
            targets,
            vec![DefinitionTarget {
                uri: "file:///a/main.rs".into(),
                path: "/a/main.rs".into(),
                line: 5,
                column: 8,
            }]
        );
    }

    #[test]
    fn an_array_of_locations_keeps_every_candidate_in_order() {
        let targets = parse_definition(&json!([
            location("file:///a/one.rs", 0, 0),
            location("file:///a/two.rs", 9, 2),
        ]));
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].path, "/a/one.rs");
        assert_eq!(targets[1].line, 10);
    }

    #[test]
    fn a_location_link_uses_its_selection_range() {
        let targets = parse_definition(&json!([{
            "targetUri": "file:///a/main.rs",
            "targetRange": {"start": {"line": 2, "character": 0},
                            "end": {"line": 6, "character": 1}},
            "targetSelectionRange": {"start": {"line": 2, "character": 7},
                                     "end": {"line": 2, "character": 11}},
        }]));
        assert_eq!(targets[0].line, 3);
        assert_eq!(targets[0].column, 7, "the name, not the whole declaration");
    }

    #[test]
    fn a_location_link_without_a_selection_range_falls_back_to_the_target_range() {
        let targets = parse_definition(&json!([{
            "targetUri": "file:///a/main.rs",
            "targetRange": {"start": {"line": 2, "character": 4},
                            "end": {"line": 6, "character": 1}},
        }]));
        assert_eq!((targets[0].line, targets[0].column), (3, 4));
    }

    #[test]
    fn nothing_parses_to_no_targets() {
        assert!(parse_definition(&Value::Null).is_empty());
        assert!(parse_definition(&json!([])).is_empty());
        assert!(parse_definition(&json!([{"uri": "file:///a.rs"}])).is_empty());
    }

    #[test]
    fn a_servers_answer_takes_precedence() {
        let targets = parse_definition(&location("file:///a/main.rs", 0, 0));
        assert_eq!(
            definition_outcome(Some(Ok(targets.clone()))),
            DefinitionOutcome::Lsp(targets)
        );
    }

    #[test]
    fn the_index_answers_when_the_server_does_not() {
        // No server was asked at all.
        assert_eq!(definition_outcome(None), DefinitionOutcome::Index);
        // A server exists but knows nothing here.
        assert_eq!(
            definition_outcome(Some(Ok(vec![]))),
            DefinitionOutcome::Index
        );
        // Configured but not currently running (starting, or in backoff).
        assert_eq!(
            definition_outcome(Some(Err(LspError::NotRunning("rust".into())))),
            DefinitionOutcome::Index
        );
        // Running but too slow, or broken.
        assert_eq!(
            definition_outcome(Some(Err(LspError::Timeout {
                method: "textDocument/definition".into()
            }))),
            DefinitionOutcome::Index
        );
    }

    // --- C12: a non-`file:` target -----------------------------------------

    #[test]
    fn a_csharp_metadata_uri_needs_a_fetch_rather_than_being_treated_as_a_path() {
        let targets = parse_definition(&location("csharp:/metadata/Projects/x/Console.cs", 10, 4));
        // `path_from_uri` cannot parse a non-`file:` URI, so `path` falls
        // back to the raw URI — the exact string a naive caller would be
        // tempted to open as a local path.
        assert_eq!(targets[0].path, "csharp:/metadata/Projects/x/Console.cs");

        assert_eq!(
            definition_outcome(Some(Ok(targets))),
            DefinitionOutcome::NeedsMetadataFetch(
                "csharp:/metadata/Projects/x/Console.cs".to_string()
            )
        );
    }

    #[test]
    fn a_mix_of_file_and_metadata_targets_still_needs_a_fetch() {
        // Rare in practice (go-to-definition usually answers with one
        // target), but the rule must not silently drop the non-file one by
        // only inspecting the first target.
        let mut targets = parse_definition(&location("file:///a/main.rs", 0, 0));
        targets.extend(parse_definition(&location(
            "csharp:/metadata/Console.cs",
            0,
            0,
        )));

        assert_eq!(
            definition_outcome(Some(Ok(targets))),
            DefinitionOutcome::NeedsMetadataFetch("csharp:/metadata/Console.cs".to_string())
        );
    }

    #[test]
    fn a_metadata_uri_splits_into_its_scheme_and_key() {
        assert_eq!(
            virtual_doc_key("csharp:/metadata/Projects/x/Console.cs"),
            Some((
                "csharp".to_string(),
                "metadata/Projects/x/Console.cs".to_string()
            ))
        );
    }

    #[test]
    fn a_uri_with_no_scheme_or_no_key_does_not_split() {
        assert_eq!(virtual_doc_key("no-colon-here"), None);
        assert_eq!(virtual_doc_key(":/metadata/Console.cs"), None);
        assert_eq!(virtual_doc_key("csharp:/"), None);
    }
}
