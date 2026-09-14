#include "containers_menu.h"

#include "dock_layout.h"
#include "keymap_page.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>

namespace ui_shell {

void buildContainersMenu(QMainWindow *window, AppSettings *appSettings,
                         QHash<QString, QAction *> &actions, DockRegistry *docks,
                         QMenu *viewMenu, ProjectTreeModel *treeModel,
                         ContainerService *containerService)
{
    QAction *viewAction = registerAction(viewMenu, QStringLiteral("view.containers"),
                                         QObject::tr("Containers"), appSettings, actions);
    QObject::connect(viewAction, &QAction::triggered, window,
                     [docks]() { docks->show(QStringLiteral("containers")); });
    // C10 fix: `ContainerService` is built (and its dock's tree populated
    // from `nodes()`, empty) before any project is open, so a project's own
    // `[containers]` override (ADR-0022) never took effect until a manual
    // Refresh — the same lifecycle hook `runService->detectConfigurations()`
    // (run_menu.cpp) and `debugService->loadBreakpoints()` use, since all
    // three read project-scoped settings that only resolve once a project
    // is open.
    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, containerService,
                     [containerService](const QString &) { containerService->refreshAll(); });
}

} // namespace ui_shell
