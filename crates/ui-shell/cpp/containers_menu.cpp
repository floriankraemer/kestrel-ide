#include "containers_menu.h"

namespace ui_shell {

void wireContainersProjectHook(ProjectTreeModel *treeModel, ContainerService *containerService)
{
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
