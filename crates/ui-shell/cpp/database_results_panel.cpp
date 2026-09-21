#include "database_results_panel.h"

#include "database_console_bar.h"
#include "dock_layout.h"
#include "result_grid_view.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QPlainTextEdit>
#include <QTabWidget>
#include <QVBoxLayout>

namespace ui_shell {

DatabaseResultsPanel::DatabaseResultsPanel(EditorTabs *editorTabs, ConsoleService *consoleService,
                                           ResultProvider *resultProvider,
                                           AppSettings *appSettings, QWidget *parent)
  : QWidget(parent)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);

    bar_ = mountDatabaseConsoleBar(editorTabs, consoleService, appSettings, this);
    layout->addWidget(bar_);

    tabs_ = new QTabWidget(this);
    output_ = new QPlainTextEdit(this);
    output_->setReadOnly(true);
    tabs_->addTab(output_, tr("Output"));
    grid_ = new ResultGridView(resultProvider, this);
    tabs_->addTab(grid_, tr("Result"));
    layout->addWidget(tabs_, 1);

    connect(consoleService, &ConsoleService::outputAppended, this,
            &DatabaseResultsPanel::appendOutput);
    connect(consoleService, &ConsoleService::executionStarted, this,
            &DatabaseResultsPanel::onExecutionStarted);
    connect(consoleService, &ConsoleService::rowsAppended, this,
            [this](quint64 resultId, quint64 first, quint64 count) {
                grid_->rowsAppended(resultId, first, count);
            });
    connect(consoleService, &ConsoleService::executionFinished, this,
            &DatabaseResultsPanel::onExecutionFinished);
    connect(consoleService, &ConsoleService::askContinue, this,
            [consoleService](quint64 resultId, quint64) {
                // ponytail: no confirmation dialog yet — a script under the
                // `Ask` policy always continues rather than pausing for a
                // user decision. Wire a `QMessageBox` here once F3.3's
                // `Ask` policy needs a real UI, `resume(resultId, true)`
                // already does the right thing either way.
                consoleService->resume(resultId, true);
            });
}

void DatabaseResultsPanel::appendOutput(quint64 tabId, const QString &text)
{
    Q_UNUSED(tabId);
    output_->appendPlainText(text);
}

void DatabaseResultsPanel::onExecutionStarted(quint64 tabId, quint64 resultId, quint32 index,
                                              quint32 count)
{
    Q_UNUSED(tabId);
    Q_UNUSED(index);
    Q_UNUSED(count);
    grid_->setResultId(resultId);
    tabs_->setCurrentWidget(grid_);
}

void DatabaseResultsPanel::onExecutionFinished(quint64 resultId, bool ok, quint64 affected,
                                               quint64 elapsedMs, FfiDbError error)
{
    grid_->executionFinished(resultId, ok, affected, elapsedMs);
    if (!ok) {
        output_->appendPlainText(QString(error.message));
    }
}

DatabaseResultsPanel *buildDatabaseResultsDock(ads::CDockManager *dockManager,
                                               DockRegistry *docks,
                                               ads::CDockAreaWidget *relativeTo,
                                               EditorTabs *editorTabs,
                                               ConsoleService *consoleService,
                                               ResultProvider *resultProvider,
                                               AppSettings *appSettings)
{
    auto *panel =
      new DatabaseResultsPanel(editorTabs, consoleService, resultProvider, appSettings,
                               dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Database Results"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("databaseResults"), dock, ads::BottomDockWidgetArea,
                        relativeTo);
    docks->hide(QStringLiteral("databaseResults"));
    return panel;
}

} // namespace ui_shell
