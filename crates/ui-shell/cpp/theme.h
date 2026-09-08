#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QColor>
#include <QIcon>
#include <QPalette>
#include <QString>

class QWidget;
class ThemeProvider;

namespace ui_shell {

// The ThemeProvider applyTheme() reads every colour through (color-themes
// plan T7), for the few callers that need the provider itself rather than a
// palette it already derived — mirrors icon_cache.cpp's sharedIconProvider().
// Leaked deliberately, same reason: destroying a QObject after
// QGuiApplication is gone is a crash, not a cleanup.
ThemeProvider *sharedThemeProvider();

// The handful of theme colors a widget needs when it paints itself instead
// of being styled by QSS — today only the splash screen, which exists
// before the stylesheet can reach it. Kept next to the stylesheets so the
// two cannot drift apart.
struct ThemeColors
{
    QColor background;
    QColor foreground;
    QColor accent;
};

ThemeColors colorsForTheme(const QString &themeName);

// The semantic colours every status and severity label in the product uses
// (`docs/design/language-platform-ui.md` section 1). Kept next to the
// stylesheets for the same reason `ThemeColors` is: a colour picked in code
// and a colour picked in QSS must not drift.
//
// Meaning is never carried by hue alone — every one of these is applied to a
// word (`Error`, `Running`, `Disabled after crash`), so a greyscale
// screenshot and a screen reader carry the same information. Each value
// clears WCAG AA 4.5:1 against its theme's list background.
struct SemanticColors
{
    QColor error;
    QColor warning;
    QColor info;
    QColor ok;
    QColor muted;
};

SemanticColors semanticColorsForTheme(const QString &themeName);

// The same, for whatever theme is active — what a widget building rows wants.
SemanticColors semanticColors();

// The colours a diff paints with (`DiffView`, the unified viewer, the VCS
// gutter's change markers and the minimap's row marks), one set per theme —
// JetBrains' convention: added is green, modified is blue, deleted is grey.
// `*Line` is the full-width background of a changed line, `*Inline` the
// stronger shade over the words that actually changed within it, `*Marker`
// the opaque ink for a gutter strip or divider glyph. Each `*Line` shade
// keeps the theme's text at 4.5:1 or better, since code is read on top of
// it. Kept next to the stylesheets for the reason `SemanticColors` is: a
// colour picked in code and one picked in QSS must not drift.
struct DiffColors
{
    QColor addedLine;
    QColor addedInline;
    QColor addedMarker;
    QColor modifiedLine;
    QColor modifiedInline;
    QColor modifiedMarker;
    QColor deletedLine;
    QColor deletedInline;
    QColor deletedMarker;
};

DiffColors diffColorsForTheme(const QString &themeName);

// The same, for whatever theme is active.
DiffColors diffColors();

// The colour roles of the blend design spec (`--ide-*` in the approved
// mockup), one set per theme. Every stylesheet and palette in this file is
// generated from one of these, so a theme is only a list of colours — the
// shape of the chrome (radii, gaps, row heights) comes from ui_tokens.h and
// is identical across themes.
//
// `border` and `selection` are solid colours pre-blended over `surface`
// rather than the spec's rgba() values: a Qt stylesheet paints them over
// whatever is behind the widget, and a translucent border over a masked,
// rounded panel would show the canvas through its own corner.
struct ChromePalette
{
    QColor canvas;     // --ide-bg: the ground the panels sit on, and the editor
    QColor surface;    // --ide-surface: menu bar, toolbar, editor column, status bar
    QColor surface2;   // --ide-surface-2: side panels, inputs, buttons
    QColor raised;     // --ide-raised: a control under the mouse
    QColor border;     // --ide-border: every 1px separator
    QColor text;       // --ide-text
    QColor textDim;    // --ide-text-dim: menu bar, status bar, inactive tabs
    QColor accent;     // --ide-accent: active-tab marker, focus ring, progress
    QColor accentInk;  // text on an accent-filled control
    QColor selection;  // --ide-selection: the selected tree/list row
    QColor statusBar;  // the status bar's own ground (== surface except vscode-dark)
    QString chevron;   // resource path of the combo box arrow, tinted textDim
    QColor shadow;     // --panel-shadow ink (alpha ignored; see shadowOpacity)
    double shadowOpacity; // --panel-shadow alpha at the card's edge
};

// Any name other than "light"/"vscode-dark" is the default dark theme —
// the same fallback `Settings::theme_name()` resolves an unset theme to.
ChromePalette chromePaletteForTheme(const QString &themeName);

// The terminal's ANSI palette for one theme (T3): JetBrains Darcula's 16
// colours for the dark themes (default and "vscode-dark"), and JetBrains
// Light's for "light" — the same convention `chromePaletteForTheme` follows
// for what "light" means and what everything else falls back to.
// Background/foreground follow the editor's own colours
// (`appSettings->editorColors()`) when the user configured them, else
// `ChromePalette::canvas`/`text`; selection is `ChromePalette::selection`;
// the cursor is the resolved foreground. Picking these RGB values per theme
// name is presentation data (like `chromePaletteForTheme` itself), not a
// business rule — `AppSettings::terminalFont()` is where a real precedence
// decision (terminal override vs. editor font) is made, in Rust.
FfiTerminalPalette terminalPaletteForTheme(const QString &themeName, AppSettings *appSettings);

// The whole application stylesheet for `palette`: chrome, tabs, tree,
// inputs, scrollbars. Editor text colours are QPalette-driven, not QSS (A3).
QString chromeStyleSheet(const ChromePalette &palette);

// The same look in the docking system's own selectors. Qt gives a widget's
// own stylesheet priority over the application's however specific the
// latter is, and ADS installs one on its dock manager — so applyTheme()
// appends these to that sheet rather than to qApp's.
QString dockStyleSheet(const ChromePalette &palette);

// Picks the stylesheet for a theme name from `app-config::Settings` (T2).
QString styleSheetForTheme(const QString &themeName);

// The theme name last passed to applyTheme(). Widgets that pick colors in
// code rather than through QSS or the palette — today only the syntax
// highlighter's token colors — have no other route to the active theme.
QString activeThemeName();

// The QSS above only reaches widgets we wrote selectors for. Qt Advanced
// Docking System ships its own stylesheet, applied to the CDockManager
// widget itself, and every color in it resolves through `palette(...)` roles
// — as does `find_bar.cpp`. A widget-level sheet beats the application one
// on equal specificity, so dock chrome is themed by giving the application a
// palette that matches the stylesheet (issue #11) — everything except the
// dock tabs and splitters, which applyTheme() reaches by appending
// dockStyleSheet() to that sheet.
QPalette paletteForTheme(const QString &themeName);

// Applies both halves of a theme to the running QApplication. Callers should
// use this rather than setStyleSheet() alone, so palette and QSS can never
// drift apart.
void applyTheme(const QString &themeName);

// Registers the bundled Inter and JetBrains Mono faces (resources/fonts,
// both SIL OFL) and makes Inter the application font at the spec's 12.5px,
// before anything captures the application font — applyUiFontScale() scales
// whatever this installed. Falls back to the platform font, silently, if a
// resource cannot load. JetBrains Mono is registered but not applied here —
// it becomes the default editor/terminal font via
// `DEFAULT_EDITOR_FONT_FAMILY` (app-config) and `AppSettings::editorFont()`/
// `terminalFont()`.
void installBundledFonts();

// A 32x32 alpha mask (`resources/icons/**.a8`, rasterized offline — no
// Qt6Svg in this build) tinted with one colour. The one mechanism behind
// every glyph this app draws in code: the tab close (x), the symbol-kind
// glyphs, the diff toolbar's arrows and chevrons. The mask is re-read on
// every call; callers that need an icon repeatedly keep the result.
QIcon maskIcon(const char *maskResource, QColor tint);

// The tab/dock close (x) glyph, tinted to the active theme's dim text color
// (no Qt6Svg in this build, so the vendored ADS icon is rasterized to an
// alpha mask once — see resources/ui_icons.qrc — and recolored here rather
// than loaded as-is). Called by applyTheme() itself to keep both the plain
// QTabWidget close buttons (via a QProxyStyle) and ADS's own dock/tab close
// buttons (via ads::CIconProvider) in sync with a live theme switch.
QIcon tabCloseIcon();

// The magnifying-glass glyph for filter/search boxes (issue #233), tinted to
// the active theme's dim text color the same way tabCloseIcon() is — same
// no-Qt6Svg constraint, same alpha-mask mechanism.
QIcon searchIcon();

// Scales the whole application's default UI font to `percent` of the font
// installed at startup (100 = unchanged). Widgets that were never given a
// font of their own follow it; the two that were — see
// applyWidgetFontScale() — keep their own scale.
//
// Always relative to the font captured on the first call, so repeated live
// previews from the Settings dialog scale the original rather than
// compounding what the previous preview left behind.
void applyUiFontScale(int percent);

// Same scale, applied to one widget and its children (the menu bar and its
// popup menus, the project tree). An explicitly set font wins over the
// application one, which is exactly what keeps these two independent of
// applyUiFontScale().
void applyWidgetFontScale(QWidget *widget, int percent);

// Nudges `base` away from itself so a band or column drawn in the result
// reads as a subtle tint on both dark and light editor backgrounds. Used by
// every widget that paints its own chrome against QPalette::Base — the
// editor's gutter and current-line band, and the hex viewer's columns.
QColor tinted(const QColor &base, int darkFactor, int lightFactor);

} // namespace ui_shell
