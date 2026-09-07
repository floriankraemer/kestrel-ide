use core::pin::Pin;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use app_core::{AppError, AppSession, TabId, TabKind};
use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::convert;
use crate::bridge::convert::{
    dispatch_editor_command, flatten_symbol_tree, search_options, to_ffi_location, to_ffi_result,
    MAX_HEX_ROWS_PER_REQUEST,
};
use crate::bridge::ffi::{self, FfiOpenResult, FfiResult};
use crate::bridge::registry::{
    index_slot, mcp_control, shared_session, stop_mcp_server, McpControl,
};

/// Rust side of the `DocumentManager` QObject: a handle to the shared
/// session, nothing else — tabs, dirty flags, and the watcher-suppression
/// policy all live in `app-core`.
pub struct DocumentManagerRust {
    pub(crate) session: Rc<RefCell<AppSession>>,
}

impl Default for DocumentManagerRust {
    fn default() -> Self {
        Self {
            session: shared_session(),
        }
    }
}

impl ffi::DocumentManager {
    pub fn open_file(mut self: Pin<&mut Self>, path: &QString) -> FfiOpenResult {
        let path = std::path::PathBuf::from(path.to_string());
        let result = self.session.borrow_mut().open_file(&path);
        match result {
            Ok(opened) => {
                if opened.newly_opened {
                    self.as_mut()
                        .tab_opened(opened.id.raw(), QString::from(opened.title.as_str()));
                }
                FfiOpenResult {
                    code: AppError::CODE_OK,
                    message: QString::default(),
                    tab_id: opened.id.raw(),
                }
            }
            Err(err) => FfiOpenResult {
                code: err.code(),
                message: QString::from(err.to_string().as_str()),
                tab_id: 0,
            },
        }
    }

    pub fn find_matches(
        mut self: Pin<&mut Self>,
        text: &QString,
        pattern: &QString,
        is_regex: bool,
        case_sensitive: bool,
    ) -> Vec<ffi::FfiTextMatch> {
        let opts = search_options(is_regex, case_sensitive);
        match editor_core::find_matches(&text.to_string(), &pattern.to_string(), opts) {
            Ok(matches) => matches
                .into_iter()
                .map(|m| ffi::FfiTextMatch {
                    start: m.start as u32,
                    end: m.end as u32,
                })
                .collect(),
            Err(err) => {
                self.as_mut()
                    .find_pattern_invalid(QString::from(err.to_string().as_str()));
                Vec::new()
            }
        }
    }

    pub fn replacement_edits(
        mut self: Pin<&mut Self>,
        text: &QString,
        pattern: &QString,
        replacement: &QString,
        is_regex: bool,
        case_sensitive: bool,
        index: i32,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let opts = search_options(is_regex, case_sensitive);
        let items = editor_core::replacement_edits(
            &text,
            &pattern.to_string(),
            &replacement.to_string(),
            opts,
            usize::try_from(index).ok(),
        );
        let items = match items {
            Ok(items) => items,
            Err(err) => {
                self.as_mut()
                    .find_pattern_invalid(QString::from(err.to_string().as_str()));
                return Vec::new();
            }
        };
        // Flat UTF-16 offsets in, (line, character) out — the only work this
        // slot does beyond `editor_core::search`, and the same re-expression
        // `EditorOps::to_ffi_edits` performs for its own transactions.
        let starts = editor_core::offsets::utf16_line_starts(&text);
        items
            .into_iter()
            .map(|r| {
                let (start_line, start_character) =
                    editor_core::offsets::utf16_position_at(&starts, r.start);
                let (end_line, end_character) =
                    editor_core::offsets::utf16_position_at(&starts, r.end);
                ffi::FfiTextEdit {
                    path: QString::default(),
                    in_buffer: true,
                    start_line,
                    start_character,
                    end_line,
                    end_character,
                    new_text: QString::from(r.text.as_str()),
                }
            })
            .collect()
    }

    pub fn close_tab(mut self: Pin<&mut Self>, tab_id: u64) {
        let closed = self.session.borrow_mut().close_tab(TabId::from_raw(tab_id));
        if closed {
            self.as_mut().tab_closed(tab_id);
        }
    }

    pub fn save_tab(mut self: Pin<&mut Self>, tab_id: u64, content: &QString) -> FfiResult {
        let result = self
            .session
            .borrow_mut()
            .save_tab(TabId::from_raw(tab_id), &content.to_string());
        if result.is_ok() {
            self.as_mut().tab_modified_changed(tab_id, false);
        }
        to_ffi_result(result)
    }

    pub fn save_tab_as(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        path: &QString,
        content: &QString,
    ) -> FfiResult {
        let path = std::path::PathBuf::from(path.to_string());
        let result = self.session.borrow_mut().save_tab_as(
            TabId::from_raw(tab_id),
            path,
            &content.to_string(),
        );
        if result.is_ok() {
            self.as_mut().tab_modified_changed(tab_id, false);
        }
        to_ffi_result(result)
    }

    pub fn set_active_tab(self: Pin<&mut Self>, tab_id: u64) {
        self.session
            .borrow_mut()
            .set_active_tab(TabId::from_raw(tab_id));
    }

    pub fn set_tab_modified(mut self: Pin<&mut Self>, tab_id: u64, modified: bool) {
        let changed = self
            .session
            .borrow_mut()
            .set_tab_dirty(TabId::from_raw(tab_id), modified);
        if changed {
            self.as_mut().tab_modified_changed(tab_id, modified);
        }
    }

    pub fn record_jump(self: Pin<&mut Self>, path: &QString, line: u32, column: u32) {
        self.session.borrow_mut().record_jump(
            std::path::PathBuf::from(path.to_string()),
            line,
            column,
        );
    }

    pub fn jump_back(self: Pin<&mut Self>) -> ffi::FfiLocation {
        let location = self.session.borrow_mut().jump_back();
        to_ffi_location(location)
    }

    pub fn jump_forward(self: Pin<&mut Self>) -> ffi::FfiLocation {
        let location = self.session.borrow_mut().jump_forward();
        to_ffi_location(location)
    }

    pub fn can_jump_back(&self) -> bool {
        self.session.borrow().can_jump_back()
    }

    pub fn can_jump_forward(&self) -> bool {
        self.session.borrow().can_jump_forward()
    }

    pub fn set_cursor_position(self: Pin<&mut Self>, tab_id: u64, line: u32, column: u32) {
        self.session
            .borrow_mut()
            .set_cursor_position(TabId::from_raw(tab_id), line, column);
    }

    pub fn tab_content(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .tab_content(TabId::from_raw(tab_id))
            .map(|content| QString::from(content.as_str()))
            .unwrap_or_default()
    }

    /// C12-followup: whether `tab_id` is read-only (a virtual document, a
    /// binary tab, or a diff tab) — `false` for an unknown tab, matching
    /// every other `tab_id`-keyed accessor's "not found" answer.
    pub fn tab_is_read_only(&self, tab_id: u64) -> bool {
        self.session
            .borrow()
            .tab_is_read_only(TabId::from_raw(tab_id))
            .unwrap_or(false)
    }

    pub fn tab_file_name(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .tab_file_name(TabId::from_raw(tab_id))
            .map(|name| QString::from(name.as_str()))
            .unwrap_or_default()
    }

    pub fn tab_language_name(&self, tab_id: u64) -> QString {
        let file_name = self
            .session
            .borrow()
            .tab_file_name(TabId::from_raw(tab_id))
            .unwrap_or_default();
        let language = syntax_core::language_for_path(Path::new(&file_name));
        QString::from(&syntax_core::language_name(language))
    }

    pub fn tab_outline(&self, tab_id: u64) -> Vec<ffi::FfiSymbolNode> {
        let session = self.session.borrow();
        let tab_id = TabId::from_raw(tab_id);
        let file_name = session.tab_file_name(tab_id).unwrap_or_default();
        let language = syntax_core::language_for_path(Path::new(&file_name));
        let Some(content) = session.tab_content(tab_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        flatten_symbol_tree(&syntax_core::outline(language, &content), 0, &mut out);
        out
    }

    pub fn tab_title(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .tab_title(TabId::from_raw(tab_id))
            .map(|title| QString::from(title.as_str()))
            .unwrap_or_default()
    }

    pub fn tab_path(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .tab_path(TabId::from_raw(tab_id))
            .map(|path| QString::from(path.to_string_lossy().as_ref()))
            .unwrap_or_default()
    }

    pub fn tab_is_modified(&self, tab_id: u64) -> bool {
        self.session
            .borrow()
            .tab_is_dirty(TabId::from_raw(tab_id))
            .unwrap_or(false)
    }

    pub fn open_diff_tab(
        mut self: Pin<&mut Self>,
        path: &QString,
        left_label: &QString,
        right_label: &QString,
        left_text: &QString,
        right_text: &QString,
    ) -> u64 {
        let id = self.session.borrow_mut().open_diff_tab(
            std::path::PathBuf::from(path.to_string()),
            left_label.to_string(),
            right_label.to_string(),
            left_text.to_string(),
            right_text.to_string(),
        );
        let title = self.session.borrow().tab_title(id).unwrap_or_default();
        self.as_mut()
            .tab_opened(id.raw(), QString::from(title.as_str()));
        id.raw()
    }

    pub fn diff_left_label(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .diff_labels(TabId::from_raw(tab_id))
            .map(|(left, _)| QString::from(left))
            .unwrap_or_default()
    }

    pub fn diff_right_label(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .diff_labels(TabId::from_raw(tab_id))
            .map(|(_, right)| QString::from(right))
            .unwrap_or_default()
    }

    pub fn diff_hunks(&self, tab_id: u64) -> Vec<ffi::FfiHunk> {
        crate::bridge::convert::to_ffi_hunks(
            &self.session.borrow().diff_hunks(TabId::from_raw(tab_id)),
        )
    }

    pub fn diff_spans(&self, tab_id: u64) -> Vec<ffi::FfiInlineSpan> {
        let session = self.session.borrow();
        match session.diff_texts(TabId::from_raw(tab_id)) {
            Some((left, right)) => crate::bridge::convert::to_ffi_inline_spans(
                left,
                right,
                &session.diff_hunks(TabId::from_raw(tab_id)),
                editor_core::diff::HighlightMode::Words,
            ),
            None => Vec::new(),
        }
    }

    pub fn diff_left_text(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .diff_texts(TabId::from_raw(tab_id))
            .map(|(left, _)| QString::from(left))
            .unwrap_or_default()
    }

    pub fn diff_right_text(&self, tab_id: u64) -> QString {
        self.session
            .borrow()
            .diff_texts(TabId::from_raw(tab_id))
            .map(|(_, right)| QString::from(right))
            .unwrap_or_default()
    }

    pub fn diff_hunks_between(
        &self,
        left_text: &QString,
        right_text: &QString,
        whitespace: ffi::FfiWhitespaceMode,
    ) -> Vec<ffi::FfiHunk> {
        let hunks = editor_core::diff::diff_lines_opts(
            &left_text.to_string(),
            &right_text.to_string(),
            convert::to_whitespace_mode(whitespace),
        )
        .unwrap_or_default();
        convert::to_ffi_hunks(&hunks)
    }

    pub fn diff_spans_between(
        &self,
        left_text: &QString,
        right_text: &QString,
        whitespace: ffi::FfiWhitespaceMode,
        highlight: ffi::FfiHighlightMode,
    ) -> Vec<ffi::FfiInlineSpan> {
        let left = left_text.to_string();
        let right = right_text.to_string();
        let hunks = editor_core::diff::diff_lines_opts(
            &left,
            &right,
            convert::to_whitespace_mode(whitespace),
        )
        .unwrap_or_default();
        convert::to_ffi_inline_spans(&left, &right, &hunks, convert::to_highlight_mode(highlight))
    }

    pub fn diff_rows_between(
        &self,
        left_text: &QString,
        right_text: &QString,
        whitespace: ffi::FfiWhitespaceMode,
    ) -> Vec<ffi::FfiDiffRow> {
        let left = left_text.to_string();
        let right = right_text.to_string();
        let hunks = editor_core::diff::diff_lines_opts(
            &left,
            &right,
            convert::to_whitespace_mode(whitespace),
        )
        .unwrap_or_default();
        convert::to_ffi_rows(&editor_core::diff::diff_rows(
            left.lines().count(),
            right.lines().count(),
            &hunks,
        ))
    }

    pub fn hunk_revert_edit(&self, left_text: &QString, hunk: ffi::FfiHunk) -> ffi::FfiTextEdit {
        let edit = editor_core::diff::revert_hunk_edit(
            &left_text.to_string(),
            &convert::from_ffi_hunk(&hunk),
        );
        ffi::FfiTextEdit {
            path: QString::default(),
            in_buffer: true,
            start_line: edit.start_line as u32,
            start_character: 0,
            end_line: edit.end_line as u32,
            end_character: 0,
            new_text: QString::from(edit.new_text.as_str()),
        }
    }

    pub fn tab_kind(&self, tab_id: u64) -> i32 {
        self.session
            .borrow()
            .tab_kind(TabId::from_raw(tab_id))
            .map(|kind| kind.code())
            .unwrap_or(TabKind::CODE_TEXT)
    }

    pub fn binary_row_count(&self, tab_id: u64) -> u64 {
        self.session
            .borrow()
            .binary_row_count(TabId::from_raw(tab_id))
            .unwrap_or(0)
    }

    pub fn binary_length(&self, tab_id: u64) -> u64 {
        self.session
            .borrow()
            .binary_len(TabId::from_raw(tab_id))
            .unwrap_or(0)
    }

    pub fn hex_rows(&self, tab_id: u64, first_row: u64, count: u64) -> Vec<ffi::FfiHexRow> {
        // `count` arrives from the widget as a row count derived from its
        // viewport height, so it is small; clamping keeps a bad value from
        // turning into a huge allocation regardless.
        let count = count.min(MAX_HEX_ROWS_PER_REQUEST) as usize;
        self.session
            .borrow_mut()
            .binary_rows(TabId::from_raw(tab_id), first_row, count)
            .into_iter()
            .map(|row| ffi::FfiHexRow {
                offset: QString::from(row.offset.as_str()),
                hex: QString::from(row.hex.as_str()),
                ascii: QString::from(row.ascii.as_str()),
            })
            .collect()
    }

    pub fn check_external_change(mut self: Pin<&mut Self>, path: &QString) {
        let path_buf = std::path::PathBuf::from(path.to_string());
        let hit = self.session.borrow_mut().check_external_change(&path_buf);
        if let Some(id) = hit {
            self.as_mut()
                .external_change_detected(id.raw(), path.clone());
        }
    }

    pub fn reload_tab_from_disk(self: Pin<&mut Self>, tab_id: u64) -> FfiResult {
        let result = self
            .session
            .borrow_mut()
            .reload_tab(TabId::from_raw(tab_id));
        to_ffi_result(result)
    }

    pub fn apply_mcp_settings(mut self: Pin<&mut Self>) {
        stop_mcp_server();

        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        if !settings.mcp_enabled_or_default() {
            self.as_mut().mcp_stopped();
            return;
        }
        let port = settings.mcp_port;

        let qt_thread = self.qt_thread();
        let index = index_slot();
        // A tokio oneshot rather than a std channel: its `send` needs no
        // runtime, so `stop_mcp_server` can raise it straight from the Qt
        // thread, and the server loop can await it in a `select!`.
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Runtime::new() else {
                let _ = qt_thread.queue(|mut doc_manager: Pin<&mut Self>| {
                    doc_manager
                        .as_mut()
                        .mcp_failed(QString::from("Could not start the MCP server's runtime."));
                });
                return;
            };
            runtime.block_on(async move {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let config_dir = app_core::resolve_config_dir();
                let server = match mcp_server::start(&config_dir, tx, index, port).await {
                    Ok(server) => server,
                    Err(err) => {
                        let message =
                            format!("The MCP server could not listen on 127.0.0.1:{port}: {err}");
                        let _ = qt_thread.queue(move |mut doc_manager: Pin<&mut Self>| {
                            doc_manager
                                .as_mut()
                                .mcp_failed(QString::from(message.as_str()));
                        });
                        return;
                    }
                };
                let bound_port = server.port;
                let _ = qt_thread.queue(move |mut doc_manager: Pin<&mut Self>| {
                    doc_manager.as_mut().mcp_started(bound_port);
                });

                loop {
                    tokio::select! {
                        command = rx.recv() => match command {
                            Some(cmd) => {
                                let _ = qt_thread.queue(move |doc_manager: Pin<&mut Self>| {
                                    dispatch_editor_command(doc_manager, cmd);
                                });
                            }
                            // Every sender is gone, so nothing can arrive
                            // any more — there is nothing left to serve.
                            None => break,
                        },
                        _ = &mut stop_rx => break,
                    }
                }
                server.shutdown().await;
            });
        });

        *mcp_control().lock().expect("MCP control lock poisoned") = Some(McpControl {
            stop: stop_tx,
            thread,
        });
    }

    pub fn shutdown_mcp_server(&self) {
        stop_mcp_server();
    }
}
