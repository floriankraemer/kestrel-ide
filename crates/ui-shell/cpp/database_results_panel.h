#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

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

// The `databaseResults` dock (database-tools-plan F3.1/F3.3-F3.5): the
// console control bar over an Output tab (free-text status/errors) and a
// Result tab (`ResultGridView`).
//
// One console/result pair is shown at a time — the most recently started
// execution — rather than a tab per console tab; see this phase's own
// report for why (`main_window.cpp`/`editor_tabs.cpp` sit at hard
// line-count ceilings that made a per-tab strip not worth the risk this
// phase's time budget allowed for).
class DatabaseResultsPanel : public QWidget
{
public:
    DatabaseResultsPanel(EditorTabs *editorTabs, ConsoleService *consoleService,
                         ResultProvider *resultProvider, AppSettings *appSettings,
                         QWidget *parent);

private:
    void appendOutput(quint64 tabId, const QString &text);
    void onExecutionStarted(quint64 tabId, quint64 resultId, quint32 index, quint32 count);
    void onExecutionFinished(quint64 resultId, bool ok, quint64 affected, quint64 elapsedMs,
                             FfiDbError error);

    DatabaseConsoleBar *bar_;
    QTabWidget *tabs_;
    QPlainTextEdit *output_;
    ResultGridView *grid_;
};

DatabaseResultsPanel *buildDatabaseResultsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                               ads::CDockAreaWidget *relativeTo,
                                               EditorTabs *editorTabs,
                                               ConsoleService *consoleService,
                                               ResultProvider *resultProvider,
                                               AppSettings *appSettings);

} // namespace ui_shell
