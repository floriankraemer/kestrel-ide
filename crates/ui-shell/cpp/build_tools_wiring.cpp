#include "build_tools_wiring.h"

#include "build_tools_panel.h"
#include "dock_layout.h"
#include "editor_banner.h"
#include "editor_tabs.h"
#include "keymap_page.h"

#include "DockAreaWidget.h"
#include "DockWidget.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

// Splices `EditorBanner` above the editor area's existing content (B4)
// without main_window.cpp having to know this dock now wraps a container —
// `editorDock->widget()` before and after this call is what the caller
// keeps using either way.
void insertEditorBanner(ads::CDockWidget *editorDock, BuildToolsService *buildToolsService)
{
    QWidget *content = editorDock->widget();
    auto *container = new QWidget();
    auto *layout = new QVBoxLayout(container);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(new EditorBanner(buildToolsService, container));
    layout->addWidget(content, 1);
    editorDock->setWidget(container);
}

} // namespace

void wireBuildTools(ads::CDockManager *dockManager, DockRegistry *docks,
                     ads::CDockAreaWidget *rightArea, ads::CDockWidget *editorDock,
                     BuildToolsService *buildToolsService, RunService *runService,
                     ProjectTreeModel *treeModel, EditorTabs *editorTabs, QMainWindow *window,
                     AppSettings *appSettings, QHash<QString, QAction *> &actions, QMenu *viewMenu,
                     std::function<void()> openSettings)
{
    auto openAt = [editorTabs](const QString &path, int line, int column) {
        editorTabs->openFileAtLine(path, line, column);
    };
    BuildToolsPanel *panel =
      buildBuildToolsDock(dockManager, docks, rightArea, buildToolsService, runService, openAt);
    panel->setOpenSettingsHandler(std::move(openSettings));

    insertEditorBanner(editorDock, buildToolsService);

    QAction *viewAction = registerAction(viewMenu, QStringLiteral("view.buildTools"),
                                         QObject::tr("Build Tools"), appSettings, actions);
    QObject::connect(viewAction, &QAction::triggered, window,
                      [docks]() { docks->show(QStringLiteral("buildTools")); });

    editorTabs->setBuildToolsService(buildToolsService);

    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, buildToolsService,
                      [buildToolsService](const QString &root) {
                          buildToolsService->projectOpened(root);
                      });

    // A second, independent relay of the same watcher signal
    // `main_window.cpp` already relays to `LanguageService` — Qt signals
    // take any number of slots, so this needs no touch to that connection.
    QObject::connect(treeModel, &ProjectTreeModel::watchedFileChanged, buildToolsService,
                      [buildToolsService](const QString &path, qint32) {
                          buildToolsService->fileChanged(path);
                      });
}

} // namespace ui_shell
