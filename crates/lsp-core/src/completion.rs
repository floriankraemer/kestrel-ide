//! What a `textDocument/completion` response means, what text an item
//! actually inserts, how the list is ordered and filtered, when a request is
//! worth making, and when an answer arrives too late to be useful.
//!
//! Every one of those is a rule with edge cases, so none of them may live in
//! `bridge.rs` or `cpp/` (`docs/architecture/layering.md`): the response has
//! two legal shapes, the insertion has three sources with a precedence, and
//! ordering/filtering belong to the server (`sortText`/`filterText`), not to
//! whatever the label happens to spell.

use std::iter::Peekable;
use std::str::Chars;

use serde_json::Value;

use crate::manager::{position_params, LspError, COMPLETION_TIMEOUT, DEFAULT_REQUEST_TIMEOUT};

/// How many identifier characters must be typed before completion is asked
/// for unprompted. One character matches nearly everything the server knows,
/// so the popup would be noise; two is enough to be a guess.
pub const MIN_AUTO_PREFIX: usize = 2;

/// The range a `textEdit` replaces, in the protocol's own units: 0-based
/// lines, characters counted in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

/// One completion candidate, already reduced to what the editor needs: the
/// text to insert, where to put it, and what to show about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    /// `CompletionItemKind`, as the server's raw number (see [`kind_name`]).
    pub kind: Option<u32>,
    pub detail: String,
    pub documentation: String,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
    /// What accepting this item types, with snippet placeholders already
    /// resolved to their default text ([`strip_snippet`]).
    pub insert: String,
    /// The range the insertion replaces, when the server named one. `None`
    /// means "replace whatever word the caret is in", which is the caller's
    /// business, not the protocol's.
    pub range: Option<TextRange>,
    /// The item exactly as the server sent it, kept so it can be sent back
    /// verbatim in a later `completionItem/resolve` request (C7) — the
    /// protocol's own contract for that request is "the item you gave me,
    /// possibly narrowed by `resolveSupport`", not a reconstruction from the
    /// reduced fields above.
    pub raw: Value,
}

impl CompletionItem {
    /// The text a typed prefix is matched against: `filterText` when the
    /// server sent one, the label otherwise. Servers use this to make items
    /// match text the label does not contain (`#include` for `include`), and
    /// ignoring it is why some editors filter worse than others.
    pub fn match_text(&self) -> &str {
        self.filter_text.as_deref().unwrap_or(&self.label)
    }

    /// The key the list is ordered by: `sortText` when present, the label
    /// otherwise. This is how a server puts locals above globals, and it
    /// deliberately has nothing to do with what is displayed.
    pub fn sort_key(&self) -> &str {
        self.sort_text.as_deref().unwrap_or(&self.label)
    }
}

/// A parsed `textDocument/completion` result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionList {
    pub items: Vec<CompletionItem>,
    /// The server says this list is only valid for the prefix it was asked
    /// about: as the word grows, ask again rather than filtering locally.
    pub is_incomplete: bool,
}

/// Parse a completion result across both shapes servers send: a bare
/// `CompletionItem[]` (always complete) and a `CompletionList` with
/// `isIncomplete`. `null` and anything unparsable mean an empty list, which
/// is "nothing to complete here", not an error.
pub fn parse_completion(result: &Value) -> CompletionList {
    let (items, is_incomplete) = match result {
        Value::Array(items) => (items.as_slice(), false),
        Value::Object(_) => (
            result
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            result
                .get("isIncomplete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        _ => (&[][..], false),
    };
    CompletionList {
        items: items.iter().filter_map(item).collect(),
        is_incomplete,
    }
}

fn item(value: &Value) -> Option<CompletionItem> {
    let label = value.get("label")?.as_str()?.to_string();
    // `insertTextFormat: 2` is Snippet, and it governs both `textEdit.newText`
    // and `insertText`.
    let snippet = value
        .get("insertTextFormat")
        .and_then(Value::as_u64)
        .is_some_and(|format| format == 2);

    // Precedence: a `textEdit` wins because it is the only form that also
    // says *what it replaces*; `insertText` next; the label last.
    let edit = value.get("textEdit").filter(|e| e.is_object());
    let raw = edit
        .and_then(|e| e.get("newText"))
        .or_else(|| value.get("insertText"))
        .and_then(Value::as_str)
        .unwrap_or(&label);
    let insert = if snippet {
        strip_snippet(raw)
    } else {
        raw.to_string()
    };

    Some(CompletionItem {
        label,
        kind: value.get("kind").and_then(Value::as_u64).map(|k| k as u32),
        detail: string_at(value.get("detail")),
        documentation: string_at(value.get("documentation")),
        sort_text: value
            .get("sortText")
            .and_then(Value::as_str)
            .map(str::to_string),
        filter_text: value
            .get("filterText")
            .and_then(Value::as_str)
            .map(str::to_string),
        insert,
        range: edit.and_then(edit_range),
        raw: value.clone(),
    })
}

/// An `InsertReplaceEdit` names `insert` and `replace` instead of `range`.
/// The narrower `insert` range is taken, so accepting a completion in the
/// middle of a word never eats the rest of it.
fn edit_range(edit: &Value) -> Option<TextRange> {
    let range = edit
        .get("range")
        .or_else(|| edit.get("insert"))
        .or_else(|| edit.get("replace"))?;
    let (start, end) = (range.get("start")?, range.get("end")?);
    Some(TextRange {
        start_line: start.get("line")?.as_u64()? as u32,
        start_character: start.get("character")?.as_u64()? as u32,
        end_line: end.get("line")?.as_u64()? as u32,
        end_character: end.get("character")?.as_u64()? as u32,
    })
}

/// `string | MarkupContent`, flattened to the one string a popup can show.
fn string_at(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(_)) => value
            .and_then(|v| v.get("value"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// The candidates for a typed prefix, in the order the server asked for.
///
/// Matching is a case-insensitive prefix test against [`CompletionItem::match_text`]
/// and ordering is by [`CompletionItem::sort_key`] with the label as the
/// tie-break — both the server's choice, never the label's by default.
pub fn filter(items: &[CompletionItem], prefix: &str) -> Vec<CompletionItem> {
    let needle = prefix.to_lowercase();
    let mut matched: Vec<CompletionItem> = items
        .iter()
        .filter(|item| item.match_text().to_lowercase().starts_with(&needle))
        .cloned()
        .collect();
    matched.sort_by(|a, b| a.sort_key().cmp(b.sort_key()).then(a.label.cmp(&b.label)));
    matched
}

/// The word the caret is inside, i.e. what a completion replaces and what the
/// list is filtered by: the trailing run of identifier characters.
pub fn completion_prefix(text_before_cursor: &str) -> &str {
    let start = text_before_cursor
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(text_before_cursor.len());
    &text_before_cursor[start..]
}

/// The trigger characters a server advertised in its `initialize` result.
pub fn parse_trigger_characters(init_result: &Value) -> Vec<String> {
    init_result
        .pointer("/capabilities/completionProvider/triggerCharacters")
        .and_then(Value::as_array)
        .map(|chars| {
            chars
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the server offers `completionItem/resolve`
/// (`completionProvider.resolveProvider` in the `initialize` result) — C7.
/// A server that says no is never asked: the second round trip is only
/// worth its latency when there is something on the other end of it.
pub fn parse_resolve_provider(init_result: &Value) -> bool {
    init_result
        .pointer("/capabilities/completionProvider/resolveProvider")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Whether to ask the server for completions after this keystroke.
///
/// An explicit gesture always asks, even mid-word or on nothing at all.
/// Unprompted, there are two reasons to ask: the text now ends with one of
/// the server's own trigger characters (`.`, `:` — the server knows when its
/// language wants a member list), or the user is [`MIN_AUTO_PREFIX`]
/// characters into an identifier. Firing on the *first* identifier character
/// was rejected: it pops a list of everything on every word in the file.
///
/// A list already in hand suppresses the request entirely
/// ([`CompletionTracker::needs_request`]), so growing a word filters what is
/// there instead of asking again per keystroke — unless the server marked
/// its list `isIncomplete`, which means exactly "ask me again".
pub fn should_request(
    triggers: &[String],
    text_before_cursor: &str,
    explicit: bool,
    tracker: &CompletionTracker,
) -> bool {
    let prefix = completion_prefix(text_before_cursor);
    tracker.needs_request(prefix, explicit)
        && (explicit
            || triggers.iter().any(|trigger| {
                !trigger.is_empty() && text_before_cursor.ends_with(trigger.as_str())
            })
            || prefix.chars().count() >= MIN_AUTO_PREFIX)
}

/// Snippet text as plain text: placeholders resolved to their default,
/// tabstops dropped.
///
/// This is not snippet *support* — the caret is not parked on `$1` and Tab
/// does not walk the stops. It exists so a server that only offers snippet
/// items (many do, for function calls) inserts `foo(bar)` instead of leaving
/// `foo(${1:bar})$0` in the buffer for the user to clean up by hand.
pub fn strip_snippet(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some('$') | Some('}') | Some('\\') => out.push(chars.next().expect("peeked")),
                _ => out.push('\\'),
            },
            '$' => match chars.peek() {
                Some('{') => {
                    chars.next();
                    out.push_str(&placeholder(&mut chars));
                }
                Some(next) if next.is_ascii_digit() => {
                    while chars.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                    }
                }
                _ => out.push('$'),
            },
            _ => out.push(c),
        }
    }
    out
}

/// The default text of a `${...}` placeholder, with the opening brace already
/// consumed: the body of `${1:body}`, the first option of `${1|a,b|}`, and
/// nothing for a bare `${1}`.
fn placeholder(chars: &mut Peekable<Chars>) -> String {
    let mut body = String::new();
    let mut depth = 1usize;
    let mut separator: Option<char> = None;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(escaped) = chars.next() {
                    body.push(escaped);
                }
            }
            '{' => {
                depth += 1;
                body.push(c);
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                body.push(c);
            }
            ':' | '|' if separator.is_none() && depth == 1 => separator = Some(c),
            _ if separator.is_some() => body.push(c),
            _ => {}
        }
    }
    match separator {
        Some(':') => strip_snippet(&body),
        Some('|') => body
            .split(',')
            .next()
            .unwrap_or_default()
            .trim_end_matches('|')
            .to_string(),
        _ => String::new(),
    }
}

/// A display name for `CompletionItemKind`, so the popup can say what an item
/// is. Unknown and absent kinds render as nothing rather than a number.
pub fn kind_name(kind: Option<u32>) -> &'static str {
    match kind {
        Some(1) => "text",
        Some(2) => "method",
        Some(3) => "function",
        Some(4) => "constructor",
        Some(5) => "field",
        Some(6) => "variable",
        Some(7) => "class",
        Some(8) => "interface",
        Some(9) => "module",
        Some(10) => "property",
        Some(11) => "unit",
        Some(12) => "value",
        Some(13) => "enum",
        Some(14) => "keyword",
        Some(15) => "snippet",
        Some(16) => "color",
        Some(17) => "file",
        Some(18) => "reference",
        Some(19) => "folder",
        Some(20) => "enum member",
        Some(21) => "constant",
        Some(22) => "struct",
        Some(23) => "event",
        Some(24) => "operator",
        Some(25) => "type parameter",
        _ => "",
    }
}

/// Decides whether a completion response is still the one the user is
/// waiting for.
///
/// Completion is asked for per keystroke and answered on a worker thread, so
/// answers arrive out of order and arrive late. Two things make one stale:
/// a newer request was made (only the newest token is current), and the word
/// under the caret no longer starts with the word the request was made about
/// — a backspace, a new line, a click elsewhere. Showing either would fill
/// the popup with candidates for text that is no longer there.
#[derive(Debug, Default)]
pub struct CompletionTracker {
    latest: u64,
    prefix: String,
    /// `Some(is_incomplete)` once an answer for `prefix` has been kept.
    held: Option<bool>,
}

impl CompletionTracker {
    /// Start a request for `prefix`, invalidating any still in flight and
    /// any list already held.
    pub fn begin(&mut self, prefix: &str) -> u64 {
        self.latest += 1;
        self.prefix = prefix.to_string();
        self.held = None;
        self.latest
    }

    /// A response arrived. Returns whether it is still the current one and
    /// should be kept; a superseded answer changes nothing.
    pub fn deliver(&mut self, token: u64, is_incomplete: bool) -> bool {
        if !self.is_current(token) {
            return false;
        }
        self.held = Some(is_incomplete);
        true
    }

    /// The popup was dismissed or the caret left: nothing in flight, and
    /// nothing held, is wanted any more.
    pub fn cancel(&mut self) {
        self.latest += 1;
        self.prefix.clear();
        self.held = None;
    }

    /// Whether a fresh request is needed at all, given what is already held:
    /// an explicit gesture always is, and otherwise only a missing list, a
    /// list the server marked incomplete, or a caret that has left the word
    /// the held list describes.
    pub fn needs_request(&self, current_prefix: &str, explicit: bool) -> bool {
        explicit
            || !self.still_typing(current_prefix)
            || self.held.is_none_or(|is_incomplete| is_incomplete)
    }

    /// Is this response the newest request's?
    pub fn is_current(&self, token: u64) -> bool {
        token == self.latest
    }

    /// Is the caret still inside the word the request was made about? Typing
    /// further into it keeps the answer usable (it is filtered down);
    /// deleting back past it, or moving away, does not.
    pub fn still_typing(&self, current_prefix: &str) -> bool {
        current_prefix.starts_with(&self.prefix)
    }
}

/// The span accepting a completion replaces, given where the caret is *now*.
///
/// `range` is the item's own `textEdit` range when the server named one.
/// The live caret matters in both shapes:
///
/// * with a range, the span runs to whichever comes later, the range's end
///   or the caret. Characters typed while the request was in flight sit
///   between the two, and leaving them behind is how an editor turns a
///   completion of `fo` into `foormat`;
/// * without one, the `prefix_length` UTF-16 characters before the caret
///   are the word being completed. A zero prefix is a pure insertion, which
///   is what a server that offers a snippet at an empty caret means.
///
/// The prefix is a run of word characters ([`completion_prefix`]), so it
/// never crosses a line; a length longer than the caret's own column can
/// only come from a stale request, and clamps to the line start.
pub fn accept_range(
    range: Option<TextRange>,
    prefix_length: u32,
    caret_line: u32,
    caret_character: u32,
) -> TextRange {
    match range {
        Some(range) => {
            let (end_line, end_character) = std::cmp::max(
                (range.end_line, range.end_character),
                (caret_line, caret_character),
            );
            TextRange {
                end_line,
                end_character,
                ..range
            }
        }
        None => TextRange {
            start_line: caret_line,
            start_character: caret_character.saturating_sub(prefix_length),
            end_line: caret_line,
            end_character: caret_character,
        },
    }
}

/// The completion's own insertion, as a [`crate::workspace_edit::TextEdit`] —
/// the shape it has to be in to merge with a resolved item's
/// `additionalTextEdits` into one application
/// ([`crate::workspace_edit::descending`]/[`crate::workspace_edit::apply_to_text`]),
/// rather than two separate splices a user would have to undo separately.
pub fn own_edit(range: TextRange, insert: &str) -> crate::workspace_edit::TextEdit {
    crate::workspace_edit::TextEdit {
        start_line: range.start_line,
        start_character: range.start_character,
        end_line: range.end_line,
        end_character: range.end_character,
        new_text: insert.to_string(),
    }
}

/// `additionalTextEdits` from a resolved completion item
/// (`completionItem/resolve`) — the `using` an unimported type's completion
/// brings with it, in csharp-ls. Shaped exactly like a formatting reply's
/// `TextEdit[]`, so the same per-edit parser reads it
/// ([`crate::formatting::text_edit`]) rather than a second one; a malformed
/// entry drops the whole array, matching `workspace_edit`'s all-or-nothing
/// rule — half an import applied is worse than none.
pub fn additional_text_edits(resolved: &Value) -> Vec<crate::workspace_edit::TextEdit> {
    let Some(array) = resolved
        .get("additionalTextEdits")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut edits = Vec::with_capacity(array.len());
    for item in array {
        match crate::formatting::text_edit(item) {
            Some(edit) => edits.push(edit),
            None => return Vec::new(),
        }
    }
    edits
}

/// Decides whether a completion-item preview resolution (documentation and
/// detail, requested as the popup's selection moves) is still the one the
/// user is looking at.
///
/// Same shape as [`crate::hover::HoverTracker`] and for the same reason: the
/// request is speculative and keystroke-driven, so an answer for a row the
/// selection has already left is discarded rather than shown under the
/// wrong item. Cancelling the in-flight request over the wire, rather than
/// just discarding a late answer, is [`crate::manager::LspManager::request`]'s
/// own timeout-triggered `$/cancelRequest` — there is no second cancellation
/// mechanism to invent here.
#[derive(Debug, Default)]
pub struct CompletionResolveTracker(crate::RequestTracker);

impl CompletionResolveTracker {
    /// Start a preview resolution, invalidating any still in flight.
    pub fn begin(&mut self) -> u64 {
        self.0.begin()
    }

    /// The selection moved or the popup closed: nothing in flight is wanted
    /// any more.
    pub fn cancel(&mut self) {
        self.0.cancel();
    }

    /// Is this response still the current one?
    pub fn accept(&self, token: u64) -> bool {
        self.0.accept(token)
    }
}

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// `textDocument/completion` for a position in an open document, parsed
    /// across both response shapes. Ordering and filtering are the caller's
    /// next step ([`crate::completion::filter`]), not the manager's.
    pub fn completion(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<CompletionList, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/completion",
            position_params(uri, line, character),
            COMPLETION_TIMEOUT,
        )?;
        Ok(parse_completion(&result))
    }
    /// `completionItem/resolve`: ask the server to fill in what it left out
    /// of the initial list — most importantly `additionalTextEdits`, the
    /// `using` an unimported type's completion brings with it (C7).
    ///
    /// `item` is sent back exactly as the server gave it
    /// ([`CompletionItem::raw`]); the protocol's own contract for this
    /// request is "the item you handed me", not a reconstruction from the
    /// reduced fields the rest of this client works with.
    ///
    /// Whether it is worth calling at all — [`LspEvent::ServerReady::completion_resolve_supported`]
    /// — is the caller's decision, not this method's: a server that never
    /// advertised `resolveProvider` is never asked, so this sends the
    /// request unconditionally on the assumption that check already
    /// happened.
    pub fn resolve_completion_item(
        &self,
        language_id: &str,
        item: &Value,
    ) -> Result<Value, LspError> {
        self.request_with_timeout(
            language_id,
            "completionItem/resolve",
            item.clone(),
            DEFAULT_REQUEST_TIMEOUT,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    fn range(start: (u32, u32), end: (u32, u32)) -> TextRange {
        TextRange {
            start_line: start.0,
            start_character: start.1,
            end_line: end.0,
            end_character: end.1,
        }
    }

    #[test]
    fn a_server_named_range_is_taken_as_it_stands_when_the_caret_is_inside_it() {
        let accepted = accept_range(Some(range((3, 4), (3, 10))), 2, 3, 6);
        assert_eq!(accepted, range((3, 4), (3, 10)));
    }

    #[test]
    fn a_server_named_range_is_stretched_to_a_caret_that_has_moved_past_it() {
        // Two more characters typed while the request was in flight: they
        // are part of the word, so the replaced span has to reach them.
        let accepted = accept_range(Some(range((3, 4), (3, 8))), 6, 3, 10);
        assert_eq!(accepted, range((3, 4), (3, 10)));
    }

    #[test]
    fn without_a_range_the_typed_prefix_before_the_caret_is_replaced() {
        let accepted = accept_range(None, 3, 7, 12);
        assert_eq!(accepted, range((7, 9), (7, 12)));
    }

    #[test]
    fn an_empty_prefix_and_no_range_is_a_pure_insertion_at_the_caret() {
        let accepted = accept_range(None, 0, 7, 12);
        assert_eq!(accepted, range((7, 12), (7, 12)));
    }

    #[test]
    fn a_prefix_longer_than_the_column_clamps_to_the_line_start() {
        let accepted = accept_range(None, 40, 2, 3);
        assert_eq!(accepted, range((2, 0), (2, 3)));
    }

    #[test]
    fn a_bare_item_array_is_a_complete_list() {
        let list = parse_completion(&json!([{"label": "push"}, {"label": "pop"}]));
        assert_eq!(labels(&list.items), ["push", "pop"]);
        assert!(!list.is_incomplete);
    }

    #[test]
    fn a_completion_list_keeps_its_is_incomplete_flag() {
        let list = parse_completion(&json!({
            "isIncomplete": true,
            "items": [{"label": "push"}],
        }));
        assert_eq!(labels(&list.items), ["push"]);
        assert!(list.is_incomplete, "the server wants to be asked again");
    }

    #[test]
    fn nothing_to_complete_is_not_an_error() {
        assert_eq!(parse_completion(&Value::Null), CompletionList::default());
        assert_eq!(parse_completion(&json!([])), CompletionList::default());
        assert_eq!(parse_completion(&json!({})), CompletionList::default());
        assert!(parse_completion(&json!([{"kind": 3}])).items.is_empty());
    }

    #[test]
    fn a_text_edit_wins_over_insert_text_and_the_label() {
        let list = parse_completion(&json!([{
            "label": "push_str",
            "insertText": "push_str_insert",
            "textEdit": {
                "newText": "push_str_edit",
                "range": {"start": {"line": 2, "character": 4},
                          "end": {"line": 2, "character": 8}},
            },
        }]));
        let item = &list.items[0];
        assert_eq!(item.insert, "push_str_edit");
        assert_eq!(
            item.range,
            Some(TextRange {
                start_line: 2,
                start_character: 4,
                end_line: 2,
                end_character: 8,
            }),
            "the edit carries the range it replaces"
        );
    }

    #[test]
    fn insert_text_wins_over_the_label_and_carries_no_range() {
        let list = parse_completion(&json!([{"label": "push_str()", "insertText": "push_str"}]));
        assert_eq!(list.items[0].insert, "push_str");
        assert_eq!(list.items[0].range, None);
    }

    #[test]
    fn the_label_is_the_last_resort() {
        let list = parse_completion(&json!([{"label": "push"}]));
        assert_eq!(list.items[0].insert, "push");
    }

    #[test]
    fn an_insert_replace_edit_uses_its_insert_range() {
        let list = parse_completion(&json!([{
            "label": "push",
            "textEdit": {
                "newText": "push",
                "insert": {"start": {"line": 1, "character": 0},
                           "end": {"line": 1, "character": 2}},
                "replace": {"start": {"line": 1, "character": 0},
                            "end": {"line": 1, "character": 9}},
            },
        }]));
        assert_eq!(list.items[0].range.unwrap().end_character, 2);
    }

    #[test]
    fn a_snippet_item_inserts_its_plain_text() {
        let list = parse_completion(&json!([{
            "label": "map",
            "insertTextFormat": 2,
            "insertText": "map(${1:f})$0",
        }]));
        assert_eq!(list.items[0].insert, "map(f)");
    }

    #[test]
    fn a_plain_item_is_never_snippet_expanded() {
        let list = parse_completion(&json!([{"label": "cost", "insertText": "${1:literal}"}]));
        assert_eq!(list.items[0].insert, "${1:literal}");
    }

    #[test]
    fn snippet_placeholders_resolve_to_their_defaults() {
        assert_eq!(strip_snippet("fn $1() {\n\t$0\n}"), "fn () {\n\t\n}");
        assert_eq!(strip_snippet("${1:name}: ${2:Type}"), "name: Type");
        assert_eq!(strip_snippet("${1|Ok,Err|}"), "Ok");
        assert_eq!(strip_snippet("${1}"), "");
        assert_eq!(strip_snippet("price\\$ ${1:${2:nested}}"), "price$ nested");
        assert_eq!(strip_snippet("cost $ 5"), "cost $ 5", "a lone $ is text");
    }

    #[test]
    fn documentation_is_read_from_both_shapes() {
        let plain = parse_completion(&json!([{"label": "a", "documentation": "plain"}]));
        assert_eq!(plain.items[0].documentation, "plain");
        let markup = parse_completion(&json!([{
            "label": "a",
            "documentation": {"kind": "markdown", "value": "**rich**"},
        }]));
        assert_eq!(markup.items[0].documentation, "**rich**");
    }

    #[test]
    fn sort_text_orders_the_list_and_the_label_only_breaks_ties() {
        let list = parse_completion(&json!([
            {"label": "zebra", "sortText": "0000"},
            {"label": "alpha", "sortText": "9999"},
            {"label": "beta"},
        ]));
        // "beta" has no sortText, so its own label is the key it sorts by —
        // and a label sorts after the digits servers conventionally use.
        assert_eq!(
            labels(&filter(&list.items, "")),
            ["zebra", "alpha", "beta"],
            "the server's order, not alphabetical by label"
        );
    }

    #[test]
    fn filter_text_decides_what_matches() {
        let list = parse_completion(&json!([
            {"label": "#include", "filterText": "include"},
            {"label": "increment"},
        ]));
        assert_eq!(
            labels(&filter(&list.items, "inc")),
            ["#include", "increment"],
            "the label does not start with `inc`, but its filterText does"
        );
        assert!(filter(&list.items, "#inc").is_empty());
    }

    #[test]
    fn filtering_is_case_insensitive_and_an_empty_prefix_keeps_everything() {
        let list = parse_completion(&json!([{"label": "Vec"}, {"label": "vec_deque"}]));
        assert_eq!(labels(&filter(&list.items, "VE")), ["Vec", "vec_deque"]);
        assert_eq!(filter(&list.items, "").len(), 2);
        assert!(filter(&list.items, "x").is_empty());
    }

    #[test]
    fn the_prefix_is_the_word_the_caret_is_in() {
        assert_eq!(completion_prefix("let x = foo.ba"), "ba");
        assert_eq!(completion_prefix("let x = foo."), "");
        assert_eq!(completion_prefix(""), "");
        assert_eq!(completion_prefix("  push_str2"), "push_str2");
    }

    #[test]
    fn trigger_characters_come_from_the_initialize_result() {
        let triggers = parse_trigger_characters(&json!({
            "capabilities": {"completionProvider": {"triggerCharacters": [".", ":"]}},
        }));
        assert_eq!(triggers, [".", ":"]);
        assert!(parse_trigger_characters(&json!({"capabilities": {}})).is_empty());
    }

    #[test]
    fn requests_are_made_explicitly_on_triggers_and_two_characters_in() {
        let triggers = vec![".".to_string(), ":".to_string()];
        let idle = CompletionTracker::default();
        assert!(
            should_request(&triggers, "", true, &idle),
            "the shortcut always asks"
        );
        assert!(should_request(&triggers, "foo.", false, &idle));
        assert!(
            should_request(&triggers, "foo::", false, &idle),
            "Rust's `::`"
        );
        assert!(should_request(&triggers, "let pu", false, &idle));
        assert!(
            !should_request(&triggers, "let p", false, &idle),
            "one char is noise"
        );
        assert!(!should_request(&triggers, "let ", false, &idle));
        assert!(
            !should_request(&[], "foo.", false, &idle),
            "not this server's trigger"
        );
    }

    #[test]
    fn a_complete_list_in_hand_is_filtered_rather_than_asked_for_again() {
        let triggers = vec![".".to_string()];
        let mut tracker = CompletionTracker::default();
        let token = tracker.begin("pu");
        assert!(tracker.deliver(token, false), "a complete list");
        assert!(
            !should_request(&triggers, "let push", false, &tracker),
            "still inside the word the list describes"
        );
        assert!(
            should_request(&triggers, "let push", true, &tracker),
            "the shortcut asks anyway"
        );
        assert!(
            should_request(&triggers, "other", false, &tracker),
            "a different word needs its own list"
        );
    }

    #[test]
    fn an_incomplete_list_is_asked_for_again_as_the_word_grows() {
        let mut tracker = CompletionTracker::default();
        let token = tracker.begin("pu");
        assert!(tracker.deliver(token, true));
        assert!(should_request(&[], "let push", false, &tracker));
    }

    #[test]
    fn a_superseded_response_is_not_kept() {
        let mut tracker = CompletionTracker::default();
        let stale = tracker.begin("pu");
        tracker.begin("pus");
        assert!(
            !tracker.deliver(stale, false),
            "a later request supersedes it"
        );
    }

    #[test]
    fn only_the_newest_completion_response_is_accepted() {
        let mut tracker = CompletionTracker::default();
        let first = tracker.begin("pu");
        let second = tracker.begin("pus");
        assert!(
            !tracker.is_current(first),
            "superseded by a later keystroke"
        );
        assert!(tracker.is_current(second));
    }

    #[test]
    fn a_response_for_a_prefix_the_user_typed_past_is_discarded() {
        let mut tracker = CompletionTracker::default();
        let token = tracker.begin("pus");
        // Still inside the word that was asked about: usable, just narrower.
        assert!(tracker.is_current(token) && tracker.still_typing("push"));
        // Backspaced out of it: the answer describes text that is gone.
        assert!(!tracker.still_typing("pu"));
        // Moved to a different word entirely.
        assert!(!tracker.still_typing("other"));
    }

    #[test]
    fn a_cancelled_completion_is_discarded() {
        let mut tracker = CompletionTracker::default();
        let token = tracker.begin("pu");
        tracker.cancel();
        assert!(!tracker.is_current(token));
    }
}
