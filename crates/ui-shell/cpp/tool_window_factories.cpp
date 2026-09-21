#include "tool_window_factories.h"

#include "build_tools_panel.h"
#include "build_tools_wiring.h"
#include "containers_panel.h"
#include "dock_layout.h"
#include "keymap_page.h"

#include "DockAreaWidget.h"

#include <QAction>
#include <QDebug>
#include <QMenu>

namespace ui_shell {

namespace {

// Neither `left` nor `center` has an area widget of its own today — only
// the right column and the bottom row do — so both fall back to `bottom`
// until a contribution actually needs one, at which point this switch
// grows a real case for it rather than guessing now.
ads::CDockAreaWidget *areaFor(FfiToolWindowArea area, ads::CDockAreaWidget *rightArea,
                              ads::CDockAreaWidget *bottomArea)
{
    return area == FfiToolWindowArea::Right ? rightArea : bottomArea;
}

} // namespace

ContributedToolWindows buildContributedToolWindows(
    AppSettings *appSettings, ads::CDockManager *dockManager, DockRegistry *docks,
    ads::CDockAreaWidget *rightArea, ads::CDockAreaWidget *bottomArea,
    ads::CDockWidget *editorDock, BuildToolsService *buildToolsService, RunService *runService,
    ProjectTreeModel *treeModel, EditorTabs *editorTabs, ContainerService *containerService,
    TerminalSupervisor *terminalSupervisor, ContainersPanel::OpenAt containersOpenAt)
{
    QHash<QString, DockFactory> factories;
    factories.insert(
      QStringLiteral("buildTools"),
      [dockManager, docks, editorDock, buildToolsService, runService, treeModel,
       editorTabs](ads::CDockAreaWidget *relativeTo) -> QWidget * {
          return wireBuildToolsDock(dockManager, docks, relativeTo, editorDock, buildToolsService,
                                    runService, treeModel, editorTabs);
      });
    factories.insert(
      QStringLiteral("containers"),
      [dockManager, docks, containerService, terminalSupervisor, appSettings,
       containersOpenAt](ads::CDockAreaWidget *relativeTo) -> QWidget * {
          return buildContainersDock(dockManager, docks, relativeTo, containerService,
                                     terminalSupervisor, appSettings, containersOpenAt);
      });

    ContributedToolWindows built;
    for (const FfiToolWindow &row : appSettings->contributedToolWindows()) {
        const auto factory = factories.constFind(row.id);
        if (factory == factories.constEnd()) {
            qWarning().noquote() << QStringLiteral("tool window %1 from %2 needs a native host")
                                       .arg(row.id, row.plugin_id);
            continue;
        }
        QWidget *panel = (*factory)(areaFor(row.area, rightArea, bottomArea));
        if (row.id == QStringLiteral("buildTools")) {
            built.buildTools = static_cast<BuildToolsPanel *>(panel);
        } else if (row.id == QStringLiteral("containers")) {
            built.containers = static_cast<ContainersPanel *>(panel);
        }
    }
    return built;
}

void wireContributedToolWindowMenus(AppSettings *appSettings, QHash<QString, QAction *> &actions,
                                    DockRegistry *docks, QMenu *viewMenu)
{
    for (const FfiToolWindow &row : appSettings->contributedToolWindows()) {
        QAction *action =
          registerAction(viewMenu, QStringLiteral("view.%1").arg(row.id), row.title, appSettings, actions);
        QObject::connect(action, &QAction::triggered, viewMenu,
                         [docks, id = row.id]() { docks->show(id); });
    }
}

} // namespace ui_shell
