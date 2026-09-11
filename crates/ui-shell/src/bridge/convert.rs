//! Translation helpers shared by more than one feature module.
//!
//! Nothing here decides anything (ADR-0002): each function maps a Rust
//! domain value onto its FFI struct, or the other way round. A helper lives
//! here rather than next to its caller only when a second module needs it
//! too.

use core::pin::Pin;
use std::collections::HashMap;
use std::path::Path;

use app_core::{AppError, TabId};
use cxx_qt_lib::QString;
use syntax_core::theme;

use crate::bridge::ffi::{self, FfiResult};

/// Upper bound on rows one `hexRows` call will return. The viewer asks for
/// what fits its viewport, so this only exists so a nonsense `count` can
/// never turn into a huge allocation at the seam.
pub(crate) const MAX_HEX_ROWS_PER_REQUEST: u64 = 4096;

/// The two view-facing booleans as the domain type they mean.
pub(crate) fn search_options(is_regex: bool, case_sensitive: bool) -> editor_core::SearchOptions {
    editor_core::SearchOptions {
        regex: is_regex,
        case_sensitive,
    }
}

/// Translate a command result into the FFI struct (ADR-0003).
pub(crate) fn to_ffi_result(result: Result<(), AppError>) -> FfiResult {
    match result {
        Ok(()) => FfiResult::default(),
        Err(err) => FfiResult {
            code: err.code(),
            message: QString::from(err.to_string().as_str()),
        },
    }
}

/// Rust side of the opaque `SyntaxHighlighterHandle` (Y2/A1): one
/// `syntax_core::Highlighter` per open editor, owned across the FFI seam
/// by the C++ `SyntaxHighlighter` as a `rust::Box`.
pub(crate) struct SyntaxHighlighterHandle {
    highlighter: syntax_core::Highlighter,
    /// Kept alongside the highlighter so `palette` can resolve
    /// per-language colours without the view having to know, or plumb,
    /// a language id of its own.
    language: syntax_core::Language,
    /// C9: the tree-sitter spans from the last `set_text`/`apply_edit`,
    /// kept so `overlay_semantic_tokens` — called separately, once a
    /// server's answer arrives, not on the same revision-change hook that
    /// drives highlighting — has something to overlay onto without a third
    /// reparse.
    last_spans: Vec<syntax_core::HighlightSpan>,
}

pub(crate) fn new_syntax_highlighter(file_name: &str) -> Box<SyntaxHighlighterHandle> {
    let language = syntax_core::language_for_path(Path::new(file_name));
    Box::new(SyntaxHighlighterHandle {
        highlighter: syntax_core::Highlighter::new(language),
        language,
        last_spans: Vec::new(),
    })
}

impl SyntaxHighlighterHandle {
    pub(crate) fn set_text(&mut self, text: &str) -> Vec<ffi::FfiHighlightSpan> {
        self.last_spans = self.highlighter.set_text(text);
        to_ffi_spans(self.last_spans.clone())
    }

    pub(crate) fn apply_edit(
        &mut self,
        new_text: &str,
        start_byte: usize,
        old_end_byte: usize,
        new_end_byte: usize,
    ) -> Vec<ffi::FfiHighlightSpan> {
        self.last_spans = self
            .highlighter
            .edit(new_text, start_byte, old_end_byte, new_end_byte);
        to_ffi_spans(self.last_spans.clone())
    }

    /// C9: overlay `semantic` — already-mapped semantic-token spans, in the
    /// same byte-offset/scope-id shape as `last_spans` — onto the
    /// tree-sitter spans from this handle's last `set_text`/`apply_edit`.
    /// `lsp_core::semantic_tokens::overlay` owns the merge rule (semantic
    /// wins where it covers, tree-sitter fills the rest, per F0-16); this
    /// only translates across the FFI seam, matching every other method
    /// here (ADR-0002).
    pub(crate) fn overlay_semantic_tokens(
        &self,
        semantic: Vec<ffi::FfiHighlightSpan>,
    ) -> Vec<ffi::FfiHighlightSpan> {
        let semantic: Vec<lsp_core::MappedSemanticSpan> = semantic
            .into_iter()
            .filter_map(|span| {
                Some(lsp_core::MappedSemanticSpan {
                    start: span.start,
                    end: span.end,
                    scope: scope_from_id(span.scope)?,
                })
            })
            .collect();
        to_ffi_spans(lsp_core::overlay_semantic_tokens(
            &self.last_spans,
            &semantic,
        ))
    }

    pub(crate) fn fold_ranges(&self) -> Vec<ffi::FfiFoldRange> {
        self.highlighter
            .fold_ranges()
            .into_iter()
            .map(|range| ffi::FfiFoldRange {
                start: range.start,
                end: range.end,
                anchor: range.anchor,
            })
            .collect()
    }

    pub(crate) fn palette(&self, theme: &str) -> Vec<ffi::FfiScopeStyle> {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        let user = user_styles(&settings);
        // Resolved through the colour-theme plugin registry (T7) rather than
        // the old name-based `theme::palette`: a `color-themes` contribution
        // is the only thing "dark"/"light"/"vscode-dark" name any more.
        app_core::color_themes::build_palette(
            &plugin_host::registry(),
            theme,
            &self.language.id(),
            &user,
        )
        .styles()
        .iter()
        .map(|style| ffi::FfiScopeStyle {
            has_fg: style.fg.is_some(),
            red: style.fg.map_or(0, |rgb| rgb.r),
            green: style.fg.map_or(0, |rgb| rgb.g),
            blue: style.fg.map_or(0, |rgb| rgb.b),
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
        })
        .collect()
    }
}

/// Translate the plain string maps `app-config` persists into the typed
/// overrides `syntax_core::theme` resolves against. A colour that will not
/// parse is dropped rather than reported: a hand-edited `settings.toml`
/// with one bad hex value must not stop the editor from highlighting, and
/// `theme::palette` already ignores scope names it does not know.
pub(crate) fn user_styles(settings: &app_config::Settings) -> theme::UserStyles {
    theme::UserStyles {
        base: to_scope_styles(&settings.syntax_colors),
        by_language: settings
            .syntax_colors_by_language
            .iter()
            .map(|(language, styles)| (language.clone(), to_scope_styles(styles)))
            .collect(),
    }
}

pub(crate) fn to_scope_styles(
    styles: &app_config::ScopeStyles,
) -> HashMap<String, theme::ScopeStyle> {
    styles
        .iter()
        .map(|(scope, style)| {
            (
                scope.clone(),
                theme::ScopeStyle {
                    fg: style.fg().and_then(theme::Rgb::parse),
                    bold: style.bold(),
                    italic: style.italic(),
                    underline: style.underline(),
                },
            )
        })
        .collect()
}

pub(crate) fn syntax_scope_names() -> Vec<String> {
    syntax_core::SCOPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// The inverse of `to_ffi_spans`' per-span `scope: span.scope.id()`: rebuilds
/// a `syntax_core::Scope` from the raw id `FfiHighlightSpan` carries. `None`
/// for an id past `syntax_core::SCOPES`, which cannot happen for a span this
/// process itself produced, but a malformed one from across the seam must
/// not panic.
fn scope_from_id(id: u16) -> Option<syntax_core::Scope> {
    syntax_core::Scope::from_id(id)
}

pub(crate) fn to_ffi_spans(spans: Vec<syntax_core::HighlightSpan>) -> Vec<ffi::FfiHighlightSpan> {
    spans
        .into_iter()
        .map(|span| ffi::FfiHighlightSpan {
            start: span.start,
            end: span.end,
            scope: span.scope.id(),
        })
        .collect()
}

/// Pre-order flatten `nodes` (Task D) into `out`, recording each node's
/// `depth` (root = 0) so `FfiSymbolNode`'s doc comment's reconstruction
/// works: siblings/children stay in the tree's own document order since
/// `syntax_core::outline()` already returns them that way.
pub(crate) fn flatten_symbol_tree(
    nodes: &[syntax_core::SymbolNode],
    depth: u32,
    out: &mut Vec<ffi::FfiSymbolNode>,
) {
    for node in nodes {
        out.push(ffi::FfiSymbolNode {
            name: QString::from(node.name.as_str()),
            kind: to_ffi_symbol_kind(node.kind),
            category: to_ffi_symbol_category(node.kind.category()),
            start: node.start,
            end: node.end,
            name_start: node.name_start,
            name_end: node.name_end,
            depth,
        });
        flatten_symbol_tree(&node.children, depth + 1, out);
    }
}

pub(crate) fn to_ffi_location(location: Option<app_core::Location>) -> ffi::FfiLocation {
    match location {
        Some(location) => ffi::FfiLocation {
            found: true,
            path: QString::from(location.path.to_string_lossy().as_ref()),
            line: location.line,
            column: location.column,
        },
        None => ffi::FfiLocation::default(),
    }
}

pub(crate) fn to_ffi_symbol_match(m: index_core::SymbolMatch) -> ffi::FfiSymbolMatch {
    let kind = m.kind.unwrap_or(syntax_core::SymbolKind::Class);
    ffi::FfiSymbolMatch {
        path: QString::from(m.path.to_string_lossy().as_ref()),
        line: m.line as u32,
        column: m.col as u32,
        name: QString::from(m.name.as_str()),
        has_kind: m.kind.is_some(),
        kind: to_ffi_symbol_kind(kind),
        category: to_ffi_symbol_category(kind.category()),
        is_definition: m.is_definition,
        container: QString::from(m.container.as_deref().unwrap_or("")),
    }
}

pub(crate) fn to_ffi_resolution_tier(tier: index_core::ResolutionTier) -> ffi::FfiResolutionTier {
    match tier {
        index_core::ResolutionTier::LocalFile => ffi::FfiResolutionTier::LocalFile,
        index_core::ResolutionTier::Project => ffi::FfiResolutionTier::Project,
        index_core::ResolutionTier::None => ffi::FfiResolutionTier::None,
    }
}

pub(crate) fn to_ffi_symbol_kind(kind: syntax_core::SymbolKind) -> ffi::FfiSymbolKind {
    match kind {
        syntax_core::SymbolKind::Class => ffi::FfiSymbolKind::Class,
        syntax_core::SymbolKind::Struct => ffi::FfiSymbolKind::Struct,
        syntax_core::SymbolKind::Enum => ffi::FfiSymbolKind::Enum,
        syntax_core::SymbolKind::Interface => ffi::FfiSymbolKind::Interface,
        syntax_core::SymbolKind::Method => ffi::FfiSymbolKind::Method,
        syntax_core::SymbolKind::Function => ffi::FfiSymbolKind::Function,
        syntax_core::SymbolKind::Field => ffi::FfiSymbolKind::Field,
        syntax_core::SymbolKind::Constant => ffi::FfiSymbolKind::Constant,
        syntax_core::SymbolKind::Property => ffi::FfiSymbolKind::Property,
        syntax_core::SymbolKind::Constructor => ffi::FfiSymbolKind::Constructor,
        syntax_core::SymbolKind::EnumMember => ffi::FfiSymbolKind::EnumMember,
    }
}

/// Task 4b: `syntax_core::SymbolKind::category`'s result as the FFI
/// ordinal C++ groups children by (ADR-0002 — a translation, the mapping
/// itself is decided in `syntax_core`).
pub(crate) fn to_ffi_symbol_category(
    category: syntax_core::SymbolCategory,
) -> ffi::FfiSymbolCategory {
    match category {
        syntax_core::SymbolCategory::Constants => ffi::FfiSymbolCategory::Constants,
        syntax_core::SymbolCategory::Fields => ffi::FfiSymbolCategory::Fields,
        syntax_core::SymbolCategory::Properties => ffi::FfiSymbolCategory::Properties,
        syntax_core::SymbolCategory::Constructors => ffi::FfiSymbolCategory::Constructors,
        syntax_core::SymbolCategory::Methods => ffi::FfiSymbolCategory::Methods,
        syntax_core::SymbolCategory::NestedTypes => ffi::FfiSymbolCategory::NestedTypes,
        syntax_core::SymbolCategory::Other => ffi::FfiSymbolCategory::Other,
    }
}

/// Push `path` onto the persisted recent-projects list (C2). Best-effort:
/// a settings load/save failure here must not block the folder from
/// opening, so errors are silently dropped — same tolerance `AppSession`
/// already applies to the last-opened-project fallback.
pub(crate) fn push_recent_project(path: std::path::PathBuf) {
    let config_dir = app_core::resolve_config_dir();
    let Ok(mut settings) = app_config::load(&config_dir) else {
        return;
    };
    settings.push_recent_project(path);
    let _ = app_config::save(&config_dir, &settings);
}

/// Runs on the Qt thread (queued there by `apply_mcp_settings`'s listener):
/// does the actual `AppSession`-mediated work for one `EditorCommand` and
/// answers it through the command's own `oneshot::Sender`.
pub(crate) fn dispatch_editor_command(
    mut doc_manager: Pin<&mut ffi::DocumentManager>,
    cmd: mcp_server::EditorCommand,
) {
    match cmd {
        mcp_server::EditorCommand::ListOpenBuffers(respond) => {
            let buffers = doc_manager
                .session
                .borrow()
                .open_tabs()
                .into_iter()
                .map(|(id, title)| mcp_server::BufferInfo {
                    tab_id: id.raw(),
                    title,
                })
                .collect();
            let _ = respond.send(buffers);
        }
        mcp_server::EditorCommand::ListProjectTree(respond) => {
            let entries = doc_manager
                .session
                .borrow()
                .project_tree_entries()
                .into_iter()
                .map(|(path, is_dir)| mcp_server::ProjectTreeEntry {
                    path: path.to_string_lossy().into_owned(),
                    is_dir,
                })
                .collect();
            let _ = respond.send(entries);
        }
        mcp_server::EditorCommand::ReadBuffer { tab_id, respond } => {
            let content = doc_manager
                .session
                .borrow()
                .tab_content(TabId::from_raw(tab_id));
            let _ = respond.send(content);
        }
        mcp_server::EditorCommand::GetCursorPosition { tab_id, respond } => {
            let position = doc_manager
                .session
                .borrow()
                .cursor_position(TabId::from_raw(tab_id))
                .map(|(line, column)| mcp_server::CursorPosition { line, column });
            let _ = respond.send(position);
        }
        mcp_server::EditorCommand::BufferContentForPath { path, respond } => {
            let content = doc_manager
                .session
                .borrow()
                .content_for_path(std::path::Path::new(&path));
            let _ = respond.send(content);
        }
        mcp_server::EditorCommand::OpenFile { path, respond } => {
            // Reuses the openFile invokable's own body verbatim (path
            // translation, session call, tabOpened emission on a new tab)
            // rather than duplicating it — MCP and the UI's "Open File"
            // dialog end up on the exact same path.
            let result = doc_manager
                .as_mut()
                .open_file(&QString::from(path.as_str()));
            let mapped = if result.code == 0 {
                Ok(result.tab_id)
            } else {
                Err(result.message.to_string())
            };
            let _ = respond.send(mapped);
        }
        mcp_server::EditorCommand::EditBuffer {
            tab_id,
            content,
            respond,
        } => {
            let result = doc_manager
                .session
                .borrow_mut()
                .edit_tab(TabId::from_raw(tab_id), &content);
            let mapped = result.map_err(|err| err.to_string());
            if mapped.is_ok() {
                // Not tab_modified_changed too: the widget's own
                // modificationChanged forwarding (installed in onTabOpened)
                // already emits it once onBufferEditedExternally calls
                // setModified(true) on the widget — one path, not two.
                doc_manager
                    .as_mut()
                    .buffer_edited_externally(tab_id, QString::from(content.as_str()));
            }
            let _ = respond.send(mapped);
        }
        mcp_server::EditorCommand::SaveBuffer { tab_id, respond } => {
            let result = doc_manager
                .session
                .borrow_mut()
                .save_buffer(TabId::from_raw(tab_id));
            let mapped = result.map_err(|err| err.to_string());
            if mapped.is_ok() {
                doc_manager.as_mut().tab_modified_changed(tab_id, false);
            }
            let _ = respond.send(mapped);
        }
    }
}

/// Every edit of a plan as the view receives them, with the pile each
/// belongs to already decided (`lsp_core::plan_edit`).
pub(crate) fn to_ffi_edits(
    plan: &lsp_core::EditPlan,
    excluded: &[String],
) -> Vec<ffi::FfiTextEdit> {
    let documents = plan
        .buffers
        .iter()
        .map(|doc| (true, doc))
        .chain(plan.files.iter().map(|doc| (false, doc)));
    documents
        .filter(|(_, doc)| !excluded.contains(&doc.path))
        .flat_map(|(in_buffer, doc)| {
            doc.edits.iter().map(move |edit| ffi::FfiTextEdit {
                path: QString::from(doc.path.as_str()),
                in_buffer,
                start_line: edit.start_line,
                start_character: edit.start_character,
                end_line: edit.end_line,
                end_character: edit.end_character,
                new_text: QString::from(edit.new_text.as_str()),
            })
        })
        .collect()
}

/// `index_core`'s refusal as the code the view branches on.
pub(crate) fn to_ffi_refusal(refusal: &index_core::RenameRefusal) -> ffi::FfiRenameRefusal {
    match refusal {
        index_core::RenameRefusal::Unresolved => ffi::FfiRenameRefusal::Unresolved,
        index_core::RenameRefusal::InvalidName => ffi::FfiRenameRefusal::InvalidName,
        index_core::RenameRefusal::UnsavedChanges => ffi::FfiRenameRefusal::UnsavedChanges,
        index_core::RenameRefusal::NoSites => ffi::FfiRenameRefusal::NoSites,
    }
}

/// The kind word `index_core` recorded, or "symbol" for an occurrence with
/// no `tags.scm` entry of its own.
pub(crate) fn symbol_kind_word(kind: Option<syntax_core::SymbolKind>) -> &'static str {
    match kind {
        Some(syntax_core::SymbolKind::Class) => "class",
        Some(syntax_core::SymbolKind::Struct) => "struct",
        Some(syntax_core::SymbolKind::Enum) => "enum",
        Some(syntax_core::SymbolKind::Interface) => "interface",
        Some(syntax_core::SymbolKind::Method) => "method",
        Some(syntax_core::SymbolKind::Function) => "function",
        Some(syntax_core::SymbolKind::Field) => "field",
        Some(syntax_core::SymbolKind::Constant) => "constant",
        Some(syntax_core::SymbolKind::Property) => "property",
        Some(syntax_core::SymbolKind::Constructor) => "constructor",
        Some(syntax_core::SymbolKind::EnumMember) => "enum member",
        None => "symbol",
    }
}

pub(crate) fn load_settings() -> app_config::Settings {
    app_config::load(&app_core::resolve_config_dir()).unwrap_or_default()
}

/// The root of the open project, if there is one.
///
/// Shared rather than private to one feature module because three of them
/// now need it — the run configurations, the project settings layer and the
/// index's excludes — and they must all agree on which project is open.
pub(crate) fn current_project_root() -> Option<std::path::PathBuf> {
    crate::bridge::registry::shared_session()
        .borrow()
        .root_path()
        .map(std::path::Path::to_path_buf)
}

/// The open project's own settings layer, or an empty one when no project is
/// open or its file could not be read.
///
/// A malformed project file resolves to "overrides nothing" *here*, at the
/// adapter, rather than in `app-config`, which reports the error properly
/// (ADR-0022 §6). The settings dialog is the surface that shows that error;
/// a keystroke asking for its tab width is not.
pub(crate) fn load_project_settings() -> app_config::project_settings::ProjectSettings {
    current_project_root()
        .and_then(|root| app_config::project_settings::load(&root).ok())
        .unwrap_or_default()
}

/// The settings actually in force: the global layer with the open project's
/// overrides applied (ADR-0022).
///
/// Every consumer of a project-scoped setting reads *this*, never
/// [`load_settings`] — that one is still correct for the person-shaped
/// settings a project may not touch (theme, fonts, keymap, AI providers),
/// and for the pages that edit the global file itself.
pub(crate) fn load_resolved_settings() -> app_config::Settings {
    settings_model::scope::resolve(&load_settings(), &load_project_settings())
}

/// The `TabKind` hint `AppSession::open_file_with_hint` needs for `path`,
/// resolved from the file-association rules in force (issue #258).
///
/// `app-core` may not depend on `settings-model` (ADR-0017: settings-model
/// owns what a value means, and it sits above `app-core`'s layer), so the
/// resolution happens here and the answer crosses as a `TabKind` — the same
/// "adapter maps one type onto the other and decides nothing else" split
/// `FileOp`/`ResourceOp` already use (ADR-0029). `None` (no rule, user or
/// built-in, matched the path) is passed straight through: `open_file_with_hint`
/// treats it exactly like `open_file` always has, running the binary sniff.
pub(crate) fn resolve_open_hint(path: &Path) -> Option<app_core::TabKind> {
    let handler =
        settings_model::file_associations::resolve_handler(&load_resolved_settings(), path)?;
    Some(match handler {
        settings_model::file_associations::HandlerKind::Text => app_core::TabKind::Text,
        settings_model::file_associations::HandlerKind::Image => app_core::TabKind::Image,
        settings_model::file_associations::HandlerKind::Binary => app_core::TabKind::Binary,
    })
}

/// Same as [`load_resolved_settings`], but for an explicitly given project
/// root rather than whatever `shared_session` currently has open.
///
/// Needed off the Qt thread: `shared_session` (and so
/// [`current_project_root`]/[`load_project_settings`]) is a `thread_local`,
/// sound only because every QObject and every slot/signal lives on the one
/// Qt thread — reading it from a worker thread would silently construct a
/// second, empty `AppSession` on that thread instead of erroring. A worker
/// that already knows which project it is opening (it was handed the root)
/// has no need to ask the shared session anyway.
pub(crate) fn load_resolved_settings_for(root: &Path) -> app_config::Settings {
    let project_settings = app_config::project_settings::load(root).unwrap_or_default();
    settings_model::scope::resolve(&load_settings(), &project_settings)
}

/// `editor_core::diff::Hunk`s as `DiffView`'s change ribbon reads them
/// (F3-13). Shared by the refactor-preview and Replace-in-Files diff
/// panels — both hand `editor_core` hunks to the same widget.
pub(crate) fn to_ffi_hunks(hunks: &[editor_core::diff::Hunk]) -> Vec<ffi::FfiHunk> {
    hunks
        .iter()
        .map(|hunk| ffi::FfiHunk {
            old_start: hunk.old.start as u32,
            old_len: (hunk.old.end - hunk.old.start) as u32,
            new_start: hunk.new.start as u32,
            new_len: (hunk.new.end - hunk.new.start) as u32,
            kind: match hunk.kind {
                editor_core::diff::HunkKind::Added => ffi::FfiHunkKind::Added,
                editor_core::diff::HunkKind::Removed => ffi::FfiHunkKind::Removed,
                editor_core::diff::HunkKind::Modified => ffi::FfiHunkKind::Modified,
            },
        })
        .collect()
}

/// Intra-line spans for every modified hunk in `hunks`, as `DiffView`'s
/// `QTextEdit::ExtraSelection`s read them (F3-13): `start`/`end` are UTF-16
/// code units into the named line, matching `FfiTextEdit`'s convention.
///
/// `editor_core::diff::diff_inline` answers in byte offsets, which is the
/// right unit for slicing `old_text`/`new_text` themselves but the wrong one
/// for a `QString`; the conversion happens once, here, rather than in every
/// caller that would otherwise get it wrong the way `apply_to_text`'s own
/// doc comment warns about.
pub(crate) fn to_ffi_inline_spans(
    old_text: &str,
    new_text: &str,
    hunks: &[editor_core::diff::Hunk],
    mode: editor_core::diff::HighlightMode,
) -> Vec<ffi::FfiInlineSpan> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    let mut out = Vec::new();
    for hunk in hunks {
        let inline = editor_core::diff::diff_inline_opts(old_text, new_text, hunk, mode);
        for span in &inline.removed {
            let Some(line) = old_lines.get(span.line) else {
                continue;
            };
            out.push(ffi::FfiInlineSpan {
                side: ffi::FfiDiffSide::Old,
                line: span.line as u32,
                start: utf16_offset(line, span.range.start),
                end: utf16_offset(line, span.range.end),
            });
        }
        for span in &inline.added {
            let Some(line) = new_lines.get(span.line) else {
                continue;
            };
            out.push(ffi::FfiInlineSpan {
                side: ffi::FfiDiffSide::New,
                line: span.line as u32,
                start: utf16_offset(line, span.range.start),
                end: utf16_offset(line, span.range.end),
            });
        }
    }
    out
}

/// An `FfiHunk` back to the `editor_core` hunk it was built from — the
/// apply chevron hands the view's hunk back across the seam.
pub(crate) fn from_ffi_hunk(hunk: &ffi::FfiHunk) -> editor_core::diff::Hunk {
    let old = hunk.old_start as usize..(hunk.old_start + hunk.old_len) as usize;
    let new = hunk.new_start as usize..(hunk.new_start + hunk.new_len) as usize;
    let kind = match hunk.kind {
        ffi::FfiHunkKind::Added => editor_core::diff::HunkKind::Added,
        ffi::FfiHunkKind::Removed => editor_core::diff::HunkKind::Removed,
        // A cxx enum is an open integer; anything the view never sends
        // reads as the kind both ranges being non-empty would imply.
        _ => editor_core::diff::HunkKind::Modified,
    };
    editor_core::diff::Hunk { old, new, kind }
}

pub(crate) fn to_whitespace_mode(
    mode: ffi::FfiWhitespaceMode,
) -> editor_core::diff::WhitespaceMode {
    match mode {
        ffi::FfiWhitespaceMode::Exact => editor_core::diff::WhitespaceMode::Exact,
        ffi::FfiWhitespaceMode::TrimEnds => editor_core::diff::WhitespaceMode::TrimEnds,
        ffi::FfiWhitespaceMode::IgnoreAll => editor_core::diff::WhitespaceMode::IgnoreAll,
        ffi::FfiWhitespaceMode::IgnoreAllAndBlankLines => {
            editor_core::diff::WhitespaceMode::IgnoreAllAndBlankLines
        }
        _ => editor_core::diff::WhitespaceMode::Exact,
    }
}

pub(crate) fn to_highlight_mode(mode: ffi::FfiHighlightMode) -> editor_core::diff::HighlightMode {
    match mode {
        ffi::FfiHighlightMode::Words => editor_core::diff::HighlightMode::Words,
        ffi::FfiHighlightMode::Chars => editor_core::diff::HighlightMode::Chars,
        ffi::FfiHighlightMode::Lines => editor_core::diff::HighlightMode::Lines,
        ffi::FfiHighlightMode::None => editor_core::diff::HighlightMode::None,
        _ => editor_core::diff::HighlightMode::Words,
    }
}

/// `editor_core::diff::DiffRow`s as the view reads them; an absent line is
/// `-1` because cxx has no `Option`.
pub(crate) fn to_ffi_rows(rows: &[editor_core::diff::DiffRow]) -> Vec<ffi::FfiDiffRow> {
    let line = |l: Option<usize>| l.map_or(-1, |l| l as i32);
    rows.iter()
        .map(|row| ffi::FfiDiffRow {
            old_line: line(row.old),
            new_line: line(row.new),
            kind: match row.kind {
                editor_core::diff::RowKind::Context => ffi::FfiRowKind::Context,
                editor_core::diff::RowKind::Added => ffi::FfiRowKind::Added,
                editor_core::diff::RowKind::Removed => ffi::FfiRowKind::Removed,
            },
            old_anchor: row.old_anchor as u32,
            new_anchor: row.new_anchor as u32,
        })
        .collect()
}

/// The UTF-16 code-unit count of `line[..byte_offset]`. `byte_offset` is
/// always one `diff_inline` produced, so it is always a char boundary.
fn utf16_offset(line: &str, byte_offset: usize) -> u32 {
    line[..byte_offset.min(line.len())]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_ffi_symbol_kind_covers_every_variant() {
        // A `matches!` per variant rather than `==` (cxx-qt shared enums
        // do not derive `PartialEq`): this is the exhaustive-match
        // itself's regression guard — a new `SymbolKind` variant that
        // `to_ffi_symbol_kind` forgets to map is a compile error there,
        // but a variant mapped to the *wrong* `FfiSymbolKind` compiles
        // fine and would only show up here.
        assert!(matches!(
            to_ffi_symbol_kind(syntax_core::SymbolKind::Constant),
            ffi::FfiSymbolKind::Constant
        ));
        assert!(matches!(
            to_ffi_symbol_kind(syntax_core::SymbolKind::Property),
            ffi::FfiSymbolKind::Property
        ));
        assert!(matches!(
            to_ffi_symbol_kind(syntax_core::SymbolKind::Constructor),
            ffi::FfiSymbolKind::Constructor
        ));
        assert!(matches!(
            to_ffi_symbol_kind(syntax_core::SymbolKind::EnumMember),
            ffi::FfiSymbolKind::EnumMember
        ));
    }

    #[test]
    fn diff_mode_conversions_cover_every_variant() {
        use editor_core::diff::{HighlightMode, WhitespaceMode};
        assert_eq!(
            to_whitespace_mode(ffi::FfiWhitespaceMode::Exact),
            WhitespaceMode::Exact
        );
        assert_eq!(
            to_whitespace_mode(ffi::FfiWhitespaceMode::TrimEnds),
            WhitespaceMode::TrimEnds
        );
        assert_eq!(
            to_whitespace_mode(ffi::FfiWhitespaceMode::IgnoreAll),
            WhitespaceMode::IgnoreAll
        );
        assert_eq!(
            to_whitespace_mode(ffi::FfiWhitespaceMode::IgnoreAllAndBlankLines),
            WhitespaceMode::IgnoreAllAndBlankLines
        );
        assert_eq!(
            to_highlight_mode(ffi::FfiHighlightMode::Words),
            HighlightMode::Words
        );
        assert_eq!(
            to_highlight_mode(ffi::FfiHighlightMode::Chars),
            HighlightMode::Chars
        );
        assert_eq!(
            to_highlight_mode(ffi::FfiHighlightMode::Lines),
            HighlightMode::Lines
        );
        assert_eq!(
            to_highlight_mode(ffi::FfiHighlightMode::None),
            HighlightMode::None
        );
    }

    #[test]
    fn a_hunk_survives_the_round_trip_across_the_seam() {
        let hunks = editor_core::diff::diff_lines("a\nb\nc\n", "a\nB\nB2\nc\n").unwrap();
        let ffi_hunks = to_ffi_hunks(&hunks);
        assert_eq!(from_ffi_hunk(&ffi_hunks[0]), hunks[0]);
    }

    #[test]
    fn rows_cross_the_seam_with_minus_one_for_an_absent_line() {
        let hunks = editor_core::diff::diff_lines("a\nb\n", "a\n").unwrap();
        let rows = to_ffi_rows(&editor_core::diff::diff_rows(2, 1, &hunks));
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0].kind, ffi::FfiRowKind::Context));
        assert!(matches!(rows[1].kind, ffi::FfiRowKind::Removed));
        assert_eq!(rows[1].old_line, 1);
        assert_eq!(rows[1].new_line, -1);
        assert_eq!(rows[1].new_anchor, 0);
    }

    #[test]
    fn inline_spans_follow_the_highlight_mode() {
        let before = "alpha\n";
        let after = "alpXa\n";
        let hunks = editor_core::diff::diff_lines(before, after).unwrap();
        let words = to_ffi_inline_spans(
            before,
            after,
            &hunks,
            editor_core::diff::HighlightMode::Words,
        );
        assert_eq!((words[0].start, words[0].end), (0, 5));
        let chars = to_ffi_inline_spans(
            before,
            after,
            &hunks,
            editor_core::diff::HighlightMode::Chars,
        );
        assert_eq!((chars[0].start, chars[0].end), (3, 4));
        assert!(to_ffi_inline_spans(
            before,
            after,
            &hunks,
            editor_core::diff::HighlightMode::Lines
        )
        .is_empty());
    }

    #[test]
    fn to_ffi_symbol_category_covers_every_variant() {
        assert!(matches!(
            to_ffi_symbol_category(syntax_core::SymbolCategory::Constants),
            ffi::FfiSymbolCategory::Constants
        ));
        assert!(matches!(
            to_ffi_symbol_category(syntax_core::SymbolCategory::Constructors),
            ffi::FfiSymbolCategory::Constructors
        ));
        assert!(matches!(
            to_ffi_symbol_category(syntax_core::SymbolCategory::NestedTypes),
            ffi::FfiSymbolCategory::NestedTypes
        ));
        assert!(matches!(
            to_ffi_symbol_category(syntax_core::SymbolCategory::Other),
            ffi::FfiSymbolCategory::Other
        ));
    }

    #[test]
    fn flatten_symbol_tree_records_depth_and_derives_category_from_kind() {
        let tree = vec![syntax_core::SymbolNode {
            name: "Point".to_owned(),
            kind: syntax_core::SymbolKind::Class,
            start: 0,
            end: 40,
            name_start: 6,
            name_end: 11,
            children: vec![syntax_core::SymbolNode {
                name: "x".to_owned(),
                kind: syntax_core::SymbolKind::Property,
                start: 16,
                end: 22,
                name_start: 16,
                name_end: 17,
                children: vec![],
            }],
        }];
        let mut out = Vec::new();
        flatten_symbol_tree(&tree, 0, &mut out);

        assert_eq!(out.len(), 2, "root + one nested child");
        assert_eq!(out[0].name.to_string(), "Point");
        assert_eq!(out[0].depth, 0);
        assert!(matches!(out[0].kind, ffi::FfiSymbolKind::Class));
        assert!(matches!(
            out[0].category,
            ffi::FfiSymbolCategory::NestedTypes
        ));

        assert_eq!(out[1].name.to_string(), "x");
        assert_eq!(out[1].depth, 1);
        assert!(matches!(out[1].kind, ffi::FfiSymbolKind::Property));
        assert!(matches!(
            out[1].category,
            ffi::FfiSymbolCategory::Properties
        ));
    }
}
