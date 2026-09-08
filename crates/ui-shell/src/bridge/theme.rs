//! Rust side of the `ThemeProvider` QObject: the colour-theme seam (T7).
//!
//! `theme.cpp`'s `applyTheme()` asks this once per theme switch and caches
//! every answer — the QObject itself decides nothing beyond "which theme is
//! active", the same shape [`crate::bridge::icons::IconProviderRust`]
//! already gives icons.

use std::cell::RefCell;
use std::rc::Rc;

use app_core::color_themes::ColorThemeService;
use color_theme::{ChromeColors, ColorTheme, DiffColors, Rgba, SemanticColors, TerminalColors};
use cxx_qt_lib::QString;

use crate::bridge::ffi;
use crate::bridge::registry::shared_color_themes;

/// A handle on the process-wide colour-theme service, nothing more.
pub struct ThemeProviderRust {
    themes: Rc<RefCell<ColorThemeService>>,
}

impl Default for ThemeProviderRust {
    fn default() -> Self {
        Self {
            themes: shared_color_themes(),
        }
    }
}

fn to_ffi_rgb(color: Rgba) -> ffi::FfiRgb {
    ffi::FfiRgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn chrome_palette(c: &ChromeColors) -> ffi::FfiChromePalette {
    ffi::FfiChromePalette {
        canvas: to_ffi_rgb(c.canvas),
        surface: to_ffi_rgb(c.surface),
        surface2: to_ffi_rgb(c.surface2),
        raised: to_ffi_rgb(c.raised),
        border: to_ffi_rgb(c.border),
        text: to_ffi_rgb(c.text),
        text_dim: to_ffi_rgb(c.text_dim),
        accent: to_ffi_rgb(c.accent),
        accent_ink: to_ffi_rgb(c.accent_ink),
        selection: to_ffi_rgb(c.selection),
        status_bar: to_ffi_rgb(c.status_bar),
    }
}

fn semantic_colors(s: &SemanticColors) -> ffi::FfiSemanticColors {
    ffi::FfiSemanticColors {
        error: to_ffi_rgb(s.error),
        warning: to_ffi_rgb(s.warning),
        info: to_ffi_rgb(s.info),
        ok: to_ffi_rgb(s.ok),
        muted: to_ffi_rgb(s.muted),
    }
}

fn diff_colors(d: &DiffColors) -> ffi::FfiDiffColors {
    ffi::FfiDiffColors {
        added_line: to_ffi_rgb(d.added_line),
        added_inline: to_ffi_rgb(d.added_inline),
        added_marker: to_ffi_rgb(d.added_marker),
        modified_line: to_ffi_rgb(d.modified_line),
        modified_inline: to_ffi_rgb(d.modified_inline),
        modified_marker: to_ffi_rgb(d.modified_marker),
        deleted_line: to_ffi_rgb(d.deleted_line),
        deleted_inline: to_ffi_rgb(d.deleted_inline),
        deleted_marker: to_ffi_rgb(d.deleted_marker),
    }
}

fn terminal_palette(t: &TerminalColors) -> ffi::FfiTerminalPalette {
    ffi::FfiTerminalPalette {
        background: to_ffi_rgb(t.background),
        foreground: to_ffi_rgb(t.foreground),
        cursor: to_ffi_rgb(t.cursor),
        selection: to_ffi_rgb(t.selection),
        ansi: t.ansi().into_iter().map(to_ffi_rgb).collect(),
    }
}

impl ffi::ThemeProvider {
    /// Every colour theme the loaded plugins offer, for the Appearance
    /// page's combo.
    pub fn color_themes(&self) -> Vec<ffi::FfiColorThemeChoice> {
        app_core::color_themes::color_themes(&plugin_host::registry())
            .into_iter()
            .map(|choice| ffi::FfiColorThemeChoice {
                id: QString::from(choice.id.as_str()),
                label: QString::from(choice.label.as_str()),
            })
            .collect()
    }

    /// Switch the active colour theme without persisting anything — the
    /// Appearance page's live preview, and its Cancel path.
    pub fn apply_color_theme(&self, id: &QString) {
        self.themes.borrow_mut().set_preferred(&id.to_string());
    }

    /// Whether the active theme is dark — the one bit `theme.cpp` needs
    /// beyond the colours themselves (chevron glyph, shadow ink).
    pub fn is_dark(&self) -> bool {
        with_active(&self.themes, |theme| theme.appearance)
            .map(|appearance| appearance == color_theme::Appearance::Dark)
            .unwrap_or(true)
    }

    pub fn chrome_palette(&self) -> ffi::FfiChromePalette {
        with_active(&self.themes, |theme| chrome_palette(&theme.chrome)).unwrap_or_default()
    }

    pub fn semantic_colors(&self) -> ffi::FfiSemanticColors {
        with_active(&self.themes, |theme| semantic_colors(&theme.semantic)).unwrap_or_default()
    }

    pub fn diff_colors(&self) -> ffi::FfiDiffColors {
        with_active(&self.themes, |theme| diff_colors(&theme.diff)).unwrap_or_default()
    }

    pub fn terminal_palette(&self) -> ffi::FfiTerminalPalette {
        with_active(&self.themes, |theme| terminal_palette(&theme.terminal)).unwrap_or_default()
    }
}

/// Runs `f` over the active theme, or `None` when nothing resolved — the one
/// case every accessor above falls back to a zeroed default for, rather than
/// panicking (practically unreachable: a built-in dark theme is always
/// offered unless every colour-theme plugin is disabled).
fn with_active<T>(
    themes: &Rc<RefCell<ColorThemeService>>,
    f: impl FnOnce(&ColorTheme) -> T,
) -> Option<T> {
    themes.borrow().active().map(f)
}
