//! Application layer (ADR-0002): `AppSession` owns the one open project and
//! the open-document table, and exposes the command methods the UI adapter
//! calls. Every rule that used to live in `main_window.cpp` or the QObject
//! bodies — binary-open rejection, rename path construction, delete → tab
//! invalidation, watcher-event → tab policy, config-dir fallback — lives
//! here, Qt-free and unit-tested.
//!
//! No Qt dependency — `ui-shell`'s `bridge.rs` wraps this in thin QObject
//! adapters (slot → session call → signal); see docs/architecture/layering.md.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use editor_core::{BinaryFile, Document, HexRow, ImageFile};
use project_model::{Project, ProjectSession};

use diff_tab::DiffContent;

/// Where plugins and parsed colour themes are joined (ADR-0026, color-themes plan T6).
pub mod color_themes;
/// Read-only `TabKind::Diff` tabs: `DiffContent` and the `AppSession`
/// methods that open one and read it back (F3-14).
mod diff_tab;
/// `AppError` (ADR-0003), split out once `lib.rs` hit the file-size gate.
mod error;
/// File operations a workspace edit asks for (F2; its ADR is unwritten).
pub mod file_ops;
/// Where plugins and icon packs are joined (ADR-0026, ADR-0027).
pub mod icons;
/// Rasterising an SVG image tab's file, reusing `icon-theme`'s pipeline.
pub mod image_render;
/// Where plugins and the Markdown/Mermaid renderer are joined (ADR-0033).
pub mod preview;
mod project_open; // Swap-in half of an off-thread project open/rebuild (ADR-0037).
/// Locale-aware substring matching for UI filter boxes (issue #233).
pub mod text_search;
mod tree_sort;
mod virtual_doc; // Read-only virtual documents with no backing file (C12).

pub use error::AppError;
pub use file_ops::{FileOp, ResourceOpError};

/// Stable per-session tab identity (ADR-0003): issued monotonically from 1,
/// never reused, so an id can never silently start meaning a different tab
/// the way a `QTabWidget` index does after a close. `0` is reserved as the
/// "no tab" sentinel at the FFI edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(u64);

impl TabId {
    /// The raw `u64` crossing the FFI seam.
    pub fn raw(self) -> u64 {
        self.0
    }

    /// Reconstruct an id received back from the FFI seam.
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// How long after this session writes a path to disk itself (`save_tab`) or
/// repoints a tab onto a new path (a tree-driven rename) a matching
/// filesystem-watcher event for that path is treated as an echo of our own
/// change rather than a genuine external edit — see
/// [`AppSession::check_external_change`]. Generous enough to absorb typical
/// inotify/Qt-event-loop latency; not meant to be race-proof.
const SELF_CHANGE_SUPPRESSION_WINDOW: Duration = Duration::from_millis(1500);

/// What [`AppSession::open_file`] yielded: the tab (new or existing) now
/// holding `path`, and whether it was newly opened — the adapter only emits
/// a tab-opened signal for genuinely new tabs (US-3: focus, don't duplicate).
#[derive(Debug)]
pub struct OpenedTab {
    pub id: TabId,
    pub title: String,
    pub newly_opened: bool,
    /// Which widget the adapter must build for this tab.
    pub kind: TabKind,
}

/// An open tab whose title changed as a side effect of a tree mutation — a
/// rename retargeting it, or a delete flagging it "(deleted)" (US-2b). The
/// adapter relays this as a tab-title-changed signal.
#[derive(Debug)]
pub struct RetitledTab {
    pub id: TabId,
    pub title: String,
}

/// What kind of view a tab needs. Crosses the FFI seam so the adapter can
/// build the right widget for a newly opened tab; the view never decides
/// this from the path or the content itself (ADR-0002, ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    /// An editable text document.
    Text,
    /// A read-only hex view of a file whose bytes aren't text.
    Binary,
    /// A read-only comparison of two texts with no live `Document` on either
    /// side; see `diff_tab`'s module doc for why the working-tree-vs-`HEAD`
    /// diff is deliberately not this kind.
    Diff,
    /// A read-only image view.
    Image,
}

impl TabKind {
    /// Stable numeric code at the FFI seam, same contract as
    /// [`AppError::code`]: append only, never renumber.
    pub const CODE_TEXT: i32 = 0;
    pub const CODE_BINARY: i32 = 1;
    pub const CODE_DIFF: i32 = 2;
    pub const CODE_IMAGE: i32 = 3;

    pub fn code(self) -> i32 {
        match self {
            TabKind::Text => Self::CODE_TEXT,
            TabKind::Binary => Self::CODE_BINARY,
            TabKind::Diff => Self::CODE_DIFF,
            TabKind::Image => Self::CODE_IMAGE,
        }
    }
}

/// A tab's backing content. Everything a tab needs regardless of kind —
/// path, title, rename retargeting, delete flagging — is answered here, so
/// only the genuinely text-only operations (edit, save, reload, dirty
/// state) have to care which variant they got.
enum TabContent {
    Text(Document),
    Binary(BinaryFile),
    Diff(DiffContent),
    Image(ImageFile),
}

impl TabContent {
    fn kind(&self) -> TabKind {
        match self {
            TabContent::Text(_) => TabKind::Text,
            TabContent::Binary(_) => TabKind::Binary,
            TabContent::Diff(_) => TabKind::Diff,
            TabContent::Image(_) => TabKind::Image,
        }
    }

    fn path(&self) -> Option<&Path> {
        match self {
            TabContent::Text(doc) => doc.path(),
            TabContent::Binary(file) => Some(file.path()),
            TabContent::Diff(diff) => Some(&diff.path),
            TabContent::Image(file) => Some(file.path()),
        }
    }

    // A diff tab (two already-read texts, not a file handle) is never
    // retargeted, deleted-flagged or dirty: `set_path`/`mark_deleted` below
    // are no-ops for it and `is_deleted` always answers `false`.
    fn set_path(&mut self, path: PathBuf) {
        match self {
            TabContent::Text(doc) => doc.set_path(path),
            TabContent::Binary(file) => file.set_path(path),
            TabContent::Diff(_) => {}
            TabContent::Image(file) => file.set_path(path),
        }
    }

    fn title(&self) -> String {
        match self {
            TabContent::Text(doc) => doc.title(),
            TabContent::Binary(file) => file.title(),
            TabContent::Diff(diff) => diff.title(),
            TabContent::Image(file) => file.title(),
        }
    }

    fn is_deleted(&self) -> bool {
        match self {
            TabContent::Text(doc) => doc.is_deleted(),
            TabContent::Binary(file) => file.is_deleted(),
            TabContent::Diff(_) => false,
            TabContent::Image(file) => file.is_deleted(),
        }
    }

    fn mark_deleted(&mut self) {
        match self {
            TabContent::Text(doc) => doc.mark_deleted(),
            TabContent::Binary(file) => file.mark_deleted(),
            TabContent::Diff(_) => {}
            TabContent::Image(file) => file.mark_deleted(),
        }
    }
}

struct TabEntry {
    id: TabId,
    content: TabContent,
}

/// The config dir the session persists "last opened project" into:
/// the platform config dir, or a temp-dir fallback when the platform
/// doesn't report one (e.g. a stripped-down container environment) —
/// degrading "reopen last project" beats refusing to start.
pub fn resolve_config_dir() -> PathBuf {
    project_model::default_config_dir().unwrap_or_else(|| std::env::temp_dir().join("ide"))
}

/// The application session (ADR-0002): the one open project plus the
/// open-document table, with `Result`-returning command methods as the
/// command layer. The UI adapter holds exactly one of these and translates
/// slots into calls on it.
/// One place in the project a jump can return to: a file plus a 1-based
/// line and 0-based column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}

/// How many entries the back/forward stack keeps. Old enough entries stop
/// being useful long before memory is a concern; the cap exists so a long
/// session can't grow the stack without bound.
const NAVIGATION_HISTORY_CAP: usize = 64;

/// Back/forward jump history, JetBrains-style.
///
/// The rules that make it feel right, rather than merely correct:
///
/// - Recording a jump truncates whatever was ahead of the cursor, the same
///   way a browser drops the forward stack once you navigate somewhere new.
/// - Positions that are effectively the same place collapse instead of
///   pushing a new entry: same file, within [`SAME_PLACE_LINES`] lines of
///   the current top. Without this, scrolling around one function would
///   fill the stack with near-identical entries and "back" would appear to
///   do nothing several times in a row.
/// - [`back`](Self::back) returns the entry *before* the current one and
///   moves the cursor onto it, so repeated calls walk backwards; `forward`
///   is its mirror.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NavigationHistory {
    entries: Vec<Location>,
    /// Index of the entry the caret is considered to be on. `None` when
    /// the history is empty.
    cursor: Option<usize>,
}

/// Two positions in the same file this close together are treated as the
/// same place — see [`NavigationHistory`].
const SAME_PLACE_LINES: u32 = 1;

impl NavigationHistory {
    /// Record `location` as the current place, dropping any forward
    /// entries. A location that is effectively where we already are
    /// updates the top entry in place instead of pushing.
    pub fn record(&mut self, location: Location) {
        if let Some(cursor) = self.cursor {
            if same_place(&self.entries[cursor], &location) {
                self.entries[cursor] = location;
                self.entries.truncate(cursor + 1);
                return;
            }
            self.entries.truncate(cursor + 1);
        }
        self.entries.push(location);
        if self.entries.len() > NAVIGATION_HISTORY_CAP {
            self.entries.remove(0);
        }
        self.cursor = Some(self.entries.len() - 1);
    }

    /// The entry before the current one, if any, moving the cursor onto it.
    pub fn back(&mut self) -> Option<Location> {
        let cursor = self.cursor?;
        let previous = cursor.checked_sub(1)?;
        self.cursor = Some(previous);
        Some(self.entries[previous].clone())
    }

    /// The entry after the current one, if any, moving the cursor onto it.
    pub fn forward(&mut self) -> Option<Location> {
        let cursor = self.cursor?;
        let next = cursor + 1;
        let entry = self.entries.get(next)?.clone();
        self.cursor = Some(next);
        Some(entry)
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor.is_some_and(|c| c > 0)
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor.is_some_and(|c| c + 1 < self.entries.len())
    }
}

fn same_place(a: &Location, b: &Location) -> bool {
    a.path == b.path && a.line.abs_diff(b.line) <= SAME_PLACE_LINES
}

pub struct AppSession {
    project: ProjectSession,
    /// Open documents in opening order. A `Vec` scan instead of a map:
    /// lookups are by id or path over a handful of open tabs.
    docs: Vec<TabEntry>,
    /// Next id to issue; starts at 1 so 0 stays the FFI "no tab" sentinel.
    next_tab_id: u64,
    active: Option<TabId>,
    /// Paths this session itself just changed on disk (a `save_tab`) or
    /// repointed a tab onto (a tree-driven rename), each with the `Instant`
    /// it happened — the own-save/own-rename feedback-loop guard for
    /// `check_external_change` (the filesystem watcher would otherwise also
    /// see these as "external" changes).
    suppressed_changes: HashMap<PathBuf, Instant>,
    config_dir: PathBuf,
    /// Last-reported (line, column) per tab, forwarded from the view's own
    /// cursor (M4's `get_cursor_position` MCP tool — nothing here computes
    /// a cursor position, it only remembers what the view last reported).
    cursor_positions: HashMap<TabId, (u32, u32)>,
    /// Back/forward jump history across every file (not per tab): a jump
    /// that opened another file has to be walkable back to where it came
    /// from.
    navigation: NavigationHistory,
}

impl Default for AppSession {
    fn default() -> Self {
        Self::new()
    }
}

impl AppSession {
    pub fn new() -> Self {
        Self::with_config_dir(resolve_config_dir())
    }

    /// Like [`AppSession::new`] but persisting into `config_dir` — tests use
    /// a temp dir so they never touch the developer's real `~/.config`.
    pub fn with_config_dir(config_dir: PathBuf) -> Self {
        Self {
            project: ProjectSession::new(),
            docs: Vec::new(),
            next_tab_id: 1,
            active: None,
            suppressed_changes: HashMap::new(),
            navigation: NavigationHistory::default(),
            config_dir,
            cursor_positions: HashMap::new(),
        }
    }

    // --- project commands -------------------------------------------------

    /// The current project, if one is open — read-only tree access for the
    /// `QAbstractItemModel` adapter.
    pub fn project(&self) -> Option<&Project> {
        self.project.current()
    }

    /// Absolute path of the open project's root folder, if any.
    pub fn root_path(&self) -> Option<&Path> {
        self.project.current().map(|p| p.root.path())
    }

    /// Open `path` as the active project and persist it as "last opened".
    /// On failure the current project (if any) is left unchanged (US-1).
    pub fn open_project(&mut self, path: &Path) -> Result<(), AppError> {
        self.project
            .open_folder(path, &self.config_dir)
            .map_err(AppError::OpenFolder)
    }

    /// Reopen the last-persisted project (US-1). Returns whether a project
    /// was found and opened; startup is silent about a missing or unreadable
    /// last project rather than popping an error dialog before the window is
    /// even shown, hence `bool` and not `Result`.
    pub fn reopen_last_project(&mut self) -> bool {
        self.project.reopen_last(&self.config_dir).unwrap_or(false)
    }

    /// (Re)start the filesystem watcher for the current project root; see
    /// `ProjectSession::start_watcher` for the threading contract.
    ///
    /// `is_remote` (W6-1, ADR-0052) passes straight through to
    /// `ProjectWatcher::start` — see its doc comment for why this crate
    /// takes a plain bool rather than classifying the root itself.
    pub fn start_watcher(
        &mut self,
        is_remote: bool,
        on_change: impl Fn(project_model::EventKind, PathBuf) + Send + 'static,
    ) -> Result<(), AppError> {
        self.project
            .start_watcher(is_remote, on_change)
            .map_err(|err| AppError::WatcherFailed(err.to_string()))
    }

    /// Re-snapshot the current project's tree from disk (after a watcher
    /// event reported a structural change).
    pub fn rebuild_tree(&mut self) -> Result<(), AppError> {
        self.project.rebuild_tree().map_err(AppError::TreeRebuild)
    }

    // --- tab commands -----------------------------------------------------

    /// Open `path` as a new tab, or focus the existing tab if the file is
    /// already open (US-3: focus, don't duplicate). No hint (see
    /// [`AppSession::open_file_with_hint`]): the binary sniff alone decides
    /// the kind, as it always has (ADR-0020).
    pub fn open_file(&mut self, path: &Path) -> Result<OpenedTab, AppError> {
        self.open_file_with_hint(path, None)
    }

    /// Like [`AppSession::open_file`], but `hint` — resolved by the adapter
    /// through `settings_model::file_associations`, since this crate may
    /// not depend on that crate (ADR-0017/ADR-0029) — decides the tab kind
    /// directly, skipping the binary sniff. `None` (no association matched)
    /// or `Some(TabKind::Diff)` (never produced by that resolver) both fall
    /// back to the sniff, exactly `open_file`'s old behaviour.
    pub fn open_file_with_hint(
        &mut self,
        path: &Path,
        hint: Option<TabKind>,
    ) -> Result<OpenedTab, AppError> {
        if let Some(id) = self.find_tab_by_path(path) {
            self.active = Some(id);
            let title = self.tab_title(id).expect("tab found by path exists");
            return Ok(OpenedTab {
                id,
                title,
                newly_opened: false,
                kind: self.tab_kind(id).expect("tab found by path exists"),
            });
        }
        let kind = match hint {
            Some(kind @ (TabKind::Text | TabKind::Binary | TabKind::Image)) => kind,
            Some(TabKind::Diff) | None => {
                if editor_core::looks_binary_file(path).map_err(AppError::OpenFile)? {
                    TabKind::Binary
                } else {
                    TabKind::Text
                }
            }
        };
        let content = match kind {
            TabKind::Image => TabContent::Image(ImageFile::open(path).map_err(AppError::OpenFile)?),
            TabKind::Binary => {
                TabContent::Binary(BinaryFile::open(path).map_err(AppError::OpenFile)?)
            }
            TabKind::Text => TabContent::Text(Document::open(path).map_err(AppError::OpenFile)?),
            TabKind::Diff => unreachable!("Diff never reaches here; matched above"),
        };
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        let title = content.title();
        let kind = content.kind();
        self.docs.push(TabEntry { id, content });
        self.active = Some(id);
        Ok(OpenedTab {
            id,
            title,
            newly_opened: true,
            kind,
        })
    }

    /// Which kind of view `id` needs, or `None` for an unknown tab.
    pub fn tab_kind(&self, id: TabId) -> Option<TabKind> {
        self.entry(id).map(|e| e.content.kind())
    }

    /// How many hex rows the binary tab `id` spans — the viewer's scroll
    /// range. `None` for an unknown tab or a text tab.
    pub fn binary_row_count(&self, id: TabId) -> Option<u64> {
        match self.entry(id).map(|e| &e.content) {
            Some(TabContent::Binary(file)) => Some(file.row_count()),
            _ => None,
        }
    }

    /// Size in bytes of the binary tab `id`. `None` for an unknown tab or a
    /// text tab.
    pub fn binary_len(&self, id: TabId) -> Option<u64> {
        match self.entry(id).map(|e| &e.content) {
            Some(TabContent::Binary(file)) => Some(file.len()),
            _ => None,
        }
    }

    /// The hex rows the viewer needs for its current scroll window. Only
    /// that window is read from disk, so this stays cheap on a huge binary.
    /// Empty for an unknown tab, a text tab, or a window past the end.
    pub fn binary_rows(&mut self, id: TabId, first_row: u64, count: usize) -> Vec<HexRow> {
        match self.entry_mut(id).map(|e| &mut e.content) {
            Some(TabContent::Binary(file)) => file.rows(first_row, count).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Close the tab `id`. Returns whether a tab was actually closed. The
    /// caller (UI) is responsible for any unsaved-changes prompt first.
    pub fn close_tab(&mut self, id: TabId) -> bool {
        let Some(pos) = self.docs.iter().position(|e| e.id == id) else {
            return false;
        };
        self.docs.remove(pos);
        if self.active == Some(id) {
            // The tab strip picks the neighbouring page itself and reports
            // it back via `set_active_tab` right after.
            self.active = None;
        }
        true
    }

    /// Replace the tab's content with `content` and write it to disk. The
    /// dirty flag is cleared on success and left set on failure (US-4: no
    /// silent data loss). The written path is recorded so the watcher's echo
    /// of our own write isn't reported as an external change.
    pub fn save_tab(&mut self, id: TabId, content: &str) -> Result<(), AppError> {
        let doc = self.text_doc_mut(id)?;
        doc.replace_content(content);
        doc.save().map_err(AppError::Save)?;
        if let Some(path) = doc.path().map(|p| p.to_path_buf()) {
            self.suppressed_changes.insert(path, Instant::now());
        }
        Ok(())
    }

    /// Save As: replace the tab's content, repoint it at `path`, and write
    /// it there. Unlike `save_tab` this changes the tab's identity on disk —
    /// the caller (adapter) is responsible for telling the view to
    /// re-render the tab's title afterward. Dirty flag left set on failure
    /// (US-4: no silent data loss), same as `save_tab`.
    pub fn save_tab_as(&mut self, id: TabId, path: PathBuf, content: &str) -> Result<(), AppError> {
        let doc = self.text_doc_mut(id)?;
        doc.replace_content(content);
        doc.set_path(path.clone());
        doc.save().map_err(AppError::Save)?;
        self.suppressed_changes.insert(path, Instant::now());
        Ok(())
    }

    /// Replace the tab's in-memory content and mark it dirty, without
    /// writing to disk (MCP's `edit_buffer` tool, M5 — an MCP client edits
    /// live, same as a human typing, then decides separately whether/when
    /// to save). The caller (adapter) is responsible for telling the view
    /// to reflect the new content in its widget, mirroring how `save_tab`'s
    /// caller already owns telling the view the dirty flag changed.
    pub fn edit_tab(&mut self, id: TabId, content: &str) -> Result<(), AppError> {
        let doc = self.text_doc_mut(id)?;
        doc.replace_content(content);
        doc.set_dirty(true);
        Ok(())
    }

    /// Write the tab's *current* in-memory content to disk (MCP's
    /// `save_buffer` tool, M5) — unlike `save_tab`, takes no `content`
    /// parameter, since the caller here is the tab's own content (set by
    /// `edit_tab` or the original file load), not a live widget whose
    /// keystrokes were never synced into the rope (ADR-0003 — that
    /// asymmetry is why `save_tab` needs `content` and this doesn't).
    pub fn save_buffer(&mut self, id: TabId) -> Result<(), AppError> {
        let doc = self.text_doc_mut(id)?;
        doc.save().map_err(AppError::Save)?;
        if let Some(path) = doc.path().map(|p| p.to_path_buf()) {
            self.suppressed_changes.insert(path, Instant::now());
        }
        Ok(())
    }

    /// Update which tab is considered active. Ignores unknown ids (the tab
    /// strip can report a page that was closed in the same event burst).
    pub fn set_active_tab(&mut self, id: TabId) {
        if self.entry(id).is_some() {
            self.active = Some(id);
        }
    }

    pub fn active_tab(&self) -> Option<TabId> {
        self.active
    }

    /// Mirror the view's edit notifications into the authoritative dirty
    /// flag (ADR-0003: Rust owns dirty state; the view forwards edits).
    /// Returns whether the tab exists.
    pub fn set_tab_dirty(&mut self, id: TabId, dirty: bool) -> bool {
        match self.doc_mut(id) {
            Some(doc) => {
                doc.set_dirty(dirty);
                true
            }
            None => false,
        }
    }

    /// Whether the tab has unsaved edits. A binary tab is read-only, so it
    /// is never dirty — answering `Some(false)` rather than `None` keeps the
    /// close-without-saving prompt from treating it as an unknown tab.
    pub fn tab_is_dirty(&self, id: TabId) -> Option<bool> {
        match self.entry(id).map(|e| &e.content) {
            Some(TabContent::Text(doc)) => Some(doc.is_dirty()),
            Some(TabContent::Binary(_))
            | Some(TabContent::Diff(_))
            | Some(TabContent::Image(_)) => Some(false),
            None => None,
        }
    }

    /// The tab's current buffer content, used to populate a newly created
    /// editor page.
    pub fn tab_content(&self, id: TabId) -> Option<String> {
        self.doc(id).map(|d| d.content())
    }

    /// The live buffer content for `path` if it is open in a tab, including
    /// unsaved edits; `None` when no tab holds that path. MCP's
    /// `resolve_declaration` tool needs this: `index_core` resolves a
    /// declaration against the *current* text at an offset, and reading the
    /// file from disk would silently resolve against a stale version of a
    /// buffer the user is still typing in.
    pub fn content_for_path(&self, path: &Path) -> Option<String> {
        self.tab_content(self.find_tab_by_path(path)?)
    }

    /// Snapshot of every open tab's id and display title, in opening order
    /// (MCP's `list_open_buffers` tool, M3/M4).
    pub fn open_tabs(&self) -> Vec<(TabId, String)> {
        self.docs
            .iter()
            .map(|e| (e.id, self.tab_title(e.id).unwrap_or_default()))
            .collect()
    }

    /// Every node in the open project's tree (path + is_dir), skipping the
    /// invisible root — mirrors what `ProjectTreeModel`'s rows show (MCP's
    /// `list_project_tree` tool, M4). Empty when no project is open.
    pub fn project_tree_entries(&self) -> Vec<(PathBuf, bool)> {
        let Some(project) = self.project.current() else {
            return Vec::new();
        };
        let tree = &project.tree;
        (0..tree.len())
            .filter(|&id| id != tree.root_id())
            .map(|id| {
                let node = tree.node(id);
                (node.path.clone(), node.is_dir)
            })
            .collect()
    }

    /// Forward the view's own cursor position for `id` (M4). Nothing here
    /// computes a position — this only remembers what the view last
    /// reported, the same "Rust remembers, view forwards" split dirty state
    /// already uses (ADR-0003). Ignores unknown ids (mirrors `set_tab_dirty`).
    pub fn set_cursor_position(&mut self, id: TabId, line: u32, column: u32) {
        if self.entry(id).is_some() {
            self.cursor_positions.insert(id, (line, column));
        }
    }

    /// The last-reported (line, column) for `id`, or `None` if never
    /// reported (MCP's `get_cursor_position` tool, M4).
    pub fn cursor_position(&self, id: TabId) -> Option<(u32, u32)> {
        self.cursor_positions.get(&id).copied()
    }

    /// Record where the caret is *before* a jump, so back can return here.
    /// Called by the view from the shared tail every jump funnels through,
    /// which is what gives Find in Files, Go to Symbol, Class View and Go
    /// to Line their history for free.
    pub fn record_jump(&mut self, path: PathBuf, line: u32, column: u32) {
        self.navigation.record(Location { path, line, column });
    }

    /// Step back in the jump history, or `None` at the oldest entry.
    pub fn jump_back(&mut self) -> Option<Location> {
        self.navigation.back()
    }

    /// Step forward in the jump history, or `None` at the newest entry.
    pub fn jump_forward(&mut self) -> Option<Location> {
        self.navigation.forward()
    }

    pub fn can_jump_back(&self) -> bool {
        self.navigation.can_go_back()
    }

    pub fn can_jump_forward(&self) -> bool {
        self.navigation.can_go_forward()
    }

    /// The tab's backing file name (`"main.rs"`, `"Dockerfile"`), used to
    /// pick a highlighting language (Y2). File *name*, not extension:
    /// `Dockerfile`/`Makefile` have no extension, and the language
    /// registry matches on either. Goes through `content.title()`, already
    /// this derivation for every tab kind — including a virtual tab (C12).
    pub fn tab_file_name(&self, id: TabId) -> Option<String> {
        Some(self.entry(id)?.content.title())
    }

    /// The tab's display title: the file name, plus a "(deleted)" suffix
    /// once the tree deleted its backing file (US-2b). The view renders this
    /// verbatim (its own dirty marker aside).
    pub fn tab_title(&self, id: TabId) -> Option<String> {
        self.entry(id).map(|e| {
            if e.content.is_deleted() {
                format!("{} (deleted)", e.content.title())
            } else {
                e.content.title()
            }
        })
    }

    /// The tab's backing file path, or `None` for an unknown or virtual
    /// (C12) tab. The view persists it to reopen a split layout next launch.
    pub fn tab_path(&self, id: TabId) -> Option<PathBuf> {
        self.entry(id)?.content.path().map(|p| p.to_path_buf())
    }

    /// Re-read the tab's backing file from disk, discarding any in-editor
    /// edits (the "Reload" choice on the external-change prompt, US-3).
    pub fn reload_tab(&mut self, id: TabId) -> Result<(), AppError> {
        let doc = self.text_doc_mut(id)?;
        doc.reload().map_err(AppError::Reload)
    }

    // --- tree mutations ---------------------------------------------------

    /// Create an empty file named `name` inside `parent_dir` and re-snapshot
    /// the tree (US-2b).
    pub fn create_file(&mut self, parent_dir: &Path, name: &str) -> Result<(), AppError> {
        project_model::create_file(parent_dir, name).map_err(AppError::FileOp)?;
        self.rebuild_tree()
    }

    /// Create an empty folder named `name` inside `parent_dir` and
    /// re-snapshot the tree (US-2b).
    pub fn create_folder(&mut self, parent_dir: &Path, name: &str) -> Result<(), AppError> {
        project_model::create_folder(parent_dir, name).map_err(AppError::FileOp)?;
        self.rebuild_tree()
    }

    /// Rename `path` (file or folder) to `new_name` in place, re-snapshot
    /// the tree, and — if `path` has an open tab — retarget that tab at the
    /// new path so future saves land there (US-2b). The new path is computed
    /// here, in one place, from the rename result; the view never
    /// reconstructs it. The retargeted path is recorded so the watcher's
    /// echo of the rename isn't reported as an external change.
    pub fn rename_entry(
        &mut self,
        path: &Path,
        new_name: &str,
    ) -> Result<Option<RetitledTab>, AppError> {
        let new_path = project_model::rename_path(path, new_name).map_err(AppError::FileOp)?;
        self.rebuild_tree()?;
        let Some(id) = self.find_tab_by_path(path) else {
            return Ok(None);
        };
        self.entry_mut(id)
            .expect("tab found by path exists")
            .content
            .set_path(new_path.clone());
        self.suppressed_changes.insert(new_path, Instant::now());
        let title = self.tab_title(id).expect("tab found by path exists");
        Ok(Some(RetitledTab { id, title }))
    }

    /// Delete `path` (recursively if it's a folder), re-snapshot the tree,
    /// and — if `path` has an open tab — flag that tab deleted, which blocks
    /// further silent saves and adds the "(deleted)" title suffix (US-2b).
    /// The old two-step C++ protocol (delete, then remember to notify the
    /// tab) is collapsed into this one command.
    pub fn delete_entry(&mut self, path: &Path) -> Result<Option<RetitledTab>, AppError> {
        project_model::delete_path(path).map_err(AppError::FileOp)?;
        self.rebuild_tree()?;
        let Some(id) = self.find_tab_by_path(path) else {
            return Ok(None);
        };
        self.entry_mut(id)
            .expect("tab found by path exists")
            .content
            .mark_deleted();
        let title = self.tab_title(id).expect("tab found by path exists");
        Ok(Some(RetitledTab { id, title }))
    }

    // --- watcher policy ---------------------------------------------------

    /// Decide whether a filesystem-watcher event for `path` is a genuine
    /// external change the user must be prompted about (US-3), returning the
    /// affected tab if so. `None` when `path` has no open tab, when the tab
    /// was already flagged deleted by a tree-driven delete (nothing to
    /// reload/keep), or when `path` was changed by this session itself
    /// within the suppression window (`save_tab` or a tree-driven rename
    /// onto `path`) rather than externally.
    pub fn check_external_change(&mut self, path: &Path) -> Option<TabId> {
        let id = self.find_tab_by_path(path)?;
        if self
            .entry(id)
            .map(|e| e.content.is_deleted())
            .unwrap_or(true)
        {
            return None;
        }
        let is_own_change = self
            .suppressed_changes
            .get(path)
            .map(|at| at.elapsed() < SELF_CHANGE_SUPPRESSION_WINDOW)
            .unwrap_or(false);
        if is_own_change {
            return None;
        }
        Some(id)
    }

    // --- internals --------------------------------------------------------

    fn find_tab_by_path(&self, path: &Path) -> Option<TabId> {
        self.docs
            .iter()
            .find(|e| e.content.path() == Some(path))
            .map(|e| e.id)
    }

    fn entry(&self, id: TabId) -> Option<&TabEntry> {
        self.docs.iter().find(|e| e.id == id)
    }

    fn entry_mut(&mut self, id: TabId) -> Option<&mut TabEntry> {
        self.docs.iter_mut().find(|e| e.id == id)
    }

    /// The tab's text document — `None` for an unknown tab *and* for a
    /// binary tab, since neither has one. Callers that need to tell those
    /// two apart use [`AppSession::text_doc_mut`].
    fn doc(&self, id: TabId) -> Option<&Document> {
        match self.entry(id).map(|e| &e.content) {
            Some(TabContent::Text(doc)) => Some(doc),
            _ => None,
        }
    }

    fn doc_mut(&mut self, id: TabId) -> Option<&mut Document> {
        match self.entry_mut(id).map(|e| &mut e.content) {
            Some(TabContent::Text(doc)) => Some(doc),
            _ => None,
        }
    }

    /// The tab's text document for a command that only makes sense on one,
    /// distinguishing "no such tab" from "that tab is a binary file" so the
    /// user gets told which it was.
    fn text_doc_mut(&mut self, id: TabId) -> Result<&mut Document, AppError> {
        match self.entry_mut(id).map(|e| &mut e.content) {
            Some(TabContent::Text(doc)) => Ok(doc),
            Some(TabContent::Binary(file)) => Err(AppError::NotATextTab(file.path().to_path_buf())),
            Some(TabContent::Diff(diff)) => Err(AppError::NotATextTab(diff.path.clone())),
            Some(TabContent::Image(file)) => Err(AppError::NotATextTab(file.path().to_path_buf())),
            None => Err(AppError::NoSuchTab),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::fs;
    use std::io;

    use project_model::{FileOpError, OpenFolderError};

    fn session_with_project() -> (tempfile::TempDir, tempfile::TempDir, AppSession) {
        let project_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        fs::write(project_dir.path().join("a.txt"), "alpha").unwrap();
        fs::write(project_dir.path().join("b.txt"), "beta").unwrap();
        let mut session = AppSession::with_config_dir(config_dir.path().to_path_buf());
        session.open_project(project_dir.path()).unwrap();
        (project_dir, config_dir, session)
    }

    /// Force a suppression entry to look older than the window without
    /// actually sleeping through it.
    fn expire_suppression(session: &mut AppSession, path: &Path) {
        let expired = Instant::now()
            .checked_sub(SELF_CHANGE_SUPPRESSION_WINDOW + Duration::from_secs(1))
            .expect("process uptime exceeds the suppression window in tests");
        session
            .suppressed_changes
            .insert(path.to_path_buf(), expired);
    }

    #[test]
    fn error_codes_are_stable() {
        // These numbers are the FFI contract main_window.cpp branches on
        // (ADR-0003) — renumbering any of them is a breaking change.
        assert_eq!(AppError::CODE_OK, 0);
        assert_eq!(AppError::NoSuchTab.code(), 1);
        assert_eq!(
            AppError::OpenFolder(OpenFolderError::NotFound(PathBuf::new())).code(),
            2
        );
        assert_eq!(AppError::BinaryFile(PathBuf::new()).code(), 3);
        assert_eq!(AppError::OpenFile(io::Error::other("x")).code(), 4);
        assert_eq!(AppError::Save(io::Error::other("x")).code(), 5);
        assert_eq!(
            AppError::FileOp(FileOpError::NotFound(PathBuf::new())).code(),
            6
        );
        assert_eq!(AppError::Reload(io::Error::other("x")).code(), 7);
        assert_eq!(AppError::TreeRebuild(io::Error::other("x")).code(), 8);
    }

    #[test]
    fn resolve_config_dir_always_yields_an_ide_dir() {
        // Whether the platform config dir or the temp-dir fallback wins,
        // the app's config always lives in a directory named "ide".
        let dir = resolve_config_dir();
        assert_eq!(dir.file_name().unwrap(), "ide");
    }

    #[test]
    fn open_project_failure_leaves_current_project_unchanged() {
        let (project_dir, _config, mut session) = session_with_project();
        let missing = project_dir.path().join("does-not-exist");

        let err = session.open_project(&missing).unwrap_err();
        assert_eq!(err.code(), AppError::CODE_OPEN_FOLDER);
        assert_eq!(session.root_path().unwrap(), project_dir.path());
    }

    #[test]
    fn open_file_opens_binary_content_as_a_hex_tab() {
        let (project_dir, _config, mut session) = session_with_project();
        let binary = project_dir.path().join("blob.bin");
        fs::write(&binary, [0u8, 159, 146, 150, 0, 1, 2]).unwrap();

        let opened = session.open_file(&binary).unwrap();

        assert_eq!(opened.kind, TabKind::Binary);
        assert_eq!(opened.title, "blob.bin");
        assert!(opened.newly_opened);
        assert_eq!(session.tab_kind(opened.id), Some(TabKind::Binary));
        assert_eq!(session.binary_len(opened.id), Some(7));
        assert_eq!(session.binary_row_count(opened.id), Some(1));

        let rows = session.binary_rows(opened.id, 0, 4);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].offset, "00000000");
        assert!(rows[0].hex.starts_with("00 9f 92 96 00 01 02"));
    }

    #[test]
    fn open_file_with_hint_image_opens_an_image_tab_even_for_binary_looking_bytes() {
        // A hint bypasses the sniff entirely — an SVG's bytes would sniff
        // as `Text` otherwise.
        let (project_dir, _config, mut session) = session_with_project();
        let svg = project_dir.path().join("logo.svg");
        fs::write(&svg, b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>").unwrap();

        let opened = session
            .open_file_with_hint(&svg, Some(TabKind::Image))
            .unwrap();

        assert_eq!(opened.kind, TabKind::Image);
        assert_eq!(opened.title, "logo.svg");
        assert_eq!(session.tab_kind(opened.id), Some(TabKind::Image));
    }

    #[test]
    fn open_file_with_hint_none_falls_back_to_the_binary_sniff() {
        let (project_dir, _config, mut session) = session_with_project();
        let binary = project_dir.path().join("blob.bin");
        fs::write(&binary, [0u8, 159, 146, 150]).unwrap();

        let opened = session.open_file_with_hint(&binary, None).unwrap();

        assert_eq!(opened.kind, TabKind::Binary);
    }

    #[test]
    fn open_file_with_hint_focuses_an_already_open_tab_regardless_of_hint() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        fs::write(&path, "hello").unwrap();

        let first = session.open_file(&path).unwrap();
        assert!(first.newly_opened);
        assert_eq!(first.kind, TabKind::Text);

        // A disagreeing hint loses to US-3's focus-don't-duplicate rule.
        let second = session
            .open_file_with_hint(&path, Some(TabKind::Image))
            .unwrap();
        assert!(!second.newly_opened);
        assert_eq!(second.id, first.id);
        assert_eq!(second.kind, TabKind::Text);
    }

    #[test]
    fn a_text_file_opens_as_a_text_tab_with_no_hex_answers() {
        let (project_dir, _config, mut session) = session_with_project();

        let opened = session
            .open_file(&project_dir.path().join("a.txt"))
            .unwrap();

        assert_eq!(opened.kind, TabKind::Text);
        assert_eq!(session.tab_kind(opened.id), Some(TabKind::Text));
        assert_eq!(session.binary_len(opened.id), None);
        assert_eq!(session.binary_row_count(opened.id), None);
        assert!(session.binary_rows(opened.id, 0, 4).is_empty());
        assert_eq!(session.tab_content(opened.id).as_deref(), Some("alpha"));
    }

    #[test]
    fn open_file_reports_an_unreadable_file_as_an_error_not_as_binary() {
        // Previously an unsniffable file was reported as "is a binary file",
        // which was already misleading and becomes actively wrong now that
        // binary files open: it would open an empty hex view of a file that
        // isn't there.
        let (project_dir, _config, mut session) = session_with_project();
        let missing = project_dir.path().join("ghost.txt");

        let err = session.open_file(&missing).unwrap_err();

        assert_eq!(err.code(), AppError::CODE_OPEN_FILE);
        assert!(session.docs.is_empty(), "no tab may be created on failure");
    }

    #[test]
    fn editing_commands_refuse_a_binary_tab_and_say_why() {
        let (project_dir, _config, mut session) = session_with_project();
        let binary = project_dir.path().join("blob.bin");
        fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
        let id = session.open_file(&binary).unwrap().id;

        for err in [
            session.save_tab(id, "nope").unwrap_err(),
            session.edit_tab(id, "nope").unwrap_err(),
            session.reload_tab(id).unwrap_err(),
            session.save_buffer(id).unwrap_err(),
        ] {
            assert_eq!(err.code(), AppError::CODE_NOT_A_TEXT_TAB);
            assert!(err.to_string().contains("cannot be edited"));
        }

        // Not dirty, so closing it must not prompt, and it has no text.
        assert_eq!(session.tab_is_dirty(id), Some(false));
        assert_eq!(session.tab_content(id), None);
        // The file on disk is untouched by the refused writes.
        assert_eq!(fs::read(&binary).unwrap(), [0u8, 1, 2, 3]);
    }

    #[test]
    fn a_binary_tab_behaves_like_any_tab_for_title_path_rename_and_delete() {
        let (project_dir, _config, mut session) = session_with_project();
        let binary = project_dir.path().join("blob.bin");
        fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
        let id = session.open_file(&binary).unwrap().id;

        assert_eq!(session.tab_title(id).as_deref(), Some("blob.bin"));
        assert_eq!(session.tab_path(id).as_deref(), Some(binary.as_path()));

        let renamed = session.rename_entry(&binary, "other.bin").unwrap().unwrap();
        assert_eq!(renamed.id, id);
        assert_eq!(renamed.title, "other.bin");

        let new_path = project_dir.path().join("other.bin");
        let deleted = session.delete_entry(&new_path).unwrap().unwrap();
        assert_eq!(deleted.title, "other.bin (deleted)");
    }

    #[test]
    fn open_file_focuses_existing_tab_instead_of_duplicating() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");

        let first = session.open_file(&path).unwrap();
        assert!(first.newly_opened);
        assert_eq!(first.title, "a.txt");

        // Switch away, then re-open the same path.
        let other = session
            .open_file(&project_dir.path().join("b.txt"))
            .unwrap();
        assert_eq!(session.active_tab(), Some(other.id));

        let second = session.open_file(&path).unwrap();
        assert!(!second.newly_opened);
        assert_eq!(second.id, first.id);
        assert_eq!(session.active_tab(), Some(first.id));
        assert_eq!(session.docs.len(), 2);
    }

    #[test]
    fn tab_ids_are_monotonic_and_never_reused() {
        let (project_dir, _config, mut session) = session_with_project();
        let a = session
            .open_file(&project_dir.path().join("a.txt"))
            .unwrap();
        let b = session
            .open_file(&project_dir.path().join("b.txt"))
            .unwrap();
        assert!(b.id.raw() > a.id.raw());

        assert!(session.close_tab(a.id));
        fs::write(project_dir.path().join("c.txt"), "gamma").unwrap();
        let c = session
            .open_file(&project_dir.path().join("c.txt"))
            .unwrap();
        assert_ne!(c.id, a.id, "a closed tab's id must never be reissued");
        assert!(c.id.raw() > b.id.raw());
    }

    #[test]
    fn close_tab_clears_active_and_reports_unknown_ids() {
        let (project_dir, _config, mut session) = session_with_project();
        let a = session
            .open_file(&project_dir.path().join("a.txt"))
            .unwrap();
        assert_eq!(session.active_tab(), Some(a.id));

        assert!(session.close_tab(a.id));
        assert_eq!(session.active_tab(), None);
        assert!(!session.close_tab(a.id), "double close must be a no-op");
    }

    #[test]
    fn save_tab_writes_content_clears_dirty_and_suppresses_the_echo() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();
        session.set_tab_dirty(tab.id, true);

        session.save_tab(tab.id, "edited content").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "edited content");
        assert_eq!(session.tab_is_dirty(tab.id), Some(false));
        // The watcher's echo of our own write must not be an external change.
        assert_eq!(session.check_external_change(&path), None);
    }

    #[test]
    fn save_tab_with_unknown_id_is_no_such_tab() {
        let (_project_dir, _config, mut session) = session_with_project();
        let err = session.save_tab(TabId::from_raw(999), "x").unwrap_err();
        assert_eq!(err.code(), AppError::CODE_NO_SUCH_TAB);
        assert_eq!(err.to_string(), "no such tab");
    }

    #[test]
    fn save_tab_as_writes_to_the_new_path_and_retitles_the_tab() {
        let (project_dir, _config, mut session) = session_with_project();
        let old_path = project_dir.path().join("a.txt");
        let new_path = project_dir.path().join("a-copy.txt");
        let tab = session.open_file(&old_path).unwrap();

        session
            .save_tab_as(tab.id, new_path.clone(), "copied content")
            .unwrap();

        assert_eq!(fs::read_to_string(&new_path).unwrap(), "copied content");
        assert_eq!(session.tab_title(tab.id).unwrap(), "a-copy.txt");
        assert_eq!(session.tab_is_dirty(tab.id), Some(false));
        // The watcher's echo of our own write to the new path must not be an
        // external change.
        assert_eq!(session.check_external_change(&new_path), None);
    }

    #[test]
    fn save_tab_as_with_unknown_id_is_no_such_tab() {
        let (project_dir, _config, mut session) = session_with_project();
        let err = session
            .save_tab_as(TabId::from_raw(999), project_dir.path().join("x.txt"), "x")
            .unwrap_err();
        assert_eq!(err.code(), AppError::CODE_NO_SUCH_TAB);
    }

    #[test]
    fn open_tabs_reflects_opening_order_and_titles() {
        let (project_dir, _config, mut session) = session_with_project();
        let a = session
            .open_file(&project_dir.path().join("a.txt"))
            .unwrap();
        fs::write(project_dir.path().join("b.txt"), "beta").unwrap();
        let b = session
            .open_file(&project_dir.path().join("b.txt"))
            .unwrap();

        assert_eq!(
            session.open_tabs(),
            vec![(a.id, "a.txt".to_string()), (b.id, "b.txt".to_string())]
        );
    }

    #[test]
    fn tab_path_returns_the_backing_file_and_none_for_unknown_ids() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();

        assert_eq!(session.tab_path(tab.id), Some(path));
        assert_eq!(session.tab_path(TabId::from_raw(9999)), None);
    }

    #[test]
    fn project_tree_entries_lists_every_node_except_the_root() {
        let (project_dir, _config, session) = session_with_project();
        let mut entries = session.project_tree_entries();
        entries.sort();

        let mut expected = vec![
            (project_dir.path().join("a.txt"), false),
            (project_dir.path().join("b.txt"), false),
        ];
        expected.sort();
        assert_eq!(entries, expected);
    }

    #[test]
    fn project_tree_entries_is_empty_with_no_project_open() {
        let config_dir = tempfile::tempdir().unwrap();
        let session = AppSession::with_config_dir(config_dir.path().to_path_buf());
        assert!(session.project_tree_entries().is_empty());
    }

    #[test]
    fn cursor_position_round_trips_and_ignores_unknown_tabs() {
        let (project_dir, _config, mut session) = session_with_project();
        let tab = session
            .open_file(&project_dir.path().join("a.txt"))
            .unwrap();

        assert_eq!(session.cursor_position(tab.id), None);
        session.set_cursor_position(tab.id, 3, 7);
        assert_eq!(session.cursor_position(tab.id), Some((3, 7)));

        session.set_cursor_position(TabId::from_raw(999), 1, 1);
        assert_eq!(session.cursor_position(TabId::from_raw(999)), None);
    }

    #[test]
    fn tab_file_name_is_the_backing_files_name() {
        let (project_dir, _config, mut session) = session_with_project();
        let rust_tab = session
            .open_file(&project_dir.path().join("a.txt"))
            .unwrap();
        assert_eq!(session.tab_file_name(rust_tab.id).as_deref(), Some("a.txt"));

        fs::write(project_dir.path().join("no_ext"), "x").unwrap();
        let no_ext_tab = session
            .open_file(&project_dir.path().join("no_ext"))
            .unwrap();
        assert_eq!(
            session.tab_file_name(no_ext_tab.id).as_deref(),
            Some("no_ext")
        );

        assert_eq!(session.tab_file_name(TabId::from_raw(999)), None);
    }

    #[test]
    fn content_for_path_returns_unsaved_buffer_text() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();

        session.edit_tab(tab.id, "alpha edited").unwrap();

        // The buffer, not the file: disk still says "alpha".
        assert_eq!(fs::read_to_string(&path).unwrap(), "alpha");
        assert_eq!(
            session.content_for_path(&path).as_deref(),
            Some("alpha edited")
        );
    }

    #[test]
    fn content_for_path_is_none_for_a_file_that_is_not_open() {
        let (project_dir, _config, session) = session_with_project();

        assert!(session
            .content_for_path(&project_dir.path().join("b.txt"))
            .is_none());
    }

    #[test]
    fn edit_tab_updates_content_and_dirty_flag_without_writing_to_disk() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();

        session.edit_tab(tab.id, "edited in memory").unwrap();

        assert_eq!(session.tab_content(tab.id).unwrap(), "edited in memory");
        assert_eq!(session.tab_is_dirty(tab.id), Some(true));
        assert_eq!(fs::read_to_string(&path).unwrap(), "alpha");
    }

    #[test]
    fn edit_tab_with_unknown_id_is_no_such_tab() {
        let (_project_dir, _config, mut session) = session_with_project();
        let err = session.edit_tab(TabId::from_raw(999), "x").unwrap_err();
        assert_eq!(err.code(), AppError::CODE_NO_SUCH_TAB);
    }

    #[test]
    fn save_buffer_writes_the_tabs_current_content_and_clears_dirty() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();
        session.edit_tab(tab.id, "saved via mcp").unwrap();

        session.save_buffer(tab.id).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "saved via mcp");
        assert_eq!(session.tab_is_dirty(tab.id), Some(false));
        // The watcher's echo of our own write must not be an external change.
        assert_eq!(session.check_external_change(&path), None);
    }

    #[test]
    fn save_buffer_with_unknown_id_is_no_such_tab() {
        let (_project_dir, _config, mut session) = session_with_project();
        let err = session.save_buffer(TabId::from_raw(999)).unwrap_err();
        assert_eq!(err.code(), AppError::CODE_NO_SUCH_TAB);
    }

    #[test]
    fn rename_entry_computes_the_new_path_and_retargets_the_open_tab() {
        let (project_dir, _config, mut session) = session_with_project();
        let old_path = project_dir.path().join("a.txt");
        let new_path = project_dir.path().join("renamed.txt");
        let tab = session.open_file(&old_path).unwrap();

        let retitled = session.rename_entry(&old_path, "renamed.txt").unwrap();

        let retitled = retitled.expect("the open tab must be retargeted");
        assert_eq!(retitled.id, tab.id);
        assert_eq!(retitled.title, "renamed.txt");
        assert!(!old_path.exists());
        assert!(new_path.is_file());
        // Future saves land on the new path…
        session.save_tab(tab.id, "after rename").unwrap();
        assert_eq!(fs::read_to_string(&new_path).unwrap(), "after rename");
        // …and the watcher's echo of the rename is suppressed.
        assert_eq!(session.check_external_change(&new_path), None);
    }

    #[test]
    fn rename_entry_without_an_open_tab_retitles_nothing() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let retitled = session.rename_entry(&path, "renamed.txt").unwrap();
        assert!(retitled.is_none());
    }

    #[test]
    fn rename_entry_failure_surfaces_the_file_op_code() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        // b.txt already exists, so the rename must fail.
        let err = session.rename_entry(&path, "b.txt").unwrap_err();
        assert_eq!(err.code(), AppError::CODE_FILE_OP);
        assert!(path.exists(), "original must be untouched on error");
    }

    #[test]
    fn delete_entry_invalidates_the_open_tab_and_blocks_saves() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();

        let retitled = session.delete_entry(&path).unwrap();

        let retitled = retitled.expect("the open tab must be flagged");
        assert_eq!(retitled.id, tab.id);
        assert_eq!(retitled.title, "a.txt (deleted)");
        assert!(!path.exists());
        // A deleted tab must not silently write to nowhere (US-4)…
        let err = session.save_tab(tab.id, "x").unwrap_err();
        assert_eq!(err.code(), AppError::CODE_SAVE);
        // …and watcher events for it need no further prompt.
        assert_eq!(session.check_external_change(&path), None);
    }

    #[test]
    fn delete_entry_without_an_open_tab_retitles_nothing() {
        let (project_dir, _config, mut session) = session_with_project();
        let retitled = session
            .delete_entry(&project_dir.path().join("a.txt"))
            .unwrap();
        assert!(retitled.is_none());
    }

    #[test]
    fn external_change_is_reported_only_for_open_undeleted_unsuppressed_tabs() {
        let (project_dir, _config, mut session) = session_with_project();
        let open_path = project_dir.path().join("a.txt");
        let closed_path = project_dir.path().join("b.txt");
        let tab = session.open_file(&open_path).unwrap();

        // A path with no open tab: nothing to prompt about.
        assert_eq!(session.check_external_change(&closed_path), None);
        // A genuinely external change to an open tab: prompt.
        assert_eq!(session.check_external_change(&open_path), Some(tab.id));
    }

    #[test]
    fn suppression_expires_after_the_window() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();
        session.save_tab(tab.id, "our own write").unwrap();
        assert_eq!(session.check_external_change(&path), None);

        expire_suppression(&mut session, &path);
        assert_eq!(
            session.check_external_change(&path),
            Some(tab.id),
            "an old suppression entry must not mask a real external change"
        );
    }

    #[test]
    fn create_file_and_folder_rebuild_the_tree() {
        let (project_dir, _config, mut session) = session_with_project();
        session.create_file(project_dir.path(), "new.txt").unwrap();
        session.create_folder(project_dir.path(), "newdir").unwrap();

        let tree = &session.project().unwrap().tree;
        let names: Vec<&str> = tree
            .children(tree.root_id())
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert!(names.contains(&"new.txt"));
        assert!(names.contains(&"newdir"));
    }

    #[test]
    fn create_file_errors_when_name_taken() {
        let (project_dir, _config, mut session) = session_with_project();
        let err = session
            .create_file(project_dir.path(), "a.txt")
            .unwrap_err();
        assert_eq!(err.code(), AppError::CODE_FILE_OP);
    }

    #[test]
    fn reload_tab_discards_in_editor_edits() {
        let (project_dir, _config, mut session) = session_with_project();
        let path = project_dir.path().join("a.txt");
        let tab = session.open_file(&path).unwrap();
        session.set_tab_dirty(tab.id, true);

        fs::write(&path, "changed externally").unwrap();
        session.reload_tab(tab.id).unwrap();

        assert_eq!(session.tab_content(tab.id).unwrap(), "changed externally");
        assert_eq!(session.tab_is_dirty(tab.id), Some(false));
    }

    #[test]
    fn reopen_last_project_round_trips_through_the_config_dir() {
        let project_dir = tempfile::tempdir().unwrap();
        fs::write(project_dir.path().join("x.txt"), "x").unwrap();
        let config_dir = tempfile::tempdir().unwrap();

        let mut first = AppSession::with_config_dir(config_dir.path().to_path_buf());
        first.open_project(project_dir.path()).unwrap();

        // Simulate a fresh launch: new session, same config dir.
        let mut second = AppSession::with_config_dir(config_dir.path().to_path_buf());
        assert!(second.reopen_last_project());
        assert_eq!(second.root_path().unwrap(), project_dir.path());

        // And with nothing persisted: silent no-op, no project.
        let empty_config = tempfile::tempdir().unwrap();
        let mut third = AppSession::with_config_dir(empty_config.path().to_path_buf());
        assert!(!third.reopen_last_project());
        assert!(third.project().is_none());
    }

    // --- navigation history (N5) ---

    fn at(path: &str, line: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            line,
            column: 0,
        }
    }

    #[test]
    fn history_walks_back_and_forward_through_recorded_jumps() {
        let mut history = NavigationHistory::default();
        history.record(at("a.rs", 10));
        history.record(at("b.rs", 20));
        history.record(at("c.rs", 30));

        assert_eq!(history.back(), Some(at("b.rs", 20)));
        assert_eq!(history.back(), Some(at("a.rs", 10)));
        assert_eq!(history.back(), None, "at the oldest entry");
        assert_eq!(history.forward(), Some(at("b.rs", 20)));
        assert_eq!(history.forward(), Some(at("c.rs", 30)));
        assert_eq!(history.forward(), None, "at the newest entry");
    }

    #[test]
    fn recording_a_jump_drops_the_forward_tail() {
        let mut history = NavigationHistory::default();
        history.record(at("a.rs", 10));
        history.record(at("b.rs", 20));
        history.back();

        history.record(at("c.rs", 30));

        assert!(!history.can_go_forward(), "b.rs was ahead and is gone");
        assert_eq!(history.back(), Some(at("a.rs", 10)));
    }

    #[test]
    fn nearby_positions_in_one_file_collapse_instead_of_stacking() {
        let mut history = NavigationHistory::default();
        history.record(at("a.rs", 10));
        history.record(at("a.rs", 11));
        history.record(at("a.rs", 10));

        assert!(
            !history.can_go_back(),
            "three near-identical positions are one entry"
        );
        // The collapsed entry holds the most recent position.
        history.record(at("b.rs", 1));
        history.back();
        assert_eq!(history.forward(), Some(at("b.rs", 1)));
    }

    #[test]
    fn a_distant_position_in_the_same_file_is_its_own_entry() {
        let mut history = NavigationHistory::default();
        history.record(at("a.rs", 10));
        history.record(at("a.rs", 200));

        assert_eq!(history.back(), Some(at("a.rs", 10)));
    }

    #[test]
    fn history_is_capped_and_drops_the_oldest_entries() {
        let mut history = NavigationHistory::default();
        for line in 0..(NAVIGATION_HISTORY_CAP as u32 + 10) {
            // Spaced beyond SAME_PLACE_LINES so nothing collapses.
            history.record(at("a.rs", line * 10));
        }
        let mut walked = 1;
        while history.back().is_some() {
            walked += 1;
        }
        assert_eq!(walked, NAVIGATION_HISTORY_CAP);
    }

    #[test]
    fn an_empty_history_goes_nowhere() {
        let mut history = NavigationHistory::default();
        assert!(!history.can_go_back());
        assert!(!history.can_go_forward());
        assert_eq!(history.back(), None);
        assert_eq!(history.forward(), None);
    }

    #[test]
    fn session_exposes_the_history_as_commands() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = AppSession::with_config_dir(dir.path().to_path_buf());

        assert!(!session.can_jump_back());
        session.record_jump(PathBuf::from("a.rs"), 1, 0);
        session.record_jump(PathBuf::from("b.rs"), 2, 4);

        assert!(session.can_jump_back());
        assert_eq!(
            session.jump_back(),
            Some(Location {
                path: PathBuf::from("a.rs"),
                line: 1,
                column: 0
            })
        );
        assert!(session.can_jump_forward());
    }
}
