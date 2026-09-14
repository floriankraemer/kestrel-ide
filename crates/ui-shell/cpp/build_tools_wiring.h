#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <functional>

class QAction;
class QMainWindow;
class QMenu;
class QWidget;

namespace ads {
class CDockAreaWidget;
class CDockManager;
class CDockWidget;
} // namespace ads

namespace ui_shell {

class BuildToolsPanel;
class DockRegistry;
class EditorTabs;

// The editor dock's widget, wrapped once and for all in a plain container
// with room above `editorRoot` for `wireBuildToolsDock`'s banner (B4) to
// insert into later — `main_window.cpp` calls this instead of handing
// `editorRoot` to `editorDock->setWidget()` directly, so that call never
// has to happen a second time. See `insertEditorBanner`'s own comment
// (`build_tools_wiring.cpp`) for why a second `setWidget()` on this,
// ADS's *central* dock, is the thing this avoids.
QWidget *wrapEditorDockContent(QWidget *editorRoot);

// The dock (B2), the editor banner (B4) spliced above the existing editor
// widget, and the two C++ relays (`watchedFileChanged`, and a save via
// `EditorTabs::setBuildToolsService`) — everything `buildCentralWidget`
// already has the collaborators for, called from inside it exactly where
// `buildContainersDock` is (`main_window.cpp`'s own footprint stays one
// call plus one `CentralWidgets` field, `buildToolsPanel`, the same shape
// `containersPanel` already has). Returns the panel so
// `wireBuildToolsMenuAndSettings` (below) can reach it once the View menu
// and `SettingsContext` exist, which `buildCentralWidget` does not have.
BuildToolsPanel *wireBuildToolsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *rightArea, ads::CDockWidget *editorDock,
                                     BuildToolsService *buildToolsService, RunService *runService,
                                     ProjectTreeModel *treeModel, EditorTabs *editorTabs);

// The View menu's `view.buildTools` entry and the dock's Settings button
// handler — called once from the View-menu section, the same place
// `buildContainersMenu`/`central.containersPanel->setOpenSettingsHandler`
// already are.
void wireBuildToolsMenuAndSettings(QMainWindow *window, AppSettings *appSettings,
                                    QHash<QString, QAction *> &actions, DockRegistry *docks,
                                    QMenu *viewMenu, BuildToolsPanel *panel,
                                    std::function<void()> openSettings);

} // namespace ui_shell
