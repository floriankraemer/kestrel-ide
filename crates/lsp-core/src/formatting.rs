//! Reformat: what `textDocument/formatting` and `textDocument/rangeFormatting`
//! answer, and what to do when nothing does.
//!
//! Unlike rename (see [`crate::rename`]) there is **no fallback**. Renaming
//! without a server is possible because a name is a name in any language, and
//! `index-core` can find the sites. Formatting is the opposite: it is entirely
//! a matter of one language's conventions, and guessing at them would produce
//! a diff the user did not ask for in a shape their project does not use.
//!
//! So when no server implements formatting, the honest answer is to say so and
//! change nothing — which is why [`FormattingOutcome`] has a variant for it
//! rather than returning an empty edit list. "Nothing happened" and "there is
//! no formatter for this language" look identical to a user pressing a
//! shortcut, and only one of them is worth a message.
//!
//! Rules, so not `bridge.rs` and not `cpp/` (`docs/architecture/layering.md`).

use serde_json::{json, Value};

use crate::manager::{LspError, FORMATTING_TIMEOUT, METHOD_NOT_FOUND};
use crate::workspace_edit::TextEdit;

/// What a formatting request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormattingOutcome {
    /// The server returned edits. Never empty — an empty reply is
    /// [`FormattingOutcome::AlreadyFormatted`], because the two mean
    /// different things to the user.
    Edits(Vec<TextEdit>),
    /// The server answered, and had nothing to change. The document is
    /// already formatted the way the server would format it.
    AlreadyFormatted,
    /// No server is running for this language, or the running one does not
    /// implement formatting. The distinction matters to the message shown,
    /// so it is carried rather than flattened into "no edits".
    Unsupported,
}

/// The formatting options every request carries.
///
/// These come from the editing settings rather than being invented here: a
/// project that indents with four spaces expects its formatter to be told
/// so. `trim_trailing_whitespace` and friends are the protocol's own
/// optional keys, and are sent only when set, because a server that has its
/// own opinion (rustfmt does) should not be overridden by a default we made
/// up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattingOptions {
    pub tab_size: u32,
    pub insert_spaces: bool,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub trim_final_newlines: Option<bool>,
}

impl Default for FormattingOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            trim_trailing_whitespace: None,
            insert_final_newline: None,
            trim_final_newlines: None,
        }
    }
}

impl FormattingOptions {
    /// The `options` object of a formatting request.
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("tabSize".into(), Value::from(self.tab_size));
        map.insert("insertSpaces".into(), Value::from(self.insert_spaces));
        for (key, value) in [
            ("trimTrailingWhitespace", self.trim_trailing_whitespace),
            ("insertFinalNewline", self.insert_final_newline),
            ("trimFinalNewlines", self.trim_final_newlines),
        ] {
            if let Some(value) = value {
                map.insert(key.into(), Value::from(value));
            }
        }
        Value::Object(map)
    }
}

/// Read a formatting reply.
///
/// The protocol allows `TextEdit[]` or `null`. `null` and `[]` are both
/// "nothing to change" — a server with no opinion and a server whose opinion
/// the document already satisfies are indistinguishable here, and both mean
/// the user's file stays as it is.
///
/// A malformed entry invalidates the **whole** reply rather than being
/// skipped, matching `workspace_edit`'s rule: half a reformat is worse than
/// none, because the result is a file in neither the old shape nor the new.
pub fn parse_formatting(result: &Value) -> FormattingOutcome {
    if result.is_null() {
        return FormattingOutcome::AlreadyFormatted;
    }
    let Some(array) = result.as_array() else {
        return FormattingOutcome::Unsupported;
    };
    if array.is_empty() {
        return FormattingOutcome::AlreadyFormatted;
    }
    let mut edits = Vec::with_capacity(array.len());
    for item in array {
        match text_edit(item) {
            Some(edit) => edits.push(edit),
            None => return FormattingOutcome::Unsupported,
        }
    }
    FormattingOutcome::Edits(edits)
}

/// Whether a server's advertised capabilities include formatting.
///
/// Checked before sending, so a language whose server does not format gets
/// the honest message immediately rather than after a round trip and a
/// `MethodNotFound`.
pub fn supports_formatting(capabilities: &Value) -> bool {
    capability_enabled(capabilities, "documentFormattingProvider")
}

/// Whether a server's advertised capabilities include range formatting.
pub fn supports_range_formatting(capabilities: &Value) -> bool {
    capability_enabled(capabilities, "documentRangeFormattingProvider")
}

/// A capability is present when it is `true` or an options object. `false`,
/// `null` and absent all mean no — the protocol allows all three and servers
/// use all three.
fn capability_enabled(capabilities: &Value, key: &str) -> bool {
    match capabilities.get(key) {
        Some(Value::Bool(enabled)) => *enabled,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

/// One `TextEdit` from a formatting reply. Shaped exactly like
/// `workspace_edit`'s, deliberately: the two paths produce the same type so
/// the same ordering, overlap and staleness rules apply to both.
///
/// `pub(crate)` rather than private: `completion::additional_text_edits`
/// (C7) reads the exact same `TextEdit` shape out of a resolved completion
/// item's `additionalTextEdits`, and reusing this parser there is the point
/// — not a second copy of it.
pub(crate) fn text_edit(value: &Value) -> Option<TextEdit> {
    let range = value.get("range")?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(TextEdit {
        start_line: start.get("line")?.as_u64()? as u32,
        start_character: start.get("character")?.as_u64()? as u32,
        end_line: end.get("line")?.as_u64()? as u32,
        end_character: end.get("character")?.as_u64()? as u32,
        new_text: value.get("newText")?.as_str()?.to_string(),
    })
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/formatting` for a whole open document.
    ///
    /// A server that does not implement formatting answers with
    /// `MethodNotFound` rather than an empty list, so that is mapped to
    /// [`FormattingOutcome::Unsupported`] here: from the user's side
    /// "there is no formatter for this language" and "the file is already
    /// formatted" are different messages, and only one of them is a
    /// disappointment worth explaining.
    pub fn format(
        &self,
        uri: &str,
        options: &FormattingOptions,
    ) -> Result<FormattingOutcome, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let params = json!({
            "textDocument": {"uri": uri},
            "options": options.to_json(),
        });
        match self.request_with_timeout(
            &language_id,
            "textDocument/formatting",
            params,
            FORMATTING_TIMEOUT,
        ) {
            Ok(result) => Ok(parse_formatting(&result)),
            Err(LspError::Response { code, .. }) if code == METHOD_NOT_FOUND => {
                Ok(FormattingOutcome::Unsupported)
            }
            Err(err) => Err(err),
        }
    }
    /// `textDocument/rangeFormatting` for a selection.
    ///
    /// Servers commonly implement one of the two and not the other, so this
    /// falls back to whole-document formatting when the range variant is
    /// unsupported — reformatting more than was asked is a better answer
    /// than reformatting nothing, and the preview shows what changed either
    /// way.
    pub fn format_range(
        &self,
        uri: &str,
        start: (u32, u32),
        end: (u32, u32),
        options: &FormattingOptions,
    ) -> Result<FormattingOutcome, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let params = json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": start.0, "character": start.1},
                "end": {"line": end.0, "character": end.1},
            },
            "options": options.to_json(),
        });
        match self.request_with_timeout(
            &language_id,
            "textDocument/rangeFormatting",
            params,
            FORMATTING_TIMEOUT,
        ) {
            Ok(result) => match parse_formatting(&result) {
                FormattingOutcome::Unsupported => self.format(uri, options),
                outcome => Ok(outcome),
            },
            Err(LspError::Response { code, .. }) if code == METHOD_NOT_FOUND => {
                self.format(uri, options)
            }
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, text: &str) -> Value {
        json!({
            "range": {
                "start": {"line": sl, "character": sc},
                "end": {"line": el, "character": ec},
            },
            "newText": text,
        })
    }

    #[test]
    fn edits_are_parsed_in_the_order_the_server_sent_them() {
        let result = json!([edit(0, 0, 0, 4, "  "), edit(2, 0, 2, 8, "    ")]);
        let FormattingOutcome::Edits(edits) = parse_formatting(&result) else {
            panic!("expected edits");
        };
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].new_text, "  ");
        assert_eq!(edits[1].start_line, 2);
    }

    // An empty reply and a null reply both mean the document already matches
    // what the server would produce. That is NOT the same as the language
    // having no formatter, and the user-visible message differs.
    #[test]
    fn an_empty_reply_means_already_formatted_not_unsupported() {
        assert_eq!(
            parse_formatting(&json!([])),
            FormattingOutcome::AlreadyFormatted
        );
    }

    #[test]
    fn a_null_reply_means_already_formatted() {
        assert_eq!(
            parse_formatting(&Value::Null),
            FormattingOutcome::AlreadyFormatted
        );
    }

    #[test]
    fn a_non_array_reply_is_unsupported() {
        assert_eq!(
            parse_formatting(&json!({"unexpected": true})),
            FormattingOutcome::Unsupported
        );
    }

    // Half a reformat leaves a file in neither the old shape nor the new, so
    // one unusable entry invalidates the whole reply rather than being
    // skipped — the same rule workspace_edit applies.
    #[test]
    fn one_malformed_edit_invalidates_the_whole_reply() {
        let result = json!([edit(0, 0, 0, 1, "x"), {"range": {"start": {"line": 1}}}]);
        assert_eq!(parse_formatting(&result), FormattingOutcome::Unsupported);
    }

    #[test]
    fn capabilities_accept_true_and_an_options_object() {
        assert!(supports_formatting(
            &json!({"documentFormattingProvider": true})
        ));
        assert!(supports_formatting(
            &json!({"documentFormattingProvider": {"workDoneProgress": true}})
        ));
        assert!(supports_range_formatting(
            &json!({"documentRangeFormattingProvider": true})
        ));
    }

    #[test]
    fn capabilities_reject_false_null_and_absent() {
        assert!(!supports_formatting(
            &json!({"documentFormattingProvider": false})
        ));
        assert!(!supports_formatting(
            &json!({"documentFormattingProvider": null})
        ));
        assert!(!supports_formatting(&json!({})));
    }

    // A server with its own strong opinion (rustfmt) should not be handed
    // defaults we invented, so the optional keys are sent only when set.
    #[test]
    fn unset_options_are_omitted_rather_than_defaulted() {
        let options = FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..FormattingOptions::default()
        };
        let json = options.to_json();
        assert_eq!(json["tabSize"], 2);
        assert_eq!(json["insertSpaces"], true);
        assert!(json.get("trimTrailingWhitespace").is_none());
        assert!(json.get("insertFinalNewline").is_none());
    }

    #[test]
    fn set_options_are_sent() {
        let options = FormattingOptions {
            tab_size: 8,
            insert_spaces: false,
            trim_trailing_whitespace: Some(true),
            insert_final_newline: Some(false),
            trim_final_newlines: None,
        };
        let json = options.to_json();
        assert_eq!(json["insertSpaces"], false);
        assert_eq!(json["trimTrailingWhitespace"], true);
        assert_eq!(json["insertFinalNewline"], false);
        assert!(json.get("trimFinalNewlines").is_none());
    }
}
