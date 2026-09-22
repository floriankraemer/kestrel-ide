#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QWidget>

class QPlainTextEdit;
class QTabWidget;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DatabaseConsoleBar;
class DockRegistry;
class EditorTabs;
class ResultGridView;

// The `databaseResults` dock (database-tools-plan F3.1/F3.3-F3.5/F3e): one
// shared console control bar (it already tracks "whichever `.sql` tab last
// had focus" on its own, `DatabaseConsoleBar`'s own doc comment) over a
// `QTabWidget` with one page per attached console — each page its own
// Output/Result sub-tab pair, so two consoles running at once never share
// one grid or one output log.
//
// A page is created lazily the first time its console does anything
// observable (`outputAppended`'s "Attached to ..." line, or sooner if a
// statement runs first) and removed when its editor tab closes
// (`DocumentManager::tabClosed`, the one real Qt signal `EditorTabs`
// itself deliberately has none of — its own doc comment on why). The
// current page follows the active editor tab the same way the console bar
// already does, via the same `qApp::focusChanged` hook.
class DatabaseResultsPanel : public QWidget
{
public:
    DatabaseResultsPanel(EditorTabs *editorTabs, ConsoleService *consoleService,
                         ResultProvider *resultProvider, ExchangeService *exchangeService,
                         AppSettings *appSettings, QWidget *parent);

    // Switches to `tabId`'s page if one exists — never creates one, so
    // merely browsing to a `.sql` file that has not attached yet does not
    // conjure an empty console page.
    void followActiveTab(quint64 tabId);

private:
    struct ConsolePage
    {
        QWidget *root;
        QTabWidget *subTabs;
        QPlainTextEdit *output;
        ResultGridView *grid;
        // The result currently shown in `grid` — `exportButton_`'s click
        // handler needs it and `ResultGridView` exposes no accessor of its
        // own (F4a's file; not this phase's to extend), so it is tracked
        // here from the same `executionStarted` that already feeds `grid`.
        quint64 currentResultId = 0;
    };

    ConsolePage &pageFor(quint64 tabId);
    void closePage(quint64 tabId);
    void appendOutput(quint64 tabId, const QString &text);
    void onExecutionStarted(quint64 tabId, quint64 resultId, quint32 index, quint32 count);
    void onRowsAppended(quint64 resultId, quint64 first, quint64 count);
    void onExecutionFinished(quint64 resultId, bool ok, quint64 affected, quint64 elapsedMs,
                             FfiDbError error);
    void exportCurrentResult(quint64 tabId);
    void onEditabilityChanged(quint64 resultId, bool editable, const QString &reason);
    void onSubmitFinished(quint64 resultId, bool ok, const QString &message);
    void onResultRefreshed(quint64 oldResultId, quint64 newResultId);

    EditorTabs *editorTabs_;
    ConsoleService *consoleService_;
    ResultProvider *resultProvider_;
    ExchangeService *exchangeService_;
    AppSettings *appSettings_;
    DatabaseConsoleBar *bar_;
    QTabWidget *consoleTabs_;
    QHash<quint64, ConsolePage> pages_;
    // Which console tab started each still-relevant result — populated by
    // `executionStarted` (the one signal carrying both ids together),
    // read by `rowsAppended`/`executionFinished`, which carry only the
    // result id, to route them to the right page's grid.
    QHash<quint64, quint64> resultTab_;
};

DatabaseResultsPanel *buildDatabaseResultsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                               ads::CDockAreaWidget *relativeTo,
                                               EditorTabs *editorTabs,
                                               ConsoleService *consoleService,
                                               ResultProvider *resultProvider,
                                               ExchangeService *exchangeService,
                                               AppSettings *appSettings);

} // namespace ui_shell
