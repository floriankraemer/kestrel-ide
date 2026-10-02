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
use crate::bridge::format_tool::ToolFormat;
use crate::bridge::language::{to_ffi_resource_op, to_file_op, PendingRefactor};

/// On-type formatting fires on every trigger keystroke, so it must not share
/// `edits`/`pending` with Rename and code actions: a typed `;` would re-arm
/// the Rename's gate against the moved buffer (applying its stale edits) or
/// replace its pending plan.
#[derive(Default)]
pub(crate) struct OnTypeSlot {
    gate: lsp_core::EditGate,
    plan: Option<lsp_core::EditPlan>,
}

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
    /// Reformat Code (F1-14, N4): the selection when there is one, the whole
    /// document otherwise, through the same pending-edit protocol a rename
    /// uses — `code.reformat` is confined to the file the user is looking
    /// at, so `touches_other_files` is always false and
    /// `RefactorController::onRefactorReady` applies it straight away: one
    /// Ctrl+Z undoes a reformat exactly as it undoes a rename.
    ///
    /// A configured tool formatter (`format_tool.rs`) formats the whole
    /// file when there is no selection; with a selection, or without a
    /// usable tool, the language server answers.
    pub fn request_formatting(
        self: Pin<&mut Self>,
        path: &QString,
        buffer_revision: i64,
        selection: ffi::FfiSelection,
    ) {
        let scope = lsp_core::formatting::selection_scope(
            (selection.start_line, selection.start_character),
            (selection.end_line, selection.end_character),
        );
        let path = path.to_string();
        let tool = self
            .open_docs
            .borrow()
            .get(&path)
            .and_then(|language_id| ToolFormat::resolve(language_id, scope.is_some()));
        if let Some(tool) = tool {
            return self.format_with_tool(
                tool,
                path,
                buffer_revision,
                "Reformat Code".to_string(),
                true,
            );
        }
        let path = QString::from(path.as_str());
        self.format_with(
            &path,
            buffer_revision,
            "Reformat Code",
            false,
            move |m, uri, o| match scope {
                Some((start, end)) => m.format_range(uri, start, end, o),
                None => m.format(uri, o),
            },
        );
    }

    /// Rewrite the whole file `path` with `tool` and apply the difference as
    /// one undo step. With `lsp_fallback` (Reformat Code) a tool that turns
    /// out not to be installed falls back to the language server, silently:
    /// the setting names a formatter the machine may not have.
    pub(crate) fn format_with_tool(
        mut self: Pin<&mut Self>,
        tool: ToolFormat,
        path: String,
        buffer_revision: i64,
        title: String,
        lsp_fallback: bool,
    ) {
        let Some(text) = self.session.borrow().content_for_path(Path::new(&path)) else {
            return;
        };
        self.edits.borrow_mut().begin(buffer_revision);
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result = tool.run(
                &text,
                Path::new(&path),
                crate::bridge::format_tool::FORMAT_TIMEOUT,
            );
            let _ = qt_thread.queue(move |service: Pin<&mut Self>| match result {
                Err(analysis_core::FormatError::NotInstalled) if lsp_fallback => {
                    service.format_whole_with_lsp(&path, buffer_revision)
                }
                Err(error) => {
                    service.finish_refactor(Err(format!("{} failed: {error}", tool.name())))
                }
                Ok(formatted) => {
                    let edits = lsp_core::edits_between(&text, &formatted);
                    if edits.is_empty() {
                        return service.finish_refactor(Ok(()));
                    }
                    let plan = lsp_core::EditPlan {
                        buffers: vec![lsp_core::DocumentEdits {
                            uri: lsp_core::uri_from_path(&path),
                            path,
                            version: None,
                            edits,
                        }],
                        files: Vec::new(),
                        ops: Vec::new(),
                        touches_other_files: false,
                    };
                    service.publish_refactor(title, plan, None);
                }
            });
        });
    }

    /// Reformat Code's language-server path for the whole file.
    pub(crate) fn format_whole_with_lsp(self: Pin<&mut Self>, path: &str, buffer_revision: i64) {
        self.format_with(
            &QString::from(path),
            buffer_revision,
            "Reformat Code",
            false,
            |m, uri, o| m.format(uri, o),
        );
    }

    /// Reformat Selection (N4): the selection only; with none, say so
    /// rather than reformatting the whole file.
    pub fn request_selection_formatting(
        mut self: Pin<&mut Self>,
        path: &QString,
        buffer_revision: i64,
        selection: ffi::FfiSelection,
    ) {
        let scope = lsp_core::formatting::selection_scope(
            (selection.start_line, selection.start_character),
            (selection.end_line, selection.end_character),
        );
        let Some((start, end)) = scope else {
            self.edits.borrow_mut().begin(buffer_revision);
            self.as_mut()
                .finish_refactor(Err("Select the text to reformat first.".to_string()));
            return;
        };
        self.format_with(
            path,
            buffer_revision,
            "Reformat Selection",
            false,
            move |m, uri, o| m.format_range(uri, start, end, o),
        );
    }

    /// N5: `typed` was just typed at `line`/`character` (the position after
    /// it). When a running server named it as an on-type formatting trigger
    /// the server's edits are applied like any reformat — silently: nothing
    /// to do, no formatter and failures are not worth interrupting typing.
    pub fn request_on_type_formatting(
        self: Pin<&mut Self>,
        path: &QString,
        buffer_revision: i64,
        line: u32,
        character: u32,
        typed: &QString,
    ) {
        let typed = typed.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path.to_string()).cloned() else {
            return;
        };
        let is_trigger = self
            .advertised
            .borrow()
            .get(&language_id)
            .is_some_and(|a| a.on_type_triggers().contains(&typed));
        if !is_trigger {
            return;
        }
        self.format_with(
            path,
            buffer_revision,
            "Format on Typing",
            true,
            move |m, uri, o| m.format_on_type(uri, (line, character), &typed, o),
        );
    }

    /// The shared tail of every reformat: options from the settings, one
    /// request on the LSP worker, the answer published as a one-file edit
    /// plan. `quiet` swallows "no formatter" and errors.
    fn format_with(
        mut self: Pin<&mut Self>,
        path: &QString,
        buffer_revision: i64,
        title: &'static str,
        quiet: bool,
        ask: impl FnOnce(
                &lsp_core::LspManager,
                &str,
                &lsp_core::formatting::FormattingOptions,
            )
                -> Result<lsp_core::formatting::FormattingOutcome, lsp_core::LspError>
            + Send
            + 'static,
    ) {
        use lsp_core::formatting::FormattingOutcome;
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let options = formatting_options(&language_id);
        let uri = lsp_core::uri_from_path(&path);
        // `quiet` is on-type formatting: its own gate and slot, so a trigger
        // keystroke can never disturb a Rename or code action in flight.
        if quiet {
            self.on_type.borrow_mut().gate.begin(buffer_revision);
        } else {
            self.edits.borrow_mut().begin(buffer_revision);
        }
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let outcome = ask(manager, &uri, &options);
            let version = manager.document_version(&uri);
            let _ = qt_thread.queue(move |service: Pin<&mut Self>| match outcome {
                Ok(FormattingOutcome::Edits(edits)) => {
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
                    if quiet {
                        service.publish_on_type(plan);
                    } else {
                        service.publish_refactor(title.to_string(), plan, None);
                    }
                }
                Ok(FormattingOutcome::AlreadyFormatted) if !quiet => {
                    service.finish_refactor(Ok(()))
                }
                Ok(FormattingOutcome::Unsupported) if !quiet => service
                    .finish_refactor(Err(format!("No formatter is available for {language_id}."))),
                Err(error) if !quiet => service.finish_refactor(Err(error.to_string())),
                // Quiet: nothing to apply, nothing to report, and the
                // pending refactoring (someone else's) stays untouched.
                Ok(_) | Err(_) => {}
            });
        });
    }

    /// Park N5's plan in its own slot and tell the view; never touches the
    /// pending refactoring.
    fn publish_on_type(mut self: Pin<&mut Self>, plan: lsp_core::EditPlan) {
        self.on_type.borrow_mut().plan = Some(plan);
        self.as_mut().on_type_format_ready();
    }

    /// The on-type edits, once, if the buffer is still at the revision they
    /// were computed against (`lsp_core::EditGate`'s rule).
    pub fn take_on_type_edits(&self, buffer_revision: i64) -> Vec<ffi::FfiTextEdit> {
        let mut slot = self.on_type.borrow_mut();
        let fresh = slot.gate.accept(buffer_revision);
        match slot.plan.take() {
            Some(plan) if fresh => to_ffi_edits(&plan, &[]),
            _ => Vec::new(),
        }
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
    /// Report a refactoring that produced nothing, answering anything that
    /// was waiting on it.
    pub(crate) fn finish_refactor(mut self: Pin<&mut Self>, outcome: Result<(), String>) {
        if let Some(pending) = self.pending.borrow_mut().take() {
            pending.settle(false, "the refactoring could not be applied");
        }
        if let Err(message) = outcome {
            self.as_mut()
                .refactor_failed(QString::from(message.as_str()));
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

/// The options a formatting request for `language_id` carries, from the
/// language's editing settings.
pub(crate) fn formatting_options(language_id: &str) -> lsp_core::formatting::FormattingOptions {
    let settings = crate::bridge::convert::load_resolved_settings();
    let rules = settings_model::editing::resolve_for_language(&settings, language_id);
    let style = rules.indent_style();
    lsp_core::formatting::FormattingOptions {
        tab_size: style.tab_width as u32,
        insert_spaces: style.use_spaces,
        trim_trailing_whitespace: Some(rules.trim_trailing_whitespace),
        insert_final_newline: Some(rules.insert_final_newline),
        trim_final_newlines: None,
    }
}
