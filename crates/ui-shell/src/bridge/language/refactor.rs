//! RF8: code actions, rename, formatting, and the pending-edit preview
//! (diff/hunks/spans, cancel/exclude, `takePendingEdits`) they all publish
//! through — split out of `mod.rs` once it crossed the file-size ceiling
//! (#162), the way `lsp_surface.rs` split out before it: a second
//! `impl ffi::LanguageService` block for the same QObject, reaching into
//! `LanguageServiceRust`'s fields and `mod.rs`'s `pub(crate)` helpers
//! (`run_action`, `finish_refactor`, `push_job`) rather than duplicating
//! them.

use core::pin::Pin;
use std::path::Path;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::convert::to_ffi_edits;
use crate::bridge::ffi::{self};
use crate::bridge::language::{to_ffi_resource_op, to_file_op, PendingRefactor};

impl ffi::LanguageService {
    pub fn code_actions_at(
        mut self: Pin<&mut Self>,
        path: &QString,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
        only: &QString,
    ) {
        let path = path.to_string();
        if !self.open_docs.borrow().contains_key(&path) {
            return;
        }
        let language_id = self
            .open_docs
            .borrow()
            .get(&path)
            .cloned()
            .unwrap_or_default();
        let uri = lsp_core::uri_from_path(&path);
        let only = only.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let filters: Vec<&str> = if only.is_empty() {
                Vec::new()
            } else {
                vec![&only]
            };
            let filtered = manager
                .code_action(
                    &uri,
                    (start_line, start_character),
                    (end_line, end_character),
                    &filters,
                )
                .unwrap_or_default();
            // An empty answer to a filtered request proves nothing: `only`
            // is a hint servers treat inconsistently, so ask again for
            // everything and let `lsp_core` classify what comes back.
            let actions = if !only.is_empty() && lsp_core::needs_unfiltered_retry(&filtered) {
                let all = manager
                    .code_action(
                        &uri,
                        (start_line, start_character),
                        (end_line, end_character),
                        &[],
                    )
                    .unwrap_or_default();
                lsp_core::filter_by_kind(&all, &only)
            } else {
                filtered
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                *service.actions.borrow_mut() = actions;
                *service.actions_language.borrow_mut() = language_id;
                service.as_mut().code_actions_ready();
            });
        });
    }
    pub fn code_actions(&self) -> Vec<ffi::FfiCodeAction> {
        self.actions
            .borrow()
            .iter()
            .map(|action| ffi::FfiCodeAction {
                title: QString::from(action.title.as_str()),
                kind: QString::from(action.kind.as_deref().unwrap_or_default()),
                disabled_reason: QString::from(action.disabled.as_deref().unwrap_or_default()),
            })
            .collect()
    }
    pub fn apply_code_action(mut self: Pin<&mut Self>, index: u32, buffer_revision: i64) {
        let Some(action) = self.actions.borrow().get(index as usize).cloned() else {
            return;
        };
        let language_id = self.actions_language.borrow().clone();
        self.as_mut()
            .run_action(action, language_id, buffer_revision);
    }
    pub fn prepare_rename(mut self: Pin<&mut Self>, path: &QString, line: u32, character: u32) {
        let path = path.to_string();
        if !self.open_docs.borrow().contains_key(&path) {
            return;
        }
        let uri = lsp_core::uri_from_path(&path);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let outcome =
                lsp_core::prepare_outcome(Some(manager.prepare_rename(&uri, line, character)));
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match outcome {
                // A server that cannot answer is not a server that said no,
                // so both of these let the rename go ahead.
                lsp_core::PrepareOutcome::Ready(prepared) => {
                    let placeholder = prepared.placeholder.unwrap_or_default();
                    service
                        .as_mut()
                        .rename_prepared(QString::from(placeholder.as_str()));
                }
                lsp_core::PrepareOutcome::Unknown => {
                    service.as_mut().rename_prepared(QString::default());
                }
                lsp_core::PrepareOutcome::Rejected => {
                    service
                        .as_mut()
                        .rename_rejected(QString::from("This element cannot be renamed."));
                }
            });
        });
    }
    pub fn rename_at(
        mut self: Pin<&mut Self>,
        path: &QString,
        line: u32,
        character: u32,
        new_name: &QString,
        buffer_revision: i64,
    ) {
        let path = path.to_string();
        let new_name = new_name.to_string();
        let open_paths = self.open_document_paths();
        self.edits.borrow_mut().begin(buffer_revision);

        if !self.open_docs.borrow().contains_key(&path) {
            // No server has this document, so there is nothing to ask —
            // which is a fallback, not a failure.
            self.as_mut().refactor_fallback();
            return;
        }
        let uri = lsp_core::uri_from_path(&path);
        let qt_thread = self.as_mut().qt_thread();
        let queued = self.push_job(move |manager| {
            let _session = manager.begin_refactor();
            let answer = manager.rename(&uri, line, character, &new_name);
            let outcome = lsp_core::rename_outcome(Some(answer));
            let title = format!("Rename to {new_name}");
            let planned = match outcome {
                lsp_core::RenameOutcome::Lsp(documents) => {
                    let versions: std::collections::HashMap<String, i32> = documents
                        .iter()
                        .filter_map(|doc| {
                            manager
                                .document_version(&doc.uri)
                                .map(|v| (doc.uri.clone(), v))
                        })
                        .collect();
                    Some(lsp_core::plan_edit(documents, &open_paths, &path, &|uri| {
                        versions.get(uri).copied()
                    }))
                }
                lsp_core::RenameOutcome::Index => None,
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match planned {
                Some(Ok(plan)) => service.publish_refactor(title, plan, None),
                Some(Err(e)) => service.finish_refactor(Err(e.to_string())),
                None => service.as_mut().refactor_fallback(),
            });
        });
        if !queued {
            self.as_mut().refactor_fallback();
        }
    }
    /// Reformat one open document (F1-14), through the same pending-edit
    /// protocol a rename uses: `code.reformat` is confined to the file the
    /// user is looking at, so `touches_other_files` is always false and
    /// `RefactorController::onRefactorReady` applies it straight away —
    /// one Ctrl+Z undoes a reformat exactly as it undoes a rename, with no
    /// new C++ needed for it.
    ///
    /// Whole-document only. `textDocument/rangeFormatting` over a selection
    /// is a real `lsp_core::LspManager::format_range` capability, left for
    /// whichever future task wires "Reformat Selection" to it — nothing
    /// here calls it yet.
    pub fn request_formatting(mut self: Pin<&mut Self>, path: &QString, buffer_revision: i64) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let settings = crate::bridge::convert::load_resolved_settings();
        let rules = settings_model::editing::resolve_for_language(&settings, &language_id);
        let style = rules.indent_style();
        let options = lsp_core::formatting::FormattingOptions {
            tab_size: style.tab_width as u32,
            insert_spaces: style.use_spaces,
            trim_trailing_whitespace: Some(rules.trim_trailing_whitespace),
            insert_final_newline: Some(rules.insert_final_newline),
            trim_final_newlines: None,
        };
        let uri = lsp_core::uri_from_path(&path);
        self.edits.borrow_mut().begin(buffer_revision);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let outcome = manager.format(&uri, &options);
            let version = manager.document_version(&uri);
            let _ = qt_thread.queue(move |service: Pin<&mut Self>| match outcome {
                Ok(lsp_core::formatting::FormattingOutcome::Edits(edits)) => {
                    let plan = lsp_core::EditPlan {
                        buffers: vec![lsp_core::DocumentEdits {
                            uri,
                            path,
                            version,
                            edits,
                        }],
                        files: Vec::new(),
                        ops: Vec::new(),
                        touches_other_files: false,
                    };
                    service.publish_refactor("Reformat Code".to_string(), plan, None);
                }
                Ok(lsp_core::formatting::FormattingOutcome::AlreadyFormatted) => {
                    service.finish_refactor(Ok(()));
                }
                Ok(lsp_core::formatting::FormattingOutcome::Unsupported) => {
                    service.finish_refactor(Err(format!(
                        "No formatter is available for {language_id}."
                    )));
                }
                Err(error) => service.finish_refactor(Err(error.to_string())),
            });
        });
    }
    pub fn pending_edits(&self) -> Vec<ffi::FfiTextEdit> {
        match self.pending.borrow().as_ref() {
            Some(pending) => to_ffi_edits(&pending.plan, &[]),
            None => Vec::new(),
        }
    }
    pub fn pending_ops(&self) -> Vec<ffi::FfiResourceOp> {
        match self.pending.borrow().as_ref() {
            Some(pending) => pending.plan.ops.iter().map(to_ffi_resource_op).collect(),
            None => Vec::new(),
        }
    }
    /// The pending plan's document for `path`, and the text it applies
    /// against — the live buffer if `path` is open, the file on disk
    /// otherwise. `None` when there is no pending refactoring or `path` is
    /// not one of its documents.
    fn pending_file_diff_source(&self, path: &str) -> Option<(lsp_core::DocumentEdits, String)> {
        let pending = self.pending.borrow();
        let doc = pending
            .as_ref()?
            .plan
            .buffers
            .iter()
            .chain(pending.as_ref()?.plan.files.iter())
            .find(|doc| doc.path == path)?
            .clone();
        let old_text = self
            .session
            .borrow()
            .content_for_path(Path::new(path))
            .or_else(|| std::fs::read_to_string(path).ok())?;
        Some((doc, old_text))
    }
    /// [`Self::pending_file_diff_source`], diffed — the shared computation
    /// behind `pendingFileDiff`/`pendingFileHunks`/`pendingFileSpans`.
    fn compute_pending_file_diff(&self, path: &str) -> Option<lsp_core::FileDiff> {
        let (doc, old_text) = self.pending_file_diff_source(path)?;
        lsp_core::file_diff(&old_text, &doc).ok()
    }
    pub fn pending_file_diff(&self, path: &QString) -> ffi::FfiFileDiff {
        let path = path.to_string();
        match self.compute_pending_file_diff(&path) {
            Some(diff) => ffi::FfiFileDiff {
                path: QString::from(path.as_str()),
                old_text: QString::from(diff.old_text.as_str()),
                new_text: QString::from(diff.new_text.as_str()),
            },
            None => ffi::FfiFileDiff::default(),
        }
    }
    pub fn pending_file_hunks(&self, path: &QString) -> Vec<ffi::FfiHunk> {
        match self.compute_pending_file_diff(&path.to_string()) {
            Some(diff) => crate::bridge::convert::to_ffi_hunks(&diff.hunks),
            None => Vec::new(),
        }
    }
    pub fn pending_file_spans(&self, path: &QString) -> Vec<ffi::FfiInlineSpan> {
        match self.compute_pending_file_diff(&path.to_string()) {
            Some(diff) => crate::bridge::convert::to_ffi_inline_spans(
                &diff.old_text,
                &diff.new_text,
                &diff.hunks,
                editor_core::diff::HighlightMode::Words,
            ),
            None => Vec::new(),
        }
    }
    pub fn exclude_from_refactor(self: Pin<&mut Self>, path: &QString) {
        if let Some(pending) = self.pending.borrow_mut().as_mut() {
            pending.excluded.push(path.to_string());
        }
    }
    pub fn take_pending_edits(
        mut self: Pin<&mut Self>,
        buffer_revision: i64,
    ) -> Vec<ffi::FfiTextEdit> {
        let fresh = self.edits.borrow_mut().accept(buffer_revision);
        let Some(pending) = self.pending.borrow_mut().take() else {
            return Vec::new();
        };
        if !fresh {
            // The buffer moved under the answer. Applying it would rewrite
            // the wrong bytes, so it is dropped — and a server waiting on it
            // is told so rather than left hanging.
            pending.settle(
                false,
                "the file changed while the refactoring was being prepared",
            );
            return Vec::new();
        }
        // ADR-0026: every resource operation is performed, all-or-nothing,
        // before any text edit is written. A failure here means the text
        // edits below never run at all.
        if !pending.plan.ops.is_empty() {
            let file_ops: Vec<app_core::FileOp> = pending.plan.ops.iter().map(to_file_op).collect();
            let outcome = self.session.borrow_mut().apply_file_ops(&file_ops);
            match outcome {
                Ok(retitled) => {
                    for tab in retitled {
                        self.as_mut()
                            .tab_title_changed(tab.id.raw(), QString::from(tab.title.as_str()));
                    }
                }
                Err(err) => {
                    pending.settle(false, "the refactoring could not be applied");
                    self.as_mut()
                        .refactor_failed(QString::from(err.to_string().as_str()));
                    return Vec::new();
                }
            }
        }
        let edits = to_ffi_edits(&pending.plan, &pending.excluded);
        pending.settle(
            !edits.is_empty() || !pending.plan.ops.is_empty(),
            "the refactoring was not applied",
        );
        edits
    }
    pub fn cancel_refactor(self: Pin<&mut Self>) {
        self.edits.borrow_mut().cancel();
        if let Some(pending) = self.pending.borrow_mut().take() {
            pending.settle(false, "the refactoring was cancelled");
        }
    }
    /// Publish a plan for the view to apply, replacing (and answering) any
    /// refactoring that was already waiting.
    pub(crate) fn publish_refactor(
        mut self: Pin<&mut Self>,
        title: String,
        plan: lsp_core::EditPlan,
        gate: Option<lsp_core::ApplyEditGate>,
    ) {
        let summary = ffi::FfiRefactorSummary {
            title: QString::from(title.as_str()),
            document_count: plan.document_count() as u32,
            edit_count: plan.edit_count() as u32,
            op_count: plan.ops.len() as u32,
            touches_other_files: plan.touches_other_files,
        };
        if let Some(previous) = self.pending.borrow_mut().replace(PendingRefactor {
            plan,
            excluded: Vec::new(),
            gate,
        }) {
            previous.settle(false, "a newer refactoring replaced this one");
        }
        self.as_mut().refactor_ready(summary);
    }
    /// The documents servers have open, which is what `lsp_core::plan_edit`
    /// splits a workspace edit against.
    pub(crate) fn open_document_paths(&self) -> Vec<String> {
        self.open_docs.borrow().keys().cloned().collect()
    }
    /// The file a code action was asked about, so an edit confined to it
    /// needs no preview. Taken from the action's own edit rather than
    /// remembered separately.
    ///
    /// ponytail: uses the untranslated `lsp_core::parse_workspace_edit`, so
    /// on a WSL project root this is a Linux path compared against a UNC
    /// one — the confinement check below always sees "touches other files"
    /// and shows a preview it didn't strictly need to. A correctness ceiling,
    /// not a correctness bug: `run_action`'s own `manager.parse_workspace_changes`
    /// (ADR-0052) is what actually applies the edit, already translated.
    /// Upgrade path if the extra preview ever annoys someone: give this
    /// struct the same `self.host` `apply_event` uses and retranslate here too.
    pub(crate) fn current_path_of(&self, action: &lsp_core::CodeActionItem) -> String {
        action
            .edit
            .as_ref()
            .and_then(|edit| lsp_core::parse_workspace_edit(edit).ok())
            .and_then(|docs| docs.first().map(|doc| doc.path.clone()))
            .unwrap_or_default()
    }
}
