#include "result_view_modes.h"

#include <QAction>
#include <QMenu>
#include <QStandardItemModel>
#include <QTableView>
#include <QToolButton>
#include <QVector>

namespace ui_shell {

TransposeTableModel::TransposeTableModel(QAbstractItemModel *source, QObject *parent)
  : QAbstractTableModel(parent)
  , source_(source)
{
    connect(source_, &QAbstractItemModel::modelReset, this, [this]() {
        beginResetModel();
        endResetModel();
    });
    connect(source_, &QAbstractItemModel::rowsInserted, this, [this]() {
        beginResetModel();
        endResetModel();
    });
    connect(source_, &QAbstractItemModel::dataChanged, this, [this]() {
        if (rowCount() > 0 && columnCount() > 0) {
            emit dataChanged(index(0, 0), index(rowCount() - 1, columnCount() - 1));
        }
    });
}

int TransposeTableModel::rowCount(const QModelIndex &parent) const
{
    if (parent.isValid()) {
        return 0;
    }
    return source_->columnCount();
}

int TransposeTableModel::columnCount(const QModelIndex &parent) const
{
    if (parent.isValid()) {
        return 0;
    }
    return source_->rowCount();
}

QVariant TransposeTableModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid()) {
        return {};
    }
    return source_->data(source_->index(index.column(), index.row()), role);
}

QVariant TransposeTableModel::headerData(int section, Qt::Orientation orientation, int role) const
{
    if (role != Qt::DisplayRole) {
        return {};
    }
    // Transposed: a column of this model heads with the source row's
    // ordinal (1-based, for readability); a row of this model heads with
    // the source column's own name.
    if (orientation == Qt::Horizontal) {
        return section + 1;
    }
    return source_->headerData(section, Qt::Horizontal, role);
}

QStandardItemModel *buildRecordModel(const ::rust::Vec<FfiRecordRow> &rows, QObject *parent)
{
    auto *model = new QStandardItemModel(parent);
    model->setHorizontalHeaderLabels(
      { QObject::tr("Key"), QObject::tr("Value"), QObject::tr("Type") });
    // `stack[d]` is the last Key item inserted at depth `d` — a
    // `FfiRecordRow` list is already in depth-first, parent-before-child
    // order (`db_core::value::record_rows`'s own doc comment), so a flat
    // walk with this one stack is enough to rebuild the tree; no lookahead
    // or recursion needed.
    QVector<QStandardItem *> stack;
    for (const FfiRecordRow &row : rows) {
        auto *keyItem = new QStandardItem(QString(row.key));
        auto *valueItem = new QStandardItem(QString(row.value));
        auto *typeItem = new QStandardItem(QString(row.typeName));
        keyItem->setEditable(false);
        valueItem->setEditable(false);
        typeItem->setEditable(false);
        QStandardItem *parentItem = nullptr;
        if (row.depth > 0 && int(row.depth) - 1 < stack.size()) {
            parentItem = stack[int(row.depth) - 1];
        }
        if (parentItem != nullptr) {
            parentItem->appendRow({ keyItem, valueItem, typeItem });
        } else {
            model->appendRow({ keyItem, valueItem, typeItem });
        }
        if (stack.size() <= int(row.depth)) {
            stack.resize(int(row.depth) + 1);
        }
        stack[int(row.depth)] = keyItem;
    }
    return model;
}

void installColumnVisibilityMenu(QToolButton *button, QTableView *view,
                                 const ::rust::Vec<FfiDbColumn> &columns)
{
    auto *menu = new QMenu(button);
    for (int i = 0; i < int(columns.size()); ++i) {
        QAction *action = menu->addAction(QString(columns[size_t(i)].name));
        action->setCheckable(true);
        action->setChecked(!view->isColumnHidden(i));
        QObject::connect(action, &QAction::toggled, view,
                         [view, i](bool checked) { view->setColumnHidden(i, !checked); });
    }
    button->setMenu(menu);
    button->setPopupMode(QToolButton::InstantPopup);
}

} // namespace ui_shell
