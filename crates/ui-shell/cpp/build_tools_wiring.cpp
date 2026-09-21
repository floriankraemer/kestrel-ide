#include "build_tools_wiring.h"

#include "build_tools_panel.h"
#include "dock_layout.h"
#include "editor_banner.h"
#include "editor_tabs.h"

#include "DockAreaWidget.h"
#include "DockWidget.h"

#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

// Splices `EditorBanner` above the editor area's existing content (B4) by
// inserting into `wrapEditorDockContent`'s already-existing layout — never
// `editorDock->setWidget()` again (see that function's own comment for
// why a second call on this, ADS's *central* dock, is the thing this
// avoids: root cause of a real e2e regression, bisected and fixed).
void insertEditorBanner(ads::CDockWidget *editorDock, BuildToolsService *buildToolsService)
{
    auto *layout = qobject_cast<QVBoxLayout *>(editorDock->widget()->layout());
    Q_ASSERT(layout);
    layout->insertWidget(0, new EditorBanner(buildToolsService, editorDock->widget()));
}

} // namespace

QWidget *wrapEditorDockContent(QWidget *editorRoot)
{
    // A container `editorDock->setWidget()` (`main_window.cpp`) is given
    // exactly once, with room above `editorRoot` for `insertEditorBanner`
    // to splice into later. Debugging root cause: an earlier version built
    // this wrapper *inside* `insertEditorBanner` instead, and called
    // `editorDock->setWidget()` there a second time — swapping the widget
    // of ADS's already-`setCentralWidget`'d dock out from under it broke
    // its focus-restore-on-activate, so every e2e flow that typed right
    // after opening/switching a tab silently went nowhere (a
    // `setFocusProxy` on the wrapper didn't fix it: the damage was to
    // ADS's own bookkeeping, not the wrapper's focus policy). This dock's
    // widget must never be reassigned after construction.
    auto *container = new QWidget();
    auto *layout = new QVBoxLayout(container);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(editorRoot, 1);
    // Kept from the first fix attempt (4bc13c9): correct on its own
    // merits even though it wasn't what broke this — a plain QWidget takes
    // no focus of its own, so anything that asks *this* wrapper to take
    // focus should still reach the editor underneath.
    container->setFocusProxy(editorRoot);
    return container;
}

BuildToolsPanel *wireBuildToolsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *rightArea, ads::CDockWidget *editorDock,
                                     BuildToolsService *buildToolsService, RunService *runService,
                                     ProjectTreeModel *treeModel, EditorTabs *editorTabs)
{
    auto openAt = [editorTabs](const QString &path, int line, int column) {
        editorTabs->openFileAtLine(path, line, column);
    };
    BuildToolsPanel *panel =
      buildBuildToolsDock(dockManager, docks, rightArea, buildToolsService, runService, openAt);

    insertEditorBanner(editorDock, buildToolsService);
    editorTabs->setDocumentSavedCallback(
      [buildToolsService](const QString &path) { buildToolsService->fileSaved(path); });

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

    return panel;
}

void wireBuildToolsSettings(BuildToolsPanel *panel, std::function<void()> openSettings)
{
    panel->setOpenSettingsHandler(std::move(openSettings));
}

} // namespace ui_shell
