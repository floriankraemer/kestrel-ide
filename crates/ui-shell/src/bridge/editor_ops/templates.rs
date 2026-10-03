//! Live templates at the editor seam (ADR-0072): translation only.
//!
//! Which template fires, where it fits and what it expands to are
//! `settings_model::live_templates`' and `edit_ops::templates`' decisions;
//! this file reads the tab's caret, hands the text over, and turns the
//! answer into the edits and tab stops the view applies.

use cxx_qt_lib::QString;
use edit_ops::templates::Expansion;
use editor_core::offsets::{line_starts, utf16_offset};
use editor_core::transaction::Transaction;
use settings_model::live_templates::{self, LiveTemplate};

use super::{language_of, position_at, snippet_stop, EditorOpsRust};
use crate::bridge::ffi;

/// Kind 15 is LSP's `Snippet`, which is how the popup icons a template.
const SNIPPET_KIND: u32 = 15;

impl EditorOpsRust {
    fn live_templates(&self) -> Vec<LiveTemplate> {
        let registry = plugin_host::registry();
        live_templates::resolve(
            registry.live_templates().map(|(_, template)| template),
            &self.settings.borrow().live_templates,
        )
    }

    /// The tab's caret, when it is one bare caret: templates expand at a
    /// caret, never over a selection or at several places at once.
    fn bare_caret(&self, tab_id: u64) -> Option<usize> {
        let selection = self.selection_of(tab_id);
        let caret = selection.primary();
        (!selection.is_multi() && caret.is_empty()).then_some(caret.head)
    }

    /// Splice `expansion` in as one transaction and start the snippet
    /// session over its tab stops, exactly as accepting an LSP snippet does.
    fn apply_expansion(
        &self,
        tab_id: u64,
        text: &str,
        expansion: Expansion,
    ) -> ffi::FfiTemplateExpansion {
        let mut edited = text.to_string();
        edited.replace_range(expansion.edit.range.clone(), &expansion.edit.text);
        let edits = self.commit(tab_id, text, Transaction::new(vec![expansion.edit]));
        let utf16 = |byte: usize| utf16_offset(&edited, byte);
        let ranges = expansion
            .stops
            .iter()
            .map(|stop| utf16(stop.start)..utf16(stop.end))
            .collect();
        let stop = match editor_core::SnippetSession::new(ranges) {
            Some(session) => {
                let current = session.current();
                let more = !session.is_last();
                self.tabs.borrow_mut().entry(tab_id).or_default().snippet = more.then_some(session);
                snippet_stop(current, more)
            }
            None => {
                self.tabs.borrow_mut().entry(tab_id).or_default().snippet = None;
                let caret = utf16(expansion.caret);
                snippet_stop(caret..caret, false)
            }
        };
        ffi::FfiTemplateExpansion {
            applied: true,
            edits,
            stop,
        }
    }

    fn template_items(templates: &[&LiveTemplate]) -> Vec<ffi::FfiTemplateItem> {
        templates
            .iter()
            .map(|t| ffi::FfiTemplateItem {
                abbreviation: QString::from(t.abbreviation.as_str()),
                description: QString::from(t.description.as_str()),
            })
            .collect()
    }
}

impl ffi::EditorOps {
    /// Tab: expand the template named at the caret, if any.
    pub fn expand_template(&self, tab_id: u64, text: &QString) -> ffi::FfiTemplateExpansion {
        let text = text.to_string();
        let Some(caret) = self.bare_caret(tab_id) else {
            return ffi::FfiTemplateExpansion::default();
        };
        let language = language_of(&self.session.borrow(), tab_id);
        let style = self.indent_style(language);
        live_templates::expand_before_caret(&self.live_templates(), language, &text, caret, style)
            .map_or_else(Default::default, |e| self.apply_expansion(tab_id, &text, e))
    }

    /// Ctrl+J: the templates that fit the caret's place.
    pub fn insertable_templates(&self, tab_id: u64, text: &QString) -> Vec<ffi::FfiTemplateItem> {
        let Some(caret) = self.bare_caret(tab_id) else {
            return Vec::new();
        };
        let language = language_of(&self.session.borrow(), tab_id);
        let all = self.live_templates();
        EditorOpsRust::template_items(&live_templates::insertable(
            &all,
            language,
            &text.to_string(),
            caret,
        ))
    }

    pub fn insert_template(
        &self,
        tab_id: u64,
        text: &QString,
        abbreviation: &QString,
    ) -> ffi::FfiTemplateExpansion {
        let text = text.to_string();
        let Some(caret) = self.bare_caret(tab_id) else {
            return ffi::FfiTemplateExpansion::default();
        };
        let language = language_of(&self.session.borrow(), tab_id);
        let style = self.indent_style(language);
        live_templates::insert(
            &self.live_templates(),
            language,
            &text,
            caret,
            &abbreviation.to_string(),
            style,
        )
        .map_or_else(Default::default, |e| self.apply_expansion(tab_id, &text, e))
    }

    /// Ctrl+Alt+T: the templates that can wrap a selection.
    pub fn surround_templates(&self, tab_id: u64) -> Vec<ffi::FfiTemplateItem> {
        let language = language_of(&self.session.borrow(), tab_id);
        let all = self.live_templates();
        EditorOpsRust::template_items(&live_templates::surround_candidates(&all, language))
    }

    pub fn surround_with(
        &self,
        tab_id: u64,
        text: &QString,
        abbreviation: &QString,
    ) -> ffi::FfiTemplateExpansion {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        if selection.is_multi() {
            return ffi::FfiTemplateExpansion::default();
        }
        let language = language_of(&self.session.borrow(), tab_id);
        let style = self.indent_style(language);
        live_templates::surround_with(
            &self.live_templates(),
            language,
            &text,
            selection.primary().range(),
            &abbreviation.to_string(),
            style,
        )
        .map_or_else(Default::default, |e| self.apply_expansion(tab_id, &text, e))
    }

    /// Templates as completion items: postfix ones after `expr.`, and the plain
    /// one that exactly matches the typed word.
    pub fn template_completions(&self, tab_id: u64, text: &QString) -> Vec<ffi::FfiCompletionItem> {
        let text = text.to_string();
        let Some(caret) = self.bare_caret(tab_id) else {
            return Vec::new();
        };
        let language = language_of(&self.session.borrow(), tab_id);
        let style = self.indent_style(language);
        let starts = line_starts(&text);
        let offers =
            live_templates::completions(&self.live_templates(), language, &text, caret, style);
        offers
            .into_iter()
            .map(|offer| {
                let (start_line, start_character) =
                    position_at(&text, &starts, offer.prepared.range.start);
                let (end_line, end_character) =
                    position_at(&text, &starts, offer.prepared.range.end);
                ffi::FfiCompletionItem {
                    label: QString::from(offer.abbreviation.as_str()),
                    kind: QString::from(lsp_core::kind_name(Some(SNIPPET_KIND))),
                    detail: QString::from(offer.description.as_str()),
                    documentation: QString::default(),
                    insert: QString::from(offer.prepared.source.as_str()),
                    has_range: true,
                    start_line,
                    start_character,
                    end_line,
                    end_character,
                    prefix_length: 0,
                    resolve_data: QString::default(),
                    deprecated: false,
                    is_snippet: true,
                    match_positions: Vec::new(),
                }
            })
            .collect()
    }
}
