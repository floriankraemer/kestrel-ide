//! What a `textDocument/codeAction` response means: which of the two legal
//! item shapes each entry is, which kind filter an item satisfies, whether
//! it still needs resolving, and what has to happen — in what order — to
//! apply it.
//!
//! All rules, so none of it belongs in `bridge.rs` or `cpp/`
//! (`docs/architecture/layering.md`). The kind taxonomy in particular is a
//! decision with teeth: servers disagree about how specific a kind they
//! report (`refactor.extract`, `refactor.extract.function`,
//! `refactor.extract.method`), so matching is a dotted-prefix walk — the
//! same shape `syntax_core::Scope::resolve` uses for capture names — and
//! never an equality test against a hardcoded list.

use std::time::Duration;

use serde_json::{json, Value};

use crate::manager::{LspError, REFACTOR_TIMEOUT};

/// A `Command`: something the server does itself when asked, rather than an
/// edit it hands over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRef {
    pub title: String,
    pub command: String,
    pub arguments: Vec<Value>,
}

/// One entry of a `textDocument/codeAction` response, whichever of the two
/// shapes it arrived in.
///
/// `edit` and `command` are kept as raw JSON: an edit is parsed by
/// [`crate::workspace_edit`] only once the user has chosen this action, and
/// the arguments of a command are opaque to us by design — they are the
/// server's own state, and forwarding them unread is the whole contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeActionItem {
    pub title: String,
    /// `None` for a bare `Command`, which has no kind.
    pub kind: Option<String>,
    pub edit: Option<Value>,
    pub command: Option<CommandRef>,
    /// Why the server says this action cannot be used here. Such an item is
    /// still listed — greyed out, with the reason — because hiding it makes
    /// the menu change shape depending on the caret, which reads as a bug.
    pub disabled: Option<String>,
    /// The item as the server sent it, which is what `codeAction/resolve`
    /// has to be given back verbatim.
    pub raw: Value,
}

impl CodeActionItem {
    /// Does this item still need `codeAction/resolve` before it can be
    /// applied? An item that carries neither an edit nor a command is a
    /// promise the server has not filled in yet — which is what
    /// `resolveSupport` buys: the expensive edit is computed only for the
    /// action the user actually picked.
    pub fn needs_resolve(&self) -> bool {
        self.edit.is_none() && self.command.is_none()
    }

    /// Whether this item can be used at all.
    pub fn is_usable(&self) -> bool {
        self.disabled.is_none()
    }
}

/// What applying an item involves, in the order it has to happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionStep {
    /// A `WorkspaceEdit`, still to be parsed by [`crate::workspace_edit`].
    ApplyEdit(Value),
    /// A `workspace/executeCommand` to send.
    Execute(CommandRef),
}

/// The steps that apply `item`.
///
/// An item carrying both an edit and a command applies the edit **first**
/// and then executes the command — the specification says so, and it is not
/// arbitrary: the command usually acts on the text the edit just produced.
/// A disabled item has no steps at all.
pub fn steps(item: &CodeActionItem) -> Vec<ActionStep> {
    if !item.is_usable() {
        return Vec::new();
    }
    let mut steps = Vec::with_capacity(2);
    if let Some(edit) = &item.edit {
        steps.push(ActionStep::ApplyEdit(edit.clone()));
    }
    if let Some(command) = &item.command {
        steps.push(ActionStep::Execute(command.clone()));
    }
    steps
}

/// Parse a `textDocument/codeAction` result: an array mixing `Command` and
/// `CodeAction` entries, in any proportion, or `null` for "nothing here".
///
/// The two are told apart by what `command` holds — a string on a bare
/// `Command`, an object on a `CodeAction` that carries one — which is the
/// discriminator the protocol itself defines.
pub fn parse_code_actions(result: &Value) -> Vec<CodeActionItem> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(code_action).collect()
}

fn code_action(value: &Value) -> Option<CodeActionItem> {
    let title = value.get("title")?.as_str()?.to_string();

    // A bare Command: `command` is the identifier itself.
    if let Some(command) = value.get("command").and_then(Value::as_str) {
        return Some(CodeActionItem {
            title: title.clone(),
            kind: None,
            edit: None,
            command: Some(CommandRef {
                title,
                command: command.to_string(),
                arguments: arguments(value),
            }),
            disabled: None,
            raw: value.clone(),
        });
    }

    Some(CodeActionItem {
        title,
        kind: value
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string),
        edit: value.get("edit").filter(|e| !e.is_null()).cloned(),
        command: value.get("command").and_then(command_ref),
        disabled: value
            .get("disabled")
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_string),
        raw: value.clone(),
    })
}

/// Parses one `Command`-shaped object into a [`CommandRef`]. `pub(crate)`
/// because [`crate::code_lens`] carries the same wire shape for a resolved
/// lens's command and reuses this rather than a parallel parser — a
/// `Command` is a `Command` regardless of which LSP feature carries it.
pub(crate) fn command_ref(value: &Value) -> Option<CommandRef> {
    Some(CommandRef {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        command: value.get("command")?.as_str()?.to_string(),
        arguments: arguments(value),
    })
}

fn arguments(value: &Value) -> Vec<Value> {
    value
        .get("arguments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Does `kind` fall under `filter`?
///
/// Dotted-prefix containment, the taxonomy the protocol defines: `refactor`
/// matches `refactor.extract.function`, `refactor.extract` matches it too,
/// and `refactor.extractall` does not — a segment boundary is required, so
/// a filter can never half-match a name that merely starts with it.
/// An item with no kind (a bare `Command`) matches nothing, since there is
/// nothing to classify it by.
pub fn kind_matches(kind: Option<&str>, filter: &str) -> bool {
    let Some(kind) = kind else {
        return false;
    };
    if filter.is_empty() {
        return true;
    }
    kind == filter || (kind.starts_with(filter) && kind.as_bytes().get(filter.len()) == Some(&b'.'))
}

/// The items under `filter`, in the order the server sent them.
///
/// The server's order is kept deliberately: a code-action list is ranked by
/// the server (most servers put the most likely action first), and there is
/// nothing this side knows that would improve on it.
pub fn filter_by_kind(items: &[CodeActionItem], filter: &str) -> Vec<CodeActionItem> {
    items
        .iter()
        .filter(|item| kind_matches(item.kind.as_deref(), filter))
        .cloned()
        .collect()
}

/// Should the request be re-sent without an `only` filter?
///
/// `only` is a hint, and servers treat it inconsistently: some filter by it,
/// some ignore it, and some answer nothing at all when they do not recognise
/// a kind. An empty answer to a filtered request is therefore not proof that
/// no such refactoring exists — so it is asked again unfiltered and filtered
/// here, where the taxonomy is understood. A non-empty answer is trusted as
/// it stands.
pub fn needs_unfiltered_retry(filtered: &[CodeActionItem]) -> bool {
    filtered.is_empty()
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/codeAction` for a range of an open document.
    ///
    /// `only` narrows the request to a kind family (`refactor.extract`), or
    /// is empty for "everything you have". It is only ever a hint — see
    /// [`crate::code_action::needs_unfiltered_retry`] for what an empty
    /// answer to a filtered request does and does not prove.
    pub fn code_action(
        &self,
        uri: &str,
        start: (u32, u32),
        end: (u32, u32),
        only: &[&str],
    ) -> Result<Vec<CodeActionItem>, LspError> {
        self.code_action_scoped(uri, start, end, only, &[], REFACTOR_TIMEOUT)
    }
    /// One `textDocument/codeAction` request, with both of the things that
    /// scope it: a kind filter and the diagnostics the answer should address.
    ///
    /// Handing the diagnostics back in `context.diagnostics` is not optional
    /// decoration — several servers return their quick fixes *only* for
    /// diagnostics they were given, because a fix is computed from the
    /// diagnostic's own `data`.
    pub(crate) fn code_action_scoped(
        &self,
        uri: &str,
        start: (u32, u32),
        end: (u32, u32),
        only: &[&str],
        diagnostics: &[Value],
        timeout: Duration,
    ) -> Result<Vec<CodeActionItem>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let mut context = json!({"diagnostics": diagnostics});
        if !only.is_empty() {
            context["only"] = json!(only);
        }
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri},
                "range": {
                    "start": {"line": start.0, "character": start.1},
                    "end": {"line": end.0, "character": end.1},
                },
                "context": context,
            }),
            timeout,
        )?;
        Ok(parse_code_actions(&result))
    }
    /// `codeAction/resolve` for an item the server sent without an edit.
    ///
    /// The item goes back exactly as it arrived — its `data` is the server's
    /// own bookkeeping, and editing it would break the round trip.
    pub fn resolve_code_action(
        &self,
        language_id: &str,
        item: &CodeActionItem,
    ) -> Result<Vec<CodeActionItem>, LspError> {
        let result = self.request_with_timeout(
            language_id,
            "codeAction/resolve",
            item.raw.clone(),
            REFACTOR_TIMEOUT,
        )?;
        Ok(parse_code_actions(&json!([result])))
    }
    /// `workspace/executeCommand`: ask the server to carry out a command a
    /// code action or a code lens (C10) named — both hand this the same
    /// [`CommandRef`], so there is exactly one execution path for either.
    ///
    /// This is the request during which a server may ask us to apply an edit
    /// (`crate::apply_edit`), so the caller must be holding a
    /// [`LspManager::begin_refactor`] guard — without one the edit that
    /// results is refused as unsolicited, and the refactoring quietly does
    /// nothing.
    pub fn execute_command(
        &self,
        language_id: &str,
        command: &CommandRef,
    ) -> Result<Value, LspError> {
        self.request_with_timeout(
            language_id,
            "workspace/executeCommand",
            json!({"command": command.command, "arguments": command.arguments}),
            REFACTOR_TIMEOUT,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn actions(value: Value) -> Vec<CodeActionItem> {
        parse_code_actions(&value)
    }

    #[test]
    fn a_bare_command_and_a_code_action_are_told_apart() {
        let items = actions(json!([
            {"title": "Do it", "command": "server.doIt", "arguments": [1, "x"]},
            {"title": "Extract", "kind": "refactor.extract.function",
             "command": {"title": "Extract", "command": "server.extract", "arguments": [2]}},
        ]));

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, None, "a bare Command has no kind");
        assert_eq!(items[0].command.as_ref().unwrap().command, "server.doIt",);
        assert_eq!(
            items[0].command.as_ref().unwrap().arguments,
            vec![json!(1), json!("x")]
        );
        assert_eq!(items[1].kind.as_deref(), Some("refactor.extract.function"));
        assert_eq!(items[1].command.as_ref().unwrap().command, "server.extract");
    }

    #[test]
    fn an_edit_carrying_action_is_parsed_with_its_edit_left_raw() {
        let items = actions(json!([{
            "title": "Extract into function",
            "kind": "refactor.extract.function",
            "edit": {"documentChanges": []},
        }]));

        assert!(items[0].edit.is_some());
        assert!(items[0].command.is_none());
        assert!(!items[0].needs_resolve());
    }

    #[test]
    fn an_explicit_null_edit_is_the_same_as_no_edit() {
        let items = actions(json!([{"title": "x", "kind": "refactor", "edit": null}]));
        assert!(items[0].edit.is_none());
        assert!(items[0].needs_resolve());
    }

    #[test]
    fn an_item_with_neither_edit_nor_command_needs_resolving() {
        let items = actions(json!([{
            "title": "Inline variable", "kind": "refactor.inline", "data": {"token": 42},
        }]));

        assert!(items[0].needs_resolve());
        assert_eq!(
            items[0].raw["data"]["token"], 42,
            "the item is kept whole, because resolve is given it back verbatim",
        );
    }

    #[test]
    fn a_disabled_item_is_listed_with_its_reason_and_has_no_steps() {
        let items = actions(json!([{
            "title": "Extract", "kind": "refactor.extract",
            "edit": {"documentChanges": []},
            "disabled": {"reason": "selection is not an expression"},
        }]));

        assert_eq!(
            items[0].disabled.as_deref(),
            Some("selection is not an expression"),
        );
        assert!(!items[0].is_usable());
        assert!(
            steps(&items[0]).is_empty(),
            "a disabled item must not be applicable even though it carries an edit",
        );
    }

    #[test]
    fn an_item_with_both_applies_the_edit_before_the_command() {
        let items = actions(json!([{
            "title": "Extract", "kind": "refactor.extract",
            "edit": {"documentChanges": []},
            "command": {"title": "Finish", "command": "server.finish"},
        }]));

        let steps = steps(&items[0]);
        assert_eq!(steps.len(), 2);
        assert!(matches!(steps[0], ActionStep::ApplyEdit(_)));
        assert!(
            matches!(&steps[1], ActionStep::Execute(c) if c.command == "server.finish"),
            "the command runs after the edit it usually depends on",
        );
    }

    #[test]
    fn nothing_and_nonsense_both_parse_to_no_actions() {
        assert!(actions(Value::Null).is_empty());
        assert!(actions(json!("nonsense")).is_empty());
        assert!(
            actions(json!([{"kind": "refactor"}])).is_empty(),
            "an entry with no title is not an offer we can show",
        );
    }

    #[test]
    fn kinds_match_by_dotted_segment_not_by_string_prefix() {
        assert!(kind_matches(Some("refactor.extract.function"), "refactor"));
        assert!(kind_matches(
            Some("refactor.extract.function"),
            "refactor.extract",
        ));
        assert!(kind_matches(Some("refactor.extract"), "refactor.extract"));

        assert!(
            !kind_matches(Some("refactorial.thing"), "refactor"),
            "a segment boundary is required, so a longer word must not match",
        );
        assert!(!kind_matches(Some("quickfix"), "refactor"));
        assert!(
            !kind_matches(None, "refactor"),
            "a bare Command has no kind"
        );
        assert!(
            kind_matches(Some("anything.at.all"), ""),
            "an empty filter is Refactor This: everything the server offered",
        );
    }

    #[test]
    fn a_kind_we_have_never_heard_of_still_matches_its_family() {
        // The point of prefix matching: a server inventing
        // `refactor.extract.interface` shows up under Extract without this
        // crate learning the name.
        assert!(kind_matches(
            Some("refactor.extract.interface"),
            "refactor.extract",
        ));
    }

    #[test]
    fn filtering_keeps_the_servers_order() {
        let items = actions(json!([
            {"title": "second best", "kind": "refactor.extract.function"},
            {"title": "unrelated", "kind": "quickfix"},
            {"title": "best", "kind": "refactor.extract.class"},
        ]));

        let extract = filter_by_kind(&items, "refactor.extract");
        assert_eq!(
            extract.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            vec!["second best", "best"],
            "the server ranks its own list; we do not re-rank it",
        );
    }

    #[test]
    fn an_empty_filtered_answer_is_re_asked_unfiltered() {
        assert!(needs_unfiltered_retry(&[]));
        assert!(!needs_unfiltered_retry(&actions(
            json!([{"title": "x", "kind": "refactor.extract"}])
        )));
    }
}
