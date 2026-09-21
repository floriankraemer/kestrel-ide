//! Rust side of the `EditorOps` QObject (task F1-13): the seam the editor
//! ergonomics reach the view through.
//!
//! # Why the buffer text is a parameter of every slot
//!
//! `editor_core::Document`'s rope is populated when a file is opened and
//! refreshed only on save, so it is one save behind what the user can see.
//! Every entry point below therefore takes the *live* buffer text the way
//! `findMatches` and `replacementEdits` already do, and computes against
//! that. Nothing here reads the rope.
//!
//! # Units
//!
//! Carets and edits are byte offsets in Rust, because tree-sitter is
//! byte-addressed and `edit-ops` speaks bytes. They cross the seam in
//! UTF-16 code units, because that is what `QTextCursor` counts:
//!
//! * carets as [`ffi::FfiCaret`], flat document positions — the same unit
//!   every existing `CodeEditor` signal already carries;
//! * edits as [`ffi::FfiTextEdit`], 0-based line plus UTF-16 character, so
//!   `EditorTabs::applyBufferEdits` splices them inside one `beginEditBlock`
//!   and one Ctrl+Z undoes the whole thing (ADR-0023).
//!
//! The conversion happens here and nowhere else, through
//! `editor_core::offsets`.
//!
//! # Why the caret state lives in this object
//!
//! Next-occurrence, the expand/shrink stack and the auto-close pair tracker
//! are all *stateful*, and none of them belongs on `Document` (stale) or in
//! `AppSession` (which knows nothing about carets). One entry per open tab
//! lives here, in the one object that reads and writes it, and the view
//! drops an entry with `forgetTab` when the tab closes.

use core::pin::Pin;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use cxx_qt_lib::QString;

use crate::bridge::errors;

use editor_core::line_ops;
use editor_core::offsets::{line_of, line_range, line_starts, Utf16Cursor};
use editor_core::selection::{Caret, CaretMotion, SelectionError, SelectionSet};
use editor_core::transaction::{map_carets, Transaction};

use edit_ops::indent::IndentStyle;
use edit_ops::pairs::{PairTracker, TypeEdit};
use edit_ops::selection_expand::SelectionHistory;
use syntax_core::Language;

use crate::bridge::ffi::{self, FfiResult};
use crate::bridge::registry::shared_session;

/// Which line operation `lineOp` was asked for. Mirrored as plain integers
/// across the seam because cxx shared enums would be a second FFI type for
/// what is one menu's worth of choices; the view passes the constant its
/// action was registered with and decides nothing.
const LINE_OP_DUPLICATE: u8 = 0;
const LINE_OP_MOVE_UP: u8 = 1;
const LINE_OP_MOVE_DOWN: u8 = 2;
const LINE_OP_DELETE: u8 = 3;
const LINE_OP_JOIN: u8 = 4;

/// Which motion `moveCarets` was asked for, in the order `CodeEditor`
/// declares them — the ADR-0023 follow-up: every caret moves instead of
/// collapsing to the primary one.
const MOTION_LEFT: u8 = 0;
const MOTION_RIGHT: u8 = 1;
const MOTION_UP: u8 = 2;
const MOTION_DOWN: u8 = 3;
const MOTION_HOME: u8 = 4;
const MOTION_END: u8 = 5;
const MOTION_WORD_LEFT: u8 = 6;
const MOTION_WORD_RIGHT: u8 = 7;

/// Everything one open tab remembers between gestures.
struct TabOps {
    selection: SelectionSet,
    /// The expand/shrink stack, so Ctrl+Shift+W retraces exactly the path
    /// Ctrl+W took rather than guessing a smaller node.
    history: SelectionHistory,
    /// Which closers auto-close inserted, so type-over applies to those and
    /// only those (F1-8). Cleared by any edit that did not come from this
    /// tracker — a line operation, a comment toggle, an intention — since a
    /// stale tracked offset would type over a character somebody else put
    /// there.
    pairs: PairTracker,
    /// R2: the snippet just-accepted-completion left this tab in, if any.
    /// `None` outside of one, which is every tab most of the time.
    snippet: Option<editor_core::SnippetSession>,
}

impl Default for TabOps {
    /// A tab no gesture has touched yet has one caret at the start of the
    /// file — the widget overwrites it with `setCarets` on the first caret
    /// move, which happens before any edit can.
    fn default() -> Self {
        Self {
            selection: SelectionSet::single(Caret::at(0)),
            history: SelectionHistory::new(),
            pairs: PairTracker::new(),
            snippet: None,
        }
    }
}

/// [`ffi::FfiSnippetStop`] for "no stop" — no active session, or a move
/// that had nowhere to go.
fn no_snippet_stop() -> ffi::FfiSnippetStop {
    ffi::FfiSnippetStop::default()
}

/// The snippet's own doc comment on [`ffi::FfiSnippetStop`] says what
/// `start`/`end` are here: absolute document positions, already computed by
/// the caller.
fn snippet_stop(range: std::ops::Range<usize>, more: bool) -> ffi::FfiSnippetStop {
    ffi::FfiSnippetStop {
        has_stop: true,
        start: range.start as u32,
        end: range.end as u32,
        more,
    }
}

/// Rust side of the `EditorOps` QObject.
///
/// `settings` is a cached copy rather than a read per call: `toggleComment`
/// and friends need the effective tab width, and re-reading `settings.toml`
/// on every keystroke would put a file read on the typing path. The settings
/// dialog calls `reloadSettings` when it commits.
pub struct EditorOpsRust {
    tabs: RefCell<HashMap<u64, TabOps>>,
    settings: RefCell<app_config::Settings>,
    /// Only ever read, and only for one thing: which language a tab's file
    /// is, so the grammar-aware operations know which grammar.
    session: Rc<RefCell<app_core::AppSession>>,
}

impl Default for EditorOpsRust {
    fn default() -> Self {
        Self {
            tabs: RefCell::new(HashMap::new()),
            settings: RefCell::new(crate::bridge::convert::load_resolved_settings()),
            session: shared_session(),
        }
    }
}

/// A refusal the view has to show, with the adapter's own code (ADR-0003 §4,
/// range 1000–1099): nothing here belongs to a domain crate, and nothing
/// branches on *which* refusal it was — the message says it in full.
fn refused(message: &str) -> FfiResult {
    errors::failure(errors::CODE_REFUSED, message)
}

fn ok() -> FfiResult {
    FfiResult::default()
}

/// A selection refusal keeps `editor-core`'s own code (900–999) rather than
/// being flattened into the adapter's: the rule that said no lives there.
fn to_ffi_selection_error(error: SelectionError) -> FfiResult {
    FfiResult {
        code: error.code(),
        message: QString::from(error.to_string().as_str()),
    }
}

/// Byte offsets to flat document UTF-16 positions, in one forward pass.
///
/// The offsets are visited in ascending order because `Utf16Cursor` only
/// moves forward; the answers are put back in the caller's order.
fn to_utf16(text: &str, offsets: &[usize]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..offsets.len()).collect();
    order.sort_by_key(|i| offsets[*i]);
    let mut cursor = Utf16Cursor::new(text);
    let mut out = vec![0usize; offsets.len()];
    for i in order {
        out[i] = cursor.utf16_at(offsets[i]);
    }
    out
}

/// Flat document UTF-16 positions to byte offsets, in one forward pass —
/// the inverse of [`to_utf16`], and written out because
/// `editor_core::offsets::byte_offset` rescans from the start each call and
/// a column selection asks for a thousand of them at once.
fn to_bytes(text: &str, positions: &[usize]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..positions.len()).collect();
    order.sort_by_key(|i| positions[*i]);
    let mut out = vec![0usize; positions.len()];
    let mut chars = text.char_indices();
    let (mut byte, mut utf16) = (0usize, 0usize);
    for i in order {
        while utf16 < positions[i] {
            let Some((at, ch)) = chars.next() else {
                byte = text.len();
                break;
            };
            byte = at + ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        out[i] = byte.min(text.len());
    }
    out
}

/// The visual column of `offset` on its own line, counting a tab as the
/// distance to the next tab stop — the same rule `column_block` applies, so
/// a drag and the block it produces agree.
fn visual_column(text: &str, starts: &[usize], offset: usize, tab_width: usize) -> (usize, usize) {
    let line = line_of(starts, offset);
    let range = line_range(text, starts, line);
    let mut column = 0usize;
    for ch in text[range.start..offset.clamp(range.start, range.end)].chars() {
        if ch == '\t' {
            column += tab_width - (column % tab_width);
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// The language of the file a tab holds, for the grammar-aware operations.
fn language_of(session: &app_core::AppSession, tab_id: u64) -> Language {
    let name = session
        .tab_file_name(app_core::TabId::from_raw(tab_id))
        .unwrap_or_default();
    syntax_core::language_for_path(Path::new(&name))
}

impl EditorOpsRust {
    /// The editing rules in force for a tab's language — tab width and
    /// spaces-vs-tabs resolved through `settings-model`, which owns the
    /// question of what a language may override.
    fn indent_style(&self, language: Language) -> IndentStyle {
        let settings = self.settings.borrow();
        let rules = settings_model::editing::resolve_for_language(&settings, &language.id());
        rules.indent_style()
    }

    fn tab_width(&self, language: Language) -> usize {
        self.indent_style(language).tab_width.max(1)
    }

    /// The save rules in force for a tab's language — trim, final newline
    /// and line-ending policy (F1-11), through the same resolution the
    /// indent style already uses.
    fn save_rules(&self, language: Language) -> editor_core::save_rules::SaveRules {
        let settings = self.settings.borrow();
        let mut rules =
            settings_model::editing::resolve_for_language(&settings, &language.id()).save_rules();
        // W6 (line endings, ADR-0052): `on_save`'s `LineEnding::platform()`
        // fallback only ever fires for a file with no existing line ending
        // to preserve (a brand-new empty file) — every other file already
        // keeps its own endings regardless of host. That one fallback is
        // `cfg!(windows)`, which is wrong for a WSL project on a Windows
        // build of this IDE: the file lives inside a Linux distro and wants
        // LF, not the compiled-for platform's CRLF.
        if rules.line_endings.is_none() {
            let is_remote = crate::bridge::convert::current_project_root()
                .map(|root| lsp_core::ExecHost::for_path(&root).is_remote())
                .unwrap_or(false);
            if is_remote {
                rules.line_endings = Some(editor_core::save_rules::LineEnding::Lf);
            }
        }
        rules
    }

    /// The selection a tab is on, or a single caret at the start for a tab
    /// no gesture has touched yet.
    fn selection_of(&self, tab_id: u64) -> SelectionSet {
        self.tabs
            .borrow()
            .get(&tab_id)
            .map(|ops| ops.selection.clone())
            .unwrap_or_else(|| SelectionSet::single(Caret::at(0)))
    }

    fn store_selection(&self, tab_id: u64, selection: SelectionSet) {
        self.tabs.borrow_mut().entry(tab_id).or_default().selection = selection;
    }

    /// The edits of `transaction` as the view receives them, **descending**.
    ///
    /// Descending is load-bearing: `applyBufferEdits` re-resolves each
    /// (line, character) pair against the document as it splices, so an
    /// ascending list would have every edit after the first land at a
    /// position its predecessor already moved.
    fn to_ffi_edits(&self, text: &str, transaction: &Transaction) -> Vec<ffi::FfiTextEdit> {
        let starts = line_starts(text);
        let mut edits: Vec<&editor_core::transaction::TextEdit> =
            transaction.edits.iter().collect();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));

        let mut out = Vec::with_capacity(edits.len());
        // One cursor per edit rather than one for the batch: the list is
        // descending, and `Utf16Cursor` only walks forward.
        for edit in edits {
            let (start_line, start_character) = position_at(text, &starts, edit.range.start);
            let (end_line, end_character) = position_at(text, &starts, edit.range.end);
            out.push(ffi::FfiTextEdit {
                path: QString::default(),
                in_buffer: true,
                start_line,
                start_character,
                end_line,
                end_character,
                new_text: QString::from(edit.text.as_str()),
            });
        }
        out
    }

    /// Apply `transaction` to the tab: move its carets through the edit and
    /// hand the view the splice list. An empty transaction changes nothing
    /// and returns nothing, which is what a line operation with nothing to
    /// do produces.
    fn commit(&self, tab_id: u64, text: &str, transaction: Transaction) -> Vec<ffi::FfiTextEdit> {
        if transaction.is_empty() {
            return Vec::new();
        }
        let selection = self.selection_of(tab_id);
        let moved = map_carets(&selection, &transaction);
        {
            let mut tabs = self.tabs.borrow_mut();
            let ops = tabs.entry(tab_id).or_default();
            ops.selection = moved;
            // Any edit invalidates the expand/shrink path — the nodes it
            // recorded are no longer the nodes in the text — and the pair
            // tracker, whose whole premise is that it saw every edit since
            // the closers it is tracking were inserted.
            ops.history.clear();
            ops.pairs.invalidate();
        }
        self.to_ffi_edits(text, &transaction)
    }

    /// The commit path for a [`TypeEdit`]: `edit-ops::pairs` already worked
    /// out where the carets land — after inserting `()` the caret belongs
    /// *between* the two characters, which `map_carets` cannot know — so
    /// its answer is taken as given rather than recomputed. The tracker's
    /// own state is left alone; it already updated itself.
    fn commit_typed(&self, tab_id: u64, text: &str, edit: TypeEdit) -> Vec<ffi::FfiTextEdit> {
        {
            let mut tabs = self.tabs.borrow_mut();
            let ops = tabs.entry(tab_id).or_default();
            ops.selection = edit.selection;
            ops.history.clear();
        }
        if edit.transaction.is_empty() {
            return Vec::new();
        }
        self.to_ffi_edits(text, &edit.transaction)
    }
}

/// A byte offset as the protocol's (0-based line, UTF-16 character).
fn position_at(text: &str, starts: &[usize], offset: usize) -> (u32, u32) {
    let line = line_of(starts, offset);
    let range = line_range(text, starts, line);
    let mut cursor = Utf16Cursor::new(&text[range.start..]);
    let character = cursor.utf16_at(offset.clamp(range.start, range.end) - range.start);
    (line as u32, character as u32)
}

impl ffi::EditorOps {
    /// Replace what this tab's carets are with what the widget has. Called
    /// on every caret move, so the single-caret case stays exactly as cheap
    /// as it was: one entry, no allocation beyond the vector.
    pub fn set_carets(
        self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        carets: Vec<ffi::FfiCaret>,
    ) {
        let text = text.to_string();
        let flat: Vec<usize> = carets
            .iter()
            .flat_map(|c| [c.anchor as usize, c.head as usize])
            .collect();
        let bytes = to_bytes(&text, &flat);
        let primary = carets.iter().position(|caret| caret.primary).unwrap_or(0);
        let set = SelectionSet::from_carets(
            bytes
                .chunks(2)
                .map(|pair| Caret::new(pair[0], pair[1]))
                .collect(),
            primary,
        );
        if let Ok(set) = set {
            self.store_selection(tab_id, set);
        }
    }

    /// Where this tab's carets are, for the widget to paint.
    pub fn carets(&self, tab_id: u64, text: &QString) -> Vec<ffi::FfiCaret> {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        let flat: Vec<usize> = selection
            .carets()
            .iter()
            .flat_map(|c| [c.anchor, c.head])
            .collect();
        let utf16 = to_utf16(&text, &flat);
        let primary = selection.primary_index();
        utf16
            .chunks(2)
            .enumerate()
            .map(|(index, pair)| ffi::FfiCaret {
                anchor: pair[0] as u32,
                head: pair[1] as u32,
                primary: index == primary,
            })
            .collect()
    }

    /// How many carets this tab has. The widget branches on `> 1` to decide
    /// whether a keystroke goes through Rust at all — the only branch it is
    /// allowed, because it is about which code path runs, not about what an
    /// edit means.
    pub fn caret_count(&self, tab_id: u64) -> u32 {
        self.selection_of(tab_id).len() as u32
    }

    /// The primary caret's byte offset — already the unit `SelectionSet`
    /// stores internally (this module's own doc comment on why: bytes,
    /// because tree-sitter/`edit-ops` are byte-addressed), so this needs no
    /// conversion the way `carets()` does for the view.
    pub fn caret_offset(&self, tab_id: u64) -> i64 {
        self.selection_of(tab_id).primary().head as i64
    }

    /// Esc: back to one caret.
    pub fn clear_secondary_carets(mut self: Pin<&mut Self>, tab_id: u64) {
        let mut selection = self.selection_of(tab_id);
        selection.collapse_to_primary();
        self.store_selection(tab_id, selection);
        self.as_mut().carets_changed(tab_id);
    }

    /// The tab closed; forget everything about it. Without this the map
    /// would grow for the life of the process.
    pub fn forget_tab(self: Pin<&mut Self>, tab_id: u64) {
        self.tabs.borrow_mut().remove(&tab_id);
    }

    /// Re-read the settings this object caches. Called when the settings
    /// dialog commits, so a changed tab width takes effect without a
    /// restart.
    pub fn reload_settings(self: Pin<&mut Self>) {
        *self.settings.borrow_mut() = crate::bridge::convert::load_resolved_settings();
    }

    /// Alt+Click: one more caret at `position`.
    pub fn add_caret_at(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        position: u32,
    ) -> FfiResult {
        let text = text.to_string();
        let at = to_bytes(&text, &[position as usize])[0];
        let mut selection = self.selection_of(tab_id);
        if let Err(error) = selection.add_caret(Caret::at(at)) {
            return to_ffi_selection_error(error);
        }
        self.store_selection(tab_id, selection);
        self.as_mut().carets_changed(tab_id);
        ok()
    }

    /// Ctrl+Alt+Up / Ctrl+Alt+Down: a caret on the neighbouring line, at the
    /// same visual column, clipped to that line's length.
    pub fn add_caret_vertically(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        downwards: bool,
    ) -> FfiResult {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        let language = language_of(&self.session.borrow(), tab_id);
        let width = self.tab_width(language);

        let starts = line_starts(&text);
        let from = selection.primary().head;
        let (line, column) = visual_column(&text, &starts, from, width);
        let Some(target) = (if downwards {
            (line + 1 < starts.len()).then(|| line + 1)
        } else {
            line.checked_sub(1)
        }) else {
            return refused("there is no line that way");
        };

        let block = match editor_core::selection::column_block(
            &text, target, column, target, column, width,
        ) {
            Ok(block) => block,
            Err(error) => return to_ffi_selection_error(error),
        };
        let mut selection = selection;
        if let Err(error) = selection.add_caret(block.primary()) {
            return to_ffi_selection_error(error);
        }
        self.store_selection(tab_id, selection);
        self.as_mut().carets_changed(tab_id);
        ok()
    }

    /// Ctrl+D: select the next occurrence of what the primary caret covers.
    pub fn select_next_occurrence(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
    ) -> FfiResult {
        let text = text.to_string();
        let mut selection = self.selection_of(tab_id);
        match selection.add_next_occurrence(&text) {
            Ok(true) => {
                self.store_selection(tab_id, selection);
                self.as_mut().carets_changed(tab_id);
                ok()
            }
            Ok(false) => refused("no further occurrence"),
            Err(error) => to_ffi_selection_error(error),
        }
    }

    /// Alt+Shift+drag: one caret per line between two document positions,
    /// at the visual columns those positions sit at.
    pub fn column_select(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        anchor: u32,
        head: u32,
    ) -> FfiResult {
        let text = text.to_string();
        let language = language_of(&self.session.borrow(), tab_id);
        let width = self.tab_width(language);
        let starts = line_starts(&text);
        let ends = to_bytes(&text, &[anchor as usize, head as usize]);
        let (anchor_line, anchor_col) = visual_column(&text, &starts, ends[0], width);
        let (head_line, head_col) = visual_column(&text, &starts, ends[1], width);
        match editor_core::selection::column_block(
            &text,
            anchor_line,
            anchor_col,
            head_line,
            head_col,
            width,
        ) {
            Ok(block) => {
                self.store_selection(tab_id, block);
                self.as_mut().carets_changed(tab_id);
                ok()
            }
            Err(error) => to_ffi_selection_error(error),
        }
    }

    /// Typing at every caret, as one transaction.
    ///
    /// A single character goes through `edit-ops::pairs`, which is what
    /// makes auto-close, type-over and surround happen — including with a
    /// plain letter, which the tracker treats identically to today. A
    /// longer string (an IME committing more than one character at once)
    /// is inserted plainly: pairing a composed run character-by-character
    /// would double-close a bracket the composition typed whole.
    pub fn type_text(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        typed: &QString,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let typed = typed.to_string();
        let mut chars = typed.chars();
        let edits = match (chars.next(), chars.next()) {
            (Some(ch), None) => {
                let language = language_of(&self.session.borrow(), tab_id);
                let selection = self.selection_of(tab_id);
                // A closer typed at a bare caret with nothing but whitespace
                // before it on the line dedents that line first (F1's
                // dedent-on-closer rule). This is a narrower ceiling than
                // the rest of this method's multi-caret reach — it only
                // looks at a single collapsed caret — in exchange for not
                // combining it with the pair tracker's own auto-close/
                // type-over transaction, which `edit_ops::pairs` was never
                // asked to compute against a text this rule has already
                // edited.
                if selection.len() == 1 && selection.primary().is_empty() {
                    let style = self.indent_style(language);
                    let offset = selection.primary().start();
                    if let Some(mut dedent) =
                        edit_ops::indent::dedent_closer(language, &text, offset, ch, style)
                    {
                        dedent
                            .edits
                            .push(editor_core::transaction::TextEdit::insert(
                                offset,
                                ch.to_string(),
                            ));
                        let edits = self.commit(tab_id, &text, dedent);
                        self.as_mut().carets_changed(tab_id);
                        return edits;
                    }
                }
                let type_edit = {
                    let mut tabs = self.tabs.borrow_mut();
                    let ops = tabs.entry(tab_id).or_default();
                    let selection = ops.selection.clone();
                    ops.pairs.type_char(language, &text, &selection, ch)
                };
                self.commit_typed(tab_id, &text, type_edit)
            }
            _ => {
                let selection = self.selection_of(tab_id);
                let transaction = Transaction::type_text(&selection, &typed);
                self.commit(tab_id, &text, transaction)
            }
        };
        self.as_mut().carets_changed(tab_id);
        edits
    }

    /// Backspace at every caret: a caret sitting between an opener and its
    /// own matching closer with nothing between them deletes both.
    pub fn backspace(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let language = language_of(&self.session.borrow(), tab_id);
        let type_edit = {
            let mut tabs = self.tabs.borrow_mut();
            let ops = tabs.entry(tab_id).or_default();
            let selection = ops.selection.clone();
            ops.pairs.backspace(language, &text, &selection)
        };
        let edits = self.commit_typed(tab_id, &text, type_edit);
        self.as_mut().carets_changed(tab_id);
        edits
    }

    /// Insert `pasted` verbatim at every caret. Its own slot rather than a
    /// long `typeText` string, because pasting `foo(bar` must not become
    /// `foo(bar)` — paste is not typing, and routing it through the same
    /// path as a keystroke is the mistake `edit_ops::pairs::paste` exists
    /// to rule out.
    pub fn paste_text(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        pasted: &QString,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let pasted = pasted.to_string();
        let type_edit = {
            let mut tabs = self.tabs.borrow_mut();
            let ops = tabs.entry(tab_id).or_default();
            let selection = ops.selection.clone();
            ops.pairs.paste(&selection, &pasted)
        };
        let edits = self.commit_typed(tab_id, &text, type_edit);
        self.as_mut().carets_changed(tab_id);
        edits
    }

    /// Delete at every caret.
    pub fn delete_forward(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        let transaction = Transaction::delete_forward(&text, &selection);
        let edits = self.commit(tab_id, &text, transaction);
        self.as_mut().carets_changed(tab_id);
        edits
    }

    /// Enter at every caret: the newline plus the indent the language wants
    /// at that point, or — when the caret sits between a bracket and its own
    /// closer with nothing between them — a three-line block instead
    /// (`edit_ops::indent::enter_between_pair`). The newline is `\n` here —
    /// normalising to the file's line ending is the save path's job
    /// (`editor_core::save_rules`).
    pub fn newline(mut self: Pin<&mut Self>, tab_id: u64, text: &QString) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        let language = language_of(&self.session.borrow(), tab_id);
        let style = self.indent_style(language);

        // `enter_between_pair`'s caret target sits *inside* what it
        // inserts, not at the end of it — the one case the generic
        // collapsed-caret mapping below (ride to the end of the insertion)
        // gets wrong. `pair_trim` is how much shorter such a caret's final
        // position is than that generic rule would put it, so it can be
        // corrected after `commit` maps every caret the usual way.
        let mut edits = Vec::with_capacity(selection.len());
        let mut pair_trim = vec![0usize; selection.len()];
        for (i, caret) in selection.carets().iter().enumerate() {
            if let Some((transaction, trim)) =
                edit_ops::indent::enter_between_pair(language, &text, caret.start(), style)
            {
                edits.extend(transaction.edits);
                pair_trim[i] = trim;
                continue;
            }
            let indent =
                edit_ops::indent::indent_for_new_line(language, &text, caret.start(), style);
            edits.push(editor_core::transaction::TextEdit::new(
                caret.range(),
                format!("\n{indent}"),
            ));
        }

        let ffi_edits = self.commit(tab_id, &text, Transaction::new(edits));
        if pair_trim.iter().any(|trim| *trim > 0) {
            let mut tabs = self.tabs.borrow_mut();
            if let Some(ops) = tabs.get_mut(&tab_id) {
                // Same caret order `commit`'s `map_carets` produced them in:
                // neither the between-pair inserts nor the plain newlines
                // above can make two carets swap places or merge, since
                // each is a non-overlapping insertion at its own caret.
                let carets: Vec<Caret> = ops
                    .selection
                    .carets()
                    .iter()
                    .enumerate()
                    .map(|(i, c)| Caret::at(c.start().saturating_sub(pair_trim[i])))
                    .collect();
                let primary = ops.selection.primary_index();
                if let Ok(set) = SelectionSet::from_carets(carets, primary) {
                    ops.selection = set;
                }
            }
        }
        self.as_mut().carets_changed(tab_id);
        ffi_edits
    }

    /// Duplicate / move / delete / join, by the constants above.
    pub fn line_op(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        kind: u8,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        let transaction = match kind {
            LINE_OP_DUPLICATE => line_ops::duplicate(&text, &selection),
            LINE_OP_MOVE_UP => line_ops::move_up(&text, &selection),
            LINE_OP_MOVE_DOWN => line_ops::move_down(&text, &selection),
            LINE_OP_DELETE => line_ops::delete(&text, &selection),
            LINE_OP_JOIN => line_ops::join(&text, &selection),
            _ => Transaction::empty(),
        };
        let edits = self.commit(tab_id, &text, transaction);
        self.as_mut().carets_changed(tab_id);
        edits
    }

    /// Ctrl+/ and Ctrl+Shift+/.
    pub fn toggle_comment(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        block: bool,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        let language = language_of(&self.session.borrow(), tab_id);
        let transaction = if block {
            edit_ops::comment::toggle_block(language, &text, &selection)
        } else {
            edit_ops::comment::toggle_line(language, &text, &selection)
        };
        let edits = self.commit(tab_id, &text, transaction);
        self.as_mut().carets_changed(tab_id);
        edits
    }

    /// Tab / Shift+Tab over a selection.
    ///
    /// Shift+Tab always unindents the lines any caret touches. Tab does the
    /// same when a caret is selecting something; a bare (collapsed) caret
    /// instead gets one indent unit inserted at the caret, which is what
    /// typing Tab into the middle of a line has always meant.
    pub fn indent_selection(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        outdent: bool,
    ) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let selection = self.selection_of(tab_id);
        let language = language_of(&self.session.borrow(), tab_id);
        let style = self.indent_style(language);
        let transaction = if outdent {
            edit_ops::indent::unindent_selection(&text, &selection, style)
        } else if selection.carets().iter().any(|c| !c.is_empty()) {
            edit_ops::indent::indent_selection(&text, &selection, style)
        } else {
            Transaction::type_text(&selection, &style.unit())
        };
        let edits = self.commit(tab_id, &text, transaction);
        self.as_mut().carets_changed(tab_id);
        edits
    }

    /// Ctrl+W: grow every caret to its enclosing syntax node.
    pub fn expand_selection(mut self: Pin<&mut Self>, tab_id: u64, text: &QString) {
        let text = text.to_string();
        let language = language_of(&self.session.borrow(), tab_id);
        {
            let mut tabs = self.tabs.borrow_mut();
            let ops = tabs.entry(tab_id).or_default();
            let current = ops.selection.clone();
            ops.selection = ops.history.expand(language, &text, &current);
        }
        self.as_mut().carets_changed(tab_id);
    }

    /// Ctrl+Shift+W: back down the path Ctrl+W took. A selection the history
    /// does not recognise is left alone rather than shrunk by guesswork.
    pub fn shrink_selection(mut self: Pin<&mut Self>, tab_id: u64) {
        {
            let mut tabs = self.tabs.borrow_mut();
            let ops = tabs.entry(tab_id).or_default();
            let current = ops.selection.clone();
            if let Some(previous) = ops.history.shrink(&current) {
                ops.selection = previous;
            }
        }
        self.as_mut().carets_changed(tab_id);
    }

    /// Left/Right/Up/Down/Home/End/word-move with more than one caret
    /// active: every caret moves at once (ADR-0023's ceiling lifted for the
    /// keys `editor_core::selection::CaretMotion` covers), rather than the
    /// widget's default of collapsing to the primary caret and moving only
    /// it. `extend` is Shift held.
    pub fn move_carets(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        motion: u8,
        extend: bool,
    ) {
        let text = text.to_string();
        let language = language_of(&self.session.borrow(), tab_id);
        let tab_width = self.tab_width(language);
        let motion = match motion {
            MOTION_LEFT => CaretMotion::Left,
            MOTION_RIGHT => CaretMotion::Right,
            MOTION_UP => CaretMotion::Up,
            MOTION_DOWN => CaretMotion::Down,
            MOTION_HOME => CaretMotion::Home,
            MOTION_END => CaretMotion::End,
            MOTION_WORD_LEFT => CaretMotion::WordLeft,
            MOTION_WORD_RIGHT => CaretMotion::WordRight,
            _ => CaretMotion::Right,
        };
        let moved = {
            let mut tabs = self.tabs.borrow_mut();
            let ops = tabs.entry(tab_id).or_default();
            ops.selection.move_carets(&text, motion, extend, tab_width)
        };
        if let Ok(selection) = moved {
            self.tabs.borrow_mut().entry(tab_id).or_default().selection = selection;
        }
        self.as_mut().carets_changed(tab_id);
    }

    /// Ctrl+]: where the bracket under `position` is answered by, as a
    /// document position, or -1 when the caret is not on a bracket.
    pub fn matching_bracket(&self, tab_id: u64, text: &QString, position: u32) -> i64 {
        let text = text.to_string();
        let language = language_of(&self.session.borrow(), tab_id);
        let at = to_bytes(&text, &[position as usize])[0];
        match edit_ops::brackets::jump_target(language, &text, at) {
            Some(target) => to_utf16(&text, &[target])[0] as i64,
            None => -1,
        }
    }

    /// The bracket at `position` and its partner, for the live pair
    /// highlight — painted while the caret sits on either side of a pair,
    /// in the error colour when the bracket has no partner. `has_bracket`
    /// false means the caret is not on one at all, which is not the same
    /// thing as an unmatched bracket and paints nothing.
    pub fn bracket_pair_at(
        &self,
        tab_id: u64,
        text: &QString,
        position: u32,
    ) -> ffi::FfiBracketPair {
        let text = text.to_string();
        let language = language_of(&self.session.borrow(), tab_id);
        let at = to_bytes(&text, &[position as usize])[0];
        let Some(found) = edit_ops::brackets::pair_at(language, &text, at) else {
            return ffi::FfiBracketPair::default();
        };
        let mut offsets = vec![found.bracket.start, found.bracket.end];
        if let Some(partner) = &found.partner {
            offsets.push(partner.start);
            offsets.push(partner.end);
        }
        let utf16 = to_utf16(&text, &offsets);
        ffi::FfiBracketPair {
            has_bracket: true,
            bracket_start: utf16[0] as u32,
            bracket_end: utf16[1] as u32,
            has_partner: found.partner.is_some(),
            partner_start: utf16.get(2).copied().unwrap_or(0) as u32,
            partner_end: utf16.get(3).copied().unwrap_or(0) as u32,
        }
    }

    /// The edits a save would make before it writes the file (F1-11): trim
    /// trailing whitespace, a final newline, line-ending normalisation —
    /// whichever of them the file's language has turned on. The caller
    /// splices these into the buffer *before* reading its text to save, the
    /// same `applyEditsTo` path every other operation here uses, so the
    /// tidying is one undo entry and the caret the trim would otherwise
    /// have jumped from lands wherever Qt's own cursor adjustment during
    /// the splice puts it — never column 0.
    ///
    /// A pure computation: it touches no caret state and never emits
    /// `caretsChanged`, because nothing about where the carets are changes
    /// here — only what the text says.
    pub fn save_rule_edits(&self, tab_id: u64, text: &QString) -> Vec<ffi::FfiTextEdit> {
        let text = text.to_string();
        let language = language_of(&self.session.borrow(), tab_id);
        let rules = self.save_rules(language);
        let transaction = editor_core::save_rules::on_save(&text, &rules);
        if transaction.is_empty() {
            return Vec::new();
        }
        self.to_ffi_edits(&text, &transaction)
    }

    /// The tab width `text` in this tab renders at, resolved through
    /// `settings-model` for the tab's language — what `CodeEditor` sets
    /// `setTabStopDistance` from, so a rendered tab glyph ends where the
    /// tab actually ends (show-whitespace-characters task).
    pub fn tab_width_for_tab(&self, tab_id: u64) -> u32 {
        let language = language_of(&self.session.borrow(), tab_id);
        self.tab_width(language) as u32
    }

    /// The column this tab's language wants the wrap guide painted at, `0`
    /// for "never". Resolved the same way `tabWidthForTab` is, and read once
    /// at tab-open time — the same reasoning: it does not change for the
    /// tab's lifetime.
    pub fn wrap_column_for_tab(&self, tab_id: u64) -> u32 {
        let language = language_of(&self.session.borrow(), tab_id);
        let settings = self.settings.borrow();
        settings_model::editing::resolve_for_language(&settings, &language.id()).wrap_column
    }

    /// Whether this tab's language wants text reflowed at that column rather
    /// than only guided by it.
    pub fn soft_wrap_for_tab(&self, tab_id: u64) -> bool {
        let language = language_of(&self.session.borrow(), tab_id);
        let settings = self.settings.borrow();
        settings_model::editing::resolve_for_language(&settings, &language.id()).soft_wrap
    }

    /// The cached global soft-wrap setting, for the View menu's toggle to
    /// show its current state when the menu is built.
    pub fn soft_wrap_enabled(&self) -> bool {
        self.settings.borrow().editing.soft_wrap_or_default()
    }

    /// Flip the *global* soft-wrap setting and persist it — the View menu's
    /// quick toggle (`view.toggleSoftWrap`), as opposed to the per-language
    /// override the Editing settings page offers. Best-effort: a settings
    /// write failure here must not stop the toggle from taking effect for
    /// the running session, the same tolerance `push_recent_project` already
    /// applies to its own settings write.
    pub fn toggle_soft_wrap(self: Pin<&mut Self>) -> bool {
        let config_dir = app_core::resolve_config_dir();
        let mut settings = app_config::load(&config_dir).unwrap_or_default();
        let enabled = !settings.editing.soft_wrap_or_default();
        settings.editing.soft_wrap = Some(enabled);
        let _ = app_config::save(&config_dir, &settings);
        *self.settings.borrow_mut() = settings;
        enabled
    }

    /// R2: begin a snippet session for `tab_id` from the tab stops the
    /// caller already resolved to absolute document positions, replacing
    /// any session the tab already had. Returns the first stop, or "no
    /// stop" when there was nothing to begin (an empty `stops`, which
    /// `begin_snippet`'s own caller never sends, but is cheap to allow).
    pub fn begin_snippet(
        self: Pin<&mut Self>,
        tab_id: u64,
        stops: Vec<ffi::FfiSnippetStop>,
    ) -> ffi::FfiSnippetStop {
        let ranges: Vec<std::ops::Range<usize>> = stops
            .iter()
            .map(|stop| stop.start as usize..stop.end as usize)
            .collect();
        let Some(session) = editor_core::SnippetSession::new(ranges) else {
            return no_snippet_stop();
        };
        let current = session.current();
        let more = !session.is_last();
        // Nothing left to keep once the first stop is already the last one
        // — no session outlives its own only stop.
        self.tabs.borrow_mut().entry(tab_id).or_default().snippet = more.then_some(session);
        snippet_stop(current, more)
    }

    /// R2: Tab/Shift+Tab while a snippet session may be active. "No stop"
    /// covers both "this tab has no session" and "the move had nowhere to
    /// go" — either way the caller falls through to what the key ordinarily
    /// does. Landing on the last stop clears the session immediately (R2's
    /// "`$0` ends the session").
    pub fn step_snippet(self: Pin<&mut Self>, tab_id: u64, backward: bool) -> ffi::FfiSnippetStop {
        let mut tabs = self.tabs.borrow_mut();
        let Some(ops) = tabs.get_mut(&tab_id) else {
            return no_snippet_stop();
        };
        let Some(session) = ops.snippet.as_mut() else {
            return no_snippet_stop();
        };
        let moved = if backward {
            session.retreat()
        } else {
            session.advance()
        };
        let Some(current) = moved else {
            return no_snippet_stop();
        };
        let more = !session.is_last();
        if !more {
            ops.snippet = None;
        }
        snippet_stop(current, more)
    }

    /// Escape, or the caret left the snippet another way: drop this tab's
    /// session, if it has one.
    pub fn end_snippet(self: Pin<&mut Self>, tab_id: u64) {
        if let Some(ops) = self.tabs.borrow_mut().get_mut(&tab_id) {
            ops.snippet = None;
        }
    }

    /// Classifies every space/tab character in `text` into leading, inner,
    /// and trailing whitespace (`editor_core::whitespace`), one line at a
    /// time. `text` is whatever multi-line slice the caller wants
    /// classified — the view passes only its currently visible blocks,
    /// joined with `\n`, so this is one call per repaint rather than one
    /// per line (show-whitespace-characters task). Stateless: unlike
    /// `matching_bracket`/`save_rule_edits` above it needs no tab id or
    /// language, since space/tab classification does not depend on either.
    pub fn whitespace_spans(&self, text: &QString) -> Vec<ffi::FfiWhitespaceSpan> {
        let text = text.to_string();
        text.split('\n')
            .enumerate()
            .flat_map(|(line, line_text)| {
                editor_core::whitespace::classify_whitespace(line_text)
                    .into_iter()
                    .map(move |c| ffi::FfiWhitespaceSpan {
                        line: line as u32,
                        column: c.column as u32,
                        is_tab: c.is_tab,
                        category: match c.category {
                            editor_core::whitespace::WhitespaceCategory::Leading => 0,
                            editor_core::whitespace::WhitespaceCategory::Inner => 1,
                            editor_core::whitespace::WhitespaceCategory::Trailing => 2,
                        },
                    })
            })
            .collect()
    }
}
