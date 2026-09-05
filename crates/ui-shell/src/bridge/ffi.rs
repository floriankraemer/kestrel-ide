// cxx-qt bridge boundary for ui-shell.
//
// Adapter layer only (ADR-0002): the two QObjects here — `ProjectTreeModel`
// (a `QAbstractItemModel` over the project tree) and `DocumentManager` (the
// open-tab surface for the tab strip) — hold no domain state and decide
// nothing. They share the single `app_core::AppSession` and translate:
// slot → QString/QModelIndex → `AppSession` call → emit signal / refresh
// model. Errors cross as a typed code + message struct and tabs are
// identified by stable `TabId`s (ADR-0003).

// cxx-qt resolves everything `mod ffi` declares against its parent module
// and rejects any other path — `type T = super::TRust` is the only spelling
// it accepts — so the state structs and the `extern "Rust"` items are named
// here, next to the bridge, and defined in the feature module that owns
// them.
use crate::bridge::ai::chat::AiChatRust;
use crate::bridge::build::BuildServiceRust;
use crate::bridge::convert::{new_syntax_highlighter, syntax_scope_names, SyntaxHighlighterHandle};
use crate::bridge::debug::DebugServiceRust;
use crate::bridge::editor::DocumentManagerRust;
use crate::bridge::editor_ops::EditorOpsRust;
use crate::bridge::icons::IconProviderRust;
use crate::bridge::language::LanguageServiceRust;
use crate::bridge::plugins::PluginCatalogRust;
use crate::bridge::preview::PreviewProviderRust;
use crate::bridge::run::{RunConfigEditorRust, RunServiceRust};
use crate::bridge::search::SearchModelRust;
use crate::bridge::settings::{
    AiProviderEditorRust, AppSettingsRust, EditingEditorRust, KeymapEditorRust,
    LanguageCatalogRust, LanguageServerEditorRust, SyntaxColorEditorRust,
};
use crate::bridge::terminal::TerminalSupervisorRust;
use crate::bridge::tree::ProjectTreeModelRust;
use crate::bridge::vcs::VcsServiceRust;

#[cxx_qt::bridge]
mod ffi {
    /// Typed command result crossing the FFI seam (ADR-0003): `code` is the
    /// stable `app_core::AppError` code (0 = success), `message` the
    /// user-facing text shown verbatim. The UI branches on `code`, never on
    /// the message — the `QString`-sentinel convention ("" = success) is
    /// banned.
    #[derive(Default)]
    struct FfiResult {
        code: i32,
        message: QString,
    }

    /// `FfiResult` plus the tab the command yielded — `openFile`'s return.
    /// `tab_id` is 0 (the "no tab" sentinel; real ids start at 1) when
    /// `code` is non-zero.
    #[derive(Default)]
    struct FfiOpenResult {
        code: i32,
        message: QString,
        tab_id: u64,
    }

    /// One row of the binary (hex) viewer, 1:1 with `editor_core::HexRow`.
    ///
    /// Three ready-to-paint strings, not bytes: the offset format, the byte
    /// grouping, which bytes count as printable and what stands in for the
    /// ones that don't are all decided in `editor-core` (ADR-0002), so the
    /// widget only lays these out in three columns.
    #[derive(Default)]
    struct FfiHexRow {
        offset: QString,
        hex: QString,
        ascii: QString,
    }

    /// Persisted window geometry (L1), 1:1 with `app_config::WindowGeometry`.
    /// A freshly-defaulted value (all zero) means "nothing saved yet" — the
    /// view falls back to its own default size in that case.
    #[derive(Default)]
    struct FfiWindowGeometry {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    }

    /// Editor font (S2), 1:1 with `Settings::editor_font_family`/`_size`.
    /// Always resolved (`editor_font_family_or_default`/`_size_or_default`)
    /// — never empty/zero — so the view never has to fall back itself.
    #[derive(Default)]
    struct FfiEditorFont {
        family: QString,
        size: u32,
    }

    /// Interface font scales in percent, one per area of chrome that gets its
    /// own knob. Always resolved and clamped by `app-config`, so the view
    /// applies what it is given without range-checking it.
    #[derive(Default)]
    struct FfiUiFontScales {
        /// Everything that has no scale of its own: tabs, docks, dialogs,
        /// the status bar.
        ui: u32,
        project_tree: u32,
        menu: u32,
    }

    /// Editor text colors (S2), hex strings ("#rrggbb") or empty for "use
    /// the theme's default palette role" — the view (not this struct)
    /// decides what empty means.
    #[derive(Default)]
    struct FfiEditorColors {
        background: QString,
        foreground: QString,
        current_line: QString,
    }

    /// JetBrains-style "show whitespace characters" (task
    /// show-whitespace-characters), 1:1 with the five `Settings::show_*`
    /// fields. Bundled into one struct rather than five get/set pairs,
    /// matching how `FfiEditorFont`/`FfiEditorColors` bundle their related
    /// settings above.
    #[derive(Default)]
    struct FfiWhitespaceOptions {
        enabled: bool,
        leading: bool,
        inner: bool,
        trailing: bool,
        eol_markers: bool,
    }

    /// One row of the Keymap settings page, 1:1 with `app_config::Binding`.
    /// `shortcut` is `QKeySequence` portable text, empty for "unbound";
    /// `is_default` is resolved in Rust so the view can style rebound rows
    /// without re-deriving the rule.
    #[derive(Default)]
    struct FfiKeyBinding {
        action_id: QString,
        label: QString,
        category: QString,
        shortcut: QString,
        is_default: bool,
    }

    /// A classified span within the text passed to `highlight_line`, in
    /// UTF-8 byte offsets (matching `syntax_core::HighlightSpan`) — not
    /// `ui-shell`'s usual QString/UTF-16 offsets, since classification
    /// happens on the UTF-8 buffer the Rust side receives. The view maps
    /// these back to UTF-16 offsets itself.
    struct FfiHighlightSpan {
        start: usize,
        end: usize,
        /// Index into `syntax_core::SCOPES`, carried as a bare id on
        /// purpose: a cxx enum would make every new scope a bridge
        /// change. ADR-0003 governs error shapes and entity identity;
        /// this is neither — it indexes a table the view fetches through
        /// `syntax_scope_names()` in the same session. The view MUST
        /// range-guard it against that table.
        scope: u16,
    }

    /// One resolved entry of a syntax palette (T3), at the index that is
    /// its `syntax_core::Scope` id — the same `u16` `FfiHighlightSpan`
    /// carries. Every colour rule (theme lookup, user override
    /// precedence, parent-scope inheritance) has already been applied on
    /// the Rust side; the view only paints.
    ///
    /// `has_fg == false` means "no colour of this scope's own": the
    /// editor's default foreground, which is what an invalid `QColor`
    /// used to mean in `syntax_highlighter.cpp`.
    struct FfiScopeStyle {
        has_fg: bool,
        red: u8,
        green: u8,
        blue: u8,
        bold: bool,
        italic: bool,
        underline: bool,
    }

    /// One classified space/tab character, from `EditorOps::whitespaceSpans`
    /// (show-whitespace-characters task). `line`/`column` are 0-based,
    /// relative to the multi-line text the call was made with — not
    /// absolute document positions, since the view only ever asks about
    /// its currently visible blocks and maps `line` back to a `QTextBlock`
    /// number itself. `category` is a bare integer rather than a second
    /// cxx enum for one field's worth of values, the same call
    /// `line_ops.rs`'s `LINE_OP_*` constants make: 0 = leading, 1 = inner,
    /// 2 = trailing (`editor_core::whitespace::WhitespaceCategory`).
    struct FfiWhitespaceSpan {
        line: u32,
        column: u32,
        is_tab: bool,
        category: u8,
    }

    /// One in-editor find match, as a half-open `[start, end)` range of
    /// UTF-16 code units — the unit `QTextCursor::setPosition` takes, so
    /// the view can use these directly without an offset table (unlike
    /// `FfiHighlightSpan`, which stays in UTF-8 to match `syntax_core`).
    struct FfiTextMatch {
        start: u32,
        end: u32,
    }

    /// One project-wide replace target, addressed exactly like the
    /// `searchMatchFound` payload it came from: 1-based `line`, byte offsets
    /// within that line.
    #[derive(Default)]
    struct FfiFileReplacement {
        path: QString,
        line: u32,
        start: u32,
        end: u32,
    }

    /// Which tier a Search Everywhere hit came from. The view uses it to
    /// group results under section headers and to decide what activating a
    /// row does (open a file, jump to a line, trigger an action).
    enum FfiHitKind {
        RecentFile,
        File,
        Symbol,
        Text,
        Action,
    }

    /// Which tiers a Search Everywhere query should run, mirroring the
    /// popup's tabs. Narrowing here rather than filtering in the view means
    /// the Files tab never greps the project and the Text tab never scans
    /// symbols — the work is skipped, not discarded.
    enum FfiTierFilter {
        All,
        Files,
        Symbols,
        Text,
        Actions,
    }

    /// One Search Everywhere hit, tier-agnostic on purpose: every tier
    /// produces the same row shape so the view renders one list rather than
    /// four.
    ///
    /// `text` is the primary label and the string `positions` (character
    /// offsets) highlight; `detail` is the dimmer secondary label. For file
    /// and text hits `path`/`line` address where to jump; for actions
    /// `action_id` names the command to trigger and everything else is
    /// empty.
    struct FfiSearchHit {
        kind: FfiHitKind,
        path: QString,
        line: u32,
        start: u32,
        end: u32,
        text: QString,
        detail: QString,
        action_id: QString,
        positions: Vec<u32>,
    }

    /// Structural symbol kind (Task D), 1:1 with `syntax_core::SymbolKind`.
    /// `Class` is only nominally the default — a row with no kind of its
    /// own carries `has_kind == false` and this value is not read.
    #[derive(Default)]
    enum FfiSymbolKind {
        #[default]
        Class,
        Struct,
        Enum,
        Interface,
        Method,
        Function,
        Field,
        Constant,
        Property,
        Constructor,
        EnumMember,
    }

    /// Which fixed-order group (Task 4b) a symbol belongs to among its
    /// siblings under the same parent in the Class View tree, 1:1 with
    /// `syntax_core::SymbolCategory`. An ordinal rather than a label: the
    /// view groups children by this value alone (equal category -> same
    /// group, groups created in ascending order) and never has to know
    /// which `FfiSymbolKind`s make up a group — that mapping is a business
    /// rule and stays in Rust (CLAUDE.md's hard layering rule).
    #[derive(Default)]
    enum FfiSymbolCategory {
        #[default]
        Constants,
        Fields,
        Properties,
        Constructors,
        Methods,
        NestedTypes,
        Other,
    }

    /// One entry of `DocumentManager::tabOutline`'s flattened tree (Task
    /// D), matching `syntax_core::SymbolNode` minus its `children: Vec`
    /// (a directly self-recursive struct isn't needed here): `depth` is
    /// how many ancestors this symbol has (0 = a root), so the view
    /// reconstructs the tree by depth alone from this pre-order-flattened
    /// list — walk it in order, popping back to `depth` parents deep and
    /// pushing under whatever is left on top. `start`/`end` are the whole
    /// definition's UTF-8 byte range (used to jump/select it);
    /// `name_start`/`name_end` are just the identifier's (used to place
    /// the cursor exactly on the name) — both in the tab's UTF-8 buffer,
    /// same convention as `FfiHighlightSpan`. `category` (Task 4b) is this
    /// symbol's group among its siblings under the same parent; the view
    /// creates one group node per distinct category actually present, in
    /// `FfiSymbolCategory`'s declared order, and nests this item under it.
    struct FfiSymbolNode {
        name: QString,
        kind: FfiSymbolKind,
        category: FfiSymbolCategory,
        start: usize,
        end: usize,
        name_start: usize,
        name_end: usize,
        depth: u32,
    }

    /// A foldable region (Task C), UTF-8 byte offsets — same convention as
    /// `FfiHighlightSpan`, 1:1 with `syntax_core::FoldRange`. The view maps
    /// these back to UTF-16/block offsets itself.
    struct FfiFoldRange {
        start: usize,
        end: usize,
        anchor: usize,
    }

    /// One renderable terminal cell (Task F3), 1:1 with
    /// `terminal_core::RenderCell` minus its `char`/`CellColor`/
    /// `CellAttributes` Rust types, which cxx can't pass directly — `character`
    /// is always exactly one code point (never empty: blank cells are `' '`,
    /// matching `terminal_core`'s own convention).
    #[derive(Default)]
    struct FfiTerminalCell {
        character: QString,
        fg_r: u8,
        fg_g: u8,
        fg_b: u8,
        bg_r: u8,
        bg_g: u8,
        bg_b: u8,
        bold: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
        /// Inside the current mouse selection — the view paints it by
        /// swapping fg/bg, the same way it already handles `inverse`.
        selected: bool,
    }

    /// What a terminal mouse gesture selects (Task F4), 1:1 with
    /// `terminal_core::SelectionKind`.
    enum FfiSelectionKind {
        Simple,
        Word,
        Line,
    }

    /// One symbol row crossing the seam — a usage, an implementation, or
    /// a declaration candidate — 1:1 with `index_core::SymbolMatch`.
    ///
    /// Carried as one struct rather than eight signal parameters: these
    /// rows travel on three different signals, and a positional parameter
    /// list that long is both easy to mis-order at the call site and past
    /// what clippy will accept.
    ///
    /// `line` is 1-based; `column` is a byte offset within that line.
    /// `has_kind` distinguishes "no kind recorded" from `Class`, since a
    /// plain occurrence has no `tags.scm` entry of its own — a typed flag
    /// rather than an overloaded kind value (ADR-0003). `container` is
    /// empty when the symbol has none.
    #[derive(Default)]
    struct FfiSymbolMatch {
        path: QString,
        line: u32,
        column: u32,
        name: QString,
        kind: FfiSymbolKind,
        // Task 4b: meaningless when `has_kind == false`, same convention
        // as `kind` itself — the Project tier only groups a row into a
        // category when it has a kind to derive one from.
        category: FfiSymbolCategory,
        has_kind: bool,
        is_definition: bool,
        container: QString,
    }

    /// Which tier of `index_core::resolve_declaration` produced the
    /// candidates (N2), 1:1 with `index_core::ResolutionTier`. The view
    /// uses it only to phrase its status message — it never re-ranks.
    enum FfiResolutionTier {
        LocalFile,
        Project,
        None,
    }

    /// A place in the project to jump to, as `DocumentManager`'s
    /// navigation-history invokables return it (N5). `found == false`
    /// means "there is nowhere to go", at which point the other fields are
    /// meaningless — a typed flag rather than an empty-`QString` sentinel
    /// (ADR-0003), the same shape `FfiTerminalLink` uses.
    #[derive(Default)]
    struct FfiLocation {
        found: bool,
        path: QString,
        line: u32,
        column: u32,
    }

    /// `TerminalSession::linkAt`'s result. `found == false` means "no link
    /// at that cell", at which point the other fields are meaningless — a
    /// typed flag rather than an empty-`QString` sentinel (ADR-0003).
    ///
    /// Two kinds of link live here, told apart by `is_file` (R2-6): a
    /// `http(s)` URL the grid recognised, which opens in a browser, and a
    /// `file:line[:col]` location `run_core::links` recognised, which opens
    /// in the editor. `url` is set for the first, `path`/`line`/`column`
    /// for the second; both carry the cell span so the view can underline
    /// what it is offering to open.
    #[derive(Default)]
    struct FfiTerminalLink {
        found: bool,
        url: QString,
        row: u32,
        start_col: u32,
        end_col: u32,
        is_file: bool,
        path: QString,
        line: u32,
        has_column: bool,
        column: u32,
    }

    /// Severity of one diagnostic, 1:1 with `lsp_core::Severity` — the
    /// worst-first order is the domain's, not the view's.
    enum FfiSeverity {
        Error,
        Warning,
        Information,
        Hint,
    }

    /// One row of the Problems panel / one squiggle, 1:1 with
    /// `lsp_core::DiagnosticRow`. `line` is 1-based and `column` 0-based,
    /// both counted in UTF-16 code units — which is what LSP speaks and what
    /// `QTextBlock`/`QTextCursor` count, so the view needs no conversion
    /// table (unlike `FfiHighlightSpan`'s UTF-8 byte offsets).
    struct FfiDiagnostic {
        path: QString,
        line: u32,
        column: u32,
        end_line: u32,
        end_column: u32,
        severity: FfiSeverity,
        message: QString,
        source: QString,
    }

    /// One place a language server says a symbol is defined (L4), 1:1 with
    /// `lsp_core::DefinitionTarget`. Same units as `FfiDiagnostic`: `line`
    /// 1-based, `column` 0-based, both UTF-16 code units.
    struct FfiDefinition {
        path: QString,
        line: u32,
        column: u32,
    }

    /// One completion candidate (L5), 1:1 with `lsp_core::CompletionItem`
    /// once it has been filtered and ordered. `insert` is the text to type —
    /// the server's `textEdit`, `insertText` or label, whichever it chose,
    /// with snippet placeholders already resolved. When `has_range` is true
    /// the server said which span to replace (0-based lines, UTF-16
    /// characters, the protocol's own units); otherwise the caller replaces
    /// the word the caret is in.
    struct FfiCompletionItem {
        label: QString,
        kind: QString,
        detail: QString,
        documentation: QString,
        insert: QString,
        has_range: bool,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
        /// How many UTF-16 characters before the caret the typed word
        /// occupies — what the view replaces when `has_range` is false.
        prefix_length: u32,
        /// C7: the server's own item, as JSON text — opaque here, carried
        /// only so `acceptCompletion`/`resolveCompletionPreview` can hand it
        /// back for `completionItem/resolve`. The view never reads it.
        resolve_data: QString,
    }

    /// One caret, as flat document positions in UTF-16 code units — the
    /// unit `QTextCursor::position()` counts in, so the view uses these
    /// directly rather than converting.
    ///
    /// `anchor == head` is a collapsed caret; `anchor > head` is a
    /// selection made backwards, and the direction is preserved because
    /// Shift+Left from the end of a selection has to shrink it, not flip it.
    #[derive(Default)]
    struct FfiCaret {
        anchor: u32,
        head: u32,
        /// Exactly one caret in a set is the primary. The view keeps it as
        /// its own `QTextCursor` — so scrolling, Find and the status bar
        /// keep working unchanged — and paints the rest itself.
        primary: bool,
    }

    /// One edit a refactoring makes, in the protocol's own units (0-based
    /// lines, UTF-16 characters — which is what `QTextCursor` counts too, so
    /// the view re-expresses these rather than converting them).
    ///
    /// `in_buffer` is not a hint the view may second-guess: `lsp_core`
    /// decided which documents are open and therefore spliced live, and
    /// which are rewritten on disk. The view routes by this flag.
    #[derive(Default)]
    struct FfiTextEdit {
        path: QString,
        in_buffer: bool,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
        new_text: QString,
    }

    /// What happened to a run of lines in a diff, 1:1 with
    /// `editor_core::diff::HunkKind` (F3-13).
    enum FfiHunkKind {
        Added,
        Removed,
        Modified,
    }

    /// One hunk of a text diff, 1:1 with `editor_core::diff::Hunk` — a
    /// half-open line range into each side, expressed as start+len because
    /// there is no `Range` to cross the seam with. `DiffView`'s change
    /// ribbon and F7/Shift+F7 hunk navigation are painted from these; no
    /// other logic reads them.
    struct FfiHunk {
        old_start: u32,
        old_len: u32,
        new_start: u32,
        new_len: u32,
        kind: FfiHunkKind,
    }

    /// Which pane of a `DiffView` an `FfiInlineSpan` highlights.
    enum FfiDiffSide {
        Old,
        New,
    }

    /// One intra-line changed span, 1:1 with `editor_core::diff::InlineSpan`
    /// — `start`/`end` are UTF-16 code units into `line`, the same units
    /// `FfiTextEdit` already uses, so `DiffView` can position a
    /// `QTextCursor` with them directly rather than re-deriving an offset.
    struct FfiInlineSpan {
        side: FfiDiffSide,
        line: u32,
        start: u32,
        end: u32,
    }

    /// Whole-file before/after text for one file in a pending change, for
    /// `DiffView`'s two panes (F3-13/F3-15). Hunks and inline spans are
    /// fetched separately — `pendingFileHunks`/`pendingFileSpans` and their
    /// `replacePreview*` counterparts — since a `Vec` field on a shared
    /// struct is not a shape cxx supports; every other list already crosses
    /// the seam as a method's own return value, so this follows the same
    /// convention instead of a new one.
    #[derive(Default)]
    struct FfiFileDiff {
        path: QString,
        old_text: QString,
        new_text: QString,
    }

    /// What a refactoring is about to do, for the confirm text and for the
    /// decision the view is not allowed to make: `touches_other_files` is
    /// what says whether a preview is required, computed in `lsp_core`.
    #[derive(Default)]
    struct FfiRefactorSummary {
        title: QString,
        document_count: u32,
        edit_count: u32,
        /// Files this refactoring creates, renames or deletes (F2-3). Shown
        /// alongside `edit_count` so "moves a type to a new file" reads as
        /// what it is rather than as a text edit that mentions a path.
        op_count: u32,
        touches_other_files: bool,
    }

    /// What kind of resource operation a `WorkspaceEdit` step performs.
    /// Mirrors `lsp_core::ResourceOp`'s three variants exactly — the bridge
    /// translates one to the other and decides nothing (ADR-0026).
    enum FfiResourceOpKind {
        Create,
        Rename,
        Delete,
    }

    /// One file a pending refactoring will create, rename or delete, for the
    /// preview to list as such rather than as a text edit. `new_path` is
    /// empty except for `Rename`.
    struct FfiResourceOp {
        kind: FfiResourceOpKind,
        path: QString,
        new_path: QString,
    }

    /// What kind of change a path has, staged or unstaged, 1:1 with
    /// `vcs_core::ChangeKind` plus `None` (no change of that kind) and
    /// `Untracked` (F3-12a) — the ADR-0003 "no `Option` at the seam"
    /// convention `FfiHeadInfo`-style enums already use elsewhere in this
    /// file.
    enum FfiChangeKind {
        None,
        Added,
        Modified,
        Deleted,
        TypeChanged,
        Untracked,
    }

    /// The two `vcs_core::VcsError` codes the view has to *act* on rather
    /// than merely display: an unmerged branch offers a force-delete, and
    /// dubious ownership offers to mark the folder safe.
    ///
    /// They are exported as an enum so `vcs_menu.cpp` names them instead of
    /// writing `705` and `710`, which is the C++ half of ADR-0003 §4's rule
    /// that no call site spells a code out. The numbers here are checked
    /// against `vcs-core`'s own constants by a test in `bridge::errors`, so
    /// the two cannot drift apart silently.
    enum FfiVcsErrorCode {
        UnmergedBranch = 705,
        DubiousOwnership = 710,
    }

    /// One path `VcsService::changedFiles` reports: `vcs_core::FileStatus`
    /// plus the untracked pile folded in as `unstaged: Untracked`, so the
    /// view reads one list rather than three.
    struct FfiChangedFile {
        path: QString,
        staged: FfiChangeKind,
        unstaged: FfiChangeKind,
    }

    /// One commit, 1:1 with `vcs_core::LogEntry` (F3-12d).
    struct FfiLogEntry {
        id: QString,
        summary: QString,
        author_name: QString,
        author_email: QString,
        /// Seconds since the Unix epoch, author time.
        author_time: i64,
    }

    /// One shell this machine offers (`pty_core::ShellCandidate`), for the
    /// terminal dock's "+" dropdown and the Terminal settings page. A
    /// struct rather than a pair of parallel string lists for the same
    /// reason `FfiBranch` below is one: `cxx`'s `Vec<T>` needs
    /// `T: ImplVec`, which `QString` alone does not satisfy.
    ///
    /// `id` is what gets stored in `settings.toml` and handed back to
    /// `start()`; `label` is only ever shown.
    struct FfiShellCandidate {
        id: QString,
        label: QString,
    }

    /// The `[terminal]` section as one row, for the Settings > Terminal
    /// page. `env` crosses as `KEY=VALUE` lines separated by `\n`, the same
    /// convention `FfiRunConfig::env` uses, since a `Vec` field on a shared
    /// struct is not a shape cxx supports.
    #[derive(Default)]
    struct FfiTerminalSettings {
        /// A `FfiShellCandidate::id`, or empty for the platform default.
        shell_id: QString,
        /// A shell named by path, which beats `shell_id` when set.
        shell_path: QString,
        /// Space-separated, like `FfiRunConfig::args`.
        shell_args: QString,
        /// Empty means the open project's root.
        start_directory: QString,
        env: QString,
    }

    /// One local branch name. `cxx`'s `Vec<T>` needs `T: ImplVec`, which
    /// `QString` alone does not satisfy — this one-field wrapper is what
    /// lets `branches()` cross as a list at all, the same reason
    /// `FfiResourceOp` wraps a `QString` rather than the seam carrying a
    /// bare `Vec<QString>` anywhere.
    struct FfiBranch {
        name: QString,
    }

    /// One run configuration, 1:1 with `run_core::RunConfig`
    /// (`app_config::RunConfigSetting`). `args` crosses space-joined — the
    /// same convention `FfiLanguageServerRow::args` already uses (shell-style
    /// quoting is the upgrade if a literal space in an argument ever
    /// matters, not a list editor) — and `env` as `KEY=VALUE` lines
    /// separated by `\n`, since a `Vec` field on a shared struct is not a
    /// shape cxx supports (see `FfiFileDiff`'s doc comment).
    #[derive(Default)]
    struct FfiRunConfig {
        id: QString,
        name: QString,
        program: QString,
        args: QString,
        cwd: QString,
        env: QString,
        /// The build tool this configuration belongs to
        /// (`run_core::ToolchainId::as_str`), empty for a hand-written one.
        /// A label for the view, never a decision it makes (R1-2).
        toolchain: QString,
        /// What that toolchain runs — a Cargo bin, an npm script, a Make
        /// target. Empty for a hand-written configuration.
        target: QString,
        /// Created on the fly by running from context, and evicted once
        /// `run_core::TEMPORARY_CAP` newer ones exist. The view shows these
        /// differently, the way IntelliJ italicises a temporary entry.
        temporary: bool,
        /// A second launch opens a second console instead of replacing the
        /// running one.
        allow_parallel: bool,
        /// The before-launch tasks (B2-4), one per line, in order:
        /// `build`, `run <configuration id>`, or `tool <program> [args…]`.
        /// A `\n`-separated string for the same reason `env` is one — a
        /// `Vec` field on a shared struct is not a shape cxx supports.
        before_launch: QString,
    }

    /// One frame of a stopped thread's stack (D3-3), 1:1 with
    /// `dap_core::StackFrame`. `path` is empty for a frame the adapter knows
    /// only by address — a runtime-internal frame — which the view shows
    /// without offering to open it.
    /// One line's inline debug values (D3-7): what to paint after the text
    /// of line `line` (1-based) while the debuggee is stopped.
    #[derive(Default)]
    struct FfiInlineValue {
        line: u32,
        text: QString,
    }

    struct FfiStackFrame {
        id: i64,
        name: QString,
        path: QString,
        line: u32,
        column: u32,
    }

    /// One of the debuggee's threads (D3-3).
    struct FfiDebugThread {
        id: i64,
        name: QString,
    }

    /// One variable, or one child of one (D3-4). A non-zero
    /// `variables_reference` means it has children to fetch on expansion —
    /// the view asks for them with `expand`, so a deep object costs one
    /// round trip per level the user actually opens.
    struct FfiVariable {
        name: QString,
        value: QString,
        type_name: QString,
        variables_reference: i64,
    }

    /// One row of the Show Running List popup (R2-5): a console this
    /// session started, and whether its process is still alive.
    #[derive(Clone, Default)]
    struct FfiRunningConsole {
        console_id: u64,
        config_id: QString,
        running: bool,
    }

    /// One styled span of the text a `consoleOutput` signal just carried
    /// (R2-1). `start`/`length` are offsets **in UTF-16 code units** into
    /// that signal's `text`, because that is what `QTextCursor` counts;
    /// `run_core` measures the same runs in bytes and `RunService`
    /// converts at this seam.
    ///
    /// `has_fg`/`has_bg` false means "the view's own default colour" — SGR
    /// 39/49 and a reset say exactly that, and substituting a concrete
    /// colour here would stop the console following the editor theme
    /// (ADR-0003's typed-flag rule rather than a sentinel colour).
    #[derive(Clone, Default)]
    struct FfiStyledRun {
        start: u32,
        length: u32,
        has_fg: bool,
        fg_r: u8,
        fg_g: u8,
        fg_b: u8,
        has_bg: bool,
        bg_r: u8,
        bg_g: u8,
        bg_b: u8,
        bold: bool,
        italic: bool,
        underline: bool,
        inverse: bool,
    }

    /// `RunService::resolveLink`'s result. `found == false` means "no link
    /// at that byte offset", the same typed-flag convention `FfiTerminalLink`
    /// and `FfiLocation` already use instead of an empty-`QString` sentinel
    /// (ADR-0003). `has_column` is false for a location with no column (e.g.
    /// Python's `File "...", line N`).
    #[derive(Default)]
    struct FfiResolvedLink {
        found: bool,
        path: QString,
        line: u32,
        has_column: bool,
        column: u32,
    }

    /// One blamed line, 1:1 with `vcs_core::BlameLine` (F3-12d).
    struct FfiBlameLine {
        line: u32,
        commit: QString,
        author_name: QString,
        author_email: QString,
        summary: QString,
        content: QString,
    }

    /// Why a name-based rename will not run, as a code rather than a
    /// message (ADR-0003) — the view has to *act* differently on one of
    /// these, not merely word it differently, and branching on a sentence
    /// would break the first time it was reworded.
    enum FfiRenameRefusal {
        /// The caret is not on a symbol this index resolved.
        Unresolved,
        /// The new name is not an identifier.
        InvalidName,
        /// Files are open with unsaved changes, which the index cannot see.
        /// The view offers to save them and try again.
        UnsavedChanges,
        /// The symbol resolved, but no occurrence of it was found.
        NoSites,
        /// The index could not answer at all — none built yet, or still
        /// building.
        Unavailable,
    }

    /// One occurrence a name-based rename would rewrite, as the preview
    /// lists it. `resolved` and `checked` are `index_core`'s judgements
    /// about how much this rename knows — the dialog paints them, it does
    /// not decide them.
    #[derive(Default)]
    struct FfiRenameSite {
        path: QString,
        line: u32,
        col: u32,
        resolved: bool,
        is_definition: bool,
        checked: bool,
    }

    /// One offer from `textDocument/codeAction`. `disabled_reason` is empty
    /// when the action is usable; a disabled action is still listed, greyed,
    /// because a menu that changes shape with the caret reads as a bug.
    #[derive(Default)]
    struct FfiCodeAction {
        title: QString,
        kind: QString,
        disabled_reason: QString,
    }

    /// Which section of the Alt+Enter popup a row belongs in, 1:1 with
    /// `lsp_core::IntentionGroup`. Declared in the order the menu is built,
    /// which the view relies on rather than re-deriving.
    enum FfiIntentionGroup {
        QuickFix,
        Refactor,
        Source,
        Other,
    }

    /// One row of the Alt+Enter popup (F2-8), 1:1 with `lsp_core::Intention`.
    /// A `disabled_reason`-carrying row is still listed, greyed, exactly as
    /// `FfiCodeAction`'s is.
    struct FfiIntention {
        title: QString,
        kind: QString,
        group: FfiIntentionGroup,
        preferred: bool,
        disabled_reason: QString,
    }

    /// The overload the tip shows (F2-9), reduced from `lsp_core::
    /// SignatureHelp`'s full overload set to what `signature_tip.cpp` paints:
    /// `resolved_signature()`'s label and doc, and `resolved_parameter()`'s
    /// span within that label to embolden. `signature_index`/`signature_
    /// count` are only for the "(1/3)" overload indicator; cycling overloads
    /// is not F2's scope. `has_signature` false means nothing to show — the
    /// default value, so a tip that never asked reads as empty rather than
    /// as overload zero of nothing.
    #[derive(Default)]
    struct FfiSignatureHelp {
        has_signature: bool,
        label: QString,
        documentation: QString,
        has_active_parameter: bool,
        parameter_start: u32,
        parameter_end: u32,
        signature_index: u32,
        signature_count: u32,
    }

    /// What kind of occurrence a document highlight is, 1:1 with
    /// `lsp_core::HighlightKind`.
    enum FfiHighlightKind {
        Text,
        Read,
        Write,
    }

    /// One occurrence of the symbol under the caret (F2-9), 1:1 with
    /// `lsp_core::DocumentHighlight`. Same units as `FfiTextEdit`: 0-based
    /// lines, UTF-16 characters.
    struct FfiDocumentHighlight {
        kind: FfiHighlightKind,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
    }

    /// What an inlay hint stands for, 1:1 with `lsp_core::InlayHintKind` —
    /// the view paints a type hint and a parameter-name hint differently.
    enum FfiInlayHintKind {
        Type,
        Parameter,
        Other,
    }

    /// One inlay hint (F2-9), 1:1 with `lsp_core::InlayHint`.
    struct FfiInlayHint {
        line: u32,
        character: u32,
        label: QString,
        kind: FfiInlayHintKind,
        padding_left: bool,
        padding_right: bool,
    }

    /// One code lens (C10), reduced to what the strip above/on `line`
    /// paints: a label, and whether a click does anything yet. A lens that
    /// still needs `codeLens/resolve` shows its placeholder label but is
    /// not `clickable` until the click itself triggers the resolve — see
    /// `LanguageService::runCodeLens`. Which lenses exist and what a click
    /// means is decided in Rust; the view only draws this and forwards a
    /// click back by index.
    struct FfiCodeLens {
        line: u32,
        label: QString,
        clickable: bool,
    }

    /// One call-hierarchy or type-hierarchy item (C11), 1:1 with
    /// `lsp_core::HierarchyItem` — the same shape either feature's item
    /// takes, since `CallHierarchyItem` and `TypeHierarchyItem` are
    /// structurally identical on the wire. `kind` is the raw LSP
    /// `SymbolKind` number, same convention `FfiCompletionItem`'s own
    /// `kind` follows, so the view picks the icon rather than Rust.
    struct FfiHierarchyItem {
        name: QString,
        detail: QString,
        path: QString,
        line: u32,
        column: u32,
        kind: u32,
    }

    /// One `callHierarchy/incomingCalls` entry (C11): who calls the item
    /// that was asked about, and how many call sites `fromRanges` counted —
    /// the dock draws one row per caller with that count, not the ranges
    /// themselves.
    struct FfiIncomingCall {
        from: FfiHierarchyItem,
        call_count: u32,
    }

    /// The `callHierarchy/outgoingCalls` twin of `FfiIncomingCall`.
    struct FfiOutgoingCall {
        to: FfiHierarchyItem,
        call_count: u32,
    }

    /// How many diagnostics of each severity exist right now, 1:1 with
    /// `lsp_core::DiagnosticCounts` — for the status-bar counter and the
    /// Problems panel's filter buttons.
    struct FfiDiagnosticCounts {
        errors: u32,
        warnings: u32,
        infos: u32,
        hints: u32,
    }

    /// What just happened to one language server. The view turns this into
    /// wording; nothing here decides whether or when to restart (that is
    /// `LspManager`'s job, ADR-0016).
    enum FfiServerState {
        Starting,
        Ready,
        Exited,
        Failed,
    }

    extern "Rust" {
        /// Opaque per-editor incremental highlighter handle (Y2/A1):
        /// wraps a `syntax_core::Highlighter`, which keeps a persistent
        /// `tree_sitter::Tree` and reparses incrementally rather than
        /// re-parsing the whole buffer on every keystroke. Owned by the
        /// C++ `SyntaxHighlighter` instance (one per open editor/tab) as
        /// a `rust::Box`, matching that type's own lifetime — no separate
        /// registry or `TabId` lookup needed since the box's lifetime
        /// already tracks the editor's.
        type SyntaxHighlighterHandle;

        /// Create a handle for `extension`'s language (`PlainText` for
        /// anything unrecognized, which is a cheap no-op — see
        /// `syntax_core::Highlighter`'s doc comment).
        fn new_syntax_highlighter(extension: &str) -> Box<SyntaxHighlighterHandle>;

        /// `syntax_core::SCOPES`, in id order: entry `i` is the canonical
        /// capture name of scope id `i`. The view builds its format table
        /// from this, so it keys colours off names and never off a
        /// hardcoded id, and its table is always exactly as long as the
        /// Rust one.
        fn syntax_scope_names() -> Vec<String>;

        /// Full (re)parse of `text`, discarding any previous incremental
        /// tree. Call once, on initial attach/file load.
        fn set_text(self: &mut SyntaxHighlighterHandle, text: &str) -> Vec<FfiHighlightSpan>;

        /// Incremental reparse: `new_text` is the full new document text;
        /// `start_byte..old_end_byte` is the byte range being replaced in
        /// the previous text, `start_byte..new_end_byte` the
        /// corresponding range in `new_text` (tree-sitter's `InputEdit`
        /// shape, byte offsets only — row/column is derived internally).
        fn apply_edit(
            self: &mut SyntaxHighlighterHandle,
            new_text: &str,
            start_byte: usize,
            old_end_byte: usize,
            new_end_byte: usize,
        ) -> Vec<FfiHighlightSpan>;

        /// Foldable regions (Task C) off the same incremental tree
        /// `set_text`/`apply_edit` just left current — no second parse.
        /// Call after either, on the same revision-change hook that
        /// already drives highlighting.
        fn fold_ranges(self: &SyntaxHighlighterHandle) -> Vec<FfiFoldRange>;

        /// The resolved syntax palette for `theme` and *this* handle's
        /// language, indexed by scope id and always exactly as long as
        /// `syntax_scope_names()`. User overrides are read from
        /// `settings.toml` here, so the view neither knows the config
        /// shape nor the precedence rules. Build once per (theme,
        /// language) — it is pure data afterwards.
        fn palette(self: &SyntaxHighlighterHandle, theme: &str) -> Vec<FfiScopeStyle>;

        /// C9: overlays `semantic` — `LanguageService::semanticTokenSpans`'s
        /// answer for this same document, already mapped onto
        /// `syntax_core`'s taxonomy and converted to byte offsets — onto
        /// the tree-sitter spans this handle produced at its last
        /// `set_text`/`apply_edit`. `lsp_core::semantic_tokens::overlay`
        /// decides the merge (semantic spans win where they cover; the
        /// tree-sitter colouring underneath still shows through
        /// everywhere else, per F0-16); this only carries its inputs and
        /// answer across the seam.
        ///
        /// Called from `EditorTabs::onSemanticTokensReady` (C9-followup)
        /// once `semanticTokensReady` fires, via
        /// `SyntaxHighlighter::applySemanticTokens`.
        fn overlay_semantic_tokens(
            self: &SyntaxHighlighterHandle,
            semantic: Vec<FfiHighlightSpan>,
        ) -> Vec<FfiHighlightSpan>;
    }

    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractItemModel>);
        /// Base Qt class `ProjectTreeModel` inherits from.
        #[qobject]
        type QAbstractItemModel;
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qbytearray.h");
        type QByteArray = cxx_qt_lib::QByteArray;
    }

    /// Extra data roles `data()` answers, alongside `Qt::DisplayRole` (0 —
    /// the node's name, used for the tree view's label).
    ///
    /// These are *offsets from `Qt::UserRole`*, not role numbers: cxx-qt's
    /// `qenum` doesn't support explicit discriminants, so the variants can
    /// only ever be 0, 1, 2..., which is squarely inside the range Qt
    /// reserves for itself. Both sides add `Qt::UserRole` before the number
    /// reaches `data()` — Rust through `user_role()` below, C++ through
    /// `Qt::UserRole + static_cast<int>(...)`. Without that, `Path` would be
    /// `Qt::DecorationRole` and the view would reserve icon width for the
    /// `QString` it got back, pushing every label ~22px right of the branch
    /// indicator that belongs to it.
    #[qenum(ProjectTreeModel)]
    enum Roles {
        /// Absolute filesystem path of the node, as a `QString`.
        Path,
        /// Whether the node is a directory (`bool`).
        IsDir,
        /// The row's icon key (`"<pack-id>/<icon-id>"`, as a `QString`), or
        /// an empty string when no icon theme is active.
        ///
        /// A custom role rather than `Qt::DecorationRole`: answering a
        /// Qt-defined role from here would put pixels in the Rust model and
        /// break the rule the comment above states. `IconDecorationProxy`
        /// (`cpp/icon_decoration_proxy.h`) turns this key into a decoration
        /// for the tree view, and P6's tab strip and result lists read the
        /// same keys straight off `IconProvider`.
        IconKey,
    }

    extern "RustQt" {
        /// `QAbstractItemModel` over the shared `AppSession`'s project tree
        /// (`project-model`'s arena-based `DirectoryTree`). The model's
        /// invisible root corresponds to the arena's root node (the open
        /// project folder); top-level rows are that folder's direct children.
        #[qobject]
        #[base = QAbstractItemModel]
        type ProjectTreeModel = super::ProjectTreeModelRust;
    }

    unsafe extern "RustQt" {
        /// # Safety
        ///
        /// Inherited `createIndex` from the base class.
        #[inherit]
        #[cxx_name = "createIndex"]
        unsafe fn create_index(
            self: &ProjectTreeModel,
            row: i32,
            column: i32,
            id: usize,
        ) -> QModelIndex;

        /// # Safety
        ///
        /// Inherited `beginResetModel`/`endResetModel` from the base class —
        /// bracket any full-tree replacement (open, mutation refresh, or a
        /// structural watcher event).
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut ProjectTreeModel>);
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut ProjectTreeModel>);
    }

    extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &ProjectTreeModel, parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "columnCount"]
        fn column_count(self: &ProjectTreeModel, _parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        fn index(
            self: &ProjectTreeModel,
            row: i32,
            column: i32,
            parent: &QModelIndex,
        ) -> QModelIndex;

        #[qinvokable]
        #[cxx_override]
        fn parent(self: &ProjectTreeModel, child: &QModelIndex) -> QModelIndex;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &ProjectTreeModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &ProjectTreeModel) -> QHash_i32_QByteArray;

        /// Whether the tree currently sorts descending (folders still lead
        /// either way — this only flips the name comparison within each
        /// group). Read fresh from `settings.toml` on every call, the same
        /// pattern `AppSettings::mcpEnabled` uses.
        #[qinvokable]
        #[cxx_name = "sortDescending"]
        fn sort_descending(self: &ProjectTreeModel) -> bool;

        /// Flip the sort direction, persist it, and reset the model.
        #[qinvokable]
        #[cxx_name = "setSortDescending"]
        fn set_sort_descending(self: Pin<&mut ProjectTreeModel>, descending: bool);

        /// Open `path` as the active project (persisted as last-opened) and
        /// reset the model to reflect the new tree. Fire-and-forget: the
        /// directory walk runs on a worker thread (ADR-0037), and the
        /// outcome arrives as `projectOpened` or `projectOpenFailed` rather
        /// than a synchronous return, so this never blocks the Qt thread.
        /// The current tree (if any) is left unchanged on failure (US-1).
        #[qinvokable]
        #[cxx_name = "openFolder"]
        fn open_folder(self: Pin<&mut ProjectTreeModel>, path: &QString);

        /// Absolute path of the open project's root folder, or an empty
        /// string if none is open. Used by the tree-view context menu to
        /// target "New File"/"New Folder" at the root when the user
        /// right-clicks empty space rather than a node (US-2b).
        #[qinvokable]
        #[cxx_name = "rootPath"]
        fn root_path(self: &ProjectTreeModel) -> QString;

        /// Create an empty file named `name` inside `parent_dir` and
        /// refresh the tree.
        #[qinvokable]
        #[cxx_name = "createFile"]
        fn create_file(
            self: Pin<&mut ProjectTreeModel>,
            parent_dir: &QString,
            name: &QString,
        ) -> FfiResult;

        /// Create an empty folder named `name` inside `parent_dir` and
        /// refresh the tree.
        #[qinvokable]
        #[cxx_name = "createFolder"]
        fn create_folder(
            self: Pin<&mut ProjectTreeModel>,
            parent_dir: &QString,
            name: &QString,
        ) -> FfiResult;

        /// Rename `path` (file or folder) to `new_name` in place and refresh
        /// the tree. The session computes the new path itself and retargets
        /// any open tab at it (US-2b) — `tabTitleChanged` is emitted for the
        /// affected tab; the old two-step C++ protocol is gone.
        #[qinvokable]
        #[cxx_name = "renamePath"]
        fn rename_path(
            self: Pin<&mut ProjectTreeModel>,
            path: &QString,
            new_name: &QString,
        ) -> FfiResult;

        /// Delete `path` (recursively if it's a folder) and refresh the
        /// tree. Any open tab on `path` is flagged deleted by the session
        /// (blocking further silent saves) and `tabTitleChanged` is emitted
        /// with its "(deleted)" title (US-2b).
        #[qinvokable]
        #[cxx_name = "deletePath"]
        fn delete_path(self: Pin<&mut ProjectTreeModel>, path: &QString) -> FfiResult;

        /// Reopen the last-persisted project (US-1's "relaunch reopens the
        /// last project" criterion) and start its filesystem watcher.
        /// Fire-and-forget like `openFolder` (ADR-0037): the walk itself
        /// runs on a worker thread, and `projectOpened`/`projectOpenFailed`
        /// report the outcome. Returns whether a reopen was kicked off at
        /// all — `false` (with the model left empty) only if nothing was
        /// ever persisted, which the caller (the splash screen) uses to know
        /// no `projectOpened`/`projectOpenFailed` is coming and it should
        /// stop waiting. `true` covers a persisted path that turns out to be
        /// missing or unreadable too — that failure surfaces asynchronously
        /// through `projectOpenFailed`, same as any other failed open, and
        /// startup deliberately does not turn it into a popup dialog before
        /// the window is even shown.
        #[qinvokable]
        #[cxx_name = "reopenLastProject"]
        fn reopen_last_project(self: Pin<&mut ProjectTreeModel>) -> bool;

        /// Emitted on the Qt thread after a filesystem-watcher event has
        /// already been folded into a tree rebuild + reset. `main_window.cpp`
        /// connects this to `DocumentManager::checkExternalChange` so an
        /// open tab whose backing file changed on disk gets the reload/keep
        /// prompt (US-3).
        #[qsignal]
        #[cxx_name = "filesChangedExternally"]
        fn files_changed_externally(self: Pin<&mut ProjectTreeModel>, path: QString);

        /// C5: the same filesystem-watcher event as `filesChangedExternally`,
        /// plus the LSP `FileChangeType` it maps onto (1=created, 2=changed,
        /// 3=deleted). `main_window.cpp` connects this to
        /// `LanguageService::watchedFileChanged`, which is the only consumer
        /// — the reload/keep-prompt path stays on `filesChangedExternally`
        /// and does not need the kind.
        #[qsignal]
        #[cxx_name = "watchedFileChanged"]
        fn watched_file_changed(self: Pin<&mut ProjectTreeModel>, path: QString, kind: i32);

        /// Emitted when a tree mutation (rename/delete) changed an open
        /// tab's title as a side effect (US-2b) — the tab strip updates its
        /// label in response, preserving the unsaved-changes indicator.
        /// Lives on this QObject (not `DocumentManager`) because the tree
        /// mutations are its slots; `main_window.cpp` wires it to the same
        /// tab-strip handler.
        #[qsignal]
        #[cxx_name = "tabTitleChanged"]
        fn tab_title_changed(self: Pin<&mut ProjectTreeModel>, tab_id: u64, title: QString);

        /// Emitted after `openFolder`/`reopenLastProject` successfully swap
        /// in a new project root (Task H) — `main_window.cpp` relays this to
        /// `SearchModel::buildIndex` so the text index is (re)built off the
        /// same project-open lifecycle event the tree/watcher already use,
        /// rather than a second, parallel "project opened" hook.
        #[qsignal]
        #[cxx_name = "projectOpened"]
        fn project_opened(self: Pin<&mut ProjectTreeModel>, root_path: QString);

        /// Emitted instead of `projectOpened` when `openFolder`/
        /// `reopenLastProject`'s off-thread walk fails (ADR-0037) — the
        /// typed-code-plus-message convention (ADR-0003), same as every
        /// other fallible slot's `FfiResult`, just delivered as a signal
        /// since the walk itself no longer has a synchronous return to
        /// carry it on. The current tree (if any) is left unchanged.
        #[qsignal]
        #[cxx_name = "projectOpenFailed"]
        fn project_open_failed(self: Pin<&mut ProjectTreeModel>, result: FfiResult);
    }

    // Enables `self.qt_thread()` on `ProjectTreeModel`, giving the
    // `notify` watcher thread (owned by `project-model`) a `CxxQtThread`
    // handle it can queue tree-rebuild closures onto safely — the only
    // cross-thread communication in the watcher design, no hand-rolled
    // synchronization.
    impl cxx_qt::Threading for ProjectTreeModel {}

    extern "RustQt" {
        /// `QObject` adapter for the shared `AppSession`'s open-document
        /// table — the tab strip's FFI surface. Owns nothing; the
        /// `QPlainTextEdit` widgets own live keystroke editing while Rust's
        /// `Document` owns the authoritative dirty flag (ADR-0003).
        #[qobject]
        type DocumentManager = super::DocumentManagerRust;

        /// Emitted when `openFile` opens a genuinely new tab (not when it
        /// just focuses an already-open one) — the tab strip appends a new
        /// page in response.
        #[qsignal]
        #[cxx_name = "tabOpened"]
        fn tab_opened(self: Pin<&mut DocumentManager>, tab_id: u64, title: QString);

        /// Emitted after `closeTab` actually removes a tab — the tab strip
        /// removes the corresponding page in response.
        #[qsignal]
        #[cxx_name = "tabClosed"]
        fn tab_closed(self: Pin<&mut DocumentManager>, tab_id: u64);

        /// Emitted when a tab's dirty flag changes (via `setTabModified` or
        /// a successful `saveTab`) — the tab strip updates its
        /// unsaved-changes indicator in response.
        #[qsignal]
        #[cxx_name = "tabModifiedChanged"]
        fn tab_modified_changed(self: Pin<&mut DocumentManager>, tab_id: u64, modified: bool);

        /// Emitted from `checkExternalChange` when the session's watcher
        /// policy decided the change is genuinely external to an open,
        /// still-existing tab — `main_window.cpp` shows the reload/keep
        /// prompt in response (US-3).
        #[qsignal]
        #[cxx_name = "externalChangeDetected"]
        fn external_change_detected(self: Pin<&mut DocumentManager>, tab_id: u64, path: QString);

        /// Emitted after MCP's `edit_buffer` tool (M5) changes a tab's
        /// content — the tab strip replaces the widget's text so the edit
        /// is visible, the same "session decides, view displays" split
        /// every other cross-thread/external mutation in this file uses.
        #[qsignal]
        #[cxx_name = "bufferEditedExternally"]
        fn buffer_edited_externally(self: Pin<&mut DocumentManager>, tab_id: u64, content: QString);

        /// Emitted when a find/replace pattern does not compile. A
        /// `Vec<T>` return has no room for an error code, and ADR-0003
        /// bans a sentinel value, so the failure travels as its own typed
        /// signal (the shape `SearchModel::searchFailed` already uses) and
        /// the invokable returns an empty vec.
        #[qsignal]
        #[cxx_name = "findPatternInvalid"]
        fn find_pattern_invalid(self: Pin<&mut DocumentManager>, message: QString);

        /// Every match of `pattern` in `text`, in document order.
        ///
        /// `text` is the widget's *current* buffer, passed in rather than
        /// read from the session: `Document`'s rope only catches up at
        /// save time, so searching it would search pre-edit text. Same
        /// reason `saveTab` takes its content.
        #[qinvokable]
        #[cxx_name = "findMatches"]
        fn find_matches(
            self: Pin<&mut DocumentManager>,
            text: &QString,
            pattern: &QString,
            is_regex: bool,
            case_sensitive: bool,
        ) -> Vec<FfiTextMatch>;

        /// The splice list for one Replace or Replace All gesture:
        /// `findMatches`' matches, each carrying its already
        /// capture-expanded (`$1`) replacement text, **descending** so the
        /// view can hand the whole thing to `EditorTabs::applyEditsTo`
        /// unmodified.
        ///
        /// A non-negative `index` selects the single match at that position
        /// in document order — Replace-this-one, whose index is the one the
        /// match counter shows. A negative `index` takes every match.
        /// Which spans those are, what replaces them, and what order they
        /// apply in are all `editor_core::search`'s call.
        #[qinvokable]
        #[cxx_name = "replacementEdits"]
        fn replacement_edits(
            self: Pin<&mut DocumentManager>,
            text: &QString,
            pattern: &QString,
            replacement: &QString,
            is_regex: bool,
            case_sensitive: bool,
            index: i32,
        ) -> Vec<FfiTextEdit>;

        /// Open `path` as a new tab, or focus its existing tab if already
        /// open (US-3: focus-not-duplicate). The session enforces the
        /// binary-open rule (US-2b); the UI branches on the returned code
        /// (`CODE_BINARY_FILE` gets an information dialog, other failures an
        /// error dialog). For a new tab, `tabOpened` is emitted before this
        /// returns.
        #[qinvokable]
        #[cxx_name = "openFile"]
        fn open_file(self: Pin<&mut DocumentManager>, path: &QString) -> FfiOpenResult;

        /// Close the tab `tab_id`. The caller (UI) is responsible for any
        /// unsaved-changes prompt before calling this.
        #[qinvokable]
        #[cxx_name = "closeTab"]
        fn close_tab(self: Pin<&mut DocumentManager>, tab_id: u64);

        /// Replace the tab's content with `content` and write it to disk
        /// (US-4: no silent data loss — the dirty flag is left set on
        /// failure).
        #[qinvokable]
        #[cxx_name = "saveTab"]
        fn save_tab(self: Pin<&mut DocumentManager>, tab_id: u64, content: &QString) -> FfiResult;

        /// Save As (L2): write `content` to `path`, repointing the tab at
        /// it (same reason `saveTab` takes `content` rather than reading
        /// the session's own copy — live keystrokes aren't marshalled
        /// through the rope, ADR-0003). On success the caller re-renders
        /// the tab's title (`tabTitle` now reflects the new path) — reuses
        /// the existing `tabModifiedChanged` signal rather than adding a
        /// new one.
        #[qinvokable]
        #[cxx_name = "saveTabAs"]
        fn save_tab_as(
            self: Pin<&mut DocumentManager>,
            tab_id: u64,
            path: &QString,
            content: &QString,
        ) -> FfiResult;

        /// Update which tab the session considers active.
        #[qinvokable]
        #[cxx_name = "setActiveTab"]
        fn set_active_tab(self: Pin<&mut DocumentManager>, tab_id: u64);

        /// Forward `QPlainTextEdit`'s own `QTextDocument::modificationChanged`
        /// notification into the authoritative Rust dirty flag (ADR-0003 —
        /// live keystrokes are not marshalled through the rope; the widget
        /// forwards its edit state and reads the flag back).
        #[qinvokable]
        #[cxx_name = "setTabModified"]
        fn set_tab_modified(self: Pin<&mut DocumentManager>, tab_id: u64, modified: bool);

        /// The tab's current buffer content, used to populate a newly
        /// created `QPlainTextEdit` page when a tab is opened.
        #[qinvokable]
        #[cxx_name = "tabContent"]
        fn tab_content(self: &DocumentManager, tab_id: u64) -> QString;

        /// C12-followup: whether the tab is read-only — a virtual document
        /// (decompiled/generated source with no backing file), a binary
        /// tab, or a diff tab. `EditorTabs::onTabOpened` uses this to build
        /// the `CodeEditor` with typing disabled rather than relying only
        /// on `AppSession::save_tab`'s refusal at save time.
        #[qinvokable]
        #[cxx_name = "tabIsReadOnly"]
        fn tab_is_read_only(self: &DocumentManager, tab_id: u64) -> bool;

        /// The tab's backing file name (`"main.rs"`, `"Dockerfile"`),
        /// empty when there is none — used to pick a highlighting language
        /// (Y2). File name, not extension: extensionless languages are
        /// matched by whole name in the language registry.
        #[qinvokable]
        #[cxx_name = "tabFileName"]
        fn tab_file_name(self: &DocumentManager, tab_id: u64) -> QString;

        /// Human-readable language name for the tab's file (L3's
        /// status bar), e.g. "Rust", "JSON", "Plain Text".
        #[qinvokable]
        #[cxx_name = "tabLanguageName"]
        fn tab_language_name(self: &DocumentManager, tab_id: u64) -> QString;

        /// Class View's per-file tier (Task D): the tab's symbol outline
        /// (`syntax_core::outline()` on its current content, language-
        /// picked the same way `tabLanguageName` picks a display name),
        /// pre-order-flattened per `FfiSymbolNode`'s doc comment. Pull-
        /// based like `tabContent`/`tabFileName` rather than a push
        /// signal — the view calls this once on tab open and again after
        /// each successful save (not per keystroke; see the plan doc's
        /// Task D — a project-wide-scope panel doesn't need live updates).
        #[qinvokable]
        #[cxx_name = "tabOutline"]
        fn tab_outline(self: &DocumentManager, tab_id: u64) -> Vec<FfiSymbolNode>;

        /// The tab's display title (file name, plus the "(deleted)" suffix
        /// once its backing file is gone). The tab strip renders this
        /// verbatim, adding only its own dirty marker.
        #[qinvokable]
        #[cxx_name = "tabTitle"]
        fn tab_title(self: &DocumentManager, tab_id: u64) -> QString;

        /// The tab's backing file path, empty for an unknown id — the view
        /// records it in the persisted editor split layout so the same files
        /// reopen into the same groups next launch.
        #[qinvokable]
        #[cxx_name = "tabPath"]
        fn tab_path(self: &DocumentManager, tab_id: u64) -> QString;

        /// Which kind of page the tab needs: `app_core::TabKind`'s code —
        /// 0 text, 1 binary (ADR-0020). The view builds a `CodeEditor` or a
        /// `HexViewer` from this; it never decides the kind itself from the
        /// path or the bytes. Unknown ids answer 0, the same "treat it as
        /// ordinary" default the widget-construction path already takes.
        #[qinvokable]
        #[cxx_name = "tabKind"]
        fn tab_kind(self: &DocumentManager, tab_id: u64) -> i32;

        /// How many hex rows a binary tab spans — the viewer's vertical
        /// scroll range. 0 for a text tab or an unknown id.
        #[qinvokable]
        #[cxx_name = "binaryRowCount"]
        fn binary_row_count(self: &DocumentManager, tab_id: u64) -> u64;

        /// Size in bytes of a binary tab's file, for the status bar. 0 for a
        /// text tab or an unknown id.
        #[qinvokable]
        #[cxx_name = "binaryLength"]
        fn binary_length(self: &DocumentManager, tab_id: u64) -> u64;

        /// `count` hex rows starting at `first_row`, clamped to the end of
        /// the file. Pull-based per repaint, like `tabContent` — only the
        /// rows currently on screen are ever read from disk, which is what
        /// keeps a multi-gigabyte binary cheap to scroll.
        #[qinvokable]
        #[cxx_name = "hexRows"]
        fn hex_rows(
            self: &DocumentManager,
            tab_id: u64,
            first_row: u64,
            count: u64,
        ) -> Vec<FfiHexRow>;

        /// The authoritative dirty flag for `tab_id` (ADR-0003: the view
        /// reads this rather than trusting its own copy).
        #[qinvokable]
        #[cxx_name = "tabIsModified"]
        fn tab_is_modified(self: &DocumentManager, tab_id: u64) -> bool;

        /// Open a read-only `TabKind::Diff` tab comparing two already-read
        /// texts (F3-14) — used by File History's "compare revisions" and
        /// the Project Tree's "Compare with…", neither of which has a live
        /// `Document` on either side. Returns the new tab's id and emits
        /// `tabOpened` like `openFile` does.
        #[qinvokable]
        #[cxx_name = "openDiffTab"]
        fn open_diff_tab(
            self: Pin<&mut DocumentManager>,
            path: &QString,
            left_label: &QString,
            right_label: &QString,
            left_text: &QString,
            right_text: &QString,
        ) -> u64;

        /// The left/right side labels a diff tab was opened with (e.g. two
        /// revision short-ids, or two file names). Empty for any other tab.
        #[qinvokable]
        #[cxx_name = "diffLeftLabel"]
        fn diff_left_label(self: &DocumentManager, tab_id: u64) -> QString;
        #[qinvokable]
        #[cxx_name = "diffRightLabel"]
        fn diff_right_label(self: &DocumentManager, tab_id: u64) -> QString;

        /// The two texts a diff tab is comparing, for `DiffView`'s two
        /// panes. Empty for any other tab.
        #[qinvokable]
        #[cxx_name = "diffLeftText"]
        fn diff_left_text(self: &DocumentManager, tab_id: u64) -> QString;
        #[qinvokable]
        #[cxx_name = "diffRightText"]
        fn diff_right_text(self: &DocumentManager, tab_id: u64) -> QString;

        /// The line hunks between a diff tab's two texts, computed once when
        /// it opened (F3-14). Empty for any other tab.
        #[qinvokable]
        #[cxx_name = "diffHunks"]
        fn diff_hunks(self: &DocumentManager, tab_id: u64) -> Vec<FfiHunk>;

        /// Intra-line spans for a diff tab's hunks, `DiffView`'s
        /// `ExtraSelection`s (mirrors `pendingFileSpans`). Empty for any
        /// other tab.
        #[qinvokable]
        #[cxx_name = "diffSpans"]
        fn diff_spans(self: &DocumentManager, tab_id: u64) -> Vec<FfiInlineSpan>;

        /// Diff two arbitrary texts directly — no tab, no `AppSession`
        /// state — for `DiffViewPage`'s "ignore whitespace" toggle, which
        /// needs a second hunk set for the same two texts a diff is already
        /// open on.
        #[qinvokable]
        #[cxx_name = "diffHunksBetween"]
        fn diff_hunks_between(
            self: &DocumentManager,
            left_text: &QString,
            right_text: &QString,
            ignore_whitespace: bool,
        ) -> Vec<FfiHunk>;
        #[qinvokable]
        #[cxx_name = "diffSpansBetween"]
        fn diff_spans_between(
            self: &DocumentManager,
            left_text: &QString,
            right_text: &QString,
            ignore_whitespace: bool,
        ) -> Vec<FfiInlineSpan>;

        /// Handle a filesystem-watcher event for `path` (relayed via
        /// `ProjectTreeModel::filesChangedExternally`, already running on
        /// the Qt thread by the time this is called — plain signal/slot,
        /// no further cross-thread hop needed). The session's watcher
        /// policy decides whether this is a genuine external change to an
        /// open tab; if so `externalChangeDetected(tabId, path)` is emitted.
        #[qinvokable]
        #[cxx_name = "checkExternalChange"]
        fn check_external_change(self: Pin<&mut DocumentManager>, path: &QString);

        /// Re-read the tab's backing file from disk, discarding any
        /// in-editor edits (the "Reload" choice on the external-change
        /// prompt, US-3).
        #[qinvokable]
        #[cxx_name = "reloadTabFromDisk"]
        fn reload_tab_from_disk(self: Pin<&mut DocumentManager>, tab_id: u64) -> FfiResult;

        /// Forward the view's own cursor position for `tab_id` (M4) — the
        /// same "Rust remembers, view forwards" split `setTabModified`
        /// already uses for dirty state (ADR-0003).
        #[qinvokable]
        #[cxx_name = "setCursorPosition"]
        fn set_cursor_position(
            self: Pin<&mut DocumentManager>,
            tab_id: u64,
            line: u32,
            column: u32,
        );

        /// Record where the caret is *before* a jump, so Back can return
        /// here (N5). Called from the shared tail every jump in the app
        /// funnels through, which is what gives Find in Files, Go to
        /// Symbol, Class View and Go to Line their history for free.
        #[qinvokable]
        #[cxx_name = "recordJump"]
        fn record_jump(self: Pin<&mut DocumentManager>, path: &QString, line: u32, column: u32);

        /// Step back in the jump history. `found == false` means there is
        /// nowhere further back to go.
        #[qinvokable]
        #[cxx_name = "jumpBack"]
        fn jump_back(self: Pin<&mut DocumentManager>) -> FfiLocation;

        /// Step forward in the jump history. `found == false` means there
        /// is nowhere further forward to go.
        #[qinvokable]
        #[cxx_name = "jumpForward"]
        fn jump_forward(self: Pin<&mut DocumentManager>) -> FfiLocation;

        /// Whether Back/Forward have anywhere to go — the view enables or
        /// disables its menu actions from these rather than tracking a
        /// stack of its own.
        #[qinvokable]
        #[cxx_name = "canJumpBack"]
        fn can_jump_back(self: &DocumentManager) -> bool;

        #[qinvokable]
        #[cxx_name = "canJumpForward"]
        fn can_jump_forward(self: &DocumentManager) -> bool;

        /// Brings the MCP server in line with the saved settings: stops a
        /// running one, then starts a fresh one on the configured port if
        /// MCP is enabled. Idempotent — the view calls it once at startup
        /// and again whenever the Settings dialog commits, and never has to
        /// track what is currently running.
        ///
        /// The server lives on a dedicated background thread with its own
        /// Tokio runtime (`run_app()`'s Qt event loop isn't async); its
        /// `EditorCommand` listener loop marshals each command back onto
        /// this QObject's `CxxQtThread` (M3). The outcome arrives as
        /// `mcpStarted`/`mcpStopped`/`mcpFailed` rather than a return value,
        /// because binding happens on that other thread.
        #[qinvokable]
        #[cxx_name = "applyMcpSettings"]
        fn apply_mcp_settings(self: Pin<&mut DocumentManager>);

        /// Stops the MCP server and removes its discovery file. The view
        /// calls this as the window closes so a stale discovery file never
        /// points a client at a dead port.
        #[qinvokable]
        #[cxx_name = "shutdownMcpServer"]
        fn shutdown_mcp_server(self: &DocumentManager);

        /// Emitted once the MCP server is listening, with the port it
        /// actually bound (which is the OS's choice when the configured
        /// port is 0).
        #[qsignal]
        #[cxx_name = "mcpStarted"]
        fn mcp_started(self: Pin<&mut DocumentManager>, port: u16);

        /// Emitted when MCP is turned off in settings and the running
        /// server has been shut down.
        #[qsignal]
        #[cxx_name = "mcpStopped"]
        fn mcp_stopped(self: Pin<&mut DocumentManager>);

        /// Emitted when the server could not start — almost always a
        /// configured port that is already in use. Carries the message to
        /// show; the IDE itself keeps running without MCP.
        #[qsignal]
        #[cxx_name = "mcpFailed"]
        fn mcp_failed(self: Pin<&mut DocumentManager>, message: QString);
    }

    // Enables `self.qt_thread()` on `DocumentManager` — the MCP listener
    // thread's one cross-thread hop (M3), same `CxxQtThread::queue()`
    // pattern `ProjectTreeModel`'s watcher relay above already established.
    impl cxx_qt::Threading for DocumentManager {}

    extern "RustQt" {
        /// Icons for a path (ADR-0027), for any view that has one — the
        /// project tree through `IconDecorationProxy`, the tab strip and
        /// the result lists directly.
        ///
        /// It also owns the two live-preview switches the Appearance page
        /// needs — the icon theme and the colour theme's appearance —
        /// because both change what a key resolves to and this is the
        /// object every view already asks.
        ///
        /// Split in two on purpose: `iconKeyForPath` is cheap enough to run
        /// per visible row on every repaint, `iconPixels` rasterises. The
        /// key is what the view memoises its `QIcon`s by, so the expensive
        /// half runs once per distinct icon and size.
        #[qobject]
        type IconProvider = super::IconProviderRust;

        /// The icon key for a row, or an empty string when no icon theme is
        /// active — which is what tells the view to draw no decoration at
        /// all rather than a blank one.
        #[qinvokable]
        #[cxx_name = "iconKeyForPath"]
        fn icon_key_for_path(
            self: &IconProvider,
            path: &QString,
            is_dir: bool,
            expanded: bool,
        ) -> QString;

        /// `px` by `px` premultiplied RGBA8 for a key, `px * px * 4` bytes,
        /// or empty when there is nothing to draw. Wrap it in a
        /// `QImage::Format_RGBA8888_Premultiplied` — see `icon_cache.cpp`
        /// for why no other format will do.
        #[qinvokable]
        #[cxx_name = "iconPixels"]
        fn icon_pixels(self: &IconProvider, key: &QString, px: u32) -> QByteArray;

        /// Every icon theme the loaded plugins offer — the Appearance
        /// page's combo, in registry order.
        #[qinvokable]
        #[cxx_name = "iconThemes"]
        fn icon_themes(self: &IconProvider) -> Vec<FfiIconTheme>;

        /// Draw with this icon theme from now on, without persisting the
        /// choice: the Appearance page's live preview, and the Cancel path
        /// that puts the previous one back. An id nothing offers falls back
        /// to the first theme there is, so a preview can never leave the
        /// tree bare.
        #[qinvokable]
        #[cxx_name = "applyIconTheme"]
        fn apply_icon_theme(self: &IconProvider, id: &QString);

        /// Tell the icons which colour theme is in force, so a pack's light
        /// variants swap in with it. Pass the theme name that was applied;
        /// what it means for the art is decided in `app-core`.
        #[qinvokable]
        #[cxx_name = "applyColorTheme"]
        fn apply_color_theme(self: &IconProvider, theme_name: &QString);
    }

    /// One rasterised diagram inside a rendered preview — premultiplied
    /// RGBA8, `IconProvider::iconPixels`'s own byte order, so the view's
    /// `QImage::Format_RGBA8888_Premultiplied` decode is one function
    /// shared between the two rather than written twice.
    struct FfiPreviewImage {
        key: QString,
        width: u32,
        height: u32,
        pixels: QByteArray,
    }

    /// What a link in the preview turned out to be, for
    /// `PreviewProvider::previewLinkTarget` — never acted on by the view
    /// itself beyond the one case `kind` names.
    enum FfiPreviewLinkKind {
        /// Scroll to an anchor already in the current document.
        Anchor,
        /// Open `path` as a tab, at `line` when it is not negative.
        OpenFile,
        /// Never opened. `message` is shown in the status bar.
        Refused,
    }

    /// See [`FfiPreviewLinkKind`]. `path`/`line` are meaningful only for
    /// `OpenFile`; `message` only for `Anchor` (the anchor name) and
    /// `Refused` (why).
    struct FfiPreviewLinkTarget {
        kind: FfiPreviewLinkKind,
        path: QString,
        line: i32,
        message: QString,
    }

    extern "RustQt" {
        /// Renders Markdown (and inline Mermaid diagrams) for the Preview
        /// dock (ADR-0033). Pull-based like `DocumentManager`: `requestPreview`
        /// schedules a render on a worker thread and returns immediately;
        /// `previewReady` announces a finished revision; the view then pulls
        /// `previewHtml`/`previewImages` for that tab.
        ///
        /// A request carries a revision the caller never sees — an older
        /// result racing a newer request is dropped rather than shown, so a
        /// document edited faster than it renders never flickers backwards.
        #[qobject]
        type PreviewProvider = super::PreviewProviderRust;

        /// Does any loaded plugin preview `path`'s extension? Drives the
        /// dock's enabled/empty state without rendering anything.
        #[qinvokable]
        #[cxx_name = "hasPreview"]
        fn has_preview(self: &PreviewProvider, path: &QString) -> bool;

        /// Schedule a render of `source` (the tab's current buffer text,
        /// already read by the caller — this object never touches a
        /// document itself) at `width_px`, the dock's content width in
        /// device pixels. Returns immediately; the result arrives via
        /// `previewReady`.
        #[qinvokable]
        #[cxx_name = "requestPreview"]
        fn request_preview(
            self: Pin<&mut PreviewProvider>,
            tab_id: u64,
            path: &QString,
            source: &QString,
            width_px: u32,
        );

        /// The finished HTML for `tabId`'s latest ready revision, or empty
        /// when nothing has rendered yet.
        #[qinvokable]
        #[cxx_name = "previewHtml"]
        fn preview_html(self: &PreviewProvider, tab_id: u64) -> QString;

        /// Every diagram the latest ready revision needs painted, keyed the
        /// way `previewHtml`'s `<img src="ide-preview:{key}">` tags name
        /// them.
        #[qinvokable]
        #[cxx_name = "previewImages"]
        fn preview_images(self: &PreviewProvider, tab_id: u64) -> Vec<FfiPreviewImage>;

        /// Classify one `href` clicked in the preview — see
        /// [`FfiPreviewLinkTarget`]. `doc_path` is the previewed file's own
        /// path, so a relative link resolves against its directory.
        #[qinvokable]
        #[cxx_name = "previewLinkTarget"]
        fn preview_link_target(
            self: &PreviewProvider,
            doc_path: &QString,
            href: &QString,
        ) -> FfiPreviewLinkTarget;

        /// Emitted on the Qt thread once `tabId`'s render for `revision`
        /// finished — success or failure both arrive this way, since a
        /// failed render still has something to show (M4's fallback
        /// block), not nothing. `main_window.cpp` connects this to the
        /// Preview dock.
        #[qsignal]
        #[cxx_name = "previewReady"]
        fn preview_ready(self: Pin<&mut PreviewProvider>, tab_id: u64, revision: u64);
    }

    impl cxx_qt::Threading for PreviewProvider {}

    extern "RustQt" {
        /// Settings-I/O adapter (L1 window geometry/state, C2 recent
        /// projects) — wraps `app_config::{load,save}` the same way
        /// `DocumentManager` wraps `AppSession`. Owns no settings state
        /// itself; every call re-reads or re-writes `settings.toml`.
        #[qobject]
        type AppSettings = super::AppSettingsRust;

        /// Most-recently-opened projects, newest first (C2).
        #[qinvokable]
        #[cxx_name = "recentProjects"]
        fn recent_projects(self: &AppSettings) -> QStringList;

        /// Last-persisted main window geometry, or all-zero if none was
        /// ever saved (L1).
        #[qinvokable]
        #[cxx_name = "windowGeometry"]
        fn window_geometry(self: &AppSettings) -> FfiWindowGeometry;

        /// Persist the main window's geometry (L1's `closeEvent`).
        #[qinvokable]
        #[cxx_name = "saveWindowGeometry"]
        fn save_window_geometry(self: &AppSettings, x: i32, y: i32, width: u32, height: u32);

        /// Opaque persisted dock layout blob (D4), base64-encoded by the
        /// view — `ads::CDockManager::saveState()`/`restoreState()` deal in
        /// `QByteArray`, not text, and `Settings::window_state` is a plain
        /// Rust `String` (must be valid UTF-8). Empty when nothing was ever
        /// saved.
        #[qinvokable]
        #[cxx_name = "windowState"]
        fn window_state(self: &AppSettings) -> QString;

        /// Persist the dock layout blob (D4's `closeEvent`).
        #[qinvokable]
        #[cxx_name = "saveWindowState"]
        fn save_window_state(self: &AppSettings, state: &QString);

        /// Opaque persisted editor split layout: the tab-group splitter tree
        /// plus the files open in each group, serialized as JSON by the view
        /// (the split layout is view state — nothing in `app-core` models
        /// editor groups). Empty when nothing was ever saved.
        #[qinvokable]
        #[cxx_name = "editorLayout"]
        fn editor_layout(self: &AppSettings) -> QString;

        /// Persist the editor split layout, alongside the dock layout on
        /// window close.
        #[qinvokable]
        #[cxx_name = "saveEditorLayout"]
        fn save_editor_layout(self: &AppSettings, layout: &QString);

        /// Active theme name (T2), e.g. "dark" or "light" — defaults to
        /// "dark" when unset (`Settings::theme_name`). The view maps this to
        /// a stylesheet via `styleSheetForTheme`.
        #[qinvokable]
        #[cxx_name = "themeName"]
        fn theme_name(self: &AppSettings) -> QString;

        /// Persist the chosen theme name (S1's Appearance page, on OK).
        #[qinvokable]
        #[cxx_name = "saveTheme"]
        fn save_theme(self: &AppSettings, theme: &QString);

        /// The persisted icon theme id, or an empty string when the user
        /// has never chosen one — which is not the same as "no icons": the
        /// first theme the plugins offer is used until they do.
        #[qinvokable]
        #[cxx_name = "iconThemeId"]
        fn icon_theme_id(self: &AppSettings) -> QString;

        /// Persist the chosen icon theme id (P7's Appearance page, on OK).
        #[qinvokable]
        #[cxx_name = "saveIconTheme"]
        fn save_icon_theme(self: &AppSettings, id: &QString);

        /// Editor font, always resolved to a usable value (S2).
        #[qinvokable]
        #[cxx_name = "editorFont"]
        fn editor_font(self: &AppSettings) -> FfiEditorFont;

        /// Persist the editor font (S2's Editor page, on OK).
        #[qinvokable]
        #[cxx_name = "saveEditorFont"]
        fn save_editor_font(self: &AppSettings, family: &QString, size: u32);

        /// Interface font scales, always resolved and clamped.
        #[qinvokable]
        #[cxx_name = "uiFontScales"]
        fn ui_font_scales(self: &AppSettings) -> FfiUiFontScales;

        /// Persist the interface font scales (the Appearance page, on OK).
        #[qinvokable]
        #[cxx_name = "saveUiFontScales"]
        fn save_ui_font_scales(self: &AppSettings, ui: u32, project_tree: u32, menu: u32);

        /// Editor text colors, empty when unset (S2).
        #[qinvokable]
        #[cxx_name = "editorColors"]
        fn editor_colors(self: &AppSettings) -> FfiEditorColors;

        /// Persist the editor colors (S2's Editor page, on OK).
        #[qinvokable]
        #[cxx_name = "saveEditorColors"]
        fn save_editor_colors(
            self: &AppSettings,
            background: &QString,
            foreground: &QString,
            current_line: &QString,
        );

        /// JetBrains-style "show whitespace characters", off by default.
        #[qinvokable]
        #[cxx_name = "whitespaceOptions"]
        fn whitespace_options(self: &AppSettings) -> FfiWhitespaceOptions;

        /// Persist the whitespace display options (the Editor page, live
        /// preview + on OK).
        #[qinvokable]
        #[cxx_name = "saveWhitespaceOptions"]
        fn save_whitespace_options(self: &AppSettings, options: &FfiWhitespaceOptions);

        /// Where the running server publishes its port and auth token, so
        /// the Settings page can tell the user what to point an agent at.
        #[qinvokable]
        #[cxx_name = "mcpDiscoveryFilePath"]
        fn mcp_discovery_file_path(self: &AppSettings) -> QString;

        /// Whether the MCP server should run, defaulting to on for a
        /// settings file that predates the switch.
        #[qinvokable]
        #[cxx_name = "mcpEnabled"]
        fn mcp_enabled(self: &AppSettings) -> bool;

        /// The configured MCP port; `0` means "let the OS choose", which is
        /// what keeps two IDE instances from colliding (ADR-0004).
        #[qinvokable]
        #[cxx_name = "mcpPort"]
        fn mcp_port(self: &AppSettings) -> u16;

        /// Persist both MCP settings together (the Settings dialog's MCP
        /// page, on OK) — one load-modify-save instead of two, so a port
        /// change and an enable change cannot half-apply.
        #[qinvokable]
        #[cxx_name = "saveMcpSettings"]
        fn save_mcp_settings(self: &AppSettings, enabled: bool, port: u16);

        /// The `[terminal]` section of the layer the dialog is editing
        /// (`settingsScope()`), for the Settings > Terminal page.
        #[qinvokable]
        #[cxx_name = "terminalSettings"]
        fn terminal_settings(self: &AppSettings) -> FfiTerminalSettings;

        /// Persist the whole `[terminal]` section together, on OK — one
        /// load-modify-save, so a shell change and a start-directory change
        /// cannot half-apply. Writes to the global file or the project's,
        /// whichever `settingsScope()` names, and reports a typed error
        /// (ADR-0003) rather than failing silently.
        #[qinvokable]
        #[cxx_name = "saveTerminalSettings"]
        fn save_terminal_settings(self: &AppSettings, terminal: &FfiTerminalSettings) -> FfiResult;

        /// Every shell this machine offers, for the Terminal page's combo —
        /// the same list, from the same place, as the terminal dock's "+"
        /// dropdown.
        #[qinvokable]
        #[cxx_name = "availableShells"]
        fn available_shells(self: &AppSettings) -> Vec<FfiShellCandidate>;

        /// The shortcut `action_id` currently responds to, as `QKeySequence`
        /// portable text — the user's override if there is one, otherwise the
        /// default from `app_config::ACTIONS`. Empty means unbound. Menu
        /// construction asks this per action instead of hardcoding a
        /// `QKeySequence`, so the fallback rule stays in Rust.
        #[qinvokable]
        #[cxx_name = "shortcutFor"]
        fn shortcut_for(self: &AppSettings, action_id: &QString) -> QString;

        /// Re-scan `<config_dir>/languages` and swap in the rebuilt
        /// language registry (G2), returning one line per language that
        /// failed to load — empty when everything loaded. Editors already
        /// open keep the grammar they were built with; files opened after
        /// this call see the new registry.
        #[qinvokable]
        #[cxx_name = "reloadLanguages"]
        fn reload_languages(self: &AppSettings) -> QStringList;

        /// Which settings layer the dialog is editing: `"global"` or
        /// `"project"` (F0-10, ADR-0022).
        #[qinvokable]
        #[cxx_name = "settingsScope"]
        fn settings_scope(self: &AppSettings) -> QString;

        /// Switch the layer the dialog edits, emitting `settingsScopeChanged`
        /// so every open page reloads its draft from the new layer. An
        /// unrecognised name selects the global layer, which is the answer
        /// that cannot write into a file the whole project shares.
        #[qinvokable]
        #[cxx_name = "setSettingsScope"]
        fn set_settings_scope(self: Pin<&mut AppSettings>, scope: &QString);

        /// Whether the open project overrides anything at all — what the
        /// scope selector needs to say so, rather than showing an empty
        /// Project tab that looks broken.
        #[qinvokable]
        #[cxx_name = "hasProjectSettings"]
        fn has_project_settings(self: &AppSettings) -> bool;

        /// Whether a project is open at all. Distinct from
        /// `hasProjectSettings`: a freshly opened project has no `.ide`
        /// file yet and can still be given one, while with no project open
        /// there is nowhere for project settings to live and the scope
        /// selector says so instead of offering a choice that cannot be
        /// saved.
        #[qinvokable]
        #[cxx_name = "isProjectOpen"]
        fn is_project_open(self: &AppSettings) -> bool;

        /// Where one scoped field's effective value comes from, as the word
        /// the badge shows: "from project", "from global" or "default".
        /// `field_id` is a `settings_model::ScopedField` id — `"editing"`,
        /// `"languageServers"`, `"runConfigs"`, `"indexExcludes"`.
        ///
        /// The view displays this and never re-derives it (ADR-0022): a
        /// badge computed apart from the value it labels eventually lies.
        #[qinvokable]
        #[cxx_name = "fieldOrigin"]
        fn field_origin(self: &AppSettings, field_id: &QString) -> QString;
    }

    unsafe extern "RustQt" {
        /// The scope selector changed which layer is being edited. Every
        /// open settings page reloads its draft from the layer now selected;
        /// a page that ignored this would show one layer's values and save
        /// them into the other.
        #[qsignal]
        #[cxx_name = "settingsScopeChanged"]
        fn settings_scope_changed(self: Pin<&mut AppSettings>);
    }

    extern "RustQt" {
        /// Keymap settings page adapter: holds the *draft* keymap the dialog
        /// edits, so Cancel discards it by simply never calling `commit`.
        /// The draft is dialog session state, not domain state — every rule
        /// it exercises (default fallback, conflict detection, stealing) is
        /// an `app_config::Keymap` call.
        #[qobject]
        type KeymapEditor = super::KeymapEditorRust;

        /// Load the persisted overrides into the draft. Called each time the
        /// settings dialog opens, so a Cancel-ed edit never leaks into the
        /// next one.
        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &KeymapEditor);

        /// Every action with its effective shortcut, in menu order.
        #[qinvokable]
        #[cxx_name = "bindings"]
        fn bindings(self: &KeymapEditor) -> Vec<FfiKeyBinding>;

        /// Labels of the actions that would lose their binding if `shortcut`
        /// were assigned to `action_id` — what the view puts in its
        /// confirmation prompt. Empty when there is nothing to steal.
        #[qinvokable]
        #[cxx_name = "conflicts"]
        fn conflicts(self: &KeymapEditor, action_id: &QString, shortcut: &QString) -> QStringList;

        /// Bind `shortcut` to `action_id` in the draft, unbinding whoever
        /// held it before (the view is expected to have confirmed via
        /// `conflicts` first). An empty `shortcut` just unbinds `action_id`.
        #[qinvokable]
        #[cxx_name = "assign"]
        fn assign(self: &KeymapEditor, action_id: &QString, shortcut: &QString);

        /// Drop every override in the draft, back to the shipped defaults.
        #[qinvokable]
        #[cxx_name = "resetDefaults"]
        fn reset_defaults(self: &KeymapEditor);

        /// Persist the draft into `Settings::keymap` (the dialog's OK path).
        #[qinvokable]
        #[cxx_name = "commit"]
        fn commit(self: &KeymapEditor);
    }

    extern "RustQt" {
        /// Find-in-Files adapter (Task H): owns an `index_core::TextIndex`
        /// for the currently open project and translates the query box's
        /// intent into it. Like `DocumentManager`/`ProjectTreeModel`, it
        /// decides nothing itself — building the index and running a
        /// search both happen on a background `std::thread` (index
        /// building and search are both I/O-bound; neither may block the
        /// Qt thread), with every result marshaled back via
        /// `CxxQtThread::queue()`, the exact pattern `apply_mcp_settings`
        /// already established.
        #[qobject]
        type SearchModel = super::SearchModelRust;

        /// Open the project index for `root_path`, reusing what is already
        /// on disk and re-reading only the files that changed since the last
        /// run (a full build only happens on a first run or an unusable
        /// index). Wired to `ProjectTreeModel::projectOpened` in
        /// `main_window.cpp` — the same project-open lifecycle event the
        /// tree/watcher already hook, not a second parallel one.
        #[qinvokable]
        #[cxx_name = "openIndex"]
        fn open_index(self: Pin<&mut SearchModel>, root_path: &QString);

        /// Re-index one file after it changed on disk, so search results
        /// never go stale while the project stays open. Driven by the
        /// existing filesystem watcher; a path that is gone or unreadable
        /// simply drops out of the index.
        #[qinvokable]
        #[cxx_name = "reindexFile"]
        fn reindex_file(self: Pin<&mut SearchModel>, path: &QString);

        /// Drop a deleted file from the index (the watcher's remove/rename
        /// counterpart to `reindexFile`).
        #[qinvokable]
        #[cxx_name = "removeIndexedFile"]
        fn remove_indexed_file(self: Pin<&mut SearchModel>, path: &QString);

        /// Bring a whole batch of changed paths up to date at once — the
        /// watcher's coalesced window, handed over as one call.
        ///
        /// Whether a path is re-indexed or dropped is decided in Rust from
        /// whether it still exists, not by the caller: that is a rule about
        /// what the index holds, and the view has no business splitting the
        /// batch. One commit and one write lock for the whole batch, rather
        /// than one of each per file.
        #[qinvokable]
        #[cxx_name = "syncIndexedFiles"]
        fn sync_indexed_files(self: Pin<&mut SearchModel>, paths: &QStringList);

        /// Record `path` as most-recently-opened: it feeds Search
        /// Everywhere's Recent tier and is persisted to `settings.toml`.
        #[qinvokable]
        #[cxx_name = "noteRecentFile"]
        fn note_recent_file(self: Pin<&mut SearchModel>, path: &QString);

        /// Re-read the keymap so the action tier reports current shortcuts.
        /// Called at startup and after the Settings keymap page commits.
        #[qinvokable]
        #[cxx_name = "refreshKeymap"]
        fn refresh_keymap(self: Pin<&mut SearchModel>);

        /// Search Everywhere: run `query` across every tier (recent files,
        /// actions, file names, symbols, then full text) and stream the hits
        /// back as `resultsBatch` emissions tagged with `generation`,
        /// followed by exactly one `queryFinished`/`queryFailed` for that
        /// same generation.
        ///
        /// `generation` is the view's monotonically increasing query id. A
        /// newer call cancels the running one mid-scan, and the view drops
        /// any batch whose generation is not the one it is waiting for —
        /// which is what keeps search-as-you-type from either stalling or
        /// interleaving stale results.
        #[qinvokable]
        #[cxx_name = "searchEverywhere"]
        fn search_everywhere(
            self: Pin<&mut SearchModel>,
            query: &QString,
            tiers: FfiTierFilter,
            generation: u64,
            limit: u32,
        );

        /// A batch of Search Everywhere hits for `generation`, in rank
        /// order within a tier and tier order across batches. Batched
        /// rather than one signal per hit because a signal per hit means a
        /// cross-thread hop per hit.
        #[qsignal]
        #[cxx_name = "resultsBatch"]
        fn results_batch(self: Pin<&mut SearchModel>, generation: u64, hits: Vec<FfiSearchHit>);

        /// Emitted once after the last `resultsBatch` of a
        /// `searchEverywhere` call, including when it found nothing or was
        /// superseded before finishing.
        #[qsignal]
        #[cxx_name = "queryFinished"]
        fn query_finished(self: Pin<&mut SearchModel>, generation: u64);

        /// Emitted instead of `queryFinished` when the query couldn't run
        /// at all (no project open yet).
        #[qsignal]
        #[cxx_name = "queryFailed"]
        fn query_failed(self: Pin<&mut SearchModel>, generation: u64, message: QString);

        /// Emitted once a `buildIndex` call finishes indexing successfully.
        #[qsignal]
        #[cxx_name = "indexReady"]
        fn index_ready(self: Pin<&mut SearchModel>);

        /// Emitted when a `buildIndex` call fails (ADR-0003: a typed signal
        /// per outcome, never a QString success/failure sentinel).
        #[qsignal]
        #[cxx_name = "indexFailed"]
        fn index_failed(self: Pin<&mut SearchModel>, message: QString);

        /// How far the running index build has got. Emitted once with
        /// `done == 0` as soon as the total is known, then at most every
        /// [`PROGRESS_INTERVAL`] until `done == total` — a hop per file
        /// would cost more than the indexing it reports on. Always followed
        /// by exactly one `indexReady` or `indexFailed`.
        #[qsignal]
        #[cxx_name = "indexProgress"]
        fn index_progress(self: Pin<&mut SearchModel>, done: u32, total: u32);

        /// Run Find-in-Files: `pattern` is a literal substring unless
        /// `is_regex` is set. Matches stream back as `searchBatch`
        /// emissions tagged with `generation`, followed by exactly one
        /// `searchFinished` or `searchFailed`. `generation` works exactly as
        /// it does for `searchEverywhere` — a newer search cancels the
        /// running one — but the two use separate counters so typing in the
        /// popup never cancels the results panel's search.
        #[qinvokable]
        #[cxx_name = "search"]
        fn search(
            self: Pin<&mut SearchModel>,
            pattern: &QString,
            is_regex: bool,
            case_sensitive: bool,
            generation: u64,
        );

        /// Apply a project-wide replace to exactly the spans in `edits` —
        /// the ones the user left checked in the results list, not "every
        /// match of the pattern". The replacement text per span is expanded
        /// here (so `$1` works), the write goes to disk, and the touched
        /// files are re-indexed; open tabs learn about it through the
        /// existing external-change flow.
        #[qinvokable]
        #[cxx_name = "replaceInFiles"]
        fn replace_in_files(
            self: Pin<&mut SearchModel>,
            edits: Vec<FfiFileReplacement>,
            pattern: &QString,
            replacement: &QString,
            is_regex: bool,
            case_sensitive: bool,
        );

        /// F3-15: build the diff preview `replaceInFiles` would produce for
        /// the same `edits`, without writing anything. Answers on
        /// `replacePreviewReady` (with the paths that got a preview) or
        /// `replacePreviewFailed`; each path's text and hunks are then read
        /// with `replacePreviewDiff`/`replacePreviewHunks`/
        /// `replacePreviewSpans`.
        #[qinvokable]
        #[cxx_name = "previewReplacements"]
        fn preview_replacements(
            self: Pin<&mut SearchModel>,
            edits: Vec<FfiFileReplacement>,
            pattern: &QString,
            replacement: &QString,
            is_regex: bool,
            case_sensitive: bool,
        );

        /// The before/after text of one file from the last
        /// `previewReplacements` answer. Empty when `path` was not in it.
        #[qinvokable]
        #[cxx_name = "replacePreviewDiff"]
        fn replace_preview_diff(self: &SearchModel, path: &QString) -> FfiFileDiff;

        /// The line hunks for the same file `replacePreviewDiff` describes.
        #[qinvokable]
        #[cxx_name = "replacePreviewHunks"]
        fn replace_preview_hunks(self: &SearchModel, path: &QString) -> Vec<FfiHunk>;

        /// The intra-line spans for the same file.
        #[qinvokable]
        #[cxx_name = "replacePreviewSpans"]
        fn replace_preview_spans(self: &SearchModel, path: &QString) -> Vec<FfiInlineSpan>;

        /// RF12 — the declaration of the symbol at `byte_offset`, rendered
        /// as a tooltip.
        ///
        /// The hover fallback: with no language server there is no stored
        /// signature anywhere, so the declaration's own source line (plus
        /// its continuations, capped) is shown — `index_core::
        /// declaration_signature`'s heuristic. Resolution is
        /// `resolve_declaration`, the same two tiers Go to Declaration uses,
        /// so hovering and Ctrl+Click agree about what a name means.
        ///
        /// Answers on `hoverSignatureReady`, and on nothing at all when the
        /// pointer has moved on or nothing resolved.
        #[qinvokable]
        #[cxx_name = "hoverSignature"]
        fn hover_signature(
            self: Pin<&mut SearchModel>,
            path: &QString,
            content: &QString,
            byte_offset: usize,
        );

        /// The pointer moved or left: an outstanding `hoverSignature` is no
        /// longer wanted. The LSP leg has its own tracker, so the view
        /// cancels both.
        #[qinvokable]
        #[cxx_name = "cancelHoverSignature"]
        fn cancel_hover_signature(self: Pin<&mut SearchModel>);

        /// Tooltip HTML for the most recent, still-current request.
        #[qsignal]
        #[cxx_name = "hoverSignatureReady"]
        fn hover_signature_ready(self: Pin<&mut SearchModel>, html: QString);

        /// RF9 — work out what renaming the symbol under the caret would
        /// change, with no language server involved.
        ///
        /// This is ADR-0011's name-based resolution, so it is deliberately
        /// cautious: it refuses when the caret resolved to nothing (that is
        /// Replace in Files, not a rename), when the new name is not an
        /// identifier, and when any buffer is unsaved, because the index
        /// reads from disk. `index_core::plan_index_rename` owns all three
        /// rules, including which sites start ticked.
        ///
        /// Answers on `indexRenameReady` or `indexRenameFailed`.
        #[qinvokable]
        #[cxx_name = "planIndexRename"]
        fn plan_index_rename(
            self: Pin<&mut SearchModel>,
            path: &QString,
            content: &QString,
            byte_offset: usize,
            new_name: &QString,
            has_unsaved_changes: bool,
        );

        /// A rename plan is ready; the view reads its sites back with
        /// `indexRenameSites`. `ambiguous` means more than one symbol in the
        /// project carries this name, which is what the preview has to say
        /// out loud.
        #[qsignal]
        #[cxx_name = "indexRenameReady"]
        fn index_rename_ready(self: Pin<&mut SearchModel>, name: QString, ambiguous: bool);

        /// The rename will not be offered. `reason` says which case it is,
        /// so the view can offer to save and retry rather than only
        /// reporting; `message` is the sentence to show.
        #[qsignal]
        #[cxx_name = "indexRenameFailed"]
        fn index_rename_failed(
            self: Pin<&mut SearchModel>,
            reason: FfiRenameRefusal,
            message: QString,
        );

        /// The sites of the pending name-based rename, in project order.
        #[qinvokable]
        #[cxx_name = "indexRenameSites"]
        fn index_rename_sites(self: &SearchModel) -> Vec<FfiRenameSite>;

        /// Leave `path` out of the pending name-based rename.
        #[qinvokable]
        #[cxx_name = "excludeFromIndexRename"]
        fn exclude_from_index_rename(self: Pin<&mut SearchModel>, path: &QString);

        /// Take the pending rename's sites in `path` as edits for that open
        /// editor to splice, removing them from the plan.
        ///
        /// A file the user has open must not be rewritten underneath them:
        /// that loses the undo history and makes the editor prompt about a
        /// change it made itself. So the view takes the open files first and
        /// `applyIndexRename` writes only what is left — the same split
        /// `lsp_core::plan_edit` makes for a server-driven edit.
        #[qinvokable]
        #[cxx_name = "takeIndexRenameBufferEdits"]
        fn take_index_rename_buffer_edits(
            self: Pin<&mut SearchModel>,
            path: &QString,
        ) -> Vec<FfiTextEdit>;

        /// Apply what is left of the pending name-based rename — every
        /// ticked site that was neither excluded nor taken for an open
        /// buffer — writing to disk and re-indexing. The same applier
        /// Replace in Files uses, because a rename site really is a
        /// single-line span of a known length.
        ///
        /// Answers on `refactorFilesFinished`/`refactorFilesFailed`.
        #[qinvokable]
        #[cxx_name = "applyIndexRename"]
        fn apply_index_rename(self: Pin<&mut SearchModel>);

        /// RF9 — apply refactoring edits to files no editor has open.
        ///
        /// Each file is read, the edits are applied to its whole text
        /// (`lsp_core::apply_to_text`, which validates every range before it
        /// produces anything), and the result is written and re-indexed.
        /// Only edits whose `in_buffer` is false belong here — the rest are
        /// spliced into their live buffers by the view, which is what keeps
        /// one Ctrl+Z undoing the whole refactoring in the files the user
        /// can see.
        ///
        /// Answers on `refactorFilesFinished` or `refactorFilesFailed`.
        #[qinvokable]
        #[cxx_name = "applyFileEdits"]
        fn apply_file_edits(self: Pin<&mut SearchModel>, edits: Vec<FfiTextEdit>);

        /// How many closed files a refactoring rewrote, and how many it left
        /// alone because they could not be read, could not be written, or no
        /// longer matched the edit.
        #[qsignal]
        #[cxx_name = "refactorFilesFinished"]
        fn refactor_files_finished(self: Pin<&mut SearchModel>, files: u32, skipped_files: u32);

        /// The write could not be attempted at all — no index, or it is
        /// still building. Nothing was changed.
        #[qsignal]
        #[cxx_name = "refactorFilesFailed"]
        fn refactor_files_failed(self: Pin<&mut SearchModel>, message: QString);

        /// Emitted once a `replaceInFiles` call finishes: how many files
        /// were rewritten, how many spans, and how many files were skipped
        /// because they changed since the search.
        #[qsignal]
        #[cxx_name = "replaceFinished"]
        fn replace_finished(
            self: Pin<&mut SearchModel>,
            files: u32,
            matches: u32,
            skipped_files: u32,
        );

        /// Emitted instead of `replaceFinished` when the replace could not
        /// run at all (no index built yet, or an invalid pattern).
        #[qsignal]
        #[cxx_name = "replaceFailed"]
        fn replace_failed(self: Pin<&mut SearchModel>, message: QString);

        /// A `previewReplacements` call finished: `paths` names every file
        /// that got a preview, in the order `SearchResultsPanel` should show
        /// them. A file the spans no longer fit (changed since the search)
        /// is left out, the same way `replaceFinished`'s `skipped_files`
        /// leaves it out of the write.
        #[qsignal]
        #[cxx_name = "replacePreviewReady"]
        fn replace_preview_ready(self: Pin<&mut SearchModel>, paths: QStringList);

        /// Emitted instead of `replacePreviewReady` when the preview could
        /// not run at all (no index built yet, or an invalid pattern).
        #[qsignal]
        #[cxx_name = "replacePreviewFailed"]
        fn replace_preview_failed(self: Pin<&mut SearchModel>, message: QString);

        /// A batch of Find-in-Files matches for `generation`, as
        /// `FfiHitKind::Text` hits: `line` is 1-based, `start`/`end` are
        /// byte offsets of the match within that line (matching
        /// `index_core::SearchMatch`), `text` is the trimmed line for
        /// display.
        #[qsignal]
        #[cxx_name = "searchBatch"]
        fn search_batch(self: Pin<&mut SearchModel>, generation: u64, hits: Vec<FfiSearchHit>);

        /// Emitted once after the last `searchBatch` of a `search` call
        /// (including when there were zero matches).
        #[qsignal]
        #[cxx_name = "searchFinished"]
        fn search_finished(self: Pin<&mut SearchModel>, generation: u64);

        /// Emitted instead of `searchFinished` when `search` couldn't run
        /// at all (no index built yet, or an invalid regex pattern).
        #[qsignal]
        #[cxx_name = "searchFailed"]
        fn search_failed(self: Pin<&mut SearchModel>, generation: u64, message: QString);

        /// Class View's project-wide tier (Task I): list every indexed
        /// symbol *definition* across the whole project — same
        /// `index_core::TextIndex` this QObject already owns for Find in
        /// Files (`find_definitions("")`, an empty substring query matches
        /// every name), not a second, redundant index build. Runs on a
        /// background thread and streams results like `search` does, for
        /// the same reason: querying goes through the same `Mutex` a
        /// concurrent `buildIndex`/`search` call might be holding.
        #[qinvokable]
        #[cxx_name = "projectSymbols"]
        fn project_symbols(self: Pin<&mut SearchModel>);

        /// One project-wide symbol definition. Carries the same
        /// `FfiSymbolMatch` row every other symbol signal does, so a jump
        /// from Class View lands on the identifier rather than at column
        /// 0 like it used to.
        #[qsignal]
        #[cxx_name = "projectSymbolFound"]
        fn project_symbol_found(self: Pin<&mut SearchModel>, row: FfiSymbolMatch);

        /// Emitted once after the last `projectSymbolFound` of a
        /// `projectSymbols` call (including when there were zero symbols).
        #[qsignal]
        #[cxx_name = "projectSymbolsFinished"]
        fn project_symbols_finished(self: Pin<&mut SearchModel>);

        /// Emitted instead of `projectSymbolsFinished` when `projectSymbols`
        /// couldn't run at all (no index built yet).
        #[qsignal]
        #[cxx_name = "projectSymbolsFailed"]
        fn project_symbols_failed(self: Pin<&mut SearchModel>, message: QString);

        /// Task J — find-usages: every occurrence (definitions and
        /// references alike) of the exact name `name`, across the whole
        /// project. `index_core::TextIndex::find_usages` already sorts by
        /// (path, line), so consecutive results share a file — the view
        /// groups by file simply by rendering them in the order they
        /// arrive, no server-side grouping needed.
        #[qinvokable]
        #[cxx_name = "findUsages"]
        fn find_usages(self: Pin<&mut SearchModel>, name: &QString);

        /// One usage — or, from `findImplementations`/`findSupertypes`,
        /// one hierarchy row. `is_definition` distinguishes the defining
        /// occurrence from a reference.
        #[qsignal]
        #[cxx_name = "usagesFound"]
        fn usages_found(self: Pin<&mut SearchModel>, row: FfiSymbolMatch);

        /// Emitted once after the last `usagesFound` of a `findUsages`
        /// call (including when there were zero usages).
        #[qsignal]
        #[cxx_name = "usagesFinished"]
        fn usages_finished(self: Pin<&mut SearchModel>);

        /// Emitted instead of `usagesFinished` when `findUsages` couldn't
        /// run at all (no index built yet).
        #[qsignal]
        #[cxx_name = "usagesFailed"]
        fn usages_failed(self: Pin<&mut SearchModel>, message: QString);

        /// N2 — Go to Declaration: where is the identifier at
        /// `byte_offset` in `content` declared? `path` and `content`
        /// describe the buffer the caret is in; passing the live text
        /// rather than reading the file means an unsaved edit resolves
        /// against what the user is actually looking at (the same shape
        /// `saveTab(id, content)` and the find invokables use).
        ///
        /// Results stream as `declarationFound`, best candidate first,
        /// then exactly one `declarationFinished` carrying which tier
        /// answered. Several candidates is a legitimate outcome, not an
        /// error: resolution is name-based (ADR-0008), so the view offers
        /// the choice rather than guessing.
        #[qinvokable]
        #[cxx_name = "resolveDeclaration"]
        fn resolve_declaration(
            self: Pin<&mut SearchModel>,
            path: &QString,
            content: &QString,
            byte_offset: usize,
        );

        /// One declaration candidate, best first.
        #[qsignal]
        #[cxx_name = "declarationFound"]
        fn declaration_found(self: Pin<&mut SearchModel>, row: FfiSymbolMatch);

        /// Emitted once after the last `declarationFound` of a
        /// `resolveDeclaration` call, including when there were none —
        /// `tier == None` with an empty `name` means the caret wasn't on
        /// an identifier at all.
        #[qsignal]
        #[cxx_name = "declarationFinished"]
        fn declaration_finished(
            self: Pin<&mut SearchModel>,
            tier: FfiResolutionTier,
            name: QString,
        );

        /// Emitted instead of `declarationFinished` when the lookup itself
        /// failed (an unreadable index). A missing index is *not* such a
        /// failure: the local tier resolves from the buffer alone, so a
        /// declaration in the file the caret is in still answers with no
        /// project open and while one is still being indexed.
        #[qsignal]
        #[cxx_name = "declarationFailed"]
        fn declaration_failed(self: Pin<&mut SearchModel>, message: QString);

        /// N3 — Go to Implementation: every type declaring `name` as a
        /// base class, implemented interface, or (in Rust) an implemented
        /// trait.
        ///
        /// Results arrive on the `usagesFound`/`usagesFinished`/
        /// `usagesFailed` trio rather than a trio of their own: a list of
        /// file:line locations is exactly what the Find Usages dock
        /// already renders, and a second identical signal set would buy
        /// nothing but a second set of connections to keep in sync.
        #[qinvokable]
        #[cxx_name = "findImplementations"]
        fn find_implementations(self: Pin<&mut SearchModel>, name: &QString);

        /// N3 — Go to Interface: every supertype `name` declares. Same
        /// signals as `findImplementations`.
        #[qinvokable]
        #[cxx_name = "findSupertypes"]
        fn find_supertypes(self: Pin<&mut SearchModel>, name: &QString);
    }

    // Enables `self.qt_thread()` on `SearchModel` for the background
    // index-build/search threads to marshal results back, same pattern as
    // `ProjectTreeModel`'s watcher relay and `DocumentManager`'s MCP
    // listener above.
    impl cxx_qt::Threading for SearchModel {}

    extern "RustQt" {
        /// Embedded terminal adapter (Task F4-14a): owns every open terminal
        /// session — each a `pty_core::PtySession` (a spawned shell) plus a
        /// `terminal_core::TerminalEmulator` (its VT100/grid state) — keyed
        /// by a `u64` session id the view carries per tab. Same
        /// "adapter owns nothing but a handle to Qt-free state" shape every
        /// other QObject in this file uses, generalized from Task F3's
        /// single-session `TerminalSession` the same way `RunService`
        /// already owns N run consoles behind one QObject
        /// (`bridge/run/mod.rs`).
        #[qobject]
        type TerminalSupervisor = super::TerminalSupervisorRust;

        /// Allocate a new session id. The shell is not spawned yet — call
        /// `start()` once the new tab's `TerminalWidget` knows its own pixel
        /// size, same lazy-start rule Task F3 established.
        #[qinvokable]
        #[cxx_name = "newSession"]
        fn new_session(self: Pin<&mut TerminalSupervisor>) -> u64;

        /// Kill `session_id`'s shell (and everything it started —
        /// `pty_core::PtySession::kill_tree`) and forget its state. Safe to
        /// call on an id that was never started or is already gone.
        #[qinvokable]
        #[cxx_name = "closeSession"]
        fn close_session(self: Pin<&mut TerminalSupervisor>, session_id: u64);

        /// Spawn `session_id`'s shell and size both the PTY and the grid to
        /// `rows`/`cols` — call once, when `cpp/terminal_widget.cpp` first
        /// knows its pixel size (its own font-metrics-derived cell count).
        /// A background `std::thread` starts doing blocking
        /// `PtySession::read` in a loop, feeding `TerminalEmulator::feed`
        /// and emitting `gridUpdated(session_id)` after each chunk via
        /// `CxxQtThread::queue()` — the exact pattern `apply_mcp_settings`
        /// already established. Spawn failure (e.g. no shell resolvable, or
        /// an unknown `session_id`) returns a typed non-zero `code`
        /// (ADR-0003); no `QString` sentinel.
        ///
        /// `shell_id` is a `FfiShellCandidate::id` when the tab was opened
        /// from the "+" dropdown, and empty for "whatever the settings say"
        /// — the precedence between the two lives in `bridge/terminal.rs`'s
        /// `shell_for`, not here and not in the view.
        #[qinvokable]
        #[cxx_name = "start"]
        fn start(
            self: Pin<&mut TerminalSupervisor>,
            session_id: u64,
            shell_id: &QString,
            rows: u32,
            cols: u32,
        ) -> FfiResult;

        /// Every shell this machine offers, most-preferred first, for the
        /// dock's "+" dropdown and the Terminal settings page's combo.
        ///
        /// The list is Rust's answer (`pty_core::shells::detect`) and the
        /// view only renders it: which shells exist, what they are called
        /// and in what order are decisions, and none of them belongs in
        /// `cpp/`.
        #[qinvokable]
        #[cxx_name = "availableShells"]
        fn available_shells(self: &TerminalSupervisor) -> Vec<FfiShellCandidate>;

        /// Forward keystrokes (already translated to the byte sequence a
        /// shell expects by the view) to `session_id`'s PTY stdin.
        #[qinvokable]
        #[cxx_name = "write"]
        fn write(self: Pin<&mut TerminalSupervisor>, session_id: u64, input: &QString);

        /// Resize both `session_id`'s PTY and grid — call from
        /// `cpp/terminal_widget.cpp`'s `resizeEvent` whenever the
        /// font-metrics-derived row/column count actually changes.
        #[qinvokable]
        #[cxx_name = "resize"]
        fn resize(self: Pin<&mut TerminalSupervisor>, session_id: u64, rows: u32, cols: u32);

        /// Pull-based grid read (Qt thread only — never touches the PTY):
        /// `cpp/terminal_widget.cpp`'s paint routine calls this in response
        /// to `gridUpdated`, same "signal says refresh, invokable getter
        /// hands over the data" shape `ClassViewPanel` already uses for
        /// `tabOutline`. Cells are `gridRows() * gridCols()` long, row-major
        /// — flattened because cxx has no `Vec<Vec<T>>` support; the view
        /// reshapes using `gridCols()`.
        #[qinvokable]
        #[cxx_name = "gridCells"]
        fn grid_cells(self: &TerminalSupervisor, session_id: u64) -> Vec<FfiTerminalCell>;

        /// Row count of the snapshot `gridCells()` would return right now.
        #[qinvokable]
        #[cxx_name = "gridRows"]
        fn grid_rows(self: &TerminalSupervisor, session_id: u64) -> u32;

        /// Column count of the snapshot `gridCells()` would return right now.
        #[qinvokable]
        #[cxx_name = "gridCols"]
        fn grid_cols(self: &TerminalSupervisor, session_id: u64) -> u32;

        /// Cursor's current row, zero-indexed from the top.
        #[qinvokable]
        #[cxx_name = "cursorRow"]
        fn cursor_row(self: &TerminalSupervisor, session_id: u64) -> u32;

        /// Cursor's current column, zero-indexed from the left.
        #[qinvokable]
        #[cxx_name = "cursorCol"]
        fn cursor_col(self: &TerminalSupervisor, session_id: u64) -> u32;

        /// Begin a mouse selection at a grid cell (Task F4). `right_half`
        /// is which half of the cell the click landed on, which decides
        /// whether that cell is included; out-of-range coordinates are
        /// clamped by `terminal-core`, not here.
        #[qinvokable]
        #[cxx_name = "selectionStart"]
        fn selection_start(
            self: &TerminalSupervisor,
            session_id: u64,
            row: u32,
            col: u32,
            right_half: bool,
            kind: FfiSelectionKind,
        );

        /// Extend the in-progress selection to a cell (drag).
        #[qinvokable]
        #[cxx_name = "selectionUpdate"]
        fn selection_update(
            self: &TerminalSupervisor,
            session_id: u64,
            row: u32,
            col: u32,
            right_half: bool,
        );

        #[qinvokable]
        #[cxx_name = "selectionClear"]
        fn selection_clear(self: &TerminalSupervisor, session_id: u64);

        /// Whether a selection covers at least one cell. The view gates
        /// its Copy action on this rather than on `selectionText()` being
        /// non-empty.
        #[qinvokable]
        #[cxx_name = "hasSelection"]
        fn has_selection(self: &TerminalSupervisor, session_id: u64) -> bool;

        /// The selected text, empty when there is no selection (guard with
        /// `hasSelection()`).
        #[qinvokable]
        #[cxx_name = "selectionText"]
        fn selection_text(self: &TerminalSupervisor, session_id: u64) -> QString;

        /// Paste clipboard text into `session_id`'s shell. The rules —
        /// control-character stripping, newline normalization, and
        /// bracketed-paste framing — live in `terminal-core`; the view only
        /// supplies the text.
        #[qinvokable]
        #[cxx_name = "paste"]
        fn paste(self: Pin<&mut TerminalSupervisor>, session_id: u64, text: &QString);

        /// The `http(s)` link covering a grid cell, for hover feedback and
        /// Ctrl+Click activation.
        #[qinvokable]
        #[cxx_name = "linkAt"]
        fn link_at(
            self: &TerminalSupervisor,
            session_id: u64,
            row: u32,
            col: u32,
        ) -> FfiTerminalLink;

        /// Emitted on the Qt thread (queued there from `session_id`'s
        /// background reader thread) after new PTY output has been fed into
        /// its emulator and is ready to paint. Every session's widget is
        /// connected to this one signal and filters on `session_id`.
        #[qsignal]
        #[cxx_name = "gridUpdated"]
        fn grid_updated(self: Pin<&mut TerminalSupervisor>, session_id: u64);
    }

    // Enables `self.qt_thread()` on `TerminalSupervisor` for the background
    // PTY reader threads to marshal `gridUpdated` back, same pattern as
    // `SearchModel`/`DocumentManager` above.
    impl cxx_qt::Threading for TerminalSupervisor {}

    extern "RustQt" {
        /// Editor ergonomics adapter (task F1-13): carets, transactions and
        /// the language-aware editing operations, for one editor widget.
        ///
        /// No threading. Caret arithmetic and line operations are
        /// microseconds on a rope, and a thread would add a frame of
        /// latency to every keystroke to save nothing.
        ///
        /// **Every slot that computes over the buffer takes the buffer
        /// text.** `editor_core::Document`'s rope is refreshed only on
        /// save, so it is one save behind what the user sees; the live text
        /// is the widget's, and it is passed in. This is the same stateless
        /// shape `findMatches` and `replacementEdits` already have.
        ///
        /// Positions in and out are flat document UTF-16 offsets; edits come
        /// back as `FfiTextEdit`s in the protocol's line/character units so
        /// `EditorTabs::applyBufferEdits` can splice them inside one
        /// `beginEditBlock` — which is what makes a 200-caret keystroke one
        /// Ctrl+Z (ADR-0023).
        #[qobject]
        type EditorOps = super::EditorOpsRust;

        /// Tell this object where the widget's carets are. Called on every
        /// caret move, including the ordinary single-caret one.
        #[qinvokable]
        #[cxx_name = "setCarets"]
        fn set_carets(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            carets: Vec<FfiCaret>,
        );

        /// Where the carets are now, for the widget to paint.
        #[qinvokable]
        #[cxx_name = "carets"]
        fn carets(self: &EditorOps, tab_id: u64, text: &QString) -> Vec<FfiCaret>;

        /// How many carets this tab has. The widget branches on `> 1` to
        /// decide whether a keystroke is routed through Rust at all — a
        /// branch about which code path runs, not about what an edit means.
        #[qinvokable]
        #[cxx_name = "caretCount"]
        fn caret_count(self: &EditorOps, tab_id: u64) -> u32;

        /// Esc: back to the primary caret alone.
        #[qinvokable]
        #[cxx_name = "clearSecondaryCarets"]
        fn clear_secondary_carets(self: Pin<&mut EditorOps>, tab_id: u64);

        /// The tab closed — drop everything remembered about it.
        #[qinvokable]
        #[cxx_name = "forgetTab"]
        fn forget_tab(self: Pin<&mut EditorOps>, tab_id: u64);

        /// Re-read the cached settings after the dialog commits, so a
        /// changed tab width takes effect without a restart.
        #[qinvokable]
        #[cxx_name = "reloadSettings"]
        fn reload_settings(self: Pin<&mut EditorOps>);

        /// Alt+Click: one more caret at a document position. Refuses past
        /// the caret ceiling with a typed code (ADR-0003).
        #[qinvokable]
        #[cxx_name = "addCaretAt"]
        fn add_caret_at(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            position: u32,
        ) -> FfiResult;

        /// Ctrl+Alt+Up / Ctrl+Alt+Down: a caret on the neighbouring line at
        /// the primary caret's visual column.
        #[qinvokable]
        #[cxx_name = "addCaretVertically"]
        fn add_caret_vertically(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            downwards: bool,
        ) -> FfiResult;

        /// Ctrl+D: add the next occurrence of what the primary caret
        /// covers, selecting the word under it first when it is collapsed.
        #[qinvokable]
        #[cxx_name = "selectNextOccurrence"]
        fn select_next_occurrence(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
        ) -> FfiResult;

        /// Alt+Shift+drag: one caret per line between two document
        /// positions, at the visual columns those positions sit at.
        #[qinvokable]
        #[cxx_name = "columnSelect"]
        fn column_select(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            anchor: u32,
            head: u32,
        ) -> FfiResult;

        /// Typing at every caret, as one transaction.
        #[qinvokable]
        #[cxx_name = "typeText"]
        fn type_text(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            typed: &QString,
        ) -> Vec<FfiTextEdit>;

        /// Backspace at every caret.
        #[qinvokable]
        #[cxx_name = "backspace"]
        fn backspace(self: Pin<&mut EditorOps>, tab_id: u64, text: &QString) -> Vec<FfiTextEdit>;

        /// Insert pasted text verbatim, at every caret. Paste is not
        /// typing: `foo(bar` must not become `foo(bar)`.
        #[qinvokable]
        #[cxx_name = "pasteText"]
        fn paste_text(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            pasted: &QString,
        ) -> Vec<FfiTextEdit>;

        /// Delete at every caret.
        #[qinvokable]
        #[cxx_name = "deleteForward"]
        fn delete_forward(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
        ) -> Vec<FfiTextEdit>;

        /// Enter at every caret: the newline and the indent the language
        /// wants at that point.
        #[qinvokable]
        #[cxx_name = "newline"]
        fn newline(self: Pin<&mut EditorOps>, tab_id: u64, text: &QString) -> Vec<FfiTextEdit>;

        /// Duplicate (0), move up (1), move down (2), delete (3), join (4).
        #[qinvokable]
        #[cxx_name = "lineOp"]
        fn line_op(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            kind: u8,
        ) -> Vec<FfiTextEdit>;

        /// Ctrl+/ (`block` false) and Ctrl+Shift+/ (`block` true).
        #[qinvokable]
        #[cxx_name = "toggleComment"]
        fn toggle_comment(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            block: bool,
        ) -> Vec<FfiTextEdit>;

        /// Tab / Shift+Tab over a selection.
        #[qinvokable]
        #[cxx_name = "indentSelection"]
        fn indent_selection(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            outdent: bool,
        ) -> Vec<FfiTextEdit>;

        /// Ctrl+W: grow every caret to its enclosing syntax node.
        #[qinvokable]
        #[cxx_name = "expandSelection"]
        fn expand_selection(self: Pin<&mut EditorOps>, tab_id: u64, text: &QString);

        /// Ctrl+Shift+W: back down the path Ctrl+W took. A selection the
        /// history does not recognise is left alone.
        #[qinvokable]
        #[cxx_name = "shrinkSelection"]
        fn shrink_selection(self: Pin<&mut EditorOps>, tab_id: u64);

        /// Ctrl+]: the document position the bracket at `position` is
        /// answered by, or -1 when the caret is not on one.
        #[qinvokable]
        #[cxx_name = "matchingBracket"]
        fn matching_bracket(self: &EditorOps, tab_id: u64, text: &QString, position: u32) -> i64;

        /// The edits a save would make before it writes the file (F1-11):
        /// trim, final newline, line-ending normalisation. Splice these
        /// into the buffer first so the tidying is one undo entry, then
        /// read the (now tidied) text to hand to `saveTab`.
        #[qinvokable]
        #[cxx_name = "saveRuleEdits"]
        fn save_rule_edits(self: &EditorOps, tab_id: u64, text: &QString) -> Vec<FfiTextEdit>;

        /// The tab width this tab's language resolves to (show-whitespace-
        /// characters task): what `CodeEditor::setTabStopDistance` uses.
        #[qinvokable]
        #[cxx_name = "tabWidthForTab"]
        fn tab_width_for_tab(self: &EditorOps, tab_id: u64) -> u32;

        /// Classified space/tab spans for `text` (show-whitespace-
        /// characters task) — the view passes its currently visible
        /// blocks' text, joined with `\n`, once per repaint rather than
        /// once per line.
        #[qinvokable]
        #[cxx_name = "whitespaceSpans"]
        fn whitespace_spans(self: &EditorOps, text: &QString) -> Vec<FfiWhitespaceSpan>;

        /// The carets changed without an edit — after Ctrl+D, Alt+Click, a
        /// column selection or an expansion — so the widget repaints them.
        #[qsignal]
        #[cxx_name = "caretsChanged"]
        fn carets_changed(self: Pin<&mut EditorOps>, tab_id: u64);
    }

    extern "RustQt" {
        /// Language-server adapter (Task L2): owns one `lsp_core::LspManager`
        /// (on a worker thread) and the `DiagnosticStore` the panel and the
        /// editor read.
        ///
        /// Translation only, per `docs/architecture/layering.md`: every rule
        /// — which server serves a language, when one is restarted, which
        /// rows exist in which order, how severities rank — lives in
        /// `lsp-core` or `app-config`. What is left here is a worker thread
        /// (so a blocking `initialize` handshake never freezes the UI) and a
        /// listener thread draining `Receiver<LspEvent>` through
        /// `CxxQtThread::queue()`, the same shape `SearchModel` and
        /// `TerminalSession` already use (ADR-0004, ADR-0007).
        #[qobject]
        type LanguageService = super::LanguageServiceRust;

        /// Point the language servers at a project root and (re)load the
        /// `[[language_server]]` settings. Stops whatever was running for the
        /// previous project. No server is launched here — that happens
        /// lazily, on the first file of a language (see `documentOpened`),
        /// because launching every catalog server at startup would spawn a
        /// dozen processes for a project that uses one language.
        #[qinvokable]
        #[cxx_name = "openProject"]
        fn open_project(self: Pin<&mut LanguageService>, root_path: &QString);

        /// A tab was opened: start that language's server if this is the
        /// first file of its kind, then send `didOpen`. A file whose language
        /// has no configured, enabled server is silently ignored — the
        /// panel's empty state says so.
        #[qinvokable]
        #[cxx_name = "documentOpened"]
        fn document_opened(self: Pin<&mut LanguageService>, path: &QString, text: &QString);

        /// The buffer changed (`didChange`, full-text sync). Cheap enough to
        /// call on a debounce from the view; the version counter is the
        /// manager's.
        #[qinvokable]
        #[cxx_name = "documentChanged"]
        fn document_changed(self: Pin<&mut LanguageService>, path: &QString, text: &QString);

        /// The buffer was written to disk (`didSave`).
        #[qinvokable]
        #[cxx_name = "documentSaved"]
        fn document_saved(self: Pin<&mut LanguageService>, path: &QString);

        /// The tab was closed (`didClose`); its diagnostics stop being shown.
        #[qinvokable]
        #[cxx_name = "documentClosed"]
        fn document_closed(self: Pin<&mut LanguageService>, path: &QString);

        /// C5: `ProjectTreeModel::watchedFileChanged` — a file on disk
        /// changed under the project root. Buffered and coalesced (a `git
        /// checkout` fires thousands of these) before reaching any server as
        /// one batched `workspace/didChangeWatchedFiles`. `kind` is the LSP
        /// `FileChangeType` (1=created, 2=changed, 3=deleted).
        #[qinvokable]
        #[cxx_name = "watchedFileChanged"]
        fn watched_file_changed(self: Pin<&mut LanguageService>, path: &QString, kind: i32);

        /// L6 — the `[[language_server]]` settings were committed: re-read
        /// them and stop every server whose configuration changed or was
        /// switched off, so the next `reopenDocument` starts the new one.
        /// Servers whose configuration is untouched are left running.
        #[qinvokable]
        #[cxx_name = "applyServerSettings"]
        fn apply_server_settings(self: Pin<&mut LanguageService>);

        /// `documentOpened` for a document that may already be open: after
        /// `applyServerSettings` the view re-announces every open tab, and
        /// only the ones whose server was stopped need re-sending.
        #[qinvokable]
        #[cxx_name = "reopenDocument"]
        fn reopen_document(self: Pin<&mut LanguageService>, path: &QString, text: &QString);

        /// L6 — `Restart Server`: stop this language's server and start it
        /// again from the saved configuration. An action, not a setting, so
        /// it takes effect immediately rather than on OK.
        #[qinvokable]
        #[cxx_name = "restartServer"]
        fn restart_server(self: Pin<&mut LanguageService>, language_id: &QString);

        /// Every known diagnostic, grouped by file and ordered within it.
        #[qinvokable]
        fn diagnostics(self: &LanguageService) -> Vec<FfiDiagnostic>;

        /// Just one file's diagnostics — what an editor underlines.
        #[qinvokable]
        #[cxx_name = "diagnosticsForFile"]
        fn diagnostics_for_file(self: &LanguageService, path: &QString) -> Vec<FfiDiagnostic>;

        /// Counts per severity, for the status bar and the filter buttons.
        #[qinvokable]
        #[cxx_name = "diagnosticCounts"]
        fn diagnostic_counts(self: &LanguageService) -> FfiDiagnosticCounts;

        /// Whether a server is configured, enabled and started for this
        /// file's language — the difference between "no problems" and "no
        /// language server", which is the panel's empty state.
        #[qinvokable]
        #[cxx_name = "hasServerForFile"]
        fn has_server_for_file(self: &LanguageService, path: &QString) -> bool;

        /// The configured server's display name for this file's language, or
        /// empty when there is none — the "Waiting for rust-analyzer..." wording.
        #[qinvokable]
        #[cxx_name = "serverNameForFile"]
        fn server_name_for_file(self: &LanguageService, path: &QString) -> QString;

        /// L3 — the pointer dwelled over an identifier: ask the server what
        /// it is. `line` is 0-based and `character` counts UTF-16 code
        /// units, which is what the protocol speaks and what `QTextCursor`
        /// already counts. The answer arrives (or doesn't) on `hoverReady`;
        /// nothing blocks, because the request runs on the worker thread.
        #[qinvokable]
        #[cxx_name = "hoverAt"]
        fn hover_at(self: Pin<&mut LanguageService>, path: &QString, line: u32, character: u32);

        /// The pointer moved or left the editor: whatever hover is in flight
        /// is no longer wanted. Discarding it is `lsp_core::HoverTracker`'s
        /// rule, not the view's — a late answer shown at the new position
        /// would describe the wrong symbol.
        #[qinvokable]
        #[cxx_name = "cancelHover"]
        fn cancel_hover(self: Pin<&mut LanguageService>);

        /// Hover text for the most recent, still-current request, as the
        /// HTML subset Qt tooltips render. Never emitted for a superseded or
        /// cancelled request, and never for an empty hover.
        #[qsignal]
        #[cxx_name = "hoverReady"]
        fn hover_ready(self: Pin<&mut LanguageService>, html: QString);

        /// RF12 — emitted instead of `hoverReady` when no server answered:
        /// no server for the language, none running yet, an error, a
        /// timeout, or an empty hover. The declaration the name-based index
        /// resolves to is shown instead, which is what gives a signature
        /// tooltip in the languages this IDE has a grammar but no server
        /// for. Which of the two it is, is `lsp_core::hover_outcome`'s
        /// decision — the same shape as `definitionFallback`.
        #[qsignal]
        #[cxx_name = "hoverFallback"]
        fn hover_fallback(self: Pin<&mut LanguageService>);

        /// L4 — Go to Declaration at a position, asked of the language
        /// server first (ADR-0016). Answers on exactly one of two paths:
        /// `definitionFound`* then `definitionFinished` when the server had
        /// an answer, or `definitionFallback` when it did not — no server for
        /// the language, none running yet, an error, a timeout, or an empty
        /// result. Which of those it is, is
        /// `lsp_core::definition_outcome`'s decision, never the view's.
        #[qinvokable]
        #[cxx_name = "resolveDefinition"]
        fn resolve_definition(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
        );

        /// One target of a `resolveDefinition`, in the server's own order.
        #[qsignal]
        #[cxx_name = "definitionFound"]
        fn definition_found(self: Pin<&mut LanguageService>, target: FfiDefinition);

        /// Emitted once after the last `definitionFound`: the server answered
        /// and its answer is complete.
        #[qsignal]
        #[cxx_name = "definitionFinished"]
        fn definition_finished(self: Pin<&mut LanguageService>);

        /// Emitted instead of the pair above when the server did not answer:
        /// ADR-0011's name-based index resolves the gesture instead, which is
        /// what makes Go to Declaration work with no server installed.
        #[qsignal]
        #[cxx_name = "definitionFallback"]
        fn definition_fallback(self: Pin<&mut LanguageService>);

        /// C12 — emitted instead of the pair above when the server's answer
        /// is a non-`file:` URI (csharp-ls's `csharp:/metadata/...` for
        /// decompiled framework code) that this IDE cannot yet open as a
        /// tab. `message` is shown as-is; this is the clean refusal
        /// `docs/architecture/decisions/0003-ffi-conventions.md`'s C12
        /// amendment calls for — never a broken tab built from the raw URI
        /// treated as a path.
        #[qsignal]
        #[cxx_name = "definitionUnavailable"]
        fn definition_unavailable(self: Pin<&mut LanguageService>, message: QString);

        /// C12-followup — the fetch `definitionUnavailable`'s doc comment
        /// describes landed: `csharp/metadata` answered, and its text is now
        /// open as a read-only virtual document with this `tab_id`.
        /// `newly_opened` tells `EditorTabs` whether to build the tab widget
        /// (via the same path `DocumentManager::tabOpened` drives) before
        /// focusing it, or only focus the one already open for this
        /// decompiled symbol. A fetch failure still emits
        /// `definitionUnavailable` instead — never a signal of its own, so
        /// the refusal message stays in one place.
        #[qsignal]
        #[cxx_name = "virtualDocumentOpened"]
        fn virtual_document_opened(
            self: Pin<&mut LanguageService>,
            tab_id: u64,
            title: QString,
            newly_opened: bool,
        );

        /// L5 — ask the server what could be typed at this position.
        /// `text_before_cursor` is the current line up to the caret, from
        /// which `lsp_core::completion` derives both the word being typed
        /// and whether the request is worth making at all: `explicit_request`
        /// (the shortcut) always asks, otherwise a server trigger character or
        /// two identifier characters do. A request that is not worth making
        /// is dropped here — including one whose answer is already in hand
        /// (a complete list is filtered locally as the word grows) — so the
        /// view may call this on every keystroke.
        ///
        /// Answers on `completionReady`, never synchronously and never on
        /// the UI thread. A superseded or too-late answer produces no signal
        /// at all — `lsp_core::CompletionTracker`'s rule.
        #[qinvokable]
        #[cxx_name = "completionAt"]
        fn completion_at(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
            text_before_cursor: &QString,
            // `explicit` is a C++ keyword, so the parameter cannot be named
            // that: this is the Ctrl+Space gesture.
            explicit_request: bool,
        );

        /// The popup closed, or the caret left the word: whatever is in
        /// flight is no longer wanted.
        #[qinvokable]
        #[cxx_name = "cancelCompletion"]
        fn cancel_completion(self: Pin<&mut LanguageService>);

        /// The last answer's candidates for the word inside
        /// `text_before_cursor`, ordered by the server's `sortText` and
        /// matched against its `filterText`. Empty when nothing matches, and
        /// empty when the caret has left the word the answer was about — all
        /// of that is `lsp_core::completion`'s decision, including picking
        /// the word out of the line, so the popup can be driven straight
        /// from this.
        #[qinvokable]
        #[cxx_name = "completionItems"]
        fn completion_items(
            self: &LanguageService,
            text_before_cursor: &QString,
        ) -> Vec<FfiCompletionItem>;

        /// Accept `item` — the row `completionItems` handed over, passed
        /// straight back — with the caret where the user has it now.
        ///
        /// Which span the insertion replaces is a rule, not arithmetic the
        /// view may do: it depends on whether the server named a range, and
        /// on characters typed while the request was in flight, both of
        /// which `lsp_core::completion` decides. C7: when the server offers
        /// `completionItem/resolve`, this also asks for it and merges
        /// whatever `additionalTextEdits` comes back — the `using` an
        /// unimported type's completion brings with it — into the same
        /// splice, bounded by the crate's default request timeout so an
        /// accept can never hang. Answers on `completionEditReady`, never
        /// synchronously, because the resolve round trip may be in it.
        #[qinvokable]
        #[cxx_name = "acceptCompletion"]
        fn accept_completion(
            self: Pin<&mut LanguageService>,
            item: &FfiCompletionItem,
            caret_line: u32,
            caret_character: u32,
        );

        /// The splice list for the last `acceptCompletion`, ready for
        /// `EditorTabs::applyEditsTo` like every other buffer change — one
        /// edit block, so accepting an import along with the item it
        /// belongs to is one Ctrl+Z.
        #[qsignal]
        #[cxx_name = "completionEditReady"]
        fn completion_edit_ready(self: Pin<&mut LanguageService>, edits: Vec<FfiTextEdit>);

        /// C7 — as the popup's selection moves, ask the server to fill in
        /// documentation and detail for `resolve_data` (opaque; from the
        /// row's own `FfiCompletionItem`). A server that never advertised
        /// `completionItem/resolve`, or a `resolve_data` that carries none,
        /// is a silent no-op — the initial list's own fields are shown as
        /// they are. Cancelling a stale request is
        /// `resolveCompletionPreview`'s own re-request-invalidates-the-last-one
        /// rule (`lsp_core::CompletionResolveTracker`), the same shape
        /// `hoverAt`/`cancelHover` already use; a server round trip that
        /// outlives its usefulness is left to time out on its own rather
        /// than cancelled a second way.
        #[qinvokable]
        #[cxx_name = "resolveCompletionPreview"]
        fn resolve_completion_preview(self: Pin<&mut LanguageService>, resolve_data: &QString);

        /// The selection moved again, or the popup closed: whatever preview
        /// resolution is in flight is no longer wanted.
        #[qinvokable]
        #[cxx_name = "cancelCompletionPreview"]
        fn cancel_completion_preview(self: Pin<&mut LanguageService>);

        /// A preview resolution arrived and is still current — replace the
        /// row's shown detail/documentation with these.
        #[qsignal]
        #[cxx_name = "completionPreviewReady"]
        fn completion_preview_ready(
            self: Pin<&mut LanguageService>,
            detail: QString,
            documentation: QString,
        );

        /// A completion answer arrived and is still current. The view reads
        /// it back with `completionItems`, the same
        /// re-read-what-you-display shape `diagnosticsChanged` uses.
        #[qsignal]
        #[cxx_name = "completionReady"]
        fn completion_ready(self: Pin<&mut LanguageService>);

        /// RF8 — ask the server what refactorings it offers for a range.
        ///
        /// `only` narrows the request to a kind family (`refactor.extract`)
        /// or is empty for everything. It is only ever a hint: a server that
        /// ignores it, or answers nothing to it, is asked again unfiltered
        /// and the answer filtered here — `lsp_core::code_action`'s rule.
        /// Answers on `codeActionsReady`, which the view reads back with
        /// `codeActions`.
        #[qinvokable]
        #[cxx_name = "codeActionsAt"]
        fn code_actions_at(
            self: Pin<&mut LanguageService>,
            path: &QString,
            start_line: u32,
            start_character: u32,
            end_line: u32,
            end_character: u32,
            only: &QString,
        );

        /// The offers from the last `codeActionsAt`, in the server's own
        /// order — it ranks its list and nothing here knows better.
        #[qinvokable]
        #[cxx_name = "codeActions"]
        fn code_actions(self: &LanguageService) -> Vec<FfiCodeAction>;

        /// Reformat one open document, whole-file (F1-14). Answers through
        /// the same `refactorReady`/`refactorFailed`/`pendingEdits`
        /// protocol a rename uses — `touches_other_files` is always false,
        /// so the view applies it straight away, and one Ctrl+Z undoes it.
        #[qinvokable]
        #[cxx_name = "requestFormatting"]
        fn request_formatting(
            self: Pin<&mut LanguageService>,
            path: &QString,
            buffer_revision: i64,
        );

        /// A `codeActionsAt` answered. Empty is a legitimate answer and is
        /// still signalled, so the view can say "nothing here" rather than
        /// leaving the gesture hanging.
        #[qsignal]
        #[cxx_name = "codeActionsReady"]
        fn code_actions_ready(self: Pin<&mut LanguageService>);

        /// RF8 — carry out the offer at `index` of the last `codeActions`.
        ///
        /// Resolving it, applying its edit and running its command all
        /// happen off the UI thread, in the order `lsp_core::code_action`
        /// prescribes, under a refactoring session — without which the edit
        /// a command produces would be refused as unsolicited.
        /// `buffer_revision` is the editor's document revision now, and what
        /// a later `takePendingEdits` is checked against.
        #[qinvokable]
        #[cxx_name = "applyCodeAction"]
        fn apply_code_action(self: Pin<&mut LanguageService>, index: u32, buffer_revision: i64);

        /// F2-8 — everything that can be done at the caret: `code.
        /// showIntentions` (Alt+Enter). Merges the diagnostic-scoped and
        /// range-scoped `codeAction` answers (`lsp_core::intentions::
        /// assemble`), grouped and ordered for the popup. Answers on
        /// `intentionsReady`.
        #[qinvokable]
        #[cxx_name = "requestIntentions"]
        fn request_intentions(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
        );

        /// The caret moved, or the tab did: whatever `requestIntentions` is
        /// waiting on is no longer wanted.
        #[qinvokable]
        #[cxx_name = "cancelIntentions"]
        fn cancel_intentions(self: Pin<&mut LanguageService>);

        /// The offers from the last `requestIntentions`, grouped and ordered
        /// for the popup — see `lsp_core::intentions::assemble`.
        #[qinvokable]
        #[cxx_name = "intentions"]
        fn intentions(self: &LanguageService) -> Vec<FfiIntention>;

        /// A `requestIntentions` answered — possibly with nothing, which is
        /// still signalled so the bulb can hide.
        #[qsignal]
        #[cxx_name = "intentionsReady"]
        fn intentions_ready(self: Pin<&mut LanguageService>);

        /// Carry out the offer at `index` of the last `intentions`. Shares
        /// `applyCodeAction`'s pending-refactor protocol exactly — see
        /// `run_action` on the Rust side.
        #[qinvokable]
        #[cxx_name = "applyIntention"]
        fn apply_intention(self: Pin<&mut LanguageService>, index: u32, buffer_revision: i64);

        /// F2-8's remaining LSP surface: organize imports for a whole
        /// document. `last_line` is the document's last line (the view's own
        /// `QTextDocument::blockCount() - 1`, exactly as `codeActionsAt`'s
        /// range comes from the view). Applies through the same
        /// pending-refactor protocol as `applyCodeAction`, or reports
        /// `refactorFailed` when the server has nothing to organize.
        #[qinvokable]
        #[cxx_name = "organizeImports"]
        fn organize_imports(
            self: Pin<&mut LanguageService>,
            path: &QString,
            last_line: u32,
            buffer_revision: i64,
        );

        /// F2-9 — signature help for a call. `text` and `byte_offset` are
        /// the live buffer and the caret's byte offset in it, per §1: the
        /// call-site scan (`lsp_core::signature_help::call_site_at`) needs
        /// the surrounding text, not just a position. `explicit` is
        /// Ctrl+P; `showing` is whether a tip is already up, which decides
        /// whether an ordinary keystroke is worth asking again over.
        /// Answers on `signatureHelpReady` — including to say "nothing here
        /// any more", which is how the tip knows to close.
        #[qinvokable]
        #[cxx_name = "requestSignatureHelp"]
        fn request_signature_help(
            self: Pin<&mut LanguageService>,
            path: &QString,
            text: &QString,
            byte_offset: u64,
            explicit_request: bool,
            showing: bool,
        );

        #[qsignal]
        #[cxx_name = "signatureHelpReady"]
        fn signature_help_ready(self: Pin<&mut LanguageService>);

        /// The overload `requestSignatureHelp` last resolved, or the default
        /// (`has_signature: false`) when there is nothing to show.
        #[qinvokable]
        #[cxx_name = "signatureHelp"]
        fn signature_help(self: &LanguageService) -> FfiSignatureHelp;

        /// F2-9 — every occurrence of the symbol under the caret in this
        /// file, for `signature_tip.cpp`'s occurrence painting. Answers on
        /// `documentHighlightsReady`.
        #[qinvokable]
        #[cxx_name = "requestDocumentHighlights"]
        fn request_document_highlights(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
        );

        #[qsignal]
        #[cxx_name = "documentHighlightsReady"]
        fn document_highlights_ready(self: Pin<&mut LanguageService>);

        #[qinvokable]
        #[cxx_name = "documentHighlights"]
        fn document_highlights(self: &LanguageService) -> Vec<FfiDocumentHighlight>;

        /// F2-9 — inlay hints for the visible lines, inclusive. There is no
        /// whole-document form on purpose (`lsp_core::inlay_hint`'s own
        /// doc): a 10,000-line file must not be asked for 10,000 hints to
        /// paint fifty. Answers on `inlayHintsReady`.
        #[qinvokable]
        #[cxx_name = "requestInlayHints"]
        fn request_inlay_hints(
            self: Pin<&mut LanguageService>,
            path: &QString,
            first_line: u32,
            last_line: u32,
        );

        #[qsignal]
        #[cxx_name = "inlayHintsReady"]
        fn inlay_hints_ready(self: Pin<&mut LanguageService>);

        #[qinvokable]
        #[cxx_name = "inlayHints"]
        fn inlay_hints(self: &LanguageService) -> Vec<FfiInlayHint>;

        /// C9 — fire-and-forget `textDocument/semanticTokens/full` for
        /// `path`'s whole document, gated on
        /// `LspManager::semantic_tokens_legend` (checked at call time, so
        /// this also covers a server that registered the capability
        /// dynamically after this method's first no-op call — see
        /// `request_semantic_tokens`'s own doc comment). A server with no
        /// legend yet, or with nothing to say, leaves the previous answer
        /// (if any) in place rather than clearing it: never let "waiting
        /// for the server" mean "no colour at all" (F0-16). Answers on
        /// `semanticTokensReady`.
        #[qinvokable]
        #[cxx_name = "requestSemanticTokens"]
        fn request_semantic_tokens(self: Pin<&mut LanguageService>, path: &QString, text: &QString);

        #[qsignal]
        #[cxx_name = "semanticTokensReady"]
        fn semantic_tokens_ready(self: Pin<&mut LanguageService>, path: QString);

        /// The last decoded-and-mapped semantic-token spans for `path`,
        /// already in `syntax_core::HighlightSpan`'s byte-offset/scope-id
        /// shape — the same shape `SyntaxHighlighterHandle::overlay_semantic_tokens`
        /// takes as its `semantic` argument. Empty before the first answer,
        /// or for a document nothing has ever requested tokens for.
        #[qinvokable]
        #[cxx_name = "semanticTokenSpans"]
        fn semantic_token_spans(self: &LanguageService, path: &QString) -> Vec<FfiHighlightSpan>;

        /// C10 — fire-and-forget `textDocument/codeLens` for `path`'s whole
        /// document, gated on `LspManager::code_lenses_supported` (checked
        /// at call time, so this also covers a server that registered the
        /// capability dynamically after this method's first no-op call —
        /// see `request_code_lenses`'s own doc comment). Answers on
        /// `codeLensesReady`.
        #[qinvokable]
        #[cxx_name = "requestCodeLenses"]
        fn request_code_lenses(self: Pin<&mut LanguageService>, path: &QString);

        #[qsignal]
        #[cxx_name = "codeLensesReady"]
        fn code_lenses_ready(self: Pin<&mut LanguageService>, path: QString);

        /// The last-fetched lenses for `path`: line, label, clickable —
        /// what the C++ lens strip needs to draw one row per lens and
        /// forward a click back by index. Empty before the first answer,
        /// or for a document nothing has ever requested lenses for.
        #[qinvokable]
        #[cxx_name = "codeLenses"]
        fn code_lenses(self: &LanguageService, path: &QString) -> Vec<FfiCodeLens>;

        /// C10 — run the lens at `index` in the last answer `codeLenses`
        /// returned for `path`: resolve it if it still needs
        /// `codeLens/resolve`, then send its command through the existing
        /// `workspace/executeCommand` path with the refactoring session
        /// gate held around it, so a `workspace/applyEdit` the command
        /// provokes is recognised as legitimate. Any resulting edit arrives
        /// on the usual `refactorReady`/`refactorFailed` refactor-preview
        /// flow, not a signal of its own.
        ///
        /// Called from `CodeEditor::codeLensClicked` via
        /// `EditorTabs`'s connection to it (C10-followup): the lens strip
        /// paints one pill per `FfiCodeLens` on its line and forwards a
        /// click back by index.
        #[qinvokable]
        #[cxx_name = "runCodeLens"]
        fn run_code_lens(self: Pin<&mut LanguageService>, path: &QString, index: u32);

        /// C11 — `textDocument/prepareCallHierarchy` at a caret position.
        /// Gated on `LspManager::call_hierarchy_supported` inside the job,
        /// same as `requestCodeLenses`. Answers on `callHierarchyReady`; call
        /// hierarchy has no index fallback at all (`lsp_core::hierarchy`
        /// module docs), so an unsupported server or empty answer simply
        /// leaves `callHierarchyItems` empty.
        ///
        /// Consumed by `cpp/hierarchy_panel.cpp` (C11-followup), triggered
        /// from Navigate > Show Call Hierarchy.
        #[qinvokable]
        #[cxx_name = "requestCallHierarchy"]
        fn request_call_hierarchy(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
        );

        #[qsignal]
        #[cxx_name = "callHierarchyReady"]
        fn call_hierarchy_ready(self: Pin<&mut LanguageService>);

        /// The last `prepareCallHierarchy` answer.
        #[qinvokable]
        #[cxx_name = "callHierarchyItems"]
        fn call_hierarchy_items(self: &LanguageService) -> Vec<FfiHierarchyItem>;

        /// `callHierarchy/incomingCalls` for the item at `index` in the last
        /// `callHierarchyItems` answer. Answers on `incomingCallsReady`.
        #[qinvokable]
        #[cxx_name = "requestIncomingCalls"]
        fn request_incoming_calls(self: Pin<&mut LanguageService>, index: u32);

        #[qsignal]
        #[cxx_name = "incomingCallsReady"]
        fn incoming_calls_ready(self: Pin<&mut LanguageService>);

        #[qinvokable]
        #[cxx_name = "incomingCalls"]
        fn incoming_calls(self: &LanguageService) -> Vec<FfiIncomingCall>;

        /// `callHierarchy/outgoingCalls` for the item at `index` in the last
        /// `callHierarchyItems` answer. Answers on `outgoingCallsReady`; an
        /// empty answer is a real leaf, not a hint to look elsewhere.
        #[qinvokable]
        #[cxx_name = "requestOutgoingCalls"]
        fn request_outgoing_calls(self: Pin<&mut LanguageService>, index: u32);

        #[qsignal]
        #[cxx_name = "outgoingCallsReady"]
        fn outgoing_calls_ready(self: Pin<&mut LanguageService>);

        #[qinvokable]
        #[cxx_name = "outgoingCalls"]
        fn outgoing_calls(self: &LanguageService) -> Vec<FfiOutgoingCall>;

        /// C11 — `textDocument/prepareTypeHierarchy` at a caret position.
        /// LSP-only, like `requestCallHierarchy` — the index fallback
        /// applies one step later, to `requestSupertypes`/`requestSubtypes`,
        /// once a type name is known.
        #[qinvokable]
        #[cxx_name = "requestTypeHierarchy"]
        fn request_type_hierarchy(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
        );

        #[qsignal]
        #[cxx_name = "typeHierarchyReady"]
        fn type_hierarchy_ready(self: Pin<&mut LanguageService>);

        #[qinvokable]
        #[cxx_name = "typeHierarchyItems"]
        fn type_hierarchy_items(self: &LanguageService) -> Vec<FfiHierarchyItem>;

        /// `typeHierarchy/supertypes` for the item at `index` in the last
        /// `typeHierarchyItems` answer — LSP-first, `index-core`'s
        /// supertype-edge data as the fallback
        /// (`lsp_core::hierarchy::type_hierarchy_outcome`, ADR-0016's same
        /// precedence as go-to-definition). Answers on `supertypesReady`.
        #[qinvokable]
        #[cxx_name = "requestSupertypes"]
        fn request_supertypes(self: Pin<&mut LanguageService>, index: u32);

        #[qsignal]
        #[cxx_name = "supertypesReady"]
        fn supertypes_ready(self: Pin<&mut LanguageService>);

        #[qinvokable]
        #[cxx_name = "supertypes"]
        fn supertypes(self: &LanguageService) -> Vec<FfiHierarchyItem>;

        /// `typeHierarchy/subtypes`, the other direction of the same walk.
        /// Answers on `subtypesReady`.
        #[qinvokable]
        #[cxx_name = "requestSubtypes"]
        fn request_subtypes(self: Pin<&mut LanguageService>, index: u32);

        #[qsignal]
        #[cxx_name = "subtypesReady"]
        fn subtypes_ready(self: Pin<&mut LanguageService>);

        #[qinvokable]
        #[cxx_name = "subtypes"]
        fn subtypes(self: &LanguageService) -> Vec<FfiHierarchyItem>;

        /// RF8 — rename the symbol at a position.
        ///
        /// Asks `prepareRename` first where the server implements it, then
        /// `rename`. Answers on `refactorReady` when the server produced an
        /// edit, on `refactorFallback` when no server did (which is what
        /// makes rename work for a language with a grammar and no server),
        /// and on `refactorFailed` when the server refused. Which of those
        /// it is, is `lsp_core::rename`'s decision.
        #[qinvokable]
        #[cxx_name = "renameAt"]
        fn rename_at(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
            new_name: &QString,
            buffer_revision: i64,
        );

        /// Whether the server would let the symbol at this position be
        /// renamed, and what to prefill the input with. Blocking and cheap
        /// only because it is not: it queues like everything else and answers
        /// on `renamePrepared`.
        #[qinvokable]
        #[cxx_name = "prepareRename"]
        fn prepare_rename(
            self: Pin<&mut LanguageService>,
            path: &QString,
            line: u32,
            character: u32,
        );

        /// The rename may go ahead; `placeholder` is what to prefill the
        /// input with, empty when the server did not name one.
        #[qsignal]
        #[cxx_name = "renamePrepared"]
        fn rename_prepared(self: Pin<&mut LanguageService>, placeholder: QString);

        /// The server said this element cannot be renamed. Only an explicit
        /// refusal reaches here — a server that does not implement
        /// `prepareRename` produces `renamePrepared`, because its silence is
        /// not a refusal (`lsp_core::rename::prepare_outcome`).
        #[qsignal]
        #[cxx_name = "renameRejected"]
        fn rename_rejected(self: Pin<&mut LanguageService>, reason: QString);

        /// A refactoring produced edits and is waiting to be applied. The
        /// summary says how much it changes and whether a preview is
        /// required; the edits themselves come from `pendingEdits`.
        #[qsignal]
        #[cxx_name = "refactorReady"]
        fn refactor_ready(self: Pin<&mut LanguageService>, summary: FfiRefactorSummary);

        /// No language server answered the rename, so the name-based index
        /// answers instead — the same shape as `definitionFallback`.
        #[qsignal]
        #[cxx_name = "refactorFallback"]
        fn refactor_fallback(self: Pin<&mut LanguageService>);

        /// The refactoring could not be done, and nothing was changed.
        #[qsignal]
        #[cxx_name = "refactorFailed"]
        fn refactor_failed(self: Pin<&mut LanguageService>, message: QString);

        /// Every edit the pending refactoring would make, for the preview.
        /// Reading them changes nothing.
        #[qinvokable]
        #[cxx_name = "pendingEdits"]
        fn pending_edits(self: &LanguageService) -> Vec<FfiTextEdit>;

        /// Every file the pending refactoring would create, rename or
        /// delete (F2-3), for the preview to list as such. Reading them
        /// changes nothing.
        #[qinvokable]
        #[cxx_name = "pendingOps"]
        fn pending_ops(self: &LanguageService) -> Vec<FfiResourceOp>;

        /// The before/after text of one file in the pending refactoring, for
        /// `RefactorPreviewDialog`'s `DiffView` panel (F3-15). `path` must be
        /// one `pendingEdits()` named, and the preview only asks for one
        /// when that file's row is selected — computing every file's diff
        /// up front would cost more than most refactorings ever need shown.
        /// Empty texts when there is nothing pending or `path` is not in it.
        #[qinvokable]
        #[cxx_name = "pendingFileDiff"]
        fn pending_file_diff(self: &LanguageService, path: &QString) -> FfiFileDiff;

        /// The line hunks for the same file `pendingFileDiff` describes.
        #[qinvokable]
        #[cxx_name = "pendingFileHunks"]
        fn pending_file_hunks(self: &LanguageService, path: &QString) -> Vec<FfiHunk>;

        /// The intra-line spans for the same file, one entry per changed
        /// word in every modified hunk.
        #[qinvokable]
        #[cxx_name = "pendingFileSpans"]
        fn pending_file_spans(self: &LanguageService, path: &QString) -> Vec<FfiInlineSpan>;

        /// Leave `path` out of the pending refactoring — the user unticked
        /// it in the preview. Call before `takePendingEdits`; excluding a
        /// path that is not in the plan does nothing.
        #[qinvokable]
        #[cxx_name = "excludeFromRefactor"]
        fn exclude_from_refactor(self: Pin<&mut LanguageService>, path: &QString);

        /// Take the pending edits to apply them, minus every excluded file.
        ///
        /// Empty when the buffer has moved since the request (`buffer_revision`
        /// no longer matches) or when there is nothing pending — the staleness
        /// rule is `lsp_core::EditGate`'s, so the view applies whatever it is
        /// handed and never decides that a late answer is safe.
        ///
        /// Edits are already ordered last-first per document, so the view
        /// splices them in the order given.
        #[qinvokable]
        #[cxx_name = "takePendingEdits"]
        fn take_pending_edits(
            self: Pin<&mut LanguageService>,
            buffer_revision: i64,
        ) -> Vec<FfiTextEdit>;

        /// The gesture was abandoned. Any edit a server is still waiting on
        /// is refused, rather than left unanswered.
        #[qinvokable]
        #[cxx_name = "cancelRefactor"]
        fn cancel_refactor(self: Pin<&mut LanguageService>);

        /// A resource operation this service performed (F2-3) retargeted an
        /// open tab — the same relay `ProjectTreeModel::tabTitleChanged`
        /// sends for a tree-driven rename, reused here because the tab strip
        /// listens to it the same way regardless of who moved the file.
        #[qsignal]
        #[cxx_name = "tabTitleChanged"]
        fn tab_title_changed(self: Pin<&mut LanguageService>, tab_id: u64, title: QString);

        /// Emitted on the Qt thread after the store changed: a server
        /// published, or a document was closed. The view re-reads whatever it
        /// displays rather than being handed a delta.
        #[qsignal]
        #[cxx_name = "diagnosticsChanged"]
        fn diagnostics_changed(self: Pin<&mut LanguageService>);

        /// F0-16: whether any language server is still working on the
        /// project, and on what. `initialize` returning is not the same as
        /// being able to answer — rust-analyzer accepts requests while it
        /// indexes and answers every one of them with nothing — so the
        /// status bar says so, the way it already does for the project
        /// index.
        ///
        /// `busy` false means every server is idle and the other fields are
        /// empty. `has_percent` is false for a server that reports work
        /// without a percentage, which the view shows as an indeterminate
        /// bar rather than as 0%.
        #[qsignal]
        #[cxx_name = "serverBusyChanged"]
        fn server_busy_changed(
            self: Pin<&mut LanguageService>,
            busy: bool,
            name: QString,
            activity: QString,
            has_percent: bool,
            percent: u32,
        );

        /// A server started, became ready, died or gave up. Non-modal by
        /// contract: a crashing server must never raise a dialog, because the
        /// restart backoff would make the application unusable.
        #[qsignal]
        #[cxx_name = "serverStateChanged"]
        fn server_state_changed(
            self: Pin<&mut LanguageService>,
            language_id: QString,
            name: QString,
            state: FfiServerState,
            detail: QString,
            retry_ms: u32,
        );
    }

    // Enables `self.qt_thread()` on `LanguageService` for the LSP listener
    // thread's one cross-thread hop, same pattern as `SearchModel` above.
    impl cxx_qt::Threading for LanguageService {}

    /// One row of the Syntax Colors tree (T4).
    ///
    /// Carries both halves of the row: the *resolved* style the editor will
    /// paint this scope with (what the Sample cell renders, including
    /// parent-scope inheritance), and the entry the control strip edits.
    /// Both are resolved in `settings-model`/`syntax-core`; the view paints.
    struct FfiSyntaxScopeRow {
        scope: QString,
        /// The group header this row belongs under.
        family: QString,
        /// A short fragment representative of the scope.
        sample: QString,
        origin: FfiColorOrigin,
        /// Resolved style for the Sample cell. `has_fg == false` means the
        /// editor's default foreground, as in `FfiScopeStyle`.
        has_fg: bool,
        red: u8,
        green: u8,
        blue: u8,
        sample_bold: bool,
        sample_italic: bool,
        sample_underline: bool,
        /// The stored entry, as the hex field and the three checkboxes show
        /// it. `hex` is empty when nothing but the theme has an opinion.
        hex: QString,
        bold: bool,
        italic: bool,
        underline: bool,
        /// Whether `Reset Scope` would change anything on this row.
        can_reset: bool,
    }

    /// Where a Syntax Colors row's value comes from — the "From" column.
    enum FfiColorOrigin {
        Theme,
        Base,
        Language,
    }

    /// One entry of the Syntax Colors language combo, and of any other list
    /// of languages the settings pages show.
    struct FfiLanguageOption {
        id: QString,
        name: QString,
    }

    /// Where a language came from — the Languages page's grouping.
    enum FfiLanguageSource {
        BuiltIn,
        Overlay,
        Library,
    }

    /// How a Languages row's status word is coloured. `Healthy` renders no
    /// status text at all.
    enum FfiRowSeverity {
        Healthy,
        /// `status.muted`: a true statement about the row that is not a
        /// problem — a language the user turned off.
        Muted,
        Warning,
        Error,
    }

    /// One row of the Languages page (G3).
    struct FfiLanguageRow {
        id: QString,
        name: QString,
        /// Extensions and file names this language claims.
        matches: QString,
        /// The status word, already chosen on the Rust side; empty for a
        /// language that loaded correctly.
        status: QString,
        source: FfiLanguageSource,
        severity: FfiRowSeverity,
    }

    /// The Languages details pane: one failure, already turned into a
    /// sentence a user can act on. The raw Rust error is never sent.
    #[derive(Default)]
    struct FfiLanguageProblem {
        /// The artefact that failed, for the title line.
        artifact: QString,
        sentence: QString,
        /// The specific detail, with a line number when there is one.
        detail: QString,
        path: QString,
        /// What to ask before `enable` goes ahead; empty means ask nothing.
        confirm: QString,
        /// The crash marker to delete when `enable` is offered.
        marker: QString,
        open_file: bool,
        reload: bool,
        open_folder: bool,
    }

    /// The Languages page's bottom-strip toggle, for the selected row.
    /// Both its caption and whether it can be pressed are decided in Rust.
    struct FfiLanguageToggle {
        label: QString,
        enabled: bool,
        /// What to pass to `setDisabled` when pressed.
        disable: bool,
    }

    /// One entry of the Appearance page's icon-theme combo. The id is the
    /// contribution's, which is what `Settings::icon_theme` persists.
    struct FfiIconTheme {
        id: QString,
        label: QString,
    }

    /// Where a plugin came from — the Plugins page's grouping.
    enum FfiPluginSource {
        Builtin,
        Installed,
    }

    /// One row of the Plugins page (P7). Every word in it was chosen in
    /// `settings-model`; the view renders and never derives.
    struct FfiPluginRow {
        id: QString,
        name: QString,
        version: QString,
        description: QString,
        /// What this plugin adds, in words.
        contributes: QString,
        /// The status word, empty for a plugin that is working.
        status: QString,
        source: FfiPluginSource,
        severity: FfiRowSeverity,
    }

    /// The Plugins details pane: one failure, already turned into a
    /// sentence. A raw `LoadErrorKind` or wasm trap never crosses here —
    /// which is the point of the page, see ADR-0028.
    #[derive(Default)]
    struct FfiPluginProblem {
        sentence: QString,
        /// The specific detail, or empty when the sentence says everything.
        detail: QString,
        path: QString,
    }

    /// The Plugins page's bottom-strip toggle, for the selected row.
    struct FfiPluginToggle {
        label: QString,
        enabled: bool,
        /// What to pass to `setDisabled` when pressed.
        disable: bool,
    }

    /// The configuration half of a Language Servers row's status; the live
    /// half arrives on `LanguageService::serverStateChanged`.
    enum FfiServerRowStatus {
        NotConfigured,
        Disabled,
        Enabled,
    }

    /// One row of the Language Servers page (L6).
    struct FfiLanguageServerRow {
        language_id: QString,
        language_name: QString,
        command: QString,
        /// One space-separated line, not a list (see `settings_model::ServerRow`).
        args: QString,
        enabled: bool,
        status: FfiServerRowStatus,
    }

    extern "RustQt" {
        /// Settings > Syntax Colors (T4): the draft of the base and
        /// per-language colour tables the page edits.
        ///
        /// Stateful like `KeymapEditor` — Cancel must discard — but, unlike
        /// it, applied live: every mutation writes settings out so the open
        /// editors behind the dialog repaint, and `revert` puts the snapshot
        /// taken by `beginEdit` back. Every rule (precedence, what "From"
        /// says, which resets are no-ops) is `settings_model` and
        /// `syntax_core::theme`.
        #[qobject]
        type SyntaxColorEditor = super::SyntaxColorEditorRust;

        /// Take a snapshot of the saved tables and start a fresh draft.
        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &SyntaxColorEditor);

        /// Every language the registry knows, in catalog order — the combo
        /// below `(Base — all languages)`.
        #[qinvokable]
        fn languages(self: &SyntaxColorEditor) -> Vec<FfiLanguageOption>;

        /// Every scope row for one level: `languageId` empty selects the
        /// base table.
        #[qinvokable]
        fn scopes(self: &SyntaxColorEditor, language_id: &QString) -> Vec<FfiSyntaxScopeRow>;

        /// Set one scope's colour and flags at this level, and apply.
        /// An empty `hex` with no flags removes the entry.
        #[qinvokable]
        #[cxx_name = "setStyle"]
        fn set_style(
            self: &SyntaxColorEditor,
            language_id: &QString,
            scope: &QString,
            hex: &QString,
            bold: bool,
            italic: bool,
            underline: bool,
        );

        /// Remove this level's entry for one scope.
        #[qinvokable]
        #[cxx_name = "resetScope"]
        fn reset_scope(self: &SyntaxColorEditor, language_id: &QString, scope: &QString);

        /// Remove every entry at this level.
        #[qinvokable]
        #[cxx_name = "resetLevel"]
        fn reset_level(self: &SyntaxColorEditor, language_id: &QString);

        /// Whether `Reset Language...`/`Reset Base...` would change anything.
        #[qinvokable]
        #[cxx_name = "canResetLevel"]
        fn can_reset_level(self: &SyntaxColorEditor, language_id: &QString) -> bool;

        /// Discard the draft: put the snapshot back and apply it. The
        /// Cancel branch of the dialog.
        #[qinvokable]
        fn revert(self: &SyntaxColorEditor);

        /// One sentence naming any scope in `settings.toml` this build does
        /// not know, or empty when there is none — a hand-edited typo has
        /// no row to show itself in, so the page says it in words. The
        /// wording is `settings_model::unknown_scope_warning`.
        #[qinvokable]
        #[cxx_name = "unknownScopeWarning"]
        fn unknown_scope_warning(self: &SyntaxColorEditor) -> QString;
    }

    extern "RustQt" {
        /// Settings > Languages (G3): what loaded, where each language came
        /// from, and why anything that failed did.
        ///
        /// Read-mostly, and rescanned rather than watched: the page is open
        /// for seconds and a scan is a directory listing.
        #[qobject]
        type LanguageCatalog = super::LanguageCatalogRust;

        /// Rescan the config directory. Also what the `Reload languages`
        /// button calls.
        #[qinvokable]
        fn refresh(self: &LanguageCatalog);

        /// Every language, healthy or not, in catalog-then-overlay order.
        #[qinvokable]
        fn languages(self: &LanguageCatalog) -> Vec<FfiLanguageRow>;

        /// The details pane for one language. `sentence` is empty when that
        /// language has nothing to report, and the pane collapses.
        #[qinvokable]
        fn problem(self: &LanguageCatalog, id: &QString) -> FfiLanguageProblem;

        /// What the bottom strip's toggle says for `id`, and what pressing
        /// it does. An id nothing matches — no selection — comes back as a
        /// greyed `Disable Language`.
        #[qinvokable]
        fn toggle(self: &LanguageCatalog, id: &QString) -> FfiLanguageToggle;

        /// Turn one language off or back on: persist the choice, clear the
        /// crash marker if a quarantine is what turned it off, and rebuild
        /// the registry, so files already open stop (or start) resolving to
        /// it without a restart. The rows are refreshed too.
        #[qinvokable]
        #[cxx_name = "setDisabled"]
        fn set_disabled(self: &LanguageCatalog, id: &QString, disabled: bool) -> FfiResult;

        /// Copy a folder of tree-sitter queries into the config directory.
        #[qinvokable]
        #[cxx_name = "addLanguageFolder"]
        fn add_language_folder(self: &LanguageCatalog, path: &QString) -> FfiResult;

        /// Copy a compiled grammar library into the config directory, with
        /// the manifest that points at it.
        #[qinvokable]
        #[cxx_name = "addGrammarLibrary"]
        fn add_grammar_library(self: &LanguageCatalog, path: &QString) -> FfiResult;

        /// The directory languages are added to — shown so the user can
        /// find what the page is talking about.
        #[qinvokable]
        #[cxx_name = "languagesDir"]
        fn languages_dir(self: &LanguageCatalog) -> QString;
    }

    extern "RustQt" {
        /// Settings > Plugins (P7): what the host loaded, what it refused,
        /// and what the sandbox stopped.
        ///
        /// `LanguageCatalog`'s twin, and read-mostly for the same reason:
        /// the page is open for seconds and a scan is a directory listing.
        #[qobject]
        type PluginCatalog = super::PluginCatalogRust;

        /// Re-scan the plugins directory. The scan deliberately filters
        /// nothing — a plugin the user disabled still needs a row, or it
        /// could never be switched back on.
        #[qinvokable]
        fn refresh(self: &PluginCatalog);

        /// Every plugin, healthy or not, installed ones first.
        #[qinvokable]
        fn plugins(self: &PluginCatalog) -> Vec<FfiPluginRow>;

        /// The details pane for one plugin. `sentence` is empty when that
        /// plugin has nothing to report, and the pane collapses.
        #[qinvokable]
        fn problem(self: &PluginCatalog, id: &QString) -> FfiPluginProblem;

        /// What the bottom strip's toggle says for `id`, and what pressing
        /// it does. An id nothing matches — no selection — comes back as a
        /// greyed `Disable Plugin`.
        #[qinvokable]
        fn toggle(self: &PluginCatalog, id: &QString) -> FfiPluginToggle;

        /// Turn one plugin off or back on: persist the choice, re-scan, and
        /// restart the icon theme and the wasm tier over the result, so the
        /// change reaches the open window rather than waiting for a
        /// restart. The rows are refreshed too.
        #[qinvokable]
        #[cxx_name = "setDisabled"]
        fn set_disabled(self: &PluginCatalog, id: &QString, disabled: bool) -> FfiResult;

        /// The directory installed plugins are read from.
        #[qinvokable]
        #[cxx_name = "pluginsDir"]
        fn plugins_dir(self: &PluginCatalog) -> QString;
    }

    extern "RustQt" {
        /// Settings > Language Servers (L6): the draft of the
        /// `[[language_server]]` table, committed on OK.
        ///
        /// Draft-and-commit like `KeymapEditor`, and for a stronger reason:
        /// starting and stopping a server on every keystroke in a command
        /// field is not a preview.
        #[qobject]
        type LanguageServerEditor = super::LanguageServerEditorRust;

        /// Re-read the settings and build one row per language.
        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &LanguageServerEditor, scope: &QString);

        /// Every row, sorted by language name and stable while the page is
        /// open, so a live status change never moves one.
        #[qinvokable]
        fn rows(self: &LanguageServerEditor) -> Vec<FfiLanguageServerRow>;

        #[qinvokable]
        #[cxx_name = "setCommand"]
        fn set_command(self: &LanguageServerEditor, language_id: &QString, command: &QString);

        #[qinvokable]
        #[cxx_name = "setArgs"]
        fn set_args(self: &LanguageServerEditor, language_id: &QString, args: &QString);

        #[qinvokable]
        #[cxx_name = "setEnabled"]
        fn set_enabled(self: &LanguageServerEditor, language_id: &QString, enabled: bool);

        /// Whether the draft differs from what is saved — what makes
        /// `Restart Server` a no-op the page refuses rather than a restart
        /// of the command the user is halfway through replacing.
        #[qinvokable]
        #[cxx_name = "isDirty"]
        fn is_dirty(self: &LanguageServerEditor, language_id: &QString) -> bool;

        /// Write the draft to settings. The manager is reconciled
        /// separately, by `LanguageService::applyServerSettings`.
        #[qinvokable]
        fn commit(self: &LanguageServerEditor);
    }

    /// One editing-settings row — the global section, or one language's
    /// overrides — as the page edits it. `EditingSettings`'s `Option<T>`
    /// fields cross as a `has_*` flag plus the value, since cxx shared
    /// structs have no optional; the value is meaningless when its flag is
    /// false and the adapter never reads it that way.
    ///
    /// `default_encoding` and `line_endings` are carried for the global row
    /// only — a language may not override either (`settings-model`'s rule,
    /// not this struct's) — and are empty strings on every language row.
    #[derive(Default)]
    struct FfiEditingRow {
        /// Empty for the global row.
        language_id: QString,
        language_name: QString,
        /// `0` means unset.
        tab_width: u32,
        has_use_spaces: bool,
        use_spaces: bool,
        has_trim_trailing_whitespace: bool,
        trim_trailing_whitespace: bool,
        has_insert_final_newline: bool,
        insert_final_newline: bool,
        has_wrap_column: bool,
        wrap_column: u32,
        default_encoding: QString,
        line_endings: QString,
    }

    /// Something the Editing page must say out loud before it may commit.
    /// `language_id` is empty for the global section, which is what lets
    /// the page put the user back on the row that is wrong.
    #[derive(Default)]
    struct FfiEditingProblem {
        language_id: QString,
        sentence: QString,
    }

    extern "RustQt" {
        /// Settings > Editing (F1-14, F1-17): the draft of the `[editing]`
        /// section and its per-language overrides, committed on OK.
        ///
        /// Isomorphic to `LanguageServerEditor` — begin, edit, validate,
        /// commit — because a settings page with rules to refuse against is
        /// always this shape, not a special case of it.
        #[qobject]
        type EditingEditor = super::EditingEditorRust;

        /// Re-read the settings and start a fresh draft from them.
        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &EditingEditor, scope: &QString);

        /// The global section, as the page's top row.
        #[qinvokable]
        #[cxx_name = "globalRow"]
        fn global_row(self: &EditingEditor) -> FfiEditingRow;

        #[qinvokable]
        #[cxx_name = "setGlobalRow"]
        fn set_global_row(self: &EditingEditor, row: &FfiEditingRow);

        /// One row per language the editor knows about, in registry order —
        /// not just the ones with an override, so the page can offer every
        /// language and show which already differ.
        #[qinvokable]
        #[cxx_name = "languageRows"]
        fn language_rows(self: &EditingEditor) -> Vec<FfiEditingRow>;

        #[qinvokable]
        #[cxx_name = "setLanguageRow"]
        fn set_language_row(self: &EditingEditor, row: &FfiEditingRow);

        /// The tab width a buffer of `language_id` would resolve to if the
        /// draft were saved right now — what the preview column shows.
        #[qinvokable]
        #[cxx_name = "resolvedTabWidth"]
        fn resolved_tab_width(self: &EditingEditor, language_id: &QString) -> u32;

        /// Everything wrong with the draft, in the order the page should
        /// walk the user through it. Non-empty means `commit` will refuse.
        #[qinvokable]
        fn problems(self: &EditingEditor) -> Vec<FfiEditingProblem>;

        /// Write the draft to settings. Refuses with a typed code
        /// (ADR-0003) when `problems` is non-empty — a setting that parses
        /// and then does nothing is worse than one the dialog would not
        /// save.
        #[qinvokable]
        fn commit(self: &EditingEditor) -> FfiResult;
    }

    /// One turn as the transcript renders it. `text` is every text block of
    /// the turn joined; `kind` is `text`, `tool` or `error`, so the panel
    /// picks a bubble style without inspecting the text. `streaming` marks
    /// the one turn still being written into — `messages()` includes it, so
    /// the panel can show a bubble the moment the request is accepted.
    #[derive(Default)]
    struct FfiChatMessage {
        role: QString,
        text: QString,
        streaming: bool,
        kind: QString,
    }

    /// One pending context attachment, as its chip shows it. `tokens` is
    /// what this attachment alone costs, so the panel can say why the
    /// counter moved when it was added.
    #[derive(Default)]
    struct FfiAttachment {
        kind: QString,
        label: QString,
        detail: QString,
        tokens: u32,
    }

    /// One fenced code block of an answer. `path` is empty when the block
    /// named no file — which `prepareApply` refuses rather than guesses at
    /// (`ai_chat_core::proposal::ApplyRefusal::NoTarget`).
    #[derive(Default)]
    struct FfiCodeBlock {
        language: QString,
        path: QString,
        text: QString,
    }

    /// One provider as the chat's own picker lists it. Capabilities are
    /// *declared* by `ai_chat_core::providers` and carried here so the panel
    /// can grey out Agent mode or the image button, rather than sending a
    /// request that comes back 400 (ADR-0021 §2).
    #[derive(Default)]
    struct FfiAiProvider {
        id: QString,
        label: QString,
        model: QString,
        key_present: bool,
        active: bool,
        supports_tools: bool,
        supports_images: bool,
    }

    /// One model the active provider offers, as its catalogue reports it.
    ///
    /// *Discovered*, unlike the capabilities above: a model catalogue is
    /// what a vendor publishes and changes between releases of this IDE,
    /// so it is fetched rather than compiled in. `label` falls back to the
    /// id when the provider publishes no friendlier name, so it is never
    /// empty.
    #[derive(Default)]
    struct FfiAiModel {
        id: QString,
        label: QString,
    }

    /// One row of Settings > AI Providers. `status` is a finished sentence
    /// from `settings_model::ai::key_status`, rendered verbatim;
    /// `key_present` exists only so the page can pick a colour for it. The
    /// page never composes either (ADR-0002).
    #[derive(Default)]
    struct FfiAiProviderRow {
        id: QString,
        label: QString,
        kind: QString,
        base_url: QString,
        model: QString,
        key_env_var: QString,
        enabled: bool,
        key_present: bool,
        status: QString,
    }

    /// One row of the agent's tool-policy table. `policy` is the persisted
    /// spelling (`auto`/`ask`/`never`) and `writes` is
    /// `ai_chat_core::tools::ToolKind`, so the page groups reads apart from
    /// writes without an `if` in C++ deciding which is which.
    #[derive(Default)]
    struct FfiAiToolPolicyRow {
        tool: QString,
        policy: QString,
        writes: bool,
    }

    /// A tool call waiting on the user. `summary` is the sentence
    /// `ai_chat_core::tools::summarise` composed — the one the user actually
    /// consents to — and `arguments` is the raw JSON for the "show details"
    /// disclosure. An empty `call_id` means nothing is waiting.
    ///
    /// `needs_approval` is always true at this seam: `toolCallPending` is
    /// emitted only when the loop is genuinely blocked on a decision, since
    /// the panel disables the composer while a card is up and a card that
    /// needed no answer would wedge it.
    #[derive(Default)]
    struct FfiToolCall {
        call_id: QString,
        tool: QString,
        summary: QString,
        arguments: QString,
        needs_approval: bool,
    }

    /// What became of a tool call. `status` is `ok` or `error`; a call the
    /// user declined is `ok`, because a denial is data and not a failure
    /// (ADR-0021 §1).
    #[derive(Default)]
    struct FfiToolOutcome {
        call_id: QString,
        tool: QString,
        status: QString,
        detail: QString,
    }

    /// The composer's live counter. `exact` says which of the two kinds of
    /// number this is (`ai_chat_core::tokens::TokenCount`), so the panel can
    /// mark an estimate as an estimate rather than presenting a guess as a
    /// measurement (ADR-0021 §6).
    #[derive(Default)]
    struct FfiTokenUsage {
        context_tokens: u32,
        exact: bool,
        budget: u32,
        input_tokens: u32,
        output_tokens: u32,
    }

    /// One saved conversation, as the history sidebar lists it. `updated` is
    /// already formatted (`ai_chat_core::history::format_updated`).
    #[derive(Default)]
    struct FfiConversation {
        id: QString,
        title: QString,
        updated: QString,
        message_count: u32,
    }

    extern "RustQt" {
        /// The AI chat panel's FFI surface (ADR-0021): the transcript, the
        /// pending attachments, the streaming request, the agent loop's
        /// approval protocol, applying an answer, and the conversation
        /// store.
        ///
        /// Translation only, like every other QObject here: every rule —
        /// what may be attached, what a tool may do, when a run must stop,
        /// how a code block becomes an edit, what a failure means in
        /// English — lives in `ai-chat-core`, and every sentence crossing
        /// this seam was composed there (ADR-0002, ADR-0021 §6).
        #[qobject]
        type AiChat = super::AiChatRust;

        /// Send `text` with whatever is attached. Returns as soon as the
        /// request is queued: one `std::thread` owns the blocking HTTP and
        /// marshals every delta back with `CxxQtThread::queue`, so the Qt
        /// thread never waits on a provider (ADR-0021 §4).
        #[qinvokable]
        #[cxx_name = "sendMessage"]
        fn send_message(self: Pin<&mut AiChat>, text: &QString) -> FfiResult;

        /// Stop whatever is in flight — a stream, or a whole agent run,
        /// including one parked on an approval card.
        #[qinvokable]
        #[cxx_name = "cancelRequest"]
        fn cancel_request(self: Pin<&mut AiChat>);

        /// Drop the transcript and the attachments and start over. The
        /// conversation already saved to history is left on disk.
        #[qinvokable]
        #[cxx_name = "newConversation"]
        fn new_conversation(self: Pin<&mut AiChat>);

        #[qinvokable]
        #[cxx_name = "isStreaming"]
        fn is_streaming(self: &AiChat) -> bool;

        /// `"ask"` or `"agent"`. Agent mode against a provider that
        /// declares no tool support is refused here, with the provider
        /// named, rather than at the API (ADR-0021 §2).
        #[qinvokable]
        #[cxx_name = "setMode"]
        fn set_mode(self: Pin<&mut AiChat>, mode: &QString) -> FfiResult;

        #[qinvokable]
        fn mode(self: &AiChat) -> QString;

        /// What the user typed but has not sent, so the live counter can
        /// charge for it. Cheap to call per keystroke: the token counter
        /// memoises what it measured.
        #[qinvokable]
        #[cxx_name = "setComposerText"]
        fn set_composer_text(self: Pin<&mut AiChat>, text: &QString);

        /// Every attachment goes through `context::accept_attachment`,
        /// which is the single gate refusing a credentials-shaped file, a
        /// path outside the project, and an image a provider cannot read.
        #[qinvokable]
        #[cxx_name = "attachSelection"]
        fn attach_selection(
            self: Pin<&mut AiChat>,
            path: &QString,
            start_line: u32,
            end_line: u32,
            text: &QString,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "attachFile"]
        fn attach_file(self: Pin<&mut AiChat>, path: &QString) -> FfiResult;

        /// Refused when the active provider declares no image support, and
        /// refused again for a format no dialect reads — the second is a
        /// property of the file and switching provider cannot fix it.
        #[qinvokable]
        /// Attach every text file under a folder, as one `File`
        /// attachment each.
        ///
        /// Which files those are is `ai_chat_core::expand_folder`'s
        /// answer, not a walk written here: it honours `.gitignore`, skips
        /// binaries and secret-shaped names, and stops at the token budget.
        /// The result's message is its summary sentence, so the view says
        /// what was left out without composing the wording (ADR-0021 §11).
        #[cxx_name = "attachFolder"]
        fn attach_folder(self: Pin<&mut AiChat>, path: &QString) -> FfiResult;

        #[cxx_name = "attachImage"]
        fn attach_image(self: Pin<&mut AiChat>, path: &QString) -> FfiResult;

        /// The symbol's definition, resolved through the same project index
        /// the agent's `find_definitions` tool queries.
        #[qinvokable]
        #[cxx_name = "attachSymbol"]
        fn attach_symbol(self: Pin<&mut AiChat>, name: &QString) -> FfiResult;

        /// Everything the language servers currently report.
        #[qinvokable]
        #[cxx_name = "attachDiagnostics"]
        fn attach_diagnostics(self: Pin<&mut AiChat>) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "attachTerminalOutput"]
        fn attach_terminal_output(self: Pin<&mut AiChat>, text: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "removeAttachment"]
        fn remove_attachment(self: Pin<&mut AiChat>, index: u64);

        #[qinvokable]
        fn attachments(self: &AiChat) -> Vec<FfiAttachment>;

        /// The transcript, in-flight turn included.
        #[qinvokable]
        fn messages(self: &AiChat) -> Vec<FfiChatMessage>;

        /// The fenced blocks of one turn, in the order they appear — the
        /// index a per-block Apply button carries back to `prepareApply`.
        #[qinvokable]
        #[cxx_name = "codeBlocks"]
        fn code_blocks(self: &AiChat, message_index: u64) -> Vec<FfiCodeBlock>;

        #[qinvokable]
        #[cxx_name = "tokenUsage"]
        fn token_usage(self: &AiChat) -> FfiTokenUsage;

        #[qinvokable]
        fn providers(self: &AiChat) -> Vec<FfiAiProvider>;

        #[qinvokable]
        #[cxx_name = "setActiveProvider"]
        fn set_active_provider(self: Pin<&mut AiChat>, id: &QString) -> FfiResult;

        /// Re-read `settings.toml` after the settings dialog closed.
        #[qinvokable]
        #[cxx_name = "applyAiSettings"]
        fn apply_ai_settings(self: Pin<&mut AiChat>);

        // --- choosing a model ------------------------------------------

        /// The active provider's model catalogue as last fetched. Empty
        /// until `refreshModels` has answered, and empty for good when the
        /// provider lists none — the model stays typeable either way, so
        /// this list is a convenience and never a gate.
        #[qinvokable]
        fn models(self: &AiChat) -> Vec<FfiAiModel>;

        /// Ask the active provider what it offers. Returns immediately: the
        /// fetch runs on its own `std::thread` like every other blocking
        /// call here, and answers with `modelsChanged`.
        #[qinvokable]
        #[cxx_name = "refreshModels"]
        fn refresh_models(self: Pin<&mut AiChat>);

        /// A finished sentence about the last fetch, from
        /// `ai_chat_core::models::models_status`. The panel shows it; it
        /// does not write it.
        #[qinvokable]
        #[cxx_name = "modelsStatus"]
        fn models_status(self: &AiChat) -> QString;

        /// The model the next message goes to: this conversation's
        /// override, or the active provider's configured default.
        #[qinvokable]
        #[cxx_name = "currentModel"]
        fn current_model(self: &AiChat) -> QString;

        /// Run this conversation on `model`. An empty id puts it back on
        /// the provider's default. Per conversation, not per provider: the
        /// settings page owns the default, and this owns the exception.
        #[qinvokable]
        #[cxx_name = "setModel"]
        fn set_model(self: Pin<&mut AiChat>, model: &QString) -> FfiResult;

        // --- the agent loop's approval protocol ------------------------

        /// Let the waiting call run. `remember` promotes that tool to
        /// `Auto` for the rest of this run.
        #[qinvokable]
        #[cxx_name = "approveTool"]
        fn approve_tool(self: Pin<&mut AiChat>, call_id: &QString, remember: bool) -> FfiResult;

        /// Decline the waiting call. `reason` may be empty — the sentence
        /// the model is told is `ai-chat-core`'s either way, because it is
        /// model-facing wording and not the view's to compose.
        #[qinvokable]
        #[cxx_name = "denyTool"]
        fn deny_tool(self: Pin<&mut AiChat>, call_id: &QString, reason: &QString) -> FfiResult;

        /// The call waiting on a decision; an empty `call_id` means none.
        #[qinvokable]
        #[cxx_name = "pendingToolCall"]
        fn pending_tool_call(self: &AiChat) -> FfiToolCall;

        /// End the run without applying anything still pending. Unblocks a
        /// worker parked on an approval card, which is what stops closing
        /// the panel mid-approval from stranding the thread forever.
        #[qinvokable]
        #[cxx_name = "stopRun"]
        fn stop_run(self: Pin<&mut AiChat>);

        /// Round trips taken in the current (or last) run.
        #[qinvokable]
        #[cxx_name = "runStepCount"]
        fn run_step_count(self: &AiChat) -> u32;

        // --- applying an answer, mirroring LanguageService's protocol ---

        /// Plan the apply of one code block against the buffer whose text
        /// is `current_text`, at `buffer_revision`. The summary is empty
        /// (`document_count == 0`) when it was refused — `applyRefusal`
        /// then says why, in `ai-chat-core`'s words.
        #[qinvokable]
        #[cxx_name = "prepareApply"]
        fn prepare_apply(
            self: Pin<&mut AiChat>,
            message_index: u64,
            block_index: u64,
            current_text: &QString,
            buffer_revision: i64,
        ) -> FfiRefactorSummary;

        /// Every edit the pending apply would make, for the preview.
        #[qinvokable]
        #[cxx_name = "pendingEdits"]
        fn pending_edits(self: &AiChat) -> Vec<FfiTextEdit>;

        #[qinvokable]
        #[cxx_name = "excludeFromApply"]
        fn exclude_from_apply(self: Pin<&mut AiChat>, path: &QString);

        /// Take the edits to apply them. Empty when the buffer moved since
        /// `prepareApply` recorded its revision — the staleness rule is
        /// `lsp_core::EditGate`'s, exactly as for a rename (ADR-0021 §5).
        #[qinvokable]
        #[cxx_name = "takePendingEdits"]
        fn take_pending_edits(self: Pin<&mut AiChat>, buffer_revision: i64) -> Vec<FfiTextEdit>;

        #[qinvokable]
        #[cxx_name = "cancelApply"]
        fn cancel_apply(self: Pin<&mut AiChat>);

        /// Why the last `prepareApply` produced nothing. Code `0` means it
        /// did produce something. These codes are
        /// `ai_chat_core::proposal::ApplyRefusal`'s own space, not
        /// `ChatError`'s — the panel only reads them straight after a
        /// refused `prepareApply`, so the two never mix.
        #[qinvokable]
        #[cxx_name = "applyRefusal"]
        fn apply_refusal(self: &AiChat) -> FfiResult;

        // --- history ---------------------------------------------------

        #[qinvokable]
        fn conversations(self: &AiChat) -> Vec<FfiConversation>;

        #[qinvokable]
        #[cxx_name = "loadConversation"]
        fn load_conversation(self: Pin<&mut AiChat>, id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "deleteConversation"]
        fn delete_conversation(self: Pin<&mut AiChat>, id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "renameConversation"]
        fn rename_conversation(self: Pin<&mut AiChat>, id: &QString, title: &QString) -> FfiResult;

        /// Keep this conversation out of the store entirely, or put it back
        /// in. Persisted, so the choice survives a restart.
        #[qinvokable]
        #[cxx_name = "setPersistenceEnabled"]
        fn set_persistence_enabled(self: Pin<&mut AiChat>, enabled: bool);

        // --- signals ---------------------------------------------------

        /// The user's turn was appended at this index, so the panel can add
        /// one bubble instead of rebuilding the transcript.
        #[qsignal]
        #[cxx_name = "messageAppended"]
        fn message_appended(self: Pin<&mut AiChat>, index: u64);

        /// The assistant turn at this index exists and is streaming.
        #[qsignal]
        #[cxx_name = "messageStarted"]
        fn message_started(self: Pin<&mut AiChat>, index: u64);

        /// Append this text to that turn.
        #[qsignal]
        #[cxx_name = "deltaReceived"]
        fn delta_received(self: Pin<&mut AiChat>, index: u64, text: QString);

        /// That turn is complete; `codeBlocks(index)` is readable.
        #[qsignal]
        #[cxx_name = "messageFinished"]
        fn message_finished(self: Pin<&mut AiChat>, index: u64);

        /// The turn ended in an error. `code` is
        /// `ai_chat_core::ChatError`'s stable code — 12 is "the user
        /// pressed Stop", which the panel shows as nothing at all.
        #[qsignal]
        #[cxx_name = "chatFailed"]
        fn chat_failed(self: Pin<&mut AiChat>, error: FfiResult);

        #[qsignal]
        #[cxx_name = "attachmentsChanged"]
        fn attachments_changed(self: Pin<&mut AiChat>);

        #[qsignal]
        #[cxx_name = "providersChanged"]
        fn providers_changed(self: Pin<&mut AiChat>);

        /// The model catalogue or the chosen model changed — re-read
        /// `models()`, `modelsStatus()` and `currentModel()`.
        #[qsignal]
        #[cxx_name = "modelsChanged"]
        fn models_changed(self: Pin<&mut AiChat>);

        #[qsignal]
        #[cxx_name = "tokenUsageChanged"]
        fn token_usage_changed(self: Pin<&mut AiChat>);

        /// Show the approval card: the run is blocked until `approveTool`,
        /// `denyTool` or `stopRun` answers it.
        #[qsignal]
        #[cxx_name = "toolCallPending"]
        fn tool_call_pending(self: Pin<&mut AiChat>, call: FfiToolCall);

        #[qsignal]
        #[cxx_name = "toolCallFinished"]
        fn tool_call_finished(self: Pin<&mut AiChat>, outcome: FfiToolOutcome);

        /// The agent loop ended; code `0` means it ended on an answer.
        #[qsignal]
        #[cxx_name = "runFinished"]
        fn run_finished(self: Pin<&mut AiChat>, result: FfiResult);

        #[qsignal]
        #[cxx_name = "conversationsChanged"]
        fn conversations_changed(self: Pin<&mut AiChat>);

        /// A tool opened a tab. Relayed by `main_window.cpp` to the same
        /// handler `DocumentManager::tabOpened` drives.
        ///
        /// These three exist because a tool runs against the shared
        /// `AppSession` from *this* QObject, and only `DocumentManager` can
        /// emit its own signals — without them an agent's edit would change
        /// the `Document` while the widget on screen kept the old text.
        #[qsignal]
        #[cxx_name = "toolOpenedTab"]
        fn tool_opened_tab(self: Pin<&mut AiChat>, tab_id: u64, title: QString);

        /// A tool replaced a buffer's text; same handler as
        /// `DocumentManager::bufferEditedExternally`.
        #[qsignal]
        #[cxx_name = "toolEditedBuffer"]
        fn tool_edited_buffer(self: Pin<&mut AiChat>, tab_id: u64, content: QString);

        /// A tool wrote a buffer to disk; same handler as
        /// `DocumentManager::tabModifiedChanged(id, false)`.
        #[qsignal]
        #[cxx_name = "toolSavedBuffer"]
        fn tool_saved_buffer(self: Pin<&mut AiChat>, tab_id: u64);
    }

    // The streaming thread's one cross-thread hop, same pattern as
    // `TerminalSession`'s PTY reader and `LanguageService`'s LSP listener.
    impl cxx_qt::Threading for AiChat {}

    extern "RustQt" {
        /// Settings > AI Providers (AC14): the draft of the
        /// `[[ai_provider]]` and `[[ai_tool_policy]]` tables, committed on
        /// OK. Isomorphic to `LanguageServerEditor`, and draft-and-commit
        /// for the same reason: a half-typed base URL must not become the
        /// endpoint a request is sent to.
        #[qobject]
        type AiProviderEditor = super::AiProviderEditorRust;

        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &AiProviderEditor);

        #[qinvokable]
        fn rows(self: &AiProviderEditor) -> Vec<FfiAiProviderRow>;

        /// The tool-policy table, reads first, in
        /// `settings_model::ai::known_tools` order.
        #[qinvokable]
        #[cxx_name = "toolPolicies"]
        fn tool_policies(self: &AiProviderEditor) -> Vec<FfiAiToolPolicyRow>;

        #[qinvokable]
        #[cxx_name = "setBaseUrl"]
        fn set_base_url(self: &AiProviderEditor, id: &QString, base_url: &QString);

        #[qinvokable]
        #[cxx_name = "setModel"]
        fn set_model(self: &AiProviderEditor, id: &QString, model: &QString);

        #[qinvokable]
        #[cxx_name = "setKeyEnvVar"]
        fn set_key_env_var(self: &AiProviderEditor, id: &QString, key_env_var: &QString);

        #[qinvokable]
        #[cxx_name = "setEnabled"]
        fn set_enabled(self: &AiProviderEditor, id: &QString, enabled: bool);

        /// `auto`, `ask` or `never`. An unrecognised spelling is ignored
        /// rather than widening the agent's authority on a typo.
        #[qinvokable]
        #[cxx_name = "setToolPolicy"]
        fn set_tool_policy(self: &AiProviderEditor, tool: &QString, policy: &QString);

        #[qinvokable]
        #[cxx_name = "isDirty"]
        fn is_dirty(self: &AiProviderEditor, id: &QString) -> bool;

        /// The first problem that would stop the dialog closing, as the
        /// finished sentence `settings_model::ai::validate` composed. Code
        /// `0` means the page is savable.
        #[qinvokable]
        fn validate(self: &AiProviderEditor) -> FfiResult;

        /// Write the draft to `settings.toml`.
        #[qinvokable]
        fn commit(self: &AiProviderEditor) -> FfiResult;

        #[qinvokable]
        fn revert(self: &AiProviderEditor);

        // --- the model catalogue, per row ------------------------------

        /// Ask the row's endpoint what models it offers, using the *draft*
        /// values: a base URL the user has just typed is the one that gets
        /// asked, which is what makes pointing a local runtime somewhere
        /// and picking its model one gesture instead of two dialogs.
        ///
        /// Returns immediately; answers with `modelsChanged`.
        #[qinvokable]
        #[cxx_name = "fetchModels"]
        fn fetch_models(self: Pin<&mut AiProviderEditor>, id: &QString);

        /// What that row's endpoint last reported. Empty until fetched, and
        /// never a gate: the Model cell stays typeable.
        #[qinvokable]
        fn models(self: &AiProviderEditor, id: &QString) -> Vec<FfiAiModel>;

        /// A finished sentence about that row's last fetch, from
        /// `ai_chat_core::models::models_status`.
        #[qinvokable]
        #[cxx_name = "modelsStatus"]
        fn models_status(self: &AiProviderEditor, id: &QString) -> QString;

        /// That row's catalogue changed — re-read `models(id)`.
        #[qsignal]
        #[cxx_name = "modelsChanged"]
        fn models_changed(self: Pin<&mut AiProviderEditor>, id: QString);
    }

    // The catalogue fetch's one cross-thread hop; blocking HTTP must not
    // run on the thread painting the dialog.
    impl cxx_qt::Threading for AiProviderEditor {}

    extern "RustQt" {
        /// Git v1 (F3-12): owns one `vcs_core::Repository` (on a worker
        /// thread — a repository handle plus a `git` subprocess call must
        /// not run on the UI thread) plus the caches `vcs-core` already
        /// built for hunks, history and blame. The two-thread shape is
        /// `LanguageService`'s (ADR-0004, ADR-0007): a job-queue `Sender`
        /// consumed by a worker that owns the handle, shutdown by dropping
        /// it. Translation only, per `docs/architecture/layering.md`: every
        /// rule about what a hunk, a status or a branch *is* lives in
        /// `vcs-core`.
        #[qobject]
        type VcsService = super::VcsServiceRust;

        /// Point at a project root: discovers (or re-discovers) the
        /// repository on the worker thread and drops whatever the previous
        /// project's worker was doing. `isRepository()` reads `false` until
        /// discovery answers — same asynchronous-readiness shape
        /// `LanguageService::openProject` already has.
        #[qinvokable]
        #[cxx_name = "openProject"]
        fn open_project(self: Pin<&mut VcsService>, root_path: &QString);

        /// Whether the current project root is (or is under) a Git
        /// repository. `false` before `openProject` answers and for a plain
        /// folder — `vcs_core::DiscoverResult::NotARepository` is an
        /// ordinary outcome, not a failure.
        #[qinvokable]
        #[cxx_name = "isRepository"]
        fn is_repository(self: &VcsService) -> bool;

        /// `git config --global --add safe.directory <path>` for the
        /// current project root, the fix for a `VcsError::DubiousOwnership`
        /// failure (code 710) — offered by the "Trust This Folder" button
        /// on that dialog. Re-runs `openProject` on success.
        #[qinvokable]
        #[cxx_name = "trustDirectory"]
        fn trust_directory(self: Pin<&mut VcsService>) -> FfiResult;

        /// `git init` in the current project root, then re-runs
        /// `openProject` so discovery finds the repository just created.
        /// What the Changes dock's "Initialize Git Repository" button
        /// calls.
        #[qinvokable]
        #[cxx_name = "initRepository"]
        fn init_repository(self: Pin<&mut VcsService>) -> FfiResult;

        /// Whether this machine already declined to initialize a Git
        /// repository for the current project root — drives which of the
        /// two Changes-dock empty-state wordings is shown.
        #[qinvokable]
        #[cxx_name = "declinedGitInit"]
        fn declined_git_init(self: &VcsService) -> bool;

        /// Record this machine's answer to the "Initialize Git Repository" /
        /// "Not now" choice for the current project root.
        #[qinvokable]
        #[cxx_name = "setDeclinedGitInit"]
        fn set_declined_git_init(self: &VcsService, declined: bool);

        /// Re-read `HEAD`/index/worktree status on the worker thread;
        /// answers via `statusChanged`.
        #[qinvokable]
        #[cxx_name = "refreshStatus"]
        fn refresh_status(self: Pin<&mut VcsService>);

        /// The status `refreshStatus` last found: staged, unstaged and
        /// untracked paths in one list.
        #[qinvokable]
        #[cxx_name = "changedFiles"]
        fn changed_files(self: &VcsService) -> Vec<FfiChangedFile>;

        /// Ask for `path`'s hunks against `HEAD`, diffed against
        /// `workingText` (the live buffer) and cached by `revision` (a
        /// caller-bumped counter, `HunkCache`'s cache key) — answers via
        /// `hunksChanged(path)`.
        #[qinvokable]
        #[cxx_name = "requestHunks"]
        fn request_hunks(
            self: Pin<&mut VcsService>,
            path: &QString,
            working_text: &QString,
            revision: i64,
        );

        /// The hunks the last `requestHunks` for `path` found. Empty before
        /// an answer arrives or when `path` has never been asked about.
        #[qinvokable]
        fn hunks(self: &VcsService, path: &QString) -> Vec<FfiHunk>;

        /// The `HEAD` text `requestHunks` last cached for `path` — what
        /// `DiffView`'s left pane needs for the gutter popup's "Show Diff"
        /// (F3-16), with no second repository read. Empty before an answer
        /// arrives.
        #[qinvokable]
        #[cxx_name = "headText"]
        fn head_text(self: &VcsService, path: &QString) -> QString;

        /// The edit that reverts `hunks(path)[hunk_index]`, to splice into
        /// the open buffer through `EditorTabs::applyBufferEdits` — never a
        /// write to disk (F3-11/ADR-0031). Computed from the same cached
        /// `HEAD` text `requestHunks` already read, so this needs no worker
        /// round trip. Empty when `path` or `hunk_index` names nothing
        /// cached.
        #[qinvokable]
        #[cxx_name = "revertHunk"]
        fn revert_hunk(self: &VcsService, path: &QString, hunk_index: u32) -> Vec<FfiTextEdit>;

        /// `git add <path>` on the worker thread; `statusChanged` follows on
        /// success, `vcsFailed` on failure.
        #[qinvokable]
        #[cxx_name = "stageFile"]
        fn stage_file(self: Pin<&mut VcsService>, path: &QString);

        /// `git reset -- <path>`; the whole-file inverse of `stageFile`,
        /// used by the Changes dock's per-file checkbox in the staged tree.
        #[qinvokable]
        #[cxx_name = "unstageFile"]
        fn unstage_file(self: Pin<&mut VcsService>, path: &QString);

        /// Stage `hunks(path)[hunk_index]` via a generated patch
        /// (`vcs_core::stage_hunk`), from the same cached before/working
        /// text `requestHunks` last read for `path`. No-op if nothing is
        /// cached for `path`.
        #[qinvokable]
        #[cxx_name = "stageHunk"]
        fn stage_hunk(self: Pin<&mut VcsService>, path: &QString, hunk_index: u32);

        /// The inverse of `stageHunk`.
        #[qinvokable]
        #[cxx_name = "unstageHunk"]
        fn unstage_hunk(self: Pin<&mut VcsService>, path: &QString, hunk_index: u32);

        /// `git commit -m <message> [--amend]`, exactly what is staged.
        #[qinvokable]
        fn commit(self: Pin<&mut VcsService>, message: &QString, amend: bool);

        /// Re-list local branches on the worker thread (`gix`, no
        /// subprocess); answers via `branchChanged`.
        #[qinvokable]
        #[cxx_name = "refreshBranches"]
        fn refresh_branches(self: Pin<&mut VcsService>);

        /// The branch names the last `refreshBranches` found, sorted.
        #[qinvokable]
        fn branches(self: &VcsService) -> Vec<FfiBranch>;

        /// The checked-out branch's name, as of the last `refreshBranches` —
        /// empty for a detached or unborn `HEAD`, or before an answer
        /// arrives.
        #[qinvokable]
        #[cxx_name = "currentBranch"]
        fn current_branch(self: &VcsService) -> QString;

        /// `git checkout <name>`; `branchChanged` and `statusChanged` follow
        /// on success.
        #[qinvokable]
        fn checkout(self: Pin<&mut VcsService>, name: &QString);

        /// `git branch <name> [<start_point>]`; empty `start_point` means
        /// none.
        #[qinvokable]
        #[cxx_name = "createBranch"]
        fn create_branch(self: Pin<&mut VcsService>, name: &QString, start_point: &QString);

        /// `git branch -d`, or `-D` if `force`. An unmerged branch refused
        /// without `force` surfaces via `vcsFailed`
        /// (`VcsError::UnmergedBranch`'s code), for a caller to offer a
        /// deliberate forced retry.
        #[qinvokable]
        #[cxx_name = "deleteBranch"]
        fn delete_branch(self: Pin<&mut VcsService>, name: &QString, force: bool);

        /// `git fetch <remote>`.
        #[qinvokable]
        fn fetch(self: Pin<&mut VcsService>, remote: &QString);

        /// `git pull <remote> <branch>`.
        #[qinvokable]
        fn pull(self: Pin<&mut VcsService>, remote: &QString, branch: &QString);

        /// `git push [-u] <remote> <branch>`.
        #[qinvokable]
        fn push(self: Pin<&mut VcsService>, remote: &QString, branch: &QString, set_upstream: bool);

        /// Ask for `path`'s blob at `revision` (a commit id, tag, branch
        /// name, or `"HEAD"`) — File History's "compare with revision"
        /// (F3-14), which needs a blob from *some* commit, not just `HEAD`
        /// the way `requestHunks`/`headText` do. Answers via `blobReady`.
        #[qinvokable]
        #[cxx_name = "requestBlobAt"]
        fn request_blob_at(self: Pin<&mut VcsService>, path: &QString, revision: &QString);

        /// The blob `requestBlobAt(path, revision)` last found, or empty
        /// before an answer arrives, when the path has no version at that
        /// revision, or when `revision` doesn't resolve.
        #[qinvokable]
        #[cxx_name = "blobAt"]
        fn blob_at(self: &VcsService, path: &QString, revision: &QString) -> QString;

        /// Commits that touched `path`, newest first; answers via
        /// `historyReady`.
        #[qinvokable]
        #[cxx_name = "fileHistory"]
        fn file_history(self: Pin<&mut VcsService>, path: &QString);

        /// `git blame --porcelain -- <path>`, parsed; answers via
        /// `blameReady`.
        #[qinvokable]
        fn blame(self: Pin<&mut VcsService>, path: &QString);

        /// Discovery finished, or a later `openProject` retargeted the
        /// repository: `isRepository()` has a fresh answer.
        #[qsignal]
        #[cxx_name = "repositoryChanged"]
        fn repository_changed(self: Pin<&mut VcsService>);

        /// `changedFiles()` has a fresh answer.
        #[qsignal]
        #[cxx_name = "statusChanged"]
        fn status_changed(self: Pin<&mut VcsService>);

        /// `hunks(path)` has a fresh answer for this path.
        #[qsignal]
        #[cxx_name = "hunksChanged"]
        fn hunks_changed(self: Pin<&mut VcsService>, path: QString);

        /// `branches()` has a fresh answer, or the checked-out branch
        /// changed.
        #[qsignal]
        #[cxx_name = "branchChanged"]
        fn branch_changed(self: Pin<&mut VcsService>);

        /// `blobAt(path, revision)` has a fresh answer for this
        /// `(path, revision)` pair.
        #[qsignal]
        #[cxx_name = "blobReady"]
        fn blob_ready(self: Pin<&mut VcsService>, path: QString, revision: QString);

        /// A `vcs-core` operation failed — the code/message pair to show
        /// verbatim (ADR-0003).
        #[qsignal]
        #[cxx_name = "vcsFailed"]
        fn vcs_failed(self: Pin<&mut VcsService>, error: FfiResult);

        /// `fileHistory`'s answer, tagged with the path it was requested
        /// for — `fileHistory`/`blame` carry no request id, so a caller
        /// that fired two requests in a row (e.g. the active editor tab
        /// changed mid-flight) needs this to tell which file a given
        /// answer belongs to, and drop a stale one.
        #[qsignal]
        #[cxx_name = "historyReady"]
        fn history_ready(self: Pin<&mut VcsService>, path: QString, entries: Vec<FfiLogEntry>);

        /// `blame`'s answer, tagged with the path it was requested for —
        /// see `historyReady` on why.
        #[qsignal]
        #[cxx_name = "blameReady"]
        fn blame_ready(self: Pin<&mut VcsService>, path: QString, lines: Vec<FfiBlameLine>);

        /// `fileHistory(path)` could not even be queued — no worker exists
        /// yet because the project is not a Git repository, or discovery is
        /// still running. Tagged with the path the same way `historyReady`
        /// is, so a caller can tell "not version-controlled" apart from "no
        /// commits yet" (an empty `historyReady` answer) instead of the
        /// panel sitting on a stale or empty list forever.
        #[qsignal]
        #[cxx_name = "historyUnavailable"]
        fn history_unavailable(self: Pin<&mut VcsService>, path: QString);
    }

    // Enables `self.qt_thread()` on `VcsService` for its worker thread,
    // mirroring `LanguageService`'s listener (ADR-0004).
    impl cxx_qt::Threading for VcsService {}

    extern "RustQt" {
        /// Run configurations and console (F4-9): owns one
        /// `run_core::Supervisor` on a worker thread — spawning a process,
        /// killing its tree, and a blocking PTY read must not run on the UI
        /// thread — plus one dedicated reader thread per active console
        /// feeding output back through the same job queue. `lsp-core`'s
        /// supervised-child shape, extended with the per-console reader
        /// thread `docs/architecture/next-five-features-plan.md`'s
        /// threading table calls for (see `crate::bridge::run`'s module doc
        /// for the concurrency argument). Translation only, per
        /// `docs/architecture/layering.md`: what a run configuration is, how
        /// one launches, and what a line of output links to all live in
        /// `run-core`.
        #[qobject]
        type RunService = super::RunServiceRust;

        /// The current project's run configurations — whatever
        /// `.ide/settings.toml` has, already merged with the last
        /// `detectConfigurations()` scan (F4-4/F4-5's merge rule: a
        /// user-edited configuration is never silently overwritten). Empty
        /// with no project open.
        #[qinvokable]
        fn configurations(self: &RunService) -> Vec<FfiRunConfig>;

        /// Scan the project (`Cargo.toml`, `package.json`, `Makefile`) for
        /// launchable targets on the worker thread, merge the result into
        /// the persisted list and save it; answers via
        /// `configurationsChanged`.
        #[qinvokable]
        #[cxx_name = "detectConfigurations"]
        fn detect_configurations(self: Pin<&mut RunService>);

        /// Launch `config_id`'s program in a fresh console on the worker
        /// thread. A resolvable configuration always answers via
        /// `consoleStarted`; an unresolvable one (no project open, unknown
        /// id) is reported here rather than as a silent no-op. A spawn
        /// failure inside `run-core` (bad `program`, missing `cwd`) has no
        /// console to attach to, so it answers via `runFailed` instead.
        #[qinvokable]
        fn run(self: Pin<&mut RunService>, config_id: &QString) -> FfiResult;

        /// Whether running `path` from the editor would launch anything —
        /// what decides if the gutter shows a Run icon on that file (R1-6).
        /// The rule is `run_core::context::config_for_file`'s, so the view
        /// asks rather than deciding which files look runnable.
        #[qinvokable]
        #[cxx_name = "canRunFile"]
        fn can_run_file(self: &RunService, path: &QString) -> bool;

        /// Run `path` the way IntelliJ's gutter Run does: build the
        /// configuration the file implies, remember it as a temporary one
        /// (evicting the oldest past `run_core::TEMPORARY_CAP`), and launch
        /// it. Answers via `configurationsChanged` and then the usual
        /// `consoleStarted`; a file with no run target reports
        /// `CODE_UNKNOWN_RUN_CONFIG` here rather than doing nothing.
        #[qinvokable]
        #[cxx_name = "runContext"]
        fn run_context(self: Pin<&mut RunService>, path: &QString) -> FfiResult;

        /// Stop `console_id`: `kill_tree()`s its process on the worker
        /// thread, flushes whatever output was still pending, and answers
        /// via `consoleFinished` with `escaped = true` if
        /// `KillOutcome::Escaped` was reported (a double-forked descendant
        /// this build could not reach) — never conflated with a clean kill.
        #[qinvokable]
        fn stop(self: Pin<&mut RunService>, console_id: u64);

        /// Kill a console outright, skipping the grace period `stop` gives
        /// it (R2-4) — IntelliJ's Kill next to its Stop.
        #[qinvokable]
        #[cxx_name = "kill"]
        fn kill(self: Pin<&mut RunService>, console_id: u64);

        /// Stop `console_id` if still running, then launch its configuration
        /// again. `console_id` must be one `consoleStarted` reported.
        #[qinvokable]
        fn rerun(self: Pin<&mut RunService>, console_id: u64) -> FfiResult;

        /// The `file:line[:col]` (or Python `File "...", line N`) location
        /// covering `byte_offset` in `console_id`'s accumulated output, for
        /// hover feedback and Ctrl+Click — the same
        /// `run_core::links::resolve_link` catalogue `TerminalSession` uses
        /// for terminal output.
        #[qinvokable]
        #[cxx_name = "resolveLink"]
        fn resolve_link(self: &RunService, console_id: u64, byte_offset: u32) -> FfiResolvedLink;

        /// How the text of this console's most recent `consoleOutput`
        /// signal is styled (R2-1).
        ///
        /// Pulled by the slot rather than pushed as a signal parameter: a
        /// `Vec<T>` is not a Qt metatype, so it cannot ride on a signal —
        /// the same reason `TerminalSupervisor::gridCells` is a getter
        /// beside its `gridChanged` signal. Sequential and race-free for
        /// the same reason that one is: both the signal and this call run
        /// on the Qt thread, in the order the worker queued them.
        #[qinvokable]
        #[cxx_name = "consoleStyleRuns"]
        fn console_style_runs(self: &RunService, console_id: u64) -> Vec<FfiStyledRun>;

        /// Every match of `pattern` in a console's text, in UTF-16 units
        /// (R2-3). Literal, never a regex: a console find bar is a "where
        /// did that word go" affordance, and `editor_core::search` is the
        /// matcher either way.
        #[qinvokable]
        #[cxx_name = "findInConsole"]
        fn find_in_console(
            self: &RunService,
            console_id: u64,
            pattern: &QString,
            case_sensitive: bool,
        ) -> Vec<FfiTextMatch>;

        /// Forget a console's scrollback (R2-3). The view clears its
        /// document in the same gesture; both must forget together, or the
        /// offsets `resolveLink` answers with stop meaning anything.
        #[qinvokable]
        #[cxx_name = "clearConsole"]
        fn clear_console(self: &RunService, console_id: u64);

        /// Drop a finished console's scrollback when its tab closes
        /// (R2-3). A running console is left alone — see the Rust side.
        #[qinvokable]
        #[cxx_name = "closeConsole"]
        fn close_console(self: &RunService, console_id: u64);

        /// The consoles this session has, running ones first (R2-5).
        #[qinvokable]
        #[cxx_name = "activeConsoles"]
        fn active_consoles(self: &RunService) -> Vec<FfiRunningConsole>;

        /// `configurations()` has a fresh answer.
        #[qsignal]
        #[cxx_name = "configurationsChanged"]
        fn configurations_changed(self: Pin<&mut RunService>);

        /// A console was launched and is ready to receive `consoleOutput`.
        #[qsignal]
        #[cxx_name = "consoleStarted"]
        fn console_started(self: Pin<&mut RunService>, console_id: u64, config_id: QString);

        /// A batch of output — never one event per PTY `read()`, that is
        /// the whole point of F4-7's batcher.
        #[qsignal]
        #[cxx_name = "consoleOutput"]
        fn console_output(self: Pin<&mut RunService>, console_id: u64, text: QString);

        /// This console's cache dropped `utf16_units` code units off its
        /// front, and the view must drop exactly as many so its document
        /// stays the text the offsets are measured against (R2-3).
        #[qsignal]
        #[cxx_name = "consoleTrimmed"]
        fn console_trimmed(self: Pin<&mut RunService>, console_id: u64, utf16_units: u32);

        /// `console_id` exited (on its own, or via `stop`/`rerun`).
        /// `exit_code` is `-1` when it could not be determined (an explicit
        /// stop does not wait for one). `escaped` is `stop`'s
        /// `KillOutcome::Escaped` case, reported honestly rather than as a
        /// clean kill.
        #[qsignal]
        #[cxx_name = "consoleFinished"]
        fn console_finished(
            self: Pin<&mut RunService>,
            console_id: u64,
            exit_code: i32,
            escaped: bool,
        );

        /// `run(config_id)` could not start anything at all — no console
        /// was opened for it, so this is the only signal a caller gets.
        #[qsignal]
        #[cxx_name = "runFailed"]
        fn run_failed(self: Pin<&mut RunService>, config_id: QString, error: FfiResult);

        /// A before-launch task started (B2-2). `label` is what it is —
        /// "Build", another configuration's name, an external tool's
        /// program — for the Build dock's header.
        #[qsignal]
        #[cxx_name = "beforeLaunchStarted"]
        fn before_launch_started(self: Pin<&mut RunService>, config_id: QString, label: QString);

        /// A chunk of a before-launch task's output, ANSI already stripped.
        #[qsignal]
        #[cxx_name = "beforeLaunchOutput"]
        fn before_launch_output(self: Pin<&mut RunService>, config_id: QString, text: QString);

        /// A before-launch task refused or failed, so the configuration was
        /// never launched. The only signal that run gets: no console was
        /// opened for it.
        #[qsignal]
        #[cxx_name = "beforeLaunchFailed"]
        fn before_launch_failed(self: Pin<&mut RunService>, config_id: QString, error: FfiResult);
    }

    // Enables `self.qt_thread()` on `RunService` for its worker thread and
    // the per-console reader threads it spawns.
    impl cxx_qt::Threading for RunService {}

    extern "RustQt" {
        /// Building the project (B1-6): runs the project's own build tool
        /// on a thread of its own and publishes what it said — output for
        /// the Build dock, diagnostics for the Problems dock.
        ///
        /// Translation only, per `docs/architecture/layering.md`: which
        /// steps a build runs, and what a line of its output means, are
        /// `build-core`'s (ADR-0040).
        #[qobject]
        type BuildService = super::BuildServiceRust;

        /// Build the project with whichever toolchain it uses. Answers via
        /// `buildStarted` and then `buildFinished`; a project with nothing
        /// to build is reported here rather than as a silent no-op.
        #[qinvokable]
        fn build(self: Pin<&mut BuildService>) -> FfiResult;

        /// The tool's own clean, then its build. Refused for a toolchain
        /// with no clean step rather than doing half of it.
        #[qinvokable]
        fn rebuild(self: Pin<&mut BuildService>) -> FfiResult;

        /// Build one named target, spelled the way this toolchain spells
        /// one. A toolchain with no spelling for a target builds everything.
        #[qinvokable]
        #[cxx_name = "buildTarget"]
        fn build_target(self: Pin<&mut BuildService>, target: &QString) -> FfiResult;

        /// Kill `build_id`'s process tree. Not its direct child alone:
        /// `cargo` spawns `rustc` and `gradle` spawns a daemon.
        #[qinvokable]
        fn stop(self: Pin<&mut BuildService>, build_id: u64);

        /// Whether any build is running — what the toolbar's Build/Stop
        /// enablement asks, rather than the view tracking it from signals.
        #[qinvokable]
        #[cxx_name = "isBuilding"]
        fn is_building(self: &BuildService) -> bool;

        /// What the last build said, in the shape the Problems dock already
        /// renders for a language server's diagnostics; `source` names the
        /// build tool, so the two are never confused. Cleared when a new
        /// build starts.
        #[qinvokable]
        fn diagnostics(self: &BuildService) -> Vec<FfiDiagnostic>;

        /// A build started. `command` is what is being run, for the dock's
        /// header.
        #[qsignal]
        #[cxx_name = "buildStarted"]
        fn build_started(self: Pin<&mut BuildService>, build_id: u64, command: QString);

        /// A chunk of the build's output, ANSI already stripped.
        #[qsignal]
        #[cxx_name = "buildOutput"]
        fn build_output(self: Pin<&mut BuildService>, build_id: u64, text: QString);

        /// The build ended. `exit_code` is the failing step's, or `-1` when
        /// a step could not be started at all.
        #[qsignal]
        #[cxx_name = "buildFinished"]
        fn build_finished(self: Pin<&mut BuildService>, build_id: u64, exit_code: i32);

        /// `diagnostics()` has a fresh answer — emitted while the build is
        /// still running, so the Problems dock fills as it goes.
        #[qsignal]
        #[cxx_name = "diagnosticsChanged"]
        fn diagnostics_changed(self: Pin<&mut BuildService>);
    }

    // Enables `self.qt_thread()` on `BuildService` for the thread each build
    // runs on.
    impl cxx_qt::Threading for BuildService {}

    extern "RustQt" {
        /// Debugging (D3-1): owns the breakpoints and whatever debug
        /// sessions are running, and speaks DAP to each session's adapter.
        ///
        /// One QObject for N sessions, the ADR-0032 precedent. Translation
        /// only: what a breakpoint is, which adapter a project uses and what
        /// a launch body looks like are `dap-core`'s (ADR-0041).
        #[qobject]
        type DebugService = super::DebugServiceRust;

        /// Start debugging `config_id` — the same configuration Run would
        /// launch, started by the adapter instead. Answers via
        /// `debugStarted` and then `debugStopped`/`debugTerminated`; a
        /// missing adapter is reported here, with its install hint.
        #[qinvokable]
        fn debug(self: Pin<&mut DebugService>, config_id: &QString) -> FfiResult;

        /// Attach to a process that is already running (D4-1). The pid is
        /// the user's: this IDE does not enumerate processes, because doing
        /// it portably is three implementations and a permissions story for
        /// a number the user already knows.
        #[qinvokable]
        fn attach(self: Pin<&mut DebugService>, pid: u32) -> FfiResult;

        /// Attach to a debuggee already running elsewhere (D4-2). The
        /// target is remembered in the project's settings, path mappings
        /// included — see the Rust side for why those live only there.
        #[qinvokable]
        #[cxx_name = "attachRemote"]
        fn attach_remote(self: Pin<&mut DebugService>, host: &QString, port: u32) -> FfiResult;

        /// `host:port` of the last remote target this project attached to,
        /// empty if it never has — what the dialog offers instead of an
        /// empty field.
        #[qinvokable]
        #[cxx_name = "lastRemoteTarget"]
        fn last_remote_target(self: &DebugService) -> QString;

        /// The exception filters this session's adapter offers, as
        /// `id\tlabel\tenabled` lines. Per adapter, because which
        /// exceptions can be broken on is something only the adapter knows.
        #[qinvokable]
        #[cxx_name = "exceptionFilters"]
        fn exception_filters(self: &DebugService, session_id: u64) -> QString;

        /// Break on this class of exception, or stop doing so.
        #[qinvokable]
        #[cxx_name = "setExceptionFilter"]
        fn set_exception_filter(self: Pin<&mut DebugService>, filter: &QString, enabled: bool);

        /// Every running session, as `id\tlabel` lines — the Debug dock's
        /// session picker (D4-5).
        #[qinvokable]
        fn sessions(self: &DebugService) -> QString;

        /// End the session: the adapter is asked to stop the debuggee, then
        /// killed if it does not.
        #[qinvokable]
        fn stop(self: Pin<&mut DebugService>, session_id: u64);

        #[qinvokable]
        fn resume(self: Pin<&mut DebugService>, session_id: u64);

        #[qinvokable]
        fn pause(self: Pin<&mut DebugService>, session_id: u64);

        #[qinvokable]
        #[cxx_name = "stepOver"]
        fn step_over(self: Pin<&mut DebugService>, session_id: u64);

        #[qinvokable]
        #[cxx_name = "stepInto"]
        fn step_into(self: Pin<&mut DebugService>, session_id: u64);

        #[qinvokable]
        #[cxx_name = "stepOut"]
        fn step_out(self: Pin<&mut DebugService>, session_id: u64);

        /// Continue until `path:line` — a temporary breakpoint plus a
        /// resume, which every adapter supports.
        #[qinvokable]
        #[cxx_name = "runToCursor"]
        fn run_to_cursor(self: Pin<&mut DebugService>, session_id: u64, path: &QString, line: u32);

        /// The stopped thread's frames, from the cache the last `stopped`
        /// filled. Empty while running.
        #[qinvokable]
        fn frames(self: &DebugService) -> Vec<FfiStackFrame>;

        /// Every thread the adapter reported at the last stop.
        #[qinvokable]
        fn threads(self: &DebugService) -> Vec<FfiDebugThread>;

        /// Variables already fetched for `reference`; empty means "not
        /// fetched yet", which `expand` answers.
        #[qinvokable]
        fn variables(self: &DebugService, reference: i64) -> Vec<FfiVariable>;

        /// What to paint at the end of the lines of `path`, given the
        /// buffer's current `text` (D3-7). Empty unless a session is
        /// stopped in that very file.
        ///
        /// The text is passed in because the view owns it: a file being
        /// debugged may have unsaved edits, and a value placed against a
        /// line read from disk would sit next to code the user is no longer
        /// looking at.
        #[qinvokable]
        #[cxx_name = "inlineValues"]
        fn inline_values(
            self: &DebugService,
            path: &QString,
            text: &QString,
        ) -> Vec<FfiInlineValue>;

        /// Fetch the children of `reference`; answers via
        /// `variablesChanged`.
        #[qinvokable]
        fn expand(self: Pin<&mut DebugService>, session_id: u64, reference: i64);

        /// Show a frame: its scopes are fetched and each scope expanded,
        /// and later evaluations run in it.
        #[qinvokable]
        #[cxx_name = "selectFrame"]
        fn select_frame(self: Pin<&mut DebugService>, session_id: u64, frame_id: i64);

        /// Evaluate an expression in the selected frame; answers via
        /// `evaluated`. A failed evaluation answers with its own message
        /// rather than nothing.
        #[qinvokable]
        fn evaluate(self: Pin<&mut DebugService>, session_id: u64, expression: &QString);

        /// Change a variable's value. Refused locally when the adapter said
        /// it cannot, rather than sent and failed.
        #[qinvokable]
        #[cxx_name = "setVariable"]
        fn set_variable(
            self: Pin<&mut DebugService>,
            session_id: u64,
            reference: i64,
            name: &QString,
            value: &QString,
        ) -> FfiResult;

        /// Whether this session's adapter allows changing a variable — what
        /// the Variables view enables its editing from.
        #[qinvokable]
        #[cxx_name = "canSetVariable"]
        fn can_set_variable(self: &DebugService, session_id: u64) -> bool;

        /// Whether this session's adapter can reload changed classes
        /// (D4-4) — the JVM's hot code replace. The view disables the
        /// action when it cannot; see the Rust side for why the answer is
        /// per adapter rather than a capability flag.
        #[qinvokable]
        #[cxx_name = "canReloadClasses"]
        fn can_reload_classes(self: &DebugService, session_id: u64) -> bool;

        /// Redefine the running program's classes from what the last build
        /// produced (D4-4). A no-op where the adapter cannot.
        #[qinvokable]
        #[cxx_name = "reloadClasses"]
        fn reload_classes(self: Pin<&mut DebugService>, session_id: u64);

        /// The watch expressions, newline-separated, and their last values
        /// in the same order.
        #[qinvokable]
        fn watches(self: &DebugService) -> QString;

        #[qinvokable]
        #[cxx_name = "watchValues"]
        fn watch_values(self: &DebugService) -> QString;

        #[qinvokable]
        #[cxx_name = "addWatch"]
        fn add_watch(self: Pin<&mut DebugService>, expression: &QString);

        #[qinvokable]
        #[cxx_name = "removeWatch"]
        fn remove_watch(self: Pin<&mut DebugService>, index: u32);

        /// Toggle a line breakpoint, returning whether there is now one
        /// there. Every running session is told.
        #[qinvokable]
        #[cxx_name = "toggleBreakpoint"]
        fn toggle_breakpoint(self: Pin<&mut DebugService>, path: &QString, line: u32) -> bool;

        /// The lines of `path` that have a breakpoint, newline-separated —
        /// the gutter asks for a whole file at once rather than line by
        /// line.
        #[qinvokable]
        #[cxx_name = "breakpointLines"]
        fn breakpoint_lines(self: &DebugService, path: &QString) -> QString;

        /// Give a breakpoint a condition or a log message, or enable and
        /// disable it — the breakpoints dialog's whole job.
        #[qinvokable]
        #[cxx_name = "configureBreakpoint"]
        fn configure_breakpoint(
            self: Pin<&mut DebugService>,
            path: &QString,
            line: u32,
            enabled: bool,
            condition: &QString,
            log_message: &QString,
        );

        /// Mute Breakpoints: the adapter is told there are none, and
        /// unmuting brings back exactly what was there.
        #[qinvokable]
        fn muted(self: &DebugService) -> bool;

        #[qinvokable]
        #[cxx_name = "setMuted"]
        fn set_muted(self: Pin<&mut DebugService>, muted: bool);

        /// An edit moved lines in `path`. Driven from the buffer-edit seam
        /// the editor already has, not from a hook of the debugger's own.
        #[qinvokable]
        #[cxx_name = "shiftBreakpoints"]
        fn shift_breakpoints(self: Pin<&mut DebugService>, path: &QString, from: u32, delta: i64);

        /// Load this project's breakpoints from `.ide/local/`.
        #[qinvokable]
        #[cxx_name = "loadBreakpoints"]
        fn load_breakpoints(self: Pin<&mut DebugService>);

        /// A session started.
        #[qsignal]
        #[cxx_name = "debugStarted"]
        fn debug_started(self: Pin<&mut DebugService>, session_id: u64, config_id: QString);

        /// The debuggee suspended. `path` and `line` are the top frame's, so
        /// the editor can show the execution point without asking.
        #[qsignal]
        #[cxx_name = "debugStopped"]
        fn debug_stopped(
            self: Pin<&mut DebugService>,
            session_id: u64,
            reason: QString,
            path: QString,
            line: u32,
        );

        /// The debuggee is running again.
        #[qsignal]
        #[cxx_name = "debugResumed"]
        fn debug_resumed(self: Pin<&mut DebugService>, session_id: u64);

        /// Output from the debuggee or the adapter. `category` is DAP's —
        /// `stdout`, `stderr`, `console` — shown, never branched on.
        #[qsignal]
        #[cxx_name = "debugOutput"]
        fn debug_output(
            self: Pin<&mut DebugService>,
            session_id: u64,
            category: QString,
            text: QString,
        );

        /// The session ended.
        #[qsignal]
        #[cxx_name = "debugTerminated"]
        fn debug_terminated(self: Pin<&mut DebugService>, session_id: u64, exit_code: i32);

        /// The session could not start, or a request failed.
        #[qsignal]
        #[cxx_name = "debugFailed"]
        fn debug_failed(self: Pin<&mut DebugService>, session_id: u64, error: FfiResult);

        /// `variables(reference)` has a fresh answer.
        #[qsignal]
        #[cxx_name = "variablesChanged"]
        fn variables_changed(self: Pin<&mut DebugService>, session_id: u64, reference: i64);

        /// The selected frame's scopes, newline-separated by name, in the
        /// order the adapter reported them.
        #[qsignal]
        #[cxx_name = "scopesChanged"]
        fn scopes_changed(self: Pin<&mut DebugService>, session_id: u64, names: QString);

        /// `evaluate` answered.
        #[qsignal]
        fn evaluated(
            self: Pin<&mut DebugService>,
            session_id: u64,
            expression: QString,
            value: QString,
        );

        /// The watch list or its values changed.
        #[qsignal]
        #[cxx_name = "watchesChanged"]
        fn watches_changed(self: Pin<&mut DebugService>);

        /// A breakpoint was added, removed, configured or moved.
        #[qsignal]
        #[cxx_name = "breakpointsChanged"]
        fn breakpoints_changed(self: Pin<&mut DebugService>);
    }

    // Enables `self.qt_thread()` on `DebugService` for each session's reader
    // thread and the short-lived threads its requests run on.
    impl cxx_qt::Threading for DebugService {}

    extern "RustQt" {
        /// Settings-page draft for the project's run configurations (F4-10),
        /// isomorphic to `LanguageServerEditor`: load, edit a working copy,
        /// validate, commit back to `.ide/settings.toml` on save.
        #[qobject]
        type RunConfigEditor = super::RunConfigEditorRust;

        /// Re-read the project's run configurations into the draft.
        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &RunConfigEditor);

        /// The draft's configurations, in list order.
        #[qinvokable]
        fn configurations(self: &RunConfigEditor) -> Vec<FfiRunConfig>;

        /// Append a new, empty configuration to the draft.
        #[qinvokable]
        #[cxx_name = "addConfiguration"]
        fn add_configuration(self: &RunConfigEditor);

        /// Remove `configurations()[index]` from the draft. Out-of-range is
        /// a no-op.
        #[qinvokable]
        #[cxx_name = "removeConfiguration"]
        fn remove_configuration(self: &RunConfigEditor, index: u32);

        /// Replace `configurations()[index]`'s editable fields from `form`.
        /// Out-of-range is a no-op.
        ///
        /// The whole struct rather than a field per argument: the form has
        /// outgrown a readable parameter list, and `id`, `toolchain` and
        /// `target` are read-only in the dialog — the draft keeps its own,
        /// so a caller cannot rewrite a configuration's identity by filling
        /// in the wrong field.
        #[qinvokable]
        #[cxx_name = "updateConfiguration"]
        fn update_configuration(self: &RunConfigEditor, index: u32, form: &FfiRunConfig);

        /// The first problem that would stop the dialog closing — an empty
        /// `program` (`run_core::RunError::InvalidConfig`'s own rule,
        /// mirrored here since validation this shallow does not warrant a
        /// second entry point into `run-core`). Code `0` means the draft is
        /// savable.
        #[qinvokable]
        fn validate(self: &RunConfigEditor) -> FfiResult;

        /// Write the draft to `.ide/settings.toml`.
        #[qinvokable]
        fn commit(self: &RunConfigEditor) -> FfiResult;

        /// Discard the draft, restoring what was last loaded or committed.
        #[qinvokable]
        fn revert(self: &RunConfigEditor);
    }

    unsafe extern "C++" {
        include!("main_window.h");

        /// Builds and shows the main window, then runs the Qt event loop
        /// until it's closed. Returns the process exit code.
        #[namespace = "ui_shell"]
        fn run_app() -> i32;
    }
}

// `mod ffi` cannot be split — cxx-qt permits one bridge per crate and the
// shared structs are per-bridge C++ types — so the feature modules name its
// vocabulary through this re-export rather than as `ffi::ffi::…`.
pub use ffi::run_app;
pub(crate) use ffi::*;
