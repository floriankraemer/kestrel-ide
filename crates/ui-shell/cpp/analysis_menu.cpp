#include "analysis_menu.h"

#include "keymap_page.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QMessageBox>

namespace ui_shell {

void buildAnalysisMenu(QMainWindow *window, AnalysisService *analysisService,
                       AppSettings *appSettings, QHash<QString, QAction *> &actions)
{
    QMenu *analysisMenu = window->menuBar()->addMenu(QObject::tr("&Analysis"));

    QAction *inspectAction =
      registerAction(analysisMenu, QStringLiteral("analysis.inspectProject"),
                     QObject::tr("Inspect Project"), appSettings, actions);

    QObject::connect(inspectAction, &QAction::triggered, analysisService,
                      [window, analysisService]() {
                          const FfiResult result = analysisService->inspectProject();
                          if (result.code != 0) {
                              QMessageBox::warning(window, QObject::tr("Inspect Project"),
                                                   result.message);
                          }
                      });

    // Disabled for the run's duration: `Scheduler::run_manual` already
    // refuses an overlapping batch (`inspectProject` would just report
    // "already running" instead), but greying the action out is the
    // difference between that being a rule the user reads about and one
    // the menu shows them.
    QObject::connect(analysisService, &AnalysisService::analysisStarted, inspectAction,
                      [inspectAction]() { inspectAction->setEnabled(false); });
    QObject::connect(analysisService, &AnalysisService::analysisFinished, inspectAction,
                      [inspectAction]() { inspectAction->setEnabled(true); });
}

} // namespace ui_shell
