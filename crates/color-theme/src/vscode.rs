//! VS Code theme JSON import: the workbench `colors` map plus `tokenColors`
//! TextMate rules, mapped onto [`ColorTheme`](crate::ColorTheme).
//!
//! Every mapping choice below is best-effort (per the color-themes plan):
//! the nine `github-vscode-theme` (Primer) variants this exists for do not
//! all set the same optional keys, so every role has a fallback chain and a
//! hardcoded last resort — nothing here fails a whole file over one missing
//! key. Only structurally broken input (bad JSON, no `colors` object, or an
//! unparsable hex string that *is* present) is an error.

use std::collections::HashMap;

use serde::Deserialize;

use crate::{
    Appearance, ChromeColors, ColorTheme, DiffColors, Rgba, ScopeStyle, SemanticColors,
    TerminalColors, ThemeError,
};

/// The 56 `syntax-core` scope names this crate must not import
/// `syntax-core` to get (per the plan: scope names stay plain strings here,
/// never on `syntax-core`) — copied verbatim from `syntax_core::SCOPES`
/// (`crates/syntax-core/src/lib.rs:38`). Keep in sync by hand; a mismatch
/// only means a scope import silently has no effect, since `syntax-core`'s
/// own inheritance covers anything missing from `ColorTheme::syntax`.
const SCOPES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.call",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "markup",
    "markup.bold",
    "markup.heading",
    "markup.heading.1",
    "markup.heading.2",
    "markup.heading.3",
    "markup.heading.4",
    "markup.heading.5",
    "markup.heading.6",
    "markup.italic",
    "markup.link",
    "markup.link.label",
    "markup.link.url",
    "markup.list",
    "markup.quote",
    "markup.raw",
    "markup.raw.block",
    "markup.strikethrough",
    "module",
    "number",
    "number.float",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regexp",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "type.definition",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
];

/// For each `SCOPES` entry, the TextMate scope selectors to try, most
/// specific first — standard scope names typical of VS Code TextMate
/// grammars. Best-effort: not every one of the 56 needs a hit, and several
/// (`embedded`, `markup`) have no reliable TextMate counterpart at all.
fn candidates_for(scope: &str) -> &'static [&'static str] {
    match scope {
        "attribute" => &["entity.other.attribute-name", "meta.attribute"],
        "boolean" => &["constant.language.boolean", "constant.language"],
        "character" => &["constant.character", "string.quoted.single"],
        "comment" => &["comment"],
        "comment.documentation" => &[
            "comment.block.documentation",
            "comment.documentation",
            "comment",
        ],
        "constant" => &["variable.other.constant", "constant"],
        "constant.builtin" => &["constant.language", "support.constant"],
        "constructor" => &[
            "entity.name.function.constructor",
            "entity.name.class",
            "support.class",
        ],
        "embedded" => &["meta.embedded"],
        "escape" => &["constant.character.escape"],
        "function" => &["entity.name.function"],
        "function.builtin" => &["support.function.builtin", "support.function"],
        "function.call" => &["entity.name.function", "support.function"],
        "function.macro" => &[
            "entity.name.function.macro",
            "support.function.macro",
            "entity.name.function",
        ],
        "function.method" => &[
            "entity.name.function.member",
            "meta.function.method",
            "entity.name.function",
        ],
        "keyword" => &["keyword.control", "keyword"],
        "label" => &["entity.name.label"],
        "markup" => &["markup"],
        "markup.bold" => &["markup.bold"],
        "markup.heading" => &["markup.heading"],
        "markup.heading.1" => &["markup.heading.1.markdown", "markup.heading"],
        "markup.heading.2" => &["markup.heading.2.markdown", "markup.heading"],
        "markup.heading.3" => &["markup.heading.3.markdown", "markup.heading"],
        "markup.heading.4" => &["markup.heading.4.markdown", "markup.heading"],
        "markup.heading.5" => &["markup.heading.5.markdown", "markup.heading"],
        "markup.heading.6" => &["markup.heading.6.markdown", "markup.heading"],
        "markup.italic" => &["markup.italic"],
        "markup.link" => &["markup.underline.link", "string.other.link"],
        "markup.link.label" => &["string.other.link.description", "markup.link"],
        "markup.link.url" => &["markup.underline.link", "string.other.link"],
        "markup.list" => &["markup.list", "punctuation.definition.list"],
        "markup.quote" => &["markup.quote"],
        "markup.raw" => &["markup.inline.raw", "markup.raw"],
        "markup.raw.block" => &["markup.fenced_code.block", "markup.raw.block", "markup.raw"],
        "markup.strikethrough" => &["markup.strikethrough"],
        "module" => &["entity.name.namespace", "entity.name.module"],
        "number" => &["constant.numeric"],
        "number.float" => &["constant.numeric.float", "constant.numeric"],
        "operator" => &["keyword.operator"],
        "property" => &[
            "variable.other.property",
            "meta.object-literal.key",
            "support.type.property-name",
        ],
        "punctuation" => &["punctuation"],
        "punctuation.bracket" => &[
            "punctuation.definition.begin",
            "punctuation.section.brackets",
            "punctuation",
        ],
        "punctuation.delimiter" => &[
            "punctuation.separator",
            "punctuation.terminator",
            "punctuation",
        ],
        "punctuation.special" => &["punctuation.special", "punctuation"],
        "string" => &["string"],
        "string.escape" => &["constant.character.escape"],
        "string.regexp" => &["string.regexp"],
        "string.special" => &["string.other", "string.special"],
        "tag" => &["entity.name.tag"],
        "type" => &["entity.name.type", "support.type"],
        "type.builtin" => &["support.type.builtin", "storage.type"],
        "type.definition" => &["entity.name.type.class", "entity.name.class"],
        "variable" => &["variable"],
        "variable.builtin" => &["variable.language"],
        "variable.member" => &["variable.other.member", "variable.other.property"],
        "variable.parameter" => &["variable.parameter"],
        _ => &[],
    }
}

// --- JSON shape ----------------------------------------------------------

#[derive(Deserialize)]
struct RawVsCodeTheme {
    name: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    colors: Option<HashMap<String, String>>,
    #[serde(rename = "tokenColors", default)]
    token_colors: Vec<RawTokenColor>,
}

#[derive(Deserialize)]
struct RawTokenColor {
    #[serde(default)]
    scope: Option<RawScope>,
    #[serde(default)]
    settings: RawTokenSettings,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawScope {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize, Default)]
struct RawTokenSettings {
    foreground: Option<String>,
    #[serde(rename = "fontStyle")]
    font_style: Option<String>,
}

/// One `tokenColors` entry, normalized: `scope` as a flat list of selectors
/// (an entry with no `scope` field at all becomes an empty list — the
/// catch-all every `resolve_scope` request can fall back to).
pub struct ParsedTokenColor {
    pub scopes: Vec<String>,
    pub foreground: Option<Rgba>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl From<RawTokenColor> for ParsedTokenColor {
    fn from(raw: RawTokenColor) -> Self {
        let scopes = match raw.scope {
            None => Vec::new(),
            Some(RawScope::One(s)) => s.split(',').map(|part| part.trim().to_string()).collect(),
            Some(RawScope::Many(list)) => list,
        };
        let font_style = raw.settings.font_style.unwrap_or_default();
        Self {
            scopes,
            foreground: raw
                .settings
                .foreground
                .as_deref()
                .and_then(|hex| Rgba::parse(hex).ok()),
            bold: font_style.contains("bold"),
            italic: font_style.contains("italic"),
            underline: font_style.contains("underline"),
        }
    }
}

/// Resolves `want` (e.g. `"keyword.control.rust"`) against `token_colors`
/// using VS Code's own specificity rule: the entry whose scope selector is
/// the longest dotted prefix of `want` wins (an exact match is simply the
/// longest possible prefix, so it wins under the same rule without special
/// casing). An entry with no `scope` at all matches everything but loses to
/// any entry that names a real prefix. `None` if nothing matches at all.
pub fn resolve_scope<'a>(
    token_colors: &'a [ParsedTokenColor],
    want: &str,
) -> Option<&'a ParsedTokenColor> {
    let mut best: Option<(&ParsedTokenColor, i32)> = None;
    for token_color in token_colors {
        if token_color.scopes.is_empty() {
            if best.is_none() {
                best = Some((token_color, -1));
            }
            continue;
        }
        for selector in &token_color.scopes {
            let matches = want == selector
                || (want.starts_with(selector.as_str())
                    && want.as_bytes().get(selector.len()) == Some(&b'.'));
            if !matches {
                continue;
            }
            let score = selector.len() as i32;
            if best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((token_color, score));
            }
        }
    }
    best.map(|(token_color, _)| token_color)
}

// --- workbench-key mapping, with fallback chains --------------------------

fn first_present<'a>(colors: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| colors.get(*key))
        .map(String::as_str)
}

/// Resolves a role from the first present key in `keys`, falling back to
/// `default` (a hardcoded, reasonable colour) when none of them are set —
/// an import must never fail merely because one workbench key is absent.
fn resolve_or(colors: &HashMap<String, String>, keys: &[&str], default: Rgba) -> Rgba {
    first_present(colors, keys)
        .and_then(|hex| Rgba::parse(hex).ok())
        .unwrap_or(default)
}

fn map_chrome(colors: &HashMap<String, String>) -> ChromeColors {
    let canvas = resolve_or(
        colors,
        &["editor.background"],
        Rgba::new(0x1e, 0x1e, 0x1e, 255),
    );
    let surface = resolve_or(
        colors,
        &["titleBar.activeBackground", "editor.background"],
        canvas,
    );
    let surface2 = resolve_or(colors, &["sideBar.background"], surface);
    let raised_top = resolve_or(
        colors,
        &["list.hoverBackground"],
        Rgba::new(255, 255, 255, 20),
    );
    let selection_top = resolve_or(
        colors,
        &["list.activeSelectionBackground"],
        Rgba::new(0x2f, 0x62, 0xff, 90),
    );
    ChromeColors {
        canvas,
        surface,
        surface2,
        raised: raised_top.over(surface2),
        // Best-effort: no single workbench key maps to a hairline border
        // colour used everywhere; `panel.border` is the closest reliable one.
        border: resolve_or(colors, &["panel.border"], Rgba::new(0x3a, 0x3a, 0x3a, 255)),
        text: resolve_or(
            colors,
            &["editor.foreground"],
            Rgba::new(0xcc, 0xcc, 0xcc, 255),
        ),
        text_dim: resolve_or(
            colors,
            &["descriptionForeground"],
            Rgba::new(0x8a, 0x8f, 0x98, 255),
        ),
        accent: resolve_or(colors, &["focusBorder"], Rgba::new(0x35, 0x74, 0xf0, 255)),
        // Best-effort: `button.foreground` is the only workbench key that
        // reliably pairs with an accent-filled control's background.
        accent_ink: resolve_or(
            colors,
            &["button.foreground"],
            Rgba::new(255, 255, 255, 255),
        ),
        selection: selection_top.over(surface2),
        status_bar: resolve_or(colors, &["statusBar.background"], surface),
    }
}

fn map_semantic(colors: &HashMap<String, String>) -> SemanticColors {
    SemanticColors {
        // Present only in the two classic Primer variants, per the plan —
        // every other variant falls back to the VS Code default red.
        error: resolve_or(
            colors,
            &["editorError.foreground"],
            Rgba::new(0xf1, 0x4c, 0x4c, 255),
        ),
        warning: resolve_or(
            colors,
            &["editorWarning.foreground"],
            Rgba::new(0xcc, 0xa7, 0x00, 255),
        ),
        info: resolve_or(
            colors,
            &["editorInfo.foreground"],
            Rgba::new(0x37, 0x94, 0xff, 255),
        ),
        ok: resolve_or(
            colors,
            &[
                "terminal.ansiGreen",
                "gitDecoration.addedResourceForeground",
            ],
            Rgba::new(0x89, 0xd1, 0x85, 255),
        ),
        muted: resolve_or(
            colors,
            &["descriptionForeground"],
            Rgba::new(0x8a, 0x8a, 0x8a, 255),
        ),
    }
}

fn map_diff(colors: &HashMap<String, String>, canvas: Rgba) -> DiffColors {
    let added_line_top = resolve_or(
        colors,
        &[
            "diffEditor.insertedLineBackground",
            "diffEditor.insertedTextBackground",
        ],
        Rgba::new(0x4c, 0xaf, 0x50, 40),
    );
    let added_inline_top = resolve_or(
        colors,
        &["diffEditor.insertedTextBackground"],
        Rgba::new(0x4c, 0xaf, 0x50, 90),
    );
    let deleted_line_top = resolve_or(
        colors,
        &[
            "diffEditor.removedLineBackground",
            "diffEditor.removedTextBackground",
        ],
        Rgba::new(0xf4, 0x43, 0x36, 40),
    );
    let deleted_inline_top = resolve_or(
        colors,
        &["diffEditor.removedTextBackground"],
        Rgba::new(0xf4, 0x43, 0x36, 90),
    );
    DiffColors {
        added_line: added_line_top.over(canvas),
        added_inline: added_inline_top.over(canvas),
        added_marker: resolve_or(
            colors,
            &["gitDecoration.addedResourceForeground"],
            Rgba::new(0x4c, 0xaf, 0x50, 255),
        ),
        // VS Code's diff editor has no dedicated "modified" background — a
        // changed line renders as a deletion/insertion pair — so this and
        // `modified_inline` are hand-picked blue tints rather than mapped.
        modified_line: Rgba::new(0x1e, 0x2f, 0x4a, 255),
        modified_inline: Rgba::new(0x2d, 0x4a, 0x70, 255),
        modified_marker: resolve_or(
            colors,
            &["gitDecoration.modifiedResourceForeground"],
            Rgba::new(0x35, 0x74, 0xf0, 255),
        ),
        deleted_line: deleted_line_top.over(canvas),
        deleted_inline: deleted_inline_top.over(canvas),
        deleted_marker: resolve_or(
            colors,
            &["gitDecoration.deletedResourceForeground"],
            Rgba::new(0xf4, 0x43, 0x36, 255),
        ),
    }
}

fn map_terminal(colors: &HashMap<String, String>, chrome: &ChromeColors) -> TerminalColors {
    // The 16 ANSI keys plus background/foreground are reliable across all
    // nine Primer variants; cursor/selection are not, so they fall back to
    // the chrome roles a terminal already borrows for those two states.
    let ansi = |key: &str, default: Rgba| resolve_or(colors, &[key], default);
    TerminalColors {
        black: ansi("terminal.ansiBlack", Rgba::new(0, 0, 0, 255)),
        red: ansi("terminal.ansiRed", Rgba::new(0xcd, 0x31, 0x31, 255)),
        green: ansi("terminal.ansiGreen", Rgba::new(0x0d, 0xbc, 0x79, 255)),
        yellow: ansi("terminal.ansiYellow", Rgba::new(0xe5, 0xe5, 0x10, 255)),
        blue: ansi("terminal.ansiBlue", Rgba::new(0x24, 0x72, 0xc8, 255)),
        magenta: ansi("terminal.ansiMagenta", Rgba::new(0xbc, 0x3f, 0xbc, 255)),
        cyan: ansi("terminal.ansiCyan", Rgba::new(0x11, 0xa8, 0xcd, 255)),
        white: ansi("terminal.ansiWhite", Rgba::new(0xe5, 0xe5, 0xe5, 255)),
        bright_black: ansi("terminal.ansiBrightBlack", Rgba::new(0x66, 0x66, 0x66, 255)),
        bright_red: ansi("terminal.ansiBrightRed", Rgba::new(0xf1, 0x4c, 0x4c, 255)),
        bright_green: ansi("terminal.ansiBrightGreen", Rgba::new(0x23, 0xd1, 0x8b, 255)),
        bright_yellow: ansi(
            "terminal.ansiBrightYellow",
            Rgba::new(0xf5, 0xf5, 0x43, 255),
        ),
        bright_blue: ansi("terminal.ansiBrightBlue", Rgba::new(0x3b, 0x8e, 0xea, 255)),
        bright_magenta: ansi(
            "terminal.ansiBrightMagenta",
            Rgba::new(0xd6, 0x70, 0xd6, 255),
        ),
        bright_cyan: ansi("terminal.ansiBrightCyan", Rgba::new(0x29, 0xb8, 0xdb, 255)),
        bright_white: ansi("terminal.ansiBrightWhite", Rgba::new(0xe5, 0xe5, 0xe5, 255)),
        background: resolve_or(colors, &["terminal.background"], chrome.canvas),
        foreground: resolve_or(colors, &["terminal.foreground"], chrome.text),
        cursor: resolve_or(colors, &["terminalCursor.foreground"], chrome.text),
        selection: resolve_or(colors, &["terminal.selectionBackground"], chrome.selection),
    }
}

fn map_syntax(token_colors: &[ParsedTokenColor]) -> HashMap<String, ScopeStyle> {
    let mut syntax = HashMap::new();
    for &scope in SCOPES {
        let Some(hit) = candidates_for(scope)
            .iter()
            .find_map(|candidate| resolve_scope(token_colors, candidate))
        else {
            continue;
        };
        let Some(fg) = hit.foreground else { continue };
        syntax.insert(
            scope.to_string(),
            ScopeStyle {
                fg,
                bold: hit.bold,
                italic: hit.italic,
                underline: hit.underline,
            },
        );
    }
    syntax
}

fn slugify(name: &str) -> String {
    let mut chars: Vec<char> = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    chars.dedup_by(|a, b| *a == '-' && *b == '-');
    chars
        .into_iter()
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Parses a VS Code theme JSON file: top-level `name`, `colors` (the ~241
/// workbench-key map), and `tokenColors` (TextMate rules). `id` is derived
/// from `name` since VS Code theme files carry no id of their own.
pub fn parse_vscode_json(input: &str) -> Result<ColorTheme, ThemeError> {
    let raw: RawVsCodeTheme = serde_json::from_str(input).map_err(|e| ThemeError::Parse {
        message: e.to_string(),
    })?;
    let colors = raw.colors.ok_or_else(|| ThemeError::MissingField {
        key: "colors".to_string(),
    })?;

    let name = raw.name.unwrap_or_else(|| "Imported Theme".to_string());
    let id = {
        let slug = slugify(&name);
        if slug.is_empty() {
            "imported-theme".to_string()
        } else {
            slug
        }
    };
    let appearance = match raw.kind.as_deref() {
        Some("light") => Appearance::Light,
        _ => Appearance::Dark,
    };

    let chrome = map_chrome(&colors);
    let semantic = map_semantic(&colors);
    let diff = map_diff(&colors, chrome.canvas);
    let terminal = map_terminal(&colors, &chrome);
    let token_colors: Vec<ParsedTokenColor> = raw
        .token_colors
        .into_iter()
        .map(ParsedTokenColor::from)
        .collect();
    let syntax = map_syntax(&token_colors);

    Ok(ColorTheme {
        id,
        label: name,
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

    fn fixture() -> &'static str {
        r##"{
            "name": "Fixture Dark",
            "type": "dark",
            "colors": {
                "editor.background": "#0d1117",
                "editor.foreground": "#c9d1d9",
                "sideBar.background": "#010409",
                "panel.border": "#30363d",
                "focusBorder": "#1f6feb",
                "button.foreground": "#ffffff",
                "descriptionForeground": "#8b949e",
                "statusBar.background": "#0d1117",
                "list.hoverBackground": "#6e768166",
                "list.activeSelectionBackground": "#1f6feb44",
                "editorError.foreground": "#f85149",
                "terminal.ansiBlack": "#484f58",
                "terminal.ansiRed": "#ff7b72"
            },
            "tokenColors": [
                {
                    "settings": { "foreground": "#c9d1d9" }
                },
                {
                    "scope": "keyword",
                    "settings": { "foreground": "#ff7b72" }
                },
                {
                    "scope": ["keyword.control", "keyword.operator"],
                    "settings": { "foreground": "#ff7b72", "fontStyle": "bold" }
                },
                {
                    "scope": "keyword.control.rust",
                    "settings": { "foreground": "#ffa198", "fontStyle": "italic underline" }
                },
                {
                    "scope": "comment",
                    "settings": { "foreground": "#8b949e", "fontStyle": "italic" }
                }
            ]
        }"##
    }

    #[test]
    fn maps_chrome_semantic_diff_terminal_with_present_keys() {
        let theme = parse_vscode_json(fixture()).expect("fixture parses");
        assert_eq!(theme.label, "Fixture Dark");
        assert_eq!(theme.id, "fixture-dark");
        assert_eq!(theme.appearance, Appearance::Dark);
        assert_eq!(theme.chrome.canvas, Rgba::new(0x0d, 0x11, 0x17, 255));
        assert_eq!(theme.chrome.text, Rgba::new(0xc9, 0xd1, 0xd9, 255));
        assert_eq!(theme.semantic.error, Rgba::new(0xf8, 0x51, 0x49, 255));
        assert_eq!(theme.terminal.ansi()[0], Rgba::new(0x48, 0x4f, 0x58, 255));
        assert_eq!(theme.terminal.ansi()[1], Rgba::new(0xff, 0x7b, 0x72, 255));
    }

    #[test]
    fn falls_back_when_an_optional_key_is_absent() {
        let theme = parse_vscode_json(fixture()).expect("fixture parses");
        // No "editorWarning.foreground"/"editorInfo.foreground" in the
        // fixture: the hardcoded defaults must be used, not an error.
        assert_eq!(theme.semantic.warning, Rgba::new(0xcc, 0xa7, 0x00, 255));
        assert_eq!(theme.semantic.info, Rgba::new(0x37, 0x94, 0xff, 255));
        // "surface" has no titleBar key in the fixture: falls back to
        // editor.background.
        assert_eq!(theme.chrome.surface, theme.chrome.canvas);
    }

    #[test]
    fn resolve_scope_prefers_exact_over_prefix_and_longest_over_shorter() {
        let token_colors: Vec<ParsedTokenColor> = {
            let raw: RawVsCodeTheme = serde_json::from_str(fixture()).unwrap();
            raw.token_colors
                .into_iter()
                .map(ParsedTokenColor::from)
                .collect()
        };

        // Longest matching prefix wins: "keyword.control" beats "keyword".
        let hit = resolve_scope(&token_colors, "keyword.control.something-unlisted").unwrap();
        assert_eq!(hit.foreground, Some(Rgba::new(0xff, 0x7b, 0x72, 255)));
        assert!(hit.bold);

        // Exact match ("keyword.control.rust") beats the shorter prefix
        // ("keyword.control") even though both match.
        let exact = resolve_scope(&token_colors, "keyword.control.rust").unwrap();
        assert_eq!(exact.foreground, Some(Rgba::new(0xff, 0xa1, 0x98, 255)));
        assert!(exact.italic && exact.underline);

        // No match at all beyond the unscoped catch-all.
        let fallback = resolve_scope(&token_colors, "storage.modifier").unwrap();
        assert_eq!(fallback.foreground, Some(Rgba::new(0xc9, 0xd1, 0xd9, 255)));

        assert!(resolve_scope(&[], "anything").is_none());
    }

    #[test]
    fn scope_resolver_picks_syntax_scopes_from_token_colors() {
        let theme = parse_vscode_json(fixture()).expect("fixture parses");
        let keyword = theme.syntax.get("keyword").expect("keyword resolved");
        // candidates_for("keyword") tries "keyword.control" before
        // "keyword"; the bold "keyword.control"/"keyword.operator" rule
        // must win over the plain "keyword" rule.
        assert_eq!(keyword.fg, Rgba::new(0xff, 0x7b, 0x72, 255));
        assert!(keyword.bold);

        let comment = theme.syntax.get("comment").expect("comment resolved");
        assert_eq!(comment.fg, Rgba::new(0x8b, 0x94, 0x9e, 255));
        assert!(comment.italic);
    }

    #[test]
    fn missing_colors_object_is_an_error() {
        let err = parse_vscode_json(r#"{"name":"No Colors"}"#).unwrap_err();
        assert_eq!(
            err,
            ThemeError::MissingField {
                key: "colors".to_string()
            }
        );
    }
}
