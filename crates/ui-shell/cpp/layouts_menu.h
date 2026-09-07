#pragma once

#include <QHash>
#include <QString>

class QAction;
class QMainWindow;
class QMenu;
class AppSettings;

namespace ads {
class CDockManager;
}

namespace ui_shell {

class DockRegistry;
class EditorTabs;

// The "Layouts" submenu under "&View": one entry per saved layout, plus
// Save Current Layout... and Delete Layout...
//
// A layout is the workspace arrangement — the dock state and the editor
// split grid — and deliberately not the open documents (ADR-0045), so
// applying one rearranges the window around whatever is open rather than
// replacing it.
//
// Its own translation unit for the same reason `help_menu.cpp`/`vcs_menu.cpp`
// are: `main_window.cpp` sits at its 1200-line ceiling (ADR-0025).
void buildLayoutsMenu(QMenu *viewMenu, QMainWindow *window, AppSettings *appSettings,
                       ads::CDockManager *dockManager, DockRegistry *docks,
                       EditorTabs *editorTabs, QHash<QString, QAction *> &actions);

} // namespace ui_shell
