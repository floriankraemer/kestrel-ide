//! VT100/ANSI grid state for the embedded terminal, built on
//! `alacritty_terminal`'s `Term`/grid machinery.
//!
//! Qt-free by design (see `docs/architecture/layering.md`): this crate turns
//! a raw PTY byte stream into a renderable cell grid, cursor position, and
//! basic per-cell attributes. It does not own a PTY — `pty-core` (task F1)
//! owns the byte stream, and `ui-shell` wires the
//! two together, feeding bytes read from a `pty_core::PtySession` into
//! [`TerminalEmulator::feed`].

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Processor, Rgb};

pub mod keys;
mod sgr;

pub use sgr::{SgrResolver, StyledRun, StyledText, TextStyle};

/// Terminal size in character cells, mirroring `pty_core::PtySize` without
/// depending on `pty-core` — `terminal-core` stays a standalone emulation
/// crate (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub rows: usize,
    pub cols: usize,
}

impl GridSize {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self { rows, cols }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// An RGB color as it should be rendered — resolved from `alacritty_terminal`'s
/// [`AnsiColor`], which can otherwise name a color indirectly (a palette
/// index or a named ANSI slot). The view (`ui-shell`) shouldn't need
/// its own copy of the default 16-color ANSI palette just to paint cells, so
/// resolution happens here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl CellColor {
    const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn from_ansi(color: AnsiColor, default: CellColor, ansi: &[CellColor; 16]) -> Self {
        Self::from_ansi_opt(color, ansi).unwrap_or(default)
    }

    /// The same resolution, but saying "use the caller's default" as `None`
    /// rather than substituting a color.
    ///
    /// A grid cell must end up with a concrete color to paint, so
    /// [`CellColor::from_ansi`] folds the default in immediately. Streamed
    /// console text must not: SGR 39/49 mean "back to the view's default",
    /// and a run console that baked a color in there would stop following
    /// the editor theme. Both sinks resolve through this one function so
    /// the palette stays in one place (`crate::sgr`).
    pub(crate) fn from_ansi_opt(color: AnsiColor, ansi: &[CellColor; 16]) -> Option<Self> {
        match color {
            AnsiColor::Spec(Rgb { r, g, b }) => Some(CellColor::rgb(r, g, b)),
            AnsiColor::Named(named) => named_color_opt(named, ansi),
            AnsiColor::Indexed(idx) => indexed_color(idx, ansi),
        }
    }
}

/// Resolve a [`NamedColor`] to a palette entry, or `None` where the name
/// means "the caller's default".
///
/// `NamedColor` is *not* a palette index: only its first 16 variants line up
/// with the ANSI 0-15 table, while `Foreground`/`Background`/`Cursor` and the
/// `Dim*` tail have discriminants past 255. Casting the whole enum to `u8`
/// therefore wrapped a default-background cell onto palette slot 1 (red).
fn named_color_opt(named: NamedColor, ansi: &[CellColor; 16]) -> Option<CellColor> {
    let index = match named {
        // Not palette slots — these mean "whatever the caller's default is",
        // which `from_ansi` threads through per fg/bg call and `from_ansi_opt`
        // hands back to its caller as `None`.
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => return None,
        // Dim variants share the ANSI 0-7 hues; using the normal slot is a
        // fair approximation until a real theme/palette lands.
        NamedColor::DimBlack => 0,
        NamedColor::DimRed => 1,
        NamedColor::DimGreen => 2,
        NamedColor::DimYellow => 3,
        NamedColor::DimBlue => 4,
        NamedColor::DimMagenta => 5,
        NamedColor::DimCyan => 6,
        NamedColor::DimWhite => 7,
        // `Black`..`BrightWhite` really do occupy discriminants 0-15.
        other => other as u8,
    };
    indexed_color(index, ansi)
}

/// Resolve any `Indexed` color (0-255): 0-15 is the caller's 16-color
/// palette (theme-able, T3), 16-231 is the standard 6x6x6 color cube, and
/// 232-255 is the 24-step greyscale ramp — both fixed by the xterm 256-color
/// spec, so unlike the 16-color table they carry no theme of their own.
fn indexed_color(idx: u8, ansi: &[CellColor; 16]) -> Option<CellColor> {
    match idx {
        0..=15 => ansi.get(idx as usize).copied(),
        16..=231 => Some(cube_color(idx)),
        _ => Some(grey_ramp_color(idx)),
    }
}

/// xterm's 6x6x6 color cube (indices 16-231): index `16 + 36r + 6g + b`,
/// each of `r`/`g`/`b` in `0..6` mapping through this level table rather
/// than a linear `0..256` step.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn cube_color(idx: u8) -> CellColor {
    let i = idx - 16;
    let r = (i / 36) % 6;
    let g = (i / 6) % 6;
    let b = i % 6;
    CellColor::rgb(
        CUBE_LEVELS[r as usize],
        CUBE_LEVELS[g as usize],
        CUBE_LEVELS[b as usize],
    )
}

/// xterm's 24-step greyscale ramp (indices 232-255): level `8 + 10*i` for
/// `i` in `0..24`, equal on all three channels.
fn grey_ramp_color(idx: u8) -> CellColor {
    let level = 8 + 10 * (idx - 232);
    CellColor::rgb(level, level, level)
}

/// The 16-color ANSI palette plus the fixed points (foreground, background,
/// cursor, selection) a terminal paints with — the one place every color a
/// grid cell or a streamed console run resolves through, other than the
/// 256-color cube/greyscale ramp above (fixed by the xterm spec, not
/// themeable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub foreground: CellColor,
    pub background: CellColor,
    pub cursor: CellColor,
    pub selection: CellColor,
    pub ansi: [CellColor; 16],
}

/// The 16-color table `Palette::xterm()` and every "never themed" caller
/// (`SgrResolver`, whose run-console colors this task's hard requirement
/// keeps byte-identical) use. Values match the conventional xterm default
/// palette — copied byte-for-byte from what was, before this task, a
/// hardcoded table with no name.
const XTERM_ANSI: [CellColor; 16] = [
    CellColor::rgb(0, 0, 0),
    CellColor::rgb(205, 0, 0),
    CellColor::rgb(0, 205, 0),
    CellColor::rgb(205, 205, 0),
    CellColor::rgb(0, 0, 238),
    CellColor::rgb(205, 0, 205),
    CellColor::rgb(0, 205, 205),
    CellColor::rgb(229, 229, 229),
    CellColor::rgb(127, 127, 127),
    CellColor::rgb(255, 0, 0),
    CellColor::rgb(0, 255, 0),
    CellColor::rgb(255, 255, 0),
    CellColor::rgb(92, 92, 255),
    CellColor::rgb(255, 0, 255),
    CellColor::rgb(0, 255, 255),
    CellColor::rgb(255, 255, 255),
];

impl Palette {
    /// The palette every terminal used before this task, byte-identical:
    /// black background, the `#e5e5e5` foreground `grid()` used to hardcode,
    /// and `XTERM_ANSI` — the same 16 colors `sgr.rs`'s run-console sink
    /// keeps using regardless of what a live terminal's palette is set to.
    pub const fn xterm() -> Self {
        Self {
            foreground: CellColor::rgb(229, 229, 229),
            background: CellColor::rgb(0, 0, 0),
            cursor: CellColor::rgb(229, 229, 229),
            selection: CellColor::rgb(38, 79, 150),
            ansi: XTERM_ANSI,
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::xterm()
    }
}

/// Basic per-cell rendering attributes exposed cheaply by `alacritty_terminal`'s
/// [`CellFlags`]. Deliberately not exhaustive (no undercurl/strikeout/dim
/// variants) — bold/italic/underline is what a first-slice grid widget needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAttributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// One renderable cell: character plus resolved colors and attributes.
///
/// `selected` rides along with the cell rather than being exposed as a
/// separate range accessor: the view's paint loop already walks every cell
/// and already swaps fg/bg for `inverse`, so a per-cell flag costs it one
/// XOR instead of a second lookup structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderCell {
    pub character: char,
    pub fg: CellColor,
    pub bg: CellColor,
    pub attrs: CellAttributes,
    pub selected: bool,
    /// The leading half of a double-width glyph (`Flags::WIDE_CHAR`), e.g.
    /// CJK text and most emoji. The cell immediately after it is that
    /// glyph's spacer half (`Flags::WIDE_CHAR_SPACER`) — still present in
    /// `Grid::rows` so column arithmetic stays simple, but a paint routine
    /// must skip drawing it: it is blank filler, already covered by this
    /// cell's own (double-width) glyph.
    pub wide: bool,
}

/// The cursor's position in the visible grid, zero-indexed from the
/// top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPosition {
    pub row: usize,
    pub col: usize,
}

/// A snapshot of the terminal's visible viewport: rows of cells plus the
/// cursor position, ready to hand to a paint routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    pub rows: Vec<Vec<RenderCell>>,
    pub cursor: CursorPosition,
    /// Whether the cursor should actually be painted (T5). The live cursor
    /// only makes sense on the bottom (0-offset) viewport — while scrolled
    /// up into history, `cursor`'s row/col still point at where the cursor
    /// *would* land the moment the view returns to live output, and drawing
    /// a block there would paint it over unrelated history text.
    pub cursor_visible: bool,
}

/// Scrollback position: how far back the buffer goes, and how far the
/// viewport is currently scrolled into it (Task T5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollState {
    /// Lines of history above the live screen.
    pub history: usize,
    /// Lines the viewport is scrolled up from the bottom; 0 is live.
    pub offset: usize,
}

/// A `http(s)` URL found on one grid row. `start_col..end_col` is a
/// half-open range of cells on `row`, ready for the view to hit-test a
/// mouse position against and to underline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpan {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub url: String,
}

/// What a mouse gesture selects: a free drag, the word under the pointer
/// (double click), or the whole line (triple click). Maps 1:1 onto the
/// `alacritty_terminal` selection modes this crate delegates to; `Block`
/// (alt-drag) is deliberately absent until there's a gesture for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Simple,
    Word,
    Line,
}

impl From<SelectionKind> for SelectionType {
    fn from(kind: SelectionKind) -> Self {
        match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Word => SelectionType::Semantic,
            SelectionKind::Line => SelectionType::Lines,
        }
    }
}

const URL_SCHEMES: [&str; 2] = ["https://", "http://"];

/// Length of the URL scheme starting at `col`, if one starts exactly there.
fn scheme_at(chars: &[char], col: usize) -> Option<usize> {
    URL_SCHEMES.iter().find_map(|scheme| {
        let len = scheme.chars().count();
        let matches = chars
            .get(col..col + len)?
            .iter()
            .zip(scheme.chars())
            .all(|(c, s)| c.eq_ignore_ascii_case(&s));
        matches.then_some(len)
    })
}

/// Characters a URL run keeps consuming. Whitespace and controls end it;
/// quotes and angle brackets are the conventional delimiters when a URL is
/// embedded in prose or markup, and are never part of the URL itself.
fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !matches!(c, '"' | '\'' | '<' | '>' | '`')
}

/// Walk back over trailing punctuation that reads as sentence punctuation
/// rather than part of the URL. A closing bracket only counts as trailing
/// when it is unbalanced within the match, so
/// `https://en.wikipedia.org/wiki/Foo_(bar)` keeps its paren while
/// `(see https://example.com)` does not.
fn trim_url_end(chars: &[char], body_start: usize, mut end: usize) -> usize {
    while end > body_start {
        let last = chars[end - 1];
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?') {
            end -= 1;
            continue;
        }
        let Some(open) = (match last {
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => None,
        }) else {
            break;
        };
        let span = &chars[body_start..end];
        let opens = span.iter().filter(|&&c| c == open).count();
        let closes = span.iter().filter(|&&c| c == last).count();
        if closes > opens {
            end -= 1;
            continue;
        }
        break;
    }
    end
}

/// Every `http(s)` URL in one row of text, as `(start_col, end_col, url)`
/// with `end_col` exclusive. Character index equals column because this
/// crate's grid model holds exactly one `char` per cell (the same
/// simplification `grid()` already makes for wide characters).
fn find_urls_in_line(line: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut col = 0;
    while col < chars.len() {
        let Some(scheme_len) = scheme_at(&chars, col) else {
            col += 1;
            continue;
        };
        // A scheme only counts at a word boundary, so `shttp://x` is not a
        // link and `xhttps://` cannot smuggle one in.
        if col > 0 && chars[col - 1].is_alphanumeric() {
            col += 1;
            continue;
        }
        let body_start = col + scheme_len;
        let mut end = body_start;
        while end < chars.len() && is_url_char(chars[end]) {
            end += 1;
        }
        end = trim_url_end(&chars, body_start, end);
        if end > body_start {
            found.push((col, end, chars[col..end].iter().collect()));
            col = end;
        } else {
            // A bare scheme with nothing after it is not a URL.
            col = body_start;
        }
    }
    found
}

/// Make clipboard text safe to hand to a shell.
///
/// Pasted text is attacker-controlled in the everyday "copied off a web
/// page" sense, so this is a trust boundary, not a cosmetic tidy-up:
/// newlines are normalized to CR (what a terminal sends for Enter), tabs
/// survive, and every other control character — `ESC` above all — is
/// dropped. Dropping `ESC` is what stops a pasted escape sequence from
/// being executed and what makes a smuggled `\x1b[201~` unable to close
/// the bracketed-paste wrapper early.
pub fn sanitize_paste(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\r');
            }
            '\n' => out.push('\r'),
            '\t' => out.push('\t'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// No-op event sink: `Term` reports OSC/bell/clipboard-style side effects
/// through this, none of which this first slice acts on (title bar, bell,
/// clipboard integration are `ui-shell` concerns for a later task).
struct NullEventListener;

impl EventListener for NullEventListener {
    fn send_event(&self, _event: Event) {}
}

/// Owns the VT100/grid state for one terminal session. Feed it raw bytes
/// read from a `pty_core::PtySession`; read back a [`Grid`] snapshot to
/// paint.
pub struct TerminalEmulator {
    term: Term<NullEventListener>,
    parser: Processor,
    palette: Palette,
}

impl TerminalEmulator {
    pub fn new(size: GridSize, palette: Palette) -> Self {
        let term = Term::new(TermConfig::default(), &size, NullEventListener);
        Self {
            term,
            parser: Processor::new(),
            palette,
        }
    }

    /// Replace this session's palette (T3) — live, so a theme switch is
    /// visible in an already-open terminal without restarting its shell.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Interpret raw bytes (text and/or escape sequences) read from the PTY,
    /// updating grid/cursor state in place.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// The text of one visible row, trailing blanks trimmed.
    ///
    /// The grid is the terminal's answer to "what is on screen"; this is
    /// the same answer as a line of text, for a caller that recognises
    /// spans in text rather than cells — `run_core::links` does, and
    /// `ui-shell` feeds this to it so a `file:line` in a terminal is the
    /// same link it is in a run console (R2-6).
    pub fn row_text(&self, row: usize) -> Option<String> {
        let grid = self.grid();
        let cells = grid.rows.get(row)?;
        let text: String = cells.iter().map(|cell| cell.character).collect();
        Some(text.trim_end().to_string())
    }

    /// Resize the grid to new dimensions, preserving content as
    /// `alacritty_terminal` reflows it (matches real terminal resize
    /// behavior: content shifts, it never panics).
    pub fn resize(&mut self, size: GridSize) {
        // A resize reflows content, so any anchor points the selection held
        // now refer to different text. `Selection::rotate` only fixes up
        // scroll-induced shifts, not reflow, so the honest answer is to drop
        // the selection rather than keep a stale-looking one.
        self.term.selection = None;
        self.term.resize(size);
    }

    /// Snapshot the current visible viewport as renderable cells plus
    /// cursor position.
    pub fn grid(&self) -> Grid {
        let term_grid = self.term.grid();
        let cols = term_grid.columns();

        let mut rows: Vec<Vec<RenderCell>> = (0..term_grid.screen_lines())
            .map(|_| Vec::with_capacity(cols))
            .collect();

        let selection = self.selection_range();

        for indexed in term_grid.display_iter() {
            let row_idx = (indexed.point.line.0 + term_grid.display_offset() as i32) as usize;
            let Some(row) = rows.get_mut(row_idx) else {
                continue;
            };
            let cell = indexed.cell;
            let fg = CellColor::from_ansi(cell.fg, self.palette.foreground, &self.palette.ansi);
            let bg = CellColor::from_ansi(cell.bg, self.palette.background, &self.palette.ansi);
            row.push(RenderCell {
                character: cell.c,
                fg,
                bg,
                attrs: CellAttributes {
                    bold: cell.flags.intersects(CellFlags::BOLD),
                    italic: cell.flags.contains(CellFlags::ITALIC),
                    underline: cell.flags.intersects(CellFlags::ALL_UNDERLINES),
                    inverse: cell.flags.contains(CellFlags::INVERSE),
                },
                selected: selection.is_some_and(|range| range.contains(indexed.point)),
                wide: cell.flags.contains(CellFlags::WIDE_CHAR),
            });
        }

        let cursor_point = term_grid.cursor.point;
        Grid {
            rows,
            cursor: CursorPosition {
                row: (cursor_point.line.0 + term_grid.display_offset() as i32).max(0) as usize,
                col: cursor_point.column.0,
            },
            cursor_visible: term_grid.display_offset() == 0,
        }
    }

    /// Scroll the viewport by `delta` lines: positive moves up into history,
    /// negative moves back down toward live output. The raw wheel/keyboard
    /// gesture — `alacritty_terminal` clamps the result to `0..=history()`
    /// itself, so a caller need not.
    pub fn scroll(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    /// Scroll the viewport to an absolute offset from the bottom (0 = live).
    /// `alacritty_terminal` 0.26's `Scroll` enum has no absolute-position
    /// variant (`Delta`/`PageUp`/`PageDown`/`Top`/`Bottom` only), so this
    /// computes the `Delta` that gets from the current offset to `offset`.
    pub fn scroll_to(&mut self, offset: usize) {
        let current = self.term.grid().display_offset() as i64;
        let delta = offset as i64 - current;
        if delta != 0 {
            self.term.scroll_display(Scroll::Delta(delta as i32));
        }
    }

    /// Snap the viewport back to live output — typing does this (see
    /// `bridge/terminal.rs`'s `send_key`), matching every other terminal.
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// How far back the buffer goes, and how far the viewport is currently
    /// scrolled into it.
    pub fn scroll_state(&self) -> ScrollState {
        let grid = self.term.grid();
        ScrollState {
            history: grid.history_size(),
            offset: grid.display_offset(),
        }
    }

    /// Whether the running application is on the alternate screen
    /// (`\x1b[?1049h`, what `vim`/`less`/full-screen TUIs switch to) — the
    /// view reads this to decide whether the mouse wheel should scroll
    /// history or send arrow keys to the app instead (T5).
    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Whether the running application asked for bracketed paste
    /// (`\x1b[?2004h`), i.e. whether it wants pasted text framed so it can
    /// tell it apart from typing.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Whether the running application asked for application-cursor-key
    /// mode (`\x1b[?1h`), i.e. whether arrow/Home/End keys should encode as
    /// `SS3` sequences instead of `CSI` ones (`keys::encode`).
    pub fn app_cursor_mode(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    /// The exact bytes a paste of `text` should write to the PTY:
    /// [`sanitize_paste`]'d, and wrapped in the bracketed-paste markers only
    /// when the application enabled them — sending the markers otherwise
    /// would land them in the shell's input as literal text.
    pub fn paste_payload(&self, text: &str) -> String {
        let body = sanitize_paste(text);
        if self.bracketed_paste() {
            format!("\x1b[200~{body}\x1b[201~")
        } else {
            body
        }
    }

    /// Clamp a viewport `(row, col)` from the view to a grid [`Point`].
    ///
    /// Viewport row `r` maps to `Line(r - display_offset)` (T5): row 0 is
    /// always the top of whatever is currently visible, which is the live
    /// screen only while `display_offset == 0` — scrolled up into history,
    /// the same viewport row names an earlier, more negative `Line`. Matches
    /// `grid()`'s own `row_idx = point.line + display_offset` mapping, just
    /// solved for `line` instead of `row_idx`.
    fn point_at(&self, row: usize, col: usize) -> Point {
        let grid = self.term.grid();
        let display_offset = grid.display_offset() as i32;
        let rows = grid.screen_lines();
        let cols = grid.columns();
        Point::new(
            Line(row.min(rows.saturating_sub(1)) as i32 - display_offset),
            Column(col.min(cols.saturating_sub(1))),
        )
    }

    /// Begin a selection at a cell. `right_half` says the click landed on the
    /// right half of that cell, which decides whether the cell itself is
    /// included; the view computes it from pixel arithmetic.
    pub fn selection_start(
        &mut self,
        row: usize,
        col: usize,
        right_half: bool,
        kind: SelectionKind,
    ) {
        let point = self.point_at(row, col);
        self.term.selection = Some(Selection::new(kind.into(), point, side(right_half)));
    }

    /// Extend the in-progress selection to a cell (drag). A no-op when
    /// nothing was started, so a stray drag can't invent a selection.
    pub fn selection_update(&mut self, row: usize, col: usize, right_half: bool) {
        let point = self.point_at(row, col);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side(right_half));
        }
    }

    pub fn selection_clear(&mut self) {
        self.term.selection = None;
    }

    /// Whether a selection covers at least one cell — an anchored but empty
    /// selection (press without drag) does not count.
    pub fn has_selection(&self) -> bool {
        self.selection_range().is_some()
    }

    /// The selected text, with `alacritty_terminal`'s own trailing-whitespace
    /// and line-joining behavior. Deliberately not re-implemented here.
    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    fn selection_range(&self) -> Option<SelectionRange> {
        self.term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&self.term))
    }

    /// The `http(s)` link covering this cell, if any. Only the one row is
    /// read, so a hover costs a row scan rather than a grid snapshot.
    pub fn link_at(&self, row: usize, col: usize) -> Option<LinkSpan> {
        let grid = self.term.grid();
        if row >= grid.screen_lines() || col >= grid.columns() {
            return None;
        }
        let line = Line(row as i32 - grid.display_offset() as i32);
        let text: String = (0..grid.columns())
            .map(|c| grid[Point::new(line, Column(c))].c)
            .collect();
        find_urls_in_line(&text)
            .into_iter()
            .find(|(start, end, _)| col >= *start && col < *end)
            .map(|(start_col, end_col, url)| LinkSpan {
                row,
                start_col,
                end_col,
                url,
            })
    }
}

/// Which half of a cell a click landed on, in `alacritty_terminal`'s terms.
fn side(right_half: bool) -> Side {
    if right_half {
        Side::Right
    } else {
        Side::Left
    }
}

/// Move the cursor to an absolute `(line, column)` position, both
/// zero-indexed — a thin wrapper over [`Point`]/[`Line`]/[`Column`] so
/// callers/tests don't need to import `alacritty_terminal`'s index types
/// directly. Currently only used by this crate's own tests.
#[cfg(test)]
fn point(line: i32, column: usize) -> Point {
    Point::new(Line(line), Column(column))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_appears_at_expected_cell_positions() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        emulator.feed(b"hi");

        let grid = emulator.grid();
        assert_eq!(grid.rows[0][0].character, 'h');
        assert_eq!(grid.rows[0][1].character, 'i');
        assert_eq!(grid.rows[0][2].character, ' ');
    }

    #[test]
    fn cup_escape_sequence_moves_cursor() {
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20), Palette::xterm());
        // CUP: move cursor to row 3, column 5 (1-indexed in the escape
        // sequence itself).
        emulator.feed(b"\x1b[3;5H");

        let grid = emulator.grid();
        assert_eq!(grid.cursor, CursorPosition { row: 2, col: 4 });
    }

    #[test]
    fn home_escape_sequence_moves_cursor_to_origin() {
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20), Palette::xterm());
        emulator.feed(b"hello\x1b[H");

        let grid = emulator.grid();
        assert_eq!(grid.cursor, CursorPosition { row: 0, col: 0 });
    }

    #[test]
    fn sgr_red_sets_cell_foreground() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        emulator.feed(b"\x1b[31mR\x1b[0m");

        let grid = emulator.grid();
        let cell = grid.rows[0][0];
        assert_eq!(cell.character, 'R');
        assert_eq!(cell.fg, indexed_color(1, &Palette::xterm().ansi).unwrap());
    }

    #[test]
    fn line_feed_advances_cursor_row() {
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20), Palette::xterm());
        emulator.feed(b"a\r\nb");

        let grid = emulator.grid();
        assert_eq!(grid.cursor.row, 1);
        assert_eq!(grid.rows[0][0].character, 'a');
        assert_eq!(grid.rows[1][0].character, 'b');
    }

    #[test]
    fn row_text_is_the_row_without_its_trailing_blanks() {
        let mut emulator = TerminalEmulator::new(GridSize::new(3, 40), Palette::xterm());
        emulator.feed(b"src/main.rs:42:5: error\r\n");
        assert_eq!(
            emulator.row_text(0).as_deref(),
            Some("src/main.rs:42:5: error")
        );
        assert_eq!(emulator.row_text(1).as_deref(), Some(""));
        assert_eq!(emulator.row_text(99), None);
    }

    #[test]
    fn resize_does_not_panic_and_writes_still_render() {
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20), Palette::xterm());
        emulator.feed(b"before");

        emulator.resize(GridSize::new(15, 30));
        emulator.feed(b"after");

        let grid = emulator.grid();
        assert_eq!(grid.rows.len(), 15);
        assert_eq!(grid.rows[0].len(), 30);
        assert!(grid.rows.iter().flatten().any(|c| c.character == 'a'));
    }

    #[test]
    fn bold_flag_is_reflected_in_attributes() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        emulator.feed(b"\x1b[1mB\x1b[0m");

        let grid = emulator.grid();
        assert!(grid.rows[0][0].attrs.bold);
    }

    #[test]
    fn default_cell_background_resolves_to_the_caller_supplied_default() {
        let ansi = Palette::xterm().ansi;
        let default_bg = CellColor::rgb(30, 31, 34);
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::Background), default_bg, &ansi),
            default_bg
        );
        let default_fg = CellColor::rgb(169, 183, 198);
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::Foreground), default_fg, &ansi),
            default_fg
        );
    }

    #[test]
    fn named_palette_colors_still_resolve_to_palette_entries() {
        let ansi = Palette::xterm().ansi;
        let default = CellColor::rgb(30, 31, 34);
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::Red), default, &ansi),
            indexed_color(1, &ansi).unwrap()
        );
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::BrightWhite), default, &ansi),
            indexed_color(15, &ansi).unwrap()
        );
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::DimRed), default, &ansi),
            indexed_color(1, &ansi).unwrap()
        );
    }

    #[test]
    fn a_custom_palette_changes_which_color_ansi_red_resolves_to() {
        let mut custom = Palette::xterm();
        custom.ansi[1] = CellColor::rgb(255, 105, 97);
        let default = CellColor::rgb(0, 0, 0);
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::Red), default, &custom.ansi),
            CellColor::rgb(255, 105, 97)
        );
    }

    #[test]
    fn palette_xterm_matches_the_historical_hardcoded_values() {
        let palette = Palette::xterm();
        assert_eq!(palette.background, CellColor::rgb(0, 0, 0));
        assert_eq!(palette.foreground, CellColor::rgb(229, 229, 229));
        assert_eq!(
            palette.ansi,
            [
                CellColor::rgb(0, 0, 0),
                CellColor::rgb(205, 0, 0),
                CellColor::rgb(0, 205, 0),
                CellColor::rgb(205, 205, 0),
                CellColor::rgb(0, 0, 238),
                CellColor::rgb(205, 0, 205),
                CellColor::rgb(0, 205, 205),
                CellColor::rgb(229, 229, 229),
                CellColor::rgb(127, 127, 127),
                CellColor::rgb(255, 0, 0),
                CellColor::rgb(0, 255, 0),
                CellColor::rgb(255, 255, 0),
                CellColor::rgb(92, 92, 255),
                CellColor::rgb(255, 0, 255),
                CellColor::rgb(0, 255, 255),
                CellColor::rgb(255, 255, 255),
            ]
        );
    }

    #[test]
    fn default_derives_to_xterm() {
        assert_eq!(Palette::default(), Palette::xterm());
    }

    // --- 256-color cube / greyscale ramp -----------------------------------

    #[test]
    fn cube_index_16_is_black_and_231_is_near_white() {
        let ansi = Palette::xterm().ansi;
        assert_eq!(indexed_color(16, &ansi), Some(CellColor::rgb(0, 0, 0)));
        assert_eq!(
            indexed_color(231, &ansi),
            Some(CellColor::rgb(255, 255, 255))
        );
    }

    #[test]
    fn a_mid_cube_index_resolves_to_its_documented_rgb_levels() {
        // 16 + 36*2 + 6*3 + 4 = 110.
        let ansi = Palette::xterm().ansi;
        assert_eq!(
            indexed_color(110, &ansi),
            Some(CellColor::rgb(135, 175, 215))
        );
    }

    #[test]
    fn grey_ramp_spans_232_to_255() {
        let ansi = Palette::xterm().ansi;
        assert_eq!(indexed_color(232, &ansi), Some(CellColor::rgb(8, 8, 8)));
        assert_eq!(
            indexed_color(255, &ansi),
            Some(CellColor::rgb(238, 238, 238))
        );
    }

    #[test]
    fn indexed_0_to_15_follows_the_supplied_ansi_table_not_the_fixed_one() {
        let mut custom = Palette::xterm();
        custom.ansi[4] = CellColor::rgb(1, 2, 3);
        assert_eq!(
            indexed_color(4, &custom.ansi),
            Some(CellColor::rgb(1, 2, 3))
        );
    }

    #[test]
    fn untouched_cells_paint_with_the_default_background() {
        let emulator = TerminalEmulator::new(GridSize::new(3, 10), Palette::xterm());

        let grid = emulator.grid();
        assert_eq!(grid.rows[0][0].bg, CellColor::rgb(0, 0, 0));
    }

    #[test]
    fn point_helper_builds_expected_coordinates() {
        let p = point(2, 4);
        assert_eq!(p.line, Line(2));
        assert_eq!(p.column, Column(4));
    }
    // --- URL scanning -----------------------------------------------------

    #[test]
    fn finds_http_and_https_urls_with_their_columns() {
        assert_eq!(
            find_urls_in_line("see http://example.com now"),
            vec![(4, 22, "http://example.com".to_string())]
        );
        assert_eq!(
            find_urls_in_line("https://example.com"),
            vec![(0, 19, "https://example.com".to_string())]
        );
    }

    #[test]
    fn trailing_sentence_punctuation_is_not_part_of_the_url() {
        assert_eq!(
            find_urls_in_line("go to https://example.com."),
            vec![(6, 25, "https://example.com".to_string())]
        );
        assert_eq!(
            find_urls_in_line("https://example.com, then"),
            vec![(0, 19, "https://example.com".to_string())]
        );
    }

    #[test]
    fn closing_bracket_is_kept_when_balanced_and_dropped_when_not() {
        assert_eq!(
            find_urls_in_line("https://en.wikipedia.org/wiki/Foo_(bar)"),
            vec![(0, 39, "https://en.wikipedia.org/wiki/Foo_(bar)".to_string())]
        );
        assert_eq!(
            find_urls_in_line("(see https://example.com)"),
            vec![(5, 24, "https://example.com".to_string())]
        );
    }

    #[test]
    fn a_scheme_inside_a_word_is_not_a_url() {
        assert!(find_urls_in_line("shttp://example.com").is_empty());
    }

    #[test]
    fn a_bare_scheme_with_no_host_is_not_a_url() {
        assert!(find_urls_in_line("http:// and https://").is_empty());
    }

    #[test]
    fn two_urls_on_one_row_are_found_separately() {
        assert_eq!(
            find_urls_in_line("http://a.example https://b.example"),
            vec![
                (0, 16, "http://a.example".to_string()),
                (17, 34, "https://b.example".to_string()),
            ]
        );
    }

    #[test]
    fn a_url_ending_at_the_last_column_is_still_found() {
        assert_eq!(
            find_urls_in_line("x https://example.com"),
            vec![(2, 21, "https://example.com".to_string())]
        );
    }

    #[test]
    fn quotes_and_angle_brackets_delimit_a_url() {
        assert_eq!(
            find_urls_in_line("<https://example.com>"),
            vec![(1, 20, "https://example.com".to_string())]
        );
    }

    // --- link_at ----------------------------------------------------------

    fn emulator_showing(text: &str) -> TerminalEmulator {
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 40), Palette::xterm());
        emulator.feed(text.as_bytes());
        emulator
    }

    #[test]
    fn link_at_hits_every_cell_of_the_span_and_nothing_past_it() {
        // "see https://example.com" — columns 4..23.
        let emulator = emulator_showing("see https://example.com");

        let expected = LinkSpan {
            row: 0,
            start_col: 4,
            end_col: 23,
            url: "https://example.com".to_string(),
        };
        assert_eq!(emulator.link_at(0, 4), Some(expected.clone()));
        assert_eq!(emulator.link_at(0, 22), Some(expected));
        assert_eq!(emulator.link_at(0, 3), None);
        assert_eq!(emulator.link_at(0, 23), None);
    }

    #[test]
    fn link_at_returns_none_off_the_grid_or_on_a_blank_row() {
        let emulator = emulator_showing("https://example.com");

        assert_eq!(emulator.link_at(1, 0), None);
        assert_eq!(emulator.link_at(99, 0), None);
        assert_eq!(emulator.link_at(0, 99), None);
    }

    // --- paste ------------------------------------------------------------

    #[test]
    fn paste_normalizes_newlines_to_carriage_returns() {
        assert_eq!(sanitize_paste("a\r\nb\nc\rd"), "a\rb\rc\rd");
    }

    #[test]
    fn paste_keeps_tabs_and_drops_other_control_characters() {
        assert_eq!(sanitize_paste("a\tb\x00c\x1bd\x7fe"), "a\tbcde");
    }

    #[test]
    fn bracketed_paste_follows_the_applications_request() {
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 20), Palette::xterm());
        assert!(!emulator.bracketed_paste());

        emulator.feed(b"\x1b[?2004h");
        assert!(emulator.bracketed_paste());

        emulator.feed(b"\x1b[?2004l");
        assert!(!emulator.bracketed_paste());
    }

    #[test]
    fn app_cursor_mode_follows_the_applications_request() {
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 20), Palette::xterm());
        assert!(!emulator.app_cursor_mode());

        emulator.feed(b"\x1b[?1h");
        assert!(emulator.app_cursor_mode());

        emulator.feed(b"\x1b[?1l");
        assert!(!emulator.app_cursor_mode());
    }

    #[test]
    fn paste_payload_is_wrapped_only_in_bracketed_paste_mode() {
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 20), Palette::xterm());
        assert_eq!(emulator.paste_payload("ls"), "ls");

        emulator.feed(b"\x1b[?2004h");
        assert_eq!(emulator.paste_payload("ls"), "\x1b[200~ls\x1b[201~");
    }

    #[test]
    fn a_smuggled_end_marker_cannot_close_the_paste_wrapper_early() {
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 20), Palette::xterm());
        emulator.feed(b"\x1b[?2004h");

        let payload = emulator.paste_payload("ls\x1b[201~rm -rf /");

        assert_eq!(payload, "\x1b[200~ls[201~rm -rf /\x1b[201~");
        assert_eq!(payload.matches("\x1b[201~").count(), 1);
    }

    // --- selection --------------------------------------------------------

    fn selected_text_of(grid: &Grid) -> String {
        grid.rows
            .iter()
            .flat_map(|row| row.iter())
            .filter(|cell| cell.selected)
            .map(|cell| cell.character)
            .collect()
    }

    #[test]
    fn dragging_marks_the_dragged_cells_selected_and_yields_their_text() {
        let mut emulator = emulator_showing("hello world");

        emulator.selection_start(0, 0, false, SelectionKind::Simple);
        emulator.selection_update(0, 4, true);

        assert!(emulator.has_selection());
        assert_eq!(selected_text_of(&emulator.grid()), "hello");
        assert_eq!(emulator.selection_text().as_deref(), Some("hello"));
    }

    #[test]
    fn a_backwards_drag_selects_the_same_span() {
        let mut emulator = emulator_showing("hello world");

        emulator.selection_start(0, 4, true, SelectionKind::Simple);
        emulator.selection_update(0, 0, false);

        assert_eq!(emulator.selection_text().as_deref(), Some("hello"));
    }

    #[test]
    fn a_word_selection_expands_to_the_whole_word() {
        let mut emulator = emulator_showing("hello world");

        emulator.selection_start(0, 8, false, SelectionKind::Word);

        assert_eq!(emulator.selection_text().as_deref(), Some("world"));
    }

    #[test]
    fn a_line_selection_takes_the_whole_row() {
        let mut emulator = emulator_showing("hello world");

        emulator.selection_start(0, 3, false, SelectionKind::Line);

        // A line selection carries its own newline, as alacritty produces it.
        assert_eq!(emulator.selection_text().as_deref(), Some("hello world\n"));
    }

    #[test]
    fn a_press_without_a_drag_is_not_a_selection() {
        let mut emulator = emulator_showing("hello world");

        // Press and release on the same cell half: the view passes the same
        // `right_half` both times, so this is the real no-drag case.
        emulator.selection_start(0, 2, true, SelectionKind::Simple);
        emulator.selection_update(0, 2, true);

        assert!(!emulator.has_selection());
    }

    #[test]
    fn clearing_drops_both_the_text_and_the_cell_flags() {
        let mut emulator = emulator_showing("hello world");
        emulator.selection_start(0, 0, false, SelectionKind::Simple);
        emulator.selection_update(0, 4, true);

        emulator.selection_clear();

        assert!(!emulator.has_selection());
        assert_eq!(emulator.selection_text(), None);
        assert_eq!(selected_text_of(&emulator.grid()), "");
    }

    #[test]
    fn selection_coordinates_off_the_grid_are_clamped_not_panicked_on() {
        let mut emulator = emulator_showing("hello world");

        // Both ends clamp to the grid, so this is a full-screen drag rather
        // than a panic or an out-of-bounds `Point`.
        emulator.selection_start(999, 999, true, SelectionKind::Simple);
        emulator.selection_update(0, 0, false);

        assert!(emulator.has_selection());
        assert!(emulator
            .selection_text()
            .unwrap()
            .starts_with("hello world"));
    }

    #[test]
    fn resizing_drops_the_selection() {
        let mut emulator = emulator_showing("hello world");
        emulator.selection_start(0, 0, false, SelectionKind::Simple);
        emulator.selection_update(0, 4, true);

        emulator.resize(GridSize::new(6, 30));

        assert!(!emulator.has_selection());
    }

    // --- scrollback (T5) ---------------------------------------------------

    /// Feed `count` numbered lines (`line000`, `line001`, ...) starting at
    /// `start`, each terminated with a real CRLF so every one lands on its
    /// own row and rolls older rows into scrollback once the 5-row grid
    /// tests below fill up.
    fn feed_numbered_lines(emulator: &mut TerminalEmulator, start: u32, count: u32) {
        for i in start..start + count {
            emulator.feed(format!("line{i:03}\r\n").as_bytes());
        }
    }

    /// The numeric suffix of the top visible row's `lineNNN` text — parsed
    /// rather than hardcoded, so these tests assert *relative* movement
    /// (scrolling by 2 moves the top row back by 2) instead of depending on
    /// exactly how alacritty accounts for the trailing empty line after the
    /// last `\r\n`.
    fn top_row_line_number(emulator: &TerminalEmulator) -> u32 {
        let text: String = emulator.grid().rows[0]
            .iter()
            .map(|c| c.character)
            .collect();
        text.trim()
            .trim_start_matches("line")
            .parse()
            .unwrap_or_else(|_| panic!("top row {text:?} is not a numbered line"))
    }

    #[test]
    fn scrolling_up_reveals_earlier_lines() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        feed_numbered_lines(&mut emulator, 0, 100);
        let live_top = top_row_line_number(&emulator);

        emulator.scroll(2);

        assert_eq!(emulator.scroll_state().offset, 2);
        assert_eq!(top_row_line_number(&emulator), live_top - 2);
    }

    #[test]
    fn output_arriving_while_scrolled_does_not_move_the_viewport() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        feed_numbered_lines(&mut emulator, 0, 100);

        emulator.scroll(3);
        assert_eq!(emulator.scroll_state().offset, 3);
        let scrolled_top = top_row_line_number(&emulator);

        // New output must not yank a scrolled-up view back to live, or a
        // user reading history would be interrupted by every line a
        // background process prints. `alacritty_terminal` keeps the exact
        // same lines on screen by growing `offset` along with `history` as
        // new rows are appended at the bottom — the number that must not
        // change is which line is on top, not the raw `offset` count.
        feed_numbered_lines(&mut emulator, 100, 5);

        assert_ne!(
            emulator.scroll_state().offset,
            0,
            "must not snap back to live output"
        );
        assert_eq!(top_row_line_number(&emulator), scrolled_top);
    }

    #[test]
    fn scroll_to_bottom_restores_a_zero_offset() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        feed_numbered_lines(&mut emulator, 0, 100);
        emulator.scroll(10);
        assert_ne!(emulator.scroll_state().offset, 0);

        emulator.scroll_to_bottom();

        assert_eq!(emulator.scroll_state().offset, 0);
    }

    #[test]
    fn scroll_to_sets_an_absolute_offset_in_either_direction() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        feed_numbered_lines(&mut emulator, 0, 100);

        emulator.scroll_to(20);
        assert_eq!(emulator.scroll_state().offset, 20);

        // Moving to a smaller offset exercises the negative-delta branch.
        emulator.scroll_to(5);
        assert_eq!(emulator.scroll_state().offset, 5);
    }

    #[test]
    fn cursor_is_reported_hidden_while_scrolled_and_visible_when_live() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        feed_numbered_lines(&mut emulator, 0, 100);
        assert!(emulator.grid().cursor_visible);

        emulator.scroll(1);
        assert!(!emulator.grid().cursor_visible);

        emulator.scroll_to_bottom();
        assert!(emulator.grid().cursor_visible);
    }

    #[test]
    fn selection_while_scrolled_selects_the_historical_text_under_it() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20), Palette::xterm());
        feed_numbered_lines(&mut emulator, 0, 100);
        let live_top = top_row_line_number(&emulator);

        emulator.scroll(2);
        assert_eq!(top_row_line_number(&emulator), live_top - 2);

        // Select the whole top (now-historical) row.
        emulator.selection_start(0, 0, false, SelectionKind::Line);

        let expected = format!("line{:03}", live_top - 2);
        assert_eq!(
            emulator.selection_text().as_deref(),
            Some(format!("{expected}\n").as_str())
        );
    }

    #[test]
    fn link_at_while_scrolled_finds_a_link_in_history_not_the_live_screen() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 40), Palette::xterm());
        // A line with a URL, then enough plain lines to push it well into
        // scrollback.
        emulator.feed(b"see https://example.com/history\r\n");
        feed_numbered_lines(&mut emulator, 0, 20);

        // The live (offset 0) screen must not show the link.
        assert_eq!(emulator.link_at(0, 6), None);

        // Scroll until the URL's line is the top row again.
        for _ in 0..40 {
            if emulator
                .grid()
                .rows
                .first()
                .map(|row| row.iter().map(|c| c.character).collect::<String>())
                .is_some_and(|text| text.contains("https://"))
            {
                break;
            }
            emulator.scroll(1);
        }

        let link = emulator
            .link_at(0, 6)
            .expect("the URL should be on the scrolled-to row");
        assert_eq!(link.url, "https://example.com/history");
    }
}
