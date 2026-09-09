#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QPoint>
#include <QString>

class AppSettings;
class QAction;
class QMainWindow;
class QMenu;
class QStatusBar;
class QToolButton;
class QWidget;

namespace ui_shell {

class DockRegistry;
class EditorTabs;
class FileHistoryPanel;

// F3-19: the VCS menu, its twelve `ActionDef`s, and F3-18's status-bar
// branch widget/branch menu. Its own translation unit for the reason
// `buildEditingActions` is one: `main_window.cpp` sits close to its
// 1200-line ceiling (ADR-0025), and every entry here does the same kind of
// thing — hand `VcsService` a call and let its signals refresh the UI.
//
// Not built conditionally on `isRepository()`: a project's Git-ness is
// known only asynchronously (after `openProject` answers), well after menu
// construction, and by the time it changes again (a different project
// opened) tearing down and rebuilding a `QMenu` is more machinery than
// disabling one. So the menu and the branch widget both grey themselves out
// on `repositoryChanged` instead of never existing — same "no Git UI on a
// non-repository project" intent the plan asks for, reached through a
// toggle rather than through construction. The Changes/File History docks
// follow the same rule via `hide`/`show` (`registerVcsDocks`).
// `view.changes`/`view.vcsHistory` are registered onto `viewMenu` (the
// existing View menu every other dock's show-action lives on) rather than a
// second menu of their own.
void buildVcsMenu(QMainWindow *window, VcsService *vcsService, AppSettings *appSettings,
                   QHash<QString, QAction *> &actions, EditorTabs *editorTabs,
                   DockRegistry *docks, FileHistoryPanel *fileHistoryPanel, QMenu *viewMenu);

// F3-18: the status-bar branch widget — current branch name, click opens
// the branch menu (checkout / New Branch... / Delete Branch...). Returns
// the button so the caller can add it to the status bar alongside the
// others.
QToolButton *buildBranchWidget(VcsService *vcsService, QWidget *window, QStatusBar *statusBar);

// The branch menu itself: checkout / New Branch... / Delete Branch..., for
// `anchor`, popped up at `globalPos`. Promoted out of `vcs_menu.cpp` (was
// `static`) for G6: the Changes dock toolbar's branch chip opens the same
// menu `buildBranchWidget`'s status-bar chip and the VCS menu's
// "Branches..." action already do, rather than a second copy of the
// checkout/create/delete wiring.
void showBranchMenu(VcsService *vcsService, QWidget *anchor, const QPoint &globalPos);

} // namespace ui_shell
