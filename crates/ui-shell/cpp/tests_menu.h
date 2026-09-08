#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>

class QAction;
class QMainWindow;
class QMenu;

namespace ui_shell {

class DockRegistry;

// The PHP tooling plan's D6: `view.tests` on the existing View menu — the
// Tests dock's own toolbar and context menu (`TestsPanel`) already cover
// run all/run failed/rerun one node, so there is no separate "&Tests" menu,
// only the show-the-dock entry every other dock gets.
void buildTestsMenu(QMainWindow *window, AppSettings *appSettings,
                     QHash<QString, QAction *> &actions, DockRegistry *docks, QMenu *viewMenu);

} // namespace ui_shell
