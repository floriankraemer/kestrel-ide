#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>

class QAction;
class QMainWindow;
class QMenu;

namespace ui_shell {

class DockRegistry;

// Containers plan C2: `view.containers` on the existing View menu — the
// dock's own toolbar and context menus (`ContainersPanel`) cover every
// action, so like Tests there is only the show-the-dock entry. C10: also
// the one project-lifecycle hook the feature needs (see the .cpp).
void buildContainersMenu(QMainWindow *window, AppSettings *appSettings,
                         QHash<QString, QAction *> &actions, DockRegistry *docks,
                         QMenu *viewMenu, ProjectTreeModel *treeModel,
                         ContainerService *containerService);

} // namespace ui_shell
