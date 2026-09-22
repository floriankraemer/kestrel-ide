#include "database_results_panel.h"

#include "database_console_bar.h"
#include "database_dialogs.h"
#include "database_exchange_actions.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "editor_tabs.h"
#include "result_grid_view.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QApplication>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTabWidget>
#include <QTimer>
#include <QVBoxLayout>

namespace ui_shell {

DatabaseResultsPanel::DatabaseResultsPanel(EditorTabs *editorTabs, ConsoleService *consoleService,
                                           ResultProvider *resultProvider,
                                           ExchangeService *exchangeService,
                                           AppSettings *appSettings, QWidget *parent)
  : QWidget(parent)
  , editorTabs_(editorTabs)
  , consoleService_(consoleService)
  , resultProvider_(resultProvider)
  , exchangeService_(exchangeService)
  , appSettings_(appSettings)
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
    connect(consoleService, &ConsoleService::editabilityChanged, this,
            &DatabaseResultsPanel::onEditabilityChanged);
    connect(consoleService, &ConsoleService::submitFinished, this,
            &DatabaseResultsPanel::onSubmitFinished);
    connect(consoleService, &ConsoleService::resultRefreshed, this,
            &DatabaseResultsPanel::onResultRefreshed);
    // F4.2's DML preview opens a read-only virtual document tab — wired
    // here rather than in `editor_tabs.cpp` (its own constructor, at that
    // file's line-size ceiling), the same way `DatabaseService`'s own
    // `virtualDocumentOpened` is wired there for "Go to DDL".
    connect(consoleService, &ConsoleService::virtualDocumentOpened, this,
            [this](quint64 id, const QString &title, bool isNew) {
                if (isNew) {
                    editorTabs_->onTabOpened(id, title);
                }
                editorTabs_->focusTab(id);
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
    auto *toolbar = new QHBoxLayout();
    auto *exportButton = new QPushButton(tr("Export / Copy As…"), page.root);
    toolbar->addWidget(exportButton);
    toolbar->addStretch(1);
    layout->addLayout(toolbar);
    page.subTabs = new QTabWidget(page.root);
    page.output = new QPlainTextEdit(page.root);
    page.output->setReadOnly(true);
    page.subTabs->addTab(page.output, tr("Output"));
    // F4c: `applyClauses`/`goToNavTarget` hand this grid a fresh result id
    // directly (never through `executionStarted`), so `resultTab_` must
    // learn the mapping here too — otherwise that new id's own
    // `rowsAppended`/`executionFinished` would have nowhere to route to.
    page.grid = new ResultGridView(
      consoleService_, resultProvider_, appSettings_, page.root,
      [this, tabId](quint64 resultId) { resultTab_.insert(resultId, tabId); });
    page.subTabs->addTab(page.grid, tr("Result"));
    layout->addWidget(page.subTabs);

    connect(exportButton, &QPushButton::clicked, this, [this, tabId]() {
        exportCurrentResult(tabId);
    });

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
    page.currentResultId = resultId;
    page.grid->setResultId(resultId);
    page.subTabs->setCurrentWidget(page.grid);
    consoleTabs_->setCurrentWidget(page.root);
    // E2E only (`crates/app/tests/e2e_database_console.rs`): the NFR
    // table's "first row" timing starts here, at Run — never at the
    // console-bar click itself, which races the FFI call that produces
    // this `resultId` in the first place.
    e2eMark(QStringLiteral("{\"ev\":\"db_exec_started\",\"resultId\":%1}").arg(resultId));
    // One turn of the event loop later: `page.grid` may be becoming the
    // current tab for the very first time right on this call (a page is
    // created lazily, `DatabaseResultsPanel`'s own doc comment), so its
    // geometry needs a turn to settle before `markE2eGridRect` reads it —
    // see that method's own doc comment.
    ResultGridView *grid = page.grid;
    QTimer::singleShot(0, this, [grid, resultId]() { grid->markE2eGridRect(resultId); });
}

void DatabaseResultsPanel::onRowsAppended(quint64 resultId, quint64 first, quint64 count)
{
    const auto it = pages_.find(resultTab_.value(resultId, 0));
    if (it != pages_.end()) {
        it.value().grid->rowsAppended(resultId, first, count);
    }
    // E2E only: the NFR table's "first row" timing ends at the *first*
    // of these per result — every later one is `rowPage`'s own concern,
    // not this mark's (see `ResultTableModel::ensurePage`).
    e2eMark(QStringLiteral("{\"ev\":\"db_rows_appended\",\"resultId\":%1,\"first\":%2,\"count\":%3}")
              .arg(resultId)
              .arg(first)
              .arg(count));
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
    // E2E only: `error.code == 3` is `DbErrorCode::Cancelled` — the
    // console-bar Cancel flow's own outcome (`docs/architecture/
    // database-tools.md` §4's "two cancellation layers").
    e2eMark(QStringLiteral("{\"ev\":\"db_exec_finished\",\"resultId\":%1,\"ok\":%2,"
                            "\"errorCode\":%3,\"elapsedMs\":%4}")
              .arg(resultId)
              .arg(ok ? "true" : "false")
              .arg(error.code)
              .arg(elapsedMs));
}

void DatabaseResultsPanel::exportCurrentResult(quint64 tabId)
{
    const auto it = pages_.find(tabId);
    if (it == pages_.end() || it.value().currentResultId == 0) {
        return;
    }
    const quint64 resultId = it.value().currentResultId;
    QStringList columns;
    for (const FfiDbColumn &column : resultProvider_->columns(resultId)) {
        columns.append(QString(column.name));
    }
    // ponytail: the whole result is pulled into memory for this dialog
    // (capped at 100k rows) rather than streamed the way `exportTable`
    // is — a grid export starts from rows the grid already fetched, not
    // a fresh session read, so there is no cursor here to stream from;
    // upgrade if a huge already-fetched result set ever makes this a
    // real problem.
    const quint64 rowCount = qMin<quint64>(resultProvider_->rowCount(resultId), 100000);
    QStringList rows;
    for (quint64 first = 0; first < rowCount;) {
        // `rowValues` (F4c), not `rowPage`: each entry is the row's real
        // typed `Value`s (JSON-encoded), not rendered display text, so
        // the export dialog can write a properly-typed SQL/XLSX/JSON
        // file rather than quoting every cell as a string.
        const auto page = resultProvider_->rowValues(resultId, first, 1000);
        if (page.empty()) {
            break;
        }
        for (const QString &row : page) {
            rows.append(row);
        }
        first += page.size();
    }
    showExportRowsDialog(this, exchangeService_, columns, rows);
}

void DatabaseResultsPanel::onEditabilityChanged(quint64 resultId, bool editable,
                                                const QString &reason)
{
    const auto it = pages_.find(resultTab_.value(resultId, 0));
    if (it != pages_.end()) {
        it.value().grid->editabilityChanged(resultId, editable, reason);
    }
}

void DatabaseResultsPanel::onSubmitFinished(quint64 resultId, bool ok, const QString &message)
{
    const auto it = pages_.find(resultTab_.value(resultId, 0));
    if (it != pages_.end()) {
        it.value().grid->submitFinished(resultId, ok, message);
    }
}

void DatabaseResultsPanel::onResultRefreshed(quint64 oldResultId, quint64 newResultId)
{
    const auto it = pages_.find(resultTab_.value(oldResultId, 0));
    if (it != pages_.end()) {
        it.value().grid->resultRefreshed(oldResultId, newResultId);
    }
}

DatabaseResultsPanel *buildDatabaseResultsDock(ads::CDockManager *dockManager,
                                               DockRegistry *docks,
                                               ads::CDockAreaWidget *relativeTo,
                                               EditorTabs *editorTabs,
                                               ConsoleService *consoleService,
                                               ResultProvider *resultProvider,
                                               ExchangeService *exchangeService,
                                               AppSettings *appSettings)
{
    auto *panel = new DatabaseResultsPanel(editorTabs, consoleService, resultProvider,
                                           exchangeService, appSettings, dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Database Results"));
    dock->setWidget(panel);
    // Tabbed *into* the existing bottom area (CenterDockWidgetArea relative
    // to it) like every other bottom dock — `BottomDockWidgetArea` would
    // split a second bottom row that keeps stealing editor height even while
    // the dock is hidden (found by the minimap E2E flow's strip-height premise).
    docks->registerDock(QStringLiteral("databaseResults"), dock, ads::CenterDockWidgetArea,
                        relativeTo);
    docks->hide(QStringLiteral("databaseResults"));
    // E2E only — see `ContainersPanel::refreshE2eRects`'s own doc comment
    // for why this waits a turn of the event loop past `visibilityChanged`.
    QObject::connect(dock, &ads::CDockWidget::visibilityChanged, panel, [panel](bool visible) {
        if (visible) {
            QTimer::singleShot(0, panel, [panel]() { panel->bar()->markE2eToolbarRects(); });
        }
    });
    return panel;
}

} // namespace ui_shell
