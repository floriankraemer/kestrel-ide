#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QElapsedTimer>
#include <QMainWindow>
#include <functional>

class QCloseEvent;
class QKeyEvent;

namespace ads {
class CDockManager;
} // namespace ads

namespace ui_shell {

class EditorTabs;

// Subclassed so closeEvent() can run the same unsaved-changes prompt as
// closing a tab, and persist geometry + dock layout on close (L1, D4). No
// Q_OBJECT: overriding a virtual function needs no signals/slots/
// qobject_cast, so this adds no second moc target — eventFilter() below is
// likewise a plain QObject virtual.
class IdeMainWindow : public QMainWindow
{
public:
    IdeMainWindow();

    void setEditorTabs(EditorTabs *editorTabs) { editorTabs_ = editorTabs; }
    void setAppSettings(AppSettings *appSettings) { appSettings_ = appSettings; }
    void setDockManager(ads::CDockManager *dockManager) { dockManager_ = dockManager; }
    void setDocumentManager(DocumentManager *docManager) { docManager_ = docManager; }
    // Opens Search Everywhere. Set once the popup exists; until then the
    // double-Shift gesture is simply inert.
    void setSearchEverywhereTrigger(std::function<void()> trigger)
    {
        searchEverywhere_ = std::move(trigger);
    }

protected:
    // JetBrains' double-Shift gesture: two Shift presses inside
    // kDoubleShiftMs open Search Everywhere. Handled here rather than as a
    // QShortcut because a bare modifier is not a key sequence Qt can bind.
    void keyPressEvent(QKeyEvent *event) override;

    void closeEvent(QCloseEvent *event) override;

    // Resets the gesture on any keystroke reaching the app, not only ones
    // that bubble up to keyPressEvent() unconsumed (#116). The terminal and
    // the code editor both fully consume printable/shift-modified character
    // keys and never call the base class, so without this qApp-level watch,
    // two unrelated shift-modified characters typed quickly in either would
    // spuriously open Search Everywhere.
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    std::function<void()> searchEverywhere_;
    QElapsedTimer lastShift_;
    EditorTabs *editorTabs_ = nullptr;
    AppSettings *appSettings_ = nullptr;
    ads::CDockManager *dockManager_ = nullptr;
    DocumentManager *docManager_ = nullptr;
};

// Windows 11's DWM rounds a top-level window's corners and casts its drop
// shadow on its own — but only once the window has a native handle, so this
// has to run after show(), not during construction. No-op elsewhere (DWM
// setting, not app-drawn chrome, per ADR-0001's native-chrome constraint).
// Shows `window` the way it was last left — maximised, or at the geometry
// already applied to it. The counterpart to `IdeMainWindow::closeEvent`'s
// geometry/maximised save, and here rather than at the call site so the two
// halves of that decision stay in one file.
void showRestored(QMainWindow *window, AppSettings *appSettings);

void applyNativeWindowChrome(QMainWindow *window);

} // namespace ui_shell
