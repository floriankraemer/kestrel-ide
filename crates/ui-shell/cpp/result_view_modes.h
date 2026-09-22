#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAbstractTableModel>

class QTableView;
class QToolButton;
class QStandardItemModel;

namespace ui_shell {

// Table / Transpose / Text / Record view modes for the result grid
// (database-tools-plan F4d) — hosted by `result_grid_view.cpp`, which owns
// the `QStackedWidget` switching between them; this file only builds the
// per-mode models/widgets, never a business decision (`ResultProvider::
// resultModes`/`recordRows`/`textView` already made every one of those in
// Rust).

// A read-only proxy that swaps rows and columns of `source` — the
// Transpose view. Column `c` of the transpose is source row `c`; row `r` of
// the transpose is source column `r`, headed by that column's own name.
class TransposeTableModel : public QAbstractTableModel
{
    Q_OBJECT
public:
    explicit TransposeTableModel(QAbstractItemModel *source, QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = QModelIndex()) const override;
    int columnCount(const QModelIndex &parent = QModelIndex()) const override;
    QVariant data(const QModelIndex &index, int role = Qt::DisplayRole) const override;
    QVariant headerData(int section, Qt::Orientation orientation,
                        int role = Qt::DisplayRole) const override;

private:
    QAbstractItemModel *source_;
};

// Builds the Record view's field tree for one row (`FfiRecordRow`'s own
// doc comment, `ffi.rs`) as a three-column (Key/Value/Type)
// `QStandardItemModel`, owned by `parent`.
QStandardItemModel *buildRecordModel(const ::rust::Vec<FfiRecordRow> &rows, QObject *parent);

// A checkable column-visibility popup (F4d) on `button`'s menu, toggling
// `view`'s own columns hidden/shown.
// ponytail: no per-result/per-source persistence — hidden columns reset
// the next time a different result replaces this grid; upgrade if a real
// user wants the choice remembered.
void installColumnVisibilityMenu(QToolButton *button, QTableView *view,
                                 const ::rust::Vec<FfiDbColumn> &columns);

} // namespace ui_shell
