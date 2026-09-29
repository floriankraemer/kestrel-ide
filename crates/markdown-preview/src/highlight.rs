//! Fenced code, painted with the editor's own tree-sitter highlighter and
//! theme — so the preview's colours and the editor's agree by
//! construction, rather than by a second copy of the same table.
//!
//! Also where a ```mermaid fence leaves the normal code-fence path
//! entirely: it is not "code" to highlight, it is a diagram to record and
//! swap for an `<img>` placeholder. Comrak calls the same three methods
//! either way, so the mermaid branch lives beside the highlighting one
//! rather than as a second adapter.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use comrak::adapters::SyntaxHighlighterAdapter;
use comrak::html::escape;
use std::borrow::Cow;
use syntax_core::theme::{palette, Palette, ScopeStyle, UserStyles};

use crate::html::{diagram_key, Diagram};

/// Fenced code becomes coloured `<span>`s painted from this theme, by the
/// same [`syntax_core::highlight`] table the editor uses. `mermaid` is not
/// a `syntax-core` language at all — it is intercepted before a language
/// lookup is even attempted.
pub(crate) struct Highlighter {
    theme_name: String,
    /// One resolved [`Palette`] per language actually seen, built lazily
    /// and keyed by owned `String` rather than the language id's
    /// `&'static str`: a `Highlighter` is built fresh per [`crate::render`]
    /// call, so nothing here needs to outlive it, and a `String` key means
    /// no per-render heap leak on a document that is re-rendered on every
    /// keystroke.
    ///
    /// `Mutex` rather than `RefCell`: comrak's `SyntaxHighlighterAdapter`
    /// requires `Sync`, which a `RefCell` never is. Nothing here is
    /// actually contended — one `Highlighter` serves exactly one `render`
    /// call, single-threaded — so the lock is a type-system formality, not
    /// a performance concern.
    palettes: Mutex<HashMap<String, Palette>>,
    /// Every ```mermaid fence encountered, in document order. `render`
    /// drains this after formatting.
    pub(crate) diagrams: Mutex<Vec<Diagram>>,
}

impl Highlighter {
    pub(crate) fn new(theme_name: &str) -> Self {
        Self {
            theme_name: theme_name.to_string(),
            palettes: Mutex::new(HashMap::new()),
            diagrams: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn into_diagrams(self) -> Vec<Diagram> {
        self.diagrams
            .into_inner()
            .expect("highlighter mutex poisoned")
    }
}

const MERMAID: &str = "mermaid";

impl SyntaxHighlighterAdapter for Highlighter {
    fn write_highlighted(
        &self,
        output: &mut dyn fmt::Write,
        lang: Option<&str>,
        code: &str,
    ) -> fmt::Result {
        if lang == Some(MERMAID) {
            let key = diagram_key(code);
            self.diagrams
                .lock()
                .expect("highlighter mutex poisoned")
                .push(Diagram {
                    key: key.clone(),
                    source: code.to_string(),
                });
            write!(output, "<img src=\"ide-preview:{key}\">")?;
            return Ok(());
        }

        let Some(language) = lang.and_then(syntax_core::language_by_id) else {
            return escape(output, code);
        };

        let mut palettes = self.palettes.lock().expect("highlighter mutex poisoned");
        let language_id = lang.expect("checked above");
        let resolved = palettes
            .entry(language_id.to_string())
            .or_insert_with(|| palette(&self.theme_name, language_id, &UserStyles::default()));

        write_coloured(output, language, code, resolved)
    }

    fn write_pre_tag(
        &self,
        _output: &mut dyn fmt::Write,
        _attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        // A mermaid fence has no `<pre>` of its own: comrak still appends
        // `"</code></pre>"` after `write_highlighted` regardless of what
        // this method writes (`render_code_block` in comrak's own
        // `html.rs` does that unconditionally), so the closing tags are
        // unmatched here on purpose. Every HTML parser lenient enough to
        // run inside `QTextDocument` — which this one is, by construction
        // (`html::tests::qt_rich_text_tolerates_an_unmatched_closing_tag`)
        // — treats a stray close as a no-op rather than a corruption.
        Ok(())
    }

    fn write_code_tag(
        &self,
        _output: &mut dyn fmt::Write,
        _attributes: HashMap<&'static str, Cow<'_, str>>,
    ) -> fmt::Result {
        Ok(())
    }
}

/// `code` as HTML, its tokens coloured from `palette`.
fn write_coloured(
    output: &mut dyn fmt::Write,
    language: syntax_core::Language,
    code: &str,
    palette: &Palette,
) -> fmt::Result {
    let mut last = 0;
    for span in syntax_core::highlight(language, code) {
        if span.start > last {
            escape(output, &code[last..span.start])?;
        }
        write_span(
            output,
            &palette.style(span.scope),
            &code[span.start..span.end],
        )?;
        last = span.end;
    }
    if last < code.len() {
        escape(output, &code[last..])?;
    }
    Ok(())
}

/// `code` in `language_id` as coloured spans, for callers that paint a
/// snippet with their own resolved [`Palette`] (the hover card's signature).
/// `None` when the language is unknown, so the caller escapes it plain.
pub fn highlight_html(code: &str, language_id: &str, palette: &Palette) -> Option<String> {
    let language = syntax_core::language_by_id(language_id)?;
    let mut out = String::new();
    write_coloured(&mut out, language, code, palette).ok()?;
    Some(out)
}

fn write_span(output: &mut dyn fmt::Write, style: &ScopeStyle, text: &str) -> fmt::Result {
    let Some(fg) = style.fg else {
        return escape(output, text);
    };
    let mut css = format!("color:#{:02x}{:02x}{:02x}", fg.r, fg.g, fg.b);
    if style.bold {
        css.push_str(";font-weight:bold");
    }
    if style.italic {
        css.push_str(";font-style:italic");
    }
    write!(output, "<span style=\"{css}\">")?;
    escape(output, text)?;
    output.write_str("</span>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snippet_is_coloured_from_the_given_palette() {
        let palette = palette("dark", "rust", &UserStyles::default());
        let html = highlight_html("fn f<T>()", "rust", &palette).expect("rust is known");
        assert!(html.contains("<span style=\"color:#"), "{html}");
        assert!(
            html.contains("&lt;") && !html.contains("<T>"),
            "escaped: {html}"
        );
    }

    #[test]
    fn an_unknown_language_is_left_to_the_caller() {
        let palette = palette("dark", "rust", &UserStyles::default());
        assert_eq!(highlight_html("x", "no-such-language", &palette), None);
    }
}
