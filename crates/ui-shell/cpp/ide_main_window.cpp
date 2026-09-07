#include "ide_main_window.h"

#include "editor_tabs.h"

#include "DockManager.h"

#include <QCloseEvent>
#include <QCoreApplication>
#include <QKeyEvent>
#include <QRect>
#include <QString>

#ifdef Q_OS_WIN
#include <dwmapi.h>
#endif

namespace ui_shell {

IdeMainWindow::IdeMainWindow()
{
    qApp->installEventFilter(this);
}

void IdeMainWindow::keyPressEvent(QKeyEvent *event)
{
    static constexpr int kDoubleShiftMs = 300;
    // A Shift held together with another modifier is part of a
    // shortcut, not a gesture: Ctrl+Shift+N followed within the window by
    // any capital letter would otherwise open the popup a second time.
    const bool bareShift = (event->modifiers() & ~Qt::ShiftModifier) == Qt::NoModifier;
    if (event->key() == Qt::Key_Shift && !event->isAutoRepeat() && bareShift) {
        if (lastShift_.isValid() && lastShift_.elapsed() < kDoubleShiftMs) {
            lastShift_.invalidate();
            if (searchEverywhere_) {
                searchEverywhere_();
            }
            return;
        }
        lastShift_.start();
    }
    QMainWindow::keyPressEvent(event);
}

bool IdeMainWindow::eventFilter(QObject *watched, QEvent *event)
{
    if (event->type() == QEvent::KeyPress) {
        auto *keyEvent = static_cast<QKeyEvent *>(event);
        const bool bareShift = (keyEvent->modifiers() & ~Qt::ShiftModifier) == Qt::NoModifier;
        const bool isGestureShift =
          keyEvent->key() == Qt::Key_Shift && !keyEvent->isAutoRepeat() && bareShift;
        if (!isGestureShift && !keyEvent->text().isEmpty()) {
            // Any real keystroke between the two Shift presses means the
            // user was typing, not gesturing — caught here rather than only
            // in keyPressEvent() above because a widget that fully consumes
            // the key (the terminal, the code editor) never forwards it to
            // this window's own override (#116).
            lastShift_.invalidate();
        }
    }
    return QMainWindow::eventFilter(watched, event);
}

void IdeMainWindow::closeEvent(QCloseEvent *event)
{
    if (editorTabs_ && !editorTabs_->confirmCloseAllTabs()) {
        event->ignore();
        return;
    }
    if (appSettings_) {
        // normalGeometry(), not geometry(): a maximised or minimised
        // window reports its current screen rect (0x0 while minimised),
        // and restoring that is not what the user last sized the window
        // to. Rust drops a rect it cannot use.
        const QRect g = normalGeometry();
        appSettings_->saveWindowGeometry(g.x(), g.y(), static_cast<quint32>(qMax(0, g.width())),
                                          static_cast<quint32>(qMax(0, g.height())));
        // The other half of that decision: the rect above is deliberately the
        // *normal* one, so without this flag a maximised window reopens at
        // the size it would un-maximise to.
        appSettings_->saveWindowMaximized(isMaximized() || isFullScreen());
        if (dockManager_) {
            // D4: window_state is a plain Rust String (must be valid
            // UTF-8); ADS's saveState() returns raw QByteArray, so
            // base64 round-trips it through that constraint.
            const QString state =
              QString::fromLatin1(dockManager_->saveState().toBase64());
            appSettings_->saveWindowState(state);
        }
        if (editorTabs_) {
            // The editor split layout is the view's own JSON (ADS knows
            // nothing about the splitter tree inside the editor dock).
            appSettings_->saveEditorLayout(editorTabs_->saveLayout());
        }
    }
    if (docManager_) {
        // Takes the discovery file with it — one left behind points the
        // next agent that reads it at a port nothing answers on.
        docManager_->shutdownMcpServer();
    }
    QMainWindow::closeEvent(event);
}

void showRestored(QMainWindow *window, AppSettings *appSettings)
{
    // The reading half of `closeEvent`'s `saveWindowMaximized` above, and it
    // lives beside it so the two cannot drift: the geometry has already been
    // applied by the time this runs and stays the window's *normal* rect, so
    // showing maximised here still leaves un-maximising on the size the user
    // last dragged the window to.
    if (appSettings && appSettings->windowMaximized()) {
        window->showMaximized();
        return;
    }
    window->show();
}

void applyNativeWindowChrome(QMainWindow *window)
{
#ifdef Q_OS_WIN
    auto hwnd = reinterpret_cast<HWND>(window->winId());
    DWM_WINDOW_CORNER_PREFERENCE preference = DWMWCP_ROUND;
    DwmSetWindowAttribute(
      hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &preference, sizeof(preference));
#else
    Q_UNUSED(window);
#endif
}

} // namespace ui_shell
