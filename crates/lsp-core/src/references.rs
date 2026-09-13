//! R8: `textDocument/references` — parsing the response and grouping it by
//! file, plus the request-sending half of `LspManager`.
//!
//! Same split `hover.rs` and `navigation.rs` use: the parse/group rules
//! here, the request-sending `impl LspManager` block at the bottom
//! (`docs/architecture/layering.md`'s boundary — a request-sending method
//! belongs beside the feature it serves, not lumped into `manager.rs`).

use serde_json::{json, Value};

use crate::manager::{position_params, LspError, DEFAULT_REQUEST_TIMEOUT};

/// One `Location` from a `textDocument/references` response, reduced to
/// what Find Usages needs: which file, and where the reference *starts*
/// (a reference span isn't shown highlighted the way a search match is, so
/// the end position isn't carried).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceLocation {
    pub uri: String,
    /// 0-based, as the wire protocol sends it.
    pub line: u32,
    pub character: u32,
}

/// Parse a `textDocument/references` result: an array of `Location`, or
/// `null` when the server found nothing. Any entry that doesn't parse as a
/// `Location` is dropped rather than failing the whole response — one
/// malformed row from a buggy server shouldn't hide every other one.
pub fn parse_references(result: &Value) -> Vec<ReferenceLocation> {
    result
        .as_array()
        .map(|items| items.iter().filter_map(parse_location).collect())
        .unwrap_or_default()
}

fn parse_location(value: &Value) -> Option<ReferenceLocation> {
    let uri = value.get("uri")?.as_str()?.to_string();
    let start = value.get("range")?.get("start")?;
    let line = start.get("line")?.as_u64()? as u32;
    let character = start.get("character")?.as_u64()? as u32;
    Some(ReferenceLocation {
        uri,
        line,
        character,
    })
}

/// Group references by file, in first-seen file order, each file's
/// references sorted top-to-bottom — the shape a grouped-by-file results
/// panel wants, rather than the server's arbitrary emission order.
pub fn group_by_uri(locations: Vec<ReferenceLocation>) -> Vec<(String, Vec<ReferenceLocation>)> {
    let mut order: Vec<String> = Vec::new();
    let mut by_uri: std::collections::HashMap<String, Vec<ReferenceLocation>> =
        std::collections::HashMap::new();
    for location in locations {
        if !by_uri.contains_key(&location.uri) {
            order.push(location.uri.clone());
        }
        by_uri
            .entry(location.uri.clone())
            .or_default()
            .push(location);
    }
    order
        .into_iter()
        .map(|uri| {
            let mut locations = by_uri.remove(&uri).unwrap_or_default();
            locations.sort_by_key(|l| (l.line, l.character));
            (uri, locations)
        })
        .collect()
}

/// R8: whether a `textDocument/references` answer is trustworthy enough to
/// show instead of the name-based index result — the LSP-vs-index rule the
/// plan asked for, as a small pure predicate (`docs/architecture/
/// intellij-parity-refinement-plan.md`'s R8 section; see this crate's
/// `references_at` bridge wiring for why it lives here rather than in
/// `app-core`: `app-core` has no seam to either data source today, and the
/// bridge already orchestrates both index-core and lsp-core directly for
/// search).
///
/// A server's non-empty answer wins, mirroring
/// [`crate::navigation::definition_outcome`] and [`crate::hover::hover_outcome`]'s
/// same precedence: `Err`, a timeout, or an empty reference list all mean
/// "nothing useful from the server", and the caller's index-based
/// `find_usages` result is what the user actually wanted shown.
pub fn prefer_lsp_references(response: &Result<Vec<ReferenceLocation>, LspError>) -> bool {
    matches!(response, Ok(locations) if !locations.is_empty())
}

impl crate::manager::LspManager {
    /// `textDocument/references` for a position in an open document.
    /// `include_declaration` is Find Usages' own convention: the
    /// definition site is already shown separately (index rows carry
    /// `is_definition`), so a caller usually passes `false` here to avoid
    /// double-counting it — kept as a parameter rather than hardcoded
    /// since Go to Implementation-style callers want it `true`.
    pub fn references(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Vec<ReferenceLocation>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let mut params = position_params(uri, line, character);
        params["context"] = json!({"includeDeclaration": include_declaration});
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/references",
            params,
            DEFAULT_REQUEST_TIMEOUT,
        )?;
        Ok(parse_references(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_list_of_locations() {
        let locations = parse_references(&json!([
            {"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 8}}},
            {"uri": "file:///b.rs", "range": {"start": {"line": 9, "character": 0}, "end": {"line": 9, "character": 3}}},
        ]));
        assert_eq!(
            locations,
            vec![
                ReferenceLocation {
                    uri: "file:///a.rs".into(),
                    line: 1,
                    character: 4
                },
                ReferenceLocation {
                    uri: "file:///b.rs".into(),
                    line: 9,
                    character: 0
                },
            ]
        );
    }

    #[test]
    fn null_and_malformed_entries_are_not_an_error() {
        assert!(parse_references(&Value::Null).is_empty());
        assert!(parse_references(&json!([{"nope": true}])).is_empty());
    }

    #[test]
    fn groups_by_file_preserving_first_seen_order_and_sorts_within_a_file() {
        let grouped = group_by_uri(vec![
            ReferenceLocation {
                uri: "file:///b.rs".into(),
                line: 5,
                character: 0,
            },
            ReferenceLocation {
                uri: "file:///a.rs".into(),
                line: 9,
                character: 0,
            },
            ReferenceLocation {
                uri: "file:///a.rs".into(),
                line: 2,
                character: 0,
            },
        ]);
        assert_eq!(
            grouped,
            vec![
                (
                    "file:///b.rs".to_string(),
                    vec![ReferenceLocation {
                        uri: "file:///b.rs".into(),
                        line: 5,
                        character: 0
                    }]
                ),
                (
                    "file:///a.rs".to_string(),
                    vec![
                        ReferenceLocation {
                            uri: "file:///a.rs".into(),
                            line: 2,
                            character: 0
                        },
                        ReferenceLocation {
                            uri: "file:///a.rs".into(),
                            line: 9,
                            character: 0
                        },
                    ]
                ),
            ]
        );
    }

    #[test]
    fn a_non_empty_server_answer_is_preferred_over_the_index() {
        assert!(prefer_lsp_references(&Ok(vec![ReferenceLocation {
            uri: "file:///a.rs".into(),
            line: 0,
            character: 0
        }])));
    }

    #[test]
    fn an_empty_or_failed_answer_falls_back_to_the_index() {
        assert!(!prefer_lsp_references(&Ok(Vec::new())));
        assert!(!prefer_lsp_references(&Err(LspError::NoServer(
            "zig".into()
        ))));
    }
}
