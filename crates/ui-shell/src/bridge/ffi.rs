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
use crate::bridge::analysis::{AnalysisEditorRust, AnalysisServiceRust};
use crate::bridge::app_info::AppInfoRust;
use crate::bridge::build::BuildServiceRust;
use crate::bridge::build_tools::{BuildToolsEditorRust, BuildToolsServiceRust};
use crate::bridge::containers::ContainerServiceRust;
use crate::bridge::convert::{new_syntax_highlighter, syntax_scope_names, SyntaxHighlighterHandle};
use crate::bridge::database::console::{ConsoleServiceRust, ResultProviderRust};
use crate::bridge::database::drivers::DriverInstallServiceRust;
use crate::bridge::database::exchange::ExchangeServiceRust;
use crate::bridge::database::settings::DataSourceEditorRust;
use crate::bridge::database::DatabaseServiceRust;
use crate::bridge::debug::DebugServiceRust;
use crate::bridge::diagnostics::DiagnosticsServiceRust;
use crate::bridge::editor::DocumentManagerRust;
use crate::bridge::editor_ops::EditorOpsRust;
use crate::bridge::file_associations::FileAssociationsEditorRust;
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
use crate::bridge::testing::TestServiceRust;
use crate::bridge::theme::ThemeProviderRust;
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

    /// `FfiResult` plus one named layout's two opaque blobs — `namedLayout`'s
    /// return. Both strings are empty when `code` is non-zero; the view
    /// branches on `code`, never on an empty blob.
    #[derive(Default)]
    struct FfiLayout {
        code: i32,
        message: QString,
        window_state: QString,
        editor_grid: QString,
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

    /// Air around an editor tab's label, per side, in pixels. Always
    /// resolved (`TabPaddingSettings::*_or_default`) — never absent — so the
    /// view applies what it is given directly.
    #[derive(Default)]
    struct FfiTabPadding {
        top: u32,
        bottom: u32,
        left: u32,
        right: u32,
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

    /// Editor minimap (code map) master toggle plus its five overlay
    /// switches, 1:1 with `app_config::MinimapSettings`. Bundled the same
    /// way `FfiWhitespaceOptions` bundles its five related settings above.
    #[derive(Default)]
    struct FfiMinimapOptions {
        enabled: bool,
        search_matches: bool,
        diagnostics: bool,
        vcs_changes: bool,
        breakpoints: bool,
        caret_line: bool,
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

    /// The bracket at a document position and its partner, from
    /// `EditorOps::bracketPairAt` — the live pair highlight (R1). Positions
    /// are flat document UTF-16 offsets, same as `FfiCaret`. `has_bracket`
    /// false means the position is not on a bracket at all; `has_partner`
    /// false with `has_bracket` true is an unmatched bracket, painted in the
    /// error colour rather than not painted at all.
    #[derive(Default)]
    struct FfiBracketPair {
        has_bracket: bool,
        bracket_start: u32,
        bracket_end: u32,
        has_partner: bool,
        partner_start: u32,
        partner_end: u32,
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
    /// siblings under the same parent in the Structure tree, 1:1 with
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

    /// One renderable terminal cell (Task F3, run-painting since T2), 1:1
    /// with `terminal_core::RenderCell` minus its `char`/`CellColor`/
    /// `CellAttributes` Rust types, which cxx can't pass directly.
    /// `character` is a Unicode code point rather than a `QString` — one
    /// `QString` per cell was T2's biggest single allocation source, so the
    /// view builds runs of code points and turns those into text with
    /// `QString::fromUcs4` instead. Blank cells are `' '` (0x20), matching
    /// `terminal_core`'s own convention.
    #[derive(Default)]
    struct FfiTerminalCell {
        character: u32,
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
        /// Inside the current mouse selection — the view tints the run's
        /// background with the selection colour, keeping the glyph's own
        /// foreground (unlike `inverse`, which swaps fg/bg).
        selected: bool,
        /// The leading half of a double-width glyph — see
        /// `terminal_core::RenderCell::wide`. The view skips painting the
        /// cell right after a `wide` one; it is that glyph's spacer half.
        wide: bool,
    }

    /// One RGB color as it crosses the FFI seam — the same r/g/b-bytes
    /// convention `FfiTerminalCell`'s `fg_r`/`fg_g`/`fg_b` already uses,
    /// wrapped here only because `FfiTerminalPalette::ansi` needs a `Vec`
    /// element type (the same "table of N colors" shape `palette()`'s
    /// `Vec<FfiScopeStyle>` already crosses the seam with).
    #[derive(Default, Clone, Copy)]
    struct FfiRgb {
        r: u8,
        g: u8,
        b: u8,
    }

    /// A whole terminal palette (T3), in one call: `theme.cpp`'s
    /// `terminalPaletteForTheme()` builds one from the active theme (and, for
    /// background/foreground, the editor colors when configured — see that
    /// function's doc comment), and `TerminalSupervisor::setPalette()`
    /// applies it to every open session and remembers it for new ones.
    /// `ansi` is always exactly 16 entries, ANSI 0-15.
    #[derive(Default)]
    struct FfiTerminalPalette {
        background: FfiRgb,
        foreground: FfiRgb,
        cursor: FfiRgb,
        selection: FfiRgb,
        ansi: Vec<FfiRgb>,
    }

    /// The chrome design spec's colour roles (T7), one set per active colour
    /// theme — mirrors `ui_shell::ChromePalette` (`crates/ui-shell/cpp/theme.h`)
    /// minus `chevron`/`shadow`/`shadowOpacity`, which are not colour data a
    /// theme supplies: `theme.cpp` derives them from the resolved appearance.
    #[derive(Default)]
    struct FfiChromePalette {
        canvas: FfiRgb,
        surface: FfiRgb,
        surface2: FfiRgb,
        raised: FfiRgb,
        border: FfiRgb,
        text: FfiRgb,
        text_dim: FfiRgb,
        accent: FfiRgb,
        accent_ink: FfiRgb,
        selection: FfiRgb,
        status_bar: FfiRgb,
    }

    /// The status/severity colours a theme supplies (T7) — mirrors
    /// `ui_shell::SemanticColors`.
    #[derive(Default)]
    struct FfiSemanticColors {
        error: FfiRgb,
        warning: FfiRgb,
        info: FfiRgb,
        ok: FfiRgb,
        muted: FfiRgb,
    }

    /// The colours a diff paints with (T7) — mirrors `ui_shell::DiffColors`.
    #[derive(Default)]
    struct FfiDiffColors {
        added_line: FfiRgb,
        added_inline: FfiRgb,
        added_marker: FfiRgb,
        modified_line: FfiRgb,
        modified_inline: FfiRgb,
        modified_marker: FfiRgb,
        deleted_line: FfiRgb,
        deleted_inline: FfiRgb,
        deleted_marker: FfiRgb,
    }

    /// One paint's worth of grid state (T2): the whole snapshot
    /// `gridCells`/`gridRows`/`gridCols`/`cursorRow`/`cursorCol` used to
    /// require five separate FFI round trips for — replaced with the one
    /// call `cpp/terminal_widget.cpp`'s `paintEvent` makes when (and only
    /// when) it is about to repaint. `cells` is `rows * cols` long,
    /// row-major, same flattening convention the old `gridCells` used.
    ///
    /// `cursor_visible` (T5): false while the viewport is scrolled up into
    /// history, so the view knows not to paint the cursor block over
    /// unrelated history text — `terminal_core::Grid::cursor_visible`'s own
    /// doc comment has the full reasoning.
    #[derive(Default)]
    struct FfiTerminalSnapshot {
        rows: u32,
        cols: u32,
        cursor_row: u32,
        cursor_col: u32,
        cursor_visible: bool,
        cells: Vec<FfiTerminalCell>,
    }

    /// Scrollback position (T5): how far back the buffer goes, and how far
    /// the viewport is currently scrolled into it. 1:1 with
    /// `terminal_core::ScrollState`. The view derives its scrollbar's
    /// range/value from this, refetched alongside `FfiTerminalSnapshot`.
    #[derive(Default)]
    struct FfiScrollState {
        history: u64,
        offset: u64,
    }

    /// What a terminal mouse gesture selects (Task F4), 1:1 with
    /// `terminal_core::SelectionKind`.
    enum FfiSelectionKind {
        Simple,
        Word,
        Line,
    }

    /// A logical key press crossing the seam (Task T4), 1:1 with
    /// `terminal_core::keys::Key`. `cpp/terminal_widget.cpp`'s
    /// `keyPressEvent` maps `Qt::Key` to this — pure enum translation, a
    /// humble view concern — and the actual xterm escape-sequence encoding
    /// happens on the Rust side (`terminal_core::keys::encode`), never in
    /// C++.
    ///
    /// `Char` and `F` carry no payload of their own: a cxx enum can't be a
    /// Rust-style data-carrying enum, so `send_key`'s `code_point` parameter
    /// carries the Unicode code point for `Char` and the function-key number
    /// (1-12) for `F`, the same "typed flag plus a field that means
    /// something only for certain variants" convention `FfiSymbolMatch`'s
    /// `has_kind`/`kind` already uses.
    enum FfiTerminalKey {
        Char,
        Enter,
        Tab,
        Backspace,
        Escape,
        Up,
        Down,
        Left,
        Right,
        Home,
        End,
        PageUp,
        PageDown,
        Insert,
        Delete,
        F,
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
    /// R8: `search`'s three toggles, bundled into one struct rather than
    /// three positional `bool`s — `search` was already at the clippy
    /// `too_many_arguments` ceiling before R8 added `whole_word`, and a
    /// fourth positional bool would only make the call site harder to
    /// read correctly.
    #[derive(Default)]
    struct FfiSearchOptions {
        is_regex: bool,
        case_sensitive: bool,
        whole_word: bool,
    }

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

    /// `DiagnosticsService::nextDiagnostic`/`previousDiagnostic`'s answer
    /// (R4): `found == false` means "no diagnostics in this file" and the
    /// other fields are meaningless — the same typed-flag shape as
    /// `FfiLocation` (ADR-0003), rather than a sentinel line number. Same
    /// units as `FfiDiagnostic`: `line` 1-based, `column` 0-based.
    struct FfiDiagnosticJump {
        found: bool,
        line: u32,
        column: u32,
        severity: FfiSeverity,
        message: QString,
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
        /// R2: already-rendered HTML (`markdown_preview::render`), ready
        /// for the docs panel's `QTextBrowser::setHtml` — never raw
        /// Markdown, so the view never has to decide how to render it.
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
        /// R2: the server marked this item deprecated — the delegate
        /// strikes its label through.
        deprecated: bool,
        /// R2: `insertTextFormat: 2` — `insert` is snippet source
        /// (`edit_ops::snippet::parse` still has to run on it) rather than
        /// plain text.
        is_snippet: bool,
        /// R2: char indices into `label` the typed prefix matched
        /// (`lsp_core::completion::CompletionMatch::positions`), for the
        /// delegate to bold. Empty when there is nothing to highlight.
        match_positions: Vec<u32>,
    }

    /// R2: one snippet tab stop, in two different units depending on which
    /// direction it is crossing the seam.
    ///
    /// Outgoing (`LanguageService::snippetReady`): `start`/`end` are UTF-16
    /// char offsets *within the snippet's own flattened text*
    /// (`edit_ops::snippet::parse`'s output), not document positions —
    /// `snippetReady` also carries the insertion point, and the view adds
    /// the two once the edit has landed.
    ///
    /// Incoming (`EditorOps::beginSnippet`) and the return of
    /// `EditorOps::stepSnippet`: `start`/`end` are absolute flat document
    /// positions (`QTextCursor::position()`'s own unit), and `has_stop`
    /// false means "no stop to select" — the caller falls through to
    /// whatever the key ordinarily does. `more` is false on the last stop:
    /// per R2's target, landing there ends the session outright.
    #[derive(Default)]
    struct FfiSnippetStop {
        has_stop: bool,
        start: u32,
        end: u32,
        more: bool,
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

    /// Which whitespace differences count as a change, 1:1 with
    /// `editor_core::diff::WhitespaceMode` — the diff toolbar's "Ignore
    /// whitespace" menu, in the same order.
    enum FfiWhitespaceMode {
        Exact,
        TrimEnds,
        IgnoreAll,
        IgnoreAllAndBlankLines,
    }

    /// How finely a modified hunk is compared, 1:1 with
    /// `editor_core::diff::HighlightMode` — the diff toolbar's
    /// "Highlighting mode" menu. `Lines` and `None` both produce no spans;
    /// the view additionally drops line backgrounds for `None`.
    enum FfiHighlightMode {
        Words,
        Chars,
        Lines,
        None,
    }

    /// What one aligned diff row shows, 1:1 with
    /// `editor_core::diff::RowKind`.
    enum FfiRowKind {
        Context,
        Added,
        Removed,
    }

    /// One row of the aligned diff layout, 1:1 with
    /// `editor_core::diff::DiffRow`. `old_line`/`new_line` are `-1` where
    /// the row has no line on that side; the anchors always name a real
    /// line (or 0 for an empty side). The unified viewer renders these top
    /// to bottom, the side-by-side viewer scrolls by them, and the divider
    /// joins them — the view indexes, it never derives alignment.
    struct FfiDiffRow {
        old_line: i32,
        new_line: i32,
        kind: FfiRowKind,
        old_anchor: u32,
        new_anchor: u32,
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
        // Appended for G5 (ADR-0003: append-only) — `vcs_core::ChangeKind`
        // variants `git status --porcelain=v2` reports that the original
        // four-plus-Untracked shape had no room for.
        Renamed,
        Copied,
        Conflicted,
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
        MergeConflict = 711,
        NoUpstream = 712,
    }

    /// 1:1 with `vcs_core::ResetMode` — R7's "Reset Current Branch Here".
    enum FfiResetMode {
        Soft,
        Mixed,
        Hard,
    }

    /// 1:1 with `vcs_core::RefKind` — R7's log-row ref chips.
    enum FfiRefKind {
        Head,
        Local,
        Remote,
        Tag,
    }

    /// One path `VcsService::changedFiles` reports: `vcs_core::FileStatus`
    /// plus the untracked pile folded in as `unstaged: Untracked`, so the
    /// view reads one list rather than three.
    struct FfiChangedFile {
        path: QString,
        staged: FfiChangeKind,
        unstaged: FfiChangeKind,
        /// The path this entry was renamed/copied from
        /// (`vcs_core::FileStatus::orig_path`); empty otherwise. Appended
        /// for G5 (ADR-0003: append-only).
        orig_path: QString,
    }

    /// The repository's branch/upstream/ahead-behind picture, 1:1 with
    /// `vcs_core::RepoStatus`'s own four fields — refreshed by the same
    /// `statusChanged` signal `changedFiles`/`fileStatus` already answer
    /// off of, not a new signal.
    #[derive(Default)]
    struct FfiBranchStatus {
        branch: QString,
        upstream: QString,
        ahead: u32,
        behind: u32,
        has_upstream: bool,
        detached: bool,
    }

    /// One commit, 1:1 with `vcs_core::LogEntry` (F3-12d), plus R7's lane
    /// graph position — computed once per `commitLog`/`commitLogFiltered`
    /// answer from `vcs_core::graph::lanes` rather than a second round trip
    /// the delegate would have to correlate back to this same list by id.
    struct FfiLogEntry {
        id: QString,
        summary: QString,
        author_name: QString,
        author_email: QString,
        /// Seconds since the Unix epoch, author time.
        author_time: i64,
        /// R7: this commit's lane-graph column.
        lane: u32,
        /// R7: the column each parent's line continues into, same order as
        /// `vcs_core::LogEntry::parent_ids`.
        parent_lanes: Vec<u32>,
    }

    /// One ref decorating a commit, 1:1 with `vcs_core::RefDecoration`
    /// (R7's log-row chips).
    struct FfiRefDecoration {
        name: QString,
        kind: FfiRefKind,
    }

    /// One stash entry, 1:1 with `vcs_core::StashEntry` (R7).
    struct FfiStashEntry {
        index: u32,
        message: QString,
    }

    /// One configured remote, 1:1 with `vcs_core::RemoteInfo` (R7's remote
    /// picker).
    struct FfiRemoteInfo {
        name: QString,
        url: QString,
    }

    /// One commit in full, 1:1 with `vcs_core::CommitDetail`, for the
    /// commit-detail dock's header and message body. `parent_ids` is
    /// space-joined (display-only — nothing here re-parses it into a list;
    /// a commit's own id is already the join key `changedCommitFiles`/
    /// `requestCommitFileDiff` use).
    #[derive(Default)]
    struct FfiCommitDetail {
        id: QString,
        summary: QString,
        body: QString,
        author_name: QString,
        author_email: QString,
        author_time: i64,
        committer_name: QString,
        committer_email: QString,
        committer_time: i64,
        parent_ids: QString,
    }

    /// One path a commit touched, 1:1 with `vcs_core::ChangedCommitFile` —
    /// reuses `FfiChangeKind` verbatim (a commit's changes are the same
    /// four kinds `FfiChangedFile` already carries for the working tree;
    /// `None`/`Untracked` never occur here).
    struct FfiChangedCommitFile {
        path: QString,
        change: FfiChangeKind,
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
        /// Empty means "follow the editor font" (T3) —
        /// `AppSettings::terminalFont()` is where that precedence is
        /// resolved; this raw, possibly-empty value is only for the
        /// settings page to edit.
        font_family: QString,
        /// `0` means "follow the editor font size" (T3), same idiom as
        /// `font_family`.
        font_size: u32,
    }

    /// The `[containers]` section's own flat fields (ADR-0055), for the
    /// Settings > Containers page — the connection list itself crosses
    /// separately as `Vec<FfiContainerConnection>`
    /// (`AppSettings::containerConnections`/`saveContainerConnections`),
    /// the same split `FileAssociationsEditor` draws between its scalar
    /// settings and its rule list.
    #[derive(Default)]
    struct FfiContainerSettings {
        show_stopped_containers: bool,
        show_untagged_images: bool,
        selinux_relabel: bool,
    }

    /// One row of `ContainerConnectionSetting`, 1:1 with the app-config
    /// struct: every kind-specific field is a plain (possibly empty)
    /// string, same "persistence stays dumb" rule ADR-0017/ADR-0039 already
    /// draw for run configurations — `container_core::connection` is what
    /// gives `kind` and its fields meaning.
    #[derive(Default)]
    struct FfiContainerConnection {
        id: QString,
        name: QString,
        /// `"docker"` or `"podman"`.
        engine: QString,
        /// `"auto"`, `"unix_socket"`, `"tcp"`, `"named_pipe"`, `"context"`,
        /// `"ssh"`, `"wsl"`, `"podman_machine"`, or `"minikube"`.
        kind: QString,
        path: QString,
        url: QString,
        cert_dir: QString,
        identity: QString,
        distro: QString,
        resource_name: QString,
        executable: QString,
        compose_executable: QString,
    }

    /// One candidate `discoverContainerConnections()` offers: a docker
    /// context, a podman connection/machine, or a socket preset (Colima,
    /// Rancher Desktop, the default rootless podman socket) — the source
    /// for the Containers settings page's "Add from contexts..." menu.
    /// Never saved on its own; the view turns a chosen entry into an
    /// `FfiContainerConnection` it appends to the draft list itself.
    struct FfiDiscoveredConnection {
        label: QString,
        engine: QString,
        kind: QString,
        path: QString,
        url: QString,
        distro: QString,
        resource_name: QString,
    }

    /// One row of `ContainerTargetSetting` (C8), 1:1 with the app-config
    /// struct — the same flat-row shape `FfiContainerConnection` uses for
    /// the same reason (its own doc comment): `source` tags which of the
    /// other fields apply, `container_core::target` is what gives it
    /// meaning. Crosses on `AppSettings::containerTargets`/
    /// `saveContainerTargets` (Settings > Containers > Run targets) and the
    /// New Target wizard, and read-only from `RunConfigEditor::
    /// containerTargets` for the run-config dialog's "Run on" combo.
    #[derive(Default)]
    struct FfiContainerTarget {
        id: QString,
        name: QString,
        connection_id: QString,
        /// `"image"`, `"containerfile"`, or `"compose-service"`.
        source: QString,
        image: QString,
        dockerfile: QString,
        context_dir: QString,
        image_tag: QString,
        /// `\n`-separated, ordered.
        compose_files: QString,
        service: QString,
        needs_build: bool,
        /// Empty means `/workspace` (`container_core::target::DEFAULT_WORKDIR`).
        workdir: QString,
        run_options: QString,
        env: Vec<FfiKeyValue>,
        port_bindings: Vec<FfiPortBinding>,
        publish_all_ports: bool,
        extra_mounts: Vec<FfiBindMount>,
    }

    /// One row of `RegistrySetting`, 1:1 with the app-config struct —
    /// still no secret field (ADR-0055): a secret crosses separately, only
    /// ever as a call argument (`testRegistryConnection`/
    /// `storeRegistrySecret`), never read back.
    #[derive(Default)]
    struct FfiRegistrySetting {
        id: QString,
        name: QString,
        /// `"hub"`, `"gitlab"`, `"v2"`, or `"generic"`.
        kind: QString,
        address: QString,
        username: QString,
        gitlab_project: QString,
        token_auth: bool,
    }

    /// One local branch name. `cxx`'s `Vec<T>` needs `T: ImplVec`, which
    /// `QString` alone does not satisfy — this one-field wrapper is what
    /// lets `branches()` cross as a list at all, the same reason
    /// `FfiResourceOp` wraps a `QString` rather than the seam carrying a
    /// bare `Vec<QString>` anywhere.
    struct FfiBranch {
        name: QString,
    }

    /// One past commit message. Same one-field-wrapper reason as
    /// [`FfiBranch`]: `cxx`'s `Vec<T>` needs `T: ImplVec`, which `QString`
    /// alone does not satisfy.
    struct FfiCommitMessage {
        message: QString,
    }

    /// One repository-relative path, the same one-field-wrapper reason as
    /// [`FfiBranch`]. Carried by `changedPathsReady` (R6's project-root
    /// "Compare with Branch, Tag or Revision…").
    struct FfiRepoPath {
        path: QString,
    }

    /// Whether a `HEAD`-vs-worktree hunk is not yet in the index, wholly in
    /// it, or partly — `vcs_core::HunkStageState`, IDEA's three-state
    /// gutter colouring (R6).
    enum FfiHunkStageState {
        Unstaged,
        Staged,
        Both,
    }

    /// One hunk's [`FfiHunkStageState`], parallel to `hunks(path)` by
    /// index. A one-field struct rather than a bare `Vec<FfiHunkStageState>`,
    /// for the same `cxx` `Vec<T>` reason [`FfiBranch`] gives.
    struct FfiHunkState {
        state: FfiHunkStageState,
    }

    /// One `-p`/`--publish` port binding row (C5, ADR-0056), 1:1 with
    /// `app_config::container_run::PortBinding`.
    #[derive(Default)]
    struct FfiPortBinding {
        host_ip: QString,
        host_port: QString,
        container_port: QString,
        /// `"tcp"` or `"udp"`; empty reads as `tcp`.
        protocol: QString,
    }

    /// One `-v`/`--mount` bind-mount row, 1:1 with
    /// `app_config::container_run::BindMount`.
    #[derive(Default)]
    struct FfiBindMount {
        host_path: QString,
        container_path: QString,
        read_only: bool,
    }

    /// One `--scale service=n` row, 1:1 with a `ComposeRunSetting::scale`
    /// entry.
    #[derive(Default)]
    struct FfiScaleEntry {
        service: QString,
        count: u32,
    }

    /// A container-kind run configuration's options (C5, ADR-0056) — every
    /// field `app_config::container_run::{ContainerImageRunSetting,
    /// ContainerfileRunSetting, ComposeRunSetting}` have between them,
    /// flattened onto one struct the same way `FfiContainerConnection`
    /// flattens every connection kind's fields (its own doc comment gives
    /// the reason: a tagged union has no clean `toml`/shared-struct
    /// representation, and this is read/written wholesale, never
    /// interpreted, by the dialog). Which fields apply is `FfiRunConfig::kind`
    /// — a container-image configuration ignores `dockerfile`/`context_dir`/
    /// `build_args`/the compose-only fields entirely, for instance.
    ///
    /// `run_attach`/`compose_attach` and `recreate`/`build` are separately
    /// named from what `app_config::container_run` calls them
    /// (`attach`/`recreate`/`build` on two different structs, `attach` on
    /// three) only because this one flat struct cannot have two fields
    /// named `attach`; the bridge's translation (`bridge/run/mod.rs`) maps
    /// each back to its own sub-table's field of the JetBrains-matching
    /// name.
    #[derive(Default)]
    struct FfiContainerOptions {
        /// Which `[containers.connection]` row runs this — the Server
        /// combo, populated from `ContainerService::connections()`.
        connection_id: QString,
        /// Image reference (container-image only; a containerfile
        /// configuration runs `image_tag` instead, once built).
        image: QString,
        container_name: QString,
        publish_all_ports: bool,
        port_bindings: Vec<FfiPortBinding>,
        entrypoint: QString,
        /// Space-joined, like `FfiRunConfig::args`.
        command: QString,
        bind_mounts: Vec<FfiBindMount>,
        env: Vec<FfiKeyValue>,
        /// Free-form extra `run` arguments, shell-word-split and appended
        /// verbatim.
        run_options: QString,
        /// Attach (`-a`) vs. detach (`-d`) — image/containerfile only.
        run_attach: bool,
        /// `"missing"` (default), `"always"`, or `"never"`.
        pull_policy: QString,

        // Containerfile-only:
        dockerfile: QString,
        context_dir: QString,
        image_tag: QString,
        build_args: Vec<FfiKeyValue>,
        build_options: QString,
        run_built_image: bool,

        // Compose-only:
        /// `\n`-separated, ordered — the compose files list's Up/Down
        /// reordering is reflected here on commit.
        compose_files: QString,
        /// `\n`-separated; empty means every service the files define.
        services: QString,
        project_name: QString,
        profiles: QString,
        env_files: QString,
        compatibility: bool,
        remove_orphans_on_down: bool,
        remove_volumes_on_down: bool,
        /// `"none"` (default), `"all"`, or `"local"`.
        remove_images_on_down: QString,
        /// Empty means unset.
        sigkill_timeout: QString,
        /// Empty means unset.
        exit_code_from: QString,
        scale: Vec<FfiScaleEntry>,
        always_recreate_deps: bool,
        renew_anon_volumes: bool,
        remove_orphans: bool,
        no_log_prefix: bool,
        /// `"selected_and_deps"` (default), `"none"`, or `"selected_only"`.
        start: QString,
        /// `"selected"` (default), `"none"`, or `"selected_and_deps"`.
        compose_attach: QString,
        /// `"changed"` (default), `"all"`, or `"none"`.
        recreate: QString,
        /// `"missing"` (default), `"never"`, or `"always"`.
        build: QString,
        abort_on_container_exit: bool,
    }

    /// One run configuration, 1:1 with `run_core::RunConfig`
    /// (`app_config::RunConfigSetting`). `args` crosses space-joined — the
    /// same convention `FfiLanguageServerRow::args` already uses (shell-style
    /// quoting is the upgrade if a literal space in an argument ever
    /// matters, not a list editor) — and `env` as `KEY=VALUE` lines
    /// separated by `\n`, since a bare `Vec<QString>` is not a shape cxx
    /// supports (see `FfiFileDiff`'s doc comment) — `FfiContainerOptions`'s
    /// own list fields sidestep this by wrapping each row in a named
    /// struct instead.
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
        /// What kind of thing this configuration launches (C5, ADR-0056):
        /// empty for a plain process, or `"container-image"` /
        /// `"containerfile"` / `"compose"`.
        kind: QString,
        /// The container-kind sub-table's fields — meaningless (and left at
        /// its default) for a plain process (`kind` empty).
        container: FfiContainerOptions,
        /// The `sql-script` kind's own sub-table (F3.6) — meaningless (and
        /// left at its default) unless `kind == "sql-script"`.
        sql_script: FfiSqlScriptOptions,
        /// Run targets (C8): empty for "Local" (run on this machine, as
        /// always), else `"container:<target-id>"` — the "Run on" combo's
        /// selection. Meaningless for a container-kind configuration
        /// (`kind` non-empty); the dialog's Run on combo is hidden for one.
        run_on: QString,
    }

    /// The `sql-script` run configuration's own page (database-tools-plan
    /// F3.6): which data source, which file, and its error policy —
    /// `app_config::SqlScriptRunSetting`'s exact fields, crossing the seam
    /// the same structured way `FfiContainerOptions` does.
    #[derive(Default)]
    struct FfiSqlScriptOptions {
        source_id: QString,
        file: QString,
        /// `"single_transaction"` or empty (auto-commit) — see
        /// `app_config::SqlScriptRunSetting::tx_mode`'s own doc comment.
        tx_mode: QString,
        stop_on_error: bool,
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

    /// One breakpoint's full detail (R5): what the Edit Breakpoint dialog
    /// and the Breakpoints window show, 1:1 with `dap_core::Breakpoint`
    /// minus `suspend_policy` and `depends_on`, which have no view yet.
    #[derive(Default)]
    struct FfiBreakpoint {
        path: QString,
        line: u32,
        enabled: bool,
        condition: QString,
        hit_condition: QString,
        log_message: QString,
        temporary: bool,
    }

    /// One watch expression as the Watches tree shows it (R5): 1:1 with
    /// `FfiVariable` except its name is what the user typed rather than
    /// what the adapter called it, so it stays what the user typed even
    /// while there is no session to evaluate it against.
    struct FfiWatch {
        expression: QString,
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
        /// Seconds since the Unix epoch, author time (R7: the gutter's
        /// hover tooltip and age-shaded background).
        author_time: i64,
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
    /// the shown signature's label and doc, and its active parameter's span
    /// within that label to embolden, plus that parameter's own
    /// documentation (R3). `signature_index`/`signature_count` back the
    /// "(1/3)" overload indicator and R3's Up/Down cycling
    /// (`cycleSignatureOverload`). `has_signature` false means nothing to
    /// show — the default value, so a tip that never asked reads as empty
    /// rather than as overload zero of nothing.
    #[derive(Default)]
    struct FfiSignatureHelp {
        has_signature: bool,
        label: QString,
        documentation: QString,
        has_active_parameter: bool,
        parameter_start: u32,
        parameter_end: u32,
        parameter_documentation: QString,
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

        /// W7-1 (ADR-0052): the distro name if the open project's root is a
        /// WSL UNC path, empty otherwise. `cpp/` only ever branches on
        /// "empty or not" — the classification itself is
        /// `process_exec::host::ExecHost::for_path`'s decision, not a
        /// business rule reimplemented in C++.
        #[qinvokable]
        #[cxx_name = "remoteWslDistro"]
        fn remote_wsl_distro(self: &ProjectTreeModel) -> QString;

        /// The Linux path a `remoteWslDistro()`-nonempty root translates to
        /// — the status bar's tooltip content. Empty when
        /// `remoteWslDistro()` is empty.
        #[qinvokable]
        #[cxx_name = "remoteWslLinuxRoot"]
        fn remote_wsl_linux_root(self: &ProjectTreeModel) -> QString;

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

        /// Emitted when the filesystem watcher fails to (re)start for the
        /// project `openFolder`/`reopenLastProject` just opened
        /// successfully — distinct from `projectOpenFailed`, since the
        /// project itself is open and the tree is showing; only live
        /// external-change detection (the Changes dock, the "reload from
        /// disk" prompt) is degraded until the project is reopened.
        #[qsignal]
        #[cxx_name = "watcherFailed"]
        fn watcher_failed(self: Pin<&mut ProjectTreeModel>, result: FfiResult);
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

        /// Structure's per-file tier (Task D): the tab's symbol outline
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
        /// 0 text, 1 binary, 2 diff, 3 image (ADR-0020). The
        /// view builds a `CodeEditor`, a `HexViewer`, a `DiffView` or an
        /// `ImageViewer` from this; it never decides the kind itself from
        /// the path or the bytes. Unknown ids answer 0, the same "treat it
        /// as ordinary" default the widget-construction path already takes.
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

        /// Rasterises an image tab's SVG file — raster formats
        /// (PNG/JPG/GIF/BMP/WEBP) are decoded by `ImageViewer` itself via
        /// `QImageReader`, straight from `tabPath`, and never reach here.
        /// An empty result (zero width and height) means the file could not
        /// be read or parsed as SVG.
        #[qinvokable]
        #[cxx_name = "renderSvgImage"]
        fn render_svg_image(self: &DocumentManager, tab_id: u64) -> FfiImagePixels;

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

        /// Open a read-only virtual document — no backing file — under
        /// `scheme`/`key` (C12's mechanism, generalised for F5b: an ER
        /// diagram's Mermaid text, a schema-compare migration script).
        /// Focuses the existing tab rather than duplicating one for the
        /// same `(scheme, key)`. Returns the tab's id and emits
        /// `tabOpened` for a genuinely new one, same as `openFile`.
        #[qinvokable]
        #[cxx_name = "openVirtualDocument"]
        fn open_virtual_document(
            self: Pin<&mut DocumentManager>,
            scheme: &QString,
            key: &QString,
            text: &QString,
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
        /// state — for `DiffViewPage`'s toolbar, which recomputes the same
        /// two texts under whatever whitespace and highlighting mode the
        /// user picks. Hunks, spans and rows are three calls because a
        /// `Vec` field on a shared struct is not a shape cxx supports.
        #[qinvokable]
        #[cxx_name = "diffHunksBetween"]
        fn diff_hunks_between(
            self: &DocumentManager,
            left_text: &QString,
            right_text: &QString,
            whitespace: FfiWhitespaceMode,
        ) -> Vec<FfiHunk>;
        #[qinvokable]
        #[cxx_name = "diffSpansBetween"]
        fn diff_spans_between(
            self: &DocumentManager,
            left_text: &QString,
            right_text: &QString,
            whitespace: FfiWhitespaceMode,
            highlight: FfiHighlightMode,
        ) -> Vec<FfiInlineSpan>;
        #[qinvokable]
        #[cxx_name = "diffRowsBetween"]
        fn diff_rows_between(
            self: &DocumentManager,
            left_text: &QString,
            right_text: &QString,
            whitespace: FfiWhitespaceMode,
        ) -> Vec<FfiDiffRow>;

        /// The in-buffer edit that replaces `hunk`'s lines on the right side
        /// with the left side's — the diff viewer's apply chevron. Only the
        /// left text is needed (the hunk names the right-side range). Line
        /// columns are 0 and `in_buffer` is true, the same shape
        /// `VcsService::revertHunk` builds; `path` is empty for the caller
        /// to fill in.
        #[qinvokable]
        #[cxx_name = "hunkRevertEdit"]
        fn hunk_revert_edit(
            self: &DocumentManager,
            left_text: &QString,
            hunk: FfiHunk,
        ) -> FfiTextEdit;

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
        /// Symbol, Structure and Go to Line their history for free.
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

    extern "RustQt" {
        /// The colour-theme seam (T7): every colour a theme supplies,
        /// resolved from the `color-themes` plugin contribution that is
        /// currently active. `theme.cpp` asks this once per `applyTheme()`
        /// call and caches the result — everything downstream (stylesheets,
        /// palettes, the syntax highlighter) reads that cache rather than
        /// asking again per repaint.
        #[qobject]
        type ThemeProvider = super::ThemeProviderRust;

        /// Every colour theme the loaded plugins offer — the Appearance
        /// page's combo, in registry order.
        #[qinvokable]
        #[cxx_name = "colorThemes"]
        fn color_themes(self: &ThemeProvider) -> Vec<FfiColorThemeChoice>;

        /// Make `id` the active colour theme, without persisting the choice:
        /// the Appearance page's live preview, and the Cancel path that puts
        /// the previous one back. An id nothing offers falls back the same
        /// way `ColorThemeService::load` does at startup.
        #[qinvokable]
        #[cxx_name = "applyColorTheme"]
        fn apply_color_theme(self: &ThemeProvider, id: &QString);

        /// Whether the active theme is a dark one — the one bit `theme.cpp`
        /// needs beyond the colours themselves, to pick the shared
        /// dark/light chevron glyph and shadow ink (T1 deliberately keeps
        /// those out of `ChromeColors`; see `color_theme::ChromeColors`).
        #[qinvokable]
        #[cxx_name = "isDark"]
        fn is_dark(self: &ThemeProvider) -> bool;

        /// The active theme's chrome colours, or a black fallback when
        /// nothing resolved (practically unreachable — a built-in dark
        /// theme is always offered).
        #[qinvokable]
        #[cxx_name = "chromePalette"]
        fn chrome_palette(self: &ThemeProvider) -> FfiChromePalette;

        /// The active theme's status/severity colours.
        #[qinvokable]
        #[cxx_name = "semanticColors"]
        fn semantic_colors(self: &ThemeProvider) -> FfiSemanticColors;

        /// The active theme's diff colours.
        #[qinvokable]
        #[cxx_name = "diffColors"]
        fn diff_colors(self: &ThemeProvider) -> FfiDiffColors;

        /// The active theme's terminal palette — reuses `FfiTerminalPalette`
        /// (T3) rather than a new struct, since `color_theme::TerminalColors`
        /// already mirrors its shape.
        #[qinvokable]
        #[cxx_name = "terminalPalette"]
        fn terminal_palette(self: &ThemeProvider) -> FfiTerminalPalette;
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

    /// An SVG image tab's file, rasterised at its own intrinsic size —
    /// premultiplied RGBA8, the same byte order [`FfiPreviewImage`] and
    /// `IconProvider::iconPixels` already use, via `icon-theme`'s `resvg`
    /// pipeline. `ImageViewer` builds one `QImage` from this
    /// and scales it for fit/zoom itself, the same as a raster format it
    /// decoded through `QImageReader` directly.
    #[derive(Default)]
    struct FfiImagePixels {
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

    /// Where a contributed tool window docks by default — mirrors
    /// `plugin_api::ToolWindowArea`, this crate's own copy because a shared
    /// type would put cxx-qt in `plugin-api`'s dependency tree.
    enum FfiToolWindowArea {
        Left,
        Right,
        Bottom,
        Center,
    }

    /// One `tool-windows` contribution (the database-tools plan's G1):
    /// `tool_window_factories.cpp`'s table looks a dock factory up by `id`,
    /// and the View-menu action it wires carries `title` as-is (contributed
    /// text, not a `tr()` literal).
    struct FfiToolWindow {
        /// The plugin that contributed it, for the "no native host" log
        /// line when nothing in the factory table answers to `id`.
        plugin_id: QString,
        id: QString,
        title: QString,
        area: FfiToolWindowArea,
    }

    /// Mirrors `plugin_api::SettingsPageScope`, for the same reason
    /// `FfiToolWindowArea` mirrors `ToolWindowArea`.
    enum FfiSettingsPageScope {
        Global,
        Project,
    }

    /// One `settings-pages` contribution (the database-tools plan's G1).
    struct FfiSettingsPage {
        plugin_id: QString,
        id: QString,
        title: QString,
        scope: FfiSettingsPageScope,
    }

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

        /// Whether the window was maximized when it last closed. Read
        /// alongside `windowGeometry`, never instead of it: the geometry is
        /// the *normal* rect, so a maximized window needs both.
        #[qinvokable]
        #[cxx_name = "windowMaximized"]
        fn window_maximized(self: &AppSettings) -> bool;

        /// Persist whether the window is maximized (the same `closeEvent`).
        #[qinvokable]
        #[cxx_name = "saveWindowMaximized"]
        fn save_window_maximized(self: &AppSettings, maximized: bool);

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

        /// Named workspace arrangements, ordered by name: the global ones
        /// merged with the open project's, a project entry shadowing a
        /// global one of the same name (ADR-0045). A layout holds docks and
        /// the editor grid and never files, which is what separates it from
        /// `windowState`/`editorLayout` above — those are the last session,
        /// this is an arrangement the user chose to keep.
        #[qinvokable]
        #[cxx_name = "layoutNames"]
        fn layout_names(self: &AppSettings) -> QStringList;

        /// Store a layout under `name` in `scope` (`"global"` or
        /// `"project"`), replacing one of the same name in that layer.
        /// `window_state` is base64 as for `windowState`; `editor_grid` is
        /// the view's JSON with no files in it.
        #[qinvokable]
        #[cxx_name = "saveNamedLayout"]
        fn save_named_layout(
            self: &AppSettings,
            name: &QString,
            scope: &QString,
            window_state: &QString,
            editor_grid: &QString,
        ) -> FfiResult;

        /// One resolved layout by name, for the view to apply.
        #[qinvokable]
        #[cxx_name = "namedLayout"]
        fn named_layout(self: &AppSettings, name: &QString) -> FfiLayout;

        /// Delete the layout `name` from whichever layer defines it. A
        /// project entry shadowing a global one is deleted first, revealing
        /// the global layout rather than removing both.
        #[qinvokable]
        #[cxx_name = "deleteNamedLayout"]
        fn delete_named_layout(self: &AppSettings, name: &QString) -> FfiResult;

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

        /// Active UI locale as a BCP-47 tag, e.g. "de" — defaults to "en"
        /// when unset or unsupported (`Settings::ui_locale_or_default`).
        /// Applying a change requires a restart: there is no live
        /// retranslation.
        #[qinvokable]
        #[cxx_name = "uiLocale"]
        fn ui_locale(self: &AppSettings) -> QString;

        /// Persist the chosen UI locale (Language settings page, on OK).
        #[qinvokable]
        #[cxx_name = "saveUiLocale"]
        fn save_ui_locale(self: &AppSettings, locale: &QString);

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

        /// Editor minimap (code map), on with every overlay by default.
        #[qinvokable]
        #[cxx_name = "minimapOptions"]
        fn minimap_options(self: &AppSettings) -> FfiMinimapOptions;

        /// Persist the minimap options (the Editor page, live preview + on
        /// OK).
        #[qinvokable]
        #[cxx_name = "saveMinimapOptions"]
        fn save_minimap_options(self: &AppSettings, options: &FfiMinimapOptions);

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

        /// The `[tab_padding]` section of the layer the dialog is editing
        /// (`settingsScope()`), for the Settings > Tabs page. Always
        /// resolved, never absent.
        #[qinvokable]
        #[cxx_name = "tabPadding"]
        fn tab_padding(self: &AppSettings) -> FfiTabPadding;

        /// Persist the whole `[tab_padding]` section together, on OK.
        /// Writes to the global file or the project's, whichever
        /// `settingsScope()` names, and refuses (rather than silently
        /// clamping) a side over `app_config::tab_padding::MAX_TAB_PADDING`,
        /// reporting a typed error (ADR-0003) the page shows.
        #[qinvokable]
        #[cxx_name = "saveTabPadding"]
        fn save_tab_padding(self: &AppSettings, padding: &FfiTabPadding) -> FfiResult;

        /// The tab padding actually in force: the global default with the
        /// open project's override applied, if it has one — what the
        /// stylesheet builder applies, as opposed to `tabPadding()`'s
        /// dialog-editing draft.
        #[qinvokable]
        #[cxx_name = "resolvedTabPadding"]
        fn resolved_tab_padding(self: &AppSettings) -> FfiTabPadding;

        /// The terminal's effective font (T3): the project-resolved
        /// `[terminal]` override when one is set, else the editor font —
        /// always resolved, never empty/zero, so the view never re-derives
        /// this precedence itself.
        #[qinvokable]
        #[cxx_name = "terminalFont"]
        fn terminal_font(self: &AppSettings) -> FfiEditorFont;

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

        /// Does `haystack` contain `query`, ignoring case and diacritics
        /// (issue #233)? Backs the settings dialog's search box; the view
        /// harvests each page's visible label text and calls this per
        /// label rather than the dialog owning any text-matching rules
        /// itself.
        #[qinvokable]
        #[cxx_name = "settingsSearchMatches"]
        fn settings_search_matches(self: &AppSettings, haystack: &QString, query: &QString)
            -> bool;

        /// The `[containers]` section's own flat fields (ADR-0055), for the
        /// layer `settingsScope()` names — not the connection list itself,
        /// see `containerConnections`.
        #[qinvokable]
        #[cxx_name = "containerSettings"]
        fn container_settings(self: &AppSettings) -> FfiContainerSettings;

        /// Persist the `[containers]` scalar fields together, on OK.
        #[qinvokable]
        #[cxx_name = "saveContainerSettings"]
        fn save_container_settings(
            self: &AppSettings,
            settings: &FfiContainerSettings,
        ) -> FfiResult;

        /// Every saved connection in the layer `settingsScope()` names, in
        /// the order they were added.
        #[qinvokable]
        #[cxx_name = "containerConnections"]
        fn container_connections(self: &AppSettings) -> Vec<FfiContainerConnection>;

        /// Replace the whole connection list at once — the Containers
        /// page's add/remove/edit all write through this on OK, the same
        /// whole-list-replace shape `FileAssociationsEditor::setGlobalRules`
        /// uses rather than a draft-and-commit editor: a connection row has
        /// no live side effect (unlike a language server's process) for a
        /// draft to protect against.
        #[qinvokable]
        #[cxx_name = "saveContainerConnections"]
        fn save_container_connections(
            self: &AppSettings,
            connections: Vec<FfiContainerConnection>,
        ) -> FfiResult;

        /// Every connection this build can find on its own: the engine
        /// CLIs' contexts/connections/machines, plus the Colima/Rancher
        /// Desktop/rootless-podman socket presets — feeds "Add from
        /// contexts...". Runs the discovery CLIs synchronously; unlike
        /// `testContainerConnection` this has no daemon to hang on, only a
        /// context/machine list, so it does not need a worker thread.
        #[qinvokable]
        #[cxx_name = "discoverContainerConnections"]
        fn discover_container_connections(self: &AppSettings) -> Vec<FfiDiscoveredConnection>;

        /// Probe `connection` ("Test connection"): runs `<cli> version
        /// --format json` on a worker thread and reports through
        /// `containerConnectionTested`, never blocking the UI thread on a
        /// daemon that may be unreachable — the same worker-thread-plus-
        /// `qt_thread().queue()` shape `TestService::run_all` uses for a
        /// test process. `connection` need not be saved yet: the page tests
        /// a row as it is being edited.
        #[qinvokable]
        #[cxx_name = "testContainerConnection"]
        fn test_container_connection(
            self: Pin<&mut AppSettings>,
            connection: &FfiContainerConnection,
        ) -> FfiResult;

        /// Every saved registry in the layer `settingsScope()` names (C7).
        #[qinvokable]
        #[cxx_name = "registries"]
        fn registries(self: &AppSettings) -> Vec<FfiRegistrySetting>;

        /// Replace the whole registry list at once, same whole-list-replace
        /// shape as `saveContainerConnections` — a registry row has no
        /// live side effect either.
        #[qinvokable]
        #[cxx_name = "saveRegistries"]
        fn save_registries(self: &AppSettings, registries: Vec<FfiRegistrySetting>) -> FfiResult;

        /// Every run target in the layer `settingsScope()` names (C8) —
        /// the Settings > Containers > Run targets list.
        #[qinvokable]
        #[cxx_name = "containerTargets"]
        fn container_targets(self: &AppSettings) -> Vec<FfiContainerTarget>;

        /// Replace the whole run-target list at once, same whole-list-
        /// replace shape as `saveContainerConnections`/`saveRegistries` —
        /// Edit/Remove and the New Target wizard's Finish all write through
        /// this.
        #[qinvokable]
        #[cxx_name = "saveContainerTargets"]
        fn save_container_targets(
            self: &AppSettings,
            targets: Vec<FfiContainerTarget>,
        ) -> FfiResult;

        /// "Test connection" for a registry (C7): runs on a worker thread
        /// (the same shape as `testContainerConnection`) and reports
        /// through `registryTested`. `registry` need not be saved yet;
        /// `secret` is the password/token as currently typed, never read
        /// from the keychain — the Settings page tests what is on screen.
        #[qinvokable]
        #[cxx_name = "testRegistryConnection"]
        fn test_registry_connection(
            self: Pin<&mut AppSettings>,
            registry: &FfiRegistrySetting,
            secret: &QString,
        ) -> FfiResult;

        /// Store `secret` in the OS keychain for `id` (Apply/OK on the
        /// Registries page, only when the password/token field was
        /// touched). A `SecretError::Unavailable` surfaces as an ordinary
        /// `FfiResult` failure carrying the documented "run `docker
        /// login`" hint, not a crash — push/pull still work either way.
        #[qinvokable]
        #[cxx_name = "storeRegistrySecret"]
        fn store_registry_secret(self: &AppSettings, id: &QString, secret: &QString) -> FfiResult;

        /// Whether a secret is currently stored for `id`, without reading
        /// it back — the Registries page's "a password is stored" hint.
        #[qinvokable]
        #[cxx_name = "hasRegistrySecret"]
        fn has_registry_secret(self: &AppSettings, id: &QString) -> bool;

        /// C7 review follow-up: "Remove" on a registry tree node. Strips
        /// the `RegistrySetting` with this `id` from whichever layer
        /// `settingsScope()` names (the same scope `saveRegistries` writes)
        /// and deletes its keychain entry — never one without the other,
        /// so a removed registry leaves nothing behind either the tree or
        /// the tree's own "Edit..." can bring back by surprise.
        #[qinvokable]
        #[cxx_name = "removeRegistry"]
        fn remove_registry(self: &AppSettings, id: &QString) -> FfiResult;

        /// Every enabled plugin's `tool-windows` contribution (the
        /// database-tools plan's G1), in registry (load) order — what used
        /// to be the literal `wireBuildToolsDock`/`buildContainersDock`
        /// calls in `main_window.cpp` before both migrated onto this point.
        /// A disabled plugin's rows are absent, which is what makes
        /// disabling one hide its dock and View-menu entry.
        #[qinvokable]
        #[cxx_name = "contributedToolWindows"]
        fn contributed_tool_windows(self: &AppSettings) -> Vec<FfiToolWindow>;

        /// Every enabled plugin's `settings-pages` contribution, same
        /// ordering and disabled-filtering rule as
        /// [`contributed_tool_windows`](Self::contributed_tool_windows).
        #[qinvokable]
        #[cxx_name = "contributedSettingsPages"]
        fn contributed_settings_pages(self: &AppSettings) -> Vec<FfiSettingsPage>;
    }

    unsafe extern "RustQt" {
        /// The scope selector changed which layer is being edited. Every
        /// open settings page reloads its draft from the layer now selected;
        /// a page that ignored this would show one layer's values and save
        /// them into the other.
        #[qsignal]
        #[cxx_name = "settingsScopeChanged"]
        fn settings_scope_changed(self: Pin<&mut AppSettings>);

        /// `testContainerConnection`'s result: `ok`/`message` shaped like
        /// `FfiResult` (a success carries the "Engine X.Y.Z" line in
        /// `message`, a failure carries the JetBrains-troubleshooting-style
        /// hint) rather than reusing `FfiResult` itself, since this crosses
        /// as a signal argument rather than a return value.
        #[qsignal]
        #[cxx_name = "containerConnectionTested"]
        fn container_connection_tested(self: Pin<&mut AppSettings>, ok: bool, message: QString);

        /// `testRegistryConnection`'s result, same `ok`/`message` shape as
        /// `containerConnectionTested`.
        #[qsignal]
        #[cxx_name = "registryTested"]
        fn registry_tested(self: Pin<&mut AppSettings>, ok: bool, message: QString);
    }

    impl cxx_qt::Threading for AppSettings {}

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
        ///
        /// R8: `mask` is a comma-separated glob list (`*.rs, !*_test.rs`,
        /// `index_core::FileMask` syntax) — empty means no mask.
        /// `scope_paths` restricts the search to those files/directories —
        /// empty means the whole project, which is how the view expresses
        /// the Project/Directory/Open Files/Current File scope combo
        /// without a dedicated enum crossing the seam: it already has to
        /// resolve "Open Files" to a path list to show it, so the bridge
        /// just takes the resolved list either way.
        #[qinvokable]
        #[cxx_name = "search"]
        fn search(
            self: Pin<&mut SearchModel>,
            pattern: &QString,
            options: FfiSearchOptions,
            mask: &QString,
            scope_paths: &QStringList,
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
        /// (including when there were zero matches). `total_hint` is R8's
        /// "N of M": equal to the number of matches actually sent when the
        /// result wasn't capped, and larger than it when the 10,000-match
        /// ceiling cut the scan short — the view shows "showing N of M,
        /// refine your search" exactly when the two differ.
        #[qsignal]
        #[cxx_name = "searchFinished"]
        fn search_finished(self: Pin<&mut SearchModel>, generation: u64, total_hint: u32);

        /// Emitted instead of `searchFinished` when `search` couldn't run
        /// at all (no index built yet, or an invalid regex pattern).
        #[qsignal]
        #[cxx_name = "searchFailed"]
        fn search_failed(self: Pin<&mut SearchModel>, generation: u64, message: QString);

        /// Structure's project-wide tier (Task I): list every indexed
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
        /// from Structure lands on the identifier rather than at column
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

        /// R8: like `findUsages`, but tries a running language server's
        /// `textDocument/references` first (`path`/`line`/`character` are
        /// the caret's LSP position, the same convention
        /// `requestIntentions`/`signatureHelpAt` use) and only falls back
        /// to the name-based index when the server has nothing — no server
        /// for the language, none running, an error, or an empty answer
        /// (`lsp_core::prefer_lsp_references`). Answers on the same
        /// `usagesFound`/`usagesFinished`/`usagesFailed` trio as
        /// `findUsages`.
        #[qinvokable]
        #[cxx_name = "usagesAt"]
        fn usages_at(
            self: Pin<&mut SearchModel>,
            name: &QString,
            path: &QString,
            line: u32,
            character: u32,
        );

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
        /// `cpp/`. Returns the cached catalogue — instant once
        /// `refreshShells()` has landed once; detects synchronously exactly
        /// once before that, rather than showing an empty menu.
        #[qinvokable]
        #[cxx_name = "availableShells"]
        fn available_shells(self: &TerminalSupervisor) -> Vec<FfiShellCandidate>;

        /// Detect this machine's shells on a background thread and cache
        /// the result, emitting `shellsChanged()` once it lands. Never
        /// blocks the Qt thread — a `wsl.exe --list` round trip on Windows
        /// takes 1-3s, which used to run on the Qt thread every time the
        /// "+" dropdown opened. Call from the panel's constructor and again
        /// each time the dropdown is about to show, so a WSL distro
        /// installed while the IDE is running still turns up without a
        /// restart.
        #[qinvokable]
        #[cxx_name = "refreshShells"]
        fn refresh_shells(self: Pin<&mut TerminalSupervisor>);

        /// Apply a palette (T3) to every open session, live, and remember it
        /// for every session started afterward. `theme.cpp`'s
        /// `terminalPaletteForTheme()` builds the argument; this call never
        /// decides the colors itself.
        #[qinvokable]
        #[cxx_name = "setPalette"]
        fn set_palette(self: Pin<&mut TerminalSupervisor>, palette: FfiTerminalPalette);

        /// Containers plan C3: set the exact command `session_id`'s next
        /// `start()` spawns, in preference to `shell_for`'s "which local
        /// shell" rule — `ContainerService::*SessionCommand` is the one
        /// caller, for a Log/Terminal/Exec/Attach tab over `docker`/
        /// `podman`. `args` is `\n`-separated, `env` is
        /// `KEY=VALUE\n`-separated (`FfiRunConfig::{args,env}`'s own
        /// convention — no bare `Vec<QString>` on the seam). One-shot:
        /// consumed by the next `start()` on this id, then a later
        /// `start()` (reopening a closed tab) falls back to `shell_for`
        /// again.
        #[qinvokable]
        #[cxx_name = "setCommand"]
        fn set_command(
            self: Pin<&mut TerminalSupervisor>,
            session_id: u64,
            program: &QString,
            args: &QString,
            env: &QString,
        );

        /// Forward keystrokes (already translated to the byte sequence a
        /// shell expects by the view) to `session_id`'s PTY stdin.
        #[qinvokable]
        #[cxx_name = "write"]
        fn write(self: Pin<&mut TerminalSupervisor>, session_id: u64, input: &QString);

        /// Translate one key press to the xterm bytes a shell expects
        /// (`terminal_core::keys::encode`, honoring `session_id`'s current
        /// application-cursor-key mode) and write them to its PTY stdin
        /// (Task T4). `code_point` is a Unicode code point for
        /// `FfiTerminalKey::Char`, or the function-key number (1-12) for
        /// `FfiTerminalKey::F`; meaningless for every other variant.
        #[qinvokable]
        #[cxx_name = "sendKey"]
        fn send_key(
            self: Pin<&mut TerminalSupervisor>,
            session_id: u64,
            key: FfiTerminalKey,
            code_point: u32,
            shift: bool,
            ctrl: bool,
            alt: bool,
        );

        /// Resize both `session_id`'s PTY and grid — call from
        /// `cpp/terminal_widget.cpp`'s `resizeEvent` whenever the
        /// font-metrics-derived row/column count actually changes.
        #[qinvokable]
        #[cxx_name = "resize"]
        fn resize(self: Pin<&mut TerminalSupervisor>, session_id: u64, rows: u32, cols: u32);

        /// Scroll `session_id`'s viewport by `delta` lines (Task T5):
        /// positive moves up into history, negative moves back toward live
        /// output. The raw wheel/keyboard gesture — `terminal-core` clamps
        /// the result, so the view never has to. `&self`, not
        /// `Pin<&mut Self>`: the emulator this mutates lives behind the
        /// `Arc<Mutex<..>>` `with_emulator` locks, the same reasoning
        /// `selectionStart`/`selectionUpdate` already document. The caller
        /// must still set `snapshotStale_` and repaint — this is a
        /// synchronous, widget-driven call, not PTY output, so it does not
        /// go through `gridUpdated`.
        #[qinvokable]
        #[cxx_name = "scroll"]
        fn scroll(self: &TerminalSupervisor, session_id: u64, delta: i32);

        /// Scroll `session_id`'s viewport to an absolute offset from the
        /// bottom (0 = live) — what dragging the scrollbar thumb to a
        /// position means (Task T5).
        #[qinvokable]
        #[cxx_name = "scrollTo"]
        fn scroll_to(self: &TerminalSupervisor, session_id: u64, offset: u64);

        /// Snap `session_id`'s viewport back to live output (Task T5) —
        /// called after every keystroke (`sendKey`/`write`) and by
        /// Shift+End, matching every other terminal.
        #[qinvokable]
        #[cxx_name = "scrollToBottom"]
        fn scroll_to_bottom(self: &TerminalSupervisor, session_id: u64);

        /// How far back `session_id`'s history goes, and how far the
        /// viewport is currently scrolled into it (Task T5) — what the
        /// scrollbar's range/value are derived from.
        #[qinvokable]
        #[cxx_name = "scrollState"]
        fn scroll_state(self: &TerminalSupervisor, session_id: u64) -> FfiScrollState;

        /// Whether `session_id`'s running application is on the alternate
        /// screen (Task T5) — `vim`/`less`/other full-screen TUIs. The
        /// widget reads this to decide whether the mouse wheel should
        /// scroll history (normal screen) or send arrow keys to the app
        /// (alt screen), matching every other terminal.
        #[qinvokable]
        #[cxx_name = "altScreen"]
        fn alt_screen(self: &TerminalSupervisor, session_id: u64) -> bool;

        /// Pull-based grid read (Qt thread only — never touches the PTY):
        /// `cpp/terminal_widget.cpp`'s paint routine calls this once,
        /// caches the result, and only calls it again when its
        /// `snapshotStale_` flag says the cache is out of date (T2) — set by
        /// `gridUpdated`, a selection change, or a resize. One call replaces
        /// what used to be five (`gridCells`/`gridRows`/`gridCols`/
        /// `cursorRow`/`cursorCol`), each re-snapshotting the grid on every
        /// single repaint.
        #[qinvokable]
        #[cxx_name = "snapshot"]
        fn snapshot(self: &TerminalSupervisor, session_id: u64) -> FfiTerminalSnapshot;

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

        /// Emitted on the Qt thread once `refreshShells()`'s background
        /// detect has landed and the cache `availableShells()` reads is
        /// up to date. The dock's shell menu rebuilds from this only while
        /// it is visible; the settings page's combo, built once per dialog
        /// open, does not listen.
        #[qsignal]
        #[cxx_name = "shellsChanged"]
        fn shells_changed(self: Pin<&mut TerminalSupervisor>);

        /// Emitted on the Qt thread once `session_id`'s PTY child has
        /// actually exited (`PtySession::try_wait`, checked right after the
        /// reader thread sees EOF) — `exitCode` is the child's own code.
        /// Every session's widget gets this regardless of kind; a Pull/
        /// Push console closes itself on `exitCode == 0`, an interactive
        /// Log/Terminal/Exec/Attach tab ignores it (ADR-0055's C9: closing
        /// those would drop a shell's own scrollback the user is still
        /// reading).
        #[qsignal]
        #[cxx_name = "sessionExited"]
        fn session_exited(self: Pin<&mut TerminalSupervisor>, session_id: u64, exit_code: u32);
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

        /// The primary caret's byte offset into the tab's buffer — the same
        /// unit `db_sql::split`'s spans use, so a caller can hand this
        /// straight to `ConsoleService::execute`'s `caret` parameter without
        /// its own UTF-16-to-byte conversion (database-tools-plan F3e).
        /// `0` for a tab this object has never seen a caret move for.
        #[qinvokable]
        #[cxx_name = "caretOffset"]
        fn caret_offset(self: &EditorOps, tab_id: u64) -> i64;

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

        /// The bracket at `position` and its partner, for the live pair
        /// highlight — unlike `matchingBracket`, names an unmatched bracket
        /// too rather than answering nothing.
        #[qinvokable]
        #[cxx_name = "bracketPairAt"]
        fn bracket_pair_at(
            self: &EditorOps,
            tab_id: u64,
            text: &QString,
            position: u32,
        ) -> FfiBracketPair;

        /// Left/Right/Up/Down/Home/End/word-move with more than one caret
        /// active: every caret moves, not just the primary (ADR-0023
        /// follow-up). `motion` is `CodeEditor`'s own constant; `extend` is
        /// Shift held.
        #[qinvokable]
        #[cxx_name = "moveCarets"]
        fn move_carets(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            text: &QString,
            motion: u8,
            extend: bool,
        );

        /// The column this tab's language wants the wrap guide at, `0` for
        /// never.
        #[qinvokable]
        #[cxx_name = "wrapColumnForTab"]
        fn wrap_column_for_tab(self: &EditorOps, tab_id: u64) -> u32;

        /// Whether this tab's language wants text reflowed at that column.
        #[qinvokable]
        #[cxx_name = "softWrapForTab"]
        fn soft_wrap_for_tab(self: &EditorOps, tab_id: u64) -> bool;

        /// The cached global soft-wrap setting, for the View menu's toggle
        /// to show its current state when built.
        #[qinvokable]
        #[cxx_name = "softWrapEnabled"]
        fn soft_wrap_enabled(self: &EditorOps) -> bool;

        /// View > Soft Wrap: flips and persists the global soft-wrap
        /// setting, returning the new state.
        #[qinvokable]
        #[cxx_name = "toggleSoftWrap"]
        fn toggle_soft_wrap(self: Pin<&mut EditorOps>) -> bool;

        /// R2: a snippet was just accepted and its text spliced in —
        /// `stops` are the absolute document positions each tab stop landed
        /// at (`FfiSnippetStop`'s own doc comment). Starts a session for
        /// this tab (replacing any it already had) and returns the first
        /// stop to select; `has_stop` false means the snippet had no tab
        /// stops at all, so there is nothing to begin.
        #[qinvokable]
        #[cxx_name = "beginSnippet"]
        fn begin_snippet(
            self: Pin<&mut EditorOps>,
            tab_id: u64,
            stops: Vec<FfiSnippetStop>,
        ) -> FfiSnippetStop;

        /// Tab (`backward == false`) or Shift+Tab: move this tab's snippet
        /// session, if it has one, and return the stop now current.
        /// `has_stop` false means either there is no session or the move
        /// had nowhere to go (Tab past the last stop, Shift+Tab before the
        /// first) — either way the keystroke falls through to its ordinary
        /// meaning. Landing on the last stop ends the session (`more ==
        /// false`), per R2's target.
        #[qinvokable]
        #[cxx_name = "stepSnippet"]
        fn step_snippet(self: Pin<&mut EditorOps>, tab_id: u64, backward: bool) -> FfiSnippetStop;

        /// Escape, or the caret left the snippet some other way: drop this
        /// tab's session, if any.
        #[qinvokable]
        #[cxx_name = "endSnippet"]
        fn end_snippet(self: Pin<&mut EditorOps>, tab_id: u64);

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

        /// R2: `acceptCompletion` accepted a snippet item. `start`/
        /// `start_character` are the protocol position (0-based line,
        /// UTF-16 character) the flattened text was inserted at —
        /// `FfiSnippetStop`'s own doc comment says what `stops` are
        /// relative to. Never fired for a plain item; the view only wires
        /// this up when there is a session to begin.
        #[qsignal]
        #[cxx_name = "snippetReady"]
        fn snippet_ready(
            self: Pin<&mut LanguageService>,
            start_line: u32,
            start_character: u32,
            stops: Vec<FfiSnippetStop>,
        );

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

        /// The overload `requestSignatureHelp` last resolved (or R3's
        /// `cycleSignatureOverload` last stepped to), or the default
        /// (`has_signature: false`) when there is nothing to show.
        #[qinvokable]
        #[cxx_name = "signatureHelp"]
        fn signature_help(self: &LanguageService) -> FfiSignatureHelp;

        /// R3: Up (`delta = -1`) / Down (`delta = 1`) on the signature
        /// tip — steps `signatureHelp`'s next answer to another overload of
        /// the same call, wrapping past either end. Read again with
        /// `signatureHelp` and repaint; there is no signal of its own
        /// because the caller already knows synchronously.
        #[qinvokable]
        #[cxx_name = "cycleSignatureOverload"]
        fn cycle_signature_overload(self: Pin<&mut LanguageService>, delta: i32);

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

        /// C6: the connected engines' image names, `\n`-joined, for the
        /// local half of image-name completion on `FROM`/`image:` lines.
        /// Pushed by the view from `ContainerService::localImageNames` on
        /// each `treeChanged`, so the two QObjects never reach into each
        /// other.
        #[qinvokable]
        #[cxx_name = "setLocalImages"]
        fn set_local_images(self: Pin<&mut LanguageService>, names: &QString);

        /// C6: an intention of kind `container.*` was applied — `kind`
        /// (`container.pull`) and its payload (the image reference). The
        /// view wires it to `ContainerService::pullImage`; nothing here
        /// knows how a pull happens.
        #[qsignal]
        #[cxx_name = "containerActionRequested"]
        fn container_action_requested(
            self: Pin<&mut LanguageService>,
            kind: QString,
            payload: QString,
        );

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

    extern "RustQt" {
        /// The one Problems model (ADR-0046): every diagnostic, from every
        /// source, that `LanguageService` and `BuildService` have published
        /// into the shared store. The Problems dock and the editor's
        /// squiggles read only this — `LanguageService`/`BuildService` keep
        /// their own `diagnosticsChanged` signals (meaning "my part of the
        /// store changed"), but no longer answer "what are the
        /// diagnostics" themselves.
        ///
        /// No worker thread of its own — reading the shared store never
        /// blocks — so this QObject has no `cxx_qt::Threading` impl.
        #[qobject]
        type DiagnosticsService = super::DiagnosticsServiceRust;

        /// Every known diagnostic, grouped by file and ordered within it.
        #[qinvokable]
        fn diagnostics(self: &DiagnosticsService) -> Vec<FfiDiagnostic>;

        /// Just one file's diagnostics — what an editor underlines.
        #[qinvokable]
        #[cxx_name = "diagnosticsForFile"]
        fn diagnostics_for_file(self: &DiagnosticsService, path: &QString) -> Vec<FfiDiagnostic>;

        /// Counts per severity, for the status bar and the filter buttons.
        #[qinvokable]
        #[cxx_name = "diagnosticCounts"]
        fn diagnostic_counts(self: &DiagnosticsService) -> FfiDiagnosticCounts;

        /// R4/F2: the next diagnostic in `path` strictly after
        /// `(line, character)`, wrapping to the file's first.
        #[qinvokable]
        #[cxx_name = "nextDiagnostic"]
        fn next_diagnostic(
            self: &DiagnosticsService,
            path: &QString,
            line: u32,
            character: u32,
        ) -> FfiDiagnosticJump;

        /// R4/Shift+F2: the previous diagnostic in `path` strictly before
        /// `(line, character)`, wrapping to the file's last.
        #[qinvokable]
        #[cxx_name = "previousDiagnostic"]
        fn previous_diagnostic(
            self: &DiagnosticsService,
            path: &QString,
            line: u32,
            character: u32,
        ) -> FfiDiagnosticJump;

        /// R4: how many of each severity `path` alone has — the error
        /// stripe's corner summary.
        #[qinvokable]
        #[cxx_name = "diagnosticSummary"]
        fn diagnostic_summary(self: &DiagnosticsService, path: &QString) -> FfiDiagnosticCounts;
    }

    /// `analysis_core::AnalyzerStatus`'s discriminant, crossed separately
    /// from its sentence (`FfiAnalyzerRow::status_text`) so the status
    /// bar's colour/icon choice is a `match` on this, translation the view
    /// is allowed, rather than pattern-matching English text — which would
    /// be a business decision leaking into `cpp/`.
    enum FfiAnalyzerStatusKind {
        Detected,
        DeclaredNotInstalled,
        NotDetected,
    }

    /// One row of the Analysis settings page and the status bar's
    /// per-analyzer indicator (the PHP tooling plan's B7-B9): an
    /// analyzer's configuration joined with its live detection status.
    struct FfiAnalyzerRow {
        id: QString,
        name: QString,
        enabled: bool,
        /// `settings_model::analysis::Trigger::id()` — what a settings-page
        /// edit writes back.
        #[cxx_name = "triggerId"]
        trigger_id: QString,
        /// `Trigger::label()` — what the dropdown shows.
        #[cxx_name = "triggerLabel"]
        trigger_label: QString,
        #[cxx_name = "statusKind"]
        status_kind: FfiAnalyzerStatusKind,
        /// `analysis_core::AnalyzerStatus::describe`'s sentence — detected,
        /// declared-but-not-installed, or not detected.
        #[cxx_name = "statusText"]
        status_text: QString,
    }

    extern "RustQt" {
        /// Runs analyzer jobs on worker threads (`analysis_core::Scheduler`)
        /// and publishes their findings into the one Problems model
        /// (ADR-0046), the PHP tooling plan's B8. One registered `#[qobject]`
        /// per ADR-0032's precedent.
        #[qobject]
        type AnalysisService = super::AnalysisServiceRust;

        /// Every contributed analyzer's configuration and live detection
        /// status, for the settings page and the status bar.
        #[qinvokable]
        #[cxx_name = "analyzerRows"]
        fn analyzer_rows(self: &AnalysisService) -> Vec<FfiAnalyzerRow>;

        /// Whether "Inspect Project" is already running.
        #[qinvokable]
        #[cxx_name = "isInspecting"]
        fn is_inspecting(self: &AnalysisService) -> bool;

        /// Run every enabled, installed analyzer against the whole open
        /// project (`Trigger::Manual`). Answers via `analysisStarted`, one
        /// `analyzerStarted`/`analyzerFinished` pair per analyzer, then
        /// `analysisFinished`.
        #[qinvokable]
        #[cxx_name = "inspectProject"]
        fn inspect_project(self: Pin<&mut AnalysisService>) -> FfiResult;

        /// A project-wide analysis run began.
        #[qsignal]
        #[cxx_name = "analysisStarted"]
        fn analysis_started(self: Pin<&mut AnalysisService>);

        /// One analyzer in the batch started running.
        #[qsignal]
        #[cxx_name = "analyzerStarted"]
        fn analyzer_started(self: Pin<&mut AnalysisService>, analyzer_id: QString);

        /// One analyzer in the batch finished. `ok` is false for a run
        /// failure (not found, timed out, an I/O error) — never for the
        /// tool having found something to report, which is success.
        #[qsignal]
        #[cxx_name = "analyzerFinished"]
        fn analyzer_finished(
            self: Pin<&mut AnalysisService>,
            analyzer_id: QString,
            ok: bool,
            message: QString,
        );

        /// The whole batch finished — every queued analyzer has reported.
        #[qsignal]
        #[cxx_name = "analysisFinished"]
        fn analysis_finished(self: Pin<&mut AnalysisService>);

        /// This analyzer's rows in the shared store (ADR-0046) changed —
        /// the same "my part of the store changed" meaning `LanguageService`
        /// and `BuildService` already give their own `diagnosticsChanged`.
        /// `EditorTabs::applyDiagnostics` and `ProblemsPanel::refresh` both
        /// wire to this alongside the other two sources, so an analyzer
        /// finding reaches the editor's squiggles and the Problems dock the
        /// same way a build's or a language server's does — the finding-1
        /// bug class ADR-0046 exists to prevent, for this third source too.
        #[qsignal]
        #[cxx_name = "diagnosticsChanged"]
        fn diagnostics_changed(self: Pin<&mut AnalysisService>);
    }

    impl cxx_qt::Threading for AnalysisService {}

    /// One row of the Analysis settings page (B9): an analyzer's enabled
    /// flag and trigger, as edited by `AnalysisEditor`.
    struct FfiAnalysisRow {
        id: QString,
        name: QString,
        enabled: bool,
        #[cxx_name = "triggerId"]
        trigger_id: QString,
        #[cxx_name = "triggerLabel"]
        trigger_label: QString,
    }

    extern "RustQt" {
        /// Settings > Analysis (B9): the draft the page edits, following
        /// `LanguageServerEditor`'s begin_edit(scope)/rows/set_*/is_dirty/
        /// commit shape.
        #[qobject]
        type AnalysisEditor = super::AnalysisEditorRust;

        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &AnalysisEditor, scope: &QString);

        #[qinvokable]
        fn rows(self: &AnalysisEditor) -> Vec<FfiAnalysisRow>;

        #[qinvokable]
        #[cxx_name = "setEnabled"]
        fn set_enabled(self: &AnalysisEditor, id: &QString, enabled: bool);

        #[qinvokable]
        #[cxx_name = "setTrigger"]
        fn set_trigger(self: &AnalysisEditor, id: &QString, trigger_id: &QString);

        #[qinvokable]
        #[cxx_name = "isDirty"]
        fn is_dirty(self: &AnalysisEditor, id: &QString) -> bool;

        #[qinvokable]
        fn commit(self: &AnalysisEditor);
    }

    /// A Build Tools dock row's kind (the jvm-build-tools plan's B1/B2) —
    /// `jvm_build_core::view::NodeKind` crossed the seam.
    enum FfiBuildToolNodeKind {
        ToolRoot,
        Group,
        Task,
        Module,
        SourceRoot,
        Dependency,
        /// A Maven profile id, checkable (B3).
        Profile,
        /// A Maven plugin bound into the build (review fix: Maven's own
        /// Lifecycle/Plugins/Dependencies/Profiles vocabulary).
        Plugin,
        /// A goal one Maven plugin's `<executions>` binds.
        Goal,
    }

    /// One row of the Build Tools dock's tree, flattened and
    /// parent-qualified like `FfiTestNode` — a `QTreeWidget` builds its own
    /// hierarchy from `parentId` rather than nesting a `Vec` inside a `Vec`.
    struct FfiBuildToolNode {
        id: QString,
        #[cxx_name = "parentId"]
        parent_id: QString,
        kind: FfiBuildToolNodeKind,
        label: QString,
        /// A task's description, a source root's kind/content, or a
        /// dependency's conflict — whatever `jvm_build_core::view::Node`
        /// itself carries for this row's kind; empty when it carries
        /// nothing.
        detail: QString,
        /// `"gradle"` or `"maven"` (`Tool::toolchain_id()`), for `cpp/` to
        /// pick an icon by — never used as text (ADR-0049: Rust never
        /// emits user-visible strings).
        tool: QString,
        /// The build file "Open Build File" opens — empty for a row that
        /// names none (only a `Module` row carries one today).
        #[cxx_name = "buildFile"]
        build_file: QString,
        /// Meaningful only for `FfiBuildToolNodeKind::Profile`: whether the
        /// dock's checkbox for this profile is ticked.
        checked: bool,
    }

    /// The dock's title (computed in Rust from which tools synced
    /// successfully) — `cpp/` maps this to a `tr()` string, never Rust.
    enum FfiBuildToolTitleKind {
        None,
        Gradle,
        Maven,
        Both,
    }

    /// A sync's own state — `cpp/` maps this to a `tr()` string via
    /// `syncMessage()` for the `Failed` case.
    enum FfiSyncStateKind {
        Idle,
        Syncing,
        Failed,
    }

    /// The editor banner's state (B4): which prompt, if any, is showing
    /// above the editor area. `cpp/` maps this to `tr()` text and buttons.
    enum FfiBannerKind {
        None,
        TrustGradle,
        TrustMaven,
        ReloadNeeded,
    }

    extern "RustQt" {
        /// Detects Gradle/Maven, syncs on a worker thread and shapes the
        /// result into the dock's tree (the jvm-build-tools plan's B1).
        /// One registered `#[qobject]`, mirroring `AnalysisService`'s shape.
        #[qobject]
        type BuildToolsService = super::BuildToolsServiceRust;

        /// The dock's whole tree, flattened (B2).
        #[qinvokable]
        fn rows(self: &BuildToolsService) -> Vec<FfiBuildToolNode>;

        #[qinvokable]
        #[cxx_name = "titleKind"]
        fn title_kind(self: &BuildToolsService) -> FfiBuildToolTitleKind;

        #[qinvokable]
        #[cxx_name = "syncStateKind"]
        fn sync_state_kind(self: &BuildToolsService) -> FfiSyncStateKind;

        /// The last sync's failure message — empty unless `syncStateKind`
        /// is `Failed`.
        #[qinvokable]
        #[cxx_name = "syncMessage"]
        fn sync_message(self: &BuildToolsService) -> QString;

        #[qinvokable]
        #[cxx_name = "bannerKind"]
        fn banner_kind(self: &BuildToolsService) -> FfiBannerKind;

        #[qinvokable]
        #[cxx_name = "dismissBanner"]
        fn dismiss_banner(self: Pin<&mut BuildToolsService>);

        #[qinvokable]
        #[cxx_name = "setOffline"]
        fn set_offline(self: &BuildToolsService, offline: bool);

        #[qinvokable]
        #[cxx_name = "setSkipTests"]
        fn set_skip_tests(self: &BuildToolsService, skip_tests: bool);

        /// A Maven profile checkbox was toggled (B3) — fed into every
        /// task/goal run's `-P<id>` flags from here on.
        #[qinvokable]
        #[cxx_name = "setProfileChecked"]
        fn set_profile_checked(self: Pin<&mut BuildToolsService>, profile: &QString, checked: bool);

        /// A project opened (or reopened): detect Gradle/Maven and either
        /// show the trust banner or sync automatically for an
        /// already-trusted root.
        #[qinvokable]
        #[cxx_name = "projectOpened"]
        fn project_opened(self: Pin<&mut BuildToolsService>, root: &QString);

        /// "Load" on the trust banner (ADR-0057 §3): trusts the project
        /// root in the **global** settings file, then syncs.
        #[qinvokable]
        #[cxx_name = "trustAndLoad"]
        fn trust_and_load(self: Pin<&mut BuildToolsService>) -> FfiResult;

        /// A watched build file changed on disk (`main_window.cpp`'s
        /// `watchedFileChanged` relay).
        #[qinvokable]
        #[cxx_name = "fileChanged"]
        fn file_changed(self: Pin<&mut BuildToolsService>, path: &QString);

        /// The user saved a build file open in a tab
        /// (`editor_tabs.cpp`'s `documentSaved` relay).
        #[qinvokable]
        #[cxx_name = "fileSaved"]
        fn file_saved(self: Pin<&mut BuildToolsService>, path: &QString);

        /// Run (or re-run) a sync of every detected, trusted tool.
        #[qinvokable]
        fn sync(self: Pin<&mut BuildToolsService>) -> FfiResult;

        /// Stop waiting on the current sync (see `syncMessage`'s own doc
        /// comment on this method's actual reach).
        #[qinvokable]
        #[cxx_name = "cancelSync"]
        fn cancel_sync(self: &BuildToolsService);

        /// The temporary `RunConfig` a double-click on task/goal row
        /// `node_id` launches (B3) — `RunService::runTemporary` takes this
        /// straight back.
        #[qinvokable]
        #[cxx_name = "taskConfig"]
        fn task_config(self: &BuildToolsService, node_id: &QString) -> FfiRunConfig;

        /// Same, with the "Execute…" field's text split into extra argv.
        #[qinvokable]
        #[cxx_name = "taskConfigWithArgs"]
        fn task_config_with_args(
            self: &BuildToolsService,
            node_id: &QString,
            extra_args: &QString,
        ) -> FfiRunConfig;

        /// D8: every distinct scope/configuration the synced model(s)
        /// declare a dependency under — the dependency analyzer's scope
        /// combo's item list.
        #[qinvokable]
        #[cxx_name = "dependencyScopes"]
        fn dependency_scopes(self: &BuildToolsService) -> QStringList;

        /// D8: the scope combo changed; an empty string means "every
        /// scope".
        #[qinvokable]
        #[cxx_name = "setDependencyScope"]
        fn set_dependency_scope(self: Pin<&mut BuildToolsService>, scope: &QString);

        /// D8: the "Conflicts only" toggle changed.
        #[qinvokable]
        #[cxx_name = "setConflictsOnly"]
        fn set_conflicts_only(self: Pin<&mut BuildToolsService>, conflicts_only: bool);

        /// D8: "Go to Declaration" for a Dependency row — the 1-based
        /// line to open its `buildFile` at, or `-1` when none is found
        /// (a transitive dependency, which never appears in the build
        /// file itself). `build_file` is the row's own `FfiBuildToolNode
        /// ::buildFile` (already on the C++ side from `rows()`) rather
        /// than something re-derived from `node_id` here — review fix
        /// #9: a `node_id`'s module-path segment is not safely
        /// reversible (a Gradle project path such as `:lib:core` embeds
        /// the same `:` the id itself uses as a separator), and the
        /// build file directly names which module's dependencies to
        /// search without guessing.
        #[qinvokable]
        #[cxx_name = "dependencyDeclarationLine"]
        fn dependency_declaration_line(
            self: &BuildToolsService,
            node_id: &QString,
            build_file: &QString,
        ) -> i32;

        #[qsignal]
        #[cxx_name = "modelChanged"]
        fn model_changed(self: Pin<&mut BuildToolsService>);

        #[qsignal]
        #[cxx_name = "syncStateChanged"]
        fn sync_state_changed(self: Pin<&mut BuildToolsService>);

        #[qsignal]
        #[cxx_name = "bannerChanged"]
        fn banner_changed(self: Pin<&mut BuildToolsService>);
    }

    impl cxx_qt::Threading for BuildToolsService {}

    /// Every field the Build Tools settings page (B5) edits — one struct
    /// rather than nineteen separate getters, the same convention a
    /// multi-field form elsewhere on this seam uses.
    #[derive(Default)]
    struct FfiBuildToolsFields {
        #[cxx_name = "gradleDistribution"]
        gradle_distribution: QString,
        #[cxx_name = "gradleHome"]
        gradle_home: QString,
        #[cxx_name = "gradleJavaHome"]
        gradle_java_home: QString,
        #[cxx_name = "gradleOffline"]
        gradle_offline: bool,
        #[cxx_name = "gradleAutoReload"]
        gradle_auto_reload: QString,
        #[cxx_name = "gradleDownloadSources"]
        gradle_download_sources: bool,
        #[cxx_name = "gradleJvmArgs"]
        gradle_jvm_args: QString,
        #[cxx_name = "mavenHome"]
        maven_home: QString,
        #[cxx_name = "mavenUserSettingsFile"]
        maven_user_settings_file: QString,
        #[cxx_name = "mavenLocalRepository"]
        maven_local_repository: QString,
        #[cxx_name = "mavenOffline"]
        maven_offline: bool,
        #[cxx_name = "mavenSkipTests"]
        maven_skip_tests: bool,
        #[cxx_name = "mavenThreads"]
        maven_threads: QString,
        #[cxx_name = "mavenAlwaysUpdateSnapshots"]
        maven_always_update_snapshots: bool,
        #[cxx_name = "mavenAutoReload"]
        maven_auto_reload: QString,
    }

    /// Which field a `FfiBuildToolsProblem` is about, for the page to
    /// highlight — `settings_model::build_tools::BuildToolsField` crossed.
    enum FfiBuildToolsField {
        GradleHome,
        GradleAutoReload,
        MavenHome,
        MavenAutoReload,
        MavenThreads,
    }

    struct FfiBuildToolsProblem {
        field: FfiBuildToolsField,
        sentence: QString,
    }

    extern "RustQt" {
        /// Settings > Build Tools (B5), global only — see
        /// `bridge::build_tools`'s module doc for why.
        #[qobject]
        type BuildToolsEditor = super::BuildToolsEditorRust;

        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &BuildToolsEditor, scope: &QString);

        #[qinvokable]
        fn fields(self: &BuildToolsEditor) -> FfiBuildToolsFields;

        #[qinvokable]
        fn problems(self: &BuildToolsEditor) -> Vec<FfiBuildToolsProblem>;

        #[qinvokable]
        #[cxx_name = "setGradleDistribution"]
        fn set_gradle_distribution(self: &BuildToolsEditor, id: &QString);

        #[qinvokable]
        #[cxx_name = "setGradleHome"]
        fn set_gradle_home(self: &BuildToolsEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setGradleJavaHome"]
        fn set_gradle_java_home(self: &BuildToolsEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setGradleOffline"]
        fn set_gradle_offline(self: &BuildToolsEditor, value: bool);

        #[qinvokable]
        #[cxx_name = "setGradleAutoReload"]
        fn set_gradle_auto_reload(self: &BuildToolsEditor, id: &QString);

        #[qinvokable]
        #[cxx_name = "setGradleDownloadSources"]
        fn set_gradle_download_sources(self: &BuildToolsEditor, value: bool);

        #[qinvokable]
        #[cxx_name = "setGradleJvmArgs"]
        fn set_gradle_jvm_args(self: &BuildToolsEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setMavenHome"]
        fn set_maven_home(self: &BuildToolsEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setMavenUserSettingsFile"]
        fn set_maven_user_settings_file(self: &BuildToolsEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setMavenLocalRepository"]
        fn set_maven_local_repository(self: &BuildToolsEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setMavenOffline"]
        fn set_maven_offline(self: &BuildToolsEditor, value: bool);

        #[qinvokable]
        #[cxx_name = "setMavenSkipTests"]
        fn set_maven_skip_tests(self: &BuildToolsEditor, value: bool);

        #[qinvokable]
        #[cxx_name = "setMavenThreads"]
        fn set_maven_threads(self: &BuildToolsEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setMavenAlwaysUpdateSnapshots"]
        fn set_maven_always_update_snapshots(self: &BuildToolsEditor, value: bool);

        #[qinvokable]
        #[cxx_name = "setMavenAutoReload"]
        fn set_maven_auto_reload(self: &BuildToolsEditor, id: &QString);

        #[qinvokable]
        #[cxx_name = "isDirty"]
        fn is_dirty(self: &BuildToolsEditor) -> bool;

        #[qinvokable]
        fn commit(self: &BuildToolsEditor) -> FfiResult;
    }

    /// Whether a Tests dock row is a suite/class grouping or a leaf test
    /// method — `test_core::NodeKind` crossed the seam.
    enum FfiTestNodeKind {
        Suite,
        Test,
    }

    /// Where a Tests dock row stands — `test_core::TestStatus` crossed the
    /// seam, kept as its own enum (rather than reusing `FfiSeverity`) since
    /// a test's states (running, skipped) have no diagnostic-severity
    /// analogue.
    enum FfiTestStatusKind {
        Failed,
        Running,
        Pending,
        Passed,
        Skipped,
    }

    /// One row of the Tests dock's tree (D4/D5): a flattened
    /// `test_core::TestNode`, parent-qualified rather than nested, since a
    /// `QTreeWidget` builds its own hierarchy from `parentId` the same way
    /// `ProjectTreeModel`'s rows do.
    struct FfiTestNode {
        id: QString,
        #[cxx_name = "parentId"]
        parent_id: QString,
        name: QString,
        kind: FfiTestNodeKind,
        status: FfiTestStatusKind,
        /// `-1` when the node has not finished (no duration yet) — a
        /// sentinel rather than a second `has_duration` bool, matching
        /// `FfiAnalyzerRow`-adjacent rows that use a sentinel for "absent"
        /// on a field a view only ever displays, never computes with.
        #[cxx_name = "durationMs"]
        duration_ms: i64,
        #[cxx_name = "hasFailure"]
        has_failure: bool,
    }

    extern "RustQt" {
        /// Runs the project's test framework on worker threads
        /// (`test_core::runner::run`), streaming TeamCity messages into a
        /// `test_core::TestTree` and publishing failures into the one
        /// Problems model (D3/ADR-0046) — the PHP tooling plan's D4. One
        /// registered `#[qobject]` owning a `HashMap` of in-flight runs,
        /// mirroring `BuildService`'s shape: a run is a single process read
        /// to completion or a stop, not a queue of short operations.
        #[qobject]
        type TestService = super::TestServiceRust;

        /// Every node of the current tree, flattened — the Tests dock's
        /// tree widget. Empty before any run this session.
        #[qinvokable]
        fn nodes(self: &TestService) -> Vec<FfiTestNode>;

        /// The failing node's assertion message, for the failure pane's
        /// header line. Empty when the node has no failure or does not
        /// exist.
        #[qinvokable]
        #[cxx_name = "failureMessage"]
        fn failure_message(self: &TestService, node_id: &QString) -> QString;

        /// The failing node's raw detail text (stack trace/diff) — where
        /// `run_core::links::resolve_link` (D5) finds a clickable
        /// `file:line`.
        #[qinvokable]
        #[cxx_name = "failureDetails"]
        fn failure_details(self: &TestService, node_id: &QString) -> QString;

        /// Resolve a `file:line` at `byte_offset` into this node's failure
        /// details — the failure pane's click-to-open, same contract as
        /// `RunService::resolveLink`.
        #[qinvokable]
        #[cxx_name = "resolveFailureLink"]
        fn resolve_failure_link(
            self: &TestService,
            node_id: &QString,
            byte_offset: u32,
        ) -> FfiResolvedLink;

        /// Whether a run is currently in flight — the toolbar's run/stop
        /// enablement.
        #[qinvokable]
        #[cxx_name = "isRunning"]
        fn is_running(self: &TestService) -> bool;

        /// Run the whole project's tests with no filter. Replaces whatever
        /// tree a previous run left behind.
        #[qinvokable]
        #[cxx_name = "runAll"]
        fn run_all(self: Pin<&mut TestService>) -> FfiResult;

        /// Rerun every currently `Failed` leaf test, built into one
        /// `--filter` alternation (D1's `filter-flag`). Refused when
        /// nothing is currently failing. Every other node's last result
        /// stays on the tree untouched.
        #[qinvokable]
        #[cxx_name = "runFailed"]
        fn run_failed(self: Pin<&mut TestService>) -> FfiResult;

        /// Rerun one node — a single test, or every test under a suite —
        /// from the tree's context menu (D6).
        #[qinvokable]
        #[cxx_name = "runNode"]
        fn run_node(self: Pin<&mut TestService>, node_id: &QString) -> FfiResult;

        /// Stop the run in progress, if any.
        #[qinvokable]
        fn stop(self: Pin<&mut TestService>);

        /// A run began.
        #[qsignal]
        #[cxx_name = "testRunStarted"]
        fn test_run_started(self: Pin<&mut TestService>);

        /// The tree changed — a status, a duration, a new node. The view
        /// re-reads `nodes()` rather than the signal carrying a payload,
        /// the same rule `AnalysisService::analyzerFinished` follows for
        /// its own list.
        #[qsignal]
        #[cxx_name = "testTreeChanged"]
        fn test_tree_changed(self: Pin<&mut TestService>);

        /// A chunk of the run's raw output, for the dock's output pane.
        #[qsignal]
        #[cxx_name = "testOutputAppended"]
        fn test_output_appended(self: Pin<&mut TestService>, text: QString);

        /// The run finished. `ok` is false for a run failure (not found,
        /// an I/O error) — a nonzero *test* exit code (failures found) is
        /// still `ok`, the same distinction `AnalysisService::
        /// analyzerFinished` draws.
        #[qsignal]
        #[cxx_name = "testRunFinished"]
        fn test_run_finished(self: Pin<&mut TestService>, ok: bool, message: QString);
    }

    impl cxx_qt::Threading for TestService {}

    /// A Containers dock row's status, as data — `container_core::tree::
    /// NodeStatus` crossed the seam. The view owns the word for each
    /// (ADR-0049: every user-visible string is `tr()`'d in C++); `Exited`
    /// reads its code from `exitCode`, `Connected`/`Error`/`Other` their
    /// text from `statusText`.
    enum FfiContainerNodeStatus {
        None,
        Disconnected,
        Connecting,
        Connected,
        Error,
        Running,
        Paused,
        Restarting,
        Exited,
        Created,
        Dead,
        Other,
    }

    /// The unit of a row's `ageValue` — `container_core::model::AgeUnit`
    /// plus `None` for "no age". The bucket is chosen in Rust, the word
    /// (`h`, `d`, plural forms) in the view.
    enum FfiAgeUnit {
        None,
        Seconds,
        Minutes,
        Hours,
        Days,
        Months,
        Years,
    }

    /// One row of the Containers dock's tree (containers plan C2): a
    /// flattened `container_core::tree::TreeNode`, parent-qualified like
    /// `FfiTestNode`. `kind` is the node kind's stable id (`connection`,
    /// `containers-group`, `container`, `image`, ...) — group rows carry
    /// no `name`, the view labels them by kind. `icon` is the icon key the
    /// view looks up. Every other field is discrete data (a status code,
    /// an exit code, an age bucket, counts, bytes) or engine-supplied
    /// text (`name`, `statusText`, `detail`, `tooltip`); nothing is
    /// pre-worded in Rust.
    struct FfiContainerNode {
        id: QString,
        #[cxx_name = "parentId"]
        parent_id: QString,
        kind: QString,
        name: QString,
        #[cxx_name = "connectionId"]
        connection_id: QString,
        /// The engine's own id for the row (container/image/network/pod
        /// id, volume name, compose project/service name, connection id);
        /// empty for groups. What "Copy ID" copies.
        #[cxx_name = "resourceId"]
        resource_id: QString,
        icon: QString,
        status: FfiContainerNodeStatus,
        #[cxx_name = "statusText"]
        status_text: QString,
        #[cxx_name = "exitCode"]
        exit_code: i64,
        #[cxx_name = "ageUnit"]
        age_unit: FfiAgeUnit,
        #[cxx_name = "ageValue"]
        age_value: i64,
        /// Group rows: items in the group; networks and pods: connected
        /// containers. `-1` when not applicable.
        count: i64,
        /// Compose rows: running / total containers. `-1` when not
        /// applicable.
        running: i64,
        total: i64,
        /// Images: size in bytes. `-1` when not applicable.
        #[cxx_name = "sizeBytes"]
        size_bytes: i64,
        detail: QString,
        tooltip: QString,
        /// A `Connection` row's own Podman machine `running` flag (C9):
        /// `-1` not applicable (not a `PodmanMachine` connection, or no
        /// answer yet), `0` stopped, `1` running — same `-1`-sentinel
        /// convention `count`/`running`/`total`/`sizeBytes` already use.
        #[cxx_name = "machineRunning"]
        machine_running: i64,
    }

    /// One configured connection, for a picker (C4's Copy Image to...
    /// dialog) that needs more than just an id/name pair.
    struct FfiConnectionSummary {
        id: QString,
        name: QString,
        /// `"docker"` or `"podman"` (`Engine::id()`).
        engine: QString,
        connected: bool,
    }

    /// Where one connection stands: `state` is one of `disconnected`,
    /// `connecting`, `connected`, `error`; `message` the engine banner or
    /// the error text.
    struct FfiConnectionState {
        state: QString,
        message: QString,
    }

    /// The two dock filters, as currently in force.
    struct FfiContainerFilter {
        #[cxx_name = "showStopped"]
        show_stopped: bool,
        #[cxx_name = "showUntagged"]
        show_untagged: bool,
    }

    /// Which actions apply to a node right now (C3/C4) —
    /// `container_core::tree::actions_for`'s answer, crossed as flags. The
    /// view only reads these; the rule that produced them lives in Rust.
    #[derive(Default)]
    struct FfiNodeActions {
        #[cxx_name = "canStart"]
        can_start: bool,
        #[cxx_name = "canStop"]
        can_stop: bool,
        #[cxx_name = "canRestart"]
        can_restart: bool,
        #[cxx_name = "canPause"]
        can_pause: bool,
        #[cxx_name = "canUnpause"]
        can_unpause: bool,
        #[cxx_name = "canRemove"]
        can_remove: bool,
        /// C4: an image node's re-pull.
        #[cxx_name = "canPull"]
        can_pull: bool,
        /// C4: an image node's Tag...
        #[cxx_name = "canTag"]
        can_tag: bool,
        /// C4: an image node's Create Container...
        #[cxx_name = "canCreateContainer"]
        can_create_container: bool,
        /// C4: an image node's Copy Image to...
        #[cxx_name = "canCopy"]
        can_copy: bool,
        /// C4: a group node's Clean Up.
        #[cxx_name = "canCleanUp"]
        can_clean_up: bool,
        /// C4: a group node's Create Network.../Create Volume...
        #[cxx_name = "canCreate"]
        can_create: bool,
        /// C9: a container node's "Recreate with changes" — `false` for a
        /// compose-managed container (`container_core::recreate::
        /// is_compose_managed`), which must be edited through its compose
        /// file instead.
        #[cxx_name = "canRecreate"]
        can_recreate: bool,
    }

    /// One `history` layer (C4) — `container_core::images::Layer` crossed
    /// the seam.
    struct FfiLayer {
        id: QString,
        created: QString,
        #[cxx_name = "createdBy"]
        created_by: QString,
        #[cxx_name = "sizeBytes"]
        size_bytes: u64,
        comment: QString,
    }

    /// One label, or any other key/value row (C4): image/network/volume
    /// Labels tabs all use this same shape.
    struct FfiKeyValue {
        key: QString,
        value: QString,
    }

    /// The Create Network... dialog's fields (C4). `labels` is `\n`-joined
    /// `key=value` pairs, the same convention `FfiCommand::env` uses.
    #[derive(Default)]
    struct FfiNetworkSpec {
        name: QString,
        driver: QString,
        subnet: QString,
        gateway: QString,
        internal: bool,
        attachable: bool,
        labels: QString,
    }

    /// The Create Volume... dialog's fields (C4). `labels`/`options` are
    /// `\n`-joined `key=value` pairs.
    #[derive(Default)]
    struct FfiVolumeSpec {
        name: QString,
        driver: QString,
        labels: QString,
        options: QString,
    }

    /// An image node's Dashboard tab (C4). `tags`/`digests` are `\n`-joined;
    /// `containers` is `\n`-joined `"<name>\t<node id>"` pairs so a click
    /// selects that container node without a second lookup.
    #[derive(Default)]
    struct FfiImageDashboard {
        name: QString,
        id: QString,
        #[cxx_name = "sizeBytes"]
        size_bytes: i64,
        created: QString,
        tags: QString,
        digests: QString,
        containers: QString,
    }

    /// A network node's Dashboard tab (C4). `subnets`/`containers` are
    /// `\n`-joined (`containers` as `"<name>\t<node id>"`); `labels` is
    /// `\n`-joined `key=value`.
    #[derive(Default)]
    struct FfiNetworkDashboard {
        name: QString,
        id: QString,
        driver: QString,
        scope: QString,
        subnets: QString,
        containers: QString,
        labels: QString,
    }

    /// A container node's Dashboard tab (C9): `env`/`ports`/`mounts` are
    /// `\n`-joined lines in `container_core::recreate`'s own line formats
    /// (`format_env_lines`/`format_port_lines`/`format_mount_lines`) — the
    /// Env/Ports/Mounts tables read and write these same lines, so
    /// `recreateContainer` takes them back unchanged plus whatever the
    /// tables' Add/Edit/Remove editing did to them.
    #[derive(Default)]
    struct FfiContainerDashboard {
        name: QString,
        id: QString,
        image: QString,
        status: QString,
        env: QString,
        ports: QString,
        mounts: QString,
        network: QString,
        #[cxx_name = "restartPolicy"]
        restart_policy: QString,
    }

    /// A volume node's Dashboard tab (C4).
    #[derive(Default)]
    struct FfiVolumeDashboard {
        name: QString,
        driver: QString,
        mountpoint: QString,
        containers: QString,
        labels: QString,
    }

    /// One row of `top`'s output (C3): `cells` is `\t`-joined (no bare
    /// `Vec<QString>` on the seam — see `FfiBranch`'s doc comment), in the
    /// same column order as `processesReady`'s own `titles` argument.
    struct FfiProcessRow {
        cells: QString,
    }

    /// One entry of a Files-tab directory listing (C3) —
    /// `container_core::files::FileEntry` crossed the seam. `kind` is a
    /// stable word (`dir`, `file`, `symlink`, `other`); the icon and any
    /// wording are the view's.
    struct FfiFileEntry {
        name: QString,
        kind: QString,
        size: u64,
        #[cxx_name = "mtimeEpoch"]
        mtime_epoch: i64,
        mode: QString,
        target: QString,
    }

    /// A command ready to hand to `TerminalSupervisor::setCommand` (C3):
    /// `args`/`env` already in that call's own `\n`/`KEY=VALUE\n`
    /// convention, so the view does no parsing of its own — it only wires
    /// `newSession()` → `setCommand(..)` → `start(..)`. `Default` is the
    /// "not a container node" refusal: an empty `program` is never a
    /// startable command, so the view can treat it as a failure without a
    /// separate `FfiResult` on these getters.
    #[derive(Default)]
    struct FfiCommand {
        program: QString,
        args: QString,
        env: QString,
    }

    /// One compose code lens (C6): the status of one service (`running` of
    /// `total` containers up; `exit_code` of the first failed one, `-1`
    /// when none failed) or, with `is_open_url`, one published `host_port`
    /// of a running service. The view formats the words (`tr()`); the
    /// facts are `container_core::lenses`'s.
    #[derive(Default)]
    struct FfiComposeLens {
        line: u32,
        is_open_url: bool,
        running: i64,
        total: i64,
        exit_code: i64,
        host_port: QString,
        clickable: bool,
    }

    extern "RustQt" {
        /// The Containers dock's adapter (containers plan C2, ADR-0055):
        /// starts and stops one `container_core::watcher` per connection,
        /// forwards its events onto the Qt thread, and flattens the
        /// snapshots into rows. Owns no rule: filters, search, grouping and
        /// status text are all `container-core`'s.
        #[qobject]
        type ContainerService = super::ContainerServiceRust;

        /// Every row of the tree, parents before children. Re-read after
        /// `treeChanged`.
        #[qinvokable]
        fn nodes(self: &ContainerService) -> Vec<FfiContainerNode>;

        /// Every configured connection, for a picker (C4's Copy Image to...
        /// dialog) that needs more than a tree row.
        #[qinvokable]
        fn connections(self: &ContainerService) -> Vec<FfiConnectionSummary>;

        #[qinvokable]
        #[cxx_name = "connectionState"]
        fn connection_state(self: &ContainerService, connection_id: &QString)
            -> FfiConnectionState;

        /// The filters in force — the Filter menu's initial check state.
        #[qinvokable]
        fn filter(self: &ContainerService) -> FfiContainerFilter;

        /// Start watching a configured connection. Named `connectEngine`
        /// rather than `connect` so it cannot shadow `QObject::connect` on
        /// the generated class.
        #[qinvokable]
        #[cxx_name = "connectEngine"]
        fn connect_engine(self: Pin<&mut ContainerService>, connection_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "disconnectEngine"]
        fn disconnect_engine(
            self: Pin<&mut ContainerService>,
            connection_id: &QString,
        ) -> FfiResult;

        /// Re-snapshot one connected connection now.
        #[qinvokable]
        fn refresh(self: Pin<&mut ContainerService>, connection_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "connectAll"]
        fn connect_all(self: Pin<&mut ContainerService>);

        /// Re-snapshot every connected connection and re-read the
        /// configured list.
        #[qinvokable]
        #[cxx_name = "refreshAll"]
        fn refresh_all(self: Pin<&mut ContainerService>);

        /// Persists into the `[containers]` layer in force and re-emits
        /// `treeChanged`.
        #[qinvokable]
        #[cxx_name = "setFilter"]
        fn set_filter(
            self: Pin<&mut ContainerService>,
            show_stopped: bool,
            show_untagged: bool,
        ) -> FfiResult;

        /// Type-to-filter text; empty clears.
        #[qinvokable]
        #[cxx_name = "setSearch"]
        fn set_search(self: Pin<&mut ContainerService>, text: &QString);

        /// The rows changed — a snapshot, a state, a filter or a search.
        /// The view re-reads `nodes()`.
        #[qsignal]
        #[cxx_name = "treeChanged"]
        fn tree_changed(self: Pin<&mut ContainerService>);

        #[qsignal]
        #[cxx_name = "connectionStateChanged"]
        fn connection_state_changed(self: Pin<&mut ContainerService>, connection_id: QString);

        // --- C3: container actions -----------------------------------

        /// Which actions apply to `node_id` right now — the panel's
        /// toolbar/menu enable state. Empty/unknown node: every flag false.
        #[qinvokable]
        #[cxx_name = "nodeActions"]
        fn node_actions(self: &ContainerService, node_id: &QString) -> FfiNodeActions;

        #[qinvokable]
        #[cxx_name = "startContainer"]
        fn start_container(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "stopContainer"]
        fn stop_container(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "restartContainer"]
        fn restart_container(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "removeContainer"]
        fn remove_container(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            force: bool,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "pauseContainer"]
        fn pause_container(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "unpauseContainer"]
        fn unpause_container(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        /// The Containers group's "Clean Up" — `container prune -f` on
        /// `connection_id`.
        #[qinvokable]
        #[cxx_name = "pruneContainers"]
        fn prune_containers(self: Pin<&mut ContainerService>, connection_id: &QString)
            -> FfiResult;

        // --- C9: pods (Podman), a pod node's own lifecycle -------------

        #[qinvokable]
        #[cxx_name = "startPod"]
        fn start_pod(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "stopPod"]
        fn stop_pod(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "restartPod"]
        fn restart_pod(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "removePod"]
        fn remove_pod(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            force: bool,
        ) -> FfiResult;

        // --- C9: Podman machines, from the connection node's own menu --

        /// Offered only when `connection_id`'s kind is `PodmanMachine`
        /// (the cpp context menu asks `nodeActions`... no — asks this
        /// connection's own kind, already known from `connections()`; see
        /// `containers_panel.cpp`'s context-menu builder).
        /// Whether `connection_id`'s kind is `PodmanMachine` — the
        /// connection node's context menu asks this before offering
        /// "Start machine"/"Stop machine".
        #[qinvokable]
        #[cxx_name = "isPodmanMachineConnection"]
        fn is_podman_machine_connection(self: &ContainerService, connection_id: &QString) -> bool;

        #[qinvokable]
        #[cxx_name = "startMachine"]
        fn start_machine(self: Pin<&mut ContainerService>, connection_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "stopMachine"]
        fn stop_machine(self: Pin<&mut ContainerService>, connection_id: &QString) -> FfiResult;

        // --- C9: Dashboard editing -> recreate --------------------------

        #[qinvokable]
        #[cxx_name = "containerDashboard"]
        fn container_dashboard(self: &ContainerService, node_id: &QString)
            -> FfiContainerDashboard;

        /// `env`/`ports`/`mounts` are `\n`-joined lines in
        /// `FfiContainerDashboard`'s own formats — whatever the Dashboard's
        /// tables currently hold, edited or not. Refused with
        /// `errors::CODE_REFUSED` for a compose-managed container.
        #[qinvokable]
        #[cxx_name = "recreateContainer"]
        fn recreate_container(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            env: &QString,
            ports: &QString,
            mounts: &QString,
        ) -> FfiResult;

        // --- C9: Layers tab -> "Analyze image" --------------------------

        /// `save -o` + a headers-only tar walk on a worker thread;
        /// `layerFsReady` carries the result as `\n`-joined
        /// `"<layer_id>\t<path>\t<size>\t<kind>"` lines (`kind` is
        /// `added`/`modified`/`deleted`).
        #[qinvokable]
        #[cxx_name = "analyzeImage"]
        fn analyze_image(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qsignal]
        #[cxx_name = "layerFsReady"]
        fn layer_fs_ready(self: Pin<&mut ContainerService>, node_id: QString, lines: QString);

        /// A double-clicked regular file entry in the analyzed tree, open
        /// read-only (8 MiB capped) — refused when `node_id` is not the
        /// image last analyzed.
        #[qinvokable]
        #[cxx_name = "openLayerEntry"]
        fn open_layer_entry(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            layer_id: &QString,
            path: &QString,
        ) -> FfiResult;

        /// "Download..." for a layer entry — no size cap, written to
        /// `dest_path` (the view's own `QFileDialog` choice).
        #[qinvokable]
        #[cxx_name = "downloadLayerEntry"]
        fn download_layer_entry(
            self: &ContainerService,
            node_id: &QString,
            layer_id: &QString,
            path: &QString,
            dest_path: &QString,
        ) -> FfiResult;

        /// A lifecycle action finished: `ok`/`message` from its `OpError`
        /// (empty message on success). The panel shows a failure as a
        /// non-modal banner, never a dialog (C3 — routine failures are not
        /// exceptional here).
        #[qsignal]
        #[cxx_name = "actionFinished"]
        fn action_finished(
            self: Pin<&mut ContainerService>,
            node_id: QString,
            ok: bool,
            message: QString,
        );

        /// Open `node_id`'s `inspect` JSON as a read-only editor tab (no
        /// dock tab) — mirrors `LanguageService::virtualDocumentOpened`
        /// exactly, wired the same way in `editor_tabs.cpp`. A failure
        /// (engine unreachable, no such container) reports through the
        /// returned `FfiResult` instead of the signal.
        #[qinvokable]
        #[cxx_name = "openInspect"]
        fn open_inspect(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qsignal]
        #[cxx_name = "virtualDocumentOpened"]
        fn virtual_document_opened(
            self: Pin<&mut ContainerService>,
            tab_id: u64,
            title: QString,
            newly_opened: bool,
        );

        /// Ask for `top`'s answer on a worker thread; `processesReady`
        /// carries it. `titles` is `\t`-joined, in the same column order as
        /// every `FfiProcessRow::cells`.
        #[qinvokable]
        fn processes(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qsignal]
        #[cxx_name = "processesReady"]
        fn processes_ready(
            self: Pin<&mut ContainerService>,
            node_id: QString,
            titles: QString,
            rows: Vec<FfiProcessRow>,
        );

        /// List `dir` inside `node_id`'s container on a worker thread;
        /// `filesReady` carries the answer. `dir` empty means the
        /// container's `/`.
        #[qinvokable]
        #[cxx_name = "listFiles"]
        fn list_files(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            dir: &QString,
        ) -> FfiResult;

        #[qsignal]
        #[cxx_name = "filesReady"]
        fn files_ready(
            self: Pin<&mut ContainerService>,
            node_id: QString,
            dir: QString,
            entries: Vec<FfiFileEntry>,
        );

        /// Read `path` out of `node_id`'s container (`cp <id>:<path> -`)
        /// and open it as a read-only virtual document, the same
        /// `virtualDocumentOpened` signal `openInspect` uses. Refused (via
        /// the returned `FfiResult`) for anything over
        /// `container_core::files::MAX_READ_FILE_BYTES` — Download…
        /// instead.
        #[qinvokable]
        #[cxx_name = "openFile"]
        fn open_file(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            path: &QString,
        ) -> FfiResult;

        /// `cp <id>:<path> <host_dest>` on a worker thread; the result
        /// reports through `actionFinished` (`node_id` unchanged), the same
        /// banner a lifecycle action's failure shows.
        #[qinvokable]
        #[cxx_name = "downloadFile"]
        fn download_file(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            path: &QString,
            host_dest: &QString,
        ) -> FfiResult;

        // --- C3: sessions ---------------------------------------------

        /// The Log tab's command: `logs -f --timestamps --tail <n>` for
        /// `node_id`'s container, ready for `TerminalSupervisor::
        /// setCommand`.
        #[qinvokable]
        #[cxx_name = "logSessionCommand"]
        fn log_session_command(self: &ContainerService, node_id: &QString, tail: u32)
            -> FfiCommand;

        /// The Terminal tab's command: `exec -it [-u 0] <id> sh -c '…'`.
        #[qinvokable]
        #[cxx_name = "terminalSessionCommand"]
        fn terminal_session_command(
            self: &ContainerService,
            node_id: &QString,
            as_root: bool,
        ) -> FfiCommand;

        /// The Exec dialog's command: `exec -it <id> <command...>`.
        /// `command` is the dialog's own text, split on whitespace — the
        /// same limitation a shell-typed command line already has
        /// elsewhere in this codebase (`TerminalSupervisor`'s own
        /// `split_args`). Recorded into this container's exec history
        /// (capped at 10, most recent first).
        #[qinvokable]
        #[cxx_name = "execSessionCommand"]
        fn exec_session_command(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            command: &QString,
        ) -> FfiCommand;

        /// The Exec dialog's history combo for `node_id`'s container,
        /// `\n`-separated, most recent first.
        #[qinvokable]
        #[cxx_name = "execHistory"]
        fn exec_history(self: &ContainerService, node_id: &QString) -> QString;

        /// The Attach tab's command: `attach --sig-proxy=false <id>`.
        #[qinvokable]
        #[cxx_name = "attachSessionCommand"]
        fn attach_session_command(self: &ContainerService, node_id: &QString) -> FfiCommand;

        // --- C4: images, networks, volumes -----------------------------

        /// The Images console's Pull button: `pull <reference>` on
        /// `connection_id`, ready for `TerminalSupervisor::setCommand` — a
        /// closable "Pull: <reference>" tab, the same shape Log/Terminal/
        /// Exec/Attach already use.
        #[qinvokable]
        #[cxx_name = "pullSessionCommand"]
        fn pull_session_command(
            self: &ContainerService,
            connection_id: &QString,
            reference: &QString,
        ) -> FfiCommand;

        #[qinvokable]
        #[cxx_name = "removeImage"]
        fn remove_image(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            force: bool,
        ) -> FfiResult;

        /// The Images group's "Clean Up": `image prune -f [-a]`.
        #[qinvokable]
        #[cxx_name = "pruneImages"]
        fn prune_images(
            self: Pin<&mut ContainerService>,
            connection_id: &QString,
            all: bool,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "tagImage"]
        fn tag_image(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            new_reference: &QString,
        ) -> FfiResult;

        /// `history` on a worker thread; `layersReady` carries the answer.
        #[qinvokable]
        #[cxx_name = "imageLayers"]
        fn image_layers(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qsignal]
        #[cxx_name = "layersReady"]
        fn layers_ready(self: Pin<&mut ContainerService>, node_id: QString, layers: Vec<FfiLayer>);

        /// An image's labels, straight off the current snapshot — no CLI
        /// call, `container_core::model::Image` already carries them.
        #[qinvokable]
        #[cxx_name = "imageLabels"]
        fn image_labels(self: &ContainerService, node_id: &QString) -> Vec<FfiKeyValue>;

        /// `save`\|`load` through a temp file, on a worker thread —
        /// "Copy Image to...". `actionFinished`'s message carries a
        /// progress-ish summary ("Copied to <connection>" / the failure).
        #[qinvokable]
        #[cxx_name = "copyImageTo"]
        fn copy_image_to(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            target_connection_id: &QString,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "createNetwork"]
        fn create_network(
            self: Pin<&mut ContainerService>,
            connection_id: &QString,
            spec: &FfiNetworkSpec,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "removeNetwork"]
        fn remove_network(self: Pin<&mut ContainerService>, node_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "pruneNetworks"]
        fn prune_networks(self: Pin<&mut ContainerService>, connection_id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "createVolume"]
        fn create_volume(
            self: Pin<&mut ContainerService>,
            connection_id: &QString,
            spec: &FfiVolumeSpec,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "removeVolume"]
        fn remove_volume(
            self: Pin<&mut ContainerService>,
            node_id: &QString,
            force: bool,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "pruneVolumes"]
        fn prune_volumes(self: Pin<&mut ContainerService>, connection_id: &QString) -> FfiResult;

        /// One "Clean Up" menu entry: `kind` is one of `all`,
        /// `stopped-containers`, `unused-networks`, `unused-volumes`,
        /// `dangling-images`, `build-cache` (`container_core::prune::
        /// CleanUpKind`'s own ids).
        #[qinvokable]
        #[cxx_name = "cleanUp"]
        fn clean_up(
            self: Pin<&mut ContainerService>,
            connection_id: &QString,
            kind: &QString,
        ) -> FfiResult;

        /// Local image-name completion for the Images console's
        /// `QCompleter` — ranked, off the current snapshot only (Hub search
        /// is C6/C7's). `\n`-joined (no bare `Vec<QString>` on the seam,
        /// see `FfiBranch`'s doc comment), most-relevant first.
        #[qinvokable]
        #[cxx_name = "imageCompletions"]
        fn image_completions(
            self: &ContainerService,
            connection_id: &QString,
            prefix: &QString,
        ) -> QString;

        /// [`image_completions`]'s local answer at once, then Docker Hub's
        /// (C6) merged in through `imageCompletionsReady` once the network
        /// round trip lands — the New Target wizard's Image field and
        /// `run_config_container_pages.cpp`'s own Image field both drive a
        /// `QCompleter` from this the way `containers_panel.cpp`'s Pull row
        /// already does from `imageCompletions` alone. Both halves are
        /// `\n`-joined ranked names, most-relevant first (no bare
        /// `Vec<QString>` on the seam, see `FfiBranch`'s doc comment).
        #[qinvokable]
        #[cxx_name = "requestImageCompletions"]
        fn request_image_completions(
            self: Pin<&mut ContainerService>,
            connection_id: &QString,
            prefix: &QString,
        ) -> QString;

        /// `requestImageCompletions`'s Hub half, once it answers.
        #[qsignal]
        #[cxx_name = "imageCompletionsReady"]
        fn image_completions_ready(self: Pin<&mut ContainerService>, completions: QString);

        #[qinvokable]
        #[cxx_name = "imageDashboard"]
        fn image_dashboard(self: &ContainerService, node_id: &QString) -> FfiImageDashboard;

        /// A container-image run configuration's options, prefilled with
        /// this image node's own connection and reference — "Create
        /// Container..." (C5, ADR-0056 §6, replacing C4's
        /// `createContainerQuick`): the caller adds a configuration with
        /// these through `RunConfigEditor::addContainerConfiguration` and
        /// opens the run-config dialog on it.
        #[qinvokable]
        #[cxx_name = "imageRunDefaults"]
        fn image_run_defaults(self: &ContainerService, node_id: &QString) -> FfiContainerOptions;

        #[qinvokable]
        #[cxx_name = "networkDashboard"]
        fn network_dashboard(self: &ContainerService, node_id: &QString) -> FfiNetworkDashboard;

        #[qinvokable]
        #[cxx_name = "volumeDashboard"]
        fn volume_dashboard(self: &ContainerService, node_id: &QString) -> FfiVolumeDashboard;

        // --- C6: editor assistance ------------------------------------

        /// Whether `path`'s code lenses come from this service (a compose
        /// file) rather than the language server — the click router's
        /// question, answered by the Rust-side file rule.
        #[qinvokable]
        #[cxx_name = "ownsLenses"]
        fn owns_lenses(self: &ContainerService, path: &QString) -> bool;

        /// The lenses for compose file `path` with live buffer `text`,
        /// from every connected snapshot. Re-read on `treeChanged`, open
        /// and save; empty for a file this service does not own.
        #[qinvokable]
        #[cxx_name = "composeLenses"]
        fn compose_lenses(
            self: &ContainerService,
            path: &QString,
            text: &QString,
        ) -> Vec<FfiComposeLens>;

        /// A click on lens `index` of `path` (the index into the last
        /// `composeLenses` answer): answers on `containerLogRequested` or
        /// `openUrlRequested`.
        #[qinvokable]
        #[cxx_name = "runLens"]
        fn run_lens(self: Pin<&mut ContainerService>, path: &QString, index: u32);

        /// A status lens was clicked: select `node_id` in the Containers
        /// dock and show its Log tab.
        #[qsignal]
        #[cxx_name = "containerLogRequested"]
        fn container_log_requested(self: Pin<&mut ContainerService>, node_id: QString);

        /// An "Open localhost:<port>" lens was clicked.
        #[qsignal]
        #[cxx_name = "openUrlRequested"]
        fn open_url_requested(self: Pin<&mut ContainerService>, url: QString);

        /// Every connected engine's image names, `\n`-joined — forwarded
        /// by the view to `LanguageService::setLocalImages` on each
        /// `treeChanged`, the local half of image-name completion.
        #[qinvokable]
        #[cxx_name = "localImageNames"]
        fn local_image_names(self: &ContainerService) -> QString;

        /// The editor's "Pull image <reference>" intention: picks the
        /// first connected (else the only configured) connection and
        /// answers on `pullRequested`; refuses with `CODE_REFUSED` when
        /// there is no connection to pull on.
        #[qinvokable]
        #[cxx_name = "pullImage"]
        fn pull_image(self: Pin<&mut ContainerService>, reference: &QString) -> FfiResult;

        /// Open the Images console's Pull tab for `reference` on
        /// `connection_id`.
        #[qsignal]
        #[cxx_name = "pullRequested"]
        fn pull_requested(
            self: Pin<&mut ContainerService>,
            connection_id: QString,
            reference: QString,
        );

        // --- C7: registries ---------------------------------------------

        /// Fetch `registry_id`'s repositories on a worker thread (its
        /// `RegistryClient::catalog`, page one) and cache them; answers on
        /// `registryChildrenReady` once they arrive, which the view treats
        /// like `treeChanged` — a rebuild that keeps the registry node's
        /// now-first expansion. A `Generic`/unreachable registry's error
        /// still answers `registryChildrenReady` (with nothing cached, so
        /// the node simply gets no children) rather than `actionFinished`,
        /// since there is no dialog waiting on this one to report to; the
        /// Settings page's own "Test connection" is where that error text
        /// belongs.
        #[qinvokable]
        #[cxx_name = "loadRegistryRepositories"]
        fn load_registry_repositories(
            self: Pin<&mut ContainerService>,
            registry_id: &QString,
        ) -> FfiResult;

        /// Fetch a repository's tags the same way, keyed by its
        /// `RegistryRepo` node id (`"registry/<id>/repo/<repository>"`).
        #[qinvokable]
        #[cxx_name = "loadRegistryTags"]
        fn load_registry_tags(
            self: Pin<&mut ContainerService>,
            repo_node_id: &QString,
        ) -> FfiResult;

        /// C7 review follow-up: fetches the next page for a `registry-more`
        /// row — `more_node_id` is that row's own id, a `Registry`'s or
        /// `RegistryRepo`'s id with `/more` appended — and appends it to
        /// whichever cache (`registry_repos`/`registry_tags`) the parent
        /// belongs to. Answers on `registryChildrenReady` the same as
        /// `loadRegistryRepositories`/`loadRegistryTags`.
        #[qinvokable]
        #[cxx_name = "loadMoreRegistryChildren"]
        fn load_more_registry_children(
            self: Pin<&mut ContainerService>,
            more_node_id: &QString,
        ) -> FfiResult;

        /// A registry node gained children (or a refresh replaced them).
        #[qsignal]
        #[cxx_name = "registryChildrenReady"]
        fn registry_children_ready(self: Pin<&mut ContainerService>, parent_id: QString);

        /// `[login &&] pull <reference>` for a `RegistryTag` node, on
        /// `connection_id` — a single PTY session
        /// (`container_core::session::pull_from_registry_session`), ready
        /// for `TerminalSupervisor::setCommand` the same as
        /// `pullSessionCommand`. Logs in first only when a secret is
        /// stored for the registry; otherwise anonymous/CLI-credential-
        /// store pull, per ADR-0055's documented fallback.
        #[qinvokable]
        #[cxx_name = "pullFromRegistryCommand"]
        fn pull_from_registry_command(
            self: &ContainerService,
            tag_node_id: &QString,
            connection_id: &QString,
        ) -> FfiCommand;

        /// `tag && [login &&] push` for the Push Image dialog: `image_node_id`
        /// is the image being pushed, `registry_id`/`repository`/`tag` name
        /// the destination — one PTY session, same login rule as
        /// `pullFromRegistryCommand`.
        #[qinvokable]
        #[cxx_name = "pushImageCommand"]
        fn push_image_command(
            self: &ContainerService,
            image_node_id: &QString,
            registry_id: &QString,
            repository: &QString,
            tag: &QString,
        ) -> FfiCommand;
    }

    impl cxx_qt::Threading for ContainerService {}

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

    /// One entry of the Appearance page's colour-theme combo (T7). The id is
    /// the contribution's, which is what `Settings::theme_name` persists.
    struct FfiColorThemeChoice {
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

    /// One `[[file_associations.rule]]` row: `pattern` is a
    /// glob (`*.svg`), `handler` one of `FileAssociationsEditor::handlerNames`.
    /// Which pattern syntax `pattern` accepts and what a `handler` name
    /// means are `settings_model::file_associations`'s answers, not this
    /// struct's — it only carries the two strings across the seam.
    struct FfiFileAssociationRule {
        pattern: QString,
        handler: QString,
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
        /// Settings > File Associations: which handler a file
        /// pattern opens with. Live-effect like `PluginCatalog` — no draft,
        /// every add/remove/edit writes through immediately.
        #[qobject]
        type FileAssociationsEditor = super::FileAssociationsEditorRust;

        /// Whether a project is open, so the page can disable its Project
        /// tab rather than offer to write nowhere.
        #[qinvokable]
        #[cxx_name = "hasProject"]
        fn has_project(self: &FileAssociationsEditor) -> bool;

        /// Every handler the combo may offer, in display order, one per
        /// line — `cxx`'s `Vec<T>` needs `T: ImplVec`, which bare `QString`
        /// does not satisfy (see `FfiBranch`'s doc comment), and three fixed
        /// names do not need a wrapper struct's ceremony.
        #[qinvokable]
        #[cxx_name = "handlerNames"]
        fn handler_names(self: &FileAssociationsEditor) -> QString;

        /// The global `[[file_associations.rule]]` list.
        #[qinvokable]
        #[cxx_name = "globalRules"]
        fn global_rules(self: &FileAssociationsEditor) -> Vec<FfiFileAssociationRule>;

        /// Whether the open project overrides the global rules at all.
        #[qinvokable]
        #[cxx_name = "projectOverrides"]
        fn project_overrides(self: &FileAssociationsEditor) -> bool;

        /// The project's own rules — empty when it does not override.
        #[qinvokable]
        #[cxx_name = "projectRules"]
        fn project_rules(self: &FileAssociationsEditor) -> Vec<FfiFileAssociationRule>;

        /// Replaces the global rule list.
        #[qinvokable]
        #[cxx_name = "setGlobalRules"]
        fn set_global_rules(
            self: &FileAssociationsEditor,
            rules: Vec<FfiFileAssociationRule>,
        ) -> FfiResult;

        /// Turns the project's override on or off, leaving its rules alone.
        #[qinvokable]
        #[cxx_name = "setProjectOverrides"]
        fn set_project_overrides(self: &FileAssociationsEditor, overrides: bool) -> FfiResult;

        /// Replaces the project's rule list. Refused when the project does
        /// not currently override — turn that on first.
        #[qinvokable]
        #[cxx_name = "setProjectRules"]
        fn set_project_rules(
            self: &FileAssociationsEditor,
            rules: Vec<FfiFileAssociationRule>,
        ) -> FfiResult;
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
        has_soft_wrap: bool,
        soft_wrap: bool,
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

        /// Where a `changedFiles()` row's **repository-relative** path lives on
        /// disk. Empty when there is no repository. The inverse of
        /// `fileStatus`, and the only supported way for the view to turn a
        /// Changes-dock row into something openable — resolving one against the
        /// process working directory fails in a packaged build.
        #[qinvokable]
        #[cxx_name = "absolutePath"]
        fn absolute_path(self: &VcsService, relative: &QString) -> QString;

        /// What the last `refreshStatus` says about one file, by **absolute**
        /// path — the shape the project tree holds. An empty `path` in the
        /// answer means the file has no pending change (or is not in this
        /// repository at all), which is the same thing `changedFiles()` says
        /// by omitting it.
        #[qinvokable]
        #[cxx_name = "fileStatus"]
        fn file_status(self: &VcsService, path: &QString) -> FfiChangedFile;

        /// The branch/upstream/ahead-behind picture the last `refreshStatus`
        /// found — the Changes dock toolbar's branch chip and Pull/Push
        /// counts. Answers via the same `statusChanged` signal
        /// `changedFiles`/`fileStatus` already do; there is no separate
        /// "branch status changed" signal.
        #[qinvokable]
        #[cxx_name = "branchStatus"]
        fn branch_status(self: &VcsService) -> FfiBranchStatus;

        /// Ask for `path`'s hunks against `HEAD`, diffed against
        /// `workingText` (the live buffer) and cached by `revision` — the
        /// open document's own revision, which is what makes the cache
        /// answer a repeat request for an unchanged buffer instead of
        /// rediffing it. Answers via `hunksChanged(path)`.
        #[qinvokable]
        #[cxx_name = "requestHunks"]
        fn request_hunks(
            self: Pin<&mut VcsService>,
            path: &QString,
            working_text: &QString,
            revision: i64,
        );

        /// Drop whatever `requestHunks`/`requestBlobAt` cached for `path`.
        /// Called when a tab closes: the cached entries hold that file's
        /// whole `HEAD` text and whole working text, so without this they
        /// accumulate for every file opened in the life of a project.
        #[qinvokable]
        #[cxx_name = "forgetPath"]
        fn forget_path(self: &VcsService, path: &QString);

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

        /// `git add -A`; the "Stage all" toolbar button's whole-worktree
        /// counterpart to `stageFile`. `statusChanged` follows on success,
        /// `vcsFailed` on failure.
        #[qinvokable]
        #[cxx_name = "stageAll"]
        fn stage_all(self: Pin<&mut VcsService>);

        /// `git reset`; the "Unstage all" toolbar button's whole-worktree
        /// counterpart to `unstageFile`. `statusChanged` follows on success,
        /// `vcsFailed` on failure.
        #[qinvokable]
        #[cxx_name = "unstageAll"]
        fn unstage_all(self: Pin<&mut VcsService>);

        /// `git checkout HEAD -- <path>`: discard the file's staged *and*
        /// unstaged changes, putting it back the way `HEAD` has it.
        /// `statusChanged` follows on success, `vcsFailed` on failure —
        /// including for an untracked file, which `HEAD` has no copy of.
        ///
        /// Destructive and not undoable from inside the IDE: the working-tree
        /// content is gone once `git` has run. The view confirms first.
        #[qinvokable]
        #[cxx_name = "revertFile"]
        fn revert_file(self: Pin<&mut VcsService>, path: &QString);

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

        /// `git commit -m <message> [--amend]`, exactly what is staged. On
        /// success, `message` is recorded into this project's
        /// commit-message history (`commitHistory`).
        /// `author` is the literal `Name <email>` for `--author`, empty for
        /// the configured identity; `signoff` adds `--signoff`.
        #[qinvokable]
        fn commit(
            self: Pin<&mut VcsService>,
            message: &QString,
            amend: bool,
            author: &QString,
            signoff: bool,
        );

        /// Append `path` (absolute; resolved against the repository root
        /// here) to the root `.gitignore`, creating it if missing —
        /// idempotent. Answers via `statusChanged` once `git` stops
        /// reporting the path; failure via `vcsFailed`.
        #[qinvokable]
        #[cxx_name = "addToGitignore"]
        fn add_to_gitignore(self: Pin<&mut VcsService>, path: &QString);

        /// Local branches then tags, as `refreshBranches` last found them —
        /// what "Compare with Branch, Tag or Revision…" lists (a raw
        /// revision is typed into the same, editable picker).
        #[qinvokable]
        #[cxx_name = "refNames"]
        fn ref_names(self: &VcsService) -> Vec<FfiBranch>;

        /// `git diff --name-only <revision>` on the worker; answers via
        /// `changedPathsReady(revision, paths)`.
        #[qinvokable]
        #[cxx_name = "requestChangedPathsAgainst"]
        fn request_changed_paths_against(self: Pin<&mut VcsService>, revision: &QString);

        /// The three-state colouring of `hunks(path)`, index for index —
        /// which of the gutter's `HEAD`-vs-worktree hunks are already
        /// (partly) in the index. Empty before `requestHunks` has answered.
        #[qinvokable]
        #[cxx_name = "hunkStates"]
        fn hunk_states(self: &VcsService, path: &QString) -> Vec<FfiHunkState>;

        /// The `HEAD`-side lines `hunks(path)[hunk_index]` removed, joined
        /// with `\n` — what the gutter popup shows inline. Empty for a pure
        /// addition, or when nothing is cached for `path`.
        #[qinvokable]
        #[cxx_name = "hunkRemovedText"]
        fn hunk_removed_text(self: &VcsService, path: &QString, hunk_index: u32) -> QString;

        /// The Changes dock's per-hunk rows (R6): `HEAD`-vs-worktree hunks
        /// of `path` (absolute) read from the file on disk — the dock
        /// covers files that are not open — classified against the index.
        /// Answers via `fileHunksReady(path)`; read with `fileHunks`/
        /// `fileHunkStates`. Kept apart from `requestHunks`'s buffer-based
        /// cache so a disk read never overwrites what an open editor's
        /// gutter is showing.
        #[qinvokable]
        #[cxx_name = "requestFileHunks"]
        fn request_file_hunks(self: Pin<&mut VcsService>, path: &QString);

        #[qinvokable]
        #[cxx_name = "fileHunks"]
        fn file_hunks(self: &VcsService, path: &QString) -> Vec<FfiHunk>;

        #[qinvokable]
        #[cxx_name = "fileHunkStates"]
        fn file_hunk_states(self: &VcsService, path: &QString) -> Vec<FfiHunkState>;

        /// `stageHunk`/`unstageHunk` over `fileHunks(path)` rather than
        /// `hunks(path)` — the dock's rows.
        #[qinvokable]
        #[cxx_name = "stageFileHunk"]
        fn stage_file_hunk(self: Pin<&mut VcsService>, path: &QString, hunk_index: u32);

        #[qinvokable]
        #[cxx_name = "unstageFileHunk"]
        fn unstage_file_hunk(self: Pin<&mut VcsService>, path: &QString, hunk_index: u32);

        /// `HEAD`'s own commit message, last refreshed alongside
        /// `refreshStatus` — Amend's prefill. Empty for an unborn `HEAD`.
        #[qinvokable]
        #[cxx_name = "headMessage"]
        fn head_message(self: &VcsService) -> QString;

        /// This project's past commit messages, newest first, capped at 25
        /// — the Changes dock's commit-message combo.
        #[qinvokable]
        #[cxx_name = "commitHistory"]
        fn commit_history(self: &VcsService) -> Vec<FfiCommitMessage>;

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

        /// The repository-wide commit log reachable from `HEAD`, newest
        /// first, for the repo-wide log panel — `0` means the default page
        /// size; a caller re-asks with a larger `max` for "Load more"
        /// rather than this crossing the seam as a cursor. Answers via
        /// `commitLogReady`.
        #[qinvokable]
        #[cxx_name = "commitLog"]
        fn commit_log(self: Pin<&mut VcsService>, max: u32);

        /// Ask for one commit's full detail and its changed-file list
        /// together (both come off the same commit, one worker round trip)
        /// — the commit-detail dock's first step on opening a tab.
        /// Answers via `commitDetailReady(id)`; `commitDetail`/
        /// `changedCommitFiles` then read the cache it filled.
        #[qinvokable]
        #[cxx_name = "requestCommitDetail"]
        fn request_commit_detail(self: Pin<&mut VcsService>, id: &QString);

        /// `requestCommitDetail(id)`'s last answer for this id, or a
        /// default-valued `FfiCommitDetail` before it arrives (or if `id`
        /// did not resolve to a commit).
        #[qinvokable]
        #[cxx_name = "commitDetail"]
        fn commit_detail(self: &VcsService, id: &QString) -> FfiCommitDetail;

        /// The paths commit `id` changed against its first parent —
        /// answered together with `requestCommitDetail(id)`, read here
        /// synchronously once `commitDetailReady(id)` has fired.
        #[qinvokable]
        #[cxx_name = "changedCommitFiles"]
        fn changed_commit_files(self: &VcsService, id: &QString) -> Vec<FfiChangedCommitFile>;

        /// Ask for one changed file's before/after text and hunks as of
        /// commit `id`, against its first parent. Answers via
        /// `commitFileDiffReady(id, path)`; `commitFileDiff`/
        /// `commitFileDiffHunks` then read the cache it filled, the same
        /// two-step `requestHunks`/`hunks` already uses.
        #[qinvokable]
        #[cxx_name = "requestCommitFileDiff"]
        fn request_commit_file_diff(self: Pin<&mut VcsService>, id: &QString, path: &QString);

        /// The whole-file before/after text for `(id, path)` — hunks come
        /// from `commitFileDiffHunks`, the same split `FfiFileDiff`'s own
        /// doc comment explains.
        #[qinvokable]
        #[cxx_name = "commitFileDiff"]
        fn commit_file_diff(self: &VcsService, id: &QString, path: &QString) -> FfiFileDiff;

        /// The line hunks for the same `(id, path)` pair `commitFileDiff`
        /// describes.
        #[qinvokable]
        #[cxx_name = "commitFileDiffHunks"]
        fn commit_file_diff_hunks(self: &VcsService, id: &QString, path: &QString) -> Vec<FfiHunk>;

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

        /// `fileHunks(path)`/`fileHunkStates(path)` have a fresh answer.
        #[qsignal]
        #[cxx_name = "fileHunksReady"]
        fn file_hunks_ready(self: Pin<&mut VcsService>, path: QString);

        /// `requestChangedPathsAgainst(revision)`'s answer, tagged with
        /// the revision it was asked for.
        #[qsignal]
        #[cxx_name = "changedPathsReady"]
        fn changed_paths_ready(
            self: Pin<&mut VcsService>,
            revision: QString,
            paths: Vec<FfiRepoPath>,
        );

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

        /// `commitLog`'s answer. No path tag — unlike `fileHistory`, there
        /// is only ever one repo-wide log in flight at a time.
        #[qsignal]
        #[cxx_name = "commitLogReady"]
        fn commit_log_ready(self: Pin<&mut VcsService>, entries: Vec<FfiLogEntry>);

        /// `requestCommitDetail(id)` has a fresh answer for this id —
        /// `commitDetail(id)`/`changedCommitFiles(id)` both read it now.
        #[qsignal]
        #[cxx_name = "commitDetailReady"]
        fn commit_detail_ready(self: Pin<&mut VcsService>, id: QString);

        /// `requestCommitFileDiff(id, path)` has a fresh answer for this
        /// pair.
        #[qsignal]
        #[cxx_name = "commitFileDiffReady"]
        fn commit_file_diff_ready(self: Pin<&mut VcsService>, id: QString, path: QString);

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

        // -- R7: filtered log, ref chips, remotes, integration ops, stash,
        // and the per-file blame toggle memory. --

        /// `commitLog`, filtered (R7's log filter bar). Every string
        /// parameter empty means "no filter on that field"; `since`/`until`
        /// are seconds since the Unix epoch, `i64::MIN` meaning "unset" (no
        /// real commit predates it). Answers via `commitLogReady`, the same
        /// signal `commitLog` uses — only one repo-wide log is ever in
        /// flight.
        #[qinvokable]
        #[cxx_name = "commitLogFiltered"]
        fn commit_log_filtered(
            self: Pin<&mut VcsService>,
            author: &QString,
            path: &QString,
            text: &QString,
            since: i64,
            until: i64,
            max: u32,
        );

        /// The refs (`HEAD`, local branches, tags) decorating commit `id`,
        /// or empty if none do — read from the cache `refreshBranches`
        /// fills alongside `branches()`/`refNames()`.
        #[qinvokable]
        #[cxx_name = "commitRefs"]
        fn commit_refs(self: &VcsService, id: &QString) -> Vec<FfiRefDecoration>;

        /// Every configured remote, filled by the same `refreshBranches`
        /// round trip — R7's remote picker, replacing the hard-coded
        /// `origin` every push/pull/fetch call used before.
        #[qinvokable]
        fn remotes(self: &VcsService) -> Vec<FfiRemoteInfo>;

        /// Every remote-tracking branch (e.g. `origin/main`), filled by the
        /// same round trip — R7's branch popup Remote section.
        #[qinvokable]
        #[cxx_name = "remoteBranches"]
        fn remote_branches(self: &VcsService) -> Vec<FfiBranch>;

        /// `git merge <branch>`. A conflict surfaces via `vcsFailed` with
        /// `FfiVcsErrorCode::MergeConflict` — the Changes dock's existing
        /// "Merge Conflicts" group picks it up on the `statusChanged` this
        /// still fires.
        #[qinvokable]
        fn merge(self: Pin<&mut VcsService>, branch: &QString);

        /// `git rebase <onto>`. Conflict handling matches `merge`.
        #[qinvokable]
        fn rebase(self: Pin<&mut VcsService>, onto: &QString);

        /// `git cherry-pick <id>`. Conflict handling matches `merge`.
        #[qinvokable]
        #[cxx_name = "cherryPick"]
        fn cherry_pick(self: Pin<&mut VcsService>, id: &QString);

        /// `git revert --no-edit <id>` — reverts a whole commit as a new
        /// commit. Conflict handling matches `merge`.
        #[qinvokable]
        #[cxx_name = "revertCommit"]
        fn revert_commit(self: Pin<&mut VcsService>, id: &QString);

        /// `git reset --soft|--mixed|--hard <id>` — moves the current
        /// branch to `id`.
        #[qinvokable]
        #[cxx_name = "resetTo"]
        fn reset_to(self: Pin<&mut VcsService>, id: &QString, mode: FfiResetMode);

        /// `git branch -m <old> <new>`.
        #[qinvokable]
        #[cxx_name = "renameBranch"]
        fn rename_branch(self: Pin<&mut VcsService>, old: &QString, new_name: &QString);

        /// A bare `git push`, relying on the configured upstream — the VCS
        /// menu's plain "Push" action, which has no explicit remote/branch
        /// to hand `push`. A "no upstream" refusal surfaces via `vcsFailed`
        /// with `FfiVcsErrorCode::NoUpstream`, for the caller to fall back
        /// to the branch popup's explicit remote picker.
        #[qinvokable]
        #[cxx_name = "pushTracking"]
        fn push_tracking(self: Pin<&mut VcsService>);

        /// `git stash push [-m <message>]`. Empty `message` lets `git`
        /// generate its own.
        #[qinvokable]
        #[cxx_name = "stashPush"]
        fn stash_push(self: Pin<&mut VcsService>, message: &QString);

        /// `git stash pop stash@{index}`.
        #[qinvokable]
        #[cxx_name = "stashPop"]
        fn stash_pop(self: Pin<&mut VcsService>, index: u32);

        /// `git stash drop stash@{index}`.
        #[qinvokable]
        #[cxx_name = "stashDrop"]
        fn stash_drop(self: Pin<&mut VcsService>, index: u32);

        /// `git stash list`. Answers via `stashListReady`.
        #[qinvokable]
        #[cxx_name = "requestStashList"]
        fn request_stash_list(self: Pin<&mut VcsService>);

        /// Whether `path` (a tab's absolute path) last had "Annotate with
        /// Blame" toggled on for this project, per
        /// `app_config::vcs_local_settings` — R7's per-file blame memory.
        #[qinvokable]
        #[cxx_name = "blameEnabledFor"]
        fn blame_enabled_for(self: &VcsService, path: &QString) -> bool;

        /// Record this machine's blame-toggle choice for `path`.
        #[qinvokable]
        #[cxx_name = "setBlameEnabledFor"]
        fn set_blame_enabled_for(self: &VcsService, path: &QString, enabled: bool);

        /// `requestStashList`'s answer.
        #[qsignal]
        #[cxx_name = "stashListReady"]
        fn stash_list_ready(self: Pin<&mut VcsService>, entries: Vec<FfiStashEntry>);
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

        /// Launch `config` as a temporary run configuration, remembering it
        /// first (jvm-build-tools plan B1/B3):
        /// `BuildToolsService::taskConfig`'s own answer, handed straight
        /// back here by a task/goal double-click.
        #[qinvokable]
        #[cxx_name = "runTemporary"]
        fn run_temporary(self: Pin<&mut RunService>, config: &FfiRunConfig) -> FfiResult;

        /// Whether `path`'s gutter should show the Dockerfile/Containerfile
        /// popup (C5, ADR-0056) — `syntax_core`'s own `dockerfile` language
        /// entry, so the view asks rather than reimplementing the rule.
        #[qinvokable]
        #[cxx_name = "canRunContainerfile"]
        fn can_run_containerfile(self: &RunService, path: &QString) -> bool;

        /// Whether `path`'s gutter popup should say "Containerfile" rather
        /// than "Dockerfile" (C9) — Podman's naming convention.
        #[qinvokable]
        #[cxx_name = "isNamedContainerfile"]
        fn is_named_containerfile(self: &RunService, path: &QString) -> bool;

        /// Whether `path`'s gutter should show the compose popup.
        #[qinvokable]
        #[cxx_name = "canRunComposeFile"]
        fn can_run_compose_file(self: &RunService, path: &QString) -> bool;

        /// The Dockerfile gutter's "Build image": `docker build` only, as a
        /// tracked console.
        #[qinvokable]
        #[cxx_name = "buildContainerfile"]
        fn build_containerfile(self: Pin<&mut RunService>, path: &QString) -> FfiResult;

        /// The Dockerfile gutter's "Run container": build then run,
        /// remembered as a temporary configuration and launched.
        #[qinvokable]
        #[cxx_name = "runContainerfile"]
        fn run_containerfile(self: Pin<&mut RunService>, path: &QString) -> FfiResult;

        /// The Dockerfile gutter's "New configuration...": remembers the
        /// temporary configuration without launching it, returning its id
        /// so the caller can open the run-config dialog pointed at it.
        #[qinvokable]
        #[cxx_name = "newContainerfileConfiguration"]
        fn new_containerfile_configuration(self: Pin<&mut RunService>, path: &QString) -> QString;

        /// The compose file gutter's "Run": the whole file, every service.
        #[qinvokable]
        #[cxx_name = "runComposeFile"]
        fn run_compose_file(self: Pin<&mut RunService>, path: &QString) -> FfiResult;

        /// The compose file gutter's "New configuration...".
        #[qinvokable]
        #[cxx_name = "newComposeFileConfiguration"]
        fn new_compose_file_configuration(self: Pin<&mut RunService>, path: &QString) -> QString;

        /// A compose service line's marker (C6): `compose up` for that one
        /// service, remembered as its own temporary configuration.
        #[qinvokable]
        #[cxx_name = "runComposeService"]
        fn run_compose_service(
            self: Pin<&mut RunService>,
            path: &QString,
            service: &QString,
        ) -> FfiResult;

        /// The service line's "New configuration..." (C6).
        #[qinvokable]
        #[cxx_name = "newComposeServiceConfiguration"]
        fn new_compose_service_configuration(
            self: Pin<&mut RunService>,
            path: &QString,
            service: &QString,
        ) -> QString;

        /// The 0-based lines of `path` that carry a gutter run marker (C6):
        /// a compose file's `services:` key and every service under it,
        /// line 0 for any other runnable file, none otherwise. `text` is the
        /// live buffer so the markers follow unsaved edits.
        #[qinvokable]
        #[cxx_name = "runLines"]
        fn run_lines(self: &RunService, path: &QString, text: &QString) -> Vec<u32>;

        /// The compose service declared at `line`, empty on the `services:`
        /// line and in any other file — what a marker click's popup is
        /// scoped to (C6).
        #[qinvokable]
        #[cxx_name = "composeServiceAt"]
        fn compose_service_at(
            self: &RunService,
            path: &QString,
            text: &QString,
            line: u32,
        ) -> QString;

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

        /// Whether `console_id`'s configuration is a compose configuration —
        /// the console's "Down" button is shown only then (C5, ADR-0056).
        #[qinvokable]
        #[cxx_name = "isComposeConsole"]
        fn is_compose_console(self: &RunService, console_id: u64) -> bool;

        /// `compose down`, with the configuration's remove flags, for a
        /// compose console — Ctrl-C-ing `compose up` alone leaves the
        /// containers running, which this is the console's way to actually
        /// tear down (JetBrains parity). Runs as a tracked console like any
        /// other launch (C5, ADR-0056 §6), so its output shows up in the run
        /// dock rather than disappearing.
        #[qinvokable]
        #[cxx_name = "composeDown"]
        fn compose_down(self: Pin<&mut RunService>, console_id: u64) -> FfiResult;

        /// The Containers dock compose project node's "Start All": finds or
        /// launches `compose up -d` for `files` as a tracked console (C5,
        /// ADR-0056).
        #[qinvokable]
        #[cxx_name = "runComposeProject"]
        fn run_compose_project(
            self: Pin<&mut RunService>,
            connection_id: &QString,
            files: &QString,
            project_name: &QString,
        ) -> FfiResult;

        /// The compose project node's "Stop": `compose stop`, tracked.
        #[qinvokable]
        #[cxx_name = "stopComposeProject"]
        fn stop_compose_project(
            self: Pin<&mut RunService>,
            connection_id: &QString,
            files: &QString,
            project_name: &QString,
        ) -> FfiResult;

        /// The compose project node's "Down": `compose down`, tracked.
        #[qinvokable]
        #[cxx_name = "downComposeProject"]
        fn down_compose_project(
            self: Pin<&mut RunService>,
            connection_id: &QString,
            files: &QString,
            project_name: &QString,
        ) -> FfiResult;

        /// The compose service node's "Scale...": `compose up -d --scale
        /// service=n --no-recreate`, tracked.
        #[qinvokable]
        #[cxx_name = "scaleComposeService"]
        fn scale_compose_service(
            self: Pin<&mut RunService>,
            connection_id: &QString,
            files: &QString,
            service: &QString,
            count: u32,
        ) -> FfiResult;

        /// The `file:line[:col]` (or Python `File "...", line N`) location
        /// covering `byte_offset` in `console_id`'s accumulated output, for
        /// hover feedback and Ctrl+Click — the same
        /// `run_core::links::resolve_link` catalogue `TerminalSession` uses
        /// for terminal output.
        #[qinvokable]
        #[cxx_name = "resolveLink"]
        fn resolve_link(self: &RunService, console_id: u64, byte_offset: u32) -> FfiResolvedLink;

        /// The run target `console_id`'s configuration runs on (C8), by
        /// name — empty for a local launch. The run console header shows
        /// "on &lt;name&gt;" when this is non-empty.
        #[qinvokable]
        #[cxx_name = "consoleTargetLabel"]
        fn console_target_label(self: &RunService, console_id: u64) -> QString;

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

        /// The session the views default to with none explicitly chosen —
        /// the lowest running id, 0 for none (R5). What the gutter's Run to
        /// Cursor acts on.
        #[qinvokable]
        #[cxx_name = "currentSessionId"]
        fn current_session_id(self: &DebugService) -> u64;

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

        /// Show this thread's own stack instead of the one that stopped
        /// (R5) — the threads combo. Answers via `framesChanged`.
        #[qinvokable]
        #[cxx_name = "selectThread"]
        fn select_thread(self: Pin<&mut DebugService>, session_id: u64, thread_id: i64);

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

        /// Evaluate an expression in the selected frame and render it as a
        /// tree row (R5); answers via `evaluatedToTree`. A failed evaluation
        /// answers with its own message rather than nothing.
        #[qinvokable]
        #[cxx_name = "evaluateToTree"]
        fn evaluate_to_tree(self: Pin<&mut DebugService>, session_id: u64, expression: &QString);

        /// Every expression the Evaluate box has run, newline-separated,
        /// most recent first — its history dropdown (R5).
        #[qinvokable]
        #[cxx_name = "evaluateHistory"]
        fn evaluate_history(self: &DebugService) -> QString;

        /// Children of a Watches or Evaluate tree row (R5); answers via
        /// `watchChildrenChanged`, then `variables(reference)` reads them —
        /// the same cache `expand` fills, since a `variablesReference` means
        /// the same thing regardless of which tree asked for it.
        #[qinvokable]
        #[cxx_name = "watchChildren"]
        fn watch_children(self: Pin<&mut DebugService>, session_id: u64, reference: i64);

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

        /// Every watch as one tree row each (R5): expression, last value,
        /// type and — non-zero — the reference `watchChildren` expands.
        #[qinvokable]
        #[cxx_name = "watchesDetailed"]
        fn watches_detailed(self: &DebugService) -> Vec<FfiWatch>;

        #[qinvokable]
        #[cxx_name = "addWatch"]
        fn add_watch(self: Pin<&mut DebugService>, expression: &QString);

        /// Change what a watch evaluates — the Watches tree's inline rename.
        #[qinvokable]
        #[cxx_name = "editWatch"]
        fn edit_watch(self: Pin<&mut DebugService>, index: u32, expression: &QString);

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

        /// One breakpoint's full detail (R5) — what the Edit Breakpoint
        /// dialog prefills from. A line with no breakpoint yet answers with
        /// the defaults a new one would have.
        #[qinvokable]
        #[cxx_name = "breakpointAt"]
        fn breakpoint_at(self: &DebugService, path: &QString, line: u32) -> FfiBreakpoint;

        /// Every breakpoint in the project (R5) — the Breakpoints window.
        #[qinvokable]
        #[cxx_name = "allBreakpoints"]
        fn all_breakpoints(self: &DebugService) -> Vec<FfiBreakpoint>;

        /// Give a breakpoint a condition, a hit condition or a log message,
        /// make it temporary, or enable and disable it — the Edit Breakpoint
        /// dialog's whole job. `breakpoint.path`/`.line` say which one;
        /// one struct rather than seven scalars, symmetric with what
        /// `breakpointAt` hands back to prefill the same dialog from.
        #[qinvokable]
        #[cxx_name = "configureBreakpoint"]
        fn configure_breakpoint(self: Pin<&mut DebugService>, breakpoint: FfiBreakpoint);

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

        /// `selectThread` fetched a different thread's frames.
        #[qsignal]
        #[cxx_name = "framesChanged"]
        fn frames_changed(self: Pin<&mut DebugService>, session_id: u64);

        /// The selected frame's scopes, newline-separated by name, in the
        /// order the adapter reported them.
        #[qsignal]
        #[cxx_name = "scopesChanged"]
        fn scopes_changed(self: Pin<&mut DebugService>, session_id: u64, names: QString);

        /// `evaluateToTree` answered.
        #[qsignal]
        #[cxx_name = "evaluatedToTree"]
        fn evaluated_to_tree(self: Pin<&mut DebugService>, session_id: u64, row: FfiVariable);

        /// `watchChildren(reference)` has a fresh answer; read it back with
        /// `variables(reference)`.
        #[qsignal]
        #[cxx_name = "watchChildrenChanged"]
        fn watch_children_changed(self: Pin<&mut DebugService>, session_id: u64, reference: i64);

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

        /// Add a container-kind configuration prefilled with `options`,
        /// returning its index — the Add ▸ Containers submenu and "Create
        /// Container..." (C5, ADR-0056 §6, replacing C4's
        /// `createContainerQuick`).
        #[qinvokable]
        #[cxx_name = "addContainerConfiguration"]
        fn add_container_configuration(
            self: &RunConfigEditor,
            name: &QString,
            kind: &QString,
            options: &FfiContainerOptions,
        ) -> u32;

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

        /// The shell-quoted command `form` would run (C5, ADR-0056): the
        /// container-kind dialog pages' live "Command preview". Built from
        /// `form` directly (not `configurations()[index]`), so it updates as
        /// the user types, before Apply/OK commits anything.
        #[qinvokable]
        #[cxx_name = "commandPreview"]
        fn command_preview(self: &RunConfigEditor, form: &FfiRunConfig) -> QString;

        /// `compose -f <files>… config --services` against `connection_id`,
        /// for the Compose page's Services picker: runs on a worker thread
        /// (the same shape `ContainerService::testConnection` uses — this is
        /// a CLI call against a possibly slow or unreachable daemon, never
        /// blocking the Qt thread) and reports through
        /// `composeServicesReady`. `files` is `\n`-separated, project-
        /// relative.
        #[qinvokable]
        #[cxx_name = "requestComposeServices"]
        fn request_compose_services(
            self: Pin<&mut RunConfigEditor>,
            connection_id: &QString,
            files: &QString,
        );

        /// `requestComposeServices`'s answer: every service name, `\n`-
        /// separated (no bare `Vec<QString>` on the seam — see `FfiBranch`'s
        /// doc comment), empty on any failure (connection unreachable, no
        /// compose file, ...).
        #[qsignal]
        #[cxx_name = "composeServicesReady"]
        fn compose_services_ready(self: Pin<&mut RunConfigEditor>, services: QString);

        /// Whether a compose service needs a before-launch build (C8): worker
        /// thread, reports through `composeNeedsBuildReady`.
        #[qinvokable]
        #[cxx_name = "requestComposeNeedsBuild"]
        fn request_compose_needs_build(
            self: Pin<&mut RunConfigEditor>,
            connection_id: &QString,
            files: &QString,
            service: &QString,
        );

        /// `requestComposeNeedsBuild`'s answer.
        #[qsignal]
        #[cxx_name = "composeNeedsBuildReady"]
        fn compose_needs_build_ready(self: Pin<&mut RunConfigEditor>, needs_build: bool);

        /// The run targets the "Run on" combo lists (C8): effective
        /// settings (global with the project's override applied). Edited
        /// through `AppSettings::containerTargets`/`saveContainerTargets`
        /// and the New Target wizard, never through this draft.
        #[qinvokable]
        #[cxx_name = "containerTargets"]
        fn container_targets(self: &RunConfigEditor) -> Vec<FfiContainerTarget>;

        /// The command the New Target wizard's live preview shows: `target`
        /// wrapped around a stand-in `echo hello` launch.
        #[qinvokable]
        #[cxx_name = "targetCommandPreview"]
        fn target_command_preview(self: &RunConfigEditor, target: &FfiContainerTarget) -> QString;

        /// New Target wizard's Finish (C8): appends `target` to the
        /// project's `[containers].targets`, returning its id (freshly
        /// generated when `target.id` arrives blank).
        #[qinvokable]
        #[cxx_name = "addContainerTarget"]
        fn add_container_target(self: &RunConfigEditor, target: &FfiContainerTarget) -> QString;

        /// Settings > Containers > Run targets' Edit, reopening the wizard
        /// prefilled.
        #[qinvokable]
        #[cxx_name = "updateContainerTarget"]
        fn update_container_target(
            self: &RunConfigEditor,
            target: &FfiContainerTarget,
        ) -> FfiResult;

        /// Settings > Containers > Run targets' Remove.
        #[qinvokable]
        #[cxx_name = "removeContainerTarget"]
        fn remove_container_target(self: &RunConfigEditor, id: &QString) -> FfiResult;
    }

    impl cxx_qt::Threading for RunConfigEditor {}

    extern "RustQt" {
        /// What this build is, for the About dialog: the product name, the
        /// crate version, the commit it was built from and where the project
        /// lives.
        ///
        /// Stateless, so the view constructs one where it needs it
        /// (`help_menu.cpp`) rather than being handed one — the same shape
        /// `IconProvider` already takes.
        #[qobject]
        type AppInfo = super::AppInfoRust;

        /// The product name.
        #[qinvokable]
        #[cxx_name = "appName"]
        fn app_name(self: &AppInfo) -> QString;

        /// The `app`/`ui-shell` crate version, e.g. `0.1.0`.
        #[qinvokable]
        #[cxx_name = "appVersion"]
        fn app_version(self: &AppInfo) -> QString;

        /// The abbreviated commit this binary was built from, or `unknown`
        /// when the build had no repository or no `git` to ask.
        #[qinvokable]
        #[cxx_name = "gitHash"]
        fn git_hash(self: &AppInfo) -> QString;

        /// That commit's date as `YYYY-MM-DD`, or `unknown` on the same
        /// terms as [`Self::git_hash`].
        #[qinvokable]
        #[cxx_name = "gitDate"]
        fn git_date(self: &AppInfo) -> QString;

        /// The project's public repository URL.
        #[qinvokable]
        #[cxx_name = "projectUrl"]
        fn project_url(self: &AppInfo) -> QString;
    }

    // ---- database: F1 ----

    /// One row the Settings > Database list shows — id/name/driver/group/
    /// color plus which layer (`"global"`/`"project"`) it lives in
    /// (Database Tools plan F1.6).
    #[derive(Default)]
    struct FfiDataSourceRow {
        id: QString,
        name: QString,
        driver: QString,
        group: QString,
        color: QString,
        scope: QString,
    }

    /// One driver `plugin_host::registry().database_drivers()` contributes,
    /// for the dialog's driver combo box.
    #[derive(Default)]
    struct FfiDriverOption {
        id: QString,
        name: QString,
        /// `"native"`, `"adbc"`, or `"odbc"` (F8b) — which status/field
        /// group the Data Source dialog shows for this row.
        backend: QString,
    }

    /// Every field `DataSourceEditor` edits — one struct rather than
    /// nineteen separate getters, the same convention `FfiBuildToolsFields`
    /// uses.
    #[derive(Default)]
    struct FfiDataSourceFields {
        id: QString,
        name: QString,
        driver: QString,
        group: QString,
        color: QString,
        host: QString,
        port: QString,
        database: QString,
        user: QString,
        auth: QString,
        #[cxx_name = "readOnly"]
        read_only: bool,
        history: bool,
        url: QString,
        #[cxx_name = "sslMode"]
        ssl_mode: QString,
        #[cxx_name = "sslCaFile"]
        ssl_ca_file: QString,
        #[cxx_name = "sshHost"]
        ssh_host: QString,
        #[cxx_name = "sshPort"]
        ssh_port: QString,
        #[cxx_name = "sshUser"]
        ssh_user: QString,
        #[cxx_name = "sshAuth"]
        ssh_auth: QString,
        #[cxx_name = "sshKeyFile"]
        ssh_key_file: QString,
    }

    /// Which field a `FfiDataSourceProblem` is about, for the dialog to
    /// highlight — `settings_model::database::DataSourceField` crossed.
    enum FfiDataSourceField {
        Name,
        Id,
        Port,
        Color,
        SshUser,
        FileSourcePath,
    }

    struct FfiDataSourceProblem {
        field: FfiDataSourceField,
        sentence: QString,
    }

    extern "RustQt" {
        /// Every data source, global and project merged by id
        /// (Database Tools plan F1.6).
        #[qinvokable]
        #[cxx_name = "databaseSources"]
        fn database_sources(self: &AppSettings) -> Vec<FfiDataSourceRow>;

        #[qinvokable]
        #[cxx_name = "removeDatabaseSource"]
        fn remove_database_source(self: &AppSettings, id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "duplicateDatabaseSource"]
        fn duplicate_database_source(self: &AppSettings, id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "databaseDrivers"]
        fn database_drivers(self: &AppSettings) -> Vec<FfiDriverOption>;
    }

    extern "RustQt" {
        /// The Add/Edit Data Source dialog's draft (Database Tools plan
        /// F1.6), shaped after `BuildToolsEditor` but per-item.
        #[qobject]
        type DataSourceEditor = super::DataSourceEditorRust;

        #[qinvokable]
        #[cxx_name = "beginEdit"]
        fn begin_edit(self: &DataSourceEditor, id: &QString, scope: &QString);

        #[qinvokable]
        fn fields(self: &DataSourceEditor) -> FfiDataSourceFields;

        #[qinvokable]
        fn problems(self: &DataSourceEditor) -> Vec<FfiDataSourceProblem>;

        #[qinvokable]
        #[cxx_name = "setName"]
        fn set_name(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setDriver"]
        fn set_driver(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setGroup"]
        fn set_group(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setColor"]
        fn set_color(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setHost"]
        fn set_host(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setPort"]
        fn set_port(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setDatabase"]
        fn set_database(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setUser"]
        fn set_user(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setAuth"]
        fn set_auth(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setReadOnly"]
        fn set_read_only(self: &DataSourceEditor, value: bool);

        #[qinvokable]
        #[cxx_name = "setHistory"]
        fn set_history(self: &DataSourceEditor, value: bool);

        #[qinvokable]
        #[cxx_name = "setUrl"]
        fn set_url(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setSslMode"]
        fn set_ssl_mode(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setSslCaFile"]
        fn set_ssl_ca_file(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setSshHost"]
        fn set_ssh_host(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setSshPort"]
        fn set_ssh_port(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setSshUser"]
        fn set_ssh_user(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setSshAuth"]
        fn set_ssh_auth(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "setSshKeyFile"]
        fn set_ssh_key_file(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "isDirty"]
        fn is_dirty(self: &DataSourceEditor) -> bool;

        #[qinvokable]
        fn commit(self: &DataSourceEditor) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "hasPassword"]
        fn has_password(self: &DataSourceEditor) -> bool;

        #[qinvokable]
        #[cxx_name = "setPassword"]
        fn set_password(self: &DataSourceEditor, value: &QString);

        #[qinvokable]
        #[cxx_name = "passwordHint"]
        fn password_hint(self: &DataSourceEditor) -> QString;

        /// Attempt a real connection off the UI thread; reports through
        /// `testConnectionFinished`.
        #[qinvokable]
        #[cxx_name = "testConnection"]
        fn test_connection(self: Pin<&mut DataSourceEditor>);

        #[qsignal]
        #[cxx_name = "testConnectionFinished"]
        fn test_connection_finished(self: Pin<&mut DataSourceEditor>, ok: bool, message: QString);
    }

    impl cxx_qt::Threading for DataSourceEditor {}

    // ---- database: F2 ----

    /// Where one connected data source stands (F2.5) — the same
    /// disconnected/connecting/connected/error shape `FfiConnectionState`
    /// already gives the Containers dock, its own enum since a data
    /// source's states are never conflated with a container engine's.
    enum FfiDbConnectionState {
        Disconnected,
        Connecting,
        Connected,
        Error,
    }

    /// One data source row, for the dock's own source list (its toolbar's
    /// "New data source…"/status line, and the root of its tree).
    struct FfiDbSourceRow {
        id: QString,
        name: QString,
        driver: QString,
        color: QString,
        group: QString,
        state: FfiDbConnectionState,
        message: QString,
    }

    /// Which of a row's actions apply — `db_core::tree::actions_for`'s
    /// `ActionSet` crossed as discrete bools, the same convention
    /// `FfiNodeActions` already uses for the Containers dock so the view
    /// never has to decode a bitfield.
    #[derive(Default)]
    struct FfiDbRowActions {
        #[cxx_name = "canOpenConsole"]
        can_open_console: bool,
        #[cxx_name = "canEditData"]
        can_edit_data: bool,
        #[cxx_name = "canGoToDdl"]
        can_go_to_ddl: bool,
        #[cxx_name = "canCopyName"]
        can_copy_name: bool,
        #[cxx_name = "canCopyQualifiedName"]
        can_copy_qualified_name: bool,
        #[cxx_name = "canRefresh"]
        can_refresh: bool,
        #[cxx_name = "canRename"]
        can_rename: bool,
        #[cxx_name = "canDrop"]
        can_drop: bool,
        #[cxx_name = "canTruncate"]
        can_truncate: bool,
        #[cxx_name = "canComment"]
        can_comment: bool,
        #[cxx_name = "canGenerateDdl"]
        can_generate_ddl: bool,
        #[cxx_name = "canErDiagram"]
        can_er_diagram: bool,
        #[cxx_name = "canExportData"]
        can_export_data: bool,
        #[cxx_name = "canImportData"]
        can_import_data: bool,
        #[cxx_name = "canCopyTable"]
        can_copy_table: bool,
        /// Set only on the data source's own root row (F5b.3) — dump and
        /// schema/data compare operate on a whole source, not one object,
        /// so `db_core::tree::actions_for` never sees them.
        #[cxx_name = "canDump"]
        can_dump: bool,
        #[cxx_name = "canCompare"]
        can_compare: bool,
    }

    /// One flattened row of the Database dock's tree (database-tools-plan
    /// F2.2/F2.5) — `db_core::tree::TreeRow` crossed the seam, `nodeId`
    /// prefixed with its own source id (`bridge::database::tree::
    /// to_ffi_row`) so one flat list can hold every connected source's
    /// tree at once.
    struct FfiDbTreeRow {
        #[cxx_name = "sourceId"]
        source_id: QString,
        #[cxx_name = "nodeId"]
        node_id: QString,
        depth: i32,
        /// The row's own kind, as a stable id (`table`, `view`, `column`,
        /// `folder-tables`, …) — the view looks up its icon and context
        /// menu by this, never by a translated word.
        kind: QString,
        label: QString,
        detail: QString,
        expandable: bool,
        loaded: bool,
        actions: FfiDbRowActions,
    }

    extern "RustQt" {
        /// The Database dock's adapter (database-tools-plan F2.5): one
        /// `SessionWorker` per connected source, `db_core::tree::flatten`
        /// re-run whenever a source's schema, filter or grouping changes.
        /// Owns no rule: introspection levels/scopes, grouping, filters
        /// and the action matrix are all `db_core`'s.
        #[qobject]
        type DatabaseService = super::DatabaseServiceRust;

        /// Every configured data source, connected or not.
        #[qinvokable]
        fn sources(self: &DatabaseService) -> Vec<FfiDbSourceRow>;

        /// Every visible row across every connected source, in render
        /// order. Re-read after `rowsChanged`.
        #[qinvokable]
        fn rows(self: &DatabaseService) -> Vec<FfiDbTreeRow>;

        /// Spawn `id`'s `SessionWorker` and fetch its root schema at
        /// `Names` level. Named `connectSource` so it cannot shadow
        /// `QObject::connect`.
        #[qinvokable]
        #[cxx_name = "connectSource"]
        fn connect_source(self: Pin<&mut DatabaseService>, id: &QString) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "disconnectSource"]
        fn disconnect_source(self: Pin<&mut DatabaseService>, id: &QString) -> FfiResult;

        /// Fetch `node_id`'s children at `Columns` level if not already
        /// loaded (`TreeRow::loaded`) — a no-op, successful call for a
        /// node that is already loaded or is not expandable.
        #[qinvokable]
        fn expand(self: Pin<&mut DatabaseService>, node_id: &QString) -> FfiResult;

        /// Drop `node_id`'s own cached snapshot (its own `SessionWorker`'s
        /// cache) — the next `expand` re-fetches it.
        #[qinvokable]
        fn refresh(self: Pin<&mut DatabaseService>, node_id: &QString, force: bool) -> FfiResult;

        /// A plain substring, or a `kind:pattern`/`kind:-pattern` scoped
        /// one (`db_core::tree::PatternFilter`); empty clears it.
        #[qinvokable]
        #[cxx_name = "setFilter"]
        fn set_filter(self: Pin<&mut DatabaseService>, text: &QString);

        /// `true` renders every object as a flat sibling list; `false`
        /// (the default) groups tables/views/routines/… into per-kind
        /// folders.
        #[qinvokable]
        #[cxx_name = "setGrouping"]
        fn set_grouping(self: Pin<&mut DatabaseService>, flat: bool);

        /// Run `action_id` (`db_core::tree::ActionSet`'s own names,
        /// lower-`snake_case`: `"drop"`, `"truncate"`, `"rename:<new
        /// name>"`, `"comment:<text>"`, …) against `node_id`'s object,
        /// through its source's own session. Reports through
        /// `actionFinished`, not the returned `FfiResult` — the exact
        /// statement Rust generated is what a caller confirms *before*
        /// calling this at all (`db_core::ddl`'s own doc comment), so a
        /// refusal here is only ever "no such node"/"not connected", never
        /// "the statement failed" (that is `actionFinished(false, …)`).
        #[qinvokable]
        #[cxx_name = "runAction"]
        fn run_action(
            self: Pin<&mut DatabaseService>,
            node_id: &QString,
            action_id: &QString,
        ) -> FfiResult;

        /// Fetch `node_id`'s DDL (the engine's own text, or
        /// `db_core::ddl::synthesize`'s fallback) and open it through
        /// `virtualDocumentOpened`.
        #[qinvokable]
        #[cxx_name = "goToDdl"]
        fn go_to_ddl(self: Pin<&mut DatabaseService>, node_id: &QString) -> FfiResult;

        /// A source connected/disconnected, its schema changed, or a
        /// filter/grouping change — the view re-reads `rows()`.
        #[qsignal]
        #[cxx_name = "rowsChanged"]
        fn rows_changed(self: Pin<&mut DatabaseService>);

        #[qsignal]
        #[cxx_name = "connectionStateChanged"]
        fn connection_state_changed(
            self: Pin<&mut DatabaseService>,
            id: QString,
            state: FfiDbConnectionState,
            message: QString,
        );

        /// Mirrors `ContainerService::virtualDocumentOpened` exactly
        /// (`editor_tabs.cpp` wires both the same way): `is_new` tells the
        /// view whether to register a fresh tab or just focus the
        /// existing one for this scheme/key.
        #[qsignal]
        #[cxx_name = "virtualDocumentOpened"]
        fn virtual_document_opened(
            self: Pin<&mut DatabaseService>,
            tab_id: u64,
            title: QString,
            is_new: bool,
        );

        /// `runAction`'s own outcome, once the statement actually ran.
        #[qsignal]
        #[cxx_name = "actionFinished"]
        fn action_finished(self: Pin<&mut DatabaseService>, ok: bool, message: QString);

        /// "Open Console"/"Jump to console" (F3.1/F3.3): finds `node_id`'s
        /// source's first existing console file under `db_core::console::
        /// console_dir`, or creates `console1.sql` if none exist yet, and
        /// reports its path through `consoleFileReady` — the two actions
        /// are the same call, since "open or focus" is exactly what
        /// `EditorTabs::openFile` already does for a real file.
        #[qinvokable]
        #[cxx_name = "openConsole"]
        fn open_console(self: Pin<&mut DatabaseService>, node_id: &QString) -> FfiResult;

        /// A console file is ready to be opened as a normal editor tab —
        /// `editor_tabs.cpp` wires this straight to `openFile`, the same
        /// direct wiring `virtualDocumentOpened` already gets there.
        #[qsignal]
        #[cxx_name = "consoleFileReady"]
        fn console_file_ready(self: Pin<&mut DatabaseService>, path: QString, source_id: QString);
    }

    impl cxx_qt::Threading for DatabaseService {}

    // ---- database: F3 ----

    /// Which console statement(s) to run (F3.3): `Statement` splits the
    /// whole console buffer and runs only the one the caret sits in;
    /// `Selection` splits and runs just the given text, in order; `File`
    /// is `executeFile`'s own path (reads from disk), never a valid
    /// `execute` argument.
    enum FfiDbExecWhat {
        Statement,
        Selection,
        File,
    }

    /// Auto commits every statement on its own; Manual opens a
    /// transaction on the first statement, held open until `commit`/
    /// `rollback`.
    enum FfiDbTxMode {
        Auto,
        Manual,
    }

    /// How a multi-statement run responds to one statement failing.
    #[derive(PartialEq, Eq)]
    enum FfiDbScriptPolicy {
        StopOnError,
        Continue,
        Ask,
    }

    enum FfiDbTextFormat {
        Csv,
        Tsv,
        Json,
    }

    enum FfiDbAggOp {
        Sum,
        Avg,
        Min,
        Max,
        Count,
    }

    /// `db_core::error::DbError` crossed the seam (ADR-0003): `code` is
    /// `DbErrorCode`'s own discriminant, `line`/`col` are `-1` when the
    /// backend gives no statement position for the failure. `code == 0`
    /// (never a real `DbErrorCode` discriminant, `Unknown` is `0`... a
    /// caller distinguishes "no error" by the signal's own `ok` bool, not
    /// by this code, since `Unknown` legitimately shares `0`).
    #[derive(Default)]
    struct FfiDbError {
        code: i32,
        message: QString,
        line: i32,
        col: i32,
    }

    /// One result column (F3.4), 1:1 with `db_core::value::ColumnMeta`.
    struct FfiDbColumn {
        name: QString,
        #[cxx_name = "typeName"]
        type_name: QString,
        nullable: bool,
    }

    /// One result row, already rendered (`Value::display`). `cells`/
    /// `nulls` are `\u{1f}`-joined rather than `Vec<QString>` fields — a
    /// `Vec` field on a shared struct is not a shape cxx supports (see
    /// `FfiRunConfig::before_launch`'s own doc comment for the same
    /// convention).
    struct FfiDbRow {
        cells: QString,
        nulls: QString,
        /// The data editor's own pending-change bits (F4.1): bit 0
        /// edited, bit 1 deleted, bit 2 inserted, `0` for an untouched
        /// row or a non-editable result — `db_core::dml::EditBuffer::
        /// row_flags`'s own doc comment. The delegate's highlight.
        flags: u8,
    }

    /// F4.3's FK navigation (`ConsoleService::cellNavigation`): one target
    /// a cell offers, already resolved to a runnable statement — never a
    /// raw table/column pair the view would have to assemble SQL from
    /// itself (`CLAUDE.md`'s humble-view rule).
    struct FfiDbNavTarget {
        /// The context-menu label, e.g. `"Go to customers.id"` (forward)
        /// or `"5 rows in orders.customer_id"` (reverse, `count` already
        /// known).
        label: QString,
        /// A complete, already-bound `SELECT` — the cell's own value
        /// rendered through `db_core::value::Value::sql_literal`, never
        /// user-typed text (`ADR-0061` §1's "never interpolate untrusted
        /// text" is unaffected: this value came from a result the driver
        /// already returned, not from anything a user typed into this
        /// call).
        statement: QString,
    }

    extern "RustQt" {
        /// One console tab's execution engine (F3.1/F3.3): a dedicated
        /// `SessionWorker` per attached tab (never the Database dock's
        /// tree worker — see `bridge::database::console`'s own doc
        /// comment), read-only-guarded through `db_sql::classify::
        /// SqlClassifier`.
        #[qobject]
        type ConsoleService = super::ConsoleServiceRust;

        #[qinvokable]
        fn attach(self: Pin<&mut ConsoleService>, tab_id: u64, source_id: &QString) -> FfiResult;

        #[qinvokable]
        fn detach(self: Pin<&mut ConsoleService>, tab_id: u64);

        #[qinvokable]
        #[cxx_name = "setTxMode"]
        fn set_tx_mode(self: Pin<&mut ConsoleService>, tab_id: u64, mode: FfiDbTxMode)
            -> FfiResult;

        #[qinvokable]
        #[cxx_name = "setScriptPolicy"]
        fn set_script_policy(
            self: Pin<&mut ConsoleService>,
            tab_id: u64,
            policy: FfiDbScriptPolicy,
        );

        /// See `ConsoleServiceRust`'s `script_policy`'s own doc comment.
        #[qinvokable]
        #[cxx_name = "scriptPolicy"]
        fn script_policy(self: Pin<&mut ConsoleService>, tab_id: u64) -> FfiDbScriptPolicy;

        /// See `ConsoleService::schemas`'s own doc comment
        /// (`bridge::database::console`).
        #[qinvokable]
        fn schemas(self: Pin<&mut ConsoleService>, tab_id: u64) -> QStringList;

        /// See `ConsoleService::set_schema`'s own doc comment
        /// (`bridge::database::console`).
        #[qinvokable]
        #[cxx_name = "setSchema"]
        fn set_schema(self: Pin<&mut ConsoleService>, tab_id: u64, schema: &QString) -> FfiResult;

        /// Runs a statement/selection against `tab_id`'s attached source —
        /// see `FfiDbExecWhat`'s own doc comment for what `text`/`caret`
        /// mean per variant.
        #[qinvokable]
        fn execute(
            self: Pin<&mut ConsoleService>,
            tab_id: u64,
            text: &QString,
            what: FfiDbExecWhat,
            caret: u32,
        ) -> FfiResult;

        /// Runs `path`'s whole contents as a script against `source_id`
        /// (F3.6's run configuration entry point); `tab_id` is `0` when no
        /// console tab is involved.
        #[qinvokable]
        #[cxx_name = "executeFile"]
        fn execute_file(
            self: Pin<&mut ConsoleService>,
            tab_id: u64,
            path: &QString,
            source_id: &QString,
        ) -> FfiResult;

        #[qinvokable]
        fn cancel(self: Pin<&mut ConsoleService>, tab_id: u64) -> FfiResult;

        #[qinvokable]
        fn commit(self: Pin<&mut ConsoleService>, tab_id: u64) -> FfiResult;

        #[qinvokable]
        fn rollback(self: Pin<&mut ConsoleService>, tab_id: u64) -> FfiResult;

        /// Answers an `askContinue` — see `ConsoleService::resume`'s own
        /// doc comment (`bridge::database::console`).
        #[qinvokable]
        fn resume(self: Pin<&mut ConsoleService>, result_id: u64, proceed: bool);

        /// A source's execution history, statement text only, oldest
        /// first — never a bound parameter value (ADR-0061 §1).
        #[qinvokable]
        fn history(self: Pin<&mut ConsoleService>, source_id: &QString) -> QStringList;

        #[qinvokable]
        #[cxx_name = "clearHistory"]
        fn clear_history(self: Pin<&mut ConsoleService>, source_id: &QString) -> FfiResult;

        /// See `ConsoleServiceRust::source_for_path`'s own doc comment.
        #[qinvokable]
        #[cxx_name = "sourceForPath"]
        fn source_for_path(self: Pin<&mut ConsoleService>, path: &QString) -> QString;

        /// See `ConsoleServiceRust::available_sources`'s own doc comment.
        #[qinvokable]
        #[cxx_name = "availableSources"]
        fn available_sources(self: Pin<&mut ConsoleService>) -> Vec<FfiDbSourceRow>;

        /// See `ConsoleService::dml_preview`'s own doc comment
        /// (`bridge::database::console`) — F4.2's DML preview.
        #[qinvokable]
        #[cxx_name = "dmlPreview"]
        fn dml_preview(self: Pin<&mut ConsoleService>, result_id: u64) -> FfiResult;

        /// See `ConsoleService::submit`'s own doc comment
        /// (`bridge::database::console`) — F4.2's submit; the outcome
        /// arrives asynchronously through `submitFinished`.
        #[qinvokable]
        fn submit(self: Pin<&mut ConsoleService>, result_id: u64) -> FfiResult;

        /// A statement started executing — `index`/`count` are 1-based
        /// position within a multi-statement run (`1`/`1` for a lone
        /// statement).
        #[qsignal]
        #[cxx_name = "executionStarted"]
        fn execution_started(
            self: Pin<&mut ConsoleService>,
            tab_id: u64,
            result_id: u64,
            index: u32,
            count: u32,
        );

        /// `count` more rows landed in `result_id`'s buffer, starting at
        /// (0-based) `first` — the grid re-reads through `ResultProvider`.
        #[qsignal]
        #[cxx_name = "rowsAppended"]
        fn rows_appended(self: Pin<&mut ConsoleService>, result_id: u64, first: u64, count: u64);

        /// `result_id` is done: `affected` is the row count for a
        /// `Rows`/`Affected` shape, `0` for a plain `Ok`. `error.code == 0`
        /// alone never means success — read `ok`.
        #[qsignal]
        #[cxx_name = "executionFinished"]
        fn execution_finished(
            self: Pin<&mut ConsoleService>,
            result_id: u64,
            ok: bool,
            affected: u64,
            elapsed_ms: u64,
            error: FfiDbError,
        );

        /// A `StopOnError`/`Ask`-policy script hit a failing statement and
        /// more remain — `message` is the failing statement's own error
        /// text, for a confirmation dialog to show; the view offers
        /// Continue/Stop, then calls `resume`.
        #[qsignal]
        #[cxx_name = "askContinue"]
        fn ask_continue(self: Pin<&mut ConsoleService>, result_id: u64, message: QString);

        /// Free-text status for the console's Output tab (attach/detach
        /// outcomes, transaction errors) — never a substitute for
        /// `executionFinished`'s typed error.
        #[qsignal]
        #[cxx_name = "outputAppended"]
        fn output_appended(self: Pin<&mut ConsoleService>, tab_id: u64, text: QString);

        /// A memory-cap-reached signal distinct from `executionFinished`
        /// (F3.4's own "Fetch more" affordance) — the execution itself
        /// still finishes normally right after this, since the statement
        /// did complete, only paging further stopped.
        #[qsignal]
        #[cxx_name = "capReached"]
        fn cap_reached_signal(self: Pin<&mut ConsoleService>, result_id: u64);

        /// `result_id`'s editability decision is in (F4.1) — `reason` is
        /// empty when `editable`. Fires once a candidate single-table
        /// result's `Full`-level introspect lands, or immediately for a
        /// read-only source / a statement that is not a single table.
        #[qsignal]
        #[cxx_name = "editabilityChanged"]
        fn editability_changed(
            self: Pin<&mut ConsoleService>,
            result_id: u64,
            editable: bool,
            reason: QString,
        );

        /// `result_id`'s `dmlPreview` opened a tab — same shape as
        /// `DatabaseService::virtualDocumentOpened`, a separate signal
        /// because it is a different `QObject`.
        #[qsignal]
        #[cxx_name = "virtualDocumentOpened"]
        fn virtual_document_opened(
            self: Pin<&mut ConsoleService>,
            tab_id: u64,
            title: QString,
            is_new: bool,
        );

        /// `submit`'s own outcome (F4.2) — `message` is a human summary on
        /// success, the failing statement's error text otherwise.
        #[qsignal]
        #[cxx_name = "submitFinished"]
        fn submit_finished(
            self: Pin<&mut ConsoleService>,
            result_id: u64,
            ok: bool,
            message: QString,
        );

        /// A successful submit's own refresh: `old_result_id`'s page is
        /// stale, `new_result_id` is the freshly re-run replacement the
        /// grid should switch to.
        #[qsignal]
        #[cxx_name = "resultRefreshed"]
        fn result_refreshed(self: Pin<&mut ConsoleService>, old_result_id: u64, new_result_id: u64);

        // ---- database: F4b ----

        /// F4.3's exact aggregate: re-runs `column`'s `op` as `SELECT
        /// op(col), COUNT(*) FROM (<result's own statement>) t` over its
        /// own short-lived connection (`bridge::database::edit`'s own doc
        /// comment on why not the console's shared worker) — the outcome
        /// arrives asynchronously through `aggregateComputed`, unlike
        /// `ResultProvider::aggregate`'s synchronous fetched-rows-only
        /// estimate.
        #[qinvokable]
        #[cxx_name = "aggregateExact"]
        fn aggregate_exact(
            self: Pin<&mut ConsoleService>,
            result_id: u64,
            column: &QString,
            op: FfiDbAggOp,
        ) -> FfiResult;

        /// `aggregateExact`'s outcome: `value` is the aggregate's own
        /// display text on success (`ok`) or an error message otherwise;
        /// `row_count` is the whole result's row count (`COUNT(*)` over
        /// the same derived table), for the footer's "computed over all N
        /// rows" text.
        #[qsignal]
        #[cxx_name = "aggregateComputed"]
        fn aggregate_computed(
            self: Pin<&mut ConsoleService>,
            result_id: u64,
            column: QString,
            op: FfiDbAggOp,
            ok: bool,
            value: QString,
            row_count: u64,
        );

        /// F4.3's FK navigation, forward direction only this pass ("Show
        /// referencing rows…" needs a whole-schema FK index this result's
        /// own `Full`-level table introspect does not fetch — left out,
        /// see `bridge::database::edit`'s own doc comment): every target
        /// `row`/`column`'s cell offers, decided from the table's own
        /// constraint detail (`db_core::schema::ConstraintKind`, F6c) —
        /// empty when the result is not a single-table result, the column
        /// is not part of a foreign key, or the constraint detail is not
        /// in hand yet (the same `Full`-level introspect F4.1's
        /// editability check already triggers backs this — a result
        /// offers navigation exactly when it offers editing, since both
        /// need the same snapshot).
        #[qinvokable]
        #[cxx_name = "cellNavigation"]
        fn cell_navigation(
            self: Pin<&mut ConsoleService>,
            result_id: u64,
            row: u64,
            column: &QString,
        ) -> Vec<FfiDbNavTarget>;

        /// Runs `target`'s own statement (already bound — `db_core::ddl`'s
        /// own "never interpolate" rule) as a fresh result on `result_id`'s
        /// console, same as `applyClauses` — the new result id, in
        /// `message`, same convention.
        #[qinvokable]
        #[cxx_name = "goToNavTarget"]
        fn go_to_nav_target(
            self: Pin<&mut ConsoleService>,
            result_id: u64,
            target: &FfiDbNavTarget,
        ) -> FfiResult;
    }

    impl cxx_qt::Threading for ConsoleService {}

    extern "RustQt" {
        /// A result's rows and text/aggregate views (F3.4/F3.5) — reads
        /// `bridge::database::console::Shared`, the same state
        /// `ConsoleService` populates, since cxx-qt gives two QObjects no
        /// way to share a constructor argument (see that module's own doc
        /// comment).
        #[qobject]
        type ResultProvider = super::ResultProviderRust;

        #[qinvokable]
        fn columns(self: Pin<&mut ResultProvider>, result_id: u64) -> Vec<FfiDbColumn>;

        #[qinvokable]
        #[cxx_name = "rowCount"]
        fn row_count(self: Pin<&mut ResultProvider>, result_id: u64) -> u64;

        #[qinvokable]
        #[cxx_name = "rowPage"]
        fn row_page(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            first: u64,
            count: u64,
        ) -> Vec<FfiDbRow>;

        /// Asks the parked stream for its next page — `Err` once the
        /// result already finished (nothing left to fetch) or the console
        /// detached underneath it.
        #[qinvokable]
        #[cxx_name = "fetchMore"]
        fn fetch_more(self: Pin<&mut ResultProvider>, result_id: u64) -> FfiResult;

        /// See `ConsoleServiceRust::fetch_more_available`'s own doc
        /// comment (`ResultProvider` reads the same shared state).
        #[qinvokable]
        #[cxx_name = "fetchMoreAvailable"]
        fn fetch_more_available(self: Pin<&mut ResultProvider>, result_id: u64) -> bool;

        #[qinvokable]
        #[cxx_name = "setPageSize"]
        fn set_page_size(self: Pin<&mut ResultProvider>, result_id: u64, size: u32);

        #[qinvokable]
        #[cxx_name = "textView"]
        fn text_view(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            format: FfiDbTextFormat,
        ) -> QString;

        /// A best-effort aggregate over the rows fetched so far (this
        /// type's own doc comment on `aggregate`'s ponytail note).
        #[qinvokable]
        fn aggregate(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            column: &QString,
            op: FfiDbAggOp,
        ) -> QString;

        /// Re-executes the result's statement wrapped as a derived table
        /// with `WHERE`/`ORDER BY` applied — a fresh execution, reported
        /// through `ConsoleService`'s own signals (see this module's doc
        /// comment on why `ResultProvider` cannot emit them itself).
        #[qinvokable]
        #[cxx_name = "applyClauses"]
        fn apply_clauses(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            where_clause: &QString,
            order_by: &QString,
        ) -> FfiResult;

        // ---- data editor (F4.1/F4.2) ----

        /// See `EditState`'s own doc comment (`bridge::database::console`).
        #[qinvokable]
        #[cxx_name = "isEditable"]
        fn is_editable(self: Pin<&mut ResultProvider>, result_id: u64) -> bool;

        #[qinvokable]
        #[cxx_name = "notEditableReason"]
        fn not_editable_reason(self: Pin<&mut ResultProvider>, result_id: u64) -> QString;

        #[qinvokable]
        #[cxx_name = "pendingCount"]
        fn pending_count(self: Pin<&mut ResultProvider>, result_id: u64) -> u64;

        /// See `ResultProvider::set_cell`'s own doc comment
        /// (`bridge::database::console`).
        #[qinvokable]
        #[cxx_name = "setCell"]
        fn set_cell(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            row: u64,
            column: &QString,
            text: &QString,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "setNull"]
        fn set_null(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            row: u64,
            column: &QString,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "setDefault"]
        fn set_default(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            row: u64,
            column: &QString,
        ) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "revertCell"]
        fn revert_cell(
            self: Pin<&mut ResultProvider>,
            result_id: u64,
            row: u64,
            column: &QString,
        ) -> FfiResult;

        /// See `ResultProvider::add_row`'s own doc comment — the new
        /// row's grid index travels back in `FfiResult::message`.
        #[qinvokable]
        #[cxx_name = "addRow"]
        fn add_row(self: Pin<&mut ResultProvider>, result_id: u64) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "cloneRow"]
        fn clone_row(self: Pin<&mut ResultProvider>, result_id: u64, row: u64) -> FfiResult;

        #[qinvokable]
        #[cxx_name = "deleteRows"]
        fn delete_rows(self: Pin<&mut ResultProvider>, result_id: u64, rows: Vec<u64>)
            -> FfiResult;

        #[qinvokable]
        fn revert(self: Pin<&mut ResultProvider>, result_id: u64);

        /// See `db_core::value::pretty_json`'s own doc comment — the
        /// value editor's JSON pretty-print toggle. Stateless (no
        /// `result_id`): a plain text transform, not a per-result
        /// question, kept on this `QObject` only because the value
        /// editor already talks to it for everything else.
        #[qinvokable]
        #[cxx_name = "prettyJson"]
        fn pretty_json(self: Pin<&mut ResultProvider>, text: &QString) -> FfiResult;
    }

    impl cxx_qt::Threading for ResultProvider {}

    // ---- database: F8b ----

    /// One `adbc`-backend `database-drivers` row's install status (F8.5,
    /// `database-tools.md` §9) — plain text plus two booleans the dialog
    /// needs to pick which button (if any) to show.
    #[derive(Default)]
    struct FfiDriverStatus {
        text: QString,
        installable: bool,
        #[cxx_name = "canReenable"]
        can_reenable: bool,
    }

    /// What the consent dialog names before an install: the pinned
    /// artifact's own URL/sha256/publisher (domain), empty when this row
    /// has no artifact for the current platform.
    #[derive(Default)]
    struct FfiDriverConsent {
        url: QString,
        sha256: QString,
        publisher: QString,
    }

    extern "RustQt" {
        /// Whether installing a driver contributed by a plugin other than
        /// the built-in `database-tools` one is allowed (ADR-0061 §4,
        /// `[database] allow_third_party_drivers`, default off).
        #[qinvokable]
        #[cxx_name = "allowThirdPartyDrivers"]
        fn allow_third_party_drivers(self: &AppSettings) -> bool;

        #[qinvokable]
        #[cxx_name = "setAllowThirdPartyDrivers"]
        fn set_allow_third_party_drivers(self: &AppSettings, value: bool) -> FfiResult;
    }

    extern "RustQt" {
        /// Drives `db_driver_adbc::{install,quarantine}` for one `adbc`
        /// row at a time (F8.5): a status line, a consent-dialog summary,
        /// installing off the UI thread, and re-enabling a quarantined
        /// driver.
        #[qobject]
        type DriverInstallService = super::DriverInstallServiceRust;

        #[qinvokable]
        fn status(self: &DriverInstallService, driver_id: &QString) -> FfiDriverStatus;

        #[qinvokable]
        fn consent(self: &DriverInstallService, driver_id: &QString) -> FfiDriverConsent;

        /// Downloads, verifies and installs this row's pinned artifact for
        /// this platform, off the UI thread; reports through
        /// `installFinished`. Refused immediately (no thread spawned) when
        /// third-party installs are off for a non-builtin row, or when
        /// this row has no artifact for this platform.
        #[qinvokable]
        fn install(self: Pin<&mut DriverInstallService>, driver_id: &QString) -> FfiResult;

        #[qinvokable]
        fn reenable(self: &DriverInstallService, driver_id: &QString) -> FfiResult;

        #[qsignal]
        #[cxx_name = "installFinished"]
        fn install_finished(
            self: Pin<&mut DriverInstallService>,
            driver_id: QString,
            ok: bool,
            message: QString,
        );
    }

    impl cxx_qt::Threading for DriverInstallService {}

    // ---- database: F5b ----

    /// Every export format `db_exchange::export` offers, plus the two
    /// text variants that need a per-format flag of their own (JSON
    /// Lines vs. a single array, an `UPDATE` vs. an `INSERT` SQL body) —
    /// folded into the format itself rather than a separate options bit,
    /// since a dialog's format combo already has to name them as distinct
    /// choices.
    #[repr(i32)]
    enum FfiExportFormat {
        Csv,
        Tsv,
        Json,
        JsonLines,
        Markdown,
        Html,
        SqlInsert,
        SqlUpdate,
        Xlsx,
    }

    /// The knobs every export format reads a subset of
    /// (`db_exchange::export::ExportOptions`'s own doc comment on why one
    /// shared struct rather than one per format).
    struct FfiExportOptions {
        header: bool,
        #[cxx_name = "nullText"]
        null_text: QString,
        #[cxx_name = "quoteAll"]
        quote_all: bool,
        /// A single character; empty defaults to `,` (CSV) / tab (TSV).
        delimiter: QString,
        #[cxx_name = "dateFormat"]
        date_format: QString,
        #[cxx_name = "tableName"]
        table_name: QString,
        /// Comma-separated key columns for `SqlUpdate`'s `WHERE` clause.
        #[cxx_name = "keyColumns"]
        key_columns: QString,
    }

    /// Source-parsing knobs for a CSV/XLSX import
    /// (`db_exchange::import::ImportOptions` crossed the seam).
    struct FfiImportOptions {
        header: bool,
        #[cxx_name = "nullText"]
        null_text: QString,
        delimiter: QString,
        #[cxx_name = "dateFormat"]
        date_format: QString,
        /// Folded in here rather than two more `importRun` parameters —
        /// `db_exchange::import::plan`'s own knobs, kept beside the rest
        /// of this source's parsing options to keep `importRun` under
        /// clippy's argument-count ceiling.
        #[cxx_name = "createTable"]
        create_table: bool,
        #[cxx_name = "batchSize"]
        batch_size: u32,
    }

    /// A sampled look at an import source (`db_exchange::import::
    /// ImportPreview`): every column's detected type, alongside its own
    /// name, so the dialog's mapping table has one row per source column
    /// with no further round trip.
    #[derive(Default)]
    struct FfiImportPreview {
        columns: QStringList,
        /// The detected type per column, same order as `columns` — one
        /// of `"int"`/`"float"`/`"bool"`/`"date"`/`"text"`.
        #[cxx_name = "detectedTypes"]
        detected_types: QStringList,
        /// The sample rows, each `\u{1f}`-joined (same convention as
        /// `FfiDbRow::cells`), one `QString` per row, capped at
        /// `db_exchange::import::PREVIEW_SAMPLE_ROWS`.
        #[cxx_name = "sampleRows"]
        sample_rows: QStringList,
    }

    /// One mapped column of an import (`db_exchange::import::
    /// ColumnMapping` crossed the seam) — the dialog's mapping table has
    /// one editable row per source column, built from `type_coercion` as
    /// plain text (`"int"`/`"float"`/`"bool"`/`"date"`/`"text"`) rather
    /// than the Rust enum, so the view never needs a second copy of that
    /// vocabulary.
    struct FfiImportColumnMapping {
        #[cxx_name = "sourceCol"]
        source_col: QString,
        #[cxx_name = "targetCol"]
        target_col: QString,
        #[cxx_name = "typeCoercion"]
        type_coercion: QString,
        skip: bool,
    }

    /// One row of a schema compare's summary (`db_exchange::
    /// schema_compare::SchemaDiff`, flattened) — `kind` is one of
    /// `"added-table"`/`"dropped-table"`/`"changed-table"`/`"view"`/
    /// `"routine"`, `table`/`name` the same pair `ObjectRef` carries so
    /// `openDdlDiff` can be handed exactly what it needs back.
    struct FfiCompareRow {
        kind: QString,
        table: QString,
        name: QString,
    }

    /// Two already-rendered texts for `DocumentManager::openDiffTab` —
    /// `ExchangeService` computes the text, the dialog opens the tab, so
    /// this crosses the seam once rather than the dialog re-deriving
    /// either side itself.
    #[derive(Default)]
    struct FfiTextDiff {
        left: QString,
        right: QString,
        label: QString,
    }

    /// A key-aligned data compare's outcome (`db_exchange::data_compare::
    /// DataDiffSummary` plus its two canonical TSVs, one round trip).
    #[derive(Default)]
    struct FfiDataCompareResult {
        #[cxx_name = "onlyLeft"]
        only_left: u64,
        #[cxx_name = "onlyRight"]
        only_right: u64,
        changed: u64,
        equal: u64,
        #[cxx_name = "leftText"]
        left_text: QString,
        #[cxx_name = "rightText"]
        right_text: QString,
    }

    /// What a dump/restore run should cover
    /// (`db_exchange::dump::DumpOptions` crossed the seam).
    struct FfiDumpOptions {
        #[cxx_name = "schemaOnly"]
        schema_only: bool,
        #[cxx_name = "dataOnly"]
        data_only: bool,
        /// Comma-separated; empty means every table.
        tables: QString,
        #[cxx_name = "outputFile"]
        output_file: QString,
    }

    /// Whether this source's dump tool is on `PATH`, and what to tell the
    /// user if not (`db_exchange::dump::{tool_available,install_hint}`).
    #[derive(Default)]
    struct FfiDumpToolStatus {
        program: QString,
        available: bool,
        #[cxx_name = "installHint"]
        install_hint: QString,
    }

    extern "RustQt" {
        /// Export/import/dump/copy-table/ER-diagram/schema-and-data-
        /// compare (database-tools-plan F5/F6, crate half in
        /// `db-exchange`): every long-running operation runs on its own
        /// thread and reports through `jobProgress`/`jobFinished`; every
        /// short one (a preview, a diagram, a compare summary) answers
        /// directly. Translation only, same as every other `bridge::
        /// database` QObject — every rule lives in `db_exchange`/
        /// `db_core`.
        #[qobject]
        type ExchangeService = super::ExchangeServiceRust;

        /// Streams `SELECT * FROM` `objectPath` (already-qualified, as the
        /// tree gives it) out to `destination` in `format`, never
        /// materialising the whole result in memory. Returns a job id
        /// immediately; `0` means it could not even start (see the
        /// `jobFinished(0, ...)` this still emits before returning, so a
        /// caller never has to special-case the synchronous-failure path).
        #[qinvokable]
        #[cxx_name = "exportTable"]
        fn export_table(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
            object_path: &QString,
            format: FfiExportFormat,
            options: FfiExportOptions,
            destination: &QString,
        ) -> u64;

        /// The first `maxRows` of `objectPath`, rendered in `format` —
        /// the export dialog's own preview pane.
        #[qinvokable]
        #[cxx_name = "exportPreviewText"]
        fn export_preview_text(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
            object_path: &QString,
            format: FfiExportFormat,
            options: FfiExportOptions,
            max_rows: u32,
        ) -> QString;

        /// "Copy as"/"Export…" on an already-executed result grid: the
        /// rows are already fetched and rendered (`FfiDbRow`, the same
        /// shape `ResultProvider::rowPage` returns), so this only
        /// reformats and writes them — no session, no re-query.
        /// ponytail: cells arrive pre-rendered as display text, not typed
        /// `Value`s, so a `SqlInsert`/`SqlUpdate` export quotes every
        /// value as text rather than its real type; upgrade once a typed
        /// row accessor exists on the result (F4a's `results.rs` split).
        #[qinvokable]
        #[cxx_name = "exportRowsToFile"]
        fn export_rows_to_file(
            self: Pin<&mut ExchangeService>,
            columns: &QStringList,
            rows: Vec<FfiDbRow>,
            format: FfiExportFormat,
            options: FfiExportOptions,
            destination: &QString,
        ) -> FfiResult;

        /// Same rendering as `exportRowsToFile`, returned as text for the
        /// clipboard rather than written to a file.
        #[qinvokable]
        #[cxx_name = "exportRowsToText"]
        fn export_rows_to_text(
            self: Pin<&mut ExchangeService>,
            columns: &QStringList,
            rows: Vec<FfiDbRow>,
            format: FfiExportFormat,
            options: FfiExportOptions,
        ) -> QString;

        /// Samples `path` (CSV or XLSX, by extension) and guesses each
        /// column's type — `db_exchange::import::{csv,xlsx}::preview`.
        #[qinvokable]
        #[cxx_name = "importPreview"]
        fn import_preview(
            self: Pin<&mut ExchangeService>,
            path: &QString,
            options: FfiImportOptions,
        ) -> FfiImportPreview;

        /// Reads `path` fully, compiles it under `mapping` against
        /// `targetTable`'s dialect, and runs it on `sourceId` — a job, the
        /// same shape as `exportTable`.
        #[qinvokable]
        #[cxx_name = "importRun"]
        fn import_run(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
            path: &QString,
            target_table: &QString,
            mapping: Vec<FfiImportColumnMapping>,
            options: FfiImportOptions,
        ) -> u64;

        /// Copies every row of `srcTable` (on `srcSource`) into
        /// `dstTable` (on `dstSource`) — a job, `db_exchange::
        /// copy_table::copy_table` on a fresh pair of connections.
        #[qinvokable]
        #[cxx_name = "copyTable"]
        fn copy_table(
            self: Pin<&mut ExchangeService>,
            src_source: &QString,
            src_table: &QString,
            dst_source: &QString,
            dst_table: &QString,
            create_if_missing: bool,
            batch_size: u32,
        ) -> u64;

        /// `sourceId`'s schema (or just `tableScope` plus its FK
        /// neighbours, when non-empty) as Mermaid `erDiagram` text —
        /// `db_exchange::er_diagram::to_mermaid`. The dialog opens it as
        /// a read-only virtual document via `DocumentManager::
        /// openVirtualDocument("mermaid", …)` itself.
        #[qinvokable]
        #[cxx_name = "erDiagramMermaid"]
        fn er_diagram_mermaid(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
            table_scope: &QString,
        ) -> QString;

        /// The same diagram, already rasterised at `widthPx` — the same
        /// `FfiPreviewImage` shape `PreviewProvider::previewImages`
        /// returns, so the dialog that shows it needs no second
        /// `QImage`-building code path.
        #[qinvokable]
        #[cxx_name = "erDiagramImage"]
        fn er_diagram_image(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
            table_scope: &QString,
            width_px: u32,
        ) -> FfiPreviewImage;

        /// Compares `leftSource`'s schema against `rightSource`'s
        /// (`db_exchange::schema_compare::compare`), caching the diff (by
        /// this same source pair) for `openDdlDiff`/`migrationScript` to
        /// read back without re-introspecting either side.
        #[qinvokable]
        #[cxx_name = "schemaCompare"]
        fn schema_compare(
            self: Pin<&mut ExchangeService>,
            left_source: &QString,
            right_source: &QString,
        ) -> Vec<FfiCompareRow>;

        /// One changed object's before/after DDL text, from the diff
        /// `schemaCompare` last cached for this exact source pair — the
        /// dialog opens the returned texts with `DocumentManager::
        /// openDiffTab` itself.
        #[qinvokable]
        #[cxx_name = "ddlDiffTexts"]
        fn ddl_diff_texts(
            self: Pin<&mut ExchangeService>,
            left_source: &QString,
            right_source: &QString,
            table: &QString,
            name: &QString,
        ) -> FfiTextDiff;

        /// The forward migration script (`leftSource` -> `rightSource`)
        /// for the same cached diff, in `rightSource`'s dialect — the
        /// dialog opens it as a virtual document
        /// (`"db-migration"`/`.sql`) itself.
        #[qinvokable]
        #[cxx_name = "migrationScript"]
        fn migration_script(
            self: Pin<&mut ExchangeService>,
            left_source: &QString,
            right_source: &QString,
        ) -> QString;

        /// Key-aligned data compare (`db_exchange::data_compare::
        /// compare`) between two tables, same source or different —
        /// fetches both sides fully (F6.3's recorded scope: a caller
        /// that needs a streamed compare over a huge table pages both
        /// sides itself, per that module's own doc comment).
        #[qinvokable]
        #[cxx_name = "dataCompare"]
        fn data_compare(
            self: Pin<&mut ExchangeService>,
            left_source: &QString,
            left_table: &QString,
            right_source: &QString,
            right_table: &QString,
            key_columns: &QString,
            tolerance: f64,
        ) -> FfiDataCompareResult;

        /// The argv `dump`/`restore` would run, shell-quoted for display
        /// only — `db_exchange::dump::preview`. Never includes a
        /// password (the module's own guarantee).
        #[qinvokable]
        #[cxx_name = "dumpArgvPreview"]
        fn dump_argv_preview(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
            options: FfiDumpOptions,
        ) -> QString;

        /// Whether `sourceId`'s dump tool is on `PATH`, and an install
        /// hint if not — `db_exchange::dump::{tool_available,
        /// install_hint}`.
        #[qinvokable]
        #[cxx_name = "dumpToolStatus"]
        fn dump_tool_status(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
        ) -> FfiDumpToolStatus;

        /// Runs `sourceId`'s dump tool (`pg_dump`/`mysqldump`/
        /// `mongodump`/`sqlite3 .dump`), streaming stderr lines through
        /// `jobProgress` as they arrive and stdout to `options.
        /// outputFile` (a tool that already writes its own output file,
        /// `pg_dump`/`mongodump`, gets no stdout to speak of). Restore is
        /// not implemented in this pass — see `database-tools.md` §4's
        /// recorded gap.
        #[qinvokable]
        fn dump(
            self: Pin<&mut ExchangeService>,
            source_id: &QString,
            options: FfiDumpOptions,
        ) -> u64;

        /// Best-effort: sets the job's cancel flag, checked between
        /// batches/lines by whichever job is running. A job already
        /// finished, or an unknown id, is a silent no-op.
        #[qinvokable]
        #[cxx_name = "cancelJob"]
        fn cancel_job(self: Pin<&mut ExchangeService>, job_id: u64) -> FfiResult;

        #[qsignal]
        #[cxx_name = "jobProgress"]
        fn job_progress(
            self: Pin<&mut ExchangeService>,
            job_id: u64,
            done: u64,
            total: u64,
            message: QString,
        );

        #[qsignal]
        #[cxx_name = "jobFinished"]
        fn job_finished(
            self: Pin<&mut ExchangeService>,
            job_id: u64,
            ok: bool,
            message: QString,
            #[cxx_name = "outputPath"] output_path: QString,
        );
    }

    impl cxx_qt::Threading for ExchangeService {}

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
