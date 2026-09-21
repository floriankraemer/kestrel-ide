#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

class QComboBox;
class QLabel;
class QToolButton;

namespace ui_shell {

class EditorTabs;

// The console's control bar (database-tools-plan F3.1/F3.3): source
// picker, transaction mode, Run/Cancel/Commit/Rollback. Sits above
// `DatabaseResultsPanel`'s Output/Result tabs.
//
// Tracks "which console tab is this bar driving" the same way `EditorTabs`
// itself tracks its own active group (`editor_tabs.cpp`'s own
// `qApp::focusChanged` hook) — a second, independent subscriber to the
// same signal, which is exactly what that hook's own doc comment already
// says a Qt signal is for. `.sql` files under `db_core::console::
// consoles_dir` and any other `.sql` tab both count; a non-`.sql` tab
// hides the bar entirely.
//
// Humble view: this widget decides nothing about what a statement means or
// whether it is allowed to run — `ConsoleService::execute`'s read-only
// guard is the only refusal, reported back through `outputAppended`/
// `executionFinished`, never re-decided here.
class DatabaseConsoleBar : public QWidget
{
public:
    DatabaseConsoleBar(EditorTabs *editorTabs, ConsoleService *consoleService,
                       AppSettings *appSettings, QWidget *parent);

    void runClicked();
    void runScriptClicked();
    void cancelClicked();
    void refreshForCurrentTab();

private:
    void refreshSources();
    void attachCurrentTab();
    void setStatus(const QString &text);

    EditorTabs *editorTabs_;
    ConsoleService *consoleService_;
    QComboBox *sourceCombo_;
    QComboBox *txModeCombo_;
    QToolButton *runButton_;
    QToolButton *runScriptButton_;
    QToolButton *cancelButton_;
    QToolButton *commitButton_;
    QToolButton *rollbackButton_;
    QLabel *statusLabel_;

    quint64 currentTabId_ = 0;
    QString currentPath_;
    QString attachedSourceId_;
};

// Builds the bar and wires its own tab-tracking hooks; the caller places
// the returned widget (e.g. above `DatabaseResultsPanel`'s tabs).
DatabaseConsoleBar *mountDatabaseConsoleBar(EditorTabs *editorTabs, ConsoleService *consoleService,
                                           AppSettings *appSettings, QWidget *parent);

} // namespace ui_shell
