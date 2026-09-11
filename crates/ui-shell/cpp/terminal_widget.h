#pragma once

#include <QColor>
#include <QElapsedTimer>
#include <QFont>
#include <QPoint>
#include <QRect>
#include <QWidget>

#include <functional>
#include <string>

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QAction;
class QContextMenuEvent;
class QEvent;
class QFocusEvent;
class QKeyEvent;
class QMouseEvent;
class QPainter;
class QPaintEvent;
class QResizeEvent;
class QScrollBar;
class QShowEvent;
class QWheelEvent;

namespace ui_shell {

// Embedded terminal dock widget (Task F3, multi-session since F4-14): a
// custom QWidget that paints the cell grid `TerminalSupervisor` (Rust:
// `pty_core::PtySession` + `terminal_core::TerminalEmulator`, one pair per
// `sessionId`) hands over for its one session, and forwards key events back
// to it. Humble view per CLAUDE.md's hard rule — VT100 interpretation and
// grid state live entirely in `terminal-core`/the bridge; this class only
// paints `snapshot(sessionId)`'s cached result and translates key events to
// bytes. Deliberately not QTermWidget (ADR-0007): that would put untestable
// VT logic behind Qt.
//
// One `TerminalWidget` per open tab, all sharing the one `TerminalSupervisor`
// QObject (see that type's doc comment in `bridge/ffi.rs` for why there is
// one adapter instance, not N) — `sessionId` is what tells them apart, both
// for every call this class makes and for filtering the shared
// `gridUpdated(sessionId)` signal down to the one session this widget cares
// about.
class TerminalWidget : public QWidget
{
    Q_OBJECT

public:
    // `openAt` is how a `file:line` link in the output reaches the editor
    // (R2-6) — the same callback `RunConsolePanel` takes, threaded through
    // the sessions panel from `main_window`.
    using OpenAt = std::function<void(const QString &, int, int)>;

    // `shellId` is a `FfiShellCandidate::id` when the tab was opened from
    // the dock's "+" dropdown, and empty for "whatever the settings say".
    // Carried, never interpreted: it is forwarded verbatim to `start()`,
    // and which shell it resolves to is decided in `bridge/terminal.rs`.
    TerminalWidget(TerminalSupervisor *supervisor, quint64 sessionId, QString shellId,
                   AppSettings *appSettings, OpenAt openAt, QWidget *parent = nullptr);

    quint64 sessionId() const { return sessionId_; }

    // Copy/Paste are QActions rather than hardcoded key handling so their
    // shortcuts come from the persisted keymap ("terminal.copy"/
    // "terminal.paste") like every other action in the app. main_window
    // registers them under those ids so a rebinding applies live.
    QAction *copyAction() const { return copyAction_; }
    QAction *pasteAction() const { return pasteAction_; }

    // Re-read Copy/Paste's shortcuts from `appSettings` after a keymap
    // rebind. Each `TerminalWidget` owns its own QActions (see this class's
    // doc comment on why they cannot live in a shared-by-id map), so a
    // rebind is applied per open tab, by whoever owns them
    // (`TerminalSessionsPanel::reapplyKeymap`) — not through the app-wide
    // `applyKeymap()` every menu action uses.
    void reapplyKeymap();

    // Re-read the terminal font (`AppSettings::terminalFont()`) and the
    // theme's palette (`terminalPaletteForTheme(activeThemeName())`, T3)
    // after Settings > Terminal/Appearance's OK — the per-tab counterpart of
    // `reapplyKeymap()` above, called from the same place
    // (`TerminalSessionsPanel::reapplyAppearance`).
    void reapplyAppearance();

protected:
    // A focused terminal owns its Ctrl-combinations, so this intercepts the
    // window's menu shortcuts before they can swallow them (see the .cpp).
    bool event(QEvent *event) override;
    void paintEvent(QPaintEvent *event) override;
    void resizeEvent(QResizeEvent *event) override;
    void keyPressEvent(QKeyEvent *event) override;
    void showEvent(QShowEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;
    void mouseReleaseEvent(QMouseEvent *event) override;
    void mouseDoubleClickEvent(QMouseEvent *event) override;
    void contextMenuEvent(QContextMenuEvent *event) override;
    // The cursor renders differently focused vs. not (filled block vs.
    // outline), so a focus change alone has to trigger a repaint.
    void focusInEvent(QFocusEvent *event) override;
    void focusOutEvent(QFocusEvent *event) override;
    // Scrollback (T5): the wheel scrolls history, except on the alternate
    // screen (`vim`/`less`), where it becomes arrow keys instead — see the
    // .cpp's doc comment.
    void wheelEvent(QWheelEvent *event) override;

private:
    // One run of consecutive same-styled cells within a row, the unit
    // `paintEvent` actually draws (T2): grouping by this rather than
    // painting cell-by-cell turns "one fillRect + one drawText per
    // character" into one of each per stretch of uniformly-styled text,
    // which is what a line of plain output mostly is.
    struct CellStyle
    {
        QColor fg;
        QColor bg;
        bool bold = false;
        bool italic = false;
        bool underline = false;
        bool selected = false;
        bool isCursor = false;

        bool operator==(const CellStyle &other) const
        {
            return fg == other.fg && bg == other.bg && bold == other.bold && italic == other.italic
              && underline == other.underline && selected == other.selected && isCursor == other.isCursor;
        }
    };

    // Recompute rows/cols from the widget's current pixel size and the
    // monospace font's cell metrics, and — if that changed the grid size —
    // either `start()` the session (first call) or `resize()` it.
    void syncGridSizeToWidget();

    // (Re)reads `appSettings_->terminalFont()` into `font_`/the bold/italic/
    // bold-italic variants and their cell metrics. Shared by the constructor
    // and `reapplyAppearance()` so the two can never resolve the font
    // differently.
    void applyFont();

    // (Re)reads the theme's terminal palette into `bgColor_`/
    // `selectionColor_`/`cursorColor_` and this widget's own backdrop.
    // Shared the same way `applyFont()` is.
    void applyPalette();

    // Pixel -> cell arithmetic, the one translation this view legitimately
    // owns: which cell a position lands in (clamped to the grid), and
    // whether it landed on that cell's right half (which decides whether
    // the cell itself is part of a selection).
    QPoint cellAt(const QPoint &pos) const;
    bool rightHalf(const QPoint &pos) const;

    // Translate one Qt key event to `FfiTerminalKey` + code point and hand
    // it to `supervisor_->sendKey()` — pure enum/modifier translation (a
    // humble view concern); the xterm escape-sequence encoding itself lives
    // in `terminal_core::keys::encode` (see the .cpp's doc comment).
    // Returns false for a key this widget doesn't translate (e.g. a bare
    // modifier press), leaving it for `QWidget::keyPressEvent`.
    bool sendTranslatedKey(QKeyEvent *event);

    // Copy the current selection to the clipboard, and open the hovered
    // link — both no-ops when there is nothing to act on.
    void copySelection();
    void pasteClipboard();
    void openLink(const FfiTerminalLink &link);

    // Scrollback (T5). `layoutScrollBar` positions it against the widget's
    // right edge (called from `resizeEvent`, alongside the grid-size sync);
    // `refreshScrollState` re-reads `supervisor_->scrollState()` and updates
    // the bar's range/value without re-entering `onScrollBarValueChanged`
    // (it blocks the bar's own signal while doing so — otherwise dragging
    // the thumb and a `gridUpdated`-driven refresh would fight each other).
    // `scrollLines` is the one place wheel/Shift+PgUp/PgDn/Home/End funnel
    // through: it calls the FFI scroll, marks the snapshot stale, refreshes
    // the bar, and repaints.
    void layoutScrollBar();
    void refreshScrollState();
    void scrollLines(int delta);
    void onScrollBarValueChanged(int value);

    // Refresh `hoverLink_` for a mouse position, repainting when the
    // hovered span changed. Links only light up while Ctrl is held, so a
    // plain drag over output never turns into a link gesture.
    void updateHoverLink(const QPoint &pos, bool ctrlHeld);

    // A cell's resolved paint style: fg/bg with `inverse` and the selection
    // tint already folded in, plus the flags a run boundary is drawn on.
    // `row`/`col` are only needed to compare against the cursor position.
    CellStyle styleFor(const FfiTerminalCell &cell, quint32 row, quint32 col, quint32 cursorRow,
                        quint32 cursorCol) const;

    // The cached QFont matching a run's weight/slant — built once in the
    // constructor rather than constructed per run.
    const QFont &fontFor(bool bold, bool italic) const;

    // Fill `rect` with `bg` (skipped when `bg` already matches the widget's
    // black backdrop, unless `forceFill` — the cursor block must always be
    // drawn even if it happens to equal that colour) and, unless `text` is
    // empty or all spaces, draw it in `fg` with its baseline at `baselineY`;
    // draw a one-pixel underline when `underline`.
    void paintRunBody(QPainter &painter, const QColor &fg, const QColor &bg, bool bold, bool italic,
                       bool underline, const QRect &rect, qreal baselineY, const std::u32string &text,
                       bool forceFill);

    TerminalSupervisor *supervisor_;
    OpenAt openAt_;
    quint64 sessionId_;
    QString shellId_;
    AppSettings *appSettings_;
    QAction *copyAction_ = nullptr;
    QAction *pasteAction_ = nullptr;
    QFont font_;
    QFont fontBold_;
    QFont fontItalic_;
    QFont fontBoldItalic_;
    // Ctrl+wheel zoom (transient, per session): -1 defers to
    // `appSettings_->terminalFont().size`; a real point size overrides it
    // until the next `reapplyAppearance()` (Settings > Terminal OK), which
    // resets it so an explicit font change always wins.
    int fontSizeOverride_ = -1;
    qreal ascent_ = 0;
    int cellWidth_ = 1;
    int cellHeight_ = 1;
    quint32 rows_ = 0;
    quint32 cols_ = 0;
    // The widget's own backdrop and the selection/cursor tints (T3),
    // resolved from the active theme by `applyPalette()` — no longer the
    // hardcoded black/blue placeholders `styleFor()`/`paintEvent()` used to
    // paint with regardless of theme.
    QColor bgColor_{ Qt::black };
    QColor selectionColor_{ 38, 79, 150 };
    QColor cursorColor_{ Qt::white };
    bool started_ = false;
    bool dragging_ = false;
    // Time since the last double click, used to recognise the press that
    // follows it as a triple click (Qt has no triple-click event).
    QElapsedTimer doubleClickTimer_;
    // The link under the pointer, `found == false` when there is none.
    FfiTerminalLink hoverLink_{};

    // The last snapshot fetched from `supervisor_->snapshot()`, and whether
    // it is still current (T2). `paintEvent` re-fetches only when this is
    // true — set by `gridUpdated`, a selection change, a resize, and now a
    // scroll (T5), which is the whole reason this is a flag `paintEvent`
    // checks rather than an unconditional per-frame fetch.
    FfiTerminalSnapshot cachedSnapshot_{};
    bool snapshotStale_ = true;

    // Scrollback (T5): a plain vertical scrollbar this widget positions
    // itself in `layoutScrollBar` (not a `QAbstractScrollArea`, which this
    // custom-painted grid isn't one of).
    QScrollBar *scrollBar_ = nullptr;
};

} // namespace ui_shell
