#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>

class QAction;
class QMainWindow;
class QMenu;

namespace ui_shell {

class BuildPanel;
class DockRegistry;
class EditorTabs;
class RunConsolePanel;

// F4-12: the "&Run" menu (`run.run`/`run.stop`/`run.rerun`/
// `run.selectConfiguration`/`run.editConfigurations`) and `view.runConsole`
// on the existing View menu — its own translation unit for the same reason
// `vcs_menu.cpp` is one: `main_window.cpp` sits at its 1200-line ceiling
// (ADR-0025).
//
// The menu actions and the Run Console dock's own toolbar are two doors
// onto the same `RunToolbar` (`RunConsolePanel::runSelected`/`stopSelected`/
// `rerunSelected`/`focusConfigSelector`), so a shortcut acts on whatever
// configuration is selected regardless of which widget has focus — the
// same "same QAction, several triggers" shape the editor's context menu
// (`main_window.cpp`) already uses for navigate/refactor actions.
void buildRunMenu(QMainWindow *window, RunService *runService, RunConfigEditor *runConfigEditor,
                   AppSettings *appSettings, QHash<QString, QAction *> &actions,
                   DockRegistry *docks, RunConsolePanel *runConsolePanel,
                   ProjectTreeModel *treeModel, EditorTabs *editorTabs, BuildPanel *buildPanel,
                   QMenu *viewMenu, ContainerService *containerService,
                   ConsoleService *consoleService);

} // namespace ui_shell
