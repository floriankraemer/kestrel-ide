//! What a `textDocument/documentHighlight` response means: the occurrences
//! of the symbol under the caret in this one file, and what each occurrence
//! *does* to it.
//!
//! The kind is kept rather than flattened away, because it is the whole
//! reason to prefer this request over a textual find-all: a write to a
//! variable and a read of it are different facts about the program, and the
//! editor paints them differently. Deciding which colour is the theme's job;
//! deciding that they are different is this module's.

use serde_json::Value;

use crate::completion::TextRange;
use crate::manager::{position_params, LspError, DOCUMENT_HIGHLIGHT_TIMEOUT};

/// What the occurrence does to the symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    /// A plain textual occurrence — the protocol's default when a server
    /// omits `kind`, and the honest reading of "I found the name here but
    /// will not claim to know what it is doing".
    Text,
    Read,
    Write,
}

impl HighlightKind {
    fn from_lsp(kind: Option<u64>) -> HighlightKind {
        match kind {
            Some(2) => HighlightKind::Read,
            Some(3) => HighlightKind::Write,
            _ => HighlightKind::Text,
        }
    }
}

/// One occurrence, in UTF-16 code units as the protocol defines positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentHighlight {
    pub range: TextRange,
    pub kind: HighlightKind,
}

/// Parse a `textDocument/documentHighlight` result. `null`, a non-array and
/// an empty array all mean "nothing to highlight here", which is not an
/// error — the caret is simply not on a symbol this server tracks.
pub fn parse_document_highlights(result: &Value) -> Vec<DocumentHighlight> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(highlight).collect()
}

fn highlight(value: &Value) -> Option<DocumentHighlight> {
    Some(DocumentHighlight {
        range: range(value.get("range")?)?,
        kind: HighlightKind::from_lsp(value.get("kind").and_then(Value::as_u64)),
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

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/documentHighlight`: the occurrences of the symbol under
    /// the caret in this file, each with what it does to the symbol. An
    /// empty vector means the caret is not on a symbol, which is not an
    /// error.
    pub fn document_highlights(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<DocumentHighlight>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/documentHighlight",
            position_params(uri, line, character),
            DOCUMENT_HIGHLIGHT_TIMEOUT,
        )?;
        Ok(parse_document_highlights(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(line: u64, kind: Option<u64>) -> Value {
        let mut item = json!({"range": {
            "start": {"line": line, "character": 4},
            "end": {"line": line, "character": 8},
        }});
        if let Some(kind) = kind {
            item["kind"] = json!(kind);
        }
        item
    }

    #[test]
    fn every_kind_survives_parsing() {
        let highlights =
            parse_document_highlights(&json!([at(0, Some(1)), at(1, Some(2)), at(2, Some(3))]));

        assert_eq!(
            highlights.iter().map(|h| h.kind).collect::<Vec<_>>(),
            vec![
                HighlightKind::Text,
                HighlightKind::Read,
                HighlightKind::Write
            ],
        );
        assert_eq!(highlights[1].range.start_line, 1);
        assert_eq!(highlights[1].range.end_character, 8);
    }

    #[test]
    fn a_missing_kind_is_a_plain_textual_occurrence() {
        let highlights = parse_document_highlights(&json!([at(7, None)]));
        assert_eq!(highlights[0].kind, HighlightKind::Text);
    }

    #[test]
    fn an_unknown_kind_degrades_to_text_rather_than_being_dropped() {
        // A server inventing kind 9 still found the occurrence; refusing to
        // paint it would be worse than painting it neutrally.
        let highlights = parse_document_highlights(&json!([at(7, Some(9))]));
        assert_eq!(highlights[0].kind, HighlightKind::Text);
    }

    #[test]
    fn nothing_nonsense_and_a_rangeless_entry_all_parse_to_nothing() {
        assert!(parse_document_highlights(&Value::Null).is_empty());
        assert!(parse_document_highlights(&json!("nonsense")).is_empty());
        assert!(parse_document_highlights(&json!([{"kind": 2}])).is_empty());
    }
}
