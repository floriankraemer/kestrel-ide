//! Keyboard-to-PTY-bytes translation: turns a logical key press into the
//! xterm escape sequence (or plain byte) a shell expects to read.
//!
//! Qt-free by design (see the crate's module doc and
//! `docs/architecture/layering.md`): `ui-shell` maps `Qt::Key` to [`Key`]
//! at the FFI seam (pure enum translation, a humble view concern) and hands
//! it here, so the actual xterm encoding rules — and their test coverage —
//! live in one place with no Qt in the loop.

/// A logical key press, independent of any GUI toolkit's key codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
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
    /// Function key number, 1-12.
    F(u8),
}

/// Modifier keys held during a press. `shift` only matters for the keys
/// where xterm actually encodes it (arrows/Home/End/PageUp/PageDown/Insert/
/// Delete/F-keys/Tab) — a plain `Char` already carries its shifted glyph via
/// the char itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    fn none(self) -> bool {
        !self.shift && !self.ctrl && !self.alt
    }

    /// xterm's modifier parameter for `CSI 1;{m}X` / `CSI n;{m}~` style
    /// sequences: `1 + shift(1) + alt(2) + ctrl(4)`.
    fn xterm_param(self) -> u8 {
        1 + (self.shift as u8) + 2 * (self.alt as u8) + 4 * (self.ctrl as u8)
    }
}

const ESC: &str = "\x1b";
const CSI: &str = "\x1b[";
const SS3: &str = "\x1bO";

/// Encode one key press into the exact bytes to write to the PTY, or `None`
/// when this key press produces nothing (there is no such input today, but
/// the `Option` keeps the door open without every caller needing a special
/// case).
///
/// `app_cursor` is [`crate::TerminalEmulator::app_cursor_mode`] — whether the
/// running application asked for application-cursor-key mode
/// (`CSI ? 1 h`), which swaps the arrow/Home/End encoding from `CSI` to
/// `SS3` while unmodified.
pub fn encode(key: Key, mods: Modifiers, app_cursor: bool) -> Option<String> {
    // Ctrl/Alt take priority over plain-char encoding regardless of key
    // order below, per the task spec.
    if let Key::Char(c) = key {
        if mods.ctrl {
            if let Some(s) = ctrl_char(c) {
                return Some(s);
            }
        }
        if mods.alt {
            return Some(format!("{ESC}{c}"));
        }
    }

    match key {
        Key::Char(c) => Some(c.to_string()),
        Key::Enter => Some("\r".to_string()),
        Key::Backspace => Some("\x7f".to_string()),
        Key::Escape => Some("\x1b".to_string()),
        Key::Tab => {
            if mods.shift {
                Some(format!("{CSI}Z"))
            } else {
                Some("\t".to_string())
            }
        }
        Key::Up => Some(cursor_key('A', mods, app_cursor)),
        Key::Down => Some(cursor_key('B', mods, app_cursor)),
        Key::Right => Some(cursor_key('C', mods, app_cursor)),
        Key::Left => Some(cursor_key('D', mods, app_cursor)),
        Key::Home => Some(cursor_key('H', mods, app_cursor)),
        Key::End => Some(cursor_key('F', mods, app_cursor)),
        Key::Insert => Some(tilde_key(2, mods)),
        Key::Delete => Some(tilde_key(3, mods)),
        Key::PageUp => Some(tilde_key(5, mods)),
        Key::PageDown => Some(tilde_key(6, mods)),
        Key::F(n) => f_key(n),
    }
}

/// Arrow/Home/End encoding: `SS3` + letter when unmodified in app-cursor
/// mode, `CSI` + letter unmodified otherwise, `CSI 1;{m}` + letter when any
/// modifier is held (app-cursor mode has no modified variant of its own).
fn cursor_key(letter: char, mods: Modifiers, app_cursor: bool) -> String {
    if mods.none() {
        if app_cursor {
            format!("{SS3}{letter}")
        } else {
            format!("{CSI}{letter}")
        }
    } else {
        format!("{CSI}1;{}{letter}", mods.xterm_param())
    }
}

/// Insert/Delete/PageUp/PageDown encoding: `CSI n~` unmodified, `CSI n;{m}~`
/// modified.
fn tilde_key(n: u8, mods: Modifiers) -> String {
    if mods.none() {
        format!("{CSI}{n}~")
    } else {
        format!("{CSI}{n};{}~", mods.xterm_param())
    }
}

/// F1-F12: F1-F4 are `SS3` letters, F5 and up are `CSI n~` with xterm's
/// historical gap at 16 and 22 (never assigned, keep it — matches real
/// xterm).
fn f_key(n: u8) -> Option<String> {
    match n {
        1 => Some(format!("{SS3}P")),
        2 => Some(format!("{SS3}Q")),
        3 => Some(format!("{SS3}R")),
        4 => Some(format!("{SS3}S")),
        5 => Some(format!("{CSI}15~")),
        6 => Some(format!("{CSI}17~")),
        7 => Some(format!("{CSI}18~")),
        8 => Some(format!("{CSI}19~")),
        9 => Some(format!("{CSI}20~")),
        10 => Some(format!("{CSI}21~")),
        11 => Some(format!("{CSI}23~")),
        12 => Some(format!("{CSI}24~")),
        _ => None,
    }
}

/// Ctrl+char, xterm's control-code encoding. `Some` only for the
/// combinations that actually produce a control code; a caller falls back to
/// plain-char / alt-char encoding otherwise.
fn ctrl_char(c: char) -> Option<String> {
    match c {
        ' ' => Some('\u{0}'.to_string()),
        '[' => Some('\u{1b}'.to_string()),
        '\\' => Some('\u{1c}'.to_string()),
        ']' => Some('\u{1d}'.to_string()),
        c if c.is_ascii_alphabetic() => {
            let code = (c.to_ascii_uppercase() as u8) & 0x1f;
            Some((code as char).to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, ctrl: bool, alt: bool) -> Modifiers {
        Modifiers { shift, ctrl, alt }
    }

    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
    };

    #[test]
    fn plain_char_returns_itself() {
        assert_eq!(encode(Key::Char('a'), NONE, false).as_deref(), Some("a"));
        assert_eq!(encode(Key::Char('Z'), NONE, true).as_deref(), Some("Z"));
    }

    #[test]
    fn enter_backspace_escape_tab() {
        assert_eq!(encode(Key::Enter, NONE, false).as_deref(), Some("\r"));
        assert_eq!(encode(Key::Backspace, NONE, false).as_deref(), Some("\x7f"));
        assert_eq!(encode(Key::Escape, NONE, false).as_deref(), Some("\x1b"));
        assert_eq!(encode(Key::Tab, NONE, false).as_deref(), Some("\t"));
    }

    #[test]
    fn shift_tab_is_back_tab() {
        assert_eq!(
            encode(Key::Tab, mods(true, false, false), false).as_deref(),
            Some("\x1b[Z")
        );
    }

    // --- arrows: normal mode ------------------------------------------------

    #[test]
    fn arrows_normal_mode_unmodified() {
        assert_eq!(encode(Key::Up, NONE, false).as_deref(), Some("\x1b[A"));
        assert_eq!(encode(Key::Down, NONE, false).as_deref(), Some("\x1b[B"));
        assert_eq!(encode(Key::Right, NONE, false).as_deref(), Some("\x1b[C"));
        assert_eq!(encode(Key::Left, NONE, false).as_deref(), Some("\x1b[D"));
    }

    // --- arrows: app-cursor mode ---------------------------------------------

    #[test]
    fn arrows_app_cursor_mode_unmodified_uses_ss3() {
        assert_eq!(encode(Key::Up, NONE, true).as_deref(), Some("\x1bOA"));
        assert_eq!(encode(Key::Down, NONE, true).as_deref(), Some("\x1bOB"));
        assert_eq!(encode(Key::Right, NONE, true).as_deref(), Some("\x1bOC"));
        assert_eq!(encode(Key::Left, NONE, true).as_deref(), Some("\x1bOD"));
    }

    #[test]
    fn arrows_modified_use_csi_1_m_regardless_of_app_cursor() {
        // shift = m2
        assert_eq!(
            encode(Key::Up, mods(true, false, false), false).as_deref(),
            Some("\x1b[1;2A")
        );
        assert_eq!(
            encode(Key::Up, mods(true, false, false), true).as_deref(),
            Some("\x1b[1;2A")
        );
        // alt = m3
        assert_eq!(
            encode(Key::Left, mods(false, false, true), false).as_deref(),
            Some("\x1b[1;3D")
        );
        // shift+alt = m4
        assert_eq!(
            encode(Key::Left, mods(true, false, true), false).as_deref(),
            Some("\x1b[1;4D")
        );
        // ctrl = m5
        assert_eq!(
            encode(Key::Right, mods(false, true, false), false).as_deref(),
            Some("\x1b[1;5C")
        );
        // shift+ctrl = m6
        assert_eq!(
            encode(Key::Right, mods(true, true, false), false).as_deref(),
            Some("\x1b[1;6C")
        );
        // alt+ctrl = m7
        assert_eq!(
            encode(Key::Down, mods(false, true, true), false).as_deref(),
            Some("\x1b[1;7B")
        );
        // shift+alt+ctrl = m8
        assert_eq!(
            encode(Key::Down, mods(true, true, true), false).as_deref(),
            Some("\x1b[1;8B")
        );
    }

    // --- Home/End ------------------------------------------------------------

    #[test]
    fn home_end_normal_mode() {
        assert_eq!(encode(Key::Home, NONE, false).as_deref(), Some("\x1b[H"));
        assert_eq!(encode(Key::End, NONE, false).as_deref(), Some("\x1b[F"));
    }

    #[test]
    fn home_end_app_cursor_mode_uses_ss3() {
        assert_eq!(encode(Key::Home, NONE, true).as_deref(), Some("\x1bOH"));
        assert_eq!(encode(Key::End, NONE, true).as_deref(), Some("\x1bOF"));
    }

    #[test]
    fn home_end_modified() {
        assert_eq!(
            encode(Key::Home, mods(true, false, false), false).as_deref(),
            Some("\x1b[1;2H")
        );
        assert_eq!(
            encode(Key::End, mods(false, true, false), true).as_deref(),
            Some("\x1b[1;5F")
        );
    }

    // --- Insert/Delete/PageUp/PageDown (no app-cursor variant) ---------------

    #[test]
    fn insert_delete_page_up_page_down_unmodified() {
        assert_eq!(encode(Key::Insert, NONE, false).as_deref(), Some("\x1b[2~"));
        assert_eq!(encode(Key::Delete, NONE, false).as_deref(), Some("\x1b[3~"));
        assert_eq!(encode(Key::PageUp, NONE, false).as_deref(), Some("\x1b[5~"));
        assert_eq!(
            encode(Key::PageDown, NONE, false).as_deref(),
            Some("\x1b[6~")
        );
        // app_cursor has no effect on these.
        assert_eq!(encode(Key::Insert, NONE, true).as_deref(), Some("\x1b[2~"));
        assert_eq!(
            encode(Key::PageDown, NONE, true).as_deref(),
            Some("\x1b[6~")
        );
    }

    #[test]
    fn insert_delete_page_up_page_down_modified() {
        assert_eq!(
            encode(Key::Delete, mods(false, true, false), false).as_deref(),
            Some("\x1b[3;5~")
        );
        assert_eq!(
            encode(Key::PageUp, mods(true, false, false), false).as_deref(),
            Some("\x1b[5;2~")
        );
    }

    // --- F-keys (no app-cursor variant) ---------------------------------------

    #[test]
    fn f1_to_f4_use_ss3() {
        assert_eq!(encode(Key::F(1), NONE, false).as_deref(), Some("\x1bOP"));
        assert_eq!(encode(Key::F(2), NONE, false).as_deref(), Some("\x1bOQ"));
        assert_eq!(encode(Key::F(3), NONE, false).as_deref(), Some("\x1bOR"));
        assert_eq!(encode(Key::F(4), NONE, false).as_deref(), Some("\x1bOS"));
        // app_cursor has no effect on F-keys.
        assert_eq!(encode(Key::F(1), NONE, true).as_deref(), Some("\x1bOP"));
    }

    #[test]
    fn f5_to_f12_use_csi_tilde_with_the_historical_gap() {
        assert_eq!(encode(Key::F(5), NONE, false).as_deref(), Some("\x1b[15~"));
        assert_eq!(encode(Key::F(6), NONE, false).as_deref(), Some("\x1b[17~"));
        assert_eq!(encode(Key::F(7), NONE, false).as_deref(), Some("\x1b[18~"));
        assert_eq!(encode(Key::F(8), NONE, false).as_deref(), Some("\x1b[19~"));
        assert_eq!(encode(Key::F(9), NONE, false).as_deref(), Some("\x1b[20~"));
        assert_eq!(encode(Key::F(10), NONE, false).as_deref(), Some("\x1b[21~"));
        assert_eq!(encode(Key::F(11), NONE, false).as_deref(), Some("\x1b[23~"));
        assert_eq!(encode(Key::F(12), NONE, false).as_deref(), Some("\x1b[24~"));
    }

    // --- Ctrl+letter / special ctrl combos -------------------------------------

    #[test]
    fn ctrl_letter_produces_the_control_code() {
        assert_eq!(
            encode(Key::Char('a'), mods(false, true, false), false).as_deref(),
            Some("\u{1}")
        );
        assert_eq!(
            encode(Key::Char('C'), mods(false, true, false), false).as_deref(),
            Some("\u{3}")
        );
        assert_eq!(
            encode(Key::Char('z'), mods(false, true, false), false).as_deref(),
            Some("\u{1a}")
        );
    }

    #[test]
    fn ctrl_special_combos() {
        assert_eq!(
            encode(Key::Char(' '), mods(false, true, false), false).as_deref(),
            Some("\u{0}")
        );
        assert_eq!(
            encode(Key::Char('['), mods(false, true, false), false).as_deref(),
            Some("\u{1b}")
        );
        assert_eq!(
            encode(Key::Char('\\'), mods(false, true, false), false).as_deref(),
            Some("\u{1c}")
        );
        assert_eq!(
            encode(Key::Char(']'), mods(false, true, false), false).as_deref(),
            Some("\u{1d}")
        );
    }

    #[test]
    fn alt_char_meta_prefixes() {
        assert_eq!(
            encode(Key::Char('x'), mods(false, false, true), false).as_deref(),
            Some("\x1bx")
        );
    }

    #[test]
    fn ctrl_takes_priority_over_alt_when_both_held() {
        assert_eq!(
            encode(Key::Char('a'), mods(false, true, true), false).as_deref(),
            Some("\u{1}")
        );
    }

    #[test]
    fn ctrl_on_a_non_letter_non_special_char_falls_back_to_plain_char() {
        // No control code defined for '1'; ctrl is simply not encoded rather
        // than swallowing the keystroke.
        assert_eq!(
            encode(Key::Char('1'), mods(false, true, false), false).as_deref(),
            Some("1")
        );
    }
}
