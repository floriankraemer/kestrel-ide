//! A blocking LSP client: framing, child-process transport, and the
//! [`LspManager`] that owns server lifecycle, restart policy, request routing
//! and document versions.
//!
//! Qt-free and runtime-free by design (see `docs/architecture/layering.md` and
//! the language-platform plan's decisions 8 and 9). Verified by:
//!
//! ```sh
//! cargo tree -p lsp-core -e normal | grep -i -e qt -e tokio   # must be empty
//! ```

pub mod apply_edit;
pub mod catalog;
pub mod code_action;
pub mod code_lens;
pub mod completion;
pub mod configuration;
pub mod diagnostics;
pub mod diff_preview;
pub mod document_highlight;
pub mod formatting;
pub mod framing;
pub mod hierarchy;
pub mod hover;
pub mod inlay_hint;
pub mod intentions;
pub mod manager;
pub mod navigation;
pub mod progress;
pub mod registration;
pub mod rename;
pub mod semantic_tokens;
pub mod signature_help;
pub mod tracker;
pub mod watched_files;
pub mod workspace_edit;

pub use apply_edit::{
    ApplyEditGate, ApplyEditVerdict, RefactorSession, RefactorSessions, APPLY_EDIT_TIMEOUT,
};
pub use catalog::{
    default_server, enabled_server, lsp_language_id, resolve_servers, PluginServer, ServerConfig,
    ServerDef, ServerOverride, ServerSource, SERVERS,
};
pub use code_action::{
    filter_by_kind, kind_matches, needs_unfiltered_retry, parse_code_actions,
    steps as action_steps, ActionStep, CodeActionItem, CommandRef,
};
pub use code_lens::{is_offered as code_lens_offered, parse_code_lenses, CodeLensItem};
pub use completion::{
    accept_range as completion_accept_range, additional_text_edits as completion_additional_edits,
    completion_prefix, filter as filter_completions, kind_name, own_edit as completion_own_edit,
    parse_completion, parse_resolve_provider as parse_completion_resolve_provider, should_request,
    strip_snippet, CompletionItem, CompletionList, CompletionResolveTracker, CompletionTracker,
    TextRange,
};
pub use configuration::resolve as resolve_configuration;
pub use diagnostics::{path_from_uri, to_diagnostics, uri_from_path};
pub use diff_preview::{file_diff, FileDiff};
pub use document_highlight::{parse_document_highlights, DocumentHighlight, HighlightKind};
pub use hierarchy::{
    parse_hierarchy_items, parse_incoming_calls, parse_outgoing_calls, type_hierarchy_outcome,
    HierarchyItem, IncomingCall, OutgoingCall, TypeHierarchyOutcome,
};
pub use hover::{
    hover_outcome, parse_hover, to_tooltip_html, HoverOutcome, HoverText, HoverTracker,
};
pub use inlay_hint::{line_range as inlay_hint_range, parse_inlay_hints, InlayHint, InlayHintKind};
pub use intentions::{
    assemble as assemble_intentions, is_preferred, suggests_organize_imports, Intention,
    IntentionGroup, ORGANIZE_IMPORTS,
};
pub use manager::{
    path_for, uri_for, LspError, LspEvent, LspManager, DEFAULT_REQUEST_TIMEOUT,
    DOCUMENT_HIGHLIGHT_TIMEOUT, INLAY_HINT_TIMEOUT, INTENTION_TIMEOUT, REFACTOR_TIMEOUT,
    SEMANTIC_TOKENS_TIMEOUT, SIGNATURE_HELP_TIMEOUT,
};
pub use navigation::{
    definition_outcome, parse_definition, virtual_doc_key, DefinitionOutcome, DefinitionTarget,
};
/// ADR-0052: where a project's tooling runs. Re-exported so an adapter
/// (`ui-shell`) that already depends on `lsp-core` can hold one — e.g. to
/// retranslate a `workspace/applyEdit`'s paths outside a
/// [`LspManager`]-holding closure — without also taking a direct
/// dependency on `process-exec`.
pub use process_exec::host::{set_remote_wsl_enabled, ExecHost};
pub use progress::{ProgressTracker, ServerActivity};
pub use registration::{Registration, Registrations, Watcher};
pub use rename::{
    parse_prepare_rename, prepare_outcome, rename_outcome, PrepareOutcome, PrepareRename,
    RenameOutcome,
};
pub use semantic_tokens::{
    overlay as overlay_semantic_tokens, parse_full_response as parse_semantic_tokens_full,
    parse_legend as parse_semantic_tokens_legend, scope_for as semantic_token_scope,
    MappedSpan as MappedSemanticSpan, SemanticToken, SemanticTokensLegend,
    STANDARD_TOKEN_MODIFIERS, STANDARD_TOKEN_TYPES,
};
pub use signature_help::{
    call_site_at, parse_signature_help, parse_signature_triggers,
    should_dismiss as signature_help_should_dismiss,
    should_request as should_request_signature_help, CallSite, ParameterInfo, SignatureHelp,
    SignatureInfo, SignatureTriggers,
};
pub use tracker::RequestTracker;
pub use workspace_edit::{
    apply_to_text, descending, parse_workspace_changes, parse_workspace_edit, plan as plan_edit,
    plan_changes, ChangeStep, DocumentEdits, EditError, EditGate, EditPlan, ResourceOp, TextEdit,
    WorkspaceChanges,
};
