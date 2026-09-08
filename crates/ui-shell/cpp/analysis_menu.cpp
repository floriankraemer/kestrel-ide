#include "analysis_menu.h"

#include "e2e_mark.h"
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
    // "Ana&lysis", not "&Analysis": "&AI" already claims Alt+A, and a menu
    // bar's ambiguous-mnemonic fallback (cycling on a repeated press) is not
    // something either a user or an E2E flow should have to rely on to
    // reach this menu — the same reasoning `vcs_menu.cpp` gives "V&CS".
    QMenu *analysisMenu = window->menuBar()->addMenu(QObject::tr("Ana&lysis"));
    // A top-level menu bar entry never goes through `exec()`, so
    // `aboutToShow`/`aboutToHide` are the only signal an E2E flow has that
    // it is safe to send keystrokes into what is, in X11 terms, a brand new
    // toplevel — same as `vcs_menu.cpp`.
    QObject::connect(analysisMenu, &QMenu::aboutToShow, analysisMenu,
                      []() { e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"analysis_menu\"}"); });
    QObject::connect(analysisMenu, &QMenu::aboutToHide, analysisMenu,
                      []() { e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"analysis_menu\"}"); });
    // Its one entry's on-screen rect, so a flow clicks "Inspect Project" by
    // name rather than by counting arrow presses.
    e2eMarkMenuActions(analysisMenu, "analysis_menu_action");

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
