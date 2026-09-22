#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

#include <functional>

class QLabel;
class QLineEdit;
class QPoint;
class QSpinBox;
class QTableView;
class QToolButton;
class QKeyEvent;

namespace ui_shell {

class ResultTableModel;

// One result's grid (database-tools-plan F3.4/F3.5/F4.1/F4.2): a
// `QTableView` over `ResultTableModel`, plus a toolbar for paging, `WHERE`/
// `ORDER BY`, the data editor's row actions, and a row-count/elapsed status
// line.
//
// Humble view: paging, filtering, sorting and every edit all round-trip
// through `ResultProvider`/`ConsoleService`; this widget only reads what
// came back and lays it out.
class ResultGridView : public QWidget
{
public:
    // `onResultAdopted`, if given, fires every time this grid starts
    // showing a *different* result id — including one this widget itself
    // discovered, not just `executionStarted`'s own (`applyClauses`'/
    // `goToNavTarget`'s new result id arrives back in an `FfiResult`
    // message, never through `executionStarted`, so without this callback
    // `DatabaseResultsPanel::resultTab_` never learns to route that id's
    // `rowsAppended`/`executionFinished` back to this grid at all).
    ResultGridView(ConsoleService *consoleService, ResultProvider *provider,
                  AppSettings *appSettings, QWidget *parent,
                  std::function<void(quint64)> onResultAdopted = {});

    // Switches the view to a fresh result — `executionStarted`'s handler.
    void setResultId(quint64 resultId);
    // `rowsAppended`'s handler.
    void rowsAppended(quint64 resultId, quint64 first, quint64 count);
    // `executionFinished`'s handler: updates the row count/elapsed status.
    void executionFinished(quint64 resultId, bool ok, quint64 affected, quint64 elapsedMs);
    // `ConsoleService::editabilityChanged`'s handler (F4.1).
    void editabilityChanged(quint64 resultId, bool editable, const QString &reason);
    // `ConsoleService::submitFinished`'s handler (F4.2).
    void submitFinished(quint64 resultId, bool ok, const QString &message);
    // `ConsoleService::resultRefreshed`'s handler: a submit's own re-run
    // landed under a new result id — follow it.
    void resultRefreshed(quint64 oldResultId, quint64 newResultId);

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    void applyClauses();
    void submit();
    void revert();
    void addRow();
    void deleteSelectedRows();
    void cloneSelectedRow();
    void previewDml();
    void openValueEditor(const QModelIndex &index);
    void updateActionsEnabled();
    // F4.3/F4c's forward FK navigation: the current cell's context menu
    // ("Go to referenced row ▸ <table>", `pos` in `tableView_` viewport
    // coordinates) and its `F4` shortcut equivalent (current cell, no
    // `pos`) — both resolve targets through `ConsoleService::
    // cellNavigation` and run the chosen one through `goToNavTarget`.
    void showCellContextMenu(const QPoint &pos);
    void goToReferencedRow();
    void navigateFromIndex(const QModelIndex &index, const QPoint &globalPos);
    void goToNavTarget(const FfiDbNavTarget &target);

    ConsoleService *consoleService_;
    ResultProvider *provider_;
    ResultTableModel *model_;
    QTableView *tableView_;
    QLineEdit *whereEdit_;
    QLineEdit *orderByEdit_;
    QToolButton *applyButton_;
    QToolButton *submitButton_;
    QToolButton *revertButton_;
    QToolButton *addRowButton_;
    QToolButton *deleteRowButton_;
    QToolButton *cloneRowButton_;
    QToolButton *previewDmlButton_;
    QLabel *editableBanner_;
    QLabel *statusLabel_;
    quint64 resultId_ = 0;
    std::function<void(quint64)> onResultAdopted_;
};

} // namespace ui_shell
