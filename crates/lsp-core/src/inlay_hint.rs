//! What a `textDocument/inlayHint` response means, and the one rule that
//! keeps the request affordable: it is asked for a **line range**, never a
//! whole document.
//!
//! A hint is only worth computing for text somebody is looking at. Asking a
//! 10,000-line file for all of its hints costs the server a full-file
//! inference pass and hands the editor ten thousand labels to lay out, for a
//! viewport that shows fifty. So the range is the caller's viewport, and
//! [`line_range`] is where that is stated once instead of at every call site.

use serde_json::{json, Value};

use crate::manager::{LspError, INLAY_HINT_TIMEOUT};

/// What a hint is telling the reader. `Other` is the honest bucket for a
/// missing or unknown `kind`: the label is still worth painting, it just
/// gets no kind-specific styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlayHintKind {
    Type,
    Parameter,
    Other,
}

impl InlayHintKind {
    fn from_lsp(kind: Option<u64>) -> InlayHintKind {
        match kind {
            Some(1) => InlayHintKind::Type,
            Some(2) => InlayHintKind::Parameter,
            _ => InlayHintKind::Other,
        }
    }
}

/// One hint, positioned in UTF-16 code units as the protocol defines
/// positions.
///
/// `padding_left`/`padding_right` are the server asking for a space on that
/// side. They are kept as flags rather than baked into `label` because the
/// editor draws the padding in its own metrics — a space glyph inside the
/// label would be the *code* font's space, not the hint's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub line: u32,
    pub character: u32,
    pub label: String,
    pub kind: InlayHintKind,
    pub padding_left: bool,
    pub padding_right: bool,
}

/// The `range` of an `textDocument/inlayHint` request for a viewport.
///
/// `last_line` is inclusive and the range ends at its start, which is enough:
/// a hint is anchored to a position, and the protocol asks servers to return
/// every hint whose position falls in the range. Column 0 of the line after
/// the last visible one would pull in hints for a line nobody can see.
pub fn line_range(first_line: u32, last_line: u32) -> Value {
    let (first, last) = if first_line <= last_line {
        (first_line, last_line)
    } else {
        // A caller that swapped its arguments gets a valid request instead
        // of an empty one; the protocol has no meaning for an inverted range.
        (last_line, first_line)
    };
    serde_json::json!({
        "start": {"line": first, "character": 0},
        "end": {"line": last, "character": u32::MAX},
    })
}

/// Parse a `textDocument/inlayHint` result. `null` means the server has no
/// hints for that range, which is not an error.
pub fn parse_inlay_hints(result: &Value) -> Vec<InlayHint> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(hint).collect()
}

fn hint(value: &Value) -> Option<InlayHint> {
    let position = value.get("position")?;
    let label = label(value.get("label")?)?;
    Some(InlayHint {
        line: position.get("line")?.as_u64()? as u32,
        character: position.get("character")?.as_u64()? as u32,
        label,
        kind: InlayHintKind::from_lsp(value.get("kind").and_then(Value::as_u64)),
        padding_left: value
            .get("paddingLeft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        padding_right: value
            .get("paddingRight")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Both legal label shapes flattened to the string that is painted: a plain
/// string, or the `InlayHintLabelPart[]` form concatenated in order.
///
/// The parts carry per-part tooltips and `location` links, which are dropped
/// deliberately — nothing renders them yet, and keeping a structure no
/// surface consumes would be a shape to maintain for free. Restoring them is
/// a change to this function and its type, not to its callers.
fn label(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text: String = parts
                .iter()
                .filter_map(|part| part.get("value").and_then(Value::as_str))
                .collect();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/inlayHint` for the visible lines, inclusive.
    ///
    /// The line range is the request's whole point (see
    /// [`crate::inlay_hint`]): there is no whole-document form of this
    /// method, because a 10,000-line file must not be asked for 10,000
    /// hints to paint fifty.
    pub fn inlay_hints(
        &self,
        uri: &str,
        first_line: u32,
        last_line: u32,
    ) -> Result<Vec<InlayHint>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/inlayHint",
            json!({"textDocument": {"uri": uri}, "range": line_range(first_line, last_line)}),
            INLAY_HINT_TIMEOUT,
        )?;
        Ok(parse_inlay_hints(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_string_label_and_a_part_label_parse_the_same_way() {
        let hints = parse_inlay_hints(&json!([
            {"position": {"line": 3, "character": 9}, "label": ": i32", "kind": 1,
             "paddingLeft": false, "paddingRight": true},
            {"position": {"line": 4, "character": 2},
             "label": [{"value": "value"}, {"value": ":"}], "kind": 2, "paddingRight": true},
        ]));

        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].label, ": i32");
        assert_eq!(hints[0].kind, InlayHintKind::Type);
        assert!(hints[0].padding_right && !hints[0].padding_left);
        assert_eq!(hints[1].label, "value:", "parts concatenate in order");
        assert_eq!(hints[1].kind, InlayHintKind::Parameter);
        assert_eq!(hints[1].line, 4);
        assert_eq!(hints[1].character, 2);
    }

    #[test]
    fn padding_defaults_to_none_and_an_unknown_kind_to_other() {
        let hints = parse_inlay_hints(&json!([
            {"position": {"line": 0, "character": 0}, "label": "x"},
        ]));
        assert!(!hints[0].padding_left && !hints[0].padding_right);
        assert_eq!(hints[0].kind, InlayHintKind::Other);
    }

    #[test]
    fn a_hint_without_a_position_or_a_label_is_not_paintable() {
        assert!(parse_inlay_hints(&json!([{"label": "x"}])).is_empty());
        assert!(parse_inlay_hints(&json!([{"position": {"line": 0, "character": 0}}])).is_empty());
        assert!(parse_inlay_hints(&Value::Null).is_empty());
    }

    #[test]
    fn the_request_range_covers_the_viewport_and_nothing_more() {
        let range = line_range(120, 168);
        assert_eq!(range["start"]["line"], 120);
        assert_eq!(range["start"]["character"], 0);
        assert_eq!(
            range["end"]["line"], 168,
            "the last visible line is included, the one after it is not",
        );
    }

    #[test]
    fn an_inverted_range_is_normalised_rather_than_sent_as_is() {
        let range = line_range(168, 120);
        assert_eq!(range["start"]["line"], 120);
        assert_eq!(range["end"]["line"], 168);
    }
}
