//! Q6/Q7: the quick fixes behind an analyzer finding (PHPStan, Psalm,
//! PHPCS) — "Suppress <rule>", and "Fix with phpcbf" — merged into Alt+Enter and the hover card next to the
//! language servers' own code actions.
//!
//! Which comment, and where it goes, are `analysis_core::suppress`'s rules;
//! this file finds the findings at the caret and wraps each answer as a
//! synthesised quick fix whose `WorkspaceEdit` the ordinary `run_action`
//! path applies — the same shape as the build-file "Update to X" fix. The
//! fixer's edit cannot be known up front (it is a tool run), so that fix
//! carries its rule in `raw` and `apply_analyzer_fix` runs it on apply.

use std::path::Path;

use core::pin::Pin;

use crate::bridge::ffi;
use crate::bridge::format_tool::ToolFormat;

/// The `kind` of the "Fix with <tool>" quick fix; it still groups under
/// quick fixes because kinds are dotted (`lsp_core::IntentionGroup::of`).
pub(crate) const FIXER_KIND: &str = "quickfix.ide.fixer";

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
        let rows = self
            .store
            .borrow()
            .at_or_line_anchored(&uri, line, character);
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
            if let Some(fixer) = fixer_for(analyzer, &row.code) {
                let title = analysis_core::suppress::fix_title(fixer, &row.code);
                if out.iter().all(|i| i.title() != title) {
                    out.push(fixer_intention(title, fixer, &row.code, path));
                }
            }
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

    /// The "Fix with <tool>" branch of `applyIntention`: run the fixer over
    /// the buffer off the Qt thread and apply the difference as one undo
    /// step. `false` when `action` is not a fixer action.
    pub(crate) fn apply_analyzer_fix(
        self: Pin<&mut Self>,
        action: &lsp_core::CodeActionItem,
        buffer_revision: i64,
    ) -> bool {
        if action.kind.as_deref() != Some(FIXER_KIND) {
            return false;
        }
        let field = |name: &str| action.raw.get(name).and_then(|v| v.as_str()).unwrap_or("");
        let Some(tool) = ToolFormat::fixer(field("fixer"), field("code")) else {
            return true;
        };
        self.format_with_tool(
            tool,
            field("path").to_string(),
            buffer_revision,
            action.title.clone(),
            false,
        );
        true
    }
}

/// The fixer to offer for a finding of `analyzer` with rule `code`: the
/// analyzer names a `formatters` contribution, and that formatter must know
/// how to narrow itself to one rule.
fn fixer_for<'a>(analyzer: &'a plugin_api::AnalyzerContribution, code: &str) -> Option<&'a str> {
    let id = analyzer.fixer.as_deref()?;
    let has_fix_args = plugin_host::registry()
        .formatters()
        .any(|(_, f)| f.id == id && !f.fix_args.is_empty());
    (has_fix_args && !code.is_empty()).then_some(id)
}

fn fixer_intention(title: String, fixer: &str, code: &str, path: &str) -> lsp_core::Intention {
    lsp_core::Intention {
        item: lsp_core::CodeActionItem {
            title,
            kind: Some(FIXER_KIND.to_string()),
            edit: None,
            command: None,
            disabled: None,
            raw: serde_json::json!({ "fixer": fixer, "code": code, "path": path }),
        },
        group: lsp_core::IntentionGroup::QuickFix,
        preferred: false,
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
