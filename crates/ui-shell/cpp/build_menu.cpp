#include "build_menu.h"

#include "build_panel.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "keymap_page.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>

namespace ui_shell {

void buildBuildMenu(QMainWindow *window, BuildPanel *buildPanel, AppSettings *appSettings,
                     QHash<QString, QAction *> &actions, DockRegistry *docks, QMenu *viewMenu,
                     BuildToolsService *buildToolsService)
{
    QMenu *buildMenu = window->menuBar()->addMenu(QObject::tr("&Build"));
    // A top-level menu never goes through `exec()`, so these are the only
    // signal an E2E flow has that it is safe to send keystrokes into it —
    // the same markers "&Run" and "V&CS" carry.
    QObject::connect(buildMenu, &QMenu::aboutToShow, buildMenu,
                      []() { e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"build_menu\"}"); });
    QObject::connect(buildMenu, &QMenu::aboutToHide, buildMenu,
                      []() { e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"build_menu\"}"); });

    // Every entry shows the Build dock first: a build whose output is
    // invisible looks like nothing happened.
    const auto showDock = [docks]() { docks->show(QStringLiteral("build")); };

    QAction *buildAction = registerAction(buildMenu, QStringLiteral("build.build"),
                                          QObject::tr("Build Project"), appSettings, actions);
    QObject::connect(buildAction, &QAction::triggered, buildPanel, [buildPanel, showDock]() {
        showDock();
        buildPanel->buildProject();
    });

    QAction *rebuildAction = registerAction(buildMenu, QStringLiteral("build.rebuild"),
                                            QObject::tr("Rebuild Project"), appSettings, actions);
    QObject::connect(rebuildAction, &QAction::triggered, buildPanel, [buildPanel, showDock]() {
        showDock();
        buildPanel->rebuildProject();
    });

    QAction *stopAction = registerAction(buildMenu, QStringLiteral("build.stop"),
                                         QObject::tr("Stop Build"), appSettings, actions);
    QObject::connect(stopAction, &QAction::triggered, buildPanel,
                      [buildPanel]() { buildPanel->stopBuild(); });

    QAction *viewBuildAction = registerAction(viewMenu, QStringLiteral("view.build"),
                                              QObject::tr("Build"), appSettings, actions);
    QObject::connect(viewBuildAction, &QAction::triggered, window, showDock);

    // The jvm-build-tools plan's B4: reload the Gradle/Maven project model,
    // the same action the editor banner's "Reload" button triggers.
    buildMenu->addSeparator();
    QAction *reloadAction =
      registerAction(buildMenu, QStringLiteral("buildTools.reload"),
                     QObject::tr("Reload Build Tool Project"), appSettings, actions);
    QObject::connect(reloadAction, &QAction::triggered, buildToolsService,
                      [buildToolsService]() { buildToolsService->sync(); });
}

} // namespace ui_shell
