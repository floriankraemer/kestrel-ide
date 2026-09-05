#pragma once

#include <QElapsedTimer>
#include <QFont>
#include <QPoint>
#include <QWidget>

#include <functional>

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QAction;
class QContextMenuEvent;
class QEvent;
class QKeyEvent;
class QMouseEvent;
class QPaintEvent;
class QResizeEvent;
class QShowEvent;

namespace ui_shell {

// Embedded terminal dock widget (Task F3, multi-session since F4-14): a
// custom QWidget that paints the cell grid `TerminalSupervisor` (Rust:
// `pty_core::PtySession` + `terminal_core::TerminalEmulator`, one pair per
// `sessionId`) hands over for its one session, and forwards key events back
// to it. Humble view per CLAUDE.md's hard rule — VT100 interpretation and
// grid state live entirely in `terminal-core`/the bridge; this class only
// paints `gridCells(sessionId)`'s snapshot and translates key events to
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

private:
    // Recompute rows/cols from the widget's current pixel size and the
    // monospace font's cell metrics, and — if that changed the grid size —
    // either `start()` the session (first call) or `resize()` it.
    void syncGridSizeToWidget();

    // Pixel -> cell arithmetic, the one translation this view legitimately
    // owns: which cell a position lands in (clamped to the grid), and
    // whether it landed on that cell's right half (which decides whether
    // the cell itself is part of a selection).
    QPoint cellAt(const QPoint &pos) const;
    bool rightHalf(const QPoint &pos) const;

    // Copy the current selection to the clipboard, and open the hovered
    // link — both no-ops when there is nothing to act on.
    void copySelection();
    void pasteClipboard();
    void openLink(const FfiTerminalLink &link);

    // Refresh `hoverLink_` for a mouse position, repainting when the
    // hovered span changed. Links only light up while Ctrl is held, so a
    // plain drag over output never turns into a link gesture.
    void updateHoverLink(const QPoint &pos, bool ctrlHeld);

    TerminalSupervisor *supervisor_;
    OpenAt openAt_;
    quint64 sessionId_;
    QString shellId_;
    AppSettings *appSettings_;
    QAction *copyAction_ = nullptr;
    QAction *pasteAction_ = nullptr;
    QFont font_;
    int cellWidth_ = 1;
    int cellHeight_ = 1;
    quint32 rows_ = 0;
    quint32 cols_ = 0;
    bool started_ = false;
    bool dragging_ = false;
    // Time since the last double click, used to recognise the press that
    // follows it as a triple click (Qt has no triple-click event).
    QElapsedTimer doubleClickTimer_;
    // The link under the pointer, `found == false` when there is none.
    FfiTerminalLink hoverLink_{};
};

} // namespace ui_shell
