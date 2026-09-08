//! SGR resolution for *streamed* output that is not a grid.
//!
//! [`TerminalEmulator`](crate::TerminalEmulator) answers "what does the
//! screen look like now"; a run console asks a different question — "what
//! text arrived, and how is each span of it styled" — because it appends to
//! a scrollback document rather than painting a fixed viewport.
//!
//! Both questions are answered by the same VT parser
//! (`alacritty_terminal`'s `vte`) and the same palette
//! ([`CellColor`]): this module is a second *sink*, not a second parser,
//! which is what `docs/architecture/layering.md`'s "no second ANSI parser"
//! rule asks for. `run_core::AnsiStripper` is a thin wrapper over it that
//! throws the styling away, so a build's diagnostics and a run console's
//! colors come from one implementation (ADR-0032, R2-1/R2-2).

use alacritty_terminal::vte::ansi::{Attr, Color as AnsiColor, Handler, Processor};

use crate::{CellAttributes, CellColor};

/// How one span of streamed text should be drawn. `None` for a color means
/// "the view's default" — SGR 39/49 and `Reset` say exactly that, and a
/// console that substituted a concrete color here would stop following the
/// editor theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextStyle {
    pub fg: Option<CellColor>,
    pub bg: Option<CellColor>,
    pub attrs: CellAttributes,
}

impl TextStyle {
    /// Whether this span needs any formatting at all — a run of plain
    /// default-styled text is the common case, and the view can skip it.
    pub fn is_default(&self) -> bool {
        *self == TextStyle::default()
    }
}

/// A styled span of the text one [`SgrResolver::feed`] call produced.
/// `start`/`len` are **byte** offsets into that call's
/// [`StyledText::text`]; a view that indexes in UTF-16 converts at its own
/// edge (`ui-shell`'s `RunService` does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyledRun {
    pub start: usize,
    pub len: usize,
    pub style: TextStyle,
}

/// The visible text one chunk of raw output carried, plus the styling of
/// each span of it. Runs cover the text end to end, in order, and never
/// overlap; unstyled spans are included so the caller can walk runs alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledText {
    pub text: String,
    pub runs: Vec<StyledRun>,
}

/// Turns raw process output into visible text plus styled runs, keeping SGR
/// state across calls: a chunk boundary may land inside an escape sequence
/// (the concern `run_core::batching` already tests for), and a color set in
/// one chunk stays in force in the next, exactly as it does on a terminal.
#[derive(Default)]
pub struct SgrResolver {
    parser: Processor,
    sink: Sink,
}

impl SgrResolver {
    pub fn feed(&mut self, text: &str) -> StyledText {
        self.sink.begin();
        self.parser.advance(&mut self.sink, text.as_bytes());
        self.sink.take()
    }
}

/// The `Handler` half: everything a stream sink cares about is "a character
/// arrived" or "the style changed". Every other terminal action — cursor
/// motion, scrolling, screen clearing — keeps `vte`'s default no-op, which
/// is the honest answer for a document that only ever grows.
#[derive(Default)]
struct Sink {
    out: StyledText,
    style: TextStyle,
    /// Where the run currently being accumulated started, or `None` when no
    /// text has arrived since the last style change.
    run_start: Option<usize>,
}

impl Sink {
    fn begin(&mut self) {
        self.out = StyledText::default();
        self.run_start = None;
    }

    fn take(&mut self) -> StyledText {
        self.close_run();
        std::mem::take(&mut self.out)
    }

    fn close_run(&mut self) {
        if let Some(start) = self.run_start.take() {
            let len = self.out.text.len() - start;
            if len > 0 {
                self.out.runs.push(StyledRun {
                    start,
                    len,
                    style: self.style,
                });
            }
        }
    }

    fn push(&mut self, c: char) {
        if self.run_start.is_none() {
            self.run_start = Some(self.out.text.len());
        }
        self.out.text.push(c);
    }
}

impl Handler for Sink {
    fn input(&mut self, c: char) {
        self.push(c);
    }

    fn linefeed(&mut self) {
        self.push('\n');
    }

    fn carriage_return(&mut self) {
        // Kept rather than acted on: a console document has no cursor to
        // return, and `build_core::DiagnosticParser` splits on `\r` to
        // survive Cargo's progress redraws (ADR-0040).
        self.push('\r');
    }

    fn put_tab(&mut self, count: u16) {
        for _ in 0..count {
            self.push('\t');
        }
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.close_run();
        match attr {
            Attr::Reset => self.style = TextStyle::default(),
            Attr::Bold => self.style.attrs.bold = true,
            Attr::Italic => self.style.attrs.italic = true,
            Attr::Underline
            | Attr::DoubleUnderline
            | Attr::Undercurl
            | Attr::DottedUnderline
            | Attr::DashedUnderline => self.style.attrs.underline = true,
            Attr::Reverse => self.style.attrs.inverse = true,
            Attr::CancelBold | Attr::CancelBoldDim => self.style.attrs.bold = false,
            Attr::CancelItalic => self.style.attrs.italic = false,
            Attr::CancelUnderline => self.style.attrs.underline = false,
            Attr::CancelReverse => self.style.attrs.inverse = false,
            Attr::Foreground(color) => self.style.fg = resolve(color),
            Attr::Background(color) => self.style.bg = resolve(color),
            // Dim, blink, hidden, strikeout and underline colors are not
            // part of `CellAttributes` (see its doc comment): the grid does
            // not render them either, and inventing them here would give
            // the two sinks different answers about the same bytes.
            _ => {}
        }
    }
}

/// A run console has no live terminal's palette to follow — it always
/// resolves through the fixed xterm 16-color table (`Palette::xterm().ansi`,
/// this task's hard byte-identical requirement for streamed output).
fn resolve(color: AnsiColor) -> Option<CellColor> {
    CellColor::from_ansi_opt(color, &crate::Palette::xterm().ansi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(text: &str) -> StyledText {
        SgrResolver::default().feed(text)
    }

    #[test]
    fn plain_text_is_one_default_run() {
        let out = styled("hello\nworld");
        assert_eq!(out.text, "hello\nworld");
        assert_eq!(out.runs.len(), 1);
        assert!(out.runs[0].style.is_default());
    }

    #[test]
    fn a_color_sequence_splits_the_text_into_runs() {
        let out = styled("\x1b[31merror\x1b[0m: oops");
        assert_eq!(out.text, "error: oops");
        assert_eq!(out.runs.len(), 2);
        assert_eq!(out.runs[0].style.fg, Some(CellColor { r: 205, g: 0, b: 0 }));
        assert_eq!(out.runs[0].start, 0);
        assert_eq!(out.runs[0].len, "error".len());
        assert!(out.runs[1].style.is_default());
    }

    #[test]
    fn a_truecolor_sequence_is_resolved_exactly() {
        let out = styled("\x1b[38;2;10;20;30mx");
        assert_eq!(
            out.runs[0].style.fg,
            Some(CellColor {
                r: 10,
                g: 20,
                b: 30
            })
        );
    }

    #[test]
    fn bold_and_underline_ride_along_with_the_color() {
        let out = styled("\x1b[1;4;32mok");
        let style = out.runs[0].style;
        assert!(style.attrs.bold && style.attrs.underline);
        assert_eq!(style.fg, Some(CellColor { r: 0, g: 205, b: 0 }));
    }

    #[test]
    fn style_carries_across_a_chunk_boundary() {
        let mut resolver = SgrResolver::default();
        let first = resolver.feed("\x1b[31mred");
        let second = resolver.feed("still red");
        assert_eq!(first.runs[0].style.fg, second.runs[0].style.fg);
    }

    #[test]
    fn a_sequence_split_across_two_chunks_is_still_resolved() {
        let mut resolver = SgrResolver::default();
        assert_eq!(resolver.feed("before\x1b[3").text, "before");
        let second = resolver.feed("1mafter");
        assert_eq!(second.text, "after");
        assert_eq!(
            second.runs[0].style.fg,
            Some(CellColor { r: 205, g: 0, b: 0 })
        );
    }

    #[test]
    fn run_offsets_are_byte_offsets_into_the_chunk() {
        let out = styled("caf\u{e9}\x1b[31mred");
        assert_eq!(out.runs[1].start, "caf\u{e9}".len());
        assert_eq!(&out.text[out.runs[1].start..], "red");
    }

    #[test]
    fn default_colors_are_left_to_the_view() {
        let out = styled("\x1b[31m\x1b[39mplain");
        assert_eq!(out.runs[0].style.fg, None);
    }
}
