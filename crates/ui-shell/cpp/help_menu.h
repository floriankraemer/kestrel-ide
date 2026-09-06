#pragma once

#include <QHash>
#include <QString>

class QAction;
class QMainWindow;
class AppSettings;

namespace ui_shell {

// The "&Help" menu — About, and nothing else yet. Added last in the menu
// bar, where every desktop convention puts it.
//
// Its own translation unit for the same reason `vcs_menu.cpp`/`ai_menu.cpp`
// are: `main_window.cpp` sits at its 1200-line ceiling (ADR-0025).
void buildHelpMenu(QMainWindow *window, AppSettings *appSettings,
                    QHash<QString, QAction *> &actions);

} // namespace ui_shell
