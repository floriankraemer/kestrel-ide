#pragma once

#include "appearance_page.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QMainWindow;
class QTreeView;

namespace ui_shell {

class DockRegistry;
class EditorTabs;
class ProblemsPanel;

// L3/Task L2/F3-18: the status bar's permanent widgets — line:col +
// language + encoding (fed via `EditorTabs::attachStatusBar`), the Problems
// counter, the VCS branch widget, and the background-indexing progress bar.
// Its own translation unit for the same reason `vcs_menu.cpp`/`run_menu.cpp`
// are: `main_window.cpp` sits at its 1200-line ceiling (ADR-0025).
//
// Returns the `UiFontTargets` that only exist once these widgets are built —
// `buildMainWindow` still needs them afterwards, for the initial
// `applyUiFontScales()` call this function already makes and for the
// Preferences dialog's `SettingsContext`, which re-applies them on a scale
// change.
UiFontTargets buildStatusBar(QMainWindow *window, AppSettings *appSettings,
                              LanguageService *languageService, BuildService *buildService,
                              DiagnosticsService *diagnosticsService, SearchModel *searchModel,
                              VcsService *vcsService, EditorTabs *editorTabs,
                              QTreeView *projectTree, DockRegistry *docks,
                              ProblemsPanel *problemsPanel, ProjectTreeModel *treeModel);

// Busy indication for an in-flight "Open Folder..."/Recent Projects open
// (ADR-0037: the walk itself runs off the Qt thread, so nothing else says a
// project is loading). Call at the moment `ProjectTreeModel::openFolder` is
// invoked; `buildStatusBar`'s own `projectOpened`/`projectOpenFailed`
// connections clear it automatically, so callers never have to.
void showProjectOpening(QMainWindow *window);

} // namespace ui_shell
