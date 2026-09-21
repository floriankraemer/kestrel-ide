use core::pin::Pin;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::ffi::{self};
use crate::bridge::registry::{self, LspJob, SharedDiagnostics};

/// F2-8/F2-9: intentions, organize imports, signature help, document
/// highlights and inlay hints — split out once this file crossed the
/// file-size ceiling, the way `ai/agent.rs` splits out of `ai/chat.rs`.
/// C6: image-name completion and the "Pull image" intention, injected
/// before the language-server gate.
/// D5 (jvm-build-tools plan): pom.xml/build.gradle(.kts)/
/// libs.versions.toml coordinate completion, injected before the
/// language-server path the same way `containers.rs` injects image-name
/// completion — split out for the same file-size-ceiling reason.
/// F3.7 (database-tools plan): completion, inspections and two
/// intentions for a `.sql` file attached to a data source, injected the
/// same way — a third non-LSP file kind alongside containers and build
/// files.
mod build_files;
mod containers;
mod database;
mod lsp_surface;

/// RF8: code actions, rename, formatting, and the pending-edit preview
/// (diff/hunks/spans, cancel/exclude, `takePendingEdits`) they all publish
/// through — split out once this file crossed the file-size ceiling
/// (#162), the same reason `lsp_surface` exists. `refactor_controller.cpp`
/// is this module's one C++ consumer, mirroring the split there.
mod refactor;

/// Every `language-servers` contribution from the live plugin registry,
/// translated into `lsp_core::PluginServer`.
///
/// `lsp-core` stays free of the plugin stack (`docs/architecture/
/// layering.md`), so this mapping — not a `From` impl in either crate —
/// is where `LanguageServerContribution` becomes the plain data
/// `resolve_servers` understands. `ui-shell` already depends on
/// `plugin-host` for icon themes (`bridge/icons.rs`), so this is the same
/// pattern, not a new dependency.
pub(crate) fn plugin_servers() -> Vec<lsp_core::PluginServer> {
    let registry = plugin_host::registry();
    registry
        .language_servers()
        .map(|(plugin, server)| lsp_core::PluginServer {
            plugin_id: plugin.id().to_string(),
            language_id: server.language_id.clone(),
            name: server.name.clone(),
            command: server.command.clone(),
            args: server.args.clone(),
            settings_section: server.settings_section.clone(),
            settings: serde_json::to_value(&server.settings).unwrap_or(serde_json::Value::Null),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Language servers (Task L2)
// ---------------------------------------------------------------------------

// `LspJob` (one unit of work for the LSP worker thread) now lives in
// `bridge::registry`, alongside `set_lsp_jobs`/`push_lsp_job` (R8): the
// worker exists because `LspManager::start` blocks until the server has
// answered `initialize` — a real server can take a second or two — and the
// UI thread must not wait for that. Running *every* call through the same
// queue (not just `start`) is what keeps ordering honest: a `didChange`
// queued while the server is still starting is still delivered after the
// `didOpen` that preceded it. `registry` needs the type too, to give
// `SearchModel` a second sender onto the same channel for Find Usages.

/// C5: how long a burst of filesystem-watcher events is allowed to run
/// before it is flushed as one batched `workspace/didChangeWatchedFiles`.
/// 200ms is comfortably inside the 150-300ms window a debounce needs to
/// absorb a `git checkout`'s event storm without making a server wait
/// noticeably longer than a human notices.
const WATCHED_FILES_DEBOUNCE: Duration = Duration::from_millis(200);

/// The shared diagnostics store's key for one language server's rows
/// (ADR-0046): distinct from a build tool's (`build::source_key`) so the
/// two never clobber each other's rows for the same file, and distinct per
/// language id so two servers publishing for the same uri (rare, but not
/// impossible for a multi-language file) replace only their own.
fn lsp_source_key(language_id: &str) -> String {
    format!("lsp:{language_id}")
}

/// LSP's `FileChangeType` wire values, as sent over `watchedFileChanged`
/// (the Qt signal can't carry `lsp_core::watched_files::FileChangeKind`
/// itself, only primitives).
fn wire_to_file_change_kind(kind: i32) -> Option<lsp_core::watched_files::FileChangeKind> {
    use lsp_core::watched_files::FileChangeKind;
    match kind {
        1 => Some(FileChangeKind::Created),
        2 => Some(FileChangeKind::Changed),
        3 => Some(FileChangeKind::Deleted),
        _ => None,
    }
}

/// Rust side of the `LanguageService` QObject: a handle to the worker, the
/// resolved server table, and the diagnostics currently published. No rules —
/// see the bridge declaration above.
pub struct LanguageServiceRust {
    /// Shared with every other adapter (`registry::shared_session`) — needed
    /// here because F2-3's resource operations perform through
    /// `AppSession::apply_file_ops`, the same call the project tree's
    /// rename/delete use, so a renamed file retargets its open tab exactly
    /// as it does from the tree.
    session: std::rc::Rc<RefCell<app_core::AppSession>>,
    /// `None` before a project is open; dropping the sender is what stops the
    /// previous project's servers.
    jobs: RefCell<Option<std::sync::mpsc::Sender<LspJob>>>,
    /// `lsp_core::resolve_servers` applied to the user's settings, resolved
    /// once per project open.
    configs: RefCell<Vec<lsp_core::ServerConfig>>,
    /// Language ids whose server has been asked to start, so the first file of
    /// a language starts it and later ones don't re-queue a launch.
    started: RefCell<std::collections::HashSet<String>>,
    /// Open document path -> language id, so a change/save/close for a file we
    /// never opened against a server is dropped rather than sent.
    pub(crate) open_docs: RefCell<std::collections::HashMap<String, String>>,
    /// ADR-0052: the same `ExecHost` `LspManager::new` derived from this
    /// project's root, kept here too for the one path-retranslation site
    /// (`LspEvent::ApplyEdit`, in `apply_event`) that runs on the event
    /// listener thread rather than inside a `push_job` closure with a
    /// `&LspManager` already in hand. Set once, in `open_project`, from the
    /// same `root` string the manager itself was constructed from, so the
    /// two never disagree.
    host: RefCell<lsp_core::ExecHost>,
    /// The one Problems model (ADR-0046), shared with `BuildService`,
    /// `DiagnosticsService` and `AiChat` — a second store would mean the
    /// editor underlining a different set of problems than the Problems
    /// panel shows. Every row this adapter publishes is keyed under
    /// [`lsp_source_key`], so a server's diagnostics for a file never
    /// clobber (or get clobbered by) a build's for the same file.
    pub(crate) store: SharedDiagnostics,
    /// L3: which hover request is still the current one. The rule is
    /// `lsp_core`'s; what is kept here is only its state.
    hover: RefCell<lsp_core::HoverTracker>,
    /// L5: the same for completion, plus the last answer it accepted — the
    /// view re-reads that rather than being handed the list in the signal.
    completion: RefCell<lsp_core::CompletionTracker>,
    completions: RefCell<lsp_core::CompletionList>,
    /// Trigger characters per language, as each server advertised them in
    /// its `initialize` result (`LspEvent::ServerReady`).
    triggers: RefCell<std::collections::HashMap<String, Vec<String>>>,
    /// C7: whether each language's server offers `completionItem/resolve`,
    /// from the same `initialize` result, stored the same way trigger
    /// characters are — a per-language flag the accept path and the preview
    /// path both gate on, rather than sending the request to a server that
    /// never advertised it.
    completion_resolve_supported: RefCell<std::collections::HashMap<String, bool>>,
    /// C7: the language of the last `completionAt`, so an accept or a
    /// preview resolution — neither of which is handed a path — knows which
    /// server to ask. Set alongside `completion`/`completions`.
    completion_language: RefCell<Option<String>>,
    /// C6: Docker Hub cache/worker/pending request behind image-name
    /// completion, the `(line, start, end)` UTF-16 span the current image
    /// items replace, and the connected engines' image names the view
    /// pushes in (`setLocalImages`).
    hub: RefCell<containers::HubState>,
    container_completion_span: RefCell<(u32, u32, u32)>,
    local_images: RefCell<Vec<String>>,
    /// F3.7: attachment cache, per-source schema/dialect cache, and the
    /// idle-debounce state behind database completion/inspections/
    /// intentions.
    pub(crate) database: database::DatabaseAssist,
    /// C7: which completion-item preview resolution (documentation/detail as
    /// the popup's selection moves) is still the current one.
    completion_resolve: RefCell<lsp_core::CompletionResolveTracker>,
    /// RF8: the offers of the last `codeActionsAt`, plus the language they
    /// came from — resolving or executing one has to go back to that server.
    actions: RefCell<Vec<lsp_core::CodeActionItem>>,
    actions_language: RefCell<String>,
    /// F2-8: the offers of the last `requestIntentions`, plus the language
    /// they came from and the generation that invalidates a stale answer —
    /// a caret move sends a fresh request rather than waiting for this one.
    pub(crate) intentions: RefCell<Vec<lsp_core::Intention>>,
    pub(crate) intentions_language: RefCell<String>,
    pub(crate) intentions_tracker: RefCell<lsp_core::RequestTracker>,
    /// F2-9: whether, and on which characters, each language's server wants
    /// signature help (re)requested — from that server's `initialize`
    /// result, published on `ServerReady` the same way completion's trigger
    /// characters are.
    pub(crate) signature_triggers:
        RefCell<std::collections::HashMap<String, lsp_core::SignatureTriggers>>,
    pub(crate) signature_help: RefCell<Option<lsp_core::SignatureHelp>>,
    pub(crate) signature_tracker: RefCell<lsp_core::RequestTracker>,
    /// D6 (review fix #10): guards `refresh_version_hints`'s
    /// Central-augmented background delivery — without it, two saves
    /// close together race, and whichever background thread happens to
    /// finish *last* wins even if it was answering the *earlier* save.
    pub(crate) version_hints_tracker: RefCell<lsp_core::RequestTracker>,
    /// D6/D7 (screenshot review — the "diagnostic says one thing, quick
    /// fix offers nothing" bug): the exact hints `refresh_version_hints`
    /// last published for each open path, keyed by path. D7's quick fix
    /// reads from here — never recomputes — so it can only ever agree
    /// with the squiggle the user is actually looking at, including a
    /// hint only Central (not the local index) found.
    pub(crate) version_hints: RefCell<
        std::collections::HashMap<String, Vec<jvm_build_core::editing::versions::VersionHint>>,
    >,
    /// R3: which overload Up/Down has manually cycled to, overriding the
    /// server's own `activeSignature` until the next `signatureHelpReady` —
    /// a fresh answer means a different call, so it resets this rather than
    /// keeping a stale index from the call the caret just left.
    pub(crate) signature_display_index: Cell<Option<usize>>,
    pub(crate) highlights: RefCell<Vec<lsp_core::DocumentHighlight>>,
    pub(crate) highlights_tracker: RefCell<lsp_core::RequestTracker>,
    pub(crate) inlay_hints: RefCell<Vec<lsp_core::InlayHint>>,
    pub(crate) inlay_hints_tracker: RefCell<lsp_core::RequestTracker>,
    /// C9: the last decoded-and-mapped semantic-token spans per open
    /// document path, fire-and-forget refreshed by `request_semantic_tokens`
    /// — see that method's doc comment for why an unsupported/failed/empty
    /// answer leaves a path's entry untouched rather than clearing it.
    pub(crate) semantic_tokens:
        RefCell<std::collections::HashMap<String, Vec<lsp_core::MappedSemanticSpan>>>,
    /// C10: the last-fetched `textDocument/codeLens` items per open
    /// document path, fire-and-forget refreshed by `request_code_lenses` —
    /// same storage shape `semantic_tokens` uses, and for the same reason
    /// (`request_code_lenses`'s own doc comment).
    pub(crate) code_lenses: RefCell<std::collections::HashMap<String, Vec<lsp_core::CodeLensItem>>>,
    /// C11: the last `prepareCallHierarchy` answer, plus the language it
    /// came from — `requestIncomingCalls`/`requestOutgoingCalls` index into
    /// this by position, the same convention `runCodeLens` follows for
    /// `code_lenses`.
    pub(crate) call_hierarchy_items: RefCell<Vec<lsp_core::HierarchyItem>>,
    pub(crate) call_hierarchy_language: RefCell<String>,
    pub(crate) incoming_calls: RefCell<Vec<lsp_core::IncomingCall>>,
    pub(crate) outgoing_calls: RefCell<Vec<lsp_core::OutgoingCall>>,
    /// C11: the type-hierarchy twins of the four fields above.
    pub(crate) type_hierarchy_items: RefCell<Vec<lsp_core::HierarchyItem>>,
    pub(crate) type_hierarchy_language: RefCell<String>,
    pub(crate) supertypes: RefCell<Vec<lsp_core::HierarchyItem>>,
    pub(crate) subtypes: RefCell<Vec<lsp_core::HierarchyItem>>,
    /// C11: the project index, shared with `SearchModel`
    /// (`registry::index_slot`) — the fallback for `supertypes`/`subtypes`
    /// when no server answers (`lsp_core::hierarchy::type_hierarchy_outcome`),
    /// built from the same `inh_type`/`inh_supertype` edges
    /// `SearchModel::find_supertypes`/`find_implementations` already read.
    pub(crate) index: mcp_server::IndexHandle,
    /// The refactoring waiting to be applied, if any: what it changes, what
    /// to call it, and — when it came from the server asking us — the gate
    /// that server is blocked on.
    pending: RefCell<Option<PendingRefactor>>,
    /// RF2's staleness rule. The comparison is `lsp_core`'s; only its state
    /// lives here.
    edits: RefCell<lsp_core::EditGate>,
    /// F0-16: what each server is currently working on, as its own
    /// `$/progress` reported it (`lsp_core::ProgressTracker` decides that
    /// per server; this only collects the answers). A `BTreeMap` because
    /// several servers can be busy at once and the status bar shows one:
    /// keying by language id makes which one deterministic rather than
    /// dependent on event arrival order.
    busy: RefCell<std::collections::BTreeMap<String, (String, lsp_core::ServerActivity)>>,
    /// C5: filesystem changes waiting for the debounce window to lapse
    /// before becoming one batched `workspace/didChangeWatchedFiles` per
    /// affected server. See `watched_file_changed`/`flush_watched_changes`.
    watched_changes: RefCell<Vec<(PathBuf, lsp_core::watched_files::FileChangeKind)>>,
    /// Whether a flush is already scheduled — sending only one debounce
    /// timer per burst, however many events land inside its window, is
    /// what makes this a debounce rather than one timer per event.
    watch_flush_pending: Cell<bool>,
}

impl Default for LanguageServiceRust {
    fn default() -> Self {
        LanguageServiceRust {
            session: crate::bridge::registry::shared_session(),
            jobs: RefCell::default(),
            configs: RefCell::default(),
            started: RefCell::default(),
            open_docs: RefCell::default(),
            host: RefCell::new(lsp_core::ExecHost::Local),
            store: SharedDiagnostics::default(),
            hover: RefCell::default(),
            completion: RefCell::default(),
            completions: RefCell::default(),
            triggers: RefCell::default(),
            completion_resolve_supported: RefCell::default(),
            completion_language: RefCell::default(),
            hub: RefCell::default(),
            container_completion_span: RefCell::default(),
            local_images: RefCell::default(),
            database: database::DatabaseAssist::default(),
            completion_resolve: RefCell::default(),
            actions: RefCell::default(),
            actions_language: RefCell::default(),
            intentions: RefCell::default(),
            intentions_language: RefCell::default(),
            intentions_tracker: RefCell::default(),
            signature_triggers: RefCell::default(),
            signature_help: RefCell::default(),
            signature_tracker: RefCell::default(),
            version_hints_tracker: RefCell::default(),
            version_hints: RefCell::default(),
            signature_display_index: Cell::default(),
            highlights: RefCell::default(),
            highlights_tracker: RefCell::default(),
            inlay_hints: RefCell::default(),
            inlay_hints_tracker: RefCell::default(),
            semantic_tokens: RefCell::default(),
            code_lenses: RefCell::default(),
            call_hierarchy_items: RefCell::default(),
            call_hierarchy_language: RefCell::default(),
            incoming_calls: RefCell::default(),
            outgoing_calls: RefCell::default(),
            type_hierarchy_items: RefCell::default(),
            type_hierarchy_language: RefCell::default(),
            supertypes: RefCell::default(),
            subtypes: RefCell::default(),
            index: crate::bridge::registry::index_slot(),
            pending: RefCell::default(),
            edits: RefCell::default(),
            busy: RefCell::default(),
            watched_changes: RefCell::default(),
            watch_flush_pending: Cell::default(),
        }
    }
}

/// A refactoring that has produced edits and is waiting for the view to
/// apply them.
pub(crate) struct PendingRefactor {
    plan: lsp_core::EditPlan,
    /// Files the user unticked in the preview.
    excluded: Vec<String>,
    /// Set when this edit came from a `workspace/applyEdit`, i.e. a server
    /// is blocked until it is answered. Answering it is not optional, so
    /// every path out of here — applied, excluded, cancelled, superseded —
    /// goes through `settle`.
    gate: Option<lsp_core::ApplyEditGate>,
}

impl PendingRefactor {
    /// Tell a waiting server what became of its edit. A refactoring the
    /// editor started has no gate and nothing to tell.
    fn settle(&self, applied: bool, reason: &str) {
        let Some(gate) = &self.gate else {
            return;
        };
        if applied {
            gate.claim();
        } else {
            gate.refuse(reason);
        }
    }
}

/// Map one `lsp_core::ResourceOp` onto the `app_core::FileOp` that performs
/// it. Translation only (ADR-0026): what each field means is decided in
/// `app_core::apply_file_ops`, not here.
pub(crate) fn to_file_op(op: &lsp_core::ResourceOp) -> app_core::FileOp {
    match op {
        lsp_core::ResourceOp::Create {
            path,
            overwrite,
            ignore_if_exists,
            ..
        } => app_core::FileOp::Create {
            path: std::path::PathBuf::from(path),
            overwrite: *overwrite,
            ignore_if_exists: *ignore_if_exists,
        },
        lsp_core::ResourceOp::Rename {
            old_path,
            new_path,
            overwrite,
            ignore_if_exists,
            ..
        } => app_core::FileOp::Rename {
            from: std::path::PathBuf::from(old_path),
            to: std::path::PathBuf::from(new_path),
            overwrite: *overwrite,
            ignore_if_exists: *ignore_if_exists,
        },
        lsp_core::ResourceOp::Delete {
            path,
            recursive,
            ignore_if_not_exists,
            ..
        } => app_core::FileOp::Delete {
            path: std::path::PathBuf::from(path),
            recursive: *recursive,
            ignore_if_not_exists: *ignore_if_not_exists,
        },
    }
}

/// The same operation, as the preview lists it.
pub(crate) fn to_ffi_resource_op(op: &lsp_core::ResourceOp) -> ffi::FfiResourceOp {
    match op {
        lsp_core::ResourceOp::Create { path, .. } => ffi::FfiResourceOp {
            kind: ffi::FfiResourceOpKind::Create,
            path: QString::from(path.as_str()),
            new_path: QString::default(),
        },
        lsp_core::ResourceOp::Rename {
            old_path, new_path, ..
        } => ffi::FfiResourceOp {
            kind: ffi::FfiResourceOpKind::Rename,
            path: QString::from(old_path.as_str()),
            new_path: QString::from(new_path.as_str()),
        },
        lsp_core::ResourceOp::Delete { path, .. } => ffi::FfiResourceOp {
            kind: ffi::FfiResourceOpKind::Delete,
            path: QString::from(path.as_str()),
            new_path: QString::default(),
        },
    }
}

/// Markdown to the HTML subset `QTextBrowser`/`QTextDocument` understand
/// (R2) — the same one-shot, cache-free entry point the Preview dock's
/// `PreviewProviderRust` uses for a single render, appropriate here since a
/// completion row's documentation is short and rendered at most once per
/// selection, not on every keystroke.
fn render_doc_html(markdown: &str) -> String {
    if markdown.is_empty() {
        return String::new();
    }
    markdown_preview::render(markdown, &markdown_preview::RenderOptions::default()).html
}

/// R3: a hover answer as HTML, replacing `lsp_core::to_tooltip_html`'s old
/// mini Markdown renderer (bold, fences and rules only) with the real one
/// this crate already has for completion documentation — lists, links and
/// tables now render instead of showing as raw source. A plaintext hover
/// still goes through `to_tooltip_html`, which is source code (or, for the
/// index-declaration fallback, a bare signature) rather than prose and must
/// not have its punctuation reinterpreted as Markdown.
fn render_hover_html(hover: &lsp_core::HoverText) -> String {
    if hover.markdown {
        render_doc_html(&hover.value)
    } else {
        lsp_core::to_tooltip_html(hover)
    }
}

/// R3: `hoverAt`'s composed popup — the hover HTML (if the server or the
/// index answered) followed by every diagnostic covering the position, each
/// as its severity and source. A tested, Qt-free function despite living in
/// a bridge module (`convert.rs`'s tests already use this shape): it takes
/// and returns plain types, so this is exercised without a Qt runtime.
/// `None`/empty in both arguments never happens at the one call site —
/// `hover_at` falls back to the index instead — but is handled here as
/// "nothing to show" rather than asserted against, so a future caller with
/// a genuinely empty answer degrades rather than panics.
fn compose_hover_html(
    hover_html: Option<&str>,
    diagnostics: &[diagnostics_core::DiagnosticRow],
) -> String {
    let mut sections: Vec<String> = Vec::new();
    if let Some(html) = hover_html {
        if !html.is_empty() {
            sections.push(html.to_string());
        }
    }
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            diagnostics_core::Severity::Error => "Error",
            diagnostics_core::Severity::Warning => "Warning",
            diagnostics_core::Severity::Information => "Info",
            diagnostics_core::Severity::Hint => "Hint",
        };
        let source = if diagnostic.source.is_empty() {
            String::new()
        } else {
            format!(" ({})", html_escape(&diagnostic.source))
        };
        sections.push(format!(
            "<b>{severity}{source}:</b> {}",
            html_escape(&diagnostic.message)
        ));
    }
    sections.join("<hr>")
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One candidate for [`LanguageServiceRust::fallback_completion`]: a bare
/// keyword or document word, with nothing a server would have added
/// (detail, documentation, a `textEdit` range) — `kind` is `Some(14)`
/// ("keyword", `lsp_core::completion::kind_name`) for a grammar keyword,
/// `None` for a document word, matching how the popup already tells a
/// plain identifier from a typed one apart.
fn fallback_item(text: String, kind: Option<u32>) -> lsp_core::CompletionItem {
    lsp_core::CompletionItem {
        insert: text.clone(),
        label: text,
        kind,
        detail: String::new(),
        documentation: String::new(),
        sort_text: None,
        filter_text: None,
        is_snippet: false,
        deprecated: false,
        range: None,
        raw: serde_json::Value::Null,
    }
}

fn to_ffi_completion(m: lsp_core::CompletionMatch, prefix_length: u32) -> ffi::FfiCompletionItem {
    let item = m.item;
    let range = item.range.unwrap_or(lsp_core::TextRange {
        start_line: 0,
        start_character: 0,
        end_line: 0,
        end_character: 0,
    });
    // C7: the item exactly as the server sent it, carried through the FFI
    // boundary as JSON text because `completionItem/resolve` needs it back
    // verbatim and cxx-qt structs cannot hold an arbitrary `serde_json::Value`.
    // Whether it is worth sending anywhere is `completion_resolve_supported`'s
    // decision, not this translation's.
    let resolve_data = serde_json::to_string(&item.raw).unwrap_or_default();
    let documentation_html = render_doc_html(&item.documentation);
    ffi::FfiCompletionItem {
        label: QString::from(item.label.as_str()),
        kind: QString::from(lsp_core::kind_name(item.kind)),
        detail: QString::from(item.detail.as_str()),
        documentation: QString::from(documentation_html.as_str()),
        insert: QString::from(item.insert.as_str()),
        has_range: item.range.is_some(),
        start_line: range.start_line,
        start_character: range.start_character,
        end_line: range.end_line,
        end_character: range.end_character,
        prefix_length,
        resolve_data: QString::from(resolve_data.as_str()),
        deprecated: item.deprecated,
        is_snippet: item.is_snippet,
        match_positions: m.positions.iter().map(|&p| p as u32).collect(),
    }
}

impl ffi::LanguageService {
    pub fn open_project(mut self: Pin<&mut Self>, root_path: &QString) {
        let root = root_path.to_string();
        if root.is_empty() {
            return;
        }

        // Dropping the previous sender ends that worker's loop, which shuts
        // its servers down — no separate stop path to keep in sync.
        self.jobs.borrow_mut().take();
        registry::set_lsp_jobs(None);
        self.started.borrow_mut().clear();
        self.open_docs.borrow_mut().clear();
        self.triggers.borrow_mut().clear();
        self.store.borrow_mut().clear();
        // The previous project's servers are gone with their worker, so
        // whatever they were still working on is over.
        self.busy.borrow_mut().clear();
        self.as_mut()
            .server_busy_changed(false, QString::default(), QString::default(), false, 0);

        // The resolved layer, not the global file: a project may name its
        // own language servers (ADR-0022). Deliberately synchronous, unlike
        // `SearchModel::open_index`'s matching read (ADR-0037 § Alternatives).
        let settings = crate::bridge::convert::load_resolved_settings();
        let overrides: Vec<lsp_core::ServerOverride> = settings
            .language_servers
            .iter()
            .map(|entry| lsp_core::ServerOverride {
                language_id: entry.language_id.clone(),
                name: entry.name.clone(),
                command: entry.command.clone(),
                args: entry.args.clone(),
                enabled: entry.enabled,
            })
            .collect();
        *self.configs.borrow_mut() = lsp_core::resolve_servers(&overrides, &plugin_servers());
        *self.host.borrow_mut() = lsp_core::ExecHost::for_path(std::path::Path::new(&root));

        let (manager, events) = lsp_core::LspManager::new(lsp_core::uri_from_path(&root));
        let (jobs, rx) = std::sync::mpsc::channel::<LspJob>();
        std::thread::spawn(move || {
            for job in rx {
                job(&manager);
            }
            // The sender was dropped: the project closed or the app is going
            // away, so the child processes must not outlive it.
            manager.stop_all();
        });

        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            for event in events {
                let _ = qt_thread.queue(move |service: Pin<&mut Self>| service.apply_event(event));
            }
        });

        // R8: `SearchModel` gets its own sender onto the same channel, so
        // Find Usages can queue a `references` request beside this
        // project's `didOpen`/`didChange` traffic instead of needing its
        // own `LspManager`.
        registry::set_lsp_jobs(Some(jobs.clone()));
        *self.jobs.borrow_mut() = Some(jobs);
        self.as_mut().diagnostics_changed();
    }

    pub fn document_opened(mut self: Pin<&mut Self>, path: &QString, text: &QString) {
        let path_str = path.to_string();
        let Some(config) = self.config_for_path(&path_str) else {
            self.as_mut().open_build_file_document(&path_str, text);
            // F3.7: a `.sql` file attached to a data source has no
            // server either — same "register it anyway, for this
            // module's own lifecycle" reasoning as the line above.
            self.as_mut().open_database_document(&path_str, text);
            return;
        };
        let language_id = config.language_id.clone();
        let uri = lsp_core::uri_from_path(&path_str);
        let text_str = text.to_string();
        self.open_docs
            .borrow_mut()
            .insert(path_str, language_id.clone());

        if self.started.borrow_mut().insert(language_id.clone()) {
            self.as_mut().start_server(config);
        }
        let language = language_id.clone();
        self.as_mut().push_job(move |manager| {
            let _ = manager.did_open(&uri, &language, &text_str);
        });
        // C9: fire-and-forget — a server whose semantic-tokens capability
        // is not yet known (still starting, or registers it dynamically
        // after `initialized`) simply gets a no-op from this call; nothing
        // here retries. `SyntaxHighlighter` re-requesting on its own next
        // revision-change hook is the second call site
        // `overlay_semantic_tokens`'s TODO leaves for follow-up.
        self.as_mut().request_semantic_tokens(path, text);
        // C10: same fire-and-forget convention, immediately above.
        self.as_mut().request_code_lenses(path);
        // D6 (review fix #6): a build file can have a real language
        // server configured too (lemminx for `pom.xml`,
        // kotlin-language-server for `build.gradle.kts`) — that branch
        // returned before this point instead of falling through to
        // `open_build_file_document`, so version hints were only ever
        // computed when *no* server existed. Cheap no-op for any other
        // file, same as every other `refresh_version_hints` call site.
        self.as_mut().refresh_version_hints(&path.to_string());
    }

    /// D0: a build file (`pom.xml`, `build.gradle(.kts)`, `libs.versions.toml`, …)
    /// has no language server, so `document_opened`'s ordinary path bails
    /// out before the file ever enters `open_docs` — leaving D7's quick fix
    /// (`refactor::plan_changes`, off `open_document_paths()`) to write
    /// straight to disk under a dirty tab instead of splicing into the open
    /// buffer. Registering it here, with no server started, gives it the
    /// same `open_docs` membership as a served document.
    ///
    /// Queuing `did_open`/`did_change`/`did_close` for a language with no
    /// server is already harmless: every `LspManager::notify` call returns
    /// `Err(LspError::NoServer(..))`, and every caller in this file already
    /// swallows that with `let _ =` — see
    /// `lsp_core::manager::no_server_document_lifecycle_tests`.
    fn open_build_file_document(mut self: Pin<&mut Self>, path_str: &str, text: &QString) {
        // Review fix #7: a basename-only rule, not the plugin's
        // root-relative sync globs (`jvm_build_core::sync::is_build_file`)
        // — those exist to answer "does this project have a Gradle/Maven
        // root at all" for trust/sync, and a literal `"build.gradle"`
        // pattern never matches a module's own `app/build.gradle`. D0
        // must register *every* build file this module can edit, at any
        // depth, or a non-root module's file never enters `open_docs` and
        // D7's quick fix falls back to writing straight to disk.
        if !jvm_build_core::editing::context::is_build_file(Path::new(path_str)) {
            return;
        }
        let language_id = syntax_core::language_for_path(Path::new(path_str)).id();
        self.open_docs
            .borrow_mut()
            .insert(path_str.to_string(), language_id.clone());
        let uri = lsp_core::uri_from_path(path_str);
        let text_str = text.to_string();
        self.push_job(move |manager| {
            let _ = manager.did_open(&uri, &language_id, &text_str);
        });
        // D6: the file just entered `open_docs`, so its version hints
        // (if any) have never been computed.
        self.as_mut().refresh_version_hints(path_str);
    }

    pub fn document_changed(mut self: Pin<&mut Self>, path: &QString, text: &QString) {
        let path = path.to_string();
        if !self.open_docs.borrow().contains_key(&path) {
            return;
        }
        let uri = lsp_core::uri_from_path(&path);
        let text_str = text.to_string();
        self.push_job(move |manager| {
            let _ = manager.did_change(&uri, &text_str);
        });
        // F3.7: cheap no-op for a path with no attachment.
        self.as_mut().schedule_database_inspection(&path);
    }

    pub fn document_saved(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        if !self.open_docs.borrow().contains_key(&path) {
            return;
        }
        let uri = lsp_core::uri_from_path(&path);
        self.push_job(move |manager| {
            let _ = manager.did_save(&uri);
        });
        // D6: a saved build file may have changed the declared versions
        // (or their coordinates) — recompute. Cheap no-op for any other
        // file: `refresh_version_hints` only does real work once
        // `editing::context::declared_versions` recognises the path.
        self.as_mut().refresh_version_hints(&path);
        // F3.7: same cheap-no-op convention, for a database-attached file.
        self.as_mut().refresh_database_inspections_now(&path);
    }

    pub fn document_closed(mut self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        let Some(language_id) = self.open_docs.borrow_mut().remove(&path) else {
            return;
        };
        let uri = lsp_core::uri_from_path(&path);
        self.store
            .borrow_mut()
            .remove(&lsp_source_key(&language_id), &uri);
        let closed = uri.clone();
        self.push_job(move |manager| {
            let _ = manager.did_close(&closed);
        });
        // D6: a closed build file's version hints must go with it, the
        // same way its language-server rows just did above.
        self.as_mut().clear_version_hints(&path);
        // F3.7: same for a database-attached file's inspections and its
        // cached attachment.
        self.as_mut().clear_database_inspections(&path);
        self.as_mut().diagnostics_changed();
    }

    /// C5: `ProjectTreeModel::watchedFileChanged` — buffer the change and
    /// make sure exactly one debounce timer is running. `git checkout` in a
    /// real repo fires thousands of these in a burst; sending one
    /// `workspace/didChangeWatchedFiles` per event would stall a server
    /// behind re-analysing between each, so every event inside the
    /// `WATCHED_FILES_DEBOUNCE` window collapses into the next flush.
    pub fn watched_file_changed(mut self: Pin<&mut Self>, path: &QString, kind: i32) {
        let Some(kind) = wire_to_file_change_kind(kind) else {
            return;
        };
        self.watched_changes
            .borrow_mut()
            .push((PathBuf::from(path.to_string()), kind));

        if self.watch_flush_pending.replace(true) {
            return; // A timer is already on its way; this event rides it.
        }
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            std::thread::sleep(WATCHED_FILES_DEBOUNCE);
            let _ = qt_thread.queue(|service: Pin<&mut Self>| service.flush_watched_changes());
        });
    }

    /// Send the buffered changes, one batched notification per server whose
    /// language they touch, filtered through that server's watchers inside
    /// `LspManager::did_change_watched_files`. A language with no server
    /// currently running is dropped here rather than queued and later
    /// silently ignored by `LspManager::server` — same "no server, nothing
    /// sent" rule either way, just paid once per flush instead of per file.
    fn flush_watched_changes(self: Pin<&mut Self>) {
        self.watch_flush_pending.set(false);
        let changes = std::mem::take(&mut *self.watched_changes.borrow_mut());
        if changes.is_empty() {
            return;
        }
        let started = self.started.borrow();
        let mut by_language: std::collections::HashMap<
            String,
            Vec<(PathBuf, lsp_core::watched_files::FileChangeKind)>,
        > = std::collections::HashMap::new();
        for (path, kind) in changes {
            let catalog_id = syntax_core::language_for_path(&path).id();
            let language_id = lsp_core::lsp_language_id(&catalog_id).to_string();
            if started.contains(&language_id) {
                by_language
                    .entry(language_id)
                    .or_default()
                    .push((path, kind));
            }
        }
        drop(started);
        for (language_id, changes) in by_language {
            self.push_job(move |manager| {
                let _ = manager.did_change_watched_files(&language_id, &changes);
            });
        }
    }

    pub fn apply_server_settings(self: Pin<&mut Self>) {
        // The resolved layer, not the global file: a project may name its
        // own language servers (ADR-0022), and a project that pins a
        // toolchain-local server is the reason that field is project-scoped.
        let settings = crate::bridge::convert::load_resolved_settings();
        let overrides: Vec<lsp_core::ServerOverride> = settings
            .language_servers
            .iter()
            .map(|entry| lsp_core::ServerOverride {
                language_id: entry.language_id.clone(),
                name: entry.name.clone(),
                command: entry.command.clone(),
                args: entry.args.clone(),
                enabled: entry.enabled,
            })
            .collect();
        let resolved = lsp_core::resolve_servers(&overrides, &plugin_servers());

        // Which running servers the new settings no longer describe: the
        // comparison is between two resolved configurations, so "changed" is
        // `lsp_core`'s definition of the launch, not a field-by-field guess.
        let previous = self.configs.borrow().clone();
        let stale: Vec<String> = self
            .started
            .borrow()
            .iter()
            .filter(|language_id| {
                let before = previous.iter().find(|c| &&c.language_id == language_id);
                let after = lsp_core::enabled_server(&resolved, language_id);
                match (before, after) {
                    (Some(before), Some(after)) => before != after,
                    _ => true,
                }
            })
            .cloned()
            .collect();
        *self.configs.borrow_mut() = resolved;

        for language_id in stale {
            self.started.borrow_mut().remove(&language_id);
            self.triggers.borrow_mut().remove(&language_id);
            // Forgetting the documents is what lets `reopenDocument` start
            // the replacement server and re-send `didOpen` to it.
            self.open_docs
                .borrow_mut()
                .retain(|_, open_for| open_for != &language_id);
            let stopping = language_id.clone();
            self.as_ref()
                .push_job(move |manager| manager.stop(&stopping));
        }
    }

    pub fn reopen_document(self: Pin<&mut Self>, path: &QString, text: &QString) {
        if self.open_docs.borrow().contains_key(&path.to_string()) {
            return;
        }
        self.document_opened(path, text);
    }

    pub fn restart_server(mut self: Pin<&mut Self>, language_id: &QString) {
        let language_id = language_id.to_string();
        let config = self
            .configs
            .borrow()
            .iter()
            .find(|config| config.language_id == language_id)
            .cloned();
        let Some(config) = config else {
            return;
        };
        let stopping = language_id.clone();
        self.as_ref()
            .push_job(move |manager| manager.stop(&stopping));
        self.started.borrow_mut().insert(language_id);
        self.as_mut().start_server(config);
    }

    pub fn has_server_for_file(&self, path: &QString) -> bool {
        self.config_for_path(&path.to_string()).is_some()
    }

    pub fn server_name_for_file(&self, path: &QString) -> QString {
        match self.config_for_path(&path.to_string()) {
            Some(config) => QString::from(config.name.as_str()),
            None => QString::default(),
        }
    }

    /// R3: the LSP hover plus every diagnostic covering `(line, character)`,
    /// composed into one popup. `hover_ready` fires whenever there is
    /// *anything* to show — a server's answer, a diagnostic, or both — and
    /// only a request with nothing at all still falls through to
    /// `hover_fallback`'s index-declaration answer, exactly as before R3.
    pub fn hover_at(mut self: Pin<&mut Self>, path: &QString, line: u32, character: u32) {
        let path = path.to_string();
        let token = self.hover.borrow_mut().begin();
        let uri = lsp_core::uri_from_path(&path);
        if !self.open_docs.borrow().contains_key(&path) {
            // No server has this document, so there is nothing to ask the
            // server for — but a build/analyzer diagnostic can still cover
            // this position, and widening the hover trigger to a squiggle
            // (R3) is pointless if that case still fell all the way back
            // to the index.
            let diagnostics = self.store.borrow().at(&uri, line, character);
            if diagnostics.is_empty() {
                self.as_mut().hover_fallback();
            } else {
                let html = compose_hover_html(None, &diagnostics);
                self.as_mut().hover_ready(QString::from(html.as_str()));
            }
            return;
        }
        let qt_thread = self.as_mut().qt_thread();
        let queued = self.push_job(move |manager| {
            let outcome = lsp_core::hover_outcome(Some(manager.hover(&uri, line, character)));
            let hover_html = match outcome {
                lsp_core::HoverOutcome::Lsp(hover) => Some(render_hover_html(&hover)),
                lsp_core::HoverOutcome::Index => None,
            };
            let uri = uri.clone();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                // A dwell the pointer has already moved on from is dropped
                // on both paths, so a late answer never appears under a
                // different word.
                if !service.hover.borrow().accept(token) {
                    return;
                }
                let diagnostics = service.store.borrow().at(&uri, line, character);
                if hover_html.is_none() && diagnostics.is_empty() {
                    service.as_mut().hover_fallback();
                    return;
                }
                let html = compose_hover_html(hover_html.as_deref(), &diagnostics);
                service.as_mut().hover_ready(QString::from(html.as_str()));
            });
        });
        if !queued {
            self.as_mut().hover_fallback();
        }
    }

    /// Resolve (if needed) and apply one code action, publishing whatever
    /// `WorkspaceChanges` its steps produce through the pending-refactor
    /// protocol. Shared by `applyCodeAction` and `applyIntention` — the two
    /// surfaces differ only in how the action was found.
    pub(crate) fn run_action(
        mut self: Pin<&mut Self>,
        action: lsp_core::CodeActionItem,
        language_id: String,
        buffer_revision: i64,
    ) {
        let open_paths = self.open_document_paths();
        let current_path = self.current_path_of(&action);
        self.edits.borrow_mut().begin(buffer_revision);
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            // The guard is what makes an edit the command asks for
            // legitimate; without it `lsp_core` refuses it as unsolicited
            // and the refactoring silently does nothing.
            let _session = manager.begin_refactor();
            let resolved = if action.needs_resolve() {
                manager
                    .resolve_code_action(&language_id, &action)
                    .ok()
                    .and_then(|mut items| items.pop())
                    .unwrap_or(action)
            } else {
                action
            };

            let mut changes = lsp_core::WorkspaceChanges::default();
            let mut failure = None;
            for step in lsp_core::action_steps(&resolved) {
                match step {
                    lsp_core::ActionStep::ApplyEdit(edit) => {
                        // `manager.parse_workspace_changes`, not the free
                        // function: it retranslates every path through this
                        // project's `ExecHost` (ADR-0052), a no-op unless
                        // the root is a WSL UNC path.
                        match manager.parse_workspace_changes(&edit) {
                            Ok(parsed) => changes.steps.extend(parsed.steps),
                            Err(e) => failure = Some(e.to_string()),
                        }
                    }
                    // Whatever the command produces arrives as its own
                    // `workspace/applyEdit`, and is published from there.
                    lsp_core::ActionStep::Execute(command) => {
                        if let Err(e) = manager.execute_command(&language_id, &command) {
                            failure = Some(e.to_string());
                        }
                    }
                }
            }
            let title = resolved.title.clone();
            let versions: std::collections::HashMap<String, i32> = changes
                .documents()
                .filter_map(|doc| {
                    manager
                        .document_version(&doc.uri)
                        .map(|v| (doc.uri.clone(), v))
                })
                .collect();
            let planned = if changes.steps.is_empty() {
                Ok(lsp_core::EditPlan::default())
            } else {
                lsp_core::plan_changes(changes, &open_paths, &current_path, &|uri| {
                    versions.get(uri).copied()
                })
            };
            let _ = qt_thread.queue(move |service: Pin<&mut Self>| {
                if let Some(message) = failure {
                    service.finish_refactor(Err(message));
                    return;
                }
                match planned {
                    // An action that only ran a command has nothing to
                    // publish here; its edit arrives as an ApplyEdit event.
                    Ok(plan) if plan.is_empty() => {}
                    Ok(plan) => service.publish_refactor(title, plan, None),
                    Err(e) => service.finish_refactor(Err(e.to_string())),
                }
            });
        });
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

    pub fn cancel_hover(self: Pin<&mut Self>) {
        self.hover.borrow_mut().cancel();
    }

    pub fn completion_at(
        mut self: Pin<&mut Self>,
        path: &QString,
        line: u32,
        character: u32,
        text_before_cursor: &QString,
        explicit_request: bool,
    ) {
        let path = path.to_string();
        // C6: an image reference in a Dockerfile/compose file is answered
        // locally (+ Docker Hub), whether or not a YAML server is running.
        if self.as_mut().container_completion(
            &path,
            line,
            character,
            &text_before_cursor.to_string(),
        ) {
            return;
        }
        // F3.7: a `.sql` file attached to a data source is answered
        // locally too, the same non-LSP short-circuit as containers.
        if self.as_mut().database_completion(
            &path,
            line,
            character,
            &text_before_cursor.to_string(),
        ) {
            return;
        }
        // D5: a build file has no server (D0 registers it in `open_docs`
        // anyway, for D7's quick fix), so without this the LSP branch
        // below would silently answer nothing for it.
        if self
            .as_mut()
            .build_file_completion(&path, line, character, explicit_request)
        {
            return;
        }
        let Some(language_id) = self.open_docs.borrow().get(&path).cloned() else {
            // R2: no server for this file's language (or none at all) —
            // keyword and document-word completion fill in, so Ctrl+Space
            // always offers something rather than nothing.
            self.as_mut().fallback_completion(
                &path,
                &text_before_cursor.to_string(),
                explicit_request,
            );
            return;
        };
        let text_before_cursor = text_before_cursor.to_string();
        let worth_asking = lsp_core::should_request(
            self.triggers
                .borrow()
                .get(&language_id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            &text_before_cursor,
            explicit_request,
            &self.completion.borrow(),
        );
        if !worth_asking {
            return;
        }

        let uri = lsp_core::uri_from_path(&path);
        *self.completion_language.borrow_mut() = Some(language_id.clone());
        let token = self
            .completion
            .borrow_mut()
            .begin(lsp_core::completion_prefix(&text_before_cursor));
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let Ok(list) = manager.completion(&uri, line, character) else {
                return;
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if !service
                    .completion
                    .borrow_mut()
                    .deliver(token, list.is_incomplete)
                {
                    return;
                }
                *service.completions.borrow_mut() = list;
                service.as_mut().completion_ready();
            });
        });
    }

    /// R2: keyword and document-word completion, for the file whose
    /// language has no running server at all (`completion_at`'s early
    /// return, above). Synchronous — no network round trip to wait for —
    /// so this fills in `self.completions`/`self.completion` exactly the
    /// way a server's answer would and fires the same `completion_ready`,
    /// letting the view's `completionItems`/`showCompletions` path stay
    /// unaware anything different happened.
    fn fallback_completion(
        mut self: Pin<&mut Self>,
        path: &str,
        text_before_cursor: &str,
        explicit_request: bool,
    ) {
        let worth_asking = lsp_core::should_request(
            &[],
            text_before_cursor,
            explicit_request,
            &self.completion.borrow(),
        );
        if !worth_asking {
            return;
        }
        *self.completion_language.borrow_mut() = None;
        self.completion
            .borrow_mut()
            .begin(lsp_core::completion_prefix(text_before_cursor));

        let language = syntax_core::language_for_path(Path::new(path));
        let content = self
            .session
            .borrow()
            .content_for_path(Path::new(path))
            .unwrap_or_default();
        let mut words = editor_core::words_in(&content);
        words.retain(|word| !word.is_empty());
        let items = syntax_core::keywords(language)
            .into_iter()
            .map(|keyword| fallback_item(keyword, Some(14)))
            .chain(words.into_iter().map(|word| fallback_item(word, None)))
            .collect();
        *self.completions.borrow_mut() = lsp_core::CompletionList {
            items,
            is_incomplete: false,
        };
        self.as_mut().completion_ready();
    }

    pub fn cancel_completion(self: Pin<&mut Self>) {
        self.completion.borrow_mut().cancel();
        *self.completions.borrow_mut() = lsp_core::CompletionList::default();
    }

    pub fn completion_items(&self, text_before_cursor: &QString) -> Vec<ffi::FfiCompletionItem> {
        let text_before_cursor = text_before_cursor.to_string();
        let prefix = lsp_core::completion_prefix(&text_before_cursor);
        if !self.completion.borrow().still_typing(prefix) {
            return Vec::new();
        }
        let prefix_length = prefix.encode_utf16().count() as u32;
        lsp_core::filter_completions(&self.completions.borrow().items, prefix)
            .into_iter()
            .map(|m| to_ffi_completion(m, prefix_length))
            .collect()
    }

    /// Accept `item`: the splice that types it, plus — when the server
    /// supports `completionItem/resolve` (C7) — whatever
    /// `additionalTextEdits` resolving it adds, merged into the same
    /// application so one Ctrl+Z undoes both.
    ///
    /// Answers on `completionEditReady`, never synchronously: the server may
    /// have to be asked, and `push_job` only ever answers off the Qt thread.
    /// A server with nothing to resolve, or one that times out
    /// ([`lsp_core::DEFAULT_REQUEST_TIMEOUT`]), still gets its answer — the
    /// item's own edit alone — because an accept must never be left half
    /// finished.
    pub fn accept_completion(
        mut self: Pin<&mut Self>,
        item: &ffi::FfiCompletionItem,
        caret_line: u32,
        caret_character: u32,
    ) {
        let range = item.has_range.then_some(lsp_core::TextRange {
            start_line: item.start_line,
            start_character: item.start_character,
            end_line: item.end_line,
            end_character: item.end_character,
        });
        let span = lsp_core::completion_accept_range(
            range,
            item.prefix_length,
            caret_line,
            caret_character,
        );

        // R2: a snippet item's `insert` is the server's raw grammar
        // (`is_snippet` says so — `lsp_core::completion::CompletionItem`'s
        // own doc comment). `edit_ops::snippet::parse` turns it into the
        // text to splice plus its tab stops; `snippet_ready` hands the
        // stops to the view in the same units they were found in (char
        // offsets into the flattened text), and the view adds the
        // insertion point once the edit has actually landed.
        //
        // Scoped down deliberately: a snippet accept skips the C7
        // resolve/`additionalTextEdits` merge below. Combining "the
        // accepted item resolves to more edits elsewhere in the file" with
        // "the accepted item is also a multi-stop snippet" is a rare
        // intersection, and skipping it keeps this the one splice whose
        // position `snippet_ready` can describe unambiguously.
        if item.is_snippet {
            let parsed = edit_ops::snippet::parse(&item.insert.to_string());
            let own_edit = lsp_core::completion_own_edit(span, &parsed.text);
            let stops: Vec<ffi::FfiSnippetStop> = edit_ops::snippet::stops(&parsed)
                .into_iter()
                .map(|range| ffi::FfiSnippetStop {
                    has_stop: true,
                    start: range.start as u32,
                    end: range.end as u32,
                    more: false,
                })
                .collect();
            self.as_mut().emit_completion_edit(vec![own_edit]);
            if !stops.is_empty() {
                self.as_mut()
                    .snippet_ready(span.start_line, span.start_character, stops);
            }
            return;
        }

        let own_edit = lsp_core::completion_own_edit(span, &item.insert.to_string());

        let language_id = self.completion_language.borrow().clone();
        let resolvable = language_id.as_deref().is_some_and(|lang| {
            self.completion_resolve_supported
                .borrow()
                .get(lang)
                .copied()
                .unwrap_or(false)
        });
        let raw: Option<serde_json::Value> = resolvable
            .then(|| serde_json::from_str(&item.resolve_data.to_string()).ok())
            .flatten();

        let Some((language_id, raw)) = language_id.zip(raw) else {
            self.as_mut().emit_completion_edit(vec![own_edit]);
            return;
        };

        let fallback = own_edit.clone();
        let qt_thread = self.as_mut().qt_thread();
        let queued = self.push_job(move |manager| {
            let additional = manager
                .resolve_completion_item(&language_id, &raw)
                .map(|resolved| lsp_core::completion_additional_edits(&resolved))
                .unwrap_or_default();
            let mut edits = additional;
            edits.push(own_edit);
            let edits = lsp_core::descending(edits);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                service.as_mut().emit_completion_edit(edits);
            });
        });
        if !queued {
            self.as_mut().emit_completion_edit(vec![fallback]);
        }
    }

    /// [`lsp_core::TextEdit`]s, translated to the FFI shape and sent on
    /// `completionEditReady` — the one place that signal is emitted, so
    /// every path out of `accept_completion` produces exactly one answer.
    fn emit_completion_edit(mut self: Pin<&mut Self>, edits: Vec<lsp_core::TextEdit>) {
        let edits = edits
            .into_iter()
            .map(|edit| ffi::FfiTextEdit {
                path: QString::default(),
                in_buffer: true,
                start_line: edit.start_line,
                start_character: edit.start_character,
                end_line: edit.end_line,
                end_character: edit.end_character,
                new_text: QString::from(edit.new_text.as_str()),
            })
            .collect();
        self.as_mut().completion_edit_ready(edits);
    }

    /// C7 — the popup's selection moved to `resolve_data`'s item: ask the
    /// server to fill in documentation and detail, when it offers
    /// `completionItem/resolve` at all. A superseded or too-late answer
    /// produces no signal, `lsp_core::CompletionResolveTracker`'s rule —
    /// the same shape `hover_at` uses for the pointer.
    pub fn resolve_completion_preview(mut self: Pin<&mut Self>, resolve_data: &QString) {
        let language_id = self.completion_language.borrow().clone();
        let resolvable = language_id.as_deref().is_some_and(|lang| {
            self.completion_resolve_supported
                .borrow()
                .get(lang)
                .copied()
                .unwrap_or(false)
        });
        let raw: Option<serde_json::Value> = resolvable
            .then(|| serde_json::from_str(&resolve_data.to_string()).ok())
            .flatten();
        let Some((language_id, raw)) = language_id.zip(raw) else {
            return;
        };

        let token = self.completion_resolve.borrow_mut().begin();
        let qt_thread = self.as_mut().qt_thread();
        self.push_job(move |manager| {
            let Ok(resolved) = manager.resolve_completion_item(&language_id, &raw) else {
                return;
            };
            // Reuses `parse_completion`'s own field extraction rather than a
            // second reader for the same `CompletionItem` shape — a resolved
            // item answers with the same fields a list entry does.
            let Some(item) = lsp_core::parse_completion(&serde_json::json!([resolved]))
                .items
                .into_iter()
                .next()
            else {
                return;
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if !service.completion_resolve.borrow().accept(token) {
                    return;
                }
                let documentation_html = render_doc_html(&item.documentation);
                service.as_mut().completion_preview_ready(
                    QString::from(item.detail.as_str()),
                    QString::from(documentation_html.as_str()),
                );
            });
        });
    }

    pub fn cancel_completion_preview(self: Pin<&mut Self>) {
        self.completion_resolve.borrow_mut().cancel();
    }

    /// The enabled server for this path's language, if the catalog plus the
    /// user's settings name one. *Which* language the file is comes from
    /// `syntax-core`'s registry — the single source of file detection — and
    /// `lsp-core` answers only what the protocol calls it and what to launch
    /// (ADR-0018).
    fn config_for_path(&self, path: &str) -> Option<lsp_core::ServerConfig> {
        let language_id = syntax_core::language_for_path(Path::new(path)).id();
        lsp_core::enabled_server(
            &self.configs.borrow(),
            lsp_core::lsp_language_id(&language_id),
        )
        .cloned()
    }

    /// Queue work for the worker thread. Returns false when there is no
    /// worker (no project open yet), which callers that must answer either
    /// way have to handle.
    pub(crate) fn push_job(
        &self,
        job: impl FnOnce(&lsp_core::LspManager) + Send + 'static,
    ) -> bool {
        match self.jobs.borrow().as_ref() {
            Some(jobs) => jobs.send(Box::new(job)).is_ok(),
            None => false,
        }
    }

    /// Queue the (blocking) launch of one server and report its outcome.
    /// A launch that fails frees the language again, so opening another file
    /// of it retries rather than staying silently dead for the session.
    fn start_server(mut self: Pin<&mut Self>, config: lsp_core::ServerConfig) {
        let language_id = config.language_id.clone();
        let name = config.name.clone();
        let qt_thread = self.as_mut().qt_thread();
        self.as_mut().server_state_changed(
            QString::from(language_id.as_str()),
            QString::from(name.as_str()),
            ffi::FfiServerState::Starting,
            QString::default(),
            0,
        );
        self.push_job(move |manager| {
            if let Err(err) = manager.start(&config) {
                let message = err.to_string();
                let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                    service.started.borrow_mut().remove(&language_id);
                    service.as_mut().server_state_changed(
                        QString::from(language_id.as_str()),
                        QString::from(name.as_str()),
                        ffi::FfiServerState::Failed,
                        QString::from(message.as_str()),
                        0,
                    );
                });
            }
        });
    }

    /// The listener thread's one hop onto the Qt thread: an `LspEvent` becomes
    /// either a store update or a status signal, and nothing else.
    fn apply_event(mut self: Pin<&mut Self>, event: lsp_core::LspEvent) {
        let name_of = |language_id: &str| {
            self.configs
                .borrow()
                .iter()
                .find(|c| c.language_id == language_id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| language_id.to_string())
        };
        match event {
            lsp_core::LspEvent::Diagnostics {
                language_id,
                uri,
                diagnostics,
                ..
            } => {
                self.store.borrow_mut().replace(
                    &lsp_source_key(&language_id),
                    &uri,
                    lsp_core::to_diagnostics(diagnostics),
                );
                self.as_mut().diagnostics_changed();
            }
            lsp_core::LspEvent::ServerReady {
                language_id,
                trigger_characters,
                signature_triggers,
                completion_resolve_supported,
                ..
            } => {
                let name = name_of(&language_id);
                self.triggers
                    .borrow_mut()
                    .insert(language_id.clone(), trigger_characters);
                self.signature_triggers
                    .borrow_mut()
                    .insert(language_id.clone(), signature_triggers);
                self.completion_resolve_supported
                    .borrow_mut()
                    .insert(language_id.clone(), completion_resolve_supported);
                self.as_mut().server_state_changed(
                    QString::from(language_id.as_str()),
                    QString::from(name.as_str()),
                    ffi::FfiServerState::Ready,
                    QString::default(),
                    0,
                );
            }
            lsp_core::LspEvent::ServerExited {
                language_id,
                retry_in,
                ..
            } => {
                let name = name_of(&language_id);
                self.as_mut().server_state_changed(
                    QString::from(language_id.as_str()),
                    QString::from(name.as_str()),
                    ffi::FfiServerState::Exited,
                    QString::default(),
                    retry_in.as_millis().min(u128::from(u32::MAX)) as u32,
                );
            }
            lsp_core::LspEvent::ServerFailed {
                language_id,
                message,
            } => {
                let name = name_of(&language_id);
                self.as_mut().server_state_changed(
                    QString::from(language_id.as_str()),
                    QString::from(name.as_str()),
                    ffi::FfiServerState::Failed,
                    QString::from(message.as_str()),
                    0,
                );
            }
            // RF8: a server applying the edit its command computed — how
            // jdtls, omnisharp and intelephense deliver an Extract. It is
            // blocked until the gate is answered, so every path out of
            // `PendingRefactor` answers it.
            lsp_core::LspEvent::ApplyEdit {
                label, edit, gate, ..
            } => {
                let changes = match lsp_core::parse_workspace_changes(&edit) {
                    Ok(mut changes) => {
                        // ADR-0052: this handler runs on the event-listener
                        // thread, not inside a `push_job` closure, so there
                        // is no `&LspManager` to call the retranslating
                        // method through — `self.host` is the same
                        // `ExecHost` the manager derived, kept for exactly
                        // this site.
                        changes.retranslate_paths(&self.host.borrow());
                        changes
                    }
                    Err(e) => {
                        gate.refuse(e.to_string());
                        self.as_mut()
                            .refactor_failed(QString::from(e.to_string().as_str()));
                        return;
                    }
                };
                let open_paths = self.open_document_paths();
                // The server chose the files, so there is no "current" one
                // to compare against: a server-driven edit always shows its
                // preview.
                let planned = lsp_core::plan_changes(changes, &open_paths, "", &|_| None);
                match planned {
                    Ok(plan) => {
                        self.publish_refactor(
                            label.unwrap_or_else(|| "Refactoring".to_string()),
                            plan,
                            Some(gate),
                        );
                    }
                    Err(e) => {
                        gate.refuse(e.to_string());
                        self.as_mut()
                            .refactor_failed(QString::from(e.to_string().as_str()));
                    }
                }
            }
            // F0-16: ready is not the same as able to answer. What the
            // server is doing is its own words; picking which busy server to
            // report is the map's ordering, and how to word it is the view's.
            lsp_core::LspEvent::ServerBusy {
                language_id,
                activity,
            } => {
                let name = name_of(&language_id);
                {
                    let mut busy = self.busy.borrow_mut();
                    match activity {
                        Some(activity) => busy.insert(language_id, (name, activity)),
                        None => busy.remove(&language_id),
                    };
                }
                let first = self
                    .busy
                    .borrow()
                    .values()
                    .next()
                    .map(|(name, activity)| (name.clone(), activity.clone()));
                match first {
                    Some((name, activity)) => self.as_mut().server_busy_changed(
                        true,
                        QString::from(name.as_str()),
                        QString::from(activity.title.as_str()),
                        activity.percentage.is_some(),
                        activity.percentage.unwrap_or(0),
                    ),
                    None => self.as_mut().server_busy_changed(
                        false,
                        QString::default(),
                        QString::default(),
                        false,
                        0,
                    ),
                }
            }
            lsp_core::LspEvent::Notification { .. } => {}
        }
    }
}

#[cfg(test)]
mod hover_popup_tests {
    use super::{compose_hover_html, html_escape};
    use diagnostics_core::{Diagnostic, DiagnosticStore, Position, Range, Severity};

    fn diagnostic_at(line: u32, character: u32, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: Position { line, character },
                end: None,
            },
            severity,
            message: message.to_string(),
            source: "rustc".to_string(),
            raw: None,
        }
    }

    #[test]
    fn hover_alone_is_shown_with_no_diagnostics_section() {
        assert_eq!(
            compose_hover_html(Some("<b>fn main()</b>"), &[]),
            "<b>fn main()</b>"
        );
    }

    #[test]
    fn diagnostics_are_appended_after_the_hover_separated_by_a_rule() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///a.rs",
            vec![diagnostic_at(0, 0, Severity::Error, "mismatched types")],
        );
        let rows = store.at("file:///a.rs", 0, 0);
        assert_eq!(
            compose_hover_html(Some("<b>fn main()</b>"), &rows),
            "<b>fn main()</b><hr><b>Error (rustc):</b> mismatched types"
        );
    }

    #[test]
    fn no_hover_shows_only_the_diagnostics() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///a.rs",
            vec![diagnostic_at(0, 0, Severity::Warning, "unused import")],
        );
        let rows = store.at("file:///a.rs", 0, 0);
        assert_eq!(
            compose_hover_html(None, &rows),
            "<b>Warning (rustc):</b> unused import"
        );
    }

    #[test]
    fn a_diagnostics_message_is_html_escaped() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///a.rs",
            vec![diagnostic_at(
                0,
                0,
                Severity::Error,
                "expected `T<U>` & got `V`",
            )],
        );
        let rows = store.at("file:///a.rs", 0, 0);
        assert_eq!(
            compose_hover_html(None, &rows),
            "<b>Error (rustc):</b> expected `T&lt;U&gt;` &amp; got `V`"
        );
    }

    #[test]
    fn escape_covers_the_three_reinterpretable_characters() {
        assert_eq!(html_escape("<a & b>"), "&lt;a &amp; b&gt;");
    }
}
