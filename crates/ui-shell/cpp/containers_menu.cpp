#include "containers_menu.h"

#include "dock_layout.h"
#include "keymap_page.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>

namespace ui_shell {

void buildContainersMenu(QMainWindow *window, AppSettings *appSettings,
                         QHash<QString, QAction *> &actions, DockRegistry *docks,
                         QMenu *viewMenu)
{
    QAction *viewAction = registerAction(viewMenu, QStringLiteral("view.containers"),
                                         QObject::tr("Containers"), appSettings, actions);
    QObject::connect(viewAction, &QAction::triggered, window,
                     [docks]() { docks->show(QStringLiteral("containers")); });
}

} // namespace ui_shell
