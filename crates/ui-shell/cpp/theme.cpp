#include "theme.h"

#include "ui_tokens.h"

#include "DockManager.h"
#include "IconProvider.h"

#include <QApplication>
#include <QEvent>
#include <QFile>
#include <QFont>
#include <QFontDatabase>
#include <QImage>
#include <QPixmap>
#include <QProxyStyle>
#include <QWidget>

#include <utility>

namespace ui_shell {

namespace {

QColor hex(const char *value)
{
    return QColor(QString::fromLatin1(value));
}

// `{name}` placeholders, filled by name. Not QString::arg(): with a
// positional `%n` sheet, one placeholder left unused shifts every later
// argument down a slot silently, and the sheet then fails to parse —
// which Qt reports by styling nothing.
QString fillTokens(const ChromePalette &c, QString sheet)
{
    const std::pair<const char *, QString> values[] = {
        {"{canvas}", c.canvas.name()},
        {"{surface}", c.surface.name()},
        {"{surface2}", c.surface2.name()},
        {"{raised}", c.raised.name()},
        {"{border}", c.border.name()},
        {"{text}", c.text.name()},
        {"{textDim}", c.textDim.name()},
        {"{accent}", c.accent.name()},
        {"{accentInk}", c.accentInk.name()},
        {"{selection}", c.selection.name()},
        {"{statusBar}", c.statusBar.name()},
        {"{chevron}", c.chevron},
        {"{r-ctl}", QString::number(tokens::kRadiusControl)},
        {"{r-panel}", QString::number(tokens::kRadiusPanel)},
        {"{panel-gap}", QString::number(tokens::kPanelGap)},
        {"{row-h}", QString::number(tokens::kRowHeight)},
        {"{sp-1}", QString::number(tokens::kSp1)},
        {"{sp-2}", QString::number(tokens::kSp2)},
        {"{sp-3}", QString::number(tokens::kSp3)},
        {"{toolbar-h}", QString::number(tokens::kToolbarHeight)},
        {"{control-h}", QString::number(tokens::kToolbarHeight - 10)},
    };
    for (const auto &[key, value] : values) {
        sheet.replace(QLatin1String(key), value);
    }
    return sheet;
}

} // namespace

// Leaked deliberately — see the declaration in theme.h.
ThemeProvider *sharedThemeProvider()
{
    static ThemeProvider *provider = new ThemeProvider();
    return provider;
}

namespace {

QColor fromFfiRgb(const FfiRgb &color)
{
    return QColor(color.r, color.g, color.b);
}

FfiRgb toFfiRgb(const QColor &color)
{
    return FfiRgb{ static_cast<quint8>(color.red()), static_cast<quint8>(color.green()),
                   static_cast<quint8>(color.blue()) };
}

// `hex` empty means "not configured" (`AppSettings::editorColors()`'s own
// convention) — the caller's theme colour wins in that case.
QColor colorOrFallback(const QString &hex, const QColor &fallback)
{
    return hex.isEmpty() ? fallback : QColor(hex);
}

// `chevron`/`shadow`/`shadowOpacity` are not colour data a theme supplies
// (`color_theme::ChromeColors` deliberately excludes them, T1): every dark
// theme shares one grey chevron glyph and shadow ink, every light theme the
// other. `isDark` is `ThemeProvider::isDark()`'s answer for the theme this
// palette was just resolved from.
ChromePalette toChromePalette(const FfiChromePalette &c, bool isDark)
{
    ChromePalette p;
    p.canvas = fromFfiRgb(c.canvas);
    p.surface = fromFfiRgb(c.surface);
    p.surface2 = fromFfiRgb(c.surface2);
    p.raised = fromFfiRgb(c.raised);
    p.border = fromFfiRgb(c.border);
    p.text = fromFfiRgb(c.text);
    p.textDim = fromFfiRgb(c.text_dim);
    p.accent = fromFfiRgb(c.accent);
    p.accentInk = fromFfiRgb(c.accent_ink);
    p.selection = fromFfiRgb(c.selection);
    p.statusBar = fromFfiRgb(c.status_bar);
    if (isDark) {
        p.chevron = QStringLiteral(":/ui/icons/chevron_dark.png");
        p.shadow = Qt::black;
        p.shadowOpacity = 0.30;
    } else {
        p.chevron = QStringLiteral(":/ui/icons/chevron_light.png");
        p.shadow = hex("#0f172a");
        p.shadowOpacity = 0.18;
    }
    return p;
}

SemanticColors toSemanticColors(const FfiSemanticColors &s)
{
    return SemanticColors{ fromFfiRgb(s.error), fromFfiRgb(s.warning), fromFfiRgb(s.info),
                           fromFfiRgb(s.ok), fromFfiRgb(s.muted) };
}

DiffColors toDiffColors(const FfiDiffColors &d)
{
    return DiffColors{ fromFfiRgb(d.added_line),     fromFfiRgb(d.added_inline),
                       fromFfiRgb(d.added_marker),    fromFfiRgb(d.modified_line),
                       fromFfiRgb(d.modified_inline), fromFfiRgb(d.modified_marker),
                       fromFfiRgb(d.deleted_line),    fromFfiRgb(d.deleted_inline),
                       fromFfiRgb(d.deleted_marker) };
}

// What `applyTheme()` asks the provider for once and every other function in
// this file reads back — the "ask once, cache, read the cache" the
// color-themes plan describes. Every caller of `chromePaletteForTheme` and
// friends passes either `activeThemeName()` or the name `applyTheme()` was
// just given (confirmed by grepping every call site in `crates/ui-shell/cpp/`
// while T7 was implemented — `splash_screen.cpp` is the one that looked like
// it might not, but `main_window.cpp` calls `applyTheme(appSettings->themeName())`
// immediately before constructing it with the same name), so none of them
// need to resolve an arbitrary *other* theme's colours — reading this cache
// regardless of the name argument is exactly as correct as the old
// name-keyed static tables were.
struct CachedTheme
{
    QString name;
    ChromePalette chrome;
    SemanticColors semantic;
    DiffColors diff;
    bool dark = true;
};

CachedTheme resolveCachedTheme(const QString &name)
{
    ThemeProvider *provider = sharedThemeProvider();
    const bool dark = provider->isDark();
    CachedTheme cached;
    cached.name = name;
    cached.chrome = toChromePalette(provider->chromePalette(), dark);
    cached.semantic = toSemanticColors(provider->semanticColors());
    cached.diff = toDiffColors(provider->diffColors());
    cached.dark = dark;
    return cached;
}

// Readable before any explicit applyTheme() call (the splash screen's own
// reason for needing colorsForTheme() at all, per its doc comment above) —
// resolved from whatever the shared ColorThemeService already picked at
// process start (settings.toml's theme, or its own fallback), same as
// `ThemeProvider` itself is available from process start.
CachedTheme &cachedTheme()
{
    static CachedTheme cached = resolveCachedTheme(QStringLiteral("dark"));
    return cached;
}

} // namespace

ChromePalette chromePaletteForTheme(const QString &themeName)
{
    Q_UNUSED(themeName);
    return cachedTheme().chrome;
}

FfiTerminalPalette terminalPaletteForTheme(const QString &themeName, AppSettings *appSettings)
{
    FfiTerminalPalette palette = sharedThemeProvider()->terminalPalette();
    const ChromePalette chrome = chromePaletteForTheme(themeName);
    const FfiEditorColors editorColors = appSettings->editorColors();
    const QColor foreground = colorOrFallback(editorColors.foreground, chrome.text);

    palette.background = toFfiRgb(colorOrFallback(editorColors.background, chrome.canvas));
    palette.foreground = toFfiRgb(foreground);
    palette.cursor = toFfiRgb(foreground);
    palette.selection = toFfiRgb(chrome.selection);
    return palette;
}

// Embedded as a compile-time string rather than a .qrc/rcc resource or an
// install-relative asset directory: the whole app ships as one binary per
// docker/Dockerfile's artifact stages, so there is no asset-deployment step
// to wire up, and no runtime path resolution to get wrong on Windows vs.
// Linux.
//
// One sheet for every theme. The numbers are the blend spec's tokens
// (ui_tokens.h) and the colours the palette's; nothing here is per-theme.
QString chromeStyleSheet(const ChromePalette &c)
{
    // Positional %n arguments, grouped so a reader can find one:
    //   {canvas} canvas  {surface} surface  {surface2} surface2  {raised} raised  {border} border
    //   {text} text    {textDim} textDim  {accent} accent    {accentInk} accentInk {selection} selection
    //   {statusBar} statusBar
    //   {r-ctl} r-ctl  {row-h} row-h   {sp-1} sp-1     {sp-2} sp-2   {sp-3} sp-3
    //   {toolbar-h} toolbar-h  {control-h} control-h (toolbar-h - 10)
    return fillTokens(c, QStringLiteral(R"(
/* ---- ground ------------------------------------------------------ */
QWidget {
    color: {text};
    selection-background-color: {selection};
    selection-color: {text};
}

QMainWindow {
    background-color: {canvas};
}

QDialog, QMessageBox, QInputDialog, QFileDialog {
    background-color: {surface};
}

QToolTip {
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: {r-ctl}px;
    padding: {sp-1}px {sp-2}px;
}

/* ---- menu bar and menus ------------------------------------------ */
QMenuBar {
    background-color: {surface};
    color: {textDim};
    border-bottom: 1px solid {border};
    padding: 2px {sp-2}px;
}

QMenuBar::item {
    background: transparent;
    padding: {sp-1}px {sp-2}px;
    border-radius: {r-ctl}px;
}

QMenuBar::item:selected {
    background-color: {surface2};
    color: {text};
}

QMenu {
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: {r-ctl}px;
    padding: {sp-1}px;
}

QMenu::item {
    padding: 5px 24px 5px {sp-3}px;
    border-radius: {r-ctl}px;
}

QMenu::item:selected {
    background-color: {selection};
}

QMenu::item:disabled {
    color: {textDim};
}

QMenu::separator {
    height: 1px;
    background: {border};
    margin: {sp-1}px {sp-2}px;
}

/* ---- the toolbar under the menu bar ------------------------------ */
QToolBar {
    background-color: {surface};
    border: none;
    border-bottom: 1px solid {border};
    padding: 0 {sp-2}px;
    spacing: {sp-1}px;
    min-height: {toolbar-h}px;
    max-height: {toolbar-h}px;
}

QToolBar::separator {
    width: 1px;
    background: {border};
    margin: 6px {sp-1}px;
}

/* ---- controls ---------------------------------------------------- */
QToolButton {
    background: transparent;
    color: {textDim};
    border: none;
    border-radius: {r-ctl}px;
    padding: 2px;
}

/* A styled QToolButton loses the native style's reserved room for its menu
   arrow, so a MenuButtonPopup button draws the arrow on top of its own label
   (issue #234). Qt's documented fix: reserve the arrow's width by hand.
   popupMode 1 is QToolButton::MenuButtonPopup. */
QToolButton[popupMode="1"] {
    padding-right: 16px;
}

QToolButton::menu-button {
    border: none;
    width: 14px;
}

QToolButton:hover {
    background-color: {raised};
    color: {text};
}

QToolButton:pressed, QToolButton:checked {
    background-color: {selection};
    color: {text};
}

QToolButton:disabled {
    background: transparent;
    color: {textDim};
}

/* ---- diff viewer chrome ------------------------------------------ */
QWidget#diffToolbar {
    background: {surface};
    border-bottom: 1px solid {border};
}

QWidget#diffToolbar QLabel#diffCount {
    color: {textDim};
    padding: 0 {sp-2}px;
}

QWidget#diffPaneHeader {
    background: {surface};
    border-bottom: 1px solid {border};
}

QWidget#diffPaneHeader QLabel {
    color: {textDim};
    padding: 0 {sp-2}px;
}

QPushButton {
    background-color: {surface2};
    color: {text};
    border: 1px solid {border};
    border-radius: {r-ctl}px;
    padding: 0 {sp-3}px;
    min-height: {control-h}px;
    max-height: {control-h}px;
}

QPushButton:hover {
    background-color: {raised};
}

QPushButton:pressed {
    background-color: {selection};
}

QPushButton:default {
    background-color: {accent};
    color: {accentInk};
    border-color: {accent};
}

QPushButton:disabled {
    color: {textDim};
}

QComboBox {
    background-color: {surface2};
    color: {text};
    border: 1px solid {border};
    border-radius: {r-ctl}px;
    padding: 0 {sp-2}px 0 {sp-3}px;
    min-height: {control-h}px;
    max-height: {control-h}px;
    font-weight: 500;
}

QComboBox:hover {
    background-color: {raised};
}

/* The pill has one border, its own: the platform style would otherwise
   frame the arrow's sub-control as a second box inside it. */
QComboBox::drop-down {
    subcontrol-origin: padding;
    subcontrol-position: center right;
    width: 20px;
    border: none;
    background: transparent;
}

QComboBox::down-arrow {
    image: url({chevron});
    width: 10px;
    height: 10px;
}

QComboBox QAbstractItemView {
    background-color: {surface};
    color: {text};
    border: 1px solid {border};
    border-radius: {r-ctl}px;
    selection-background-color: {selection};
    selection-color: {text};
    outline: 0;
}

QLineEdit {
    background-color: {surface2};
    color: {text};
    border: 1px solid {border};
    border-radius: {r-ctl}px;
    padding: 0 {sp-2}px;
    min-height: {control-h}px;
    max-height: {control-h}px;
}

QLineEdit:focus {
    border-color: {accent};
}

/* Multi-line inputs keep the palette's Base as their ground so the code
   editor — itself a QPlainTextEdit — keeps the colours the user picked in
   Settings; only the frame is styled here, and CodeEditor below sheds it. */
QPlainTextEdit, QTextEdit {
    border: 1px solid {border};
    border-radius: {r-ctl}px;
    padding: {sp-1}px {sp-2}px;
}

QPlainTextEdit:focus, QTextEdit:focus {
    border-color: {accent};
}

ui_shell--CodeEditor, ui_shell--HexViewer {
    border: none;
    border-radius: 0;
    padding: 0;
}

QCheckBox, QRadioButton {
    spacing: 6px;
}

QProgressBar {
    background-color: {surface2};
    border: 1px solid {border};
    border-radius: 4px;
    max-height: 8px;
}

QProgressBar::chunk {
    background-color: {accent};
    border-radius: 3px;
}

/* ---- trees and lists --------------------------------------------- */
QTreeView, QListView, QListWidget, QTreeWidget, QTableView {
    background-color: {surface2};
    alternate-background-color: {surface2};
    color: {text};
    border: none;
    padding: 0 {sp-1}px {sp-2}px {sp-1}px;
    outline: 0;
}

QTreeView::item, QListView::item, QListWidget::item,
QTreeView::item:hover, QListView::item:hover, QListWidget::item:hover,
QTreeView::item:selected, QListView::item:selected, QListWidget::item:selected {
    min-height: {row-h}px;
    padding: 0 {sp-1}px;
    border-radius: {r-ctl}px;
    border: none;
}

QTreeView::item:hover, QListView::item:hover, QListWidget::item:hover {
    background-color: {raised};
}

QTreeView::item:selected, QListView::item:selected, QListWidget::item:selected {
    background-color: {selection};
    color: {text};
}

QHeaderView::section {
    background: transparent;
    color: {textDim};
    border: none;
    border-bottom: 1px solid {border};
    padding: {sp-1}px {sp-2}px;
}

/* ---- editor tabs ------------------------------------------------- */
QTabWidget {
    background-color: {surface};
}

QTabWidget::pane {
    background-color: {canvas};
    border: none;
    border-top: 1px solid {border};
}

QTabBar {
    background-color: {surface};
    border: none;
}

/* Asymmetric padding on purpose: the right side carries the close button.
   Qt lays a closable tab out as [padding-left][icon][label][reserved][x] and
   spends everything but a fixed, base-style-dependent edge offset on the gap
   before the [x] — a gap that measures padding-right + 13 + half the close
   indicator's own slack. The [x]'s breathing room is owned by that indicator
   (TabCloseIconStyle::pixelMetric in main_window.cpp), which is the only
   lever that reaches the space to its *right*; this padding only sizes the
   space to its left, so it stays small enough that the [x] reads as
   belonging to the label it closes. Measured under Fusion: 22px left of the
   [x], 15px right of it. */
QTabBar::tab {
    background-color: {surface};
    color: {textDim};
    padding: 0 {sp-1}px 0 {sp-3}px;
    min-height: 32px;
    border: none;
    border-right: 1px solid {border};
    border-bottom: 2px solid transparent;
}

QTabBar::tab:selected {
    background-color: {canvas};
    color: {text};
    border-bottom: 2px solid {accent};
}

QTabBar::tab:hover:!selected {
    background-color: {raised};
    color: {text};
}

/* No `QTabBar::close-button` rule here, deliberately. Once the application
   has a stylesheet, QStyleSheetStyle answers SE_TabBarTabRightButton itself:
   it never delegates to the base style (a QProxyStyle override of it is not
   called at all), and it ignores this subcontrol's own margins (a 60px margin
   moves the [x] by zero pixels). #130's spacing fix lived here and stopped
   doing anything the moment the blend chrome rewrote this sheet, without
   anything turning red. The two levers that do move the [x] are the tab's
   padding above (space to its left) and PM_TabCloseIndicatorWidth in
   TabCloseIconStyle (space on both sides, and the only one that reaches the
   right) — change the spacing there, never here. */

/* ---- splitters, scrollbars, status bar --------------------------- */
QSplitter::handle {
    background-color: {border};
}

QScrollBar:vertical {
    background: transparent;
    border: none;
    width: 10px;
    margin: 0;
}

QScrollBar:horizontal {
    background: transparent;
    border: none;
    height: 10px;
    margin: 0;
}

QScrollBar::handle {
    background: {border};
    border-radius: 4px;
    margin: 2px;
}

QScrollBar::handle:vertical {
    min-height: 24px;
}

QScrollBar::handle:horizontal {
    min-width: 24px;
}

QScrollBar::handle:hover {
    background: {textDim};
}

QScrollBar::add-line, QScrollBar::sub-line {
    height: 0px;
    width: 0px;
}

QScrollBar::add-page, QScrollBar::sub-page {
    background: transparent;
}

QStatusBar {
    background-color: {statusBar};
    color: {textDim};
    border-top: 1px solid {border};
    min-height: 26px;
    padding: 0 {sp-1}px;
}

/* One flat strip: Qt frames every permanent widget by default, which reads
   as a row of little boxes rather than a status line. */
QStatusBar::item {
    border: none;
}

QStatusBar QLabel, QStatusBar QProgressBar, QStatusBar QToolButton {
    background: transparent;
    color: {textDim};
}
)")
    );
}

QString dockStyleSheet(const ChromePalette &c)
{
    // Appended to the dock manager's own sheet by restyleDockManagers(), so
    // these repeat ADS's selectors verbatim and win on being later. That is
    // also the only way to reach the splitter handles between docked panes:
    // Qt gives a widget's own stylesheet priority over the application's,
    // and ADS installs one on the dock manager.
    return fillTokens(c, QStringLiteral(R"(
/* The gap between two docked panels: `--panel-gap` wide and see-through,
   so what separates two panels is the canvas — and the panel shadows
   panel_shadow.cpp paints on it — rather than a divider line. ADS has no
   C++ knob for inter-panel spacing; the splitter handle it puts there
   anyway is the spacing, once it is given a width. */
ads--CDockContainerWidget ads--CDockSplitter::handle {
    background: transparent;
    width: {panel-gap}px;
    height: {panel-gap}px;
}

ads--CDockContainerWidget {
    background: {canvas};
}

/* ADS's own sheet pads the splitters by a pixel and gives every dock
   widget a palette(light) ground with a 1px top rule; both would show as
   a stripe between the tab strip and the panel body, so the dock widget
   is made see-through and the area behind it carries the colour. */
ads--CDockContainerWidget > QSplitter {
    padding: 0;
}

ads--CDockWidget {
    background: transparent;
    border: none;
}

ads--CAutoHideSideBar[sideBarLocation="0"] { border-bottom: 1px solid {border}; }
ads--CAutoHideSideBar[sideBarLocation="1"] { border-right: 1px solid {border}; }
ads--CAutoHideSideBar[sideBarLocation="2"] { border-left: 1px solid {border}; }
ads--CAutoHideSideBar[sideBarLocation="3"] { border-top: 1px solid {border}; }

/* The panel card. Its rounded corners and 1px border are painted by
   rounded_corners.cpp on top of the children — a QSS radius rounds only
   what this widget paints itself, and every child paints an opaque square
   over it. main_window.cpp gives the area's layout a 1px margin so no
   child sits under the border line. Side panels sit on surface-2, the
   editor column on surface, exactly as the mockup's `.sidebar` and
   `.editor-col` do. */
ads--CDockAreaWidget {
    background-color: {surface2};
    border: none;
}

ads--CDockAreaWidget[centralArea="true"] {
    background-color: {surface};
}

/* The tab strip: surface with the spec's 1px bottom rule, tabs 34px tall
   with a 1px rule between them, the active one dropping to the canvas
   colour with a 2px accent under it — the same rules QTabBar gets in
   chromeStyleSheet(). */
ads--CDockAreaTitleBar {
    background-color: {surface};
    border: none;
    border-bottom: 1px solid {border};
    padding: 0;
}

ads--CDockAreaTitleBar QToolButton {
    background: transparent;
    border: none;
    border-radius: {r-ctl}px;
    padding: 2px;
}

ads--CDockAreaTitleBar QToolButton:hover {
    background-color: {raised};
}

ads--CDockWidgetTab {
    background: {surface};
    border: none;
    border-right: 1px solid {border};
    border-bottom: 2px solid transparent;
    padding: 0 {sp-3}px 0 {sp-3}px;
    min-height: 32px;
}

ads--CDockWidgetTab QLabel {
    color: {textDim};
    background: transparent;
}

ads--CDockWidgetTab:hover {
    background: {raised};
}

ads--CDockWidgetTab:hover QLabel {
    color: {text};
    background: transparent;
}

ads--CDockWidgetTab[activeTab="true"] {
    background: {canvas};
    border-bottom: 2px solid {accent};
}

ads--CDockWidgetTab[activeTab="true"] QLabel {
    color: {text};
    background: transparent;
}

ads--CDockWidgetTab #tabCloseButton {
    background: none;
    border: none;
    margin: 0px 0px 0px 2px;
    padding: 0px;
    qproperty-iconSize: 12px;
}

ads--CDockWidgetTab #tabCloseButton:hover {
    background: {raised};
    border: none;
    border-radius: {r-ctl}px;
}

ads--CDockWidgetTab #tabCloseButton:pressed {
    background: {selection};
}
)")
    );
}

ThemeColors colorsForTheme(const QString &themeName)
{
    const ChromePalette c = chromePaletteForTheme(themeName);
    return ThemeColors{c.surface, c.text, c.accent};
}

SemanticColors semanticColorsForTheme(const QString &themeName)
{
    Q_UNUSED(themeName);
    return cachedTheme().semantic;
}

SemanticColors semanticColors()
{
    return semanticColorsForTheme(activeThemeName());
}

DiffColors diffColorsForTheme(const QString &themeName)
{
    Q_UNUSED(themeName);
    return cachedTheme().diff;
}

DiffColors diffColors()
{
    return diffColorsForTheme(activeThemeName());
}

QString styleSheetForTheme(const QString &themeName)
{
    return chromeStyleSheet(chromePaletteForTheme(themeName));
}

namespace {

// ADS keeps the stylesheet it installed on the dock manager here, so every
// re-style starts from its rules instead of stacking ours on themselves.
const char *const kAdsBaseStyleSheet = "ideAdsBaseStyleSheet";

void restyleDockManager(QWidget *dockManager)
{
    if (dockManager == nullptr) {
        return;
    }
    if (!dockManager->property(kAdsBaseStyleSheet).isValid()) {
        dockManager->setProperty(kAdsBaseStyleSheet, dockManager->styleSheet());
    }
    dockManager->setStyleSheet(dockManager->property(kAdsBaseStyleSheet).toString()
                               + dockStyleSheet(chromePaletteForTheme(activeThemeName())));
}

bool isDockManager(const QObject *object)
{
    return object->inherits("ads::CDockManager");
}

// run_app() themes the application before any window exists, so the dock
// manager is always born after the theme it has to wear. Rather than have
// main_window.cpp announce it — F0-4b/F0-5 left that file two lines under the
// 1200-line ceiling (scripts/check-file-size.sh) — theme.cpp styles every
// dock manager the moment Qt polishes it.
class DockManagerWatcher : public QObject
{
public:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        if (event->type() == QEvent::Polish && isDockManager(watched)) {
            restyleDockManager(qobject_cast<QWidget *>(watched));
        }
        return QObject::eventFilter(watched, event);
    }
};

void restyleDockManagers()
{
    static DockManagerWatcher *watcher = [] {
        auto *installed = new DockManagerWatcher;
        qApp->installEventFilter(installed);
        return installed;
    }();
    Q_UNUSED(watcher)

    const QWidgetList widgets = QApplication::allWidgets();
    for (QWidget *widget : widgets) {
        if (isDockManager(widget)) {
            restyleDockManager(widget);
        }
    }
}

} // namespace

QPalette paletteForTheme(const QString &themeName)
{
    const ChromePalette c = chromePaletteForTheme(themeName);
    QPalette palette;
    // Window is what a panel's plain-QWidget body paints (the project tree's
    // container, the Changes and Search panels...), so it is the side-panel
    // ground: the mockup's `.sidebar` surface-2. The surfaces that are
    // `surface` — menu bar, toolbar, editor column, status bar — are each
    // named in chromeStyleSheet().
    palette.setColor(QPalette::Window, c.surface2);
    palette.setColor(QPalette::WindowText, c.text);
    // CodeEditor derives its gutter, current-line band and find-match tints
    // from Base/Text, so the editor surface reaches it through the palette:
    // the code area is the canvas colour, as in the mockup's `.code`.
    palette.setColor(QPalette::Base, c.canvas);
    palette.setColor(QPalette::AlternateBase, c.surface2);
    palette.setColor(QPalette::Text, c.text);
    palette.setColor(QPalette::Button, c.surface2);
    palette.setColor(QPalette::ButtonText, c.text);
    palette.setColor(QPalette::ToolTipBase, c.surface);
    palette.setColor(QPalette::ToolTipText, c.text);
    palette.setColor(QPalette::Highlight, c.selection);
    palette.setColor(QPalette::HighlightedText, c.text);
    palette.setColor(QPalette::PlaceholderText, c.textDim);
    palette.setColor(QPalette::Link, c.accent);

    // ADS's own sheet resolves its colours through these roles: Light is
    // the selected-tab body, Midlight the hover shade, Mid the separators.
    // Dark colours the inactive dock-tab label, so it has to be readable
    // text on the chrome rather than a literally dark shade.
    palette.setColor(QPalette::Light, c.raised);
    palette.setColor(QPalette::Midlight, c.raised);
    palette.setColor(QPalette::Mid, c.border);
    palette.setColor(QPalette::Dark, c.textDim);
    palette.setColor(QPalette::Shadow, c.canvas.darker(130));

    palette.setColor(QPalette::Disabled, QPalette::WindowText, c.textDim);
    palette.setColor(QPalette::Disabled, QPalette::Text, c.textDim);
    palette.setColor(QPalette::Disabled, QPalette::ButtonText, c.textDim);
    return palette;
}

QString activeThemeName()
{
    return cachedTheme().name;
}

QIcon maskIcon(const char *maskResource, QColor tint)
{
    constexpr int kSide = 32;
    QFile file(QString::fromLatin1(maskResource));
    file.open(QIODevice::ReadOnly);
    const QByteArray mask = file.readAll();
    Q_ASSERT(mask.size() == kSide * kSide);

    QImage image(kSide, kSide, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::transparent);
    for (int y = 0; y < kSide; ++y) {
        for (int x = 0; x < kSide; ++x) {
            QColor pixel = tint;
            pixel.setAlpha(static_cast<uchar>(mask[y * kSide + x]));
            image.setPixelColor(x, y, pixel);
        }
    }
    return QIcon(QPixmap::fromImage(image));
}

namespace {

// Swaps Qt's platform close glyph for tabCloseIcon() below, so every plain
// QTabWidget's close button — editor tab groups, terminal session tabs —
// matches ADS's own dock/tab close buttons, themed the same way (see
// applyTheme()'s ads::CIconProvider registration for that half of the swap),
// and widens that button so the glyph is not flush against the tab's edge.
// Everything else falls through to the base style unchanged.
class TabCloseStyle : public QProxyStyle
{
public:
    using QProxyStyle::QProxyStyle;

    QIcon standardIcon(StandardPixmap standardIcon, const QStyleOption *option,
                        const QWidget *widget) const override
    {
        if (standardIcon == QStyle::SP_TabCloseButton) {
            return tabCloseIcon();
        }
        return QProxyStyle::standardIcon(standardIcon, option, widget);
    }

    // Breathing room around the [x]. QCommonStyle centres the close glyph in
    // a button PM_TabCloseIndicatorWidth wide and the base style pins that
    // button a fixed distance off the tab's right edge — 11px under Fusion,
    // ~5px under the Windows style, which is why the [x] sat flush against
    // the tab's border there. Nothing in the stylesheet can reach that space:
    // QStyleSheetStyle answers SE_TabBarTabRightButton itself and ignores
    // both `QTabBar::close-button`'s margins and a subElementRect override
    // (see the QTabBar rules in chromeStyleSheet() above). Widening the
    // indicator does reach it,
    // on every platform: the extra 8px split evenly around the centred glyph
    // buys 4px on each side, and the tab's padding-right is trimmed to match
    // so the gap to the label does not grow with it.
    int pixelMetric(PixelMetric metric, const QStyleOption *option,
                     const QWidget *widget) const override
    {
        if (metric == QStyle::PM_TabCloseIndicatorWidth) {
            return QProxyStyle::pixelMetric(metric, option, widget) + 8;
        }
        return QProxyStyle::pixelMetric(metric, option, widget);
    }
};

} // namespace

QIcon tabCloseIcon()
{
    return maskIcon(":/ui/icons/close_mask_32.a8", chromePaletteForTheme(activeThemeName()).textDim);
}

QStyle *makeTabCloseStyle(QStyle *base)
{
    return new TabCloseStyle(base);
}

QIcon searchIcon()
{
    return maskIcon(":/ui/icons/search_mask_32.a8",
                     chromePaletteForTheme(activeThemeName()).textDim);
}

void applyTheme(const QString &themeName)
{
    // Switches the shared colour-theme service's active theme, then asks it
    // once for everything downstream needs — the "ask once, cache, read the
    // cache" strategy `cachedTheme()`'s doc comment above describes.
    sharedThemeProvider()->applyColorTheme(themeName);
    cachedTheme() = resolveCachedTheme(themeName);

    qApp->setPalette(paletteForTheme(themeName));
    // Re-setting the sheet after the palette forces Qt to re-resolve every
    // `palette(...)` reference in it, including the ones inside the dock
    // manager's own sheet.
    qApp->setStyleSheet(styleSheetForTheme(themeName));
    restyleDockManagers();

    // ads::CIconProvider is a process-wide singleton (CDockManager::
    // iconProvider() is static), so this is safe to call before any
    // CDockManager exists — it just primes what the first one will read.
    // Existing dock tabs/title bars already painted keep their old-tint
    // icon until they're recreated (ADS caches the QIcon on the button at
    // construction, not resolved per paint) — a live theme switch fully
    // catching up needs restarting the app, same as it already does for a
    // few other chrome details.
    const QIcon closeIcon = tabCloseIcon();
    ads::CDockManager::iconProvider().registerCustomIcon(ads::TabCloseIcon, closeIcon);
    ads::CDockManager::iconProvider().registerCustomIcon(ads::DockAreaCloseIcon, closeIcon);
}

void installBundledFonts()
{
    // Inter (UI chrome) and JetBrains Mono (editor/terminal default,
    // `DEFAULT_EDITOR_FONT_FAMILY`) — both SIL OFL, both registered here so
    // neither font depends on what happens to be installed on the host.
    for (const char *face : {"Inter-Regular", "Inter-Medium", "Inter-SemiBold",
                              "JetBrainsMono-Regular", "JetBrainsMono-Bold",
                              "JetBrainsMono-Italic", "JetBrainsMono-BoldItalic"}) {
        QFontDatabase::addApplicationFont(
          QStringLiteral(":/ui/fonts/%1.ttf").arg(QLatin1String(face)));
    }
    // 12.5 CSS px at 96 dpi is 9.375pt; points rather than pixels so the
    // platform's own display scaling applies to it like any other font.
    QFont font(QStringLiteral("Inter"));
    font.setPointSizeF(9.5);
    qApp->setFont(font);
}

namespace {

// The interface font as installed at startup, captured before anything
// scales it. Static rather than re-read from qApp because
// applyUiFontScale() overwrites qApp's font, so after the first call qApp
// no longer knows the original.
QFont baseUiFont()
{
    static const QFont base = QApplication::font();
    return base;
}

QFont scaled(const QFont &base, int percent)
{
    QFont font = base;
    // A font carries either a point size or a pixel size; the unused one
    // reads back as -1, and setting it would discard the other.
    if (base.pointSizeF() > 0.0) {
        font.setPointSizeF(base.pointSizeF() * percent / 100.0);
    } else if (base.pixelSize() > 0) {
        font.setPixelSize(qMax(1, qRound(base.pixelSize() * percent / 100.0)));
    }
    return font;
}

} // namespace

void applyUiFontScale(int percent)
{
    qApp->setFont(scaled(baseUiFont(), percent));
    // Widgets built before this call keep the metrics QStyleSheetStyle
    // computed for them when it first polished them, so a live change from
    // the Settings dialog would not show until the next restart. Re-setting
    // the sheet re-polishes every widget against the new application font —
    // the same trick applyTheme() uses to re-resolve palette() references.
    qApp->setStyleSheet(styleSheetForTheme(activeThemeName()));
    restyleDockManagers();
}

void applyWidgetFontScale(QWidget *widget, int percent)
{
    if (widget != nullptr) {
        widget->setFont(scaled(baseUiFont(), percent));
    }
}

QColor tinted(const QColor &base, int darkFactor, int lightFactor)
{
    return base.lightness() < 128 ? base.lighter(darkFactor) : base.darker(lightFactor);
}

} // namespace ui_shell
