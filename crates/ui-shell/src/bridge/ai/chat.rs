use core::pin::Pin;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use app_core::AppSession;
use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::ai::agent::{
    assistant_turns, attachment_kind, definition_text, recover, run_agent, run_ask, severity_word,
    to_ffi_tool_call, ApprovalGate,
};
use crate::bridge::convert::{load_settings, symbol_kind_word, to_ffi_edits};
use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};
use crate::bridge::registry::{index_slot, shared_session, SharedDiagnostics};
use ai_chat_core::agent::Decision;
use ai_chat_core::context::{self, Attachment, DiagnosticNote};
use ai_chat_core::conversation::{Block, Conversation};
use ai_chat_core::history::{ConversationRecord, HistoryStore};
use ai_chat_core::models::{self, ModelInfo};
use ai_chat_core::proposal::{self, ApplyRefusal, ApplyTarget, CodeBlock};
use ai_chat_core::providers::{ProviderConfig, ProviderKind};
use ai_chat_core::tokens::TokenCounter;
use ai_chat_core::tools::{ToolCall, ToolPolicy};
use ai_chat_core::ChatError;

/// A `ChatError` as the typed result the seam carries (ADR-0003).
pub(crate) fn to_chat_result(error: ChatError) -> FfiResult {
    FfiResult {
        code: error.code(),
        message: QString::from(error.to_string().as_str()),
    }
}

/// `settings-model` and `ai-chat-core` spell the compatible kind with an
/// underscore and a hyphen respectively — two vocabularies that ADR-0017
/// deliberately keeps apart, so translating between them is exactly this
/// layer's job. An unknown string stays a `ChatError::UnknownProvider`,
/// which is what the settings page already shows for one.
pub(crate) fn to_core_kind(settings_kind: &str) -> Result<ProviderKind, ChatError> {
    ProviderKind::from_str(settings_kind)
}

/// The provider the chat sends to, as `ai-chat-core` wants it.
///
/// Nothing is chosen here: an unset or disabled active provider is
/// `NoProviderConfigured`, whose own sentence tells the user to pick one.
/// Guessing "the first enabled row" would be this layer deciding which third
/// party the user's source code goes to.
pub(crate) fn active_provider(
    settings: &app_config::Settings,
) -> Result<ProviderConfig, ChatError> {
    let draft = settings_model::ai::AiProviderDraft::begin(settings);
    let active = draft.active_provider().to_string();
    let row = draft
        .rows()
        .iter()
        .find(|row| row.id == active && row.enabled)
        .ok_or(ChatError::NoProviderConfigured)?;
    Ok(ProviderConfig {
        // The label, not the id: `ProviderConfig::label` is what every error
        // sentence names, and "Anthropic" reads better than "anthropic".
        id: row.label.clone(),
        kind: to_core_kind(&row.kind)?,
        base_url: row.base_url.clone(),
        model: row.model.clone(),
        api_key_env: row.api_key_env.clone(),
        enabled: true,
    })
}

/// Seconds since the epoch, for the ids and timestamps `history` takes from
/// its caller because it reads no clock itself.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// The `ApplyRefusal` variants as codes the panel can branch on. Their own
/// space, not `ChatError`'s: the two are read at different moments and never
/// travel the same signal (see `applyRefusal`'s declaration).
pub(crate) fn apply_refusal_code(refusal: &ApplyRefusal) -> i32 {
    match refusal {
        ApplyRefusal::NoCodeBlock => 1,
        ApplyRefusal::NoTarget => 2,
        ApplyRefusal::TargetNotOpen(_) => 3,
        ApplyRefusal::OutsideProject(_) => 4,
        ApplyRefusal::Unchanged => 5,
    }
}

/// What the Qt thread keeps hold of while one request or run is in flight.
pub(crate) struct ActiveRun {
    /// Read by `transport::stream_chat` between SSE events and by the agent
    /// loop between steps.
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) gate: std::sync::Arc<ApprovalGate>,
    /// Tools promoted to `Auto` by "always allow" during this run. Per run,
    /// never persisted: a promotion the user made for one task must not
    /// silently widen the agent's authority tomorrow.
    pub(crate) promoted: std::sync::Arc<std::sync::Mutex<HashMap<String, ToolPolicy>>>,
    /// True for a run driven by `agent::run`, so the end of it reports
    /// through `runFinished` rather than `chatFailed`.
    pub(crate) agent_mode: bool,
}

/// The apply waiting for the preview's verdict — the same shape
/// `PendingRefactor` has, minus the `workspace/applyEdit` gate a model's
/// answer never has anything to settle with.
pub(crate) struct PendingApply {
    pub(crate) plan: lsp_core::EditPlan,
    pub(crate) excluded: Vec<String>,
}

/// Rust side of the `AiChat` QObject.
///
/// Everything here is either state the panel reads back or a handle to
/// something that decides elsewhere. The transcript is `ai-chat-core`'s
/// `Conversation`, the attachments are its `Attachment`s, the token counter
/// is its `TokenCounter`, and the store is its `HistoryStore`.
pub struct AiChatRust {
    pub(crate) session: Rc<RefCell<AppSession>>,
    /// The same index `SearchModel` builds and the MCP server queries, so
    /// an in-IDE agent can never see a different project than an attached
    /// one (ADR-0021 §1).
    pub(crate) index: mcp_server::IndexHandle,
    pub(crate) diagnostics: SharedDiagnostics,
    /// The Qt thread's copy of the transcript. During a run the worker owns
    /// the authoritative one and this mirrors it event by event, so the
    /// panel can render mid-stream; the worker hands the real one back when
    /// the run ends, and it replaces this wholesale.
    pub(crate) conversation: RefCell<Conversation>,
    /// The pending context for the *next* message — deliberately not part
    /// of the transcript (see `ai_chat_core::conversation`'s module docs).
    pub(crate) attachments: RefCell<Vec<Attachment>>,
    pub(crate) counter: RefCell<TokenCounter>,
    /// What the user has typed and not sent, so the live counter charges
    /// for it.
    pub(crate) composer: RefCell<String>,
    pub(crate) agent_mode: std::cell::Cell<bool>,
    pub(crate) run: RefCell<Option<ActiveRun>>,
    /// The card on screen, so `pendingToolCall` can answer without the
    /// panel having to remember what the signal carried.
    pub(crate) pending_call: RefCell<Option<ToolCall>>,
    /// Assistant turns already in the transcript when the run started —
    /// `runStepCount` is the difference, which is one per round trip.
    pub(crate) run_baseline: std::cell::Cell<usize>,
    /// What the provider said it charged, as `StreamEvent::Usage` reported
    /// it. Ask mode only: `agent::run` has no usage callback, so an agent
    /// run leaves these at their last value.
    pub(crate) usage: std::cell::Cell<(u32, u32)>,
    pub(crate) history: HistoryStore,
    /// The record this transcript is saved as, once it has been saved.
    pub(crate) conversation_id: RefCell<Option<String>>,
    /// Distinguishes conversations started within the same second;
    /// `history::new_id` takes it because that module reads no clock.
    pub(crate) id_counter: std::cell::Cell<u64>,
    pub(crate) persist: std::cell::Cell<bool>,
    pub(crate) pending_apply: RefCell<Option<PendingApply>>,
    pub(crate) apply_refusal: RefCell<Option<ApplyRefusal>>,
    /// RF2's staleness rule, the same gate a rename goes through.
    pub(crate) edits: RefCell<lsp_core::EditGate>,
    /// The active provider, resolved from `settings.toml` once and kept
    /// until something invalidates it. The live token counter runs on the
    /// keystroke path, and re-parsing the settings file per character typed
    /// is the difference between a live counter and a stuttering one.
    pub(crate) provider: RefCell<Option<ProviderConfig>>,
    /// The last model catalogue fetched for the active provider, and the
    /// sentence describing that fetch. `None` means nothing has been asked
    /// for yet, which is what an unopened dropdown looks like.
    ///
    /// ponytail: per-process cache with no TTL, invalidated only by a
    /// provider or settings change; add expiry if catalogues start moving
    /// mid-session.
    ///
    /// ponytail: staged ahead of the FFI accessor/QML dropdown that reads
    /// it; allowed dead for now, wire up (or delete) in that follow-up.
    #[allow(dead_code)]
    pub(crate) models: RefCell<Option<Result<Vec<ModelInfo>, ChatError>>>,
    /// True while a catalogue fetch is in flight, so opening the dropdown
    /// twice does not start two requests.
    #[allow(dead_code)]
    pub(crate) models_loading: std::cell::Cell<bool>,
}

impl Default for AiChatRust {
    fn default() -> Self {
        let settings = load_settings();
        AiChatRust {
            session: shared_session(),
            index: index_slot(),
            diagnostics: SharedDiagnostics::default(),
            conversation: RefCell::default(),
            attachments: RefCell::default(),
            counter: RefCell::default(),
            composer: RefCell::default(),
            agent_mode: std::cell::Cell::new(settings.ai_mode == "agent"),
            run: RefCell::default(),
            pending_call: RefCell::default(),
            run_baseline: std::cell::Cell::default(),
            usage: std::cell::Cell::default(),
            history: HistoryStore::new(&app_core::resolve_config_dir()),
            conversation_id: RefCell::default(),
            id_counter: std::cell::Cell::default(),
            persist: std::cell::Cell::new(settings.ai_persist_conversations_or_default()),
            pending_apply: RefCell::default(),
            apply_refusal: RefCell::default(),
            edits: RefCell::default(),
            provider: RefCell::default(),
            models: RefCell::default(),
            models_loading: std::cell::Cell::default(),
        }
    }
}

impl ffi::AiChat {
    /// The active provider, from the cache when it is warm.
    ///
    /// Invalidated by [`Self::set_active_provider`] and
    /// [`Self::apply_ai_settings`], which are the only two ways the answer
    /// can change while the panel is open — the settings dialog routes
    /// through the second.
    fn provider(&self) -> Result<ProviderConfig, ChatError> {
        // Cloned out of the cell before the match, so refreshing the cache
        // in the miss arm cannot collide with the borrow that read it.
        let cached = self.provider.borrow().clone();
        let mut config = match cached {
            Some(config) => config,
            None => {
                let resolved = active_provider(&load_settings())?;
                *self.provider.borrow_mut() = Some(resolved.clone());
                resolved
            }
        };
        // The conversation's model wins over the provider's default. This
        // is the one place a request's `ProviderConfig` is built, so the
        // override reaches sending, token counting and the Gemini
        // path-embedded model without a second application.
        if let Some(model) = self.conversation.borrow().model() {
            config.model = model.to_string();
        }
        Ok(config)
    }

    // --- sending ---------------------------------------------------------

    pub fn send_message(mut self: Pin<&mut Self>, text: &QString) -> FfiResult {
        // The panel disables the composer while a run is in flight; this is
        // the belt to that pair of braces, and it must not start a second
        // worker against the same transcript.
        if self.run.borrow().is_some() {
            return FfiResult::default();
        }
        let settings = load_settings();
        let config = match self.provider() {
            Ok(config) => config,
            Err(error) => return to_chat_result(error),
        };
        let api_key = match ai_chat_core::providers::resolve_api_key(&config) {
            Ok(key) => key,
            Err(error) => return to_chat_result(error),
        };
        let agent_mode = self.agent_mode.get();
        if agent_mode && !config.capabilities().tools {
            return to_chat_result(ChatError::UnsupportedCapability {
                provider: config.label().to_string(),
                capability: ai_chat_core::providers::Capability::Tools,
            });
        }

        let root = self.session.borrow().root_path().map(Path::to_path_buf);
        let typed = text.to_string();
        let blocks = self.as_mut().compose_user_turn(&config, typed);
        if blocks.is_empty() {
            // Every dialect rejects a message with no content, so an empty
            // composer with nothing attached is a no-op rather than a 400.
            return FfiResult::default();
        }
        self.conversation.borrow_mut().push_user_blocks(blocks);
        let index = self.conversation.borrow().len() as u64 - 1;
        self.as_mut().message_appended(index);
        self.attachments.borrow_mut().clear();
        self.as_mut().attachments_changed();

        let conversation = self.conversation.borrow().clone();
        self.run_baseline.set(assistant_turns(&conversation));

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = std::sync::Arc::new(ApprovalGate::default());
        let promoted = std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));
        *self.run.borrow_mut() = Some(ActiveRun {
            cancel: std::sync::Arc::clone(&cancel),
            gate: std::sync::Arc::clone(&gate),
            promoted: std::sync::Arc::clone(&promoted),
            agent_mode,
        });

        let system = context::system_prompt(agent_mode, root.as_deref());
        let policies = self.tool_policy_snapshot(&settings);
        let qt_thread = self.as_mut().qt_thread();

        // One thread owns the blocking HTTP and marshals everything back
        // with `queue` — the PTY reader's pattern (ADR-0021 §4). The Qt
        // thread returns from this call immediately and never waits on it.
        std::thread::spawn(move || {
            let mut conversation = conversation;
            let outcome = if agent_mode {
                run_agent(
                    &qt_thread,
                    &config,
                    &api_key,
                    &mut conversation,
                    &system,
                    policies,
                    promoted,
                    &cancel,
                    &gate,
                    root,
                )
            } else {
                run_ask(
                    &qt_thread,
                    &config,
                    &api_key,
                    &mut conversation,
                    &system,
                    &cancel,
                )
            };
            let (code, message) = outcome;
            let _ = qt_thread.queue(move |chat: Pin<&mut Self>| {
                chat.finish_run(conversation, code, message);
            });
        });

        FfiResult::default()
    }

    /// The blocks the user's turn carries: the rendered attachments, what
    /// they typed, and any images `render_context` set aside.
    ///
    /// Order is context first: a model reads the question last and answers
    /// about what it just read.
    fn compose_user_turn(
        self: Pin<&mut Self>,
        config: &ProviderConfig,
        typed: String,
    ) -> Vec<Block> {
        let attachments = self.attachments.borrow();
        let mut counter = self.counter.borrow_mut();
        let budget = self.context_budget(config, &mut counter);
        let rendered = context::render_context(config, &mut counter, &attachments, budget);

        let mut blocks = Vec::new();
        if !rendered.text.trim().is_empty() {
            blocks.push(Block::Text(rendered.text));
        }
        if !typed.trim().is_empty() {
            blocks.push(Block::Text(typed));
        }
        for image in rendered.images {
            if let Attachment::Image {
                media_type,
                data_base64,
                ..
            } = image
            {
                blocks.push(Block::Image {
                    media_type,
                    data_base64,
                });
            }
        }
        blocks
    }

    /// What the attachments are allowed to spend: the model's window, less
    /// the room the answer needs and what the transcript already costs.
    ///
    /// Arithmetic over three numbers `ai-chat-core` owns, not a policy of
    /// this layer's own — the truncation *order* within that budget is
    /// `render_context`'s, and it is the part that decides anything.
    fn context_budget(&self, config: &ProviderConfig, counter: &mut TokenCounter) -> u32 {
        let spent = counter
            .count_conversation(config, &self.conversation.borrow())
            .value();
        ai_chat_core::tokens::context_window(config)
            .saturating_sub(ai_chat_core::request::DEFAULT_MAX_TOKENS)
            .saturating_sub(spent)
    }

    /// Every tool's policy as it stands right now, so the worker never
    /// touches `settings.toml`. The resolution is
    /// `settings_model::ai::tool_policy`'s; an unclassified name falls to
    /// `tools::default_policy`, which never returns `Auto` for one.
    fn tool_policy_snapshot(&self, settings: &app_config::Settings) -> HashMap<String, ToolPolicy> {
        settings_model::ai::known_tools()
            .filter_map(|tool| {
                let policy = settings_model::ai::tool_policy(settings, tool);
                ToolPolicy::parse(policy.as_str()).map(|policy| (tool.to_string(), policy))
            })
            .collect()
    }

    pub fn cancel_request(self: Pin<&mut Self>) {
        self.stop_run();
    }

    pub fn stop_run(self: Pin<&mut Self>) {
        let Some(run) = self.run.borrow().as_ref().map(|run| {
            (
                std::sync::Arc::clone(&run.cancel),
                std::sync::Arc::clone(&run.gate),
            )
        }) else {
            return;
        };
        run.0.store(true, std::sync::atomic::Ordering::SeqCst);
        // Unparks a worker sitting on an approval card. Without this, a
        // user who closes the panel mid-approval leaves the thread waiting
        // for a click that can no longer happen.
        run.1.abandon();
    }

    pub fn is_streaming(&self) -> bool {
        self.run.borrow().is_some()
    }

    pub fn new_conversation(mut self: Pin<&mut Self>) {
        self.as_mut().stop_run();
        self.conversation.borrow_mut().clear();
        self.attachments.borrow_mut().clear();
        self.composer.borrow_mut().clear();
        *self.conversation_id.borrow_mut() = None;
        *self.pending_call.borrow_mut() = None;
        self.usage.set((0, 0));
        self.as_mut().attachments_changed();
        self.as_mut().token_usage_changed();
        // `clear` dropped the model override with the transcript.
        self.as_mut().models_changed();
    }

    pub fn set_mode(mut self: Pin<&mut Self>, mode: &QString) -> FfiResult {
        let agent_mode = mode.to_string() == "agent";
        if agent_mode {
            // Declared, not discovered: a provider with no tool support is
            // refused here rather than by a request that comes back 400.
            match self.provider() {
                Ok(config) if !config.capabilities().tools => {
                    return to_chat_result(ChatError::UnsupportedCapability {
                        provider: config.label().to_string(),
                        capability: ai_chat_core::providers::Capability::Tools,
                    })
                }
                Ok(_) => {}
                Err(error) => return to_chat_result(error),
            }
        }
        self.agent_mode.set(agent_mode);
        let mode = if agent_mode { "agent" } else { "ask" }.to_string();
        let _ = app_config::update(&app_core::resolve_config_dir(), |settings| {
            settings.ai_mode = mode;
        });
        self.as_mut().token_usage_changed();
        FfiResult::default()
    }

    pub fn mode(&self) -> QString {
        QString::from(if self.agent_mode.get() {
            "agent"
        } else {
            "ask"
        })
    }

    pub fn set_composer_text(mut self: Pin<&mut Self>, text: &QString) {
        let text = text.to_string();
        if *self.composer.borrow() == text {
            return;
        }
        *self.composer.borrow_mut() = text;
        self.as_mut().token_usage_changed();
    }
}

impl ffi::AiChat {
    // --- attachments ------------------------------------------------------

    /// The one gate every attachment passes: a credentials-shaped name, a
    /// path outside the open project, and an image a provider cannot read
    /// are all refused here, in `ai-chat-core`'s words (ADR-0021 §1). No
    /// `attach_*` slot may push around it.
    fn accept(mut self: Pin<&mut Self>, attachment: Attachment) -> FfiResult {
        let config = match self.provider() {
            Ok(config) => config,
            Err(error) => return to_chat_result(error),
        };
        let root = self.session.borrow().root_path().map(Path::to_path_buf);
        if let Err(error) = context::accept_attachment(&config, root.as_deref(), &attachment) {
            return to_chat_result(error);
        }
        self.attachments.borrow_mut().push(attachment);
        self.as_mut().attachments_changed();
        self.as_mut().token_usage_changed();
        FfiResult::default()
    }

    pub fn attach_selection(
        self: Pin<&mut Self>,
        path: &QString,
        start_line: u32,
        end_line: u32,
        text: &QString,
    ) -> FfiResult {
        self.accept(Attachment::Selection {
            path: std::path::PathBuf::from(path.to_string()),
            start_line,
            end_line,
            text: text.to_string(),
        })
    }

    pub fn attach_file(self: Pin<&mut Self>, path: &QString) -> FfiResult {
        let path = std::path::PathBuf::from(path.to_string());
        // The open buffer wins over the file: attaching what is on screen,
        // unsaved edits included, is what the user means by "this file".
        let text = match self.session.borrow().content_for_path(&path) {
            Some(content) => Ok(content),
            None => std::fs::read_to_string(&path),
        };
        match text {
            Ok(text) => self.accept(Attachment::File { path, text }),
            Err(error) => FfiResult {
                code: errors::CODE_ATTACHMENT_IO,
                message: QString::from(error.to_string().as_str()),
            },
        }
    }

    pub fn attach_folder(mut self: Pin<&mut Self>, path: &QString) -> FfiResult {
        let folder = std::path::PathBuf::from(path.to_string());
        let config = match self.provider() {
            Ok(config) => config,
            Err(error) => return to_chat_result(error),
        };
        let Some(root) = self
            .session
            .borrow()
            .root_path()
            .map(std::path::Path::to_path_buf)
        else {
            // Without an open project there is no root to confine the walk
            // to, and an unconfined one could read the whole disk.
            return to_chat_result(ChatError::PathOutsideProject(folder));
        };

        let expansion = {
            let resolved = crate::bridge::convert::load_resolved_settings_for(&root);
            let mut counter = self.counter.borrow_mut();
            let budget = self.context_budget(&config, &mut counter);
            match context::expand_folder(
                &config,
                &mut counter,
                &root,
                &folder,
                budget,
                &resolved.excluded,
                &resolved.ignored_names,
            ) {
                Ok(expansion) => expansion,
                Err(error) => return to_chat_result(error),
            }
        };

        // Composed before the attachments are consumed below, and it is
        // the whole user-facing answer: what was attached, what was
        // skipped and why, and what did not fit.
        let summary = expansion.summary();

        // Each file still goes through the same gate a hand-attached one
        // does: `expand_folder` decided what is worth offering, `accept`
        // decides what may be sent, and the second is not skipped because
        // the first already looked.
        for attachment in expansion.attachments {
            let result = self.as_mut().accept(attachment);
            if result.code != 0 {
                return result;
            }
        }

        FfiResult {
            code: errors::CODE_OK,
            message: QString::from(summary.as_str()),
        }
    }

    pub fn attach_image(self: Pin<&mut Self>, path: &QString) -> FfiResult {
        let path = std::path::PathBuf::from(path.to_string());
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return FfiResult {
                    code: errors::CODE_ATTACHMENT_IO,
                    message: QString::from(error.to_string().as_str()),
                }
            }
        };
        match context::load_image(&path, &bytes) {
            Ok(attachment) => self.accept(attachment),
            Err(error) => to_chat_result(error),
        }
    }

    pub fn attach_symbol(self: Pin<&mut Self>, name: &QString) -> FfiResult {
        let name = name.to_string();
        let found = self.query_index(|index| index.find_definitions_ranked(&name, 1));
        let hit = match found {
            Ok(mut hits) if !hits.is_empty() => hits.remove(0),
            Ok(_) => {
                return to_chat_result(ChatError::ToolFailed {
                    tool: "find_definitions".to_string(),
                    detail: format!("nothing in this project defines {name}"),
                })
            }
            Err(detail) => {
                return to_chat_result(ChatError::ToolFailed {
                    tool: "find_definitions".to_string(),
                    detail,
                })
            }
        };
        let content = match self.session.borrow().content_for_path(&hit.path) {
            Some(content) => Ok(content),
            None => std::fs::read_to_string(&hit.path),
        };
        let Ok(content) = content else {
            return FfiResult {
                code: ChatError::CODE_TOOL_FAILED,
                message: QString::from(
                    ChatError::ToolFailed {
                        tool: "find_definitions".to_string(),
                        detail: format!("{} could not be read", hit.path.display()),
                    }
                    .to_string()
                    .as_str(),
                ),
            };
        };
        self.accept(Attachment::Symbol {
            name: hit.name.clone(),
            kind: symbol_kind_word(hit.kind).to_string(),
            path: hit.path.clone(),
            line: hit.line as u32,
            text: definition_text(&hit, &content),
        })
    }

    pub fn attach_diagnostics(self: Pin<&mut Self>) -> FfiResult {
        let notes: Vec<DiagnosticNote> = self
            .diagnostics
            .borrow()
            .rows()
            .into_iter()
            .map(|row| DiagnosticNote {
                path: std::path::PathBuf::from(row.path),
                line: row.line,
                severity: severity_word(row.severity).to_string(),
                message: row.message,
            })
            .collect();
        self.accept(Attachment::Diagnostics(notes))
    }

    pub fn attach_terminal_output(self: Pin<&mut Self>, text: &QString) -> FfiResult {
        self.accept(Attachment::TerminalOutput(text.to_string()))
    }

    pub fn remove_attachment(mut self: Pin<&mut Self>, index: u64) {
        let index = index as usize;
        if index >= self.attachments.borrow().len() {
            return;
        }
        self.attachments.borrow_mut().remove(index);
        self.as_mut().attachments_changed();
        self.as_mut().token_usage_changed();
    }

    pub fn attachments(&self) -> Vec<ffi::FfiAttachment> {
        let Ok(config) = self.provider() else {
            return Vec::new();
        };
        let attachments = self.attachments.borrow();
        let mut counter = self.counter.borrow_mut();
        attachments
            .iter()
            .map(|attachment| {
                // Rendered alone and unbudgeted, so the chip reports what
                // this attachment costs rather than what survived the fit.
                let tokens = context::render_context(
                    &config,
                    &mut counter,
                    std::slice::from_ref(attachment),
                    u32::MAX,
                )
                .tokens
                .value();
                ffi::FfiAttachment {
                    kind: QString::from(attachment_kind(attachment)),
                    label: QString::from(attachment.label().as_str()),
                    detail: QString::from(attachment.detail().as_str()),
                    tokens,
                }
            })
            .collect()
    }

    // --- the transcript ---------------------------------------------------

    pub fn messages(&self) -> Vec<ffi::FfiChatMessage> {
        let conversation = self.conversation.borrow();
        let streaming = conversation.streaming_index();
        conversation
            .turns()
            .iter()
            .enumerate()
            .map(|(index, turn)| {
                let text = turn.text_content();
                ffi::FfiChatMessage {
                    role: QString::from(turn.role.as_str()),
                    text: QString::from(text.as_str()),
                    streaming: streaming == Some(index),
                    // A turn with no prose at all is tool traffic: the model
                    // asking, or the editor answering.
                    kind: QString::from(if text.is_empty() { "tool" } else { "text" }),
                }
            })
            .collect()
    }

    pub fn code_blocks(&self, message_index: u64) -> Vec<ffi::FfiCodeBlock> {
        self.blocks_of(message_index)
            .into_iter()
            .map(|block| ffi::FfiCodeBlock {
                language: QString::from(block.language.as_str()),
                path: QString::from(
                    block
                        .path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default()
                        .as_str(),
                ),
                text: QString::from(block.text.as_str()),
            })
            .collect()
    }

    fn blocks_of(&self, message_index: u64) -> Vec<CodeBlock> {
        let conversation = self.conversation.borrow();
        match conversation.turns().get(message_index as usize) {
            Some(turn) => proposal::extract_code_blocks(&turn.text_content()),
            None => Vec::new(),
        }
    }

    pub fn token_usage(&self) -> ffi::FfiTokenUsage {
        let Ok(config) = self.provider() else {
            return ffi::FfiTokenUsage::default();
        };
        let (input_tokens, output_tokens) = self.usage.get();
        let mut counter = self.counter.borrow_mut();
        let budget = self.context_budget(&config, &mut counter);
        let attachments = self.attachments.borrow();
        let rendered = context::render_context(&config, &mut counter, &attachments, budget);
        let composer = counter.count_text(&config, &self.composer.borrow());
        let transcript = counter.count_conversation(&config, &self.conversation.borrow());
        ffi::FfiTokenUsage {
            context_tokens: rendered.tokens.value() + composer.value() + transcript.value(),
            // Exact only if all three were: one estimate makes the total an
            // estimate, and `Exact` has to mean exact (ADR-0021 §6).
            exact: rendered.tokens.is_exact() && composer.is_exact() && transcript.is_exact(),
            budget: ai_chat_core::tokens::context_window(&config),
            input_tokens,
            output_tokens,
        }
    }

    pub fn run_step_count(&self) -> u32 {
        assistant_turns(&self.conversation.borrow()).saturating_sub(self.run_baseline.get()) as u32
    }

    pub fn pending_tool_call(&self) -> ffi::FfiToolCall {
        match self.pending_call.borrow().as_ref() {
            Some(call) => to_ffi_tool_call(call),
            None => ffi::FfiToolCall::default(),
        }
    }

    pub fn approve_tool(mut self: Pin<&mut Self>, call_id: &QString, remember: bool) -> FfiResult {
        let call_id = call_id.to_string();
        let Some(run) = self.run.borrow().as_ref().map(|run| {
            (
                std::sync::Arc::clone(&run.gate),
                std::sync::Arc::clone(&run.promoted),
            )
        }) else {
            return FfiResult::default();
        };
        if remember {
            if let Some(call) = self.pending_call.borrow().as_ref() {
                // For this run only: a promotion made for one task must not
                // silently widen the agent's authority tomorrow.
                recover(run.1.lock()).insert(call.tool.clone(), ToolPolicy::Auto);
            }
        }
        run.0.answer(&call_id, Decision::Approved);
        *self.pending_call.borrow_mut() = None;
        self.as_mut().token_usage_changed();
        FfiResult::default()
    }

    pub fn deny_tool(mut self: Pin<&mut Self>, call_id: &QString, reason: &QString) -> FfiResult {
        let Some(gate) = self
            .run
            .borrow()
            .as_ref()
            .map(|run| std::sync::Arc::clone(&run.gate))
        else {
            return FfiResult::default();
        };
        // An empty reason is expected and fine: `agent::run` composes the
        // sentence the model is told either way, so the view never writes
        // model-facing wording.
        gate.answer(&call_id.to_string(), Decision::Denied(reason.to_string()));
        *self.pending_call.borrow_mut() = None;
        self.as_mut().token_usage_changed();
        FfiResult::default()
    }
}

impl ffi::AiChat {
    // --- applying an answer, mirroring LanguageService's protocol ----------

    pub fn prepare_apply(
        self: Pin<&mut Self>,
        message_index: u64,
        block_index: u64,
        current_text: &QString,
        buffer_revision: i64,
    ) -> ffi::FfiRefactorSummary {
        *self.apply_refusal.borrow_mut() = None;
        *self.pending_apply.borrow_mut() = None;
        self.edits.borrow_mut().begin(buffer_revision);

        let blocks = self.blocks_of(message_index);
        let Some(block) = blocks.get(block_index as usize) else {
            *self.apply_refusal.borrow_mut() = Some(ApplyRefusal::NoCodeBlock);
            return ffi::FfiRefactorSummary::default();
        };
        let Some(path) = self
            .session
            .borrow()
            .active_tab()
            .and_then(|id| self.session.borrow().tab_path(id))
        else {
            *self.apply_refusal.borrow_mut() = Some(ApplyRefusal::NoTarget);
            return ffi::FfiRefactorSummary::default();
        };

        let current_text = current_text.to_string();
        let target = ApplyTarget {
            path: &path,
            current_text: &current_text,
            // No selection: the panel applies a whole block against the
            // buffer it names, and a selection-scoped apply would need the
            // range in protocol units, which only the editor has.
            selection: None,
        };
        let documents = match proposal::plan_apply(block, &target) {
            Ok(documents) => documents,
            Err(refusal) => {
                *self.apply_refusal.borrow_mut() = Some(refusal);
                return ffi::FfiRefactorSummary::default();
            }
        };

        // The same `plan_edit` a rename goes through, so the model's edit
        // inherits the preview, the single-undo splice and the staleness
        // check unchanged (ADR-0021 §5).
        let open_paths = self.open_document_paths();
        let path_text = path.to_string_lossy().into_owned();
        let plan = match lsp_core::plan_edit(documents, &open_paths, &path_text, &|_| None) {
            Ok(plan) => plan,
            Err(error) => {
                return ffi::FfiRefactorSummary {
                    title: QString::from(error.to_string().as_str()),
                    ..Default::default()
                }
            }
        };
        let summary = ffi::FfiRefactorSummary {
            title: QString::from(format!("Apply to {}", file_name_of(&path)).as_str()),
            document_count: plan.document_count() as u32,
            edit_count: plan.edit_count() as u32,
            op_count: 0,
            touches_other_files: plan.touches_other_files,
        };
        *self.pending_apply.borrow_mut() = Some(PendingApply {
            plan,
            excluded: Vec::new(),
        });
        summary
    }

    pub fn pending_edits(&self) -> Vec<ffi::FfiTextEdit> {
        match self.pending_apply.borrow().as_ref() {
            Some(pending) => to_ffi_edits(&pending.plan, &[]),
            None => Vec::new(),
        }
    }

    pub fn exclude_from_apply(self: Pin<&mut Self>, path: &QString) {
        if let Some(pending) = self.pending_apply.borrow_mut().as_mut() {
            pending.excluded.push(path.to_string());
        }
    }

    pub fn take_pending_edits(self: Pin<&mut Self>, buffer_revision: i64) -> Vec<ffi::FfiTextEdit> {
        let fresh = self.edits.borrow_mut().accept(buffer_revision);
        let Some(pending) = self.pending_apply.borrow_mut().take() else {
            return Vec::new();
        };
        if !fresh {
            // The buffer moved while the user read the answer. Applying it
            // would rewrite the wrong bytes, so it is dropped — the same
            // rule, and the same gate, a rename is held to.
            return Vec::new();
        }
        to_ffi_edits(&pending.plan, &pending.excluded)
    }

    pub fn cancel_apply(self: Pin<&mut Self>) {
        self.edits.borrow_mut().cancel();
        *self.pending_apply.borrow_mut() = None;
    }

    pub fn apply_refusal(&self) -> FfiResult {
        match self.apply_refusal.borrow().as_ref() {
            Some(refusal) => FfiResult {
                code: apply_refusal_code(refusal),
                message: QString::from(refusal.to_string().as_str()),
            },
            None => FfiResult::default(),
        }
    }

    /// The files open in a tab, which is what `lsp_core::plan_edit` splits a
    /// set of document edits against.
    fn open_document_paths(&self) -> Vec<String> {
        let session = self.session.borrow();
        session
            .open_tabs()
            .into_iter()
            .filter_map(|(id, _)| session.tab_path(id))
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    // --- providers --------------------------------------------------------

    pub fn providers(&self) -> Vec<ffi::FfiAiProvider> {
        let settings = load_settings();
        let draft = settings_model::ai::AiProviderDraft::begin(&settings);
        let active = draft.active_provider().to_string();
        draft
            .rows()
            .iter()
            .filter(|row| row.enabled)
            .map(|row| {
                let capabilities = to_core_kind(&row.kind).ok().map(ProviderKind::capabilities);
                ffi::FfiAiProvider {
                    id: QString::from(row.id.as_str()),
                    label: QString::from(row.label.as_str()),
                    model: QString::from(row.model.as_str()),
                    key_present: row.key_status() == settings_model::ai::KeyStatus::Present,
                    active: row.id == active,
                    supports_tools: capabilities.is_some_and(|c| c.tools),
                    supports_images: capabilities.is_some_and(|c| c.images),
                }
            })
            .collect()
    }

    pub fn set_active_provider(mut self: Pin<&mut Self>, id: &QString) -> FfiResult {
        let config_dir = app_core::resolve_config_dir();
        let active = id.to_string();
        let _ = app_config::update(&config_dir, |settings| {
            settings.ai_active_provider = active;
        });
        *self.provider.borrow_mut() = None;
        // A model id from one vendor means nothing to another, so switching
        // provider puts the conversation back on the new provider's default
        // rather than sending it a name it has never heard of.
        self.conversation.borrow_mut().set_model("");
        self.as_mut().forget_models();
        // Agent mode against a provider that cannot use tools is not a mode
        // this build offers, so switching to one drops back to Ask rather
        // than leaving a toggle that would fail on the next send.
        if self.agent_mode.get()
            && !active_provider(&load_settings()).is_ok_and(|c| c.capabilities().tools)
        {
            self.agent_mode.set(false);
        }
        self.as_mut().providers_changed();
        self.as_mut().token_usage_changed();
        FfiResult::default()
    }

    // --- choosing a model -------------------------------------------------

    pub fn models(&self) -> Vec<ffi::FfiAiModel> {
        match self.models.borrow().as_ref() {
            Some(Ok(models)) => models
                .iter()
                .map(|model| ffi::FfiAiModel {
                    id: QString::from(model.id.as_str()),
                    label: QString::from(model.label.as_str()),
                })
                .collect(),
            // A failed fetch lists nothing; `models_status` says why, and
            // the combo stays typeable.
            _ => Vec::new(),
        }
    }

    pub fn models_status(&self) -> QString {
        match self.models.borrow().as_ref() {
            Some(result) => QString::from(models::models_status(result).as_str()),
            None if self.models_loading.get() => QString::from("Asking the provider…"),
            None => QString::from("No models listed yet."),
        }
    }

    pub fn current_model(&self) -> QString {
        match self.provider() {
            Ok(config) => QString::from(config.model.as_str()),
            // No provider configured is a state the picker shows as empty;
            // the panel already reports it where it matters, on send.
            Err(_) => QString::default(),
        }
    }

    pub fn set_model(mut self: Pin<&mut Self>, model: &QString) -> FfiResult {
        self.conversation.borrow_mut().set_model(&model.to_string());
        self.as_mut().save_conversation();
        self.as_mut().models_changed();
        self.as_mut().token_usage_changed();
        FfiResult::default()
    }

    pub fn refresh_models(mut self: Pin<&mut Self>) {
        // Opening the dropdown twice must not start two requests; the
        // second call simply rides the first one's answer.
        if self.models_loading.get() {
            return;
        }
        let config = match self.provider() {
            Ok(config) => config,
            Err(error) => {
                *self.models.borrow_mut() = Some(Err(error));
                self.as_mut().models_changed();
                return;
            }
        };
        self.models_loading.set(true);
        self.as_mut().models_changed();

        let qt_thread = self.as_mut().qt_thread();
        // Blocking HTTP on its own thread, marshalled back with `queue` —
        // the same pattern the stream uses (ADR-0021 §4). The Qt thread
        // must not wait on a provider, least of all to paint a dropdown.
        std::thread::spawn(move || {
            let fetched = models::list_models(&config);
            let _ = qt_thread.queue(move |chat: Pin<&mut Self>| {
                chat.finish_model_fetch(fetched);
            });
        });
    }

    /// Lands a catalogue fetch back on the Qt thread.
    fn finish_model_fetch(mut self: Pin<&mut Self>, fetched: Result<Vec<ModelInfo>, ChatError>) {
        self.models_loading.set(false);
        *self.models.borrow_mut() = Some(fetched);
        self.as_mut().models_changed();
    }

    /// Drops the catalogue, because it belongs to the provider that is no
    /// longer active.
    fn forget_models(mut self: Pin<&mut Self>) {
        *self.models.borrow_mut() = None;
        self.models_loading.set(false);
        self.as_mut().models_changed();
    }

    pub fn apply_ai_settings(mut self: Pin<&mut Self>) {
        *self.provider.borrow_mut() = None;
        self.as_mut().forget_models();
        let settings = load_settings();
        self.persist
            .set(settings.ai_persist_conversations_or_default());
        self.agent_mode.set(settings.ai_mode == "agent");
        self.as_mut().providers_changed();
        self.as_mut().token_usage_changed();
        self.as_mut().conversations_changed();
    }

    // --- history ----------------------------------------------------------

    pub fn conversations(&self) -> Vec<ffi::FfiConversation> {
        let Some(project) = self.session.borrow().root_path().map(Path::to_path_buf) else {
            return Vec::new();
        };
        self.history
            .list(&project)
            .unwrap_or_default()
            .into_iter()
            .map(|summary| ffi::FfiConversation {
                id: QString::from(summary.id.as_str()),
                title: QString::from(summary.title.as_str()),
                updated: QString::from(
                    ai_chat_core::history::format_updated(summary.updated_unix).as_str(),
                ),
                message_count: summary.message_count,
            })
            .collect()
    }

    pub fn load_conversation(mut self: Pin<&mut Self>, id: &QString) -> FfiResult {
        let Some(project) = self.session.borrow().root_path().map(Path::to_path_buf) else {
            return to_chat_result(ChatError::NoProviderConfigured);
        };
        match self.history.load(&project, &id.to_string()) {
            Ok(record) => {
                self.as_mut().stop_run();
                *self.conversation.borrow_mut() = record.conversation;
                *self.conversation_id.borrow_mut() = Some(record.id);
                self.attachments.borrow_mut().clear();
                self.as_mut().attachments_changed();
                self.as_mut().token_usage_changed();
                // The restored transcript may carry its own model.
                self.as_mut().models_changed();
                FfiResult::default()
            }
            Err(error) => to_chat_result(error),
        }
    }

    pub fn delete_conversation(mut self: Pin<&mut Self>, id: &QString) -> FfiResult {
        let Some(project) = self.session.borrow().root_path().map(Path::to_path_buf) else {
            return FfiResult::default();
        };
        let id = id.to_string();
        match self.history.delete(&project, &id) {
            Ok(()) => {
                if self.conversation_id.borrow().as_deref() == Some(id.as_str()) {
                    // The record it was saved as is gone, so what is on
                    // screen is an unsaved conversation again rather than
                    // something that would resurrect the file on next save.
                    *self.conversation_id.borrow_mut() = None;
                }
                self.as_mut().conversations_changed();
                FfiResult::default()
            }
            Err(error) => to_chat_result(error),
        }
    }

    pub fn rename_conversation(
        mut self: Pin<&mut Self>,
        id: &QString,
        title: &QString,
    ) -> FfiResult {
        let Some(project) = self.session.borrow().root_path().map(Path::to_path_buf) else {
            return FfiResult::default();
        };
        match self
            .history
            .rename(&project, &id.to_string(), &title.to_string())
        {
            Ok(()) => {
                self.as_mut().conversations_changed();
                FfiResult::default()
            }
            Err(error) => to_chat_result(error),
        }
    }

    pub fn set_persistence_enabled(mut self: Pin<&mut Self>, enabled: bool) {
        self.persist.set(enabled);
        let config_dir = app_core::resolve_config_dir();
        let _ = app_config::update(&config_dir, |settings| {
            settings.ai_persist_conversations = Some(enabled);
        });
        if enabled {
            self.as_mut().save_conversation();
        }
        self.as_mut().conversations_changed();
    }

    /// Write the transcript to the store, if this conversation is being
    /// persisted at all. Called after every completed turn, so a crash
    /// costs the answer in flight and nothing before it.
    pub(crate) fn save_conversation(mut self: Pin<&mut Self>) {
        if !self.persist.get() {
            return;
        }
        let Some(project) = self.session.borrow().root_path().map(Path::to_path_buf) else {
            return;
        };
        let conversation = self.conversation.borrow().clone();
        if conversation.is_empty() {
            return;
        }
        let now = now_unix();
        let id = self.conversation_id.borrow().clone().unwrap_or_else(|| {
            // `history` reads no clock and issues no ids; the counter tells
            // apart two conversations started in the same second.
            self.id_counter.set(self.id_counter.get() + 1);
            ai_chat_core::history::new_id(now, self.id_counter.get())
        });
        let record = ConversationRecord {
            id: id.clone(),
            title: ai_chat_core::history::derive_title(&conversation),
            project,
            updated_unix: now,
            conversation,
        };
        if self.history.save(&record).is_ok() {
            *self.conversation_id.borrow_mut() = Some(id);
            self.as_mut().conversations_changed();
        }
    }
}

/// The file name, for the apply summary's title.
pub(crate) fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_provider_vocabularies_map_onto_each_other_completely() {
        // `settings-model` spells the compatible kind with an underscore and
        // `ai-chat-core` with a hyphen (ADR-0017 keeps the vocabularies
        // apart). Every shipped kind has to survive the crossing, or a
        // provider is configurable and unusable.
        for entry in settings_model::ai::default_providers() {
            assert!(
                to_core_kind(entry.kind.as_str()).is_ok(),
                "settings kind {:?} has no ai-chat-core counterpart",
                entry.kind.as_str()
            );
        }
        assert!(to_core_kind("something_new").is_err());
    }
}
