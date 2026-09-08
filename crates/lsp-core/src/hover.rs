//! What a `textDocument/hover` response means and how it is rendered — plus
//! the rule that decides whether a hover answer is still wanted by the time
//! it arrives.
//!
//! All three are rules, so they live here rather than in `bridge.rs` or
//! `cpp/` (`docs/architecture/layering.md`): the response has four legal
//! shapes, the Markdown-to-tooltip conversion has to stop somewhere, and a
//! dwell that resolves after the pointer moved on must be dropped rather
//! than shown at the new position.

use serde_json::Value;

use crate::manager::{position_params, LspError, HOVER_TIMEOUT};

/// Hover text as the server sent it, normalised to one string.
///
/// `markdown` distinguishes `MarkupContent { kind: "plaintext" }` (which must
/// not have its asterisks and backticks reinterpreted) from everything else —
/// the deprecated `MarkedString` forms are Markdown by definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverText {
    pub value: String,
    pub markdown: bool,
}

/// Parse a `textDocument/hover` result, across every shape servers send:
/// `MarkupContent`, a bare `MarkedString`, a `{language, value}`
/// `MarkedString`, and an array of the latter two. `None` for `null`, a
/// missing `contents`, or contents that render to nothing — all of which
/// mean "no hover here", not an error.
pub fn parse_hover(result: &Value) -> Option<HoverText> {
    let contents = result.get("contents")?;
    let hover = match contents {
        // MarkedString, plain form: Markdown by specification.
        Value::String(text) => HoverText {
            value: text.clone(),
            markdown: true,
        },
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(marked_string).collect();
            HoverText {
                value: parts.join("\n\n---\n\n"),
                markdown: true,
            }
        }
        Value::Object(_) => match contents.get("kind").and_then(Value::as_str) {
            // MarkupContent.
            Some(kind) => HoverText {
                value: contents.get("value")?.as_str()?.to_string(),
                markdown: kind != "plaintext",
            },
            // MarkedString, {language, value} form.
            None => HoverText {
                value: marked_string(contents)?,
                markdown: true,
            },
        },
        _ => return None,
    };
    (!hover.value.trim().is_empty()).then_some(hover)
}

/// One `MarkedString` array element as Markdown: a bare string as-is, a
/// `{language, value}` pair as the fenced block it is shorthand for.
fn marked_string(item: &Value) -> Option<String> {
    match item {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => {
            let value = item.get("value")?.as_str()?;
            let language = item.get("language").and_then(Value::as_str).unwrap_or("");
            Some(format!("```{language}\n{value}\n```"))
        }
        _ => None,
    }
}

/// Render hover text as the HTML subset Qt tooltips understand.
///
/// Deliberately not a Markdown engine: fenced and inline code, bold, thematic
/// breaks and paragraph breaks are converted because losing them makes a
/// signature unreadable; lists, links, tables and emphasis are left as their
/// source text, which is legible on its own. Everything is escaped first, so
/// nothing a server sends can inject markup.
pub fn to_tooltip_html(hover: &HoverText) -> String {
    if !hover.markdown {
        return format!("<pre>{}</pre>", escape(hover.value.trim_end()));
    }
    let mut out = String::new();
    let mut code = String::new();
    let mut in_code = false;
    let mut gap = false;

    for line in hover.value.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code {
                push_code(&mut out, &code);
                code.clear();
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        if trimmed.is_empty() {
            gap = !out.is_empty();
            continue;
        }
        if trimmed == "---" || trimmed == "___" || trimmed == "***" {
            out.push_str("<hr>");
            gap = false;
            continue;
        }
        if gap {
            out.push_str("<br>");
            gap = false;
        }
        out.push_str(&inline(line.trim_end()));
        out.push_str("<br>");
    }
    // An unterminated fence is a server bug, not a reason to lose the text.
    if in_code && !code.is_empty() {
        push_code(&mut out, &code);
    }
    out.trim_end_matches("<br>").to_string()
}

fn push_code(out: &mut String, code: &str) {
    out.push_str("<pre>");
    out.push_str(&escape(code.trim_end_matches('\n')));
    out.push_str("</pre>");
}

fn inline(text: &str) -> String {
    let escaped = escape(text);
    let coded = wrap_pairs(&escaped, "`", "<code>", "</code>");
    wrap_pairs(&coded, "**", "<b>", "</b>")
}

/// Wrap text between paired `delim`s. An unpaired trailing delimiter is left
/// as literal text rather than swallowing the rest of the line.
fn wrap_pairs(text: &str, delim: &str, open: &str, close: &str) -> String {
    let parts: Vec<&str> = text.split(delim).collect();
    if parts.len() < 3 {
        return text.to_string();
    }
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i % 2 == 1 {
            if i + 1 < parts.len() {
                out.push_str(open);
                out.push_str(part);
                out.push_str(close);
            } else {
                out.push_str(delim);
                out.push_str(part);
            }
        } else {
            out.push_str(part);
        }
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Who answers a hover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoverOutcome {
    /// The language server had something to say.
    Lsp(HoverText),
    /// It did not, so the declaration the name-based index resolves to is
    /// shown instead — which is what gives a signature tooltip in the many
    /// languages this IDE has a grammar but no server for.
    Index,
}

/// The precedence rule, the same shape as
/// [`crate::navigation::definition_outcome`]: a server's non-empty answer
/// wins, and everything else — no server for the language, none running yet,
/// an error, a timeout, an empty hover — falls back to the index.
///
/// `None` means no request was made at all.
pub fn hover_outcome(
    response: Option<Result<Option<HoverText>, crate::manager::LspError>>,
) -> HoverOutcome {
    match response {
        Some(Ok(Some(hover))) => HoverOutcome::Lsp(hover),
        _ => HoverOutcome::Index,
    }
}

/// Decides whether a hover response is still the one the user is waiting for.
///
/// A hover request is answered on a worker thread, so the pointer can move —
/// or leave the widget entirely — while it is in flight. Showing a late
/// answer would put the previous position's text under the current cursor,
/// which is worse than showing nothing, so every request carries a token and
/// only the newest token is accepted.
///
/// A thin, named wrapper over [`crate::RequestTracker`] rather than a type
/// alias: `HoverTracker` reads at every call site as what it tracks, and the
/// method names carry `HoverTracker`'s own doc comments instead of the
/// generic ones.
#[derive(Debug, Default)]
pub struct HoverTracker(crate::RequestTracker);

impl HoverTracker {
    /// Start a hover request, invalidating any still in flight.
    pub fn begin(&mut self) -> u64 {
        self.0.begin()
    }

    /// The pointer moved or left: nothing in flight is wanted any more.
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
    /// `textDocument/hover` for a position in an open document, already
    /// reduced to the one text the tooltip shows. `Ok(None)` means the server
    /// has nothing to say here, which is not an error.
    pub fn hover(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<HoverText>, LspError> {
        let uri = &self.normalize_uri(uri);
        let language_id = self.language_of(uri)?;
        let result = self.request_with_timeout(
            &language_id,
            "textDocument/hover",
            position_params(uri, line, character),
            HOVER_TIMEOUT,
        )?;
        Ok(parse_hover(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markup_content_is_parsed_with_its_kind() {
        let hover = parse_hover(&json!({"contents": {"kind": "markdown", "value": "**x**"}}));
        assert_eq!(
            hover,
            Some(HoverText {
                value: "**x**".into(),
                markdown: true
            })
        );

        let plain = parse_hover(&json!({"contents": {"kind": "plaintext", "value": "**x**"}}));
        assert!(!plain.unwrap().markdown, "plaintext stays literal");
    }

    #[test]
    fn a_bare_marked_string_is_markdown() {
        let hover = parse_hover(&json!({"contents": "fn main()"})).unwrap();
        assert_eq!(hover.value, "fn main()");
        assert!(hover.markdown);
    }

    #[test]
    fn a_language_marked_string_becomes_a_fenced_block() {
        let hover =
            parse_hover(&json!({"contents": {"language": "rust", "value": "fn main()"}})).unwrap();
        assert_eq!(hover.value, "```rust\nfn main()\n```");
    }

    #[test]
    fn an_array_of_marked_strings_is_joined_with_a_rule() {
        let hover = parse_hover(&json!({"contents": [
            {"language": "rust", "value": "fn main()"},
            "The entry point.",
        ]}))
        .unwrap();
        assert_eq!(
            hover.value,
            "```rust\nfn main()\n```\n\n---\n\nThe entry point."
        );
    }

    #[test]
    fn nothing_to_show_is_not_an_error() {
        assert!(parse_hover(&Value::Null).is_none());
        assert!(parse_hover(&json!({})).is_none());
        assert!(parse_hover(&json!({"contents": ""})).is_none());
        assert!(parse_hover(&json!({"contents": []})).is_none());
        assert!(parse_hover(&json!({"contents": {"kind": "markdown", "value": "  "}})).is_none());
    }

    #[test]
    fn fenced_code_becomes_a_pre_block() {
        let hover = HoverText {
            value: "```rust\nfn main() {}\n```\nThe entry point.".into(),
            markdown: true,
        };
        assert_eq!(
            to_tooltip_html(&hover),
            "<pre>fn main() {}</pre>The entry point."
        );
    }

    #[test]
    fn inline_markup_is_converted_and_html_is_escaped() {
        let hover = HoverText {
            value: "**Vec**<T> holds `T` & more".into(),
            markdown: true,
        };
        assert_eq!(
            to_tooltip_html(&hover),
            "<b>Vec</b>&lt;T&gt; holds <code>T</code> &amp; more"
        );
    }

    #[test]
    fn an_unpaired_delimiter_stays_literal() {
        let hover = HoverText {
            value: "a * b `unclosed".into(),
            markdown: true,
        };
        assert_eq!(to_tooltip_html(&hover), "a * b `unclosed");
    }

    #[test]
    fn plaintext_hover_is_preformatted_and_never_reinterpreted() {
        let hover = HoverText {
            value: "a **b** <c>".into(),
            markdown: false,
        };
        assert_eq!(to_tooltip_html(&hover), "<pre>a **b** &lt;c&gt;</pre>");
    }

    #[test]
    fn blank_lines_and_rules_become_breaks() {
        let hover = HoverText {
            value: "one\n\ntwo\n---\nthree".into(),
            markdown: true,
        };
        assert_eq!(to_tooltip_html(&hover), "one<br><br>two<br><hr>three");
    }

    #[test]
    fn an_unterminated_fence_still_renders_its_code() {
        let hover = HoverText {
            value: "```\nfn main() {}".into(),
            markdown: true,
        };
        assert_eq!(to_tooltip_html(&hover), "<pre>fn main() {}</pre>");
    }

    #[test]
    fn only_the_newest_hover_response_is_accepted() {
        let mut tracker = HoverTracker::default();
        let first = tracker.begin();
        let second = tracker.begin();
        assert!(!tracker.accept(first), "the pointer moved on");
        assert!(tracker.accept(second));
    }

    #[test]
    fn a_cancelled_hover_is_discarded() {
        let mut tracker = HoverTracker::default();
        let token = tracker.begin();
        tracker.cancel();
        assert!(!tracker.accept(token));
    }
    #[test]
    fn a_servers_hover_wins() {
        let hover = HoverText {
            value: "fn main()".into(),
            markdown: false,
        };
        assert_eq!(
            hover_outcome(Some(Ok(Some(hover.clone())))),
            HoverOutcome::Lsp(hover),
        );
    }

    #[test]
    fn every_other_case_falls_back_to_the_declaration() {
        assert_eq!(hover_outcome(None), HoverOutcome::Index);
        assert_eq!(hover_outcome(Some(Ok(None))), HoverOutcome::Index);
        assert_eq!(
            hover_outcome(Some(Err(crate::manager::LspError::NoServer("zig".into())))),
            HoverOutcome::Index,
        );
        assert_eq!(
            hover_outcome(Some(Err(crate::manager::LspError::Timeout {
                method: "textDocument/hover".into()
            }))),
            HoverOutcome::Index,
        );
    }
}
