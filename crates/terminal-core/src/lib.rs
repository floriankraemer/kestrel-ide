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
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Processor, Rgb};

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

    fn from_ansi(color: AnsiColor, default: CellColor) -> Self {
        Self::from_ansi_opt(color).unwrap_or(default)
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
    pub(crate) fn from_ansi_opt(color: AnsiColor) -> Option<Self> {
        match color {
            AnsiColor::Spec(Rgb { r, g, b }) => Some(CellColor::rgb(r, g, b)),
            AnsiColor::Named(named) => named_color_opt(named),
            // Indexed colors beyond the named 16 are the 256-color cube /
            // grayscale ramp; approximating those faithfully needs a full
            // palette table, which is over-engineering for a first slice.
            // Falls back to the caller's default (usually foreground/
            // background) until a real theme/palette lands.
            AnsiColor::Indexed(idx) => named_color_by_index(idx),
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
fn named_color_opt(named: NamedColor) -> Option<CellColor> {
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
    named_color_by_index(index)
}

/// The standard 16-color ANSI palette (indices 0-15), used both for
/// [`NamedColor`] variants and low `Indexed` colors. Values match the
/// conventional xterm default palette.
fn named_color_by_index(idx: u8) -> Option<CellColor> {
    const PALETTE: [CellColor; 16] = [
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
    PALETTE.get(idx as usize).copied()
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
}

impl TerminalEmulator {
    pub fn new(size: GridSize) -> Self {
        let term = Term::new(TermConfig::default(), &size, NullEventListener);
        Self {
            term,
            parser: Processor::new(),
        }
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
        let default = CellColor::rgb(0, 0, 0);

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
            let fg = CellColor::from_ansi(cell.fg, CellColor::rgb(229, 229, 229));
            let bg = CellColor::from_ansi(cell.bg, default);
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
        }
    }

    /// Whether the running application asked for bracketed paste
    /// (`\x1b[?2004h`), i.e. whether it wants pasted text framed so it can
    /// tell it apart from typing.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
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
    /// Viewport row `r` maps to `Line(r)` only because this crate has no
    /// scrollback (`GridSize::total_lines() == screen_lines()`), which pins
    /// `display_offset` at 0. The assert is the tripwire for the day
    /// scrollback lands: without it, every mapping here would silently point
    /// at the wrong line.
    fn point_at(&self, row: usize, col: usize) -> Point {
        debug_assert_eq!(
            self.term.grid().display_offset(),
            0,
            "viewport row -> Line mapping assumes no scrollback"
        );
        let rows = self.term.grid().screen_lines();
        let cols = self.term.grid().columns();
        Point::new(
            Line(row.min(rows.saturating_sub(1)) as i32),
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
        debug_assert_eq!(
            grid.display_offset(),
            0,
            "viewport row -> Line mapping assumes no scrollback"
        );
        let line = Line(row as i32);
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
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20));
        emulator.feed(b"hi");

        let grid = emulator.grid();
        assert_eq!(grid.rows[0][0].character, 'h');
        assert_eq!(grid.rows[0][1].character, 'i');
        assert_eq!(grid.rows[0][2].character, ' ');
    }

    #[test]
    fn cup_escape_sequence_moves_cursor() {
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20));
        // CUP: move cursor to row 3, column 5 (1-indexed in the escape
        // sequence itself).
        emulator.feed(b"\x1b[3;5H");

        let grid = emulator.grid();
        assert_eq!(grid.cursor, CursorPosition { row: 2, col: 4 });
    }

    #[test]
    fn home_escape_sequence_moves_cursor_to_origin() {
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20));
        emulator.feed(b"hello\x1b[H");

        let grid = emulator.grid();
        assert_eq!(grid.cursor, CursorPosition { row: 0, col: 0 });
    }

    #[test]
    fn sgr_red_sets_cell_foreground() {
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20));
        emulator.feed(b"\x1b[31mR\x1b[0m");

        let grid = emulator.grid();
        let cell = grid.rows[0][0];
        assert_eq!(cell.character, 'R');
        assert_eq!(cell.fg, named_color_by_index(1).unwrap());
    }

    #[test]
    fn line_feed_advances_cursor_row() {
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20));
        emulator.feed(b"a\r\nb");

        let grid = emulator.grid();
        assert_eq!(grid.cursor.row, 1);
        assert_eq!(grid.rows[0][0].character, 'a');
        assert_eq!(grid.rows[1][0].character, 'b');
    }

    #[test]
    fn row_text_is_the_row_without_its_trailing_blanks() {
        let mut emulator = TerminalEmulator::new(GridSize::new(3, 40));
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
        let mut emulator = TerminalEmulator::new(GridSize::new(10, 20));
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
        let mut emulator = TerminalEmulator::new(GridSize::new(5, 20));
        emulator.feed(b"\x1b[1mB\x1b[0m");

        let grid = emulator.grid();
        assert!(grid.rows[0][0].attrs.bold);
    }

    #[test]
    fn default_cell_background_resolves_to_the_caller_supplied_default() {
        let default_bg = CellColor::rgb(30, 31, 34);
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::Background), default_bg),
            default_bg
        );
        let default_fg = CellColor::rgb(169, 183, 198);
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::Foreground), default_fg),
            default_fg
        );
    }

    #[test]
    fn named_palette_colors_still_resolve_to_palette_entries() {
        let default = CellColor::rgb(30, 31, 34);
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::Red), default),
            named_color_by_index(1).unwrap()
        );
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::BrightWhite), default),
            named_color_by_index(15).unwrap()
        );
        assert_eq!(
            CellColor::from_ansi(AnsiColor::Named(NamedColor::DimRed), default),
            named_color_by_index(1).unwrap()
        );
    }

    #[test]
    fn untouched_cells_paint_with_the_default_background() {
        let emulator = TerminalEmulator::new(GridSize::new(3, 10));

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
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 40));
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
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 20));
        assert!(!emulator.bracketed_paste());

        emulator.feed(b"\x1b[?2004h");
        assert!(emulator.bracketed_paste());

        emulator.feed(b"\x1b[?2004l");
        assert!(!emulator.bracketed_paste());
    }

    #[test]
    fn paste_payload_is_wrapped_only_in_bracketed_paste_mode() {
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 20));
        assert_eq!(emulator.paste_payload("ls"), "ls");

        emulator.feed(b"\x1b[?2004h");
        assert_eq!(emulator.paste_payload("ls"), "\x1b[200~ls\x1b[201~");
    }

    #[test]
    fn a_smuggled_end_marker_cannot_close_the_paste_wrapper_early() {
        let mut emulator = TerminalEmulator::new(GridSize::new(4, 20));
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
}
