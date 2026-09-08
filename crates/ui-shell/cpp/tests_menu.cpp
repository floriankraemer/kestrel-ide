#include "tests_menu.h"

#include "dock_layout.h"
#include "keymap_page.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>

namespace ui_shell {

void buildTestsMenu(QMainWindow *window, AppSettings *appSettings,
                     QHash<QString, QAction *> &actions, DockRegistry *docks, QMenu *viewMenu)
{
    QAction *viewTestsAction = registerAction(viewMenu, QStringLiteral("view.tests"),
                                              QObject::tr("Tests"), appSettings, actions);
    QObject::connect(viewTestsAction, &QAction::triggered, window,
                      [docks]() { docks->show(QStringLiteral("tests")); });
}

} // namespace ui_shell
