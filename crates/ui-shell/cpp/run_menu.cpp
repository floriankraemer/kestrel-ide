#include "run_menu.h"

#include "build_panel.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "keymap_page.h"
#include "run_config_dialog.h"
#include "code_editor.h"
#include "editor_tabs.h"
#include "run_console_panel.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QStringList>

namespace ui_shell {

void buildRunMenu(QMainWindow *window, RunService *runService, RunConfigEditor *runConfigEditor,
                   AppSettings *appSettings, QHash<QString, QAction *> &actions,
                   DockRegistry *docks, RunConsolePanel *runConsolePanel,
                   ProjectTreeModel *treeModel, EditorTabs *editorTabs, BuildPanel *buildPanel,
                   QMenu *viewMenu, ContainerService *containerService,
                   ConsoleService *consoleService)
{
    // Detect run configurations (Cargo.toml, package.json, Makefile) the
    // moment a project opens, same lifecycle hook LanguageService and the
    // search index already key off of — the picker has something to run
    // without the user asking for a scan first.
    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, runService,
                      [runService](const QString &) { runService->detectConfigurations(); });

    // Whatever launched it, a started console is worth looking at — reveal
    // the dock without stealing focus from what the user was already doing
    // any more forcefully than opening a new tab in it does.
    QObject::connect(runService, &RunService::consoleStarted, window,
                      [docks](quint64, const QString &) {
                          docks->show(QStringLiteral("runConsole"));
                      });

    // A before-launch task's output belongs in the Build dock: it is a
    // build (or a tool run like one), and a second place to look for it
    // would be a second place to look (B2-2).
    QObject::connect(runService, &RunService::beforeLaunchStarted, buildPanel,
                      [docks, buildPanel](const QString &, const QString &label) {
                          docks->show(QStringLiteral("build"));
                          buildPanel->showExternalTask(label);
                      });
    QObject::connect(runService, &RunService::beforeLaunchOutput, buildPanel,
                      [buildPanel](const QString &, const QString &text) {
                          buildPanel->appendExternalOutput(text);
                      });
    QObject::connect(runService, &RunService::beforeLaunchFailed, buildPanel,
                      [buildPanel](const QString &, const FfiResult &error) {
                          buildPanel->reportExternalFailure(QString(error.message));
                      });

    QMenu *runMenu = window->menuBar()->addMenu(QObject::tr("&Run"));
    // A top-level menu bar entry never goes through `exec()`, so
    // `aboutToShow`/`aboutToHide` are the only signal an E2E flow has that
    // it is safe to send keystrokes into it — same reasoning as
    // `vcs_menu.cpp`'s "V&CS" markers.
    QObject::connect(runMenu, &QMenu::aboutToShow, runMenu, [runMenu]() {
        // The item labels ride along with the marker so an E2E flow can
        // count Downs to a named item instead of to a hard-coded index —
        // adding a menu entry used to silently break whichever flow walked
        // past it (R2-5 added one and did exactly that).
        QStringList items;
        for (const QAction *action : runMenu->actions()) {
            items.append(e2eJson(action->text()));
        }
        e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"run_menu\",\"items\":[%1]}")
                  .arg(items.join(QStringLiteral(","))));
    });
    QObject::connect(runMenu, &QMenu::aboutToHide, runMenu,
                      []() { e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"run_menu\"}"); });

    QAction *runAction = registerAction(runMenu, QStringLiteral("run.run"), QObject::tr("Run"),
                                        appSettings, actions);
    QObject::connect(runAction, &QAction::triggered, runConsolePanel,
                      [runConsolePanel]() { runConsolePanel->runSelected(); });

    // IntelliJ's "run context configuration": run whatever the focused
    // editor's file is, the keyboard door onto the same gutter icon
    // `editor_tabs_run.cpp` wires up.
    QAction *runContextAction = registerAction(runMenu, QStringLiteral("run.runContext"),
                                                QObject::tr("Run File"), appSettings, actions);
    QObject::connect(runContextAction, &QAction::triggered, editorTabs, [editorTabs]() {
        editorTabs->requestRunFor(qobject_cast<CodeEditor *>(editorTabs->currentEditor()));
    });

    QAction *stopAction = registerAction(runMenu, QStringLiteral("run.stop"), QObject::tr("Stop"),
                                         appSettings, actions);
    QObject::connect(stopAction, &QAction::triggered, runConsolePanel,
                      [runConsolePanel]() { runConsolePanel->stopSelected(); });

    // Stop asks; Kill does not wait to be asked twice (R2-4).
    QAction *killAction = registerAction(runMenu, QStringLiteral("run.kill"), QObject::tr("Kill"),
                                         appSettings, actions);
    QObject::connect(killAction, &QAction::triggered, runConsolePanel,
                      [runConsolePanel]() { runConsolePanel->killSelected(); });

    QAction *rerunAction = registerAction(runMenu, QStringLiteral("run.rerun"),
                                          QObject::tr("Rerun"), appSettings, actions);
    QObject::connect(rerunAction, &QAction::triggered, runConsolePanel,
                      [runConsolePanel]() { runConsolePanel->rerunSelected(); });

    QAction *selectConfigAction =
      registerAction(runMenu, QStringLiteral("run.selectConfiguration"),
                      QObject::tr("Select Run Configuration..."), appSettings, actions);
    QObject::connect(selectConfigAction, &QAction::triggered, runConsolePanel, [docks,
                                                                                runConsolePanel]() {
        docks->show(QStringLiteral("runConsole"));
        runConsolePanel->focusConfigSelector();
    });

    // IntelliJ's Show Running List: which of this session's consoles are
    // still alive, and a way back to each one's tab (R2-5).
    QAction *runningListAction =
      registerAction(runMenu, QStringLiteral("run.showRunningList"),
                      QObject::tr("Show Running List"), appSettings, actions);
    QObject::connect(runningListAction, &QAction::triggered, runConsolePanel,
                      [docks, runConsolePanel]() {
                          docks->show(QStringLiteral("runConsole"));
                          runConsolePanel->showRunningList();
                      });

    QAction *editConfigsAction =
      registerAction(runMenu, QStringLiteral("run.editConfigurations"),
                      QObject::tr("Edit Configurations..."), appSettings, actions);
    QObject::connect(editConfigsAction, &QAction::triggered, window,
                      [window, runConfigEditor, containerService, consoleService]() {
                          showRunConfigDialog(window, runConfigEditor, containerService,
                                             QString(), consoleService);
                      });

    QAction *viewRunConsoleAction =
      registerAction(viewMenu, QStringLiteral("view.runConsole"), QObject::tr("Run Console"),
                      appSettings, actions);
    QObject::connect(viewRunConsoleAction, &QAction::triggered, window,
                      [docks]() { docks->show(QStringLiteral("runConsole")); });
}

} // namespace ui_shell
