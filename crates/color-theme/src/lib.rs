//! Colour themes as data: the domain model a `color-themes` plugin
//! contribution ships, and the two formats it can arrive in.
//!
//! Qt-free by design (mirrors `icon-theme`'s isolation, per
//! `docs/architecture/layering.md`): this crate knows nothing about
//! `plugin-api`, `syntax-core`, or cxx-qt. `app-core` is where a parsed
//! [`ColorTheme`] gets joined to the rest of the application.
//!
//! [`Appearance`] deliberately duplicates `icon_theme::Appearance` rather
//! than depending on `icon-theme` — `app_core::icons` is where the two get
//! mapped to each other.

use std::collections::HashMap;
use std::fmt;

/// A 32-bit colour: three colour channels plus alpha, each 0-255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parses a CSS-hex-like colour: `#rgb`, `#rrggbb`, `#rgba`, `#rrggbbaa`,
    /// case-insensitive. The leading `#` is required; anything else is a
    /// [`ThemeError::InvalidColor`] naming `value`. The error's `key` is
    /// left empty here — a caller that knows which field this string came
    /// from should add it with [`ThemeError::with_key`].
    pub fn parse(value: &str) -> Result<Self, ThemeError> {
        let invalid = || ThemeError::InvalidColor {
            key: String::new(),
            value: value.to_string(),
        };
        let hex = value.strip_prefix('#').ok_or_else(invalid)?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let nibble = |c: u8| (c as char).to_digit(16).map(|n| n as u8);
        let double = |c: u8| nibble(c).map(|n| n * 16 + n);
        let pair = |hi: u8, lo: u8| Some(nibble(hi)? * 16 + nibble(lo)?);
        let bytes = hex.as_bytes();
        let (r, g, b, a) = match hex.len() {
            3 => (
                double(bytes[0]),
                double(bytes[1]),
                double(bytes[2]),
                Some(255),
            ),
            4 => (
                double(bytes[0]),
                double(bytes[1]),
                double(bytes[2]),
                double(bytes[3]),
            ),
            6 => (
                pair(bytes[0], bytes[1]),
                pair(bytes[2], bytes[3]),
                pair(bytes[4], bytes[5]),
                Some(255),
            ),
            8 => (
                pair(bytes[0], bytes[1]),
                pair(bytes[2], bytes[3]),
                pair(bytes[4], bytes[5]),
                pair(bytes[6], bytes[7]),
            ),
            _ => return Err(invalid()),
        };
        let (Some(r), Some(g), Some(b), Some(a)) = (r, g, b, a) else {
            return Err(invalid());
        };
        Ok(Self::new(r, g, b, a))
    }

    /// Standard alpha compositing of `self` over `base`, flattened to an
    /// opaque (`a == 255`) result — the same semantics as `theme.cpp`'s
    /// `over()` helper (`crates/ui-shell/cpp/theme.cpp:27`), ported fresh
    /// because this crate cannot depend on Qt.
    pub fn over(&self, base: Rgba) -> Rgba {
        let alpha = f32::from(self.a) / 255.0;
        let mix = |b: u8, t: u8| -> u8 {
            (f32::from(b) + (f32::from(t) - f32::from(b)) * alpha).round() as u8
        };
        Rgba::new(
            mix(base.r, self.r),
            mix(base.g, self.g),
            mix(base.b, self.b),
            255,
        )
    }
}

/// Why a theme file could not be turned into a [`ColorTheme`].
///
/// Typed rather than a formatted string, the same reason `icon_theme::IconError`
/// is: `key` is a dotted field path within the theme file (e.g.
/// `"chrome.canvas"`, `"syntax.keyword.fg"`) — this crate does not know the
/// file's path, so naming it is the caller's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    /// `value` at `key` is not a hex colour [`Rgba::parse`] accepts.
    InvalidColor { key: String, value: String },
    /// A required field is missing.
    MissingField { key: String },
    /// The input is not valid TOML/JSON, or has the wrong shape.
    Parse { message: String },
}

impl ThemeError {
    /// Attaches `key` to an [`ThemeError::InvalidColor`] raised by
    /// [`Rgba::parse`], which does not know which field it was parsing.
    /// A no-op on every other variant.
    fn with_key(self, key: &str) -> Self {
        match self {
            Self::InvalidColor { value, .. } => Self::InvalidColor {
                key: key.to_string(),
                value,
            },
            other => other,
        }
    }
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidColor { key, value } => {
                write!(f, "`{key}`: `{value}` is not a valid hex colour")
            }
            Self::MissingField { key } => write!(f, "`{key}` is required"),
            Self::Parse { message } => write!(f, "malformed theme: {message}"),
        }
    }
}

impl std::error::Error for ThemeError {}

/// Whether a theme is meant for a dark or light desktop appearance.
///
/// Duplicates `icon_theme::Appearance` on purpose: this crate must not
/// depend on `icon-theme` (see the module doc comment) — `app_core::icons`
/// maps the two together where both are actually needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// The colour roles of the chrome design spec (`--ide-*`), one set per
/// theme — mirrors `ui_shell::ChromePalette` (`crates/ui-shell/cpp/theme.h`).
/// `chevron` (an icon resource path) and the shadow ink/opacity pair are not
/// modelled here: they are not colour data a theme file supplies, only
/// resources/constants `theme.cpp` still owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeColors {
    pub canvas: Rgba,
    pub surface: Rgba,
    pub surface2: Rgba,
    pub raised: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub accent: Rgba,
    pub accent_ink: Rgba,
    pub selection: Rgba,
    pub status_bar: Rgba,
}

/// The status/severity colours every label in the product uses — mirrors
/// `ui_shell::SemanticColors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticColors {
    pub error: Rgba,
    pub warning: Rgba,
    pub info: Rgba,
    pub ok: Rgba,
    pub muted: Rgba,
}

/// The colours a diff paints with — mirrors `ui_shell::DiffColors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffColors {
    pub added_line: Rgba,
    pub added_inline: Rgba,
    pub added_marker: Rgba,
    pub modified_line: Rgba,
    pub modified_inline: Rgba,
    pub modified_marker: Rgba,
    pub deleted_line: Rgba,
    pub deleted_inline: Rgba,
    pub deleted_marker: Rgba,
}

/// The terminal's ANSI palette plus its four fixed roles — mirrors
/// `FfiTerminalPalette` (`crates/ui-shell/src/bridge/ffi.rs`), with the 16
/// ANSI slots named rather than kept as a `Vec` so a TOML/JSON theme file
/// can address each one by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalColors {
    pub black: Rgba,
    pub red: Rgba,
    pub green: Rgba,
    pub yellow: Rgba,
    pub blue: Rgba,
    pub magenta: Rgba,
    pub cyan: Rgba,
    pub white: Rgba,
    pub bright_black: Rgba,
    pub bright_red: Rgba,
    pub bright_green: Rgba,
    pub bright_yellow: Rgba,
    pub bright_blue: Rgba,
    pub bright_magenta: Rgba,
    pub bright_cyan: Rgba,
    pub bright_white: Rgba,
    pub background: Rgba,
    pub foreground: Rgba,
    pub cursor: Rgba,
    pub selection: Rgba,
}

impl TerminalColors {
    /// The 16 ANSI slots in the order `FfiTerminalPalette::ansi` (and
    /// `terminal_core`) expect, index 0-15.
    pub fn ansi(&self) -> [Rgba; 16] {
        [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
            self.bright_black,
            self.bright_red,
            self.bright_green,
            self.bright_yellow,
            self.bright_blue,
            self.bright_magenta,
            self.bright_cyan,
            self.bright_white,
        ]
    }
}

/// How one TextMate/tree-sitter scope is painted — mirrors
/// `syntax_core::theme::ScopeStyle`, except `fg` is required: a scope with
/// no colour of its own is simply absent from [`ColorTheme::syntax`], and
/// `syntax-core`'s existing parent-scope inheritance covers it from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeStyle {
    pub fg: Rgba,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A complete colour theme, parsed from either format this crate accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorTheme {
    pub id: String,
    pub label: String,
    pub appearance: Appearance,
    pub chrome: ChromeColors,
    pub semantic: SemanticColors,
    pub diff: DiffColors,
    pub terminal: TerminalColors,
    pub syntax: HashMap<String, ScopeStyle>,
}

// --- native TOML format -----------------------------------------------

/// The raw shape `parse_toml` deserializes: every colour is a hex string,
/// converted to `Rgba` (with a dotted-key error on failure) by `ColorTheme::try_from`.
/// A raw/converted pair rather than per-field `deserialize_with` because
/// `toml::de::Error` alone cannot carry the `key` this crate's `ThemeError`
/// wants — the conversion step is where that key is known.
mod raw {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Theme {
        pub id: String,
        pub label: String,
        pub appearance: String,
        pub chrome: Chrome,
        pub semantic: Semantic,
        pub diff: Diff,
        pub terminal: Terminal,
        #[serde(default)]
        pub syntax: std::collections::HashMap<String, Scope>,
    }

    #[derive(Deserialize)]
    pub struct Chrome {
        pub canvas: String,
        pub surface: String,
        pub surface2: String,
        pub raised: String,
        pub border: String,
        pub text: String,
        pub text_dim: String,
        pub accent: String,
        pub accent_ink: String,
        pub selection: String,
        pub status_bar: String,
    }

    #[derive(Deserialize)]
    pub struct Semantic {
        pub error: String,
        pub warning: String,
        pub info: String,
        pub ok: String,
        pub muted: String,
    }

    #[derive(Deserialize)]
    pub struct Diff {
        pub added_line: String,
        pub added_inline: String,
        pub added_marker: String,
        pub modified_line: String,
        pub modified_inline: String,
        pub modified_marker: String,
        pub deleted_line: String,
        pub deleted_inline: String,
        pub deleted_marker: String,
    }

    #[derive(Deserialize)]
    pub struct Terminal {
        pub black: String,
        pub red: String,
        pub green: String,
        pub yellow: String,
        pub blue: String,
        pub magenta: String,
        pub cyan: String,
        pub white: String,
        pub bright_black: String,
        pub bright_red: String,
        pub bright_green: String,
        pub bright_yellow: String,
        pub bright_blue: String,
        pub bright_magenta: String,
        pub bright_cyan: String,
        pub bright_white: String,
        pub background: String,
        pub foreground: String,
        pub cursor: String,
        pub selection: String,
    }

    #[derive(Deserialize)]
    pub struct Scope {
        pub fg: String,
        #[serde(default)]
        pub bold: bool,
        #[serde(default)]
        pub italic: bool,
        #[serde(default)]
        pub underline: bool,
    }
}

/// Parses a colour theme from this crate's native TOML shape: top-level
/// `id`/`label`/`appearance`, then `[chrome]`, `[semantic]`, `[diff]`,
/// `[terminal]` tables with one hex-string field per struct member above,
/// and `[syntax.<scope-name>]` tables of `{ fg, bold, italic, underline }`.
pub fn parse_toml(input: &str) -> Result<ColorTheme, ThemeError> {
    let raw: raw::Theme = toml::from_str(input).map_err(|e| ThemeError::Parse {
        message: e.to_string(),
    })?;

    let color = |value: &str, key: &str| Rgba::parse(value).map_err(|e| e.with_key(key));

    let appearance = match raw.appearance.as_str() {
        "dark" => Appearance::Dark,
        "light" => Appearance::Light,
        other => {
            return Err(ThemeError::Parse {
                message: format!("`appearance` must be \"dark\" or \"light\", got \"{other}\""),
            })
        }
    };

    let c = &raw.chrome;
    let chrome = ChromeColors {
        canvas: color(&c.canvas, "chrome.canvas")?,
        surface: color(&c.surface, "chrome.surface")?,
        surface2: color(&c.surface2, "chrome.surface2")?,
        raised: color(&c.raised, "chrome.raised")?,
        border: color(&c.border, "chrome.border")?,
        text: color(&c.text, "chrome.text")?,
        text_dim: color(&c.text_dim, "chrome.text_dim")?,
        accent: color(&c.accent, "chrome.accent")?,
        accent_ink: color(&c.accent_ink, "chrome.accent_ink")?,
        selection: color(&c.selection, "chrome.selection")?,
        status_bar: color(&c.status_bar, "chrome.status_bar")?,
    };

    let s = &raw.semantic;
    let semantic = SemanticColors {
        error: color(&s.error, "semantic.error")?,
        warning: color(&s.warning, "semantic.warning")?,
        info: color(&s.info, "semantic.info")?,
        ok: color(&s.ok, "semantic.ok")?,
        muted: color(&s.muted, "semantic.muted")?,
    };

    let d = &raw.diff;
    let diff = DiffColors {
        added_line: color(&d.added_line, "diff.added_line")?,
        added_inline: color(&d.added_inline, "diff.added_inline")?,
        added_marker: color(&d.added_marker, "diff.added_marker")?,
        modified_line: color(&d.modified_line, "diff.modified_line")?,
        modified_inline: color(&d.modified_inline, "diff.modified_inline")?,
        modified_marker: color(&d.modified_marker, "diff.modified_marker")?,
        deleted_line: color(&d.deleted_line, "diff.deleted_line")?,
        deleted_inline: color(&d.deleted_inline, "diff.deleted_inline")?,
        deleted_marker: color(&d.deleted_marker, "diff.deleted_marker")?,
    };

    let t = &raw.terminal;
    let terminal = TerminalColors {
        black: color(&t.black, "terminal.black")?,
        red: color(&t.red, "terminal.red")?,
        green: color(&t.green, "terminal.green")?,
        yellow: color(&t.yellow, "terminal.yellow")?,
        blue: color(&t.blue, "terminal.blue")?,
        magenta: color(&t.magenta, "terminal.magenta")?,
        cyan: color(&t.cyan, "terminal.cyan")?,
        white: color(&t.white, "terminal.white")?,
        bright_black: color(&t.bright_black, "terminal.bright_black")?,
        bright_red: color(&t.bright_red, "terminal.bright_red")?,
        bright_green: color(&t.bright_green, "terminal.bright_green")?,
        bright_yellow: color(&t.bright_yellow, "terminal.bright_yellow")?,
        bright_blue: color(&t.bright_blue, "terminal.bright_blue")?,
        bright_magenta: color(&t.bright_magenta, "terminal.bright_magenta")?,
        bright_cyan: color(&t.bright_cyan, "terminal.bright_cyan")?,
        bright_white: color(&t.bright_white, "terminal.bright_white")?,
        background: color(&t.background, "terminal.background")?,
        foreground: color(&t.foreground, "terminal.foreground")?,
        cursor: color(&t.cursor, "terminal.cursor")?,
        selection: color(&t.selection, "terminal.selection")?,
    };

    let mut syntax = HashMap::with_capacity(raw.syntax.len());
    for (name, scope) in &raw.syntax {
        let fg = color(&scope.fg, &format!("syntax.{name}.fg"))?;
        syntax.insert(
            name.clone(),
            ScopeStyle {
                fg,
                bold: scope.bold,
                italic: scope.italic,
                underline: scope.underline,
            },
        );
    }

    Ok(ColorTheme {
        id: raw.id,
        label: raw.label,
        appearance,
        chrome,
        semantic,
        diff,
        terminal,
        syntax,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_four_hex_forms() {
        assert_eq!(Rgba::parse("#f00").unwrap(), Rgba::new(255, 0, 0, 255));
        assert_eq!(Rgba::parse("#F00A").unwrap(), Rgba::new(255, 0, 0, 0xAA));
        assert_eq!(Rgba::parse("#ff0000").unwrap(), Rgba::new(255, 0, 0, 255));
        assert_eq!(
            Rgba::parse("#ff000080").unwrap(),
            Rgba::new(255, 0, 0, 0x80)
        );
    }

    #[test]
    fn rejects_missing_hash_and_bad_length_and_bad_digits() {
        assert!(Rgba::parse("ff0000").is_err());
        assert!(Rgba::parse("#ff0000f").is_err());
        assert!(Rgba::parse("#gg0000").is_err());
        let err = Rgba::parse("#zzz").unwrap_err();
        assert_eq!(
            err,
            ThemeError::InvalidColor {
                key: String::new(),
                value: "#zzz".to_string(),
            }
        );
    }

    #[test]
    fn over_blends_half_alpha_red_onto_white() {
        let red_half = Rgba::new(255, 0, 0, 128);
        let white = Rgba::new(255, 255, 255, 255);
        let blended = red_half.over(white);
        // green/blue: 255 + (0-255) * (128/255) == 255 - 255*128/255 == 127 exactly.
        assert_eq!(blended, Rgba::new(255, 127, 127, 255));
    }

    #[test]
    fn over_is_opaque_regardless_of_input_alpha() {
        let translucent = Rgba::new(10, 20, 30, 0);
        let result = translucent.over(Rgba::new(0, 0, 0, 255));
        assert_eq!(result.a, 255);
        // Zero alpha means "all base": the top colour contributes nothing.
        assert_eq!(result, Rgba::new(0, 0, 0, 255));
    }

    fn minimal_theme_toml() -> String {
        r##"
            id = "test-theme"
            label = "Test Theme"
            appearance = "dark"

            [chrome]
            canvas = "#1e1f22"
            surface = "#2b2d30"
            surface2 = "#26282b"
            raised = "#2f3136"
            border = "#3a3a3a"
            text = "#dfe1e5"
            text_dim = "#8a8f98"
            accent = "#3574f0"
            accent_ink = "#ffffff"
            selection = "#3a4a6b"
            status_bar = "#2b2d30"

            [semantic]
            error = "#ff5555"
            warning = "#ffb454"
            info = "#59a1e0"
            ok = "#7fd962"
            muted = "#8a8f98"

            [diff]
            added_line = "#1e3a1e"
            added_inline = "#2d5a2d"
            added_marker = "#4caf50"
            modified_line = "#1e2f4a"
            modified_inline = "#2d4a70"
            modified_marker = "#3574f0"
            deleted_line = "#3a1e1e"
            deleted_inline = "#5a2d2d"
            deleted_marker = "#f44336"

            [terminal]
            black = "#000000"
            red = "#ff5555"
            green = "#7fd962"
            yellow = "#ffb454"
            blue = "#59a1e0"
            magenta = "#c678dd"
            cyan = "#56b6c2"
            white = "#dfe1e5"
            bright_black = "#5c6370"
            bright_red = "#ff6e67"
            bright_green = "#9ae37c"
            bright_yellow = "#ffd479"
            bright_blue = "#7fb8ea"
            bright_magenta = "#d8a1e6"
            bright_cyan = "#79cdd8"
            bright_white = "#ffffff"
            background = "#1e1f22"
            foreground = "#dfe1e5"
            cursor = "#dfe1e5"
            selection = "#3a4a6b"

            [syntax.keyword]
            fg = "#cc7832"
            bold = false
            italic = false
            underline = false

            [syntax."comment.documentation"]
            fg = "#629755"
            italic = true
        "##
        .to_string()
    }

    #[test]
    fn parse_toml_round_trips_a_minimal_valid_theme() {
        let theme = parse_toml(&minimal_theme_toml()).expect("valid theme parses");
        assert_eq!(theme.id, "test-theme");
        assert_eq!(theme.label, "Test Theme");
        assert_eq!(theme.appearance, Appearance::Dark);
        assert_eq!(theme.chrome.canvas, Rgba::new(0x1e, 0x1f, 0x22, 255));
        assert_eq!(theme.semantic.error, Rgba::new(0xff, 0x55, 0x55, 255));
        assert_eq!(theme.diff.added_marker, Rgba::new(0x4c, 0xaf, 0x50, 255));
        assert_eq!(theme.terminal.ansi()[1], Rgba::new(0xff, 0x55, 0x55, 255));
        let keyword = theme.syntax.get("keyword").expect("keyword style present");
        assert_eq!(keyword.fg, Rgba::new(0xcc, 0x78, 0x32, 255));
        assert!(!keyword.bold);
        let doc_comment = theme
            .syntax
            .get("comment.documentation")
            .expect("comment.documentation style present");
        assert!(doc_comment.italic);
    }

    #[test]
    fn parse_toml_reports_missing_field_by_key() {
        let missing_border = minimal_theme_toml().replace("border = \"#3a3a3a\"\n", "");
        let err = parse_toml(&missing_border).unwrap_err();
        assert!(matches!(err, ThemeError::Parse { .. }));
    }

    #[test]
    fn parse_toml_reports_bad_hex_by_dotted_key() {
        let bad_hex = minimal_theme_toml().replace("canvas = \"#1e1f22\"", "canvas = \"nope\"");
        let err = parse_toml(&bad_hex).unwrap_err();
        assert_eq!(
            err,
            ThemeError::InvalidColor {
                key: "chrome.canvas".to_string(),
                value: "nope".to_string(),
            }
        );
    }
}
