//! Rename: what `textDocument/prepareRename` and `textDocument/rename`
//! answer, and who renames when the server does not.
//!
//! The precedence rule is the same one ADR-0016 already applies to
//! go-to-definition, and it is what makes rename work at all for the many
//! languages this IDE has a grammar but no language server for: a running
//! server's answer wins, and everything else — no server, a server that does
//! not implement rename, a timeout, an empty answer — falls back to
//! `index-core`'s name-based sites.
//!
//! Rules, so not `bridge.rs` and not `cpp/` (`docs/architecture/layering.md`).

use serde_json::Value;

use crate::manager::{position_params, LspError, REFACTOR_TIMEOUT};
use crate::workspace_edit::{parse_workspace_edit, DocumentEdits};

/// What the server said about renaming the symbol under the caret, from
/// `textDocument/prepareRename`.
///
/// The protocol has three success shapes and they carry different amounts of
/// information; all that matters downstream is the range being renamed and
/// the text to prefill the input with, so they are normalised to one struct.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrepareRename {
    /// The span the server considers the name, in protocol units. `None` for
    /// the `{defaultBehavior: true}` shape, which means "use the word the
    /// editor would have picked".
    pub range: Option<(u32, u32, u32, u32)>,
    /// What to prefill the rename input with, when the server named it —
    /// which matters for languages where the identifier is not the text on
    /// screen (a decorated name, a quoted label).
    pub placeholder: Option<String>,
}

/// Parse a `textDocument/prepareRename` result across its four legal shapes:
/// a bare `Range`, `{range, placeholder}`, `{defaultBehavior: true}`, and
/// `null` for "this cannot be renamed".
pub fn parse_prepare_rename(result: &Value) -> Option<PrepareRename> {
    if result.is_null() {
        return None;
    }
    if let Some(range) = range_of(result) {
        return Some(PrepareRename {
            range: Some(range),
            placeholder: None,
        });
    }
    if let Some(inner) = result.get("range") {
        return Some(PrepareRename {
            range: range_of(inner),
            placeholder: result
                .get("placeholder")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    result
        .get("defaultBehavior")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        .then(PrepareRename::default)
}

fn range_of(value: &Value) -> Option<(u32, u32, u32, u32)> {
    let start = value.get("start")?;
    let end = value.get("end")?;
    Some((
        start.get("line")?.as_u64()? as u32,
        start.get("character")?.as_u64()? as u32,
        end.get("line")?.as_u64()? as u32,
        end.get("character")?.as_u64()? as u32,
    ))
}

/// Whether to go ahead with a rename, having asked the server first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareOutcome {
    /// The server confirmed the symbol can be renamed, and said this much
    /// about it.
    Ready(PrepareRename),
    /// The server said this element cannot be renamed. This is the one case
    /// that stops the gesture, and its message is the server's own verdict.
    Rejected,
    /// The server could not answer the question — it does not implement
    /// `prepareRename`, or the request failed. Not a refusal: the rename is
    /// attempted anyway, and `textDocument/rename` gets to decide.
    Unknown,
}

/// Read a `prepareRename` reply as a decision.
///
/// The distinction that matters: an explicit `null` result is the server
/// saying "not here", while an *error* is the server saying nothing at all.
/// Most servers do not implement `prepareRename`, and treating their
/// `-32601` as a refusal would make rename unavailable exactly where it
/// works fine. `None` means the request was never made.
pub fn prepare_outcome(
    response: Option<Result<Option<PrepareRename>, LspError>>,
) -> PrepareOutcome {
    match response {
        Some(Ok(Some(prepared))) => PrepareOutcome::Ready(prepared),
        Some(Ok(None)) => PrepareOutcome::Rejected,
        Some(Err(_)) | None => PrepareOutcome::Unknown,
    }
}

/// Who renames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameOutcome {
    /// The language server answered; these are the documents to change.
    Lsp(Vec<DocumentEdits>),
    /// Nobody answered, so ADR-0011's name-based index does it instead —
    /// which is what makes rename work for a language with a grammar but no
    /// server, before a server has finished indexing, and while one is
    /// inside its restart backoff.
    Index,
}

/// The precedence rule, deliberately the same shape as
/// [`crate::navigation::definition_outcome`]: a server's non-empty answer
/// wins, and every other case resolves to the index.
///
/// An error is not distinguished from an empty answer on purpose. A server
/// that does not implement rename, one that timed out, and one that found
/// nothing all leave the user in the same position — wanting a rename they
/// have not got — and the index can try in all three.
pub fn rename_outcome(response: Option<Result<Vec<DocumentEdits>, LspError>>) -> RenameOutcome {
    match response {
        Some(Ok(documents)) if !documents.is_empty() => RenameOutcome::Lsp(documents),
        _ => RenameOutcome::Index,
    }
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/prepareRename`: may the symbol at this position be
    /// renamed, and what should the input be prefilled with?
    ///
    /// `Ok(None)` is the server saying "not this element" — a refusal — while
    /// an `Err` is it saying nothing at all, which most servers do because
    /// they do not implement the request. [`crate::rename::prepare_outcome`]
    /// is what tells those two apart.
    pub fn prepare_rename(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<PrepareRename>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/prepareRename",
            position_params(uri, line, character),
            REFACTOR_TIMEOUT,
        )?;
        Ok(parse_prepare_rename(&result))
    }
    /// `textDocument/rename`, parsed into the documents it changes.
    ///
    /// An empty result means the server had no answer; whether that falls
    /// back to the name-based index is
    /// [`crate::rename::rename_outcome`]'s decision, not this method's.
    pub fn rename(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Vec<DocumentEdits>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let mut params = position_params(uri, line, character);
        params["newName"] = Value::String(new_name.to_string());
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/rename",
            params,
            REFACTOR_TIMEOUT,
        )?;
        let mut edits =
            parse_workspace_edit(&result).map_err(|e| LspError::Protocol(e.to_string()))?;
        crate::workspace_edit::retranslate_document_edits(&mut edits, self.host());
        Ok(edits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Value {
        json!({"start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec}})
    }

    #[test]
    fn a_bare_range_is_the_span_being_renamed() {
        let prepared = parse_prepare_rename(&range(3, 4, 3, 9)).unwrap();
        assert_eq!(prepared.range, Some((3, 4, 3, 9)));
        assert_eq!(prepared.placeholder, None);
    }

    #[test]
    fn a_placeholder_shape_carries_the_text_to_prefill() {
        let prepared =
            parse_prepare_rename(&json!({"range": range(1, 2, 1, 6), "placeholder": "old_name"}))
                .unwrap();
        assert_eq!(prepared.range, Some((1, 2, 1, 6)));
        assert_eq!(prepared.placeholder.as_deref(), Some("old_name"));
    }

    #[test]
    fn default_behavior_means_use_the_editors_own_word() {
        let prepared = parse_prepare_rename(&json!({"defaultBehavior": true})).unwrap();
        assert_eq!(prepared, PrepareRename::default());

        assert!(
            parse_prepare_rename(&json!({"defaultBehavior": false})).is_none(),
            "an explicit false is not a confirmation",
        );
    }

    #[test]
    fn null_means_this_cannot_be_renamed() {
        assert!(parse_prepare_rename(&Value::Null).is_none());
    }

    #[test]
    fn an_error_from_prepare_is_not_a_refusal() {
        // The case that matters: most servers do not implement
        // prepareRename, and reading their -32601 as "you may not rename
        // this" would take the feature away exactly where it works.
        let outcome = prepare_outcome(Some(Err(LspError::Response {
            code: -32601,
            message: "not implemented".into(),
        })));
        assert_eq!(outcome, PrepareOutcome::Unknown);
        assert_eq!(prepare_outcome(None), PrepareOutcome::Unknown);
    }

    #[test]
    fn only_an_explicit_null_rejects_the_rename() {
        assert_eq!(prepare_outcome(Some(Ok(None))), PrepareOutcome::Rejected);
        assert!(matches!(
            prepare_outcome(Some(Ok(Some(PrepareRename::default())))),
            PrepareOutcome::Ready(_),
        ));
    }

    fn documents() -> Vec<DocumentEdits> {
        vec![DocumentEdits {
            uri: "file:///a/main.rs".into(),
            path: "/a/main.rs".into(),
            version: None,
            edits: Vec::new(),
        }]
    }

    #[test]
    fn a_servers_answer_wins() {
        assert_eq!(
            rename_outcome(Some(Ok(documents()))),
            RenameOutcome::Lsp(documents()),
        );
    }

    #[test]
    fn every_other_case_falls_back_to_the_index() {
        assert_eq!(rename_outcome(None), RenameOutcome::Index);
        assert_eq!(rename_outcome(Some(Ok(Vec::new()))), RenameOutcome::Index);
        assert_eq!(
            rename_outcome(Some(Err(LspError::NoServer("zig".into())))),
            RenameOutcome::Index,
        );
        assert_eq!(
            rename_outcome(Some(Err(LspError::Timeout {
                method: "textDocument/rename".into()
            }))),
            RenameOutcome::Index,
        );
        assert_eq!(
            rename_outcome(Some(Err(LspError::Response {
                code: -32601,
                message: "not implemented".into(),
            }))),
            RenameOutcome::Index,
        );
    }
}
