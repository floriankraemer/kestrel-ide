//! F2-8/F2-9: the LSP surface beyond diagnostics, hover, completion and
//! refactoring — intentions, organize imports, signature help, document
//! highlights, inlay hints, and (C12-followup) go-to-definition plus the
//! decompiled-metadata fetch it can lead to.
//!
//! Split out of `mod.rs` once it crossed the file-size ceiling
//! (`scripts/check-file-size.sh`), the way `ai/agent.rs` splits out of
//! `ai/chat.rs`: a second `impl ffi::LanguageService` block for the same
//! QObject, reaching into `LanguageServiceRust`'s `pub(crate)` fields and
//! `mod.rs`'s `pub(crate)` helpers (`run_action`, `finish_refactor`,
//! `push_job`) rather than duplicating them.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::ffi::{self};

impl ffi::LanguageService {
    /// F2-8: everything that can be done at the caret — `code.showIntentions`
    /// (Alt+Enter). Scoped to the caret, not a selection, and merged with
    /// whatever diagnostic sits under it (`lsp_core::intentions::assemble`,
    /// via `LspManager::intentions`), which a plain `codeActionsAt` never
    /// asks for.
    pub fn request_intentions(mut self: Pin<&mut Self>, path: &QString, line: u32, character: u32) {
        let path = path.to_string();
        // C6: "Pull image" on a `FROM`/`image:` reference needs no server.
        if self.as_mut().container_intentions(&path, line, character) {
            return;
        }
        // D7: "Update to X" on a build-file dependency needs no server
        // either.
        if self.as_mut().build_file_intentions(&path, line, character) {
            return;
        }
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let uri = lsp_core::uri_from_path(&path);
        let diagnostics = self.store.borrow().diagnostics_at(&uri, line, character);
        let token = self.intentions_tracker.borrow_mut().begin();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let result =
                manager.intentions(&uri, (line, character), (line, character), &diagnostics);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if !service.intentions_tracker.borrow().accept(token) {
                    // A newer caret position superseded this request; its
                    // answer would show intentions for a place the caret
                    // has already left.
                    return;
                }
                *service.intentions.borrow_mut() = result.unwrap_or_default();
                *service.intentions_language.borrow_mut() = language_id;
                service.as_mut().intentions_ready();
            });
        });
    }

    /// The caret moved (or the tab did): whatever `requestIntentions` is
    /// still waiting on is no longer wanted.
    pub fn cancel_intentions(self: Pin<&mut Self>) {
        self.intentions_tracker.borrow_mut().cancel();
    }

    pub fn intentions(&self) -> Vec<ffi::FfiIntention> {
        self.intentions
            .borrow()
            .iter()
            .map(to_ffi_intention)
            .collect()
    }

    /// F2-8: the same gesture as `applyCodeAction`, over the caret-scoped
    /// list `requestIntentions` produced rather than the range-scoped one
    /// `codeActionsAt` did. Both end at `run_action` (`mod.rs`) because an
    /// `Intention` is a `CodeActionItem` plus a menu group — applying one is
    /// exactly applying the other.
    pub fn apply_intention(mut self: Pin<&mut Self>, index: u32, buffer_revision: i64) {
        let Some(intention) = self.intentions.borrow().get(index as usize).cloned() else {
            return;
        };
        if self.as_mut().apply_container_intention(&intention.item) {
            return;
        }
        let language_id = self.intentions_language.borrow().clone();
        self.as_mut()
            .run_action(intention.item, language_id, buffer_revision);
    }

    /// The rest of F2-8's LSP surface: organize imports for a whole
    /// document. `manager.organize_imports` already applies the
    /// `needs_unfiltered_retry` taxonomy (F2-7); here it is just another
    /// action into `run_action`.
    pub fn organize_imports(
        mut self: Pin<&mut Self>,
        path: &QString,
        last_line: u32,
        buffer_revision: i64,
    ) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let uri = lsp_core::uri_from_path(&path);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let result = manager.organize_imports(&uri, last_line);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| match result {
                Ok(Some(action)) => {
                    service
                        .as_mut()
                        .run_action(action, language_id, buffer_revision);
                }
                Ok(None) => service.finish_refactor(Err(
                    "There is nothing to organize in this file's imports.".to_string(),
                )),
                Err(e) => service.finish_refactor(Err(e.to_string())),
            });
        });
    }

    /// F2-9 — signature help for the call the caret sits in. The decision
    /// of whether to ask at all — a trigger character, an explicit request,
    /// or the caret having left every call — is `lsp_core::signature_help`'s
    /// (`should_request`/`should_dismiss`), not something reimplemented
    /// here or in `cpp/`.
    pub fn request_signature_help(
        mut self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        byte_offset: u64,
        explicit_request: bool,
        showing: bool,
    ) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let text = text.to_string();
        let byte_offset = (byte_offset as usize).min(text.len());
        if lsp_core::signature_help_should_dismiss(&text, byte_offset) {
            self.signature_tracker.borrow_mut().cancel();
            self.signature_help.borrow_mut().take();
            self.as_mut().signature_help_ready();
            return;
        }
        let triggers = self
            .signature_triggers
            .borrow()
            .get(&language_id)
            .cloned()
            .unwrap_or_default();
        if !lsp_core::should_request_signature_help(
            &triggers,
            &text,
            byte_offset,
            explicit_request,
            showing,
        ) {
            return;
        }
        let (line, character) = to_lsp_position(&text, byte_offset);
        let uri = lsp_core::uri_from_path(&path);
        let token = self.signature_tracker.borrow_mut().begin();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let result = manager.signature_help(&uri, line, character);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if !service.signature_tracker.borrow().accept(token) {
                    // A newer caret position superseded this request.
                    return;
                }
                *service.signature_help.borrow_mut() = result.ok().flatten();
                // R3: a fresh answer is a fresh call (or the same call
                // re-asked), never the overload Up/Down last cycled to.
                service.signature_display_index.set(None);
                service.as_mut().signature_help_ready();
            });
        });
    }

    pub fn signature_help(&self) -> ffi::FfiSignatureHelp {
        match self.signature_help.borrow().as_ref() {
            Some(help) => to_ffi_signature_help(help, self.signature_display_index.get()),
            None => ffi::FfiSignatureHelp::default(),
        }
    }

    /// R3: Up (`delta = -1`) / Down (`delta = 1`) on the signature tip —
    /// steps to another overload of the same call, wrapping past either
    /// end (`lsp_core::SignatureHelp::cycle_signature`). A no-op with
    /// nothing showing, which is what a keystroke racing the tip's own
    /// dismissal produces.
    pub fn cycle_signature_overload(self: Pin<&mut Self>, delta: i32) {
        let help = self.signature_help.borrow();
        let Some(help) = help.as_ref() else {
            return;
        };
        let current = self.signature_display_index.get().unwrap_or_else(|| {
            help.active_signature
                .unwrap_or(0)
                .min(help.signatures.len().saturating_sub(1))
        });
        let next = help.cycle_signature(current, delta);
        self.signature_display_index.set(Some(next));
    }

    /// F2-9 — every occurrence of the symbol under the caret, for
    /// occurrence painting.
    pub fn request_document_highlights(
        mut self: Pin<&mut Self>,
        path: &QString,
        line: u32,
        character: u32,
    ) {
        let path = path.to_string();
        if !self.open_docs.borrow().contains_key(&path) {
            return;
        }
        let uri = lsp_core::uri_from_path(&path);
        let token = self.highlights_tracker.borrow_mut().begin();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let result = manager.document_highlights(&uri, line, character);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if !service.highlights_tracker.borrow().accept(token) {
                    return;
                }
                *service.highlights.borrow_mut() = result.unwrap_or_default();
                service.as_mut().document_highlights_ready();
            });
        });
    }

    pub fn document_highlights(&self) -> Vec<ffi::FfiDocumentHighlight> {
        self.highlights
            .borrow()
            .iter()
            .map(to_ffi_document_highlight)
            .collect()
    }

    /// F2-9 — inlay hints for the visible lines. Every caller of this is
    /// expected to re-ask on scroll; there is deliberately no whole-document
    /// form (`lsp_core::inlay_hint`'s own doc comment).
    pub fn request_inlay_hints(
        mut self: Pin<&mut Self>,
        path: &QString,
        first_line: u32,
        last_line: u32,
    ) {
        let path = path.to_string();
        if !self.open_docs.borrow().contains_key(&path) {
            return;
        }
        let uri = lsp_core::uri_from_path(&path);
        let token = self.inlay_hints_tracker.borrow_mut().begin();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let result = manager.inlay_hints(&uri, first_line, last_line);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if !service.inlay_hints_tracker.borrow().accept(token) {
                    return;
                }
                *service.inlay_hints.borrow_mut() = result.unwrap_or_default();
                service.as_mut().inlay_hints_ready();
            });
        });
    }

    pub fn inlay_hints(&self) -> Vec<ffi::FfiInlayHint> {
        self.inlay_hints
            .borrow()
            .iter()
            .map(to_ffi_inlay_hint)
            .collect()
    }

    /// C9 — fire-and-forget `textDocument/semanticTokens/full` for `path`'s
    /// whole document. Gated on `LspManager::semantic_tokens_legend`
    /// *inside* the worker job rather than here, so a server that only
    /// registers the capability dynamically (`client/registerCapability`,
    /// after `ServerReady` already fired) is still reachable on the next
    /// call — there is no snapshot of "supported" taken at `ServerReady`
    /// time to go stale.
    ///
    /// Every early return in the job — no legend yet, the request errored,
    /// the server answered with nothing to say — leaves whatever spans are
    /// already stored for `path` untouched rather than clearing them: F0-16
    /// again, a server still starting or still indexing must never turn
    /// already-coloured text blank.
    pub fn request_semantic_tokens(mut self: Pin<&mut Self>, path: &QString, text: &QString) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let uri = lsp_core::uri_from_path(&path);
        let text = text.to_string();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let Some(legend) = manager.semantic_tokens_legend(&language_id) else {
                return;
            };
            let Ok(result) = manager.semantic_tokens(&language_id, &uri) else {
                return;
            };
            let Some((_result_id, tokens)) = lsp_core::parse_semantic_tokens_full(&result) else {
                return;
            };
            let spans = mapped_semantic_spans(&legend, &tokens, &text);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                service
                    .semantic_tokens
                    .borrow_mut()
                    .insert(path.clone(), spans);
                service
                    .as_mut()
                    .semantic_tokens_ready(QString::from(path.as_str()));
            });
        });
    }

    /// The last decoded-and-mapped semantic-token spans for `path`, in
    /// `FfiHighlightSpan`'s byte-offset/scope-id shape —
    /// `SyntaxHighlighterHandle::overlay_semantic_tokens`'s `semantic`
    /// argument. Empty before the first answer.
    pub fn semantic_token_spans(&self, path: &QString) -> Vec<ffi::FfiHighlightSpan> {
        self.semantic_tokens
            .borrow()
            .get(&path.to_string())
            .map(|spans| {
                spans
                    .iter()
                    .map(|s| ffi::FfiHighlightSpan {
                        start: s.start,
                        end: s.end,
                        scope: s.scope.id(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// C10 — fire-and-forget `textDocument/codeLens` for `path`'s whole
    /// document. Gated on `LspManager::code_lenses_supported` *inside* the
    /// worker job rather than here, same reasoning
    /// `request_semantic_tokens` gives: a server that only registers the
    /// capability dynamically is still reachable on the next call, with no
    /// snapshot of "supported" taken up front to go stale.
    ///
    /// Called once on document open (`document_opened`), the same call site
    /// `request_semantic_tokens` uses and for the same reason — a whole-
    /// document request is not something to re-fire per keystroke, and
    /// there is no existing "expensive request after typing settles"
    /// debounce in this file to reuse for a second trigger on every edit.
    pub fn request_code_lenses(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let uri = lsp_core::uri_from_path(&path);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            if !manager.code_lenses_supported(&language_id) {
                return;
            }
            let Ok(lenses) = manager.code_lenses(&language_id, &uri) else {
                return;
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                service
                    .code_lenses
                    .borrow_mut()
                    .insert(path.clone(), lenses);
                service
                    .as_mut()
                    .code_lenses_ready(QString::from(path.as_str()));
            });
        });
    }

    /// The last-fetched lenses for `path`, reduced to what the view paints:
    /// a line, a label, and whether a click does anything yet. A lens that
    /// still needs `codeLens/resolve` (`command` is `None`) shows its
    /// placeholder rather than nothing — the range is real even before the
    /// label is — and is not clickable until resolved.
    pub fn code_lenses(&self, path: &QString) -> Vec<ffi::FfiCodeLens> {
        self.code_lenses
            .borrow()
            .get(&path.to_string())
            .map(|lenses| lenses.iter().map(to_ffi_code_lens).collect())
            .unwrap_or_default()
    }

    /// C10 — run one lens's command by index into the last answer
    /// `codeLenses` returned for `path`. Resolves first if the lens still
    /// needs it, then sends the command through the *existing*
    /// `workspace/executeCommand` path (`LspManager::execute_command`,
    /// already used by `run_action`'s code-action `Execute` step) — there is
    /// exactly one command-execution method in this client, not one per
    /// feature that can name a command.
    ///
    /// Wrapped in the same session guard `run_action` holds across its own
    /// `execute_command` call: a lens click is exactly the user gesture
    /// `apply_edit::RefactorSessions` exists to recognise, so a command that
    /// turns around and asks for `workspace/applyEdit` is answered rather
    /// than refused as unsolicited. Any resulting edit arrives as its own
    /// `LspEvent::ApplyEdit` and is published from there, same as a code
    /// action's `Execute` step.
    pub fn run_code_lens(self: Pin<&mut Self>, path: &QString, index: u32) {
        let path = path.to_string();
        let Some(item) = self
            .code_lenses
            .borrow()
            .get(&path)
            .and_then(|lenses| lenses.get(index as usize))
            .cloned()
        else {
            return;
        };
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        self.push_job(move |manager| {
            let _session = manager.begin_refactor();
            let resolved = if item.needs_resolve() {
                manager
                    .resolve_code_lens(&language_id, &item.raw)
                    .ok()
                    .and_then(|resolved| {
                        lsp_core::parse_code_lenses(&serde_json::json!([resolved]))
                            .into_iter()
                            .next()
                    })
                    .unwrap_or(item)
            } else {
                item
            };
            if let Some(command) = resolved.command {
                let _ = manager.execute_command(&language_id, &command);
            }
        });
    }

    // -- C11: call hierarchy ------------------------------------------------

    /// `textDocument/prepareCallHierarchy` at a caret position — the
    /// starting point `requestIncomingCalls`/`requestOutgoingCalls` index
    /// into by position. Gated on `LspManager::call_hierarchy_supported`
    /// inside the job, same reasoning `request_code_lenses` gives. No index
    /// fallback exists for call hierarchy at all (`lsp_core::hierarchy`
    /// module docs) — an unsupported server or no answer simply leaves the
    /// list empty.
    pub fn request_call_hierarchy(
        mut self: Pin<&mut Self>,
        path: &QString,
        line: u32,
        character: u32,
    ) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let uri = lsp_core::uri_from_path(&path);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            if !manager.call_hierarchy_supported(&language_id) {
                return;
            }
            let Ok(items) = manager.prepare_call_hierarchy(&language_id, &uri, line, character)
            else {
                return;
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                *service.call_hierarchy_language.borrow_mut() = language_id;
                *service.call_hierarchy_items.borrow_mut() = items;
                service.as_mut().call_hierarchy_ready();
            });
        });
    }

    pub fn call_hierarchy_items(&self) -> Vec<ffi::FfiHierarchyItem> {
        self.call_hierarchy_items
            .borrow()
            .iter()
            .map(to_ffi_hierarchy_item)
            .collect()
    }

    /// `callHierarchy/incomingCalls` for the item at `index` in the last
    /// `callHierarchyItems` answer.
    pub fn request_incoming_calls(mut self: Pin<&mut Self>, index: u32) {
        let Some(item) = self
            .call_hierarchy_items
            .borrow()
            .get(index as usize)
            .cloned()
        else {
            return;
        };
        let language_id = self.call_hierarchy_language.borrow().clone();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let Ok(calls) = manager.incoming_calls(&language_id, &item.raw) else {
                return;
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                *service.incoming_calls.borrow_mut() = calls;
                service.as_mut().incoming_calls_ready();
            });
        });
    }

    pub fn incoming_calls(&self) -> Vec<ffi::FfiIncomingCall> {
        self.incoming_calls
            .borrow()
            .iter()
            .map(|call| ffi::FfiIncomingCall {
                from: to_ffi_hierarchy_item(&call.from),
                call_count: call.from_ranges.len() as u32,
            })
            .collect()
    }

    /// `callHierarchy/outgoingCalls` for the item at `index` in the last
    /// `callHierarchyItems` answer. An empty answer is the real, final
    /// answer for a leaf that calls nothing — not a cue to look anywhere
    /// else (`lsp_core::hierarchy` module docs).
    pub fn request_outgoing_calls(mut self: Pin<&mut Self>, index: u32) {
        let Some(item) = self
            .call_hierarchy_items
            .borrow()
            .get(index as usize)
            .cloned()
        else {
            return;
        };
        let language_id = self.call_hierarchy_language.borrow().clone();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let Ok(calls) = manager.outgoing_calls(&language_id, &item.raw) else {
                return;
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                *service.outgoing_calls.borrow_mut() = calls;
                service.as_mut().outgoing_calls_ready();
            });
        });
    }

    pub fn outgoing_calls(&self) -> Vec<ffi::FfiOutgoingCall> {
        self.outgoing_calls
            .borrow()
            .iter()
            .map(|call| ffi::FfiOutgoingCall {
                to: to_ffi_hierarchy_item(&call.to),
                call_count: call.from_ranges.len() as u32,
            })
            .collect()
    }

    // -- C11: type hierarchy -------------------------------------------------

    /// `textDocument/prepareTypeHierarchy` at a caret position. Like call
    /// hierarchy's own `requestCallHierarchy`, this step is LSP-only — the
    /// index has no "what type is under this caret" query of its own to
    /// fall back to; the fallback exists one step later, for
    /// `requestSupertypes`/`requestSubtypes`, once a type *name* is known.
    pub fn request_type_hierarchy(
        mut self: Pin<&mut Self>,
        path: &QString,
        line: u32,
        character: u32,
    ) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            return;
        };
        let uri = lsp_core::uri_from_path(&path);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            if !manager.type_hierarchy_supported(&language_id) {
                return;
            }
            let Ok(items) = manager.prepare_type_hierarchy(&language_id, &uri, line, character)
            else {
                return;
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                *service.type_hierarchy_language.borrow_mut() = language_id;
                *service.type_hierarchy_items.borrow_mut() = items;
                service.as_mut().type_hierarchy_ready();
            });
        });
    }

    pub fn type_hierarchy_items(&self) -> Vec<ffi::FfiHierarchyItem> {
        self.type_hierarchy_items
            .borrow()
            .iter()
            .map(to_ffi_hierarchy_item)
            .collect()
    }

    /// `typeHierarchy/supertypes` for the item at `index` in the last
    /// `typeHierarchyItems` answer — LSP-first, `index-core`'s
    /// supertype-edge data (`inh_type`/`inh_supertype`, the same data
    /// `SearchModel::find_supertypes` already reads) as the fallback,
    /// exactly the precedence `lsp_core::hierarchy::type_hierarchy_outcome`
    /// gives go-to-definition's own `definition_outcome` (ADR-0016).
    pub fn request_supertypes(self: Pin<&mut Self>, index: u32) {
        self.request_type_edge(index, true);
    }

    /// `typeHierarchy/subtypes`, the other direction of the same walk.
    pub fn request_subtypes(self: Pin<&mut Self>, index: u32) {
        self.request_type_edge(index, false);
    }

    fn request_type_edge(mut self: Pin<&mut Self>, index: u32, supertypes: bool) {
        let Some(item) = self
            .type_hierarchy_items
            .borrow()
            .get(index as usize)
            .cloned()
        else {
            return;
        };
        let language_id = self.type_hierarchy_language.borrow().clone();
        let index_handle = std::sync::Arc::clone(&self.index);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let supported = manager.type_hierarchy_supported(&language_id);
            let response = if supported {
                Some(if supertypes {
                    manager.supertypes(&language_id, &item.raw)
                } else {
                    manager.subtypes(&language_id, &item.raw)
                })
            } else {
                None
            };
            let name = item.name.clone();
            let fallback = index_fallback(&index_handle, |text_index| {
                if supertypes {
                    text_index.find_supertypes(&name)
                } else {
                    text_index.find_implementations(&name)
                }
            });
            let outcome = lsp_core::type_hierarchy_outcome(response, fallback);
            let items = match outcome {
                lsp_core::TypeHierarchyOutcome::Lsp(items) => items,
                lsp_core::TypeHierarchyOutcome::Index(items) => items,
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if supertypes {
                    *service.supertypes.borrow_mut() = items;
                    service.as_mut().supertypes_ready();
                } else {
                    *service.subtypes.borrow_mut() = items;
                    service.as_mut().subtypes_ready();
                }
            });
        });
    }

    pub fn supertypes(&self) -> Vec<ffi::FfiHierarchyItem> {
        self.supertypes
            .borrow()
            .iter()
            .map(to_ffi_hierarchy_item)
            .collect()
    }

    pub fn subtypes(&self) -> Vec<ffi::FfiHierarchyItem> {
        self.subtypes
            .borrow()
            .iter()
            .map(to_ffi_hierarchy_item)
            .collect()
    }

    pub fn resolve_definition(mut self: Pin<&mut Self>, path: &QString, line: u32, character: u32) {
        let path_string = path.to_string();
        let uri = lsp_core::uri_from_path(&path_string);
        // C12-followup: the language the *originating* document is in —
        // `fetch_metadata` needs it to pick the same server that answered
        // `manager.definition`, and it is not derivable from a metadata
        // target's own `csharp:/...` URI the way `manager.definition`
        // derives a normal target's language from an open document's path.
        let language_id = self.config_for_path(&path_string).map(|c| c.language_id);
        let qt_thread = self.as_mut().qt_thread();
        let queued = self.push_job(move |manager| {
            let outcome =
                lsp_core::definition_outcome(Some(manager.definition(&uri, line, character)));
            let _ = qt_thread.queue(move |service: Pin<&mut Self>| {
                service.apply_definition_outcome(outcome, language_id)
            });
        });
        if !queued {
            // No worker at all (no project open), which is one more case of
            // "no server answered" — the same rule decides it.
            self.apply_definition_outcome(lsp_core::definition_outcome(None), None);
        }
    }

    /// Turn the outcome into signals. The branch is which signal, never
    /// which source: `definition_outcome` already chose that. `language_id`
    /// is only used by the `NeedsMetadataFetch` arm.
    fn apply_definition_outcome(
        mut self: Pin<&mut Self>,
        outcome: lsp_core::DefinitionOutcome,
        language_id: Option<String>,
    ) {
        match outcome {
            lsp_core::DefinitionOutcome::Lsp(targets) => {
                for target in targets {
                    self.as_mut().definition_found(ffi::FfiDefinition {
                        path: QString::from(target.path.as_str()),
                        line: target.line,
                        column: target.column,
                    });
                }
                self.as_mut().definition_finished();
            }
            lsp_core::DefinitionOutcome::Index => self.as_mut().definition_fallback(),
            // C12: the server pointed at decompiled/generated source (a
            // non-`file:` URI). With the language known, fetch and open it
            // as a read-only virtual document (`apply_metadata_fetch`);
            // with no language (no server ever answered `manager.definition`
            // for a path with no configured server, which cannot actually
            // happen alongside a real `Lsp`/`NeedsMetadataFetch` answer, but
            // a typed `Option` beats assuming it away) refuse cleanly rather
            // than treating the raw URI as a path.
            lsp_core::DefinitionOutcome::NeedsMetadataFetch(uri) => match language_id {
                Some(language_id) => self.fetch_and_open_metadata(language_id, uri),
                None => self.as_mut().definition_unavailable(QString::from(
                    "Go to Definition landed in decompiled framework code, \
                     which this IDE cannot open yet.",
                )),
            },
        }
    }

    /// C12-followup: `csharp/metadata` for `uri`, on the worker thread like
    /// every other request, and apply whatever it answers with back on the
    /// Qt thread.
    fn fetch_and_open_metadata(mut self: Pin<&mut Self>, language_id: String, uri: String) {
        let qt_thread = self.as_mut().qt_thread();
        let queued = self.push_job(move |manager| {
            let result = manager.fetch_metadata(&language_id, &uri);
            let _ = qt_thread
                .queue(move |service: Pin<&mut Self>| service.apply_metadata_fetch(uri, result));
        });
        if !queued {
            self.as_mut().definition_unavailable(QString::from(
                "Go to Definition landed in decompiled framework code, \
                 which this IDE cannot open yet.",
            ));
        }
    }

    /// C12-followup: `csharp/metadata`'s answer — open it as a read-only
    /// virtual document (`app_core::AppSession::open_virtual_document`,
    /// which itself dedups a re-navigation onto the tab it already opened)
    /// and tell `EditorTabs` via `virtualDocumentOpened`, the same
    /// build-the-widget-then-focus split `DocumentManager::tabOpened` and
    /// `EditorTabs::focusTab` already follow for a normal file. A fetch
    /// failure — no `csharp/metadata` support, a timeout, a malformed
    /// response — reuses `definitionUnavailable` rather than a signal of
    /// its own, so the refusal message stays in one place.
    fn apply_metadata_fetch(
        mut self: Pin<&mut Self>,
        uri: String,
        result: Result<String, lsp_core::LspError>,
    ) {
        let text = match result {
            Ok(text) => text,
            Err(err) => {
                self.as_mut().definition_unavailable(QString::from(
                    format!("Fetching decompiled source failed: {err}").as_str(),
                ));
                return;
            }
        };
        let Some((scheme, key)) = lsp_core::virtual_doc_key(&uri) else {
            self.as_mut().definition_unavailable(QString::from(
                "Go to Definition landed in decompiled framework code, \
                 which this IDE cannot open yet.",
            ));
            return;
        };
        let opened = self
            .session
            .borrow_mut()
            .open_virtual_document(&scheme, &key, &text);
        self.as_mut().virtual_document_opened(
            opened.id.raw(),
            QString::from(opened.title.as_str()),
            opened.newly_opened,
        );
    }
}

fn to_ffi_hierarchy_item(item: &lsp_core::HierarchyItem) -> ffi::FfiHierarchyItem {
    ffi::FfiHierarchyItem {
        name: QString::from(item.name.as_str()),
        detail: QString::from(item.detail.as_deref().unwrap_or("")),
        path: QString::from(
            lsp_core::path_from_uri(&item.uri)
                .unwrap_or_else(|| item.uri.clone())
                .as_str(),
        ),
        kind: item.kind,
        line: item.selection_range.start_line + 1,
        column: item.selection_range.start_character,
    }
}

/// The index-derived fallback for `typeHierarchy/supertypes`/`subtypes`:
/// `SymbolMatch`, from `index-core`'s `inh_type`/`inh_supertype` edges,
/// converted to the same `HierarchyItem` shape an LSP answer parses into —
/// `lsp_core::hierarchy::type_hierarchy_outcome` takes its fallback already
/// converted rather than depending on `index-core` itself
/// (`docs/architecture/layering.md`), and this is the one place that join is
/// made (see `crate::bridge::language::mod` module docs on `index`).
fn index_fallback(
    index: &mcp_server::IndexHandle,
    query: impl FnOnce(
        &index_core::TextIndex,
    ) -> Result<Vec<index_core::SymbolMatch>, index_core::IndexError>,
) -> Vec<lsp_core::HierarchyItem> {
    let guard = index.read().unwrap();
    let Some(text_index) = guard.ready() else {
        return Vec::new();
    };
    query(text_index)
        .unwrap_or_default()
        .iter()
        .map(symbol_match_to_hierarchy_item)
        .collect()
}

fn symbol_match_to_hierarchy_item(m: &index_core::SymbolMatch) -> lsp_core::HierarchyItem {
    let uri = lsp_core::uri_from_path(&m.path.to_string_lossy());
    let line = m.line.saturating_sub(1) as u32;
    let character = m.col as u32;
    let range = lsp_core::completion::TextRange {
        start_line: line,
        start_character: character,
        end_line: line,
        end_character: character,
    };
    lsp_core::HierarchyItem {
        name: m.name.clone(),
        kind: lsp_symbol_kind(m.kind),
        detail: m.container.clone(),
        uri: uri.clone(),
        range,
        selection_range: range,
        data: None,
        raw: serde_json::json!({"name": m.name, "uri": uri}),
    }
}

/// `SymbolKind`, as `index-core` knows it (`syntax_core::SymbolKind`),
/// mapped onto the LSP protocol's own numbering — the same numbers
/// [`lsp_core::HierarchyItem::kind`] carries for an LSP-sourced item, so the
/// view reads one vocabulary regardless of which side answered. Numbers are
/// the LSP 3.17 `SymbolKind` enum's own values; `None` (an occurrence
/// `tags.scm` never classified) is shown as a class, the common case for
/// something a type hierarchy names at all.
fn lsp_symbol_kind(kind: Option<syntax_core::SymbolKind>) -> u32 {
    match kind {
        Some(syntax_core::SymbolKind::Struct) => 23,
        Some(syntax_core::SymbolKind::Enum) => 10,
        Some(syntax_core::SymbolKind::Interface) => 11,
        Some(syntax_core::SymbolKind::Method) => 6,
        Some(syntax_core::SymbolKind::Function) => 12,
        Some(syntax_core::SymbolKind::Field) => 8,
        Some(syntax_core::SymbolKind::Property) => 7,
        Some(syntax_core::SymbolKind::Constructor) => 9,
        Some(syntax_core::SymbolKind::Constant) => 14,
        Some(syntax_core::SymbolKind::EnumMember) => 22,
        Some(syntax_core::SymbolKind::Class) | None => 5,
    }
}

fn to_ffi_code_lens(lens: &lsp_core::CodeLensItem) -> ffi::FfiCodeLens {
    ffi::FfiCodeLens {
        line: lens.range.start_line,
        label: QString::from(
            lens.command
                .as_ref()
                .map(|c| c.title.as_str())
                .unwrap_or("\u{2026}"),
        ),
        clickable: lens.command.is_some() || lens.raw.get("data").is_some(),
    }
}

/// Decodes each token to its byte-range/scope shape: `token.line`/
/// `start_char` are UTF-16 (the protocol's own encoding), converted to a
/// byte offset via `editor_core::offsets` — the same conversion every other
/// LSP-position-carrying value in this crate reuses, not a second one. A
/// token whose type this build's taxonomy cannot place at all
/// (`semantic_token_scope` returning `None`) is dropped, same as an
/// unrecognized tree-sitter capture.
fn mapped_semantic_spans(
    legend: &lsp_core::SemanticTokensLegend,
    tokens: &[lsp_core::SemanticToken],
    text: &str,
) -> Vec<lsp_core::MappedSemanticSpan> {
    let utf16_starts = editor_core::offsets::utf16_line_starts(text);
    tokens
        .iter()
        .filter_map(|token| {
            let scope = lsp_core::semantic_token_scope(legend, token)?;
            let line_start = *utf16_starts.get(token.line as usize)?;
            let utf16_start = line_start + token.start_char as usize;
            let utf16_end = utf16_start + token.length as usize;
            Some(lsp_core::MappedSemanticSpan {
                start: editor_core::offsets::byte_offset(text, utf16_start),
                end: editor_core::offsets::byte_offset(text, utf16_end),
                scope,
            })
        })
        .collect()
}

fn to_ffi_intention(intention: &lsp_core::Intention) -> ffi::FfiIntention {
    ffi::FfiIntention {
        title: QString::from(intention.title()),
        kind: QString::from(intention.kind().unwrap_or_default()),
        group: match intention.group {
            lsp_core::IntentionGroup::QuickFix => ffi::FfiIntentionGroup::QuickFix,
            lsp_core::IntentionGroup::Refactor => ffi::FfiIntentionGroup::Refactor,
            lsp_core::IntentionGroup::Source => ffi::FfiIntentionGroup::Source,
            lsp_core::IntentionGroup::Other => ffi::FfiIntentionGroup::Other,
        },
        preferred: intention.preferred,
        disabled_reason: QString::from(intention.disabled().unwrap_or_default()),
    }
}

/// A byte offset into the live buffer as an LSP position (0-based line,
/// UTF-16 character) — needed only where the caller has a byte offset and
/// not, as everywhere else in this file, a position the view already
/// computed from its own `QTextCursor` (ADR-0023's `editor_core::offsets`).
fn to_lsp_position(text: &str, byte_offset: usize) -> (u32, u32) {
    let starts = editor_core::offsets::line_starts(text);
    let line = editor_core::offsets::line_of(&starts, byte_offset);
    let line_start = starts[line];
    let rest = &text[line_start..];
    let line_text = rest.split('\n').next().unwrap_or(rest);
    let relative = (byte_offset - line_start).min(line_text.len());
    let character = editor_core::offsets::utf16_offset(line_text, relative);
    (line as u32, character as u32)
}

/// R3: `override_index` is the overload Up/Down last cycled to
/// (`LanguageServiceRust::signature_display_index`); `None` uses the
/// server's own `activeSignature`, exactly what `resolved_signature` picked
/// before cycling existed.
fn to_ffi_signature_help(
    help: &lsp_core::SignatureHelp,
    override_index: Option<usize>,
) -> ffi::FfiSignatureHelp {
    let count = help.signatures.len();
    if count == 0 {
        return ffi::FfiSignatureHelp::default();
    }
    let index = override_index
        .unwrap_or_else(|| help.active_signature.unwrap_or(0))
        .min(count - 1);
    let Some((signature, parameter_index)) = help.signature_and_parameter_at(index) else {
        return ffi::FfiSignatureHelp::default();
    };
    let parameter = parameter_index.and_then(|i| signature.parameters.get(i));
    let (has_active_parameter, parameter_start, parameter_end) =
        match parameter.and_then(|p| p.range) {
            Some((start, end)) => (true, start, end),
            None => (false, 0, 0),
        };
    // R3: the active parameter's own documentation, shown in the tip below
    // the signature's — plain text, the same reason `documentation` below
    // is (`signature_help.rs`'s own doc comment on `documentation`).
    let parameter_documentation = parameter
        .and_then(|p| p.documentation.as_deref())
        .unwrap_or_default();
    ffi::FfiSignatureHelp {
        has_signature: true,
        label: QString::from(signature.label.as_str()),
        documentation: QString::from(signature.documentation.as_deref().unwrap_or_default()),
        has_active_parameter,
        parameter_start,
        parameter_end,
        parameter_documentation: QString::from(parameter_documentation),
        signature_index: index as u32,
        signature_count: count as u32,
    }
}

fn to_ffi_document_highlight(highlight: &lsp_core::DocumentHighlight) -> ffi::FfiDocumentHighlight {
    ffi::FfiDocumentHighlight {
        kind: match highlight.kind {
            lsp_core::HighlightKind::Text => ffi::FfiHighlightKind::Text,
            lsp_core::HighlightKind::Read => ffi::FfiHighlightKind::Read,
            lsp_core::HighlightKind::Write => ffi::FfiHighlightKind::Write,
        },
        start_line: highlight.range.start_line,
        start_character: highlight.range.start_character,
        end_line: highlight.range.end_line,
        end_character: highlight.range.end_character,
    }
}

fn to_ffi_inlay_hint(hint: &lsp_core::InlayHint) -> ffi::FfiInlayHint {
    ffi::FfiInlayHint {
        line: hint.line,
        character: hint.character,
        label: QString::from(hint.label.as_str()),
        kind: match hint.kind {
            lsp_core::InlayHintKind::Type => ffi::FfiInlayHintKind::Type,
            lsp_core::InlayHintKind::Parameter => ffi::FfiInlayHintKind::Parameter,
            lsp_core::InlayHintKind::Other => ffi::FfiInlayHintKind::Other,
        },
        padding_left: hint.padding_left,
        padding_right: hint.padding_right,
    }
}
