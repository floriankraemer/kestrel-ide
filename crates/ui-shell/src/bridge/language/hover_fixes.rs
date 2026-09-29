//! H3: the fixes behind the hover card's rows.
//!
//! The card is shown at once with every problem `Loading`; each problem's
//! fixes then arrive (the server-less dispatch `requestIntentions` uses, else
//! a `codeAction` request scoped to that diagnostic) and the card is
//! re-emitted. Which fix is primary, what the row says and which token's
//! fixes may be used are `lsp_core::hover_card` rules; this file only keeps
//! the answers and hands them to the existing apply path.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use diagnostics_core::DiagnosticRow;
use lsp_core::hover_card::{primary_fix_index, FixState, HoverCard};
use lsp_core::Intention;

use crate::bridge::ffi;

/// The card on screen and the intentions fetched for each of its problems
/// (`lsp_core::hover_card::HoverFixes` owns the per-token rule). Replaced
/// wholesale on every hover, emptied when the card goes away.
#[derive(Default)]
pub(crate) struct HoverFixes {
    card: HoverCard,
    fixes: lsp_core::hover_card::HoverFixes,
    language: String,
    /// A card is on screen; a late footer for none must not repaint anything.
    active: bool,
}

impl ffi::LanguageService {
    /// Show `card` (whose problems came from `rows`) and start fetching each
    /// problem's fixes: the server-less dispatch first (`local_intentions`,
    /// the one `requestIntentions` uses), else the language server when the
    /// file has one (`language_id`). Called only for a still-current token.
    pub(crate) fn show_hover_card(
        mut self: Pin<&mut Self>,
        mut card: HoverCard,
        rows: Vec<DiagnosticRow>,
        language_id: Option<String>,
        token: u64,
    ) {
        for problem in &mut card.problems {
            problem.fixes = FixState::Loading;
        }
        {
            let mut state = self.hover_fixes.borrow_mut();
            state.fixes.show(token, rows.len());
            state.language = language_id.clone().unwrap_or_default();
            state.card = card;
            state.active = true;
        }
        let html = self.rendered_card();
        self.as_mut().hover_ready(html);
        let has_server = language_id.is_some();
        for (index, row) in rows.into_iter().enumerate() {
            self.as_mut()
                .request_problem_fixes(index, row, token, has_server);
        }
    }

    /// The popup no longer shows this card (closed, or replaced by other
    /// content): its fixes must not be applicable any more.
    pub fn clear_hover_fixes(self: Pin<&mut Self>) {
        *self.hover_fixes.borrow_mut() = HoverFixes::default();
    }

    /// H4: the declaration behind the shown card arrived; add its footer.
    pub fn set_hover_source(self: Pin<&mut Self>, display: &QString, line: u32, column: u32) {
        {
            let mut state = self.hover_fixes.borrow_mut();
            if !state.active {
                return;
            }
            state.card.source = Some(lsp_core::hover_card::CardLocation {
                path: display.to_string(),
                line,
                column,
            });
        }
        let html = self.rendered_card();
        self.hover_card_updated(html);
    }

    fn rendered_card(&self) -> QString {
        QString::from(super::render_card(&self.hover_fixes.borrow().card).as_str())
    }

    fn request_problem_fixes(
        mut self: Pin<&mut Self>,
        index: usize,
        row: DiagnosticRow,
        token: u64,
        has_server: bool,
    ) {
        let (line, column) = (row.line - 1, row.column);
        if let Some(list) = self.local_intentions(&row.path, line, column) {
            self.set_problem_fixes(token, index, list);
            return;
        }
        if !has_server {
            self.set_problem_fixes(token, index, Vec::new());
            return;
        }
        let raw: Vec<_> = self.store.borrow().raw_for(&row).into_iter().collect();
        let uri = row.uri.clone();
        let end = (row.end_line - 1, row.end_column);
        // As in `requestIntentions`: a build file's quick fix merges into the
        // server's own list.
        let build_file_quick_fix = self.build_file_quick_fix(&row.path, line, column);
        let qt_thread = self.as_mut().qt_thread();
        let queued = self.push_job(move |manager| {
            let mut list = manager
                .intentions(&uri, (line, column), end, &raw)
                .unwrap_or_default();
            list.extend(build_file_quick_fix);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                // The pointer moved on: this row belongs to a card that is gone.
                if service.hover.borrow().accept(token) {
                    service.as_mut().set_problem_fixes(token, index, list);
                }
            });
        });
        if !queued {
            self.set_problem_fixes(token, index, Vec::new());
        }
    }

    fn set_problem_fixes(self: Pin<&mut Self>, token: u64, index: usize, list: Vec<Intention>) {
        {
            let mut state = self.hover_fixes.borrow_mut();
            let fixes = FixState::of(&list);
            if !state.fixes.fill(token, index, list) {
                return;
            }
            state.card.problems[index].fixes = fixes;
        }
        let html = self.rendered_card();
        self.hover_card_updated(html);
    }

    /// The card's "More actions…" for `problem`: make its intentions the
    /// list `intentions()`/`applyIntention` work on. False when there is
    /// nothing to show.
    pub fn select_hover_intentions(self: Pin<&mut Self>, problem: u32) -> bool {
        let state = self.hover_fixes.borrow();
        let Some(list) = state.fixes.list(problem as usize).filter(|l| !l.is_empty()) else {
            return false;
        };
        *self.intentions.borrow_mut() = list.to_vec();
        *self.intentions_language.borrow_mut() = state.language.clone();
        true
    }

    /// Apply `problem`'s primary fix (the card's fix link).
    pub fn apply_hover_fix(mut self: Pin<&mut Self>, problem: u32, buffer_revision: i64) {
        let index = self
            .hover_fixes
            .borrow()
            .fixes
            .list(problem as usize)
            .and_then(primary_fix_index);
        if let (Some(index), true) = (index, self.as_mut().select_hover_intentions(problem)) {
            self.apply_intention(index as u32, buffer_revision);
        }
    }

    /// `code.applyPreferredFix` with a card up: the first problem that has a
    /// primary fix gets it applied. False when no card is showing or it
    /// offers none.
    pub fn apply_preferred_hover_fix(self: Pin<&mut Self>, buffer_revision: i64) -> bool {
        let problem = self.hover_fixes.borrow().fixes.first_fixable();
        let Some(problem) = problem else {
            return false;
        };
        self.apply_hover_fix(problem as u32, buffer_revision);
        true
    }

    /// `code.applyPreferredFix` at the caret: the primary fix of the list the
    /// last `requestIntentions` produced. False when there is none.
    pub fn apply_preferred_intention(self: Pin<&mut Self>, buffer_revision: i64) -> bool {
        let index = primary_fix_index(&self.intentions.borrow());
        let Some(index) = index else {
            return false;
        };
        self.apply_intention(index as u32, buffer_revision);
        true
    }

    pub fn primary_intention_index(&self) -> i32 {
        primary_fix_index(&self.intentions.borrow()).map_or(-1, |i| i as i32)
    }

    pub fn intention_bulb_is_fix(&self) -> bool {
        lsp_core::bulb_kind(&self.intentions.borrow()) == lsp_core::BulbKind::Fix
    }

    /// The user's bindings for the card's two shortcut hints (empty = unbound).
    pub fn set_hover_shortcuts(self: Pin<&mut Self>, apply_fix: &QString, more_actions: &QString) {
        super::update_hover_labels(|labels| {
            labels.apply_fix_shortcut = apply_fix.to_string();
            labels.more_actions_shortcut = more_actions.to_string();
        });
    }
}
