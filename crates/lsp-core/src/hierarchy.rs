//! Call hierarchy (`textDocument/prepareCallHierarchy`,
//! `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`) and type
//! hierarchy (`textDocument/prepareTypeHierarchy`,
//! `typeHierarchy/supertypes`, `typeHierarchy/subtypes`).
//!
//! `CallHierarchyItem` and `TypeHierarchyItem` are, field for field, the same
//! shape on the wire — `{name, kind, tags?, detail?, uri, range,
//! selectionRange, data?}` — so both parse into one [`HierarchyItem`] rather
//! than two structurally identical types.
//!
//! C11 (ADR-0016/ADR-0011's precedent, `crate::navigation::definition_outcome`):
//! type hierarchy gets the same LSP-first-with-index-fallback treatment go-to-
//! definition already has, because `index-core` computes the same
//! supertype/subtype edges from `inherits.scm` that `typeHierarchy/supertypes`
//! and `/subtypes` answer. Call hierarchy has **no** index fallback — nothing
//! in `index-core` computes a call graph — so it is LSP-only: unsupported or
//! no server means an empty answer, cleanly, with no fallback to reach for.
//!
//! [`type_hierarchy_outcome`] takes its index-derived fallback already
//! converted to `Vec<HierarchyItem>`, rather than querying `index-core`
//! itself: `lsp-core` may not depend on `index-core`
//! (`docs/architecture/layering.md`), so the join between an
//! `index_core::SymbolMatch` and a `HierarchyItem` is the caller's — `ui-shell`
//! is the one crate that already depends on both. That also keeps this
//! function exactly what step 5 asks for: Qt-free and unit-testable with no
//! LSP process and no index running, because every input is already computed.

use serde_json::{json, Value};

use crate::completion::TextRange;
use crate::manager::{position_params, LspError, HIERARCHY_TIMEOUT};

/// One entry of a call-hierarchy or type-hierarchy response. The two LSP
/// types are structurally identical, so this is shared rather than
/// duplicated — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HierarchyItem {
    pub name: String,
    /// `SymbolKind`, as the server's raw number — the same convention
    /// [`crate::completion::CompletionItem::kind`] follows for its own kind
    /// field, since LSP's `SymbolKind` numbering has no reason to be
    /// re-encoded here only to be decoded again at the view.
    pub kind: u32,
    pub detail: Option<String>,
    pub uri: String,
    pub range: TextRange,
    pub selection_range: TextRange,
    /// The item's own opaque bookkeeping (`data`), if it carries any.
    pub data: Option<Value>,
    /// The item exactly as the server sent it. `incomingCalls`/
    /// `outgoingCalls`/`supertypes`/`subtypes` all take the *whole* item
    /// back, not just its `data` — the protocol re-identifies the symbol
    /// from `uri`/`range`/`data` together — so this is what
    /// [`crate::manager::LspManager::incoming_calls`] and its siblings send,
    /// the same convention [`crate::code_lens::CodeLensItem::raw`] follows
    /// for `codeLens/resolve`.
    pub raw: Value,
}

/// One `callHierarchy/incomingCalls` entry: a caller of the item that was
/// asked about, and the ranges within `from` where the call happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingCall {
    pub from: HierarchyItem,
    pub from_ranges: Vec<TextRange>,
}

/// One `callHierarchy/outgoingCalls` entry: something the item that was
/// asked about calls, and the ranges within the *asked-about* item where the
/// call happens (the protocol names this field `fromRanges` too, even though
/// it points into the caller rather than `to`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingCall {
    pub to: HierarchyItem,
    pub from_ranges: Vec<TextRange>,
}

/// Parse a `prepareCallHierarchy`/`prepareTypeHierarchy`/`supertypes`/
/// `subtypes` result: `HierarchyItem[]`, or `null`/anything else for "no
/// hierarchy available here", which is a valid answer — not every position is
/// a callable symbol or a type with a hierarchy.
pub fn parse_hierarchy_items(result: &Value) -> Vec<HierarchyItem> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(hierarchy_item).collect()
}

fn hierarchy_item(value: &Value) -> Option<HierarchyItem> {
    Some(HierarchyItem {
        name: value.get("name")?.as_str()?.to_string(),
        kind: value.get("kind")?.as_u64()? as u32,
        detail: value
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string),
        uri: value.get("uri")?.as_str()?.to_string(),
        range: range(value.get("range")?)?,
        selection_range: range(value.get("selectionRange")?)?,
        data: value.get("data").cloned(),
        raw: value.clone(),
    })
}

fn range(value: &Value) -> Option<TextRange> {
    Some(TextRange {
        start_line: position(value.get("start")?, "line")?,
        start_character: position(value.get("start")?, "character")?,
        end_line: position(value.get("end")?, "line")?,
        end_character: position(value.get("end")?, "character")?,
    })
}

fn position(value: &Value, field: &str) -> Option<u32> {
    value.get(field)?.as_u64().map(|n| n as u32)
}

/// Parse a `callHierarchy/incomingCalls` result: `CallHierarchyIncomingCall[]`
/// or `null` for none.
pub fn parse_incoming_calls(result: &Value) -> Vec<IncomingCall> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|value| {
            Some(IncomingCall {
                from: hierarchy_item(value.get("from")?)?,
                from_ranges: ranges(value.get("fromRanges")?),
            })
        })
        .collect()
}

/// Parse a `callHierarchy/outgoingCalls` result: `CallHierarchyOutgoingCall[]`
/// or `null` for none.
pub fn parse_outgoing_calls(result: &Value) -> Vec<OutgoingCall> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|value| {
            Some(OutgoingCall {
                to: hierarchy_item(value.get("to")?)?,
                from_ranges: ranges(value.get("fromRanges")?),
            })
        })
        .collect()
}

/// `fromRanges` is allowed to be empty per spec — a call whose exact site
/// isn't tracked is still a real call — so an absent or non-array field
/// parses to no ranges rather than failing the whole entry.
fn ranges(value: &Value) -> Vec<TextRange> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(range).collect())
        .unwrap_or_default()
}

/// Who answers a type-hierarchy request (supertypes or subtypes): the same
/// precedence [`crate::navigation::definition_outcome`] applies to
/// go-to-definition (ADR-0016) — a running server's non-empty answer wins,
/// and `index-core`'s supertype-edge data is the fallback for everything
/// else: no server for the language, one still starting or in backoff, a
/// request that timed out or errored, or a server that answers with nothing.
///
/// `index_fallback` is already computed — see the module docs for why this
/// function cannot query `index-core` itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeHierarchyOutcome {
    /// The language server answered; these items are the answer.
    Lsp(Vec<HierarchyItem>),
    /// Nobody asked a server, or it had nothing — `index-core`'s
    /// supertype-edge data answers instead.
    Index(Vec<HierarchyItem>),
}

pub fn type_hierarchy_outcome(
    response: Option<Result<Vec<HierarchyItem>, LspError>>,
    index_fallback: Vec<HierarchyItem>,
) -> TypeHierarchyOutcome {
    match response {
        Some(Ok(items)) if !items.is_empty() => TypeHierarchyOutcome::Lsp(items),
        _ => TypeHierarchyOutcome::Index(index_fallback),
    }
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/prepareCallHierarchy`: the call-hierarchy item(s) at a
    /// position, the starting point `incomingCalls`/`outgoingCalls` walk
    /// from. Whether it is worth calling at all is
    /// [`Self::call_hierarchy_supported`]'s answer, not this method's — same
    /// convention [`Self::code_lenses_supported`] follows.
    ///
    /// C11: call hierarchy has no `index-core` fallback (see `crate::hierarchy`
    /// module docs) — an empty answer here is final, not a signal to fall
    /// back to anything.
    pub fn prepare_call_hierarchy(
        &self,
        language_id: &str,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<HierarchyItem>, LspError> {
        let uri = &self.normalize_uri(uri);
        let result = self.request_with_timeout(
            language_id,
            "textDocument/prepareCallHierarchy",
            position_params(uri, line, character),
            HIERARCHY_TIMEOUT,
        )?;
        Ok(parse_hierarchy_items(&result))
    }
    /// `callHierarchy/incomingCalls`: everything that calls the item
    /// `prepare_call_hierarchy` returned. `item` is sent back exactly as the
    /// server gave it, same convention [`Self::resolve_code_lens`] follows
    /// for its own `data`-bearing items.
    pub fn incoming_calls(
        &self,
        language_id: &str,
        item: &Value,
    ) -> Result<Vec<IncomingCall>, LspError> {
        let result = self.request_with_timeout(
            language_id,
            "callHierarchy/incomingCalls",
            json!({"item": item}),
            HIERARCHY_TIMEOUT,
        )?;
        Ok(parse_incoming_calls(&result))
    }
    /// `callHierarchy/outgoingCalls`: everything the item
    /// `prepare_call_hierarchy` returned calls. An empty answer here is a
    /// real leaf function, not a hint to fall back — see the module docs on
    /// [`crate::hierarchy`].
    pub fn outgoing_calls(
        &self,
        language_id: &str,
        item: &Value,
    ) -> Result<Vec<OutgoingCall>, LspError> {
        let result = self.request_with_timeout(
            language_id,
            "callHierarchy/outgoingCalls",
            json!({"item": item}),
            HIERARCHY_TIMEOUT,
        )?;
        Ok(parse_outgoing_calls(&result))
    }
    /// `textDocument/prepareTypeHierarchy`: the type-hierarchy item(s) at a
    /// position, the starting point `supertypes`/`subtypes` walk from.
    /// Whether it is worth calling at all is
    /// [`Self::type_hierarchy_supported`]'s answer.
    ///
    /// Unlike call hierarchy, an empty or missing answer here is
    /// `crate::hierarchy::type_hierarchy_outcome`'s cue to fall back to
    /// `index-core`'s supertype-edge data — see the module docs.
    pub fn prepare_type_hierarchy(
        &self,
        language_id: &str,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<HierarchyItem>, LspError> {
        let uri = &self.normalize_uri(uri);
        let result = self.request_with_timeout(
            language_id,
            "textDocument/prepareTypeHierarchy",
            position_params(uri, line, character),
            HIERARCHY_TIMEOUT,
        )?;
        Ok(parse_hierarchy_items(&result))
    }
    /// `typeHierarchy/supertypes`: every supertype of the item
    /// `prepare_type_hierarchy` returned, as the server sees it.
    pub fn supertypes(
        &self,
        language_id: &str,
        item: &Value,
    ) -> Result<Vec<HierarchyItem>, LspError> {
        let result = self.request_with_timeout(
            language_id,
            "typeHierarchy/supertypes",
            json!({"item": item}),
            HIERARCHY_TIMEOUT,
        )?;
        Ok(parse_hierarchy_items(&result))
    }
    /// `typeHierarchy/subtypes`: every subtype of the item
    /// `prepare_type_hierarchy` returned, as the server sees it.
    pub fn subtypes(
        &self,
        language_id: &str,
        item: &Value,
    ) -> Result<Vec<HierarchyItem>, LspError> {
        let result = self.request_with_timeout(
            language_id,
            "typeHierarchy/subtypes",
            json!({"item": item}),
            HIERARCHY_TIMEOUT,
        )?;
        Ok(parse_hierarchy_items(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(name: &str, line: u64) -> Value {
        json!({
            "name": name,
            "kind": 12,
            "uri": "file:///a/main.cs",
            "range": {"start": {"line": line, "character": 0},
                      "end": {"line": line, "character": 10}},
            "selectionRange": {"start": {"line": line, "character": 4},
                               "end": {"line": line, "character": 8}},
        })
    }

    #[test]
    fn a_null_prepare_result_has_no_items() {
        assert!(parse_hierarchy_items(&Value::Null).is_empty());
    }

    #[test]
    fn a_populated_prepare_result_parses_every_item() {
        let items = parse_hierarchy_items(&json!([item("DoWork", 3), item("Main", 9)]));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "DoWork");
        assert_eq!(items[0].kind, 12);
        assert_eq!(items[0].range.start_line, 3);
        assert_eq!(items[1].selection_range.start_line, 9);
    }

    #[test]
    fn a_null_incoming_calls_result_has_no_calls() {
        assert!(parse_incoming_calls(&Value::Null).is_empty());
    }

    #[test]
    fn a_populated_incoming_calls_result_parses_from_and_its_ranges() {
        let calls = parse_incoming_calls(&json!([{
            "from": item("Caller", 1),
            "fromRanges": [{"start": {"line": 1, "character": 4},
                            "end": {"line": 1, "character": 11}}],
        }]));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].from.name, "Caller");
        assert_eq!(calls[0].from_ranges.len(), 1);
        assert_eq!(calls[0].from_ranges[0].start_character, 4);
    }

    #[test]
    fn from_ranges_may_be_empty_per_spec() {
        let calls = parse_incoming_calls(&json!([{
            "from": item("Caller", 1),
            "fromRanges": [],
        }]));
        assert_eq!(calls.len(), 1, "an empty fromRanges is still a real call");
        assert!(calls[0].from_ranges.is_empty());
    }

    #[test]
    fn a_null_outgoing_calls_result_has_no_calls() {
        assert!(parse_outgoing_calls(&Value::Null).is_empty());
    }

    #[test]
    fn a_populated_outgoing_calls_result_parses_to_and_its_ranges() {
        let calls = parse_outgoing_calls(&json!([{
            "to": item("Callee", 5),
            "fromRanges": [{"start": {"line": 2, "character": 0},
                            "end": {"line": 2, "character": 6}}],
        }]));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].to.name, "Callee");
        assert_eq!(calls[0].from_ranges.len(), 1);
    }

    #[test]
    fn a_servers_non_empty_type_hierarchy_answer_takes_precedence() {
        let items = parse_hierarchy_items(&json!([item("Shape", 0)]));
        let outcome = type_hierarchy_outcome(Some(Ok(items.clone())), vec![]);
        assert_eq!(outcome, TypeHierarchyOutcome::Lsp(items));
    }

    #[test]
    fn the_index_answers_type_hierarchy_when_the_server_does_not() {
        let fallback = parse_hierarchy_items(&json!([item("Shape", 0)]));
        // No server was asked at all.
        assert_eq!(
            type_hierarchy_outcome(None, fallback.clone()),
            TypeHierarchyOutcome::Index(fallback.clone())
        );
        // A server exists but knows nothing here.
        assert_eq!(
            type_hierarchy_outcome(Some(Ok(vec![])), fallback.clone()),
            TypeHierarchyOutcome::Index(fallback.clone())
        );
        // Configured but not currently running.
        assert_eq!(
            type_hierarchy_outcome(
                Some(Err(LspError::NotRunning("csharp".into()))),
                fallback.clone()
            ),
            TypeHierarchyOutcome::Index(fallback.clone())
        );
        // Running but too slow, or broken.
        assert_eq!(
            type_hierarchy_outcome(
                Some(Err(LspError::Timeout {
                    method: "typeHierarchy/supertypes".into()
                })),
                fallback.clone()
            ),
            TypeHierarchyOutcome::Index(fallback)
        );
    }

    #[test]
    fn an_empty_outgoing_calls_answer_is_a_real_leaf_not_a_fallback_trigger() {
        // Call hierarchy has no index fallback at all (see module docs), so
        // there is nothing to "fall back to" — an empty array from the
        // server is simply the correct, final answer for a leaf function
        // that calls nothing.
        let calls = parse_outgoing_calls(&json!([]));
        assert!(
            calls.is_empty(),
            "a leaf function's real, non-fallback answer"
        );
    }
}
