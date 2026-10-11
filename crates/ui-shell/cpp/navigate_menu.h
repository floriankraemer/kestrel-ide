#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>

#include <memory>

class QAction;
class QMainWindow;

namespace ui_shell {

class DockRegistry;
class EditorTabs;
class FindUsagesPanel;
class HierarchyPanel;

// N8: the "&Navigate" menu — Go to Declaration/Implementation/Interface,
// Find Usages, and the Back/Forward jump history. Every entry routes
// through one `DeclarationNavigator` (owned here) so there is a single
// place that turns a resolution result into a jump. Its own translation
// unit for the same reason `vcs_menu.cpp`/`run_menu.cpp` are:
// `main_window.cpp` sits at its 1200-line ceiling (ADR-0025).
void buildNavigateMenu(QMainWindow *window, LanguageService *languageService,
                        SearchModel *searchModel, EditorTabs *editorTabs, AppSettings *appSettings,
                        QHash<QString, QAction *> &actions, DockRegistry *docks,
                        FindUsagesPanel *findUsagesPanel, HierarchyPanel *hierarchyPanel,
                        TestService *testService);

// The editor's right-click menu: the navigation, refactoring and AI actions
// registered under `actions` (the same QActions as the menu bar's, so their
// shortcuts show and a rebind reaches both), and Go To > Test.
void wireEditorContextMenu(EditorTabs *editorTabs,
                           std::shared_ptr<QHash<QString, QAction *>> actions,
                           TestService *testService);

} // namespace ui_shell
