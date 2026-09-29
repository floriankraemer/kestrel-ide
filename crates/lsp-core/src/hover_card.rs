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
}

impl Default for CardLabels {
    fn default() -> Self {
        CardLabels {
            loading_fixes: "Loading fixes…".into(),
            more_actions: "More actions…".into(),
            source: "Source:".into(),
            error: "Error".into(),
            warning: "Warning".into(),
            info: "Info".into(),
            hint: "Hint".into(),
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
    let mut fixes = intentions
        .iter()
        .filter(|i| i.group == IntentionGroup::QuickFix && i.disabled().is_none());
    let first = fixes.clone().find(|i| i.preferred);
    first.or_else(|| fixes.next())
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
        FixState::Some {
            primary_title,
            count,
        } => {
            let more = if *count > 1 {
                format!(
                    " <a href=\"ide:more/{index}\">{}</a>",
                    escape(&labels.more_actions)
                )
            } else {
                String::new()
            };
            format!(
                "<br><a class=\"fix\" href=\"ide:fix/{index}\">{}</a>{more}",
                escape(primary_title)
            )
        }
    };
    format!(
        "<p class=\"problem\"><img src=\"ide-sev:{class}\" width=\"14\" height=\"14\" alt=\"{}\" style=\"vertical-align:middle\"> {}{origin}{fixes}</p>",
        escape(label),
        escape(&problem.message)
    )
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
        assert!(html.contains("href=\"ide:fix/1\"") && !html.contains("ide:more/1"));
        assert!(html.contains("Loading fixes"));
        assert!(!html.contains("ide:fix/2") && !html.contains("ide:fix/3"));
        assert!(!html.contains("ide:source"));
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
        assert!(!html.contains("Loading") && !html.contains("Source:"));
    }

    #[test]
    fn empty_card_renders_nothing() {
        assert_eq!(render(&HoverCard::default(), &CardLabels::default()), "");
    }
}
