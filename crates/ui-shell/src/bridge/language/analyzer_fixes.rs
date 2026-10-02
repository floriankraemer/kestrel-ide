//! Q6: the quick fixes behind an analyzer finding (PHPStan, Psalm, PHPCS) —
//! "Suppress <rule>" — merged into Alt+Enter and the hover card next to the
//! language servers' own code actions.
//!
//! Which comment, and where it goes, are `analysis_core::suppress`'s rules;
//! this file finds the findings at the caret and wraps each answer as a
//! synthesised quick fix whose `WorkspaceEdit` the ordinary `run_action`
//! path applies — the same shape as the build-file "Update to X" fix.

use std::path::Path;

use crate::bridge::ffi;

impl ffi::LanguageService {
    /// The analyzer quick fixes for the findings under `(line, character)`
    /// (0-based) in `path`; empty when none apply.
    pub(crate) fn analyzer_intentions(
        &self,
        path: &str,
        line: u32,
        character: u32,
    ) -> Vec<lsp_core::Intention> {
        let uri = lsp_core::uri_from_path(path);
        let rows = self.store.borrow().at(&uri, line, character);
        if rows.iter().all(|row| row.code.is_empty()) {
            return Vec::new();
        }
        let Some(content) = self.session.borrow().content_for_path(Path::new(path)) else {
            return Vec::new();
        };
        let analyzers: Vec<plugin_api::AnalyzerContribution> = plugin_host::registry()
            .analyzers()
            .map(|(_, analyzer)| analyzer.clone())
            .collect();

        let mut out: Vec<lsp_core::Intention> = Vec::new();
        for row in rows.iter().filter(|row| !row.code.is_empty()) {
            let Some(analyzer) = analyzers.iter().find(|a| a.name == row.source) else {
                continue;
            };
            let Some(template) = &analyzer.suppress_comment else {
                continue;
            };
            let insertion = analysis_core::suppress::suppress_insertion(
                template,
                &row.code,
                &content,
                row.line - 1,
            );
            let title = analysis_core::suppress::title(&analyzer.name, &row.code);
            if out.iter().all(|i| i.title() != title) {
                out.push(quick_fix(title, &uri, insertion));
            }
        }
        out
    }
}

/// A quick fix inserting `insertion` at the start of its line.
fn quick_fix(
    title: String,
    uri: &str,
    insertion: analysis_core::suppress::LineInsertion,
) -> lsp_core::Intention {
    let at = serde_json::json!({ "line": insertion.line, "character": 0 });
    let edit = serde_json::json!({
        "changes": { uri: [{
            "range": { "start": at, "end": at },
            "newText": insertion.new_text,
        }] }
    });
    lsp_core::Intention {
        item: lsp_core::CodeActionItem {
            title,
            kind: Some("quickfix".to_string()),
            edit: Some(edit),
            command: None,
            disabled: None,
            raw: serde_json::json!({}),
        },
        group: lsp_core::IntentionGroup::QuickFix,
        preferred: false,
    }
}
