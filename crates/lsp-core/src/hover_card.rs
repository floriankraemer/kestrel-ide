//! What the hover card contains and how it becomes HTML.
//!
//! The card is problems first (each with its fix state), then the signature,
//! then the documentation, then a "Source:" footer. Deciding that structure,
//! which fix is primary and how untrusted server text is escaped are rules,
//! so they live here rather than in `bridge/` or `cpp/`
//! (`docs/architecture/layering.md`).
//!
//! # Styling hooks
//!
//! No colour is chosen here. Elements carry semantic `class` names
//! (`problem`, `dim`, `sec`/`sep` for section cells,
//! `signature`, `doc`, `source`, `fix`) that the popup's
//! `QTextDocument::setDefaultStyleSheet` maps onto the active theme's
//! `SemanticColors`. Without a stylesheet the card degrades to plain,
//! readable rich text. Severity icons are `<img src="ide-sev:error|warning|info|hint">`
//! images the popup paints and registers as document resources.

use diagnostics_core::{DiagnosticRow, Severity};

use crate::intentions::{Intention, IntentionGroup};

/// Whether fixes for one problem are known yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixState {
    /// Asked of the server, no answer yet.
    Loading,
    /// The server offered nothing (or nothing usable).
    None,
    Some {
        primary_title: String,
        count: usize,
    },
}

impl FixState {
    /// What the server's answer for one problem means for its fix row:
    /// `Some` when there is a primary fix (`count` is the whole list the
    /// "More actions…" menu will show), `None` otherwise.
    pub fn of(intentions: &[Intention]) -> Self {
        match primary_fix(intentions) {
            Some(primary) => FixState::Some {
                primary_title: primary.title().to_string(),
                count: intentions.len(),
            },
            None => FixState::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardProblem {
    pub severity: Severity,
    pub message: String,
    pub source: String,
    pub code: String,
    pub fixes: FixState,
}

impl CardProblem {
    /// A problem with its fixes not yet requested-or-known as absent.
    pub fn from_row(row: &DiagnosticRow) -> Self {
        CardProblem {
            severity: row.severity,
            message: row.message.clone(),
            source: row.source.clone(),
            code: row.code.clone(),
            fixes: FixState::None,
        }
    }
}

/// The intentions fetched for the problems of the hover card that is on
/// screen, tagged with that card's hover token.
///
/// Fixes are usable only while their card is the one showing: the holder
/// is emptied when the popup shows anything else or closes, and an answer
/// for any other token is refused, so a shortcut can never apply a fix the
/// user is no longer looking at.
#[derive(Debug, Default)]
pub struct HoverFixes {
    token: Option<u64>,
    per_problem: Vec<Vec<Intention>>,
}

impl HoverFixes {
    /// A card for `token` with `problems` problems went up; nothing is known yet.
    pub fn show(&mut self, token: u64, problems: usize) {
        self.token = Some(token);
        self.per_problem = vec![Vec::new(); problems];
    }

    /// The card is gone (closed, or the popup shows something else).
    pub fn clear(&mut self) {
        self.token = None;
        self.per_problem.clear();
    }

    /// Record `problem`'s answer. False (and nothing stored) when `token` is
    /// not the showing card's or `problem` does not exist.
    pub fn fill(&mut self, token: u64, problem: usize, list: Vec<Intention>) -> bool {
        if self.token != Some(token) {
            return false;
        }
        match self.per_problem.get_mut(problem) {
            Some(slot) => {
                *slot = list;
                true
            }
            None => false,
        }
    }

    pub fn list(&self, problem: usize) -> Option<&[Intention]> {
        self.per_problem.get(problem).map(Vec::as_slice)
    }

    /// The first problem that has a primary fix.
    pub fn first_fixable(&self) -> Option<usize> {
        self.per_problem
            .iter()
            .position(|list| primary_fix_index(list).is_some())
    }
}

/// Where the hovered symbol is declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLocation {
    pub path: String,
    pub line: u32,
}

/// The card's fixed words. The UI layer owns translation, so it hands the
/// localized strings in; the default is the English source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLabels {
    pub loading_fixes: String,
    pub more_actions: String,
    pub source: String,
    pub error: String,
    pub warning: String,
    pub info: String,
    pub hint: String,
    /// The user's actual bindings for "apply preferred fix" and "show
    /// intention actions", shown dim after the fix row's links; empty
    /// (unbound) shows nothing.
    pub apply_fix_shortcut: String,
    pub more_actions_shortcut: String,
}

impl Default for CardLabels {
    fn default() -> Self {
        CardLabels {
            loading_fixes: "Looking for fixes…".into(),
            more_actions: "More actions…".into(),
            source: "Source:".into(),
            error: "Error".into(),
            warning: "Warning".into(),
            info: "Info".into(),
            hint: "Hint".into(),
            apply_fix_shortcut: String::new(),
            more_actions_shortcut: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HoverCard {
    pub problems: Vec<CardProblem>,
    pub signature: Option<String>,
    /// Documentation already rendered to HTML by the caller's Markdown
    /// engine (which lives beside `markdown-preview`, not in this crate).
    pub doc_html: Option<String>,
    pub source: Option<CardLocation>,
}

/// Split a Markdown hover into its signature and the remaining Markdown.
///
/// Every server this IDE ships (rust-analyzer, intelephense, pyright,
/// tsserver) sends the signature as the first fenced code block. That block
/// is lifted out; whatever surrounds it is the body, minus the `---` rule
/// servers put between signature and prose (the card draws its own
/// separation). No fence means the whole text is body. An unterminated fence
/// runs to the end of the text.
pub fn split_signature(markdown: &str) -> (Option<String>, String) {
    let lines: Vec<&str> = markdown.lines().collect();
    let is_fence = |line: &str| line.trim_start().starts_with("```");
    let Some(open) = lines.iter().position(|l| is_fence(l)) else {
        return (None, markdown.trim().to_string());
    };
    let close = lines[open + 1..]
        .iter()
        .position(|l| is_fence(l))
        .map_or(lines.len(), |i| open + 1 + i);
    let signature = lines[open + 1..close].join("\n");
    let mut after = lines.get(close + 1..).unwrap_or(&[]);
    while after.first().is_some_and(|l| l.trim().is_empty()) {
        after = &after[1..];
    }
    if after.first().is_some_and(|l| l.trim() == "---") {
        after = &after[1..];
    }
    let rest = lines[..open]
        .iter()
        .chain(after)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let body = rest.trim().to_string();
    let signature = (!signature.trim().is_empty()).then(|| signature.trim_end().to_string());
    (signature, body)
}

/// The fix the card offers inline: the first `preferred` enabled quick fix,
/// else the first enabled quick fix. Disabled actions are never primary —
/// they cannot be applied here.
pub fn primary_fix(intentions: &[Intention]) -> Option<&Intention> {
    primary_fix_index(intentions).map(|i| &intentions[i])
}

/// [`primary_fix`] as a position in `intentions`, for callers that apply
/// the fix by its index in the list the menu shows.
pub fn primary_fix_index(intentions: &[Intention]) -> Option<usize> {
    let usable = |i: &Intention| i.group == IntentionGroup::QuickFix && i.disabled().is_none();
    let position = |want_preferred: bool| {
        intentions
            .iter()
            .position(|i| usable(i) && (!want_preferred || i.preferred))
    };
    position(true).or_else(|| position(false))
}

/// The card as HTML in the subset `QTextDocument` understands. Every string
/// that came from a server or the filesystem is escaped (ADR-0021).
///
/// Anchors: `ide:fix/<problem>`, `ide:more/<problem>`, `ide:source`.
pub fn render(card: &HoverCard, labels: &CardLabels) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !card.problems.is_empty() {
        let rows: String = card
            .problems
            .iter()
            .enumerate()
            .map(|(i, p)| render_problem(i, p, labels))
            .collect();
        sections.push(format!("<div class=\"problems\">{rows}</div>"));
    }
    if let Some(signature) = &card.signature {
        sections.push(format!(
            "<pre class=\"signature\">{}</pre>",
            escape(signature)
        ));
    }
    if let Some(doc) = card.doc_html.as_deref().filter(|d| !d.is_empty()) {
        sections.push(format!("<div class=\"doc\">{doc}</div>"));
    }
    if let Some(location) = &card.source {
        sections.push(format!(
            "<p class=\"source\">{} <a href=\"ide:source\">{}:{}</a></p>",
            escape(&labels.source),
            escape(&location.path),
            location.line
        ));
    }
    sections
        .iter()
        .enumerate()
        .map(|(i, html)| {
            // `<hr>` is not drawn by `QTextDocument` under a stylesheet, but a
            // table cell's `border-top` is: every section after the first
            // opens with the `sep` rule.
            let class = if i == 0 { "sec" } else { "sep" };
            format!(
                "<table width=\"100%\" cellspacing=\"0\" cellpadding=\"0\"><tr><td class=\"{class}\">{html}</td></tr></table>"
            )
        })
        .collect()
}

fn render_problem(index: usize, problem: &CardProblem, labels: &CardLabels) -> String {
    let (class, label) = match problem.severity {
        Severity::Error => ("error", &labels.error),
        Severity::Warning => ("warning", &labels.warning),
        Severity::Information => ("info", &labels.info),
        Severity::Hint => ("hint", &labels.hint),
    };
    let origin = match (problem.source.is_empty(), problem.code.is_empty()) {
        (true, true) => String::new(),
        (false, true) => escape(&problem.source),
        (true, false) => escape(&problem.code),
        (false, false) => format!("{} {}", escape(&problem.source), escape(&problem.code)),
    };
    let origin = if origin.is_empty() {
        origin
    } else {
        format!(" <span class=\"dim\">{origin}</span>")
    };
    let fixes = match &problem.fixes {
        FixState::None => String::new(),
        FixState::Loading => format!(
            "<br><span class=\"dim\">{}</span>",
            escape(&labels.loading_fixes)
        ),
        FixState::Some { primary_title, .. } => {
            format!(
                "<br><a class=\"fix\" href=\"ide:fix/{index}\">{}</a>{} &nbsp; <a href=\"ide:more/{index}\">{}</a>{}",
                escape(primary_title),
                shortcut_hint(&labels.apply_fix_shortcut),
                escape(&labels.more_actions),
                shortcut_hint(&labels.more_actions_shortcut),
            )
        }
    };
    format!(
        "<p class=\"problem\"><img src=\"ide-sev:{class}\" width=\"14\" height=\"14\" alt=\"{}\" style=\"vertical-align:middle\"> {}{origin}{fixes}</p>",
        escape(label),
        escape(&problem.message)
    )
}

fn shortcut_hint(shortcut: &str) -> String {
    if shortcut.is_empty() {
        String::new()
    } else {
        format!(" <span class=\"dim\">{}</span>", escape(shortcut))
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_action::CodeActionItem;
    use serde_json::json;

    fn intention(title: &str, kind: &str, preferred: bool, disabled: bool) -> Intention {
        Intention {
            item: CodeActionItem {
                title: title.into(),
                kind: Some(kind.into()),
                edit: None,
                command: None,
                disabled: disabled.then(|| "nope".to_string()),
                raw: json!({ "isPreferred": preferred }),
            },
            group: IntentionGroup::of(Some(kind)),
            preferred,
        }
    }

    fn problem(message: &str, source: &str, code: &str, fixes: FixState) -> CardProblem {
        CardProblem {
            severity: Severity::Error,
            message: message.into(),
            source: source.into(),
            code: code.into(),
            fixes,
        }
    }

    #[test]
    fn rust_analyzer_shape_lifts_first_block_and_drops_the_rule() {
        let md = "```rust\nstd::collections\n```\n\n```rust\npub struct HashMap<K, V>\n```\n\n---\n\nA hash map.";
        let (sig, body) = split_signature(md);
        assert_eq!(sig.as_deref(), Some("std::collections"));
        assert_eq!(
            body,
            "```rust\npub struct HashMap<K, V>\n```\n\n---\n\nA hash map."
        );
    }

    #[test]
    fn intelephense_shape_signature_then_prose() {
        let (sig, body) = split_signature("```php\n<?php\nfunction f(): int\n```\n\nReturns one.");
        assert_eq!(sig.as_deref(), Some("<?php\nfunction f(): int"));
        assert_eq!(body, "Returns one.");
    }

    #[test]
    fn pyright_shape_with_prose_before_the_fence() {
        let (sig, body) = split_signature(
            "(function) def f(x: int) -> int\n\n```python\ndef f(x: int) -> int\n```\n---\nDocs",
        );
        assert_eq!(sig.as_deref(), Some("def f(x: int) -> int"));
        assert_eq!(body, "(function) def f(x: int) -> int\n\nDocs");
    }

    #[test]
    fn plaintext_and_unterminated_fence() {
        assert_eq!(split_signature("just text\n"), (None, "just text".into()));
        let (sig, body) = split_signature("```c\nint x;");
        assert_eq!(sig.as_deref(), Some("int x;"));
        assert_eq!(body, "");
    }

    #[test]
    fn primary_prefers_preferred_then_first_enabled_fix() {
        let list = [
            intention("plain", "quickfix", false, false),
            intention("best", "quickfix", true, false),
        ];
        assert_eq!(primary_fix(&list).unwrap().title(), "best");
        let list = [
            intention("refactor", "refactor", true, false),
            intention("first", "quickfix", false, false),
            intention("second", "quickfix", false, false),
        ];
        assert_eq!(primary_fix(&list).unwrap().title(), "first");
    }

    #[test]
    fn primary_skips_disabled_and_can_be_none() {
        let list = [
            intention("off", "quickfix", true, true),
            intention("on", "quickfix", false, false),
        ];
        assert_eq!(primary_fix(&list).unwrap().title(), "on");
        assert!(primary_fix(&[intention("off", "quickfix", true, true)]).is_none());
        assert!(primary_fix(&[intention("r", "refactor", true, false)]).is_none());
        assert!(primary_fix(&[]).is_none());
    }

    #[test]
    fn render_orders_problems_signature_doc_source() {
        let card = HoverCard {
            problems: vec![problem("bad", "rustc", "E0412", FixState::None)],
            signature: Some("fn f()".into()),
            doc_html: Some("<p>docs</p>".into()),
            source: Some(CardLocation {
                path: "/a.rs".into(),
                line: 3,
            }),
        };
        let html = render(&card, &CardLabels::default());
        let at = |needle: &str| {
            html.find(needle)
                .unwrap_or_else(|| panic!("{needle} in {html}"))
        };
        assert!(at("class=\"problem\"") < at("class=\"signature\""));
        assert!(at("class=\"signature\"") < at("class=\"doc\""));
        assert!(at("class=\"doc\"") < at("class=\"source\""));
        assert!(html.contains("<span class=\"dim\">rustc E0412</span>"));
        assert!(html.contains("<a href=\"ide:source\">/a.rs:3</a>"));
    }

    #[test]
    fn render_escapes_every_server_string() {
        let card = HoverCard {
            problems: vec![problem(
                "<img src=x> & \"q\"",
                "<s>",
                "<c>",
                FixState::Some {
                    primary_title: "<b>Fix</b>".into(),
                    count: 1,
                },
            )],
            signature: Some("fn f<T>() -> &str".into()),
            doc_html: None,
            source: Some(CardLocation {
                path: "/<x>.rs".into(),
                line: 1,
            }),
        };
        let html = render(&card, &CardLabels::default());
        assert!(!html.contains("<img src=x") && !html.contains("<s>") && !html.contains("<c>"));
        assert!(html.contains("&lt;img src=x&gt; &amp; &quot;q&quot;"));
        assert!(html.contains("fn f&lt;T&gt;() -&gt; &amp;str"));
        assert!(html.contains("&lt;b&gt;Fix&lt;/b&gt;"));
        assert!(html.contains("/&lt;x&gt;.rs:1"));
    }

    #[test]
    fn fix_anchors_follow_state() {
        let some = |count| FixState::Some {
            primary_title: "Import".into(),
            count,
        };
        let card = HoverCard {
            problems: vec![
                problem("a", "", "", some(3)),
                problem("b", "", "", some(1)),
                problem("c", "", "", FixState::Loading),
                problem("d", "", "", FixState::None),
            ],
            ..HoverCard::default()
        };
        let html = render(&card, &CardLabels::default());
        assert!(html.contains("href=\"ide:fix/0\"") && html.contains("href=\"ide:more/0\""));
        assert!(html.contains("href=\"ide:fix/1\"") && html.contains("href=\"ide:more/1\""));
        assert!(html.contains("Looking for fixes"));
        assert!(!html.contains("ide:fix/2") && !html.contains("ide:fix/3"));
        assert!(!html.contains("ide:source"));
    }

    #[test]
    fn fix_row_shows_the_bound_shortcuts_only_when_bound() {
        let card = HoverCard {
            problems: vec![problem(
                "a",
                "",
                "",
                FixState::Some {
                    primary_title: "Import".into(),
                    count: 1,
                },
            )],
            ..HoverCard::default()
        };
        let bare = render(&card, &CardLabels::default());
        assert!(!bare.contains("Alt+"));
        let labels = CardLabels {
            apply_fix_shortcut: "Alt+Shift+Enter".into(),
            more_actions_shortcut: "Alt+Enter".into(),
            ..CardLabels::default()
        };
        let html = render(&card, &labels);
        assert!(html.contains("Import</a> <span class=\"dim\">Alt+Shift+Enter</span>"));
        assert!(html.contains("More actions…</a> <span class=\"dim\">Alt+Enter</span>"));
    }

    #[test]
    fn fix_state_follows_the_answer() {
        let list = [
            intention("plain", "quickfix", false, false),
            intention("r", "refactor", false, false),
        ];
        assert_eq!(
            FixState::of(&list),
            FixState::Some {
                primary_title: "plain".into(),
                count: 2
            }
        );
        assert_eq!(FixState::of(&list[1..]), FixState::None);
        assert_eq!(FixState::of(&[]), FixState::None);
    }

    #[test]
    fn primary_index_points_into_the_menu_list() {
        let list = [
            intention("r", "refactor", true, false),
            intention("a", "quickfix", false, false),
            intention("b", "quickfix", true, false),
        ];
        assert_eq!(primary_fix_index(&list), Some(2));
        assert_eq!(primary_fix_index(&list[..2]), Some(1));
        assert_eq!(primary_fix_index(&[]), None);
    }

    #[test]
    fn hover_fixes_belong_to_their_card_only() {
        let mut fixes = HoverFixes::default();
        assert_eq!(fixes.first_fixable(), None);
        fixes.show(7, 2);
        assert!(!fixes.fill(6, 0, vec![intention("x", "quickfix", true, false)]));
        assert!(!fixes.fill(7, 5, Vec::new()));
        assert_eq!(fixes.first_fixable(), None);
        assert!(fixes.fill(7, 1, vec![intention("x", "quickfix", true, false)]));
        assert_eq!(fixes.first_fixable(), Some(1));
        assert_eq!(fixes.list(1).unwrap().len(), 1);
        fixes.clear();
        assert_eq!(fixes.first_fixable(), None);
        assert!(!fixes.fill(7, 1, vec![intention("x", "quickfix", true, false)]));
        assert!(fixes.list(1).is_none());
    }

    #[test]
    fn a_new_card_forgets_the_old_ones_fixes() {
        let mut fixes = HoverFixes::default();
        fixes.show(1, 1);
        fixes.fill(1, 0, vec![intention("x", "quickfix", true, false)]);
        fixes.show(2, 1);
        assert_eq!(fixes.first_fixable(), None);
    }

    #[test]
    fn loading_row_is_dim_and_has_no_anchors() {
        let card = HoverCard {
            problems: vec![problem("a", "", "", FixState::Loading)],
            ..HoverCard::default()
        };
        let html = render(&card, &CardLabels::default());
        assert!(html.contains("<span class=\"dim\">Looking for fixes…</span>"));
        assert!(!html.contains("ide:"));
    }

    #[test]
    fn severity_classes_and_missing_origin() {
        let mut p = problem("m", "", "", FixState::None);
        p.severity = Severity::Warning;
        let html = render(
            &HoverCard {
                problems: vec![p],
                ..HoverCard::default()
            },
            &CardLabels::default(),
        );
        assert!(html.contains("src=\"ide-sev:warning\"") && html.contains("alt=\"Warning\""));
        assert!(!html.contains("dim"));
    }

    #[test]
    fn labels_replace_every_fixed_word_and_are_escaped() {
        let labels = CardLabels {
            loading_fixes: "Lade".into(),
            more_actions: "Mehr <".into(),
            source: "Quelle:".into(),
            error: "Fehler".into(),
            ..CardLabels::default()
        };
        let mut loading = problem("m", "", "", FixState::Loading);
        loading.severity = Severity::Error;
        let more = problem(
            "n",
            "",
            "",
            FixState::Some {
                primary_title: "F".into(),
                count: 2,
            },
        );
        let html = render(
            &HoverCard {
                problems: vec![loading, more],
                source: Some(CardLocation {
                    path: "/a".into(),
                    line: 1,
                }),
                ..HoverCard::default()
            },
            &labels,
        );
        for want in ["Lade", "Mehr &lt;", "Quelle: <a", "alt=\"Fehler\""] {
            assert!(html.contains(want), "{want} in {html}");
        }
        assert!(!html.contains("Looking") && !html.contains("Source:"));
    }

    #[test]
    fn empty_card_renders_nothing() {
        assert_eq!(render(&HoverCard::default(), &CardLabels::default()), "");
    }
}
