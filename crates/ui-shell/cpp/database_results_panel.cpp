#include "database_results_panel.h"

#include "database_console_bar.h"
#include "database_dialogs.h"
#include "dock_layout.h"
#include "editor_tabs.h"
#include "result_grid_view.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QApplication>
#include <QFileInfo>
#include <QPlainTextEdit>
#include <QTabWidget>
#include <QVBoxLayout>

namespace ui_shell {

DatabaseResultsPanel::DatabaseResultsPanel(EditorTabs *editorTabs, ConsoleService *consoleService,
                                           ResultProvider *resultProvider,
                                           AppSettings *appSettings, QWidget *parent)
  : QWidget(parent)
  , editorTabs_(editorTabs)
  , consoleService_(consoleService)
  , resultProvider_(resultProvider)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);

    bar_ = mountDatabaseConsoleBar(editorTabs, consoleService, appSettings, this);
    layout->addWidget(bar_);

    consoleTabs_ = new QTabWidget(this);
    layout->addWidget(consoleTabs_, 1);

    connect(consoleService, &ConsoleService::outputAppended, this,
            &DatabaseResultsPanel::appendOutput);
    connect(consoleService, &ConsoleService::executionStarted, this,
            &DatabaseResultsPanel::onExecutionStarted);
    connect(consoleService, &ConsoleService::rowsAppended, this,
            &DatabaseResultsPanel::onRowsAppended);
    connect(consoleService, &ConsoleService::executionFinished, this,
            &DatabaseResultsPanel::onExecutionFinished);
    connect(consoleService, &ConsoleService::askContinue, this,
            [this, consoleService](quint64 resultId, const QString &message) {
                showAskContinueDialog(consoleService, resultId, message, this);
            });

    // The page a tab's console writes into disappears the moment its own
    // editor tab does — `DocumentManager::tabClosed` is the one real Qt
    // signal `EditorTabs` itself has none of (its own doc comment).
    connect(editorTabs_->documentManager(), &DocumentManager::tabClosed, this,
            &DatabaseResultsPanel::closePage);

    // A second, independent subscriber to the app-wide focus signal, same
    // as `DatabaseConsoleBar`'s own — the current page tracks whichever
    // `.sql` tab last had focus.
    QObject::connect(qApp, &QApplication::focusChanged, this,
                      [this](QWidget *, QWidget *) {
                          followActiveTab(editorTabs_->currentTabId());
                      });
}

DatabaseResultsPanel::ConsolePage &DatabaseResultsPanel::pageFor(quint64 tabId)
{
    auto it = pages_.find(tabId);
    if (it != pages_.end()) {
        return it.value();
    }
    ConsolePage page;
    page.root = new QWidget(consoleTabs_);
    auto *layout = new QVBoxLayout(page.root);
    layout->setContentsMargins(0, 0, 0, 0);
    page.subTabs = new QTabWidget(page.root);
    page.output = new QPlainTextEdit(page.root);
    page.output->setReadOnly(true);
    page.subTabs->addTab(page.output, tr("Output"));
    page.grid = new ResultGridView(resultProvider_, page.root);
    page.subTabs->addTab(page.grid, tr("Result"));
    layout->addWidget(page.subTabs);

    const QString path = editorTabs_->documentManager()->tabPath(tabId);
    const QString title = QFileInfo(path).fileName();
    consoleTabs_->addTab(page.root, title.isEmpty() ? tr("Console") : title);

    return pages_.insert(tabId, page).value();
}

void DatabaseResultsPanel::closePage(quint64 tabId)
{
    consoleService_->detach(tabId);
    const auto it = pages_.find(tabId);
    if (it == pages_.end()) {
        return;
    }
    const int index = consoleTabs_->indexOf(it.value().root);
    if (index >= 0) {
        consoleTabs_->removeTab(index);
    }
    it.value().root->deleteLater();
    pages_.erase(it);
}

void DatabaseResultsPanel::followActiveTab(quint64 tabId)
{
    const auto it = pages_.find(tabId);
    if (it != pages_.end()) {
        consoleTabs_->setCurrentWidget(it.value().root);
    }
}

void DatabaseResultsPanel::appendOutput(quint64 tabId, const QString &text)
{
    pageFor(tabId).output->appendPlainText(text);
}

void DatabaseResultsPanel::onExecutionStarted(quint64 tabId, quint64 resultId, quint32 index,
                                              quint32 count)
{
    Q_UNUSED(index);
    Q_UNUSED(count);
    resultTab_.insert(resultId, tabId);
    ConsolePage &page = pageFor(tabId);
    page.grid->setResultId(resultId);
    page.subTabs->setCurrentWidget(page.grid);
    consoleTabs_->setCurrentWidget(page.root);
}

void DatabaseResultsPanel::onRowsAppended(quint64 resultId, quint64 first, quint64 count)
{
    const auto it = pages_.find(resultTab_.value(resultId, 0));
    if (it != pages_.end()) {
        it.value().grid->rowsAppended(resultId, first, count);
    }
}

void DatabaseResultsPanel::onExecutionFinished(quint64 resultId, bool ok, quint64 affected,
                                               quint64 elapsedMs, FfiDbError error)
{
    const auto it = pages_.find(resultTab_.value(resultId, 0));
    if (it == pages_.end()) {
        return;
    }
    it.value().grid->executionFinished(resultId, ok, affected, elapsedMs);
    if (!ok) {
        it.value().output->appendPlainText(QString(error.message));
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
    auto *panel = new DatabaseResultsPanel(editorTabs, consoleService, resultProvider,
                                           appSettings, dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Database Results"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("databaseResults"), dock, ads::BottomDockWidgetArea,
                        relativeTo);
    docks->hide(QStringLiteral("databaseResults"));
    return panel;
}

} // namespace ui_shell
