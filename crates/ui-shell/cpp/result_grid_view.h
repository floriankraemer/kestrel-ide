#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QLabel;
class QLineEdit;
class QSpinBox;
class QTableView;
class QToolButton;

namespace ui_shell {

class ResultTableModel;

// One result's grid (database-tools-plan F3.4/F3.5): a `QTableView` over
// `ResultTableModel`, plus a toolbar for paging, `WHERE`/`ORDER BY` and a
// row-count/elapsed status line.
//
// Humble view: paging, filtering and sorting all round-trip through
// `ResultProvider`/`ConsoleService`; this widget only reads what came back
// and lays it out.
class ResultGridView : public QWidget
{
public:
    ResultGridView(ResultProvider *provider, QWidget *parent);

    // Switches the view to a fresh result — `executionStarted`'s handler.
    void setResultId(quint64 resultId);
    // `rowsAppended`'s handler.
    void rowsAppended(quint64 resultId, quint64 first, quint64 count);
    // `executionFinished`'s handler: updates the row count/elapsed status.
    void executionFinished(quint64 resultId, bool ok, quint64 affected, quint64 elapsedMs);

private:
    void applyClauses();

    ResultProvider *provider_;
    ResultTableModel *model_;
    QTableView *tableView_;
    QLineEdit *whereEdit_;
    QLineEdit *orderByEdit_;
    QToolButton *applyButton_;
    QLabel *statusLabel_;
    quint64 resultId_ = 0;
};

} // namespace ui_shell
