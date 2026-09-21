#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <functional>

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
// `wireBuildToolsSettings` (below) can reach it once `SettingsContext`
// exists, which `buildCentralWidget` does not have.
BuildToolsPanel *wireBuildToolsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *rightArea, ads::CDockWidget *editorDock,
                                     BuildToolsService *buildToolsService, RunService *runService,
                                     ProjectTreeModel *treeModel, EditorTabs *editorTabs);

// The dock's Settings button handler — called once from the View-menu
// section, the same place `central.containersPanel->setOpenSettingsHandler`
// already is. The `view.buildTools` action itself is no longer created
// here: `tool_window_factories.cpp`'s `wireContributedToolWindowMenus`
// covers every contributed tool window generically now (database-tools
// plan G1).
void wireBuildToolsSettings(BuildToolsPanel *panel, std::function<void()> openSettings);

} // namespace ui_shell
