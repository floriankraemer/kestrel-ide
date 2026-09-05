#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

#include <functional>

class QAction;
class QMenu;
class QTabWidget;
class QToolButton;

namespace ui_shell {

// The Terminal dock (F4-14b): a `QTabWidget` with one `TerminalWidget` per
// open session, all backed by the one `TerminalSupervisor` QObject
// (`TerminalWidget`'s doc comment explains why one adapter, not N — cxx-qt
// registers a `#[qobject]` type's QMetaObject once, so the multiplicity
// this task asked for lives in the session-id-keyed map on the Rust side,
// not in N QObject instances).
//
// Same "a dock holding a QTabWidget, one tab per backend session, tabs
// created/destroyed as sessions start/stop" shape `RunConsolePanel`
// (F4-11) already established for the Run dock — this class mirrors it,
// with one deliberate difference: a run console tab is left open after its
// process exits (F4-11's own review requirement), but a terminal tab *is*
// its session — closing the tab is the only way to end one, so the two
// stay in lock-step here instead.
//
// The "+" button is a split button: clicking it opens the configured
// default shell, and its dropdown lists every shell this machine offers —
// PowerShell, cmd and each WSL distro on Windows, `$SHELL` and the entries
// of `/etc/shells` elsewhere. The list, its labels and its order all come
// from `TerminalSupervisor::availableShells()`; this class only renders
// them and hands the chosen id back, so no shell knowledge lives in C++.
class TerminalSessionsPanel : public QWidget
{
    Q_OBJECT

public:
    // `openAt` is passed on to every tab: a `file:line` printed in a
    // terminal opens in the editor, exactly as one printed in a run console
    // does (R2-6).
    using OpenAt = std::function<void(const QString &, int, int)>;

    TerminalSessionsPanel(TerminalSupervisor *supervisor, AppSettings *appSettings, OpenAt openAt,
                           QWidget *parent = nullptr);

    // Open a new tab and give it focus — the target of both the "+" button
    // and the `terminal.newSession` action (Ctrl+Shift+T). An empty
    // `shellId` means the configured default; anything else is one of
    // `availableShells()`' ids, forwarded verbatim. `label` names the tab
    // when the shell was picked from the dropdown, and is empty for the
    // default, which keeps the plain "Terminal N" counter.
    void addSession(const QString &shellId = QString(), const QString &label = QString());

    // Focus the current tab's terminal, for the `view.terminal` action.
    void focusCurrent();

    // Re-apply Copy/Paste's shortcuts to every open tab, and this panel's own
    // `newSession` shortcut, after a keymap rebind (Settings > Keymap > OK).
    void reapplyKeymap();

    // `terminal.newSession` (Ctrl+Shift+T): one QAction on the panel itself,
    // not per tab, so — unlike Copy/Paste — it is long-lived enough to sit
    // in the app-wide `actions` map `main_window.cpp` builds for Settings >
    // Keymap and `applyKeymap()`.
    QAction *newSessionAction() const { return newSessionAction_; }

    // `terminal.selectShell`: drops the "+" button's shell menu open from
    // the keyboard. Long-lived like `newSessionAction_` and registered in
    // the same app-wide map, so Settings > Keymap can rebind it.
    QAction *selectShellAction() const { return selectShellAction_; }

private:
    void closeTab(int index);

    // Rebuild the "+" button's menu from `availableShells()`. Called each
    // time the menu is about to show rather than once at construction: a
    // WSL distro installed while the IDE is running should appear without
    // a restart.
    void refreshShellMenu();

    TerminalSupervisor *supervisor_;
    AppSettings *appSettings_;
    OpenAt openAt_;
    QTabWidget *tabs_ = nullptr;
    QAction *newSessionAction_ = nullptr;
    QAction *selectShellAction_ = nullptr;
    QToolButton *newTabButton_ = nullptr;
    QMenu *shellMenu_ = nullptr;
    int sessionCounter_ = 0;
};

} // namespace ui_shell
