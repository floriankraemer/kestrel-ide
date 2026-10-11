#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCoreApplication>
#include <QHash>
#include <QString>
#include <QWidget>

#include <functional>

class QCheckBox;
class QLabel;
class QLineEdit;
class QPlainTextEdit;
class QTabWidget;
class QToolButton;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class RunToolbar;

// The Run Console dock (F4-11): a `RunToolbar` (configuration picker,
// Run/Stop/Rerun) over a `QTabWidget` with one read-only console per active
// or finished run.
//
// Humble view: what output a console produced, what `file:line` a position
// in it resolves to, where a search term occurs, and which consoles are
// still running are all `RunService`/`run-core` calls (`consoleOutput`,
// `resolveLink`, `findInConsole`, `activeConsoles`); this widget only
// appends the text it is given to the right tab, paints the styling that
// came with it, and turns a resolved link into `openAt`.
//
// Two invariants hold the offsets together (R2-3): every character in a
// tab's document is a character `RunService` also holds, and this widget
// never trims its own document — it trims when `consoleTrimmed` says the
// cache did, by exactly as much.
class RunConsolePanel : public QWidget
{
    // Not a `Q_OBJECT` (no moc target), so `tr()` would resolve in a Qt base
    // class's context; this keeps it in this class's own, where the .ts entries live.
    Q_DECLARE_TR_FUNCTIONS(RunConsolePanel)

public:
    // No Q_OBJECT: this widget declares no signals or slots of its own —
    // it connects to `RunService`'s with member-function pointers, which
    // needs no moc registration.
    using OpenAt = std::function<void(const QString &, int, int)>;

    // `toolbar` is the main window's run toolbar (built and owned there);
    // this panel only forwards the `run.*` shortcuts to it.
    RunConsolePanel(RunService *runService, RunToolbar *toolbar, OpenAt openAt, QWidget *parent);

    // Forwarded to the embedded RunToolbar — the targets of the global
    // `run.*` shortcuts, which must act on the toolbar's current selection
    // regardless of which widget has focus.
    void runSelected();
    void stopSelected();
    void killSelected();
    void rerunSelected();
    // Makes `configId` the toolbar combo's selection.
    void selectConfiguration(const QString &configId);
    // Run the selected test configuration collecting coverage.
    void runSelectedWithCoverage();
    // D3-8: Debug the selected configuration, forwarded like the others so
    // the shortcut works regardless of focus.
    void debugSelected();
    void focusConfigSelector();

    // R2-3: open the find bar over the current console, focused and with
    // whatever was last searched for still in it.
    void showFindBar();
    // R2-5: a popup of every console this session started, running ones
    // first; picking one raises its tab.
    void showRunningList();

private:
    void onConsoleStarted(quint64 consoleId, const QString &configId);
    void onConsoleOutput(quint64 consoleId, const QString &text);
    void onConsoleTrimmed(quint64 consoleId, quint32 utf16Units);
    void onConsoleFinished(quint64 consoleId, int exitCode, bool escaped);
    void onLinkActivated(quint64 consoleId, int textPosition);

    void clearCurrentConsole();
    void togglePinned(bool pinned);
    void closeTab(int index);
    void runFind(int direction);
    void updateTabControls();

    struct ConsoleTab
    {
        QPlainTextEdit *edit;
        // A pinned tab keeps its scrollback: it has no close button, and
        // "close others" leaves it alone. IntelliJ's pin, one flag.
        bool pinned = false;
        bool finished = false;
    };

    // The console the user is looking at, or `nullptr` with no tabs open.
    ConsoleTab *currentTab();
    quint64 currentConsoleId() const;

    RunService *runService_;
    OpenAt openAt_;
    RunToolbar *toolbar_ = nullptr;
    QTabWidget *tabs_ = nullptr;
    QToolButton *pinButton_ = nullptr;
    QCheckBox *scrollLock_ = nullptr;
    QWidget *findBar_ = nullptr;
    QLineEdit *findField_ = nullptr;
    QLabel *findStatus_ = nullptr;

    QHash<quint64, ConsoleTab> consoles_;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"runConsole"`, exactly the pattern `ChangesPanel`/
// `FileHistoryPanel` follow in `main_window.cpp` — pulled out to one call so
// wiring a new dock there costs one line, not five (the file sits at its
// 1200-line ceiling, ADR-0025).
RunConsolePanel *buildRunConsoleDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *relativeTo, RunToolbar *toolbar,
                                     RunConsolePanel::OpenAt openAt);

} // namespace ui_shell
