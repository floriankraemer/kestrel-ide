use core::pin::Pin;
use std::collections::HashMap;

use app_core::{AppError, TabId};
use cxx_qt_lib::QString;

use crate::bridge::convert::{flatten_symbol_tree, symbol_kind_word};
use crate::bridge::ffi::{self, FfiResult};
use ai_chat_core::agent::{self, AgentCallbacks, Decision, RunLimits, RunOutcome};
use ai_chat_core::context::Attachment;
use ai_chat_core::conversation::{Conversation, Role};
use ai_chat_core::providers::ProviderConfig;
use ai_chat_core::tools::{self, ToolCall, ToolOutcome, ToolPolicy};
use ai_chat_core::{transport, ChatError};

/// How long a worker parked on an approval card waits before it gives up.
///
/// A wait with no ceiling is a leaked thread: the user closes the panel, the
/// window, or walks away, and the run never ends. Ten minutes is far longer
/// than a decision takes and far shorter than a session, and the timeout
/// resolves to a *denial* rather than an approval — the one direction that
/// cannot do something the user never agreed to.
pub(crate) const APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// How long the worker waits for the Qt thread to run one tool.
///
/// The Qt thread never blocks on the worker, so this can only expire if the
/// UI thread is wedged for two minutes — at which point answering the model
/// with a failure beats parking the run forever.
pub(crate) const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The lock is only ever held for a field assignment, so a poisoned one
/// carries no broken invariant — recovering beats taking the run down.
pub(crate) fn recover<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The rendezvous between `agent::run`'s `approve` callback, which blocks on
/// the worker thread, and the human clicking a card on the Qt thread.
///
/// `agent::run` calls `approve` synchronously and expects an answer, but the
/// answer comes from a widget. So the worker parks here while the Qt thread
/// shows the card, and `approveTool`/`denyTool`/`stopRun` — all of which run
/// on the Qt thread — wake it. Every exit is a *decision*: an answer, a
/// stop, or the timeout, and the last two resolve to a denial, because
/// nothing else can be inferred from silence.
#[derive(Default)]
pub(crate) struct GateInner {
    /// The call currently parked, so a stale click from a card the user
    /// left open cannot answer the next call.
    waiting: Option<String>,
    answer: Option<Decision>,
    /// Set by `stopRun`/`cancelRequest`: the run is over, so nothing may
    /// park here again either.
    abandoned: bool,
}

#[derive(Default)]
pub(crate) struct ApprovalGate {
    inner: std::sync::Mutex<GateInner>,
    answered: std::sync::Condvar,
}

impl ApprovalGate {
    /// Parks the worker until the Qt thread answers, the run is abandoned,
    /// or [`APPROVAL_TIMEOUT`] expires.
    ///
    /// The denial reason is left empty on purpose in both silent exits:
    /// `agent::run` composes what the model is told, and a sentence written
    /// here would be model-facing wording in the adapter (ADR-0021 §6).
    fn wait_for_decision(&self, call_id: &str) -> Decision {
        let mut inner = recover(self.inner.lock());
        if inner.abandoned {
            return Decision::Denied(String::new());
        }
        inner.waiting = Some(call_id.to_string());
        inner.answer = None;
        let (mut inner, wait) = recover(self.answered.wait_timeout_while(
            inner,
            APPROVAL_TIMEOUT,
            |gate| gate.answer.is_none() && !gate.abandoned,
        ));
        inner.waiting = None;
        match inner.answer.take() {
            Some(decision) => decision,
            // Timed out, or stopped: a denial either way. Silence is never
            // read as consent.
            None => {
                let _ = wait.timed_out();
                Decision::Denied(String::new())
            }
        }
    }

    /// Answers the parked call. False when `call_id` is not the one waiting
    /// — a card the user left on screen from an earlier run answers nothing.
    pub(crate) fn answer(&self, call_id: &str, decision: Decision) -> bool {
        let mut inner = recover(self.inner.lock());
        if inner.waiting.as_deref() != Some(call_id) {
            return false;
        }
        inner.answer = Some(decision);
        self.answered.notify_all();
        true
    }

    /// The run is over. Wakes anything parked and refuses to park anything
    /// else — this is what stops a user who closes the panel mid-approval
    /// from stranding the worker forever.
    pub(crate) fn abandon(&self) {
        let mut inner = recover(self.inner.lock());
        inner.abandoned = true;
        inner.waiting = None;
        self.answered.notify_all();
    }
}

/// How many assistant turns a transcript holds — one per round trip, which
/// is what `runStepCount` reports.
pub(crate) fn assistant_turns(conversation: &Conversation) -> usize {
    conversation
        .turns()
        .iter()
        .filter(|turn| turn.role == Role::Assistant)
        .count()
}

/// A tool call as the approval card shows it. `summary` is the sentence
/// `tools::summarise` composed — deciding what a call *means* is a rule, and
/// it is the sentence the user consents to.
pub(crate) fn to_ffi_tool_call(call: &ToolCall) -> ffi::FfiToolCall {
    ffi::FfiToolCall {
        call_id: QString::from(call.call_id.as_str()),
        tool: QString::from(call.tool.as_str()),
        summary: QString::from(tools::summarise(call).as_str()),
        arguments: QString::from(
            serde_json::to_string_pretty(&call.arguments)
                .unwrap_or_else(|_| call.arguments.to_string())
                .as_str(),
        ),
        // Always true here: `toolCallPending` is emitted only when the loop
        // is genuinely blocked, since the panel disables the composer while
        // a card is up.
        needs_approval: true,
    }
}

/// Ask mode: one request, one streamed answer, no tools.
///
/// Written out rather than driven through `agent::run` with everything
/// denied, because the two differ in what is *sent*: Ask sends no tool
/// schemas at all, and some OpenAI-compatible runtimes change their answer
/// format for a present-but-empty `tools` key.
pub(crate) fn run_ask(
    qt_thread: &cxx_qt::CxxQtThread<ffi::AiChat>,
    config: &ProviderConfig,
    api_key: &str,
    conversation: &mut Conversation,
    system: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> (i32, String) {
    let body = match ai_chat_core::request::build_body(config, conversation, system, &[], false) {
        Ok(body) => body,
        Err(error) => return (error.code(), error.to_string()),
    };
    let url = match ai_chat_core::request::endpoint_url(config) {
        Ok(url) => url,
        Err(error) => return (error.code(), error.to_string()),
    };
    let spec = transport::RequestSpec {
        url,
        headers: ai_chat_core::request::protocol_headers(config),
        body,
    };

    conversation.begin_assistant();
    let mut sink = |event: ai_chat_core::stream::StreamEvent| match event {
        ai_chat_core::stream::StreamEvent::TextDelta(text) => {
            conversation.append_text_delta(&text);
            let _ = qt_thread.queue(move |chat: Pin<&mut ffi::AiChat>| chat.on_delta(text));
        }
        ai_chat_core::stream::StreamEvent::Usage {
            input_tokens,
            output_tokens,
        } => {
            let _ = qt_thread.queue(move |chat: Pin<&mut ffi::AiChat>| {
                chat.on_usage(input_tokens, output_tokens)
            });
        }
        _ => {}
    };
    let result = transport::stream_chat(config, spec, api_key, cancel, &mut sink);
    conversation.finish_assistant();
    match result {
        Ok(()) => (ChatError::CODE_OK, String::new()),
        Err(error) => (error.code(), error.to_string()),
    }
}

/// Agent mode: `agent::run` with the three callbacks it needs, each of which
/// crosses back to the Qt thread.
///
/// `approve` parks on the [`ApprovalGate`]; `execute` hands the call to the
/// Qt thread and waits on a channel for the answer. Neither direction can
/// deadlock: the Qt thread never blocks on this one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_agent(
    qt_thread: &cxx_qt::CxxQtThread<ffi::AiChat>,
    config: &ProviderConfig,
    api_key: &str,
    conversation: &mut Conversation,
    system: &str,
    base_policies: HashMap<String, ToolPolicy>,
    promoted: std::sync::Arc<std::sync::Mutex<HashMap<String, ToolPolicy>>>,
    cancel: &std::sync::atomic::AtomicBool,
    gate: &ApprovalGate,
    root: Option<std::path::PathBuf>,
) -> (i32, String) {
    let limits = RunLimits::default();

    let policies = |tool: &str| -> ToolPolicy {
        if let Some(policy) = recover(promoted.lock()).get(tool) {
            return *policy;
        }
        base_policies
            .get(tool)
            .copied()
            .unwrap_or_else(|| tools::default_policy(tool))
    };

    let mut approve = |call: &ToolCall| -> Decision {
        let shown = call.clone();
        let _ = qt_thread.queue(move |chat: Pin<&mut ffi::AiChat>| chat.on_tool_pending(shown));
        gate.wait_for_decision(&call.call_id)
    };

    let mut execute = |call: &ToolCall| -> ToolOutcome {
        // SECURITY: confinement is the executor's job, because the project
        // root is the executor's knowledge — `agent::run` deliberately takes
        // no root (see its module docs). A path that leaves the project, or
        // names a credentials-shaped file, becomes a result the model can
        // read and route around, never a panic.
        if let Err(error) = tools::validate_call(call, root.as_deref()) {
            return ToolOutcome {
                content: error.to_string(),
                is_error: true,
            };
        }
        let (answer, wait) = std::sync::mpsc::channel();
        let call_for_qt = call.clone();
        let queued = qt_thread.queue(move |chat: Pin<&mut ffi::AiChat>| {
            let outcome = chat.execute_tool(&call_for_qt);
            let _ = answer.send(outcome);
        });
        if queued.is_err() {
            return ToolOutcome {
                content: ChatError::Cancelled.to_string(),
                is_error: true,
            };
        }
        wait.recv_timeout(TOOL_TIMEOUT).unwrap_or(ToolOutcome {
            content: ChatError::Cancelled.to_string(),
            is_error: true,
        })
    };

    let mut on_text_delta = |text: &str| {
        let text = text.to_string();
        let _ = qt_thread.queue(move |chat: Pin<&mut ffi::AiChat>| chat.on_delta(text));
    };
    let mut on_tool_started = |call: &ToolCall| {
        let call = call.clone();
        let _ = qt_thread.queue(move |chat: Pin<&mut ffi::AiChat>| chat.on_tool_started(call));
    };
    let mut on_tool_finished = |call: &ToolCall, outcome: &ToolOutcome| {
        let (call, outcome) = (call.clone(), outcome.clone());
        let _ = qt_thread
            .queue(move |chat: Pin<&mut ffi::AiChat>| chat.on_tool_finished(call, outcome));
    };

    let mut callbacks = AgentCallbacks {
        approve: &mut approve,
        execute: &mut execute,
        on_text_delta: &mut on_text_delta,
        on_tool_started: &mut on_tool_started,
        on_tool_finished: &mut on_tool_finished,
    };

    let outcome = agent::run(
        config,
        api_key,
        conversation,
        system,
        &policies,
        limits,
        cancel,
        &mut callbacks,
    );
    match outcome {
        // Both are "the loop ended and there is nothing further to say":
        // one because the model answered, one because it produced nothing
        // to answer with, and repeating the request would send the same
        // bytes for the same nothing.
        RunOutcome::Answered | RunOutcome::Stopped => (ChatError::CODE_OK, String::new()),
        RunOutcome::CeilingHit(limit) => {
            let ceiling = match limit {
                ai_chat_core::RunLimit::Steps => u64::from(limits.max_steps),
                ai_chat_core::RunLimit::Seconds => limits.max_seconds,
                ai_chat_core::RunLimit::Tokens => u64::from(limits.max_tokens),
            };
            let error = ChatError::RunCeilingExceeded { limit, ceiling };
            (error.code(), error.to_string())
        }
        RunOutcome::Cancelled => (
            ChatError::Cancelled.code(),
            ChatError::Cancelled.to_string(),
        ),
        RunOutcome::Failed(error) => (error.code(), error.to_string()),
    }
}

/// The index rows as JSON for the model. Shape only — the queries
/// themselves are `index_core`'s, the same methods the MCP tools call, so
/// there is no second implementation of "search the project".
pub(crate) fn search_match_json(hit: &index_core::SearchMatch) -> serde_json::Value {
    serde_json::json!({
        "path": hit.path.to_string_lossy(),
        "line": hit.line,
        "start": hit.start,
        "end": hit.end,
        "text": hit.line_text,
    })
}

pub(crate) fn file_match_json(hit: &index_core::FileMatch) -> serde_json::Value {
    serde_json::json!({ "path": hit.path.to_string_lossy(), "relative": hit.relative })
}

pub(crate) fn symbol_match_json(hit: &index_core::SymbolMatch) -> serde_json::Value {
    serde_json::json!({
        "name": hit.name,
        "kind": symbol_kind_word(hit.kind),
        "path": hit.path.to_string_lossy(),
        "line": hit.line,
        "column": hit.col,
        "is_definition": hit.is_definition,
        "container": hit.container,
    })
}

/// The severity word the server itself used, kept as a string rather than
/// re-classified — `context::DiagnosticNote` takes it that way on purpose.
pub(crate) fn severity_word(severity: diagnostics_core::Severity) -> &'static str {
    match severity {
        diagnostics_core::Severity::Error => "error",
        diagnostics_core::Severity::Warning => "warning",
        diagnostics_core::Severity::Information => "information",
        diagnostics_core::Severity::Hint => "hint",
    }
}

/// The chip's kind, which the panel picks an icon from.
pub(crate) fn attachment_kind(attachment: &Attachment) -> &'static str {
    match attachment {
        Attachment::Selection { .. } => "selection",
        Attachment::File { .. } => "file",
        Attachment::Symbol { .. } => "symbol",
        Attachment::Diagnostics(_) => "diagnostics",
        Attachment::TerminalOutput(_) => "terminal",
        Attachment::Image { .. } => "image",
    }
}

impl ffi::AiChat {
    // --- what the worker queues back onto the Qt thread -------------------

    /// Mirror one text delta into the Qt-side transcript and tell the panel
    /// which bubble to append to.
    fn on_delta(mut self: Pin<&mut Self>, text: String) {
        let (index, started) = {
            let mut conversation = self.conversation.borrow_mut();
            let started = !conversation.is_streaming();
            conversation.append_text_delta(&text);
            (conversation.len().saturating_sub(1) as u64, started)
        };
        if started {
            self.as_mut().message_started(index);
        }
        self.as_mut()
            .delta_received(index, QString::from(text.as_str()));
    }

    fn on_usage(mut self: Pin<&mut Self>, input_tokens: u32, output_tokens: u32) {
        let (input, output) = self.usage.get();
        // Anthropic sends the input count at the start and the output count
        // at the end, so one answer legitimately reports twice.
        self.usage
            .set((input.max(input_tokens), output + output_tokens));
        self.as_mut().token_usage_changed();
    }

    fn on_tool_pending(mut self: Pin<&mut Self>, call: ToolCall) {
        let shown = to_ffi_tool_call(&call);
        *self.pending_call.borrow_mut() = Some(call);
        self.as_mut().tool_call_pending(shown);
    }

    fn on_tool_started(mut self: Pin<&mut Self>, call: ToolCall) {
        self.as_mut().conversation.borrow_mut().push_tool_use(
            call.call_id,
            call.tool,
            call.arguments,
        );
    }

    fn on_tool_finished(mut self: Pin<&mut Self>, call: ToolCall, outcome: ToolOutcome) {
        self.conversation.borrow_mut().push_tool_result(
            &call.call_id,
            &outcome.content,
            outcome.is_error,
        );
        *self.pending_call.borrow_mut() = None;
        let row = ffi::FfiToolOutcome {
            call_id: QString::from(call.call_id.as_str()),
            tool: QString::from(call.tool.as_str()),
            // A declined call is `ok`: a denial is data, not a failure
            // (ADR-0021 §1), and painting it red would teach the user that
            // saying no broke something.
            status: QString::from(if outcome.is_error { "error" } else { "ok" }),
            detail: QString::from(outcome.content.as_str()),
        };
        self.as_mut().tool_call_finished(row);
    }

    /// The run is over: the worker's transcript is the authoritative one, so
    /// it replaces the mirror wholesale before anything is saved or read
    /// back.
    pub(crate) fn finish_run(
        mut self: Pin<&mut Self>,
        conversation: Conversation,
        code: i32,
        message: String,
    ) {
        let agent_mode = self
            .run
            .borrow()
            .as_ref()
            .map(|run| run.agent_mode)
            .unwrap_or(false);
        let last = conversation.len().saturating_sub(1) as u64;
        *self.conversation.borrow_mut() = conversation;
        *self.run.borrow_mut() = None;
        *self.pending_call.borrow_mut() = None;

        self.as_mut().message_finished(last);
        let result = FfiResult {
            code,
            message: QString::from(message.as_str()),
        };
        if agent_mode {
            self.as_mut().run_finished(result);
        } else if code != ChatError::CODE_OK {
            self.as_mut().chat_failed(result);
        }
        self.as_mut().save_conversation();
        self.as_mut().token_usage_changed();
    }

    // --- tool execution, on the Qt thread ---------------------------------

    /// Runs one already-validated call against the shared `AppSession` and
    /// the shared project index — the same objects the MCP server's tools
    /// reach through `dispatch_editor_command`, so an in-IDE agent and an
    /// attached one see one project and one set of buffers.
    fn execute_tool(mut self: Pin<&mut Self>, call: &ToolCall) -> ToolOutcome {
        let string = |name: &str| -> String {
            call.arguments
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let flag = |name: &str| -> bool {
            call.arguments
                .get(name)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let number = |name: &str| -> Option<u64> {
            call.arguments.get(name).and_then(serde_json::Value::as_u64)
        };
        let limit = number("limit").unwrap_or(100) as usize;
        let tab_id = || TabId::from_raw(number("tab_id").unwrap_or_default());

        let outcome = match call.tool.as_str() {
            "search_text" => self.query_index(|index| {
                let hits = index.search_with(
                    &string("pattern"),
                    flag("is_regex"),
                    flag("case_sensitive"),
                    limit,
                    &std::sync::atomic::AtomicBool::new(false),
                )?;
                Ok(serde_json::json!({
                    "matches": hits.iter().map(search_match_json).collect::<Vec<_>>(),
                }))
            }),
            "find_files" => self.query_index(|index| {
                let hits = index.find_files(&string("query"), limit);
                Ok(serde_json::json!({
                    "files": hits.iter().map(file_match_json).collect::<Vec<_>>(),
                }))
            }),
            "find_definitions" => self.query_index(|index| {
                let hits = index.find_definitions_ranked(&string("query"), limit)?;
                Ok(serde_json::json!({
                    "symbols": hits.iter().map(symbol_match_json).collect::<Vec<_>>(),
                }))
            }),
            "find_usages" => self.query_index(|index| {
                let hits = index.find_usages(&string("name"))?;
                Ok(serde_json::json!({
                    "symbols": hits.iter().map(symbol_match_json).collect::<Vec<_>>(),
                }))
            }),
            "find_implementations" => self.query_index(|index| {
                let hits = index.find_implementations(&string("supertype"))?;
                Ok(serde_json::json!({
                    "symbols": hits.iter().map(symbol_match_json).collect::<Vec<_>>(),
                }))
            }),
            "resolve_declaration" => {
                let path = std::path::PathBuf::from(string("path"));
                // The open buffer wins over the file, exactly as the MCP
                // tool does it: the user may be sitting on unsaved edits,
                // and resolving against disk would answer about text that
                // is no longer on screen.
                let content = self
                    .session
                    .borrow()
                    .content_for_path(&path)
                    .map(Ok)
                    .unwrap_or_else(|| std::fs::read_to_string(&path));
                match content {
                    Ok(content) => {
                        let offset = number("byte_offset").unwrap_or_default() as usize;
                        self.query_index(|index| {
                            let resolution = index.resolve_declaration(&path, &content, offset)?;
                            Ok(serde_json::json!({
                                "name": resolution.name,
                                "candidates": resolution
                                    .candidates
                                    .iter()
                                    .map(symbol_match_json)
                                    .collect::<Vec<_>>(),
                            }))
                        })
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
            "list_project_tree" => {
                let entries: Vec<serde_json::Value> = self
                    .session
                    .borrow()
                    .project_tree_entries()
                    .into_iter()
                    .map(|(path, is_dir)| {
                        serde_json::json!({ "path": path.to_string_lossy(), "is_dir": is_dir })
                    })
                    .collect();
                Ok(serde_json::json!({ "entries": entries }))
            }
            "read_buffer" => match self.session.borrow().tab_content(tab_id()) {
                Some(content) => Ok(serde_json::json!({ "content": content })),
                None => Err(AppError::NoSuchTab.to_string()),
            },
            "open_file" => {
                let path = std::path::PathBuf::from(string("path"));
                let opened = self.session.borrow_mut().open_file(&path);
                match opened {
                    Ok(opened) => {
                        if opened.newly_opened {
                            // The tab strip is `DocumentManager`'s to
                            // change, so this is relayed rather than emitted
                            // here — see the signal's declaration.
                            self.as_mut().tool_opened_tab(
                                opened.id.raw(),
                                QString::from(opened.title.as_str()),
                            );
                        }
                        Ok(serde_json::json!({ "tab_id": opened.id.raw() }))
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
            "edit_buffer" => {
                let (id, content) = (tab_id(), string("content"));
                let edited = self.session.borrow_mut().edit_tab(id, &content);
                match edited {
                    Ok(()) => {
                        self.as_mut()
                            .tool_edited_buffer(id.raw(), QString::from(content.as_str()));
                        Ok(serde_json::Value::Null)
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
            "save_buffer" => {
                let id = tab_id();
                let saved = self.session.borrow_mut().save_buffer(id);
                match saved {
                    Ok(()) => {
                        self.as_mut().tool_saved_buffer(id.raw());
                        Ok(serde_json::Value::Null)
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
            // `agent::run` already refuses a name with no spec before it
            // gets here; this arm exists so the match is total.
            other => Err(format!("{other} is not a tool this IDE has.")),
        };

        match outcome {
            Ok(value) => ToolOutcome {
                content: serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
                is_error: false,
            },
            Err(detail) => ToolOutcome {
                content: ChatError::ToolFailed {
                    tool: call.tool.clone(),
                    detail,
                }
                .to_string(),
                is_error: true,
            },
        }
    }

    /// Runs one read against the project index, or reports why it could not.
    pub(crate) fn query_index<T>(
        &self,
        query: impl FnOnce(&index_core::TextIndex) -> Result<T, index_core::IndexError>,
    ) -> Result<T, String> {
        let guard = self
            .index
            .read()
            .map_err(|_| "the index is unavailable".to_string())?;
        let Some(index) = guard.ready() else {
            return Err(guard
                .unavailable_reason()
                .unwrap_or_else(|| "the project index is not ready yet".to_string()));
        };
        query(index).map_err(|error| error.to_string())
    }
}

/// The text of a symbol's definition, taken from the outline `syntax_core`
/// already produces for the Structure panel rather than by guessing where a
/// definition ends. Falls back to the one line the index pointed at, which
/// is still true and still useful.
pub(crate) fn definition_text(hit: &index_core::SymbolMatch, content: &str) -> String {
    let language = syntax_core::language_for_path(&hit.path);
    let mut flat = Vec::new();
    flatten_symbol_tree(&syntax_core::outline(language, content), 0, &mut flat);
    let node = flat
        .iter()
        .find(|node| node.name.to_string() == hit.name && node.start <= content.len());
    match node {
        Some(node) => content
            .get(node.start..node.end.min(content.len()))
            .unwrap_or_default()
            .to_string(),
        None => content
            .lines()
            .nth(hit.line.saturating_sub(1))
            .unwrap_or_default()
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The approval gate is the one piece of `AiChat` that is pure Rust and
    /// can deadlock, so it is the one piece with tests here rather than in a
    /// Qt-free crate: what is being checked is the marshalling itself, which
    /// has no home anywhere else.
    fn park(
        gate: &std::sync::Arc<ApprovalGate>,
        call_id: &str,
    ) -> std::sync::mpsc::Receiver<Decision> {
        let (answered, decisions) = std::sync::mpsc::channel();
        let gate = std::sync::Arc::clone(gate);
        let call_id = call_id.to_string();
        std::thread::spawn(move || {
            let _ = answered.send(gate.wait_for_decision(&call_id));
        });
        decisions
    }

    /// Waits for the parked worker to answer, failing rather than hanging
    /// the suite if it never does.
    fn decision_within(decisions: &std::sync::mpsc::Receiver<Decision>) -> Decision {
        decisions
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the worker was left parked on the approval gate")
    }

    #[test]
    fn stopping_a_run_while_an_approval_is_pending_releases_the_worker() {
        // The deadlock this exists to prevent: the user closes the panel
        // mid-approval, the click that would have answered can never come,
        // and the worker waits on a condvar for the life of the process.
        let gate = std::sync::Arc::new(ApprovalGate::default());
        let decisions = park(&gate, "call-1");
        // Give the worker time to actually reach the wait, so this tests
        // the wake-up and not a race it happened to win.
        std::thread::sleep(std::time::Duration::from_millis(50));
        gate.abandon();
        assert!(
            matches!(decision_within(&decisions), Decision::Denied(_)),
            "an abandoned run must resolve to a denial: silence is never consent"
        );
    }

    #[test]
    fn a_run_abandoned_before_a_call_parks_never_parks_it_at_all() {
        // `stopRun` can land between the model asking and the worker
        // reaching the gate, and the second call of a step must not wait
        // for a card the panel will never show.
        let gate = std::sync::Arc::new(ApprovalGate::default());
        gate.abandon();
        assert!(matches!(
            decision_within(&park(&gate, "call-2")),
            Decision::Denied(_)
        ));
    }

    #[test]
    fn an_answer_meant_for_another_call_leaves_the_worker_parked() {
        // A card the user left open from an earlier call must not approve
        // whatever happens to be waiting now.
        let gate = std::sync::Arc::new(ApprovalGate::default());
        let decisions = park(&gate, "call-now");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !gate.answer("call-stale", Decision::Approved),
            "a stale call id must be refused, not applied to the current call"
        );
        assert!(decisions
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        gate.abandon();
    }

    #[test]
    fn an_approval_reaches_the_worker_that_asked_for_it() {
        let gate = std::sync::Arc::new(ApprovalGate::default());
        let decisions = park(&gate, "call-3");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(gate.answer("call-3", Decision::Approved));
        assert_eq!(decision_within(&decisions), Decision::Approved);
    }

    #[test]
    fn a_denial_carries_the_users_words_and_survives_them_being_empty() {
        // The panel sends an empty reason; `agent::run` composes the
        // sentence the model is told, so an empty string has to travel
        // through unchanged rather than being papered over here.
        let gate = std::sync::Arc::new(ApprovalGate::default());
        let decisions = park(&gate, "call-4");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(gate.answer("call-4", Decision::Denied(String::new())));
        assert_eq!(decision_within(&decisions), Decision::Denied(String::new()));
    }
}
