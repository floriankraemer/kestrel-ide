//! Syntax colour rules and the built-in colour tables.
//!
//! Lives here rather than in a crate of its own (plan decision D7) because
//! resolution walks the same dotted scope hierarchy [`Scope::resolve`]
//! already implements for capture names.
//!
//! Persistence is *not* here: `app-config` stores plain string maps, and
//! this module takes them as a parameter so neither crate depends on the
//! other.

use std::collections::HashMap;

use crate::{Scope, SCOPES};

/// A 24-bit colour. Parsed from the `#rrggbb` strings the config stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parses `#rrggbb` (and the same without the `#`). `None` for anything
    /// else — a malformed colour in a user's config is ignored, not fatal.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self::new(byte(0)?, byte(2)?, byte(4)?))
    }
}

/// How one scope is painted. `fg == None` means "no colour of its own":
/// either inherited from a parent scope, or — once the whole chain is
/// exhausted — the editor's default foreground.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopeStyle {
    pub fg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl ScopeStyle {
    pub const fn fg(color: Rgb) -> Self {
        Self {
            fg: Some(color),
            bold: false,
            italic: false,
            underline: false,
        }
    }

    const fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    const fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }

    const fn underline(self) -> Self {
        Self {
            underline: true,
            ..self
        }
    }
}

/// `#rrggbb` as a const literal — the tables below are long and one line
/// per scope reads like the palette it is a port of.
const fn hex(rgb: u32) -> ScopeStyle {
    ScopeStyle::fg(Rgb::new((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

/// Emphasis is a shape, not a colour: the `markup.italic`/`markup.bold`
/// entries carry no `fg` and inherit whatever the surrounding prose uses.
const PLAIN: ScopeStyle = ScopeStyle {
    fg: None,
    bold: false,
    italic: false,
    underline: false,
};

/// A named colour table: base scope entries plus optional per-language ones.
pub struct Theme {
    pub name: &'static str,
    pub base: &'static [(&'static str, ScopeStyle)],
    /// `(language id, that language's scope entries)`.
    pub languages: &'static [(&'static str, &'static [(&'static str, ScopeStyle)])],
}

// Darcula, as IntelliJ IDEA ships it (editor background `#2b2b2b`,
// default foreground `#a9b7c6`). The six original entries are the verbatim
// C++ `colorForKind` literals; the rest are Darcula's own token colours for
// the scopes the taxonomy grew after that port.
const DARCULA: &[(&str, ScopeStyle)] = &[
    ("keyword", hex(0xcc7832)),
    ("string", hex(0x6a8759)),
    ("character", hex(0x6a8759)),
    ("comment", hex(0x808080)),
    ("comment.documentation", hex(0x629755)),
    ("number", hex(0x6897bb)),
    ("function", hex(0xffc66d)),
    ("constructor", hex(0xffc66d)),
    // Darcula's class colour *is* the default foreground; see the note on
    // `type` in `unstyled_by_design` below.
    ("type", hex(0xa9b7c6)),
    // Primitive types read as keywords in IntelliJ (`int`, `usize`).
    ("type.builtin", hex(0xcc7832)),
    // Fields and constants are Darcula's purple; static finals italic.
    ("constant", hex(0x9876aa).italic()),
    ("constant.builtin", hex(0xcc7832)),
    ("boolean", hex(0xcc7832)),
    ("property", hex(0x9876aa)),
    ("variable.member", hex(0x9876aa)),
    // `this`/`self` are keywords in the languages Darcula was designed for.
    ("variable.builtin", hex(0xcc7832)),
    ("attribute", hex(0xbbb529)),
    ("escape", hex(0xcc7832)),
    ("string.escape", hex(0xcc7832)),
    ("tag", hex(0xe8bf6a)),
    ("markup.heading", hex(0xffc66d).bold()),
    ("markup.link", hex(0x589df6).underline()),
    ("markup.link.url", hex(0x6897bb)),
    ("markup.raw", hex(0x6a8759)),
    ("markup.list", hex(0xcc7832)),
    ("markup.quote", hex(0x808080).italic()),
    ("markup.italic", PLAIN.italic()),
    ("markup.bold", PLAIN.bold()),
    ("markup.strikethrough", hex(0x808080)),
    // What Darcula deliberately leaves in the default foreground is
    // recorded, with reasons, in `unstyled_by_design` in the tests below.
];

// The `light` theme paints on `#ffffff` (see `colorsForTheme` in
// `theme.cpp`), where Darcula's colours — mid-grey comments above all —
// are unreadable. IntelliJ-Light-flavoured, every one at or above WCAG AA
// 4.5:1 on white, the bar `docs/design/language-platform-ui.md` section 1
// sets for every other colour in this product. Ratio in the comment.
const LIGHT: &[(&str, ScopeStyle)] = &[
    ("keyword", hex(0x0033b3)),      // 9.9:1
    ("string", hex(0x067d17)),       // 5.3:1
    ("character", hex(0x067d17)),    // 5.3:1
    ("comment", hex(0x5f6b7a)),      // 5.4:1
    ("number", hex(0x1750eb)),       // 6.2:1
    ("function", hex(0x795e26)),     // 6.1:1
    ("constructor", hex(0x0f5b8f)),  // 7.2:1
    ("type", hex(0x0f5b8f)),         // 7.2:1
    ("type.builtin", hex(0x0033b3)), // 9.9:1
    // IntelliJ Light's field/constant purple.
    ("constant", hex(0x871094).italic()), // 8.3:1
    ("constant.builtin", hex(0x0033b3)),  // 9.9:1
    ("boolean", hex(0x0033b3)),           // 9.9:1
    ("property", hex(0x871094)),          // 8.3:1
    ("variable.member", hex(0x871094)),   // 8.3:1
    ("variable.builtin", hex(0x0033b3)),  // 9.9:1
    // IntelliJ's annotation yellow `#9e880d` is 3.5:1 on white; darkened
    // until it clears AA.
    ("attribute", hex(0x7a6a00)),     // 5.4:1
    ("escape", hex(0x0037a6)),        // 10.0:1
    ("string.escape", hex(0x0037a6)), // 10.0:1
    ("tag", hex(0x000080)),           // 16.0:1
    ("markup.heading", hex(0x0033b3).bold()),
    ("markup.link", hex(0x1750eb).underline()),
    ("markup.link.url", hex(0x0f5b8f)),
    ("markup.raw", hex(0x067d17)),
    ("markup.list", hex(0x795e26)),
    ("markup.quote", hex(0x5f6b7a).italic()),
    ("markup.italic", PLAIN.italic()),
    ("markup.bold", PLAIN.bold()),
    ("markup.strikethrough", hex(0x5f6b7a)),
    // IntelliJ's own doc-comment grey `#8c8c8c` is 3.0:1 on white, so
    // `comment.documentation` stays on the accessible `comment` grey.
    // What is deliberately left in the default foreground is recorded, with
    // reasons, in `unstyled_by_design` in the tests below.
];

// VS Code Dark+, token colours as the shipped theme defines them. The
// editor background is `#1e1e1e`; the default foreground is this app's
// `#cccccc` (`colorsForTheme` in `theme.cpp`), not VS Code's own `#d4d4d4`.
const VSCODE_DARK: &[(&str, ScopeStyle)] = &[
    ("keyword", hex(0x569cd6)),
    ("string", hex(0xce9178)),
    ("character", hex(0xce9178)),
    ("string.regexp", hex(0xd16969)),
    ("comment", hex(0x6a9955)),
    ("number", hex(0xb5cea8)),
    ("function", hex(0xdcdcaa)),
    ("type", hex(0x4ec9b0)),
    ("constructor", hex(0x4ec9b0)),
    ("module", hex(0x4ec9b0)),
    // `storage.type` primitives (`int`, `bool`) are keyword blue in Dark+.
    ("type.builtin", hex(0x569cd6)),
    // `variable.other.constant`.
    ("constant", hex(0x4fc1ff)),
    ("constant.builtin", hex(0x569cd6)),
    ("boolean", hex(0x569cd6)),
    ("variable", hex(0x9cdcfe)),
    ("variable.builtin", hex(0x569cd6)),
    ("property", hex(0x9cdcfe)),
    ("attribute", hex(0x9cdcfe)),
    ("tag", hex(0x569cd6)),
    ("escape", hex(0xd7ba7d)),
    ("string.escape", hex(0xd7ba7d)),
    ("markup.heading", hex(0x569cd6).bold()),
    ("markup.link", hex(0x3794ff).underline()),
    ("markup.link.url", hex(0xce9178)),
    ("markup.raw", hex(0xce9178)),
    ("markup.list", hex(0x6796e6)),
    ("markup.quote", hex(0x6a9955).italic()),
    ("markup.italic", PLAIN.italic()),
    ("markup.bold", PLAIN.bold()),
    ("markup.strikethrough", hex(0x808080)),
    // What Dark+ deliberately leaves in the default foreground is recorded,
    // with reasons, in `unstyled_by_design` in the tests below.
];

/// The built-in themes, keyed by the same names the chrome themes use.
/// The first entry is the fallback for an unknown name.
pub static BUILTIN_THEMES: &[Theme] = &[
    Theme {
        name: "dark",
        base: DARCULA,
        languages: &[],
    },
    Theme {
        name: "light",
        base: LIGHT,
        languages: &[],
    },
    Theme {
        name: "vscode-dark",
        base: VSCODE_DARK,
        languages: &[],
    },
];

// `Theme`/`BUILTIN_THEMES`/`theme_by_name`/`palette` (the name-based overload
// below) are the pre-`color-theme`-crate way of supplying a theme's colours.
// T7 rewired `ui-shell`'s two callers (`convert.rs`'s `SyntaxHighlighterHandle::palette`,
// `settings.rs`'s `LanguagePlatformModel::scopes`) onto `build_palette` via
// `app_core::color_themes::build_palette`. This block stays — by design, not
// oversight — because `markdown-preview::highlight` still asks for a theme
// by name and rewiring its own theming is explicitly out of the
// color-themes plan's scope; delete this block and `palette` once that
// crate no longer needs them, keeping `build_palette` as the only entry
// point.
fn theme_by_name(name: &str) -> &'static Theme {
    BUILTIN_THEMES
        .iter()
        .find(|theme| theme.name == name)
        .unwrap_or(&BUILTIN_THEMES[0])
}

/// The user's colour overrides, as `app-config` persists them: scope name
/// -> style, plus the same again per language id. Unknown scope names are
/// ignored (a newer build may know them).
#[derive(Debug, Default, Clone)]
pub struct UserStyles {
    pub base: HashMap<String, ScopeStyle>,
    pub by_language: HashMap<String, HashMap<String, ScopeStyle>>,
}

/// A theme's colours, in the same base/by-language shape [`UserStyles`]
/// already uses for the caller-supplied override — [`build_palette`] walks
/// both with the identical precedence logic. This is the shape a future
/// `ColorThemeService` (T6) hands in, once themes come from TOML rather
/// than the `BUILTIN_THEMES` table below.
#[derive(Debug, Default, Clone)]
pub struct ThemeStyles {
    pub base: HashMap<String, ScopeStyle>,
    pub by_language: HashMap<String, HashMap<String, ScopeStyle>>,
}

/// Resolved styles for one (theme, language), indexed by [`Scope::id`] and
/// always exactly `SCOPES.len()` long — the view builds its format table
/// straight from this and range-guards the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    styles: Vec<ScopeStyle>,
}

impl Palette {
    pub fn style(&self, scope: Scope) -> ScopeStyle {
        self.styles[usize::from(scope.id())]
    }

    pub fn styles(&self) -> &[ScopeStyle] {
        &self.styles
    }
}

/// Resolves every scope for one theme and language. Build once per
/// (theme, language); it is pure data afterwards.
///
/// Precedence, highest first: per-language user override, base user
/// override, per-language theme entry, base theme entry — then the same
/// four again on the parent scope (`function.method` inherits `function`),
/// and finally the editor's default foreground (`fg: None`).
///
/// Font flags accumulate up that chain, so a `function.method` entry with
/// only `italic` keeps `function`'s bold and colour.
pub fn palette(theme_name: &str, language_id: &str, user: &UserStyles) -> Palette {
    let theme = theme_by_name(theme_name);
    let lang_user = user.by_language.get(language_id);
    let lang_theme = theme
        .languages
        .iter()
        .find(|(id, _)| *id == language_id)
        .map(|(_, entries)| *entries);

    let lookup = |name: &str| -> Option<ScopeStyle> {
        lang_user
            .and_then(|map| map.get(name))
            .copied()
            .or_else(|| user.base.get(name).copied())
            .or_else(|| entry(lang_theme.unwrap_or(&[]), name))
            .or_else(|| entry(theme.base, name))
    };

    resolve_scopes(lookup)
}

/// Same resolution as [`palette`], but sourced from an owned [`ThemeStyles`]
/// instead of a built-in theme name — the entry point a `ThemeStyles`
/// supplier (a `ColorThemeService`, in T6) resolves against.
///
/// Precedence and inheritance are identical to `palette`; see its doc for
/// the full walk.
pub fn build_palette(theme: &ThemeStyles, language_id: &str, user: &UserStyles) -> Palette {
    let lang_user = user.by_language.get(language_id);
    let lang_theme = theme.by_language.get(language_id);

    let lookup = |name: &str| -> Option<ScopeStyle> {
        lang_user
            .and_then(|map| map.get(name))
            .copied()
            .or_else(|| user.base.get(name).copied())
            .or_else(|| lang_theme.and_then(|map| map.get(name)).copied())
            .or_else(|| theme.base.get(name).copied())
    };

    resolve_scopes(lookup)
}

/// The per-scope walk shared by `palette` and `build_palette`: resolve every
/// scope in [`SCOPES`] by trying `lookup` on the scope itself and then each
/// dotted ancestor, accumulating font flags and taking the first colour
/// found.
fn resolve_scopes(lookup: impl Fn(&str) -> Option<ScopeStyle>) -> Palette {
    let styles = (0..SCOPES.len())
        .map(|index| {
            let mut resolved = ScopeStyle::default();
            let mut current = Scope::resolve(SCOPES[index]);
            while let Some(scope) = current {
                if let Some(style) = lookup(scope.name()) {
                    resolved.bold |= style.bold;
                    resolved.italic |= style.italic;
                    resolved.underline |= style.underline;
                    if resolved.fg.is_none() {
                        resolved.fg = style.fg;
                    }
                }
                current = parent(scope);
            }
            resolved
        })
        .collect();

    Palette { styles }
}

fn entry(entries: &[(&str, ScopeStyle)], name: &str) -> Option<ScopeStyle> {
    entries
        .iter()
        .find(|(scope, _)| *scope == name)
        .map(|(_, style)| *style)
}

/// The next scope up the dotted hierarchy, reusing the same walk capture
/// names go through.
fn parent(scope: Scope) -> Option<Scope> {
    Scope::resolve(scope.name().rsplit_once('.')?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(name: &str) -> Scope {
        Scope::resolve(name).expect("known scope")
    }

    fn style_of(palette: &Palette, name: &str) -> ScopeStyle {
        palette.style(scope(name))
    }

    fn user_base(pairs: &[(&str, ScopeStyle)]) -> UserStyles {
        UserStyles {
            base: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            by_language: HashMap::new(),
        }
    }

    const RED: Rgb = Rgb::new(0xff, 0x00, 0x00);
    const BLUE: Rgb = Rgb::new(0x00, 0x00, 0xff);

    #[test]
    fn theme_entry_applies() {
        let palette = palette("dark", "rust", &UserStyles::default());
        assert_eq!(
            style_of(&palette, "keyword").fg,
            Some(Rgb::new(0xcc, 0x78, 0x32))
        );
        let vs = super::palette("vscode-dark", "rust", &UserStyles::default());
        assert_eq!(
            style_of(&vs, "keyword").fg,
            Some(Rgb::new(0x56, 0x9c, 0xd6))
        );
    }

    #[test]
    fn light_theme_does_not_reuse_the_dark_palette() {
        let light = palette("light", "rust", &UserStyles::default());
        let dark = super::palette("dark", "rust", &UserStyles::default());
        assert_ne!(light, dark);
        // Darcula's mid-grey comment is the worst offender on white.
        assert_ne!(
            style_of(&light, "comment").fg,
            style_of(&dark, "comment").fg
        );
    }

    #[test]
    fn unknown_theme_falls_back_to_first() {
        let unknown = palette("no-such-theme", "rust", &UserStyles::default());
        assert_eq!(
            unknown,
            super::palette("dark", "rust", &UserStyles::default())
        );
    }

    #[test]
    fn base_override_beats_theme() {
        let user = user_base(&[("keyword", ScopeStyle::fg(RED))]);
        let palette = palette("dark", "rust", &user);
        assert_eq!(style_of(&palette, "keyword").fg, Some(RED));
    }

    #[test]
    fn language_override_beats_base_override() {
        let mut user = user_base(&[("keyword", ScopeStyle::fg(RED))]);
        user.by_language.insert(
            "rust".to_string(),
            [("keyword".to_string(), ScopeStyle::fg(BLUE))]
                .into_iter()
                .collect(),
        );
        let rust = palette("dark", "rust", &user);
        assert_eq!(style_of(&rust, "keyword").fg, Some(BLUE));
        // A different language still sees the base override.
        let json = super::palette("dark", "json", &user);
        assert_eq!(style_of(&json, "keyword").fg, Some(RED));
    }

    #[test]
    fn parent_scope_is_inherited() {
        let user = user_base(&[("function", ScopeStyle::fg(RED))]);
        let palette = palette("dark", "rust", &user);
        assert_eq!(style_of(&palette, "function.method").fg, Some(RED));
        // Two levels up as well: string.special has no entry of its own.
        assert_eq!(
            style_of(&palette, "string.special").fg,
            style_of(&palette, "string").fg
        );
    }

    #[test]
    fn flags_only_style_inherits_parent_color() {
        let user = user_base(&[
            (
                "function",
                ScopeStyle {
                    bold: true,
                    ..ScopeStyle::fg(RED)
                },
            ),
            (
                "function.method",
                ScopeStyle {
                    italic: true,
                    ..ScopeStyle::default()
                },
            ),
        ]);
        let palette = palette("dark", "rust", &user);
        let method = style_of(&palette, "function.method");
        assert_eq!(method.fg, Some(RED));
        assert!(method.italic && method.bold);
    }

    #[test]
    fn unknown_scope_name_is_ignored() {
        let user = user_base(&[("no.such.scope", ScopeStyle::fg(RED))]);
        let palette = palette("dark", "rust", &user);
        assert_eq!(
            palette,
            super::palette("dark", "rust", &UserStyles::default())
        );
    }

    #[test]
    fn every_theme_resolves_every_scope() {
        for theme in BUILTIN_THEMES {
            let palette = palette(theme.name, "rust", &UserStyles::default());
            assert_eq!(palette.styles().len(), SCOPES.len());
            // Every scope with a themed ancestor must end up coloured.
            assert!(style_of(&palette, "function.method").fg.is_some());
            assert!(style_of(&palette, "comment.documentation").fg.is_some());
        }
    }

    /// The editor's default text foreground per theme, mirroring
    /// `colorsForTheme` in `crates/ui-shell/cpp/theme.cpp`. A scope painted
    /// in this colour is indistinguishable from unhighlighted text, which is
    /// the defect issue #31 reported.
    fn default_foreground(theme: &str) -> Rgb {
        match theme {
            "light" => Rgb::new(0x1a, 0x1a, 0x1a),
            "vscode-dark" => Rgb::new(0xcc, 0xcc, 0xcc),
            _ => Rgb::new(0xa9, 0xb7, 0xc6),
        }
    }

    /// Scopes a theme leaves in the editor's default foreground **on
    /// purpose**. Every other scope in `SCOPES` must resolve to a different
    /// colour, so growing the taxonomy cannot silently outrun the palettes
    /// again: a new scope fails this test until someone either themes it or
    /// adds it here with a reason.
    fn unstyled_by_design(theme: &str) -> Vec<&'static str> {
        // True of all three palettes.
        let mut scopes = vec![
            // Operators and punctuation are unstyled in Darcula, IntelliJ
            // Light and Dark+ alike — colouring every brace and comma is a
            // deliberate non-goal, not a missing entry.
            "operator",
            "punctuation",
            "punctuation.bracket",
            "punctuation.delimiter",
            "punctuation.special",
            // The root of an injected region. Painting it would tint the
            // whole injected block instead of letting the injected
            // language's own scopes show through.
            "embedded",
            // Prose body text in a markup document. Every `markup.*` leaf is
            // themed; the bare root is the surrounding paragraph.
            "markup",
            // Emphasis is a shape, not a colour (see `PLAIN`): these carry
            // bold/italic and inherit the prose colour.
            "markup.bold",
            "markup.italic",
            // Dark+ is the only one of the three with a label colour, and
            // its `entity.name.label` `#c8c8c8` is the default foreground in
            // all but the last digit. Not worth claiming as a colour.
            "label",
        ];
        if theme != "vscode-dark" {
            // Both IntelliJ palettes paint locals, parameters and package
            // names in plain text; only Dark+ gives them a colour.
            scopes.extend(["variable", "variable.parameter", "module"]);
        }
        if theme == "dark" {
            // Darcula genuinely paints class references in the default
            // foreground — it separates a type from an identifier by
            // context, not hue. Kept faithful to the palette, but it is the
            // one entry here that is worth revisiting: `type.builtin`,
            // `constructor` and `constant` all got colours above, so `type`
            // is now the odd one out in its own family.
            scopes.extend(["type", "type.definition"]);
        }
        scopes
    }

    #[test]
    fn every_theme_colours_every_scope_not_unstyled_by_design() {
        for theme in BUILTIN_THEMES {
            let palette = palette(theme.name, "rust", &UserStyles::default());
            let default = default_foreground(theme.name);
            let allowed = unstyled_by_design(theme.name);
            for name in SCOPES {
                let fg = style_of(&palette, name).fg;
                let indistinguishable = fg.is_none() || fg == Some(default);
                assert_eq!(
                    indistinguishable,
                    allowed.contains(name),
                    "theme `{}`: `{name}` renders in the default foreground: {}",
                    theme.name,
                    if indistinguishable {
                        "give it a colour, or add it to `unstyled_by_design` with a reason"
                    } else {
                        "it is coloured now — drop it from `unstyled_by_design`"
                    },
                );
            }
        }
    }

    /// The defect this table was added for: a `SCREAMING_CASE` constant or a
    /// `CamelCase` constructor must not render identically to a plain
    /// identifier, which is what `variable`'s (possibly absent) colour is.
    #[test]
    fn constants_and_constructors_are_not_plain_identifiers() {
        for theme in BUILTIN_THEMES {
            let palette = palette(theme.name, "rust", &UserStyles::default());
            let plain = style_of(&palette, "variable").fg;
            for name in ["constant", "constructor"] {
                assert_ne!(
                    style_of(&palette, name).fg,
                    plain,
                    "theme `{}` paints `{name}` like a plain identifier",
                    theme.name,
                );
            }
        }
    }

    /// Every `light` entry is claimed to clear WCAG AA 4.5:1 on `#ffffff`
    /// in the table's comments; this checks the claim instead of trusting it.
    #[test]
    fn light_theme_clears_wcag_aa_on_white() {
        fn channel(value: u8) -> f64 {
            let c = f64::from(value) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        for (name, style) in LIGHT {
            let Some(fg) = style.fg else { continue };
            let luminance =
                0.2126 * channel(fg.r) + 0.7152 * channel(fg.g) + 0.0722 * channel(fg.b);
            let ratio = 1.05 / (luminance + 0.05);
            assert!(ratio >= 4.5, "light `{name}` is only {ratio:.2}:1 on white");
        }
    }

    fn vscode_dark_styles() -> ThemeStyles {
        ThemeStyles {
            base: VSCODE_DARK
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            by_language: HashMap::new(),
        }
    }

    /// `build_palette` given the `ThemeStyles` equivalent of `vscode-dark`
    /// must resolve every scope identically to the old name-based `palette`
    /// — this is a parameter-source swap, not a behaviour change.
    #[test]
    fn build_palette_matches_name_based_palette() {
        let by_name = palette("vscode-dark", "rust", &UserStyles::default());
        let by_styles = build_palette(&vscode_dark_styles(), "rust", &UserStyles::default());
        for name in [
            "keyword",
            "string",
            "comment",
            "markup.heading",
            // Relies on parent-scope inheritance: neither has its own entry.
            "function.method",
            "string.special",
        ] {
            assert_eq!(
                style_of(&by_name, name),
                style_of(&by_styles, name),
                "scope `{name}` differs between palette and build_palette"
            );
        }
    }

    #[test]
    fn rgb_parses_hex() {
        assert_eq!(Rgb::parse("#cc7832"), Some(Rgb::new(0xcc, 0x78, 0x32)));
        assert_eq!(Rgb::parse("cc7832"), Some(Rgb::new(0xcc, 0x78, 0x32)));
        assert_eq!(Rgb::parse("#ccc"), None);
        assert_eq!(Rgb::parse("#gg7832"), None);
    }
}
