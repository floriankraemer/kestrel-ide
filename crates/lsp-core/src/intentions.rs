//! The list Alt+Enter shows: everything that can be done at the caret, in
//! the order it is worth doing.
//!
//! A code action reaches us from two different questions, and the user is
//! asking neither of them. "Fix this error" is a `textDocument/codeAction`
//! scoped to the diagnostics under the caret; "refactor this" is the same
//! request scoped to the caret's range with no diagnostics attached. Servers
//! answer them differently — several return quick fixes *only* when the
//! diagnostic is handed back to them in `context.diagnostics` — and the user
//! should not have to know which kind of thing they want before asking for
//! it. So both are sent, and [`assemble`] merges them into the one list.
//!
//! Everything here is rules: grouping, ordering, deduplication, and which
//! diagnostics are worth offering Organize Imports for. None of it belongs
//! in `bridge.rs` or `cpp/` (`docs/architecture/layering.md`).

use serde_json::Value;

use crate::code_action::{filter_by_kind, kind_matches, needs_unfiltered_retry, CodeActionItem};
use crate::manager::{LspError, INTENTION_TIMEOUT, REFACTOR_TIMEOUT};

/// Which section of the popup an action belongs in.
///
/// Ordered as declared, which is the order the menu is built in: a fix for a
/// real error outranks an optional refactoring, and a whole-file source
/// action outranks neither — it is the thing you meant least, given that you
/// pressed Alt+Enter at a particular place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntentionGroup {
    QuickFix,
    Refactor,
    Source,
    /// Anything else the server offered, including a bare `Command`, which
    /// carries no kind at all. Listed last rather than dropped: an action we
    /// cannot classify is still an action the server says applies here, and
    /// hiding it would make the menu disagree with the server for no reason
    /// the user can see.
    Other,
}

impl IntentionGroup {
    /// The group a kind falls in, by the protocol's dotted taxonomy rather
    /// than by an equality test — `crate::code_action::kind_matches` already
    /// owns that walk, so `refactor.extract.function` classifies without this
    /// module learning the name.
    pub fn of(kind: Option<&str>) -> IntentionGroup {
        if kind_matches(kind, "quickfix") {
            IntentionGroup::QuickFix
        } else if kind_matches(kind, "refactor") {
            IntentionGroup::Refactor
        } else if kind_matches(kind, "source") {
            IntentionGroup::Source
        } else {
            IntentionGroup::Other
        }
    }
}

/// Which bulb the editor shows for an offer list: red when something fixes
/// a problem, yellow when it is only an optional action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulbKind {
    Fix,
    Intention,
}

/// Red if any usable quick fix is on offer, yellow otherwise. A disabled
/// quick fix cannot be applied, so it does not turn the bulb red. Whether
/// there is a bulb at all (an empty list) stays the caller's rule.
pub fn bulb_kind(intentions: &[Intention]) -> BulbKind {
    let has_fix = intentions
        .iter()
        .any(|i| i.group == IntentionGroup::QuickFix && i.disabled().is_none());
    if has_fix {
        BulbKind::Fix
    } else {
        BulbKind::Intention
    }
}

/// One row of the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intention {
    pub item: CodeActionItem,
    pub group: IntentionGroup,
    /// The server marked this the obvious action here (`isPreferred`). It
    /// sorts to the top of its own group and nowhere else — a preferred
    /// refactoring does not outrank a quick fix for an error.
    pub preferred: bool,
}

impl Intention {
    pub fn title(&self) -> &str {
        &self.item.title
    }

    pub fn kind(&self) -> Option<&str> {
        self.item.kind.as_deref()
    }

    /// Why this action cannot be used here, if the server said so. The row
    /// is still listed, greyed out, with this as its reason — which is more
    /// useful than a shorter menu that silently omits the thing the user was
    /// looking for.
    pub fn disabled(&self) -> Option<&str> {
        self.item.disabled.as_deref()
    }
}

/// Did the server mark this the obvious action?
///
/// Read from the raw item rather than parsed into [`CodeActionItem`]:
/// `isPreferred` is only ever a ranking hint for a menu, and
/// `crate::code_action` is about what an action *is* and how to apply it.
pub fn is_preferred(item: &CodeActionItem) -> bool {
    item.raw
        .get("isPreferred")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Merge the two `textDocument/codeAction` replies into the list the popup
/// shows: grouped, preferred-first within a group, deduplicated, and
/// otherwise in the order the servers sent them.
///
/// `diagnostic_scoped` comes first because its items are the ones that
/// carry a diagnostic's context, and deduplication keeps the first copy.
///
/// **Identity is `(title, kind)`.** The two requests overlap heavily and the
/// same action routinely arrives twice, so something has to decide when two
/// items are one. The raw JSON cannot: several servers embed the requested
/// range or the diagnostic in an action's opaque `data`, so byte equality
/// reports two distinct actions for what is plainly one offer. Title and
/// kind are exactly what the user is choosing between — two rows that agree
/// on both are indistinguishable in the menu, and a menu that lists the same
/// sentence twice reads as a bug whether or not the payloads differ.
///
/// Two refinements when a duplicate arrives:
/// * `preferred` is the **or** of the copies, so a rank the second request
///   reported and the first did not is not lost.
/// * a usable copy replaces a disabled one, because "you can do this" is
///   strictly better information than "you cannot do this here".
pub fn assemble(
    diagnostic_scoped: &[CodeActionItem],
    range_scoped: &[CodeActionItem],
) -> Vec<Intention> {
    let mut out: Vec<Intention> = Vec::new();
    for item in diagnostic_scoped.iter().chain(range_scoped) {
        let identity = (item.title.as_str(), item.kind.as_deref());
        match out
            .iter_mut()
            .find(|existing| (existing.item.title.as_str(), existing.kind()) == identity)
        {
            Some(existing) => {
                existing.preferred |= is_preferred(item);
                if existing.item.disabled.is_some() && item.disabled.is_none() {
                    existing.item = item.clone();
                }
            }
            None => out.push(Intention {
                group: IntentionGroup::of(item.kind.as_deref()),
                preferred: is_preferred(item),
                item: item.clone(),
            }),
        }
    }
    // Stable, so within a group the servers' own ranking survives — they
    // rank their lists and there is nothing this side knows that improves on
    // it (the same reason `code_action::filter_by_kind` keeps the order).
    out.sort_by_key(|intention| (intention.group, !intention.preferred));
    out
}

// -- Organize imports ------------------------------------------------------

/// The kind of the Organize Imports source action, as the protocol names it.
pub const ORGANIZE_IMPORTS: &str = "source.organizeImports";

/// Is this diagnostic one that organizing the imports might fix?
///
/// **This is a heuristic, and deliberately a documented one.** LSP defines
/// no code for "this name has no import" — `Diagnostic.code` is a private
/// namespace per server, freely a string or a number — so there is nothing
/// authoritative to match on. What is matched, in order of trust:
///
/// 1. **Codes**, which are stable per server and language-independent:
///    rustc's `E0412`/`E0422`/`E0425`/`E0433` and rust-analyzer's
///    `unresolved-import`/`unresolved-macro-call`/`unresolved-reference`;
///    TypeScript's `2304`, `2307`, `2503`, `2552`; pyright's
///    `reportUndefinedVariable`/`reportMissingImports`.
/// 2. **Message shapes**, as a fallback for the servers whose codes are
///    opaque integers (jdtls) or absent: "cannot find", "cannot be
///    resolved", "is not defined", "undefined name", "unresolved", "not
///    found in this scope".
///
/// The message half is English-dependent, which is why it is second: a
/// server that localises its diagnostics falls back to its codes. Both
/// failure directions are cheap and asymmetric, which is what makes a
/// heuristic acceptable here: a false positive offers one extra menu entry
/// that reorders the import block and changes nothing else, while a false
/// negative costs only the shortcut — Organize Imports remains available in
/// its own right ([`ORGANIZE_IMPORTS`]) either way.
pub fn suggests_organize_imports(diagnostic: &Value) -> bool {
    let code = diagnostic.get("code").map(code_text).unwrap_or_default();
    if UNRESOLVED_CODES.contains(&code.as_str()) {
        return true;
    }
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    UNRESOLVED_MESSAGES
        .iter()
        .any(|shape| message.contains(shape))
}

/// A diagnostic's `code`, which the protocol allows to be a string or a
/// number, as one string.
fn code_text(code: &Value) -> String {
    match code {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

const UNRESOLVED_CODES: &[&str] = &[
    // rustc, via rust-analyzer.
    "E0412",
    "E0422",
    "E0425",
    "E0433", //
    // rust-analyzer's own.
    "unresolved-import",
    "unresolved-macro-call",
    "unresolved-reference",
    // TypeScript: cannot find name / module / namespace, and the
    // did-you-mean variant.
    "2304",
    "2307",
    "2503",
    "2552", //
    // pyright.
    "reportUndefinedVariable",
    "reportMissingImports",
];

const UNRESOLVED_MESSAGES: &[&str] = &[
    "cannot find",
    "cannot be resolved",
    "is not defined",
    "undefined name",
    "unresolved",
    "not found in this scope",
];

// C4-followup (#162): request-sending `LspManager` methods for this feature, moved out of
// `manager.rs` once it crossed the file-size ceiling. This file already held the
// parse/rule layer; this is the request-sending half `manager.rs`'s own module doc
// pointed callers to.
impl crate::manager::LspManager {
    /// Everything that can be done at the caret: the list Alt+Enter shows.
    ///
    /// Two requests, merged by [`crate::intentions::assemble`] — one scoped
    /// to `diagnostics` (the diagnostics under the caret, handed back
    /// verbatim as the protocol requires) and one to the range alone. A
    /// quick fix for the error under the cursor and a refactoring available
    /// at that position are both "things I can do here", and the user should
    /// not have to choose which kind they meant before asking.
    ///
    /// Neither request carries an `only` filter, so there is nothing for
    /// [`crate::code_action::needs_unfiltered_retry`] to retry: an empty
    /// answer to an unfiltered request really does mean the server has
    /// nothing. The retry exists for [`Self::organize_imports`], which does
    /// filter.
    ///
    /// A failure of the diagnostic-scoped request is not a failure of the
    /// whole list — a server that rejects a diagnostic payload it does not
    /// recognise should still get to offer its refactorings — so only the
    /// range-scoped request can fail this method.
    pub fn intentions(
        &self,
        uri: &str,
        start: (u32, u32),
        end: (u32, u32),
        diagnostics: &[Value],
    ) -> Result<Vec<Intention>, LspError> {
        let diagnostic_scoped = if diagnostics.is_empty() {
            Vec::new()
        } else {
            self.code_action_scoped(uri, start, end, &[], diagnostics, INTENTION_TIMEOUT)
                .unwrap_or_default()
        };
        let range_scoped = self.code_action_scoped(uri, start, end, &[], &[], INTENTION_TIMEOUT)?;
        Ok(assemble(&diagnostic_scoped, &range_scoped))
    }
    /// The `source.organizeImports` action for a whole document, if the
    /// server has one.
    ///
    /// Asked with an `only` filter first, because a whole-document code
    /// action request is expensive and there is exactly one action wanted.
    /// An empty answer to that filtered request proves nothing — servers
    /// disagree about whether `only` is a filter or a hint, and some answer
    /// nothing at all for a kind they do not recognise — so
    /// [`crate::code_action::needs_unfiltered_retry`] sends it again
    /// unfiltered and the taxonomy is applied here, where it is understood.
    pub fn organize_imports(
        &self,
        uri: &str,
        last_line: u32,
    ) -> Result<Option<CodeActionItem>, LspError> {
        let (start, end) = ((0, 0), (last_line, u32::MAX));
        let filtered =
            self.code_action_scoped(uri, start, end, &[ORGANIZE_IMPORTS], &[], REFACTOR_TIMEOUT)?;
        let items = if needs_unfiltered_retry(&filtered) {
            let all = self.code_action_scoped(uri, start, end, &[], &[], REFACTOR_TIMEOUT)?;
            filter_by_kind(&all, ORGANIZE_IMPORTS)
        } else {
            filter_by_kind(&filtered, ORGANIZE_IMPORTS)
        };
        Ok(items.into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_action::parse_code_actions;
    use serde_json::json;

    fn items(value: Value) -> Vec<CodeActionItem> {
        parse_code_actions(&value)
    }

    fn titles(intentions: &[Intention]) -> Vec<&str> {
        intentions.iter().map(Intention::title).collect()
    }

    #[test]
    fn the_bulb_is_red_only_for_a_usable_quick_fix() {
        let fix = json!({"title": "Import", "kind": "quickfix"});
        let disabled_fix =
            json!({"title": "Nope", "kind": "quickfix", "disabled": {"reason": "x"}});
        let refactor = json!({"title": "Extract", "kind": "refactor.extract"});
        let kind = |v: Value| bulb_kind(&assemble(&items(v), &[]));

        assert_eq!(kind(json!([refactor.clone(), fix])), BulbKind::Fix);
        assert_eq!(kind(json!([refactor.clone()])), BulbKind::Intention);
        assert_eq!(kind(json!([refactor, disabled_fix])), BulbKind::Intention);
        assert_eq!(kind(json!([])), BulbKind::Intention);
    }

    #[test]
    fn quick_fixes_come_before_refactorings_and_refactorings_before_source() {
        let merged = assemble(
            &items(json!([{"title": "Import `HashMap`", "kind": "quickfix"}])),
            &items(json!([
                {"title": "Organize imports", "kind": "source.organizeImports"},
                {"title": "Extract into function", "kind": "refactor.extract.function"},
            ])),
        );

        assert_eq!(
            titles(&merged),
            vec![
                "Import `HashMap`",
                "Extract into function",
                "Organize imports"
            ],
            "a fix for a real error outranks an optional refactoring",
        );
        assert_eq!(merged[0].group, IntentionGroup::QuickFix);
        assert_eq!(merged[1].group, IntentionGroup::Refactor);
        assert_eq!(merged[2].group, IntentionGroup::Source);
    }

    #[test]
    fn a_preferred_action_leads_its_own_group_and_no_other() {
        let merged = assemble(
            &items(json!([
                {"title": "Remove it", "kind": "quickfix"},
                {"title": "Import it", "kind": "quickfix", "isPreferred": true},
            ])),
            &items(json!([
                {"title": "Inline", "kind": "refactor.inline", "isPreferred": true},
            ])),
        );

        assert_eq!(
            titles(&merged),
            vec!["Import it", "Remove it", "Inline"],
            "a preferred refactoring must not jump over an unpreferred quick fix",
        );
        assert!(merged[0].preferred);
    }

    #[test]
    fn within_a_group_the_servers_own_ranking_survives() {
        let merged = assemble(
            &[],
            &items(json!([
                {"title": "first", "kind": "refactor.extract"},
                {"title": "second", "kind": "refactor.inline"},
                {"title": "third", "kind": "refactor.rewrite"},
            ])),
        );
        assert_eq!(titles(&merged), vec!["first", "second", "third"]);
    }

    #[test]
    fn an_action_arriving_from_both_requests_is_listed_once() {
        let both = json!([{"title": "Import `HashMap`", "kind": "quickfix"}]);
        let merged = assemble(&items(both.clone()), &items(both));
        assert_eq!(titles(&merged), vec!["Import `HashMap`"]);
    }

    #[test]
    fn identity_is_title_and_kind_not_the_opaque_payload() {
        // Both servers' copies carry the request they were produced for in
        // `data`; byte equality would call these two different offers.
        let merged = assemble(
            &items(json!([{"title": "Import `HashMap`", "kind": "quickfix",
                           "data": {"scope": "diagnostic"}}])),
            &items(json!([{"title": "Import `HashMap`", "kind": "quickfix",
                           "data": {"scope": "range"}}])),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].item.raw["data"]["scope"], "diagnostic",
            "the diagnostic-scoped copy is kept, because it has the context",
        );
    }

    #[test]
    fn the_same_title_under_a_different_kind_is_a_different_offer() {
        let merged = assemble(
            &items(json!([{"title": "Import `HashMap`", "kind": "quickfix"}])),
            &items(json!([{"title": "Import `HashMap`", "kind": "source.organizeImports"}])),
        );
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn a_duplicate_contributes_its_preferred_flag_and_its_usability() {
        let merged = assemble(
            &items(json!([{"title": "Import it", "kind": "quickfix",
                           "disabled": {"reason": "no candidate"}}])),
            &items(
                json!([{"title": "Import it", "kind": "quickfix", "isPreferred": true,
                           "edit": {"documentChanges": []}}]),
            ),
        );

        assert_eq!(merged.len(), 1);
        assert!(merged[0].preferred, "a rank reported by either copy counts");
        assert_eq!(
            merged[0].disabled(),
            None,
            "a copy that works replaces one that says it cannot",
        );
    }

    #[test]
    fn a_disabled_action_stays_in_the_list_with_its_reason() {
        let merged = assemble(
            &[],
            &items(json!([{"title": "Extract", "kind": "refactor.extract",
                           "disabled": {"reason": "selection is not an expression"}}])),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].disabled(), Some("selection is not an expression"));
    }

    #[test]
    fn an_unclassifiable_action_is_listed_last_rather_than_dropped() {
        let merged = assemble(
            &[],
            &items(json!([
                {"title": "Run the command directly", "command": "server.doIt"},
                {"title": "Something new", "kind": "notify.user"},
                {"title": "Extract", "kind": "refactor.extract"},
            ])),
        );

        assert_eq!(
            titles(&merged),
            vec!["Extract", "Run the command directly", "Something new"]
        );
        assert_eq!(merged[1].group, IntentionGroup::Other);
        assert_eq!(merged[2].group, IntentionGroup::Other);
    }

    #[test]
    fn a_kind_family_we_have_never_heard_of_still_groups_by_its_prefix() {
        assert_eq!(
            IntentionGroup::of(Some("refactor.extract.interface")),
            IntentionGroup::Refactor,
        );
        assert_eq!(
            IntentionGroup::of(Some("source.fixAll.eslint")),
            IntentionGroup::Source,
        );
        assert_eq!(IntentionGroup::of(None), IntentionGroup::Other);
    }

    #[test]
    fn nothing_from_either_request_is_an_empty_list_not_an_error() {
        assert!(assemble(&[], &[]).is_empty());
    }

    // -- organize imports -------------------------------------------------

    #[test]
    fn an_unresolved_symbol_diagnostic_qualifies_by_its_code() {
        for code in [
            json!("E0433"),
            json!(2304),
            json!("unresolved-import"),
            json!("reportUndefinedVariable"),
        ] {
            let diagnostic = json!({"code": code, "message": "irrelevant here"});
            assert!(
                suggests_organize_imports(&diagnostic),
                "expected {code} to qualify",
            );
        }
    }

    #[test]
    fn a_server_with_an_opaque_code_qualifies_by_its_message() {
        // jdtls reports `16777218`, which means nothing outside jdtls.
        let diagnostic = json!({
            "code": 16777218,
            "message": "HashMap cannot be resolved to a type",
        });
        assert!(suggests_organize_imports(&diagnostic));
    }

    #[test]
    fn the_message_match_is_case_insensitive() {
        assert!(suggests_organize_imports(
            &json!({"message": "Cannot find name 'foo'"})
        ));
        assert!(suggests_organize_imports(
            &json!({"message": "'foo' is not defined"})
        ));
    }

    #[test]
    fn an_ordinary_diagnostic_does_not_offer_to_organize_imports() {
        assert!(!suggests_organize_imports(&json!({
            "code": "E0308", "message": "mismatched types",
        })));
        assert!(!suggests_organize_imports(
            &json!({"message": "unused variable: `x`"})
        ));
        assert!(!suggests_organize_imports(&json!({})));
    }
}
