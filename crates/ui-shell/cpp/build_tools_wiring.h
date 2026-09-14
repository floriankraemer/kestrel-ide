#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <functional>

class QAction;
class QMainWindow;
class QMenu;

namespace ads {
class CDockAreaWidget;
class CDockManager;
class CDockWidget;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class EditorTabs;

// Every touch point the jvm-build-tools plan's B1-B4/B6 phase needs beyond
// the Rust bridge itself, in one call from `main_window.cpp`: the dock
// (B2), the editor banner (B4) spliced above the existing editor widget,
// the View menu's `view.buildTools` entry, the Settings button's handler,
// and the two C++ relays (`watchedFileChanged`, and a save via
// `EditorTabs::setBuildToolsService`) — `main_window.cpp` sits at its own
// line-count baseline (ADR-0025), so this is the one call it makes rather
// than building any of this inline, the same split
// `containers_panel.cpp`/`tests_panel.cpp` already follow. Returns nothing:
// every collaborator the panel/menu/relays need past construction time is
// reached through the QObjects passed in here, not handed back out.
void wireBuildTools(ads::CDockManager *dockManager, DockRegistry *docks,
                     ads::CDockAreaWidget *rightArea, ads::CDockWidget *editorDock,
                     BuildToolsService *buildToolsService, RunService *runService,
                     ProjectTreeModel *treeModel, EditorTabs *editorTabs, QMainWindow *window,
                     AppSettings *appSettings, QHash<QString, QAction *> &actions, QMenu *viewMenu,
                     std::function<void()> openSettings);

} // namespace ui_shell
