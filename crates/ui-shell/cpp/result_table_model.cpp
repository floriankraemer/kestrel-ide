#include "result_table_model.h"

#include "e2e_mark.h"

#include <QColor>
#include <QElapsedTimer>

namespace ui_shell {

namespace {
constexpr QChar kCellSeparator(0x1f); // unit separator — matches console.rs's `CELL_SEP`.

// `db_core::dml::EditBuffer::row_flags`'s own bit layout.
constexpr quint8 kFlagEdited = 1 << 0;
constexpr quint8 kFlagDeleted = 1 << 1;
constexpr quint8 kFlagInserted = 1 << 2;

QStringList splitField(const QString &joined)
{
    if (joined.isEmpty()) {
        return {};
    }
    return joined.split(kCellSeparator);
}
} // namespace

ResultTableModel::ResultTableModel(ResultProvider *provider, QObject *parent)
  : QAbstractTableModel(parent)
  , provider_(provider)
{
}

void ResultTableModel::setResultId(quint64 resultId)
{
    beginResetModel();
    resultId_ = resultId;
    rowCount_ = provider_->rowCount(resultId_);
    columns_ = provider_->columns(resultId_);
    pages_.clear();
    endResetModel();
    refreshEditability();
}

void ResultTableModel::refreshRows()
{
    beginResetModel();
    rowCount_ = provider_->rowCount(resultId_);
    pages_.clear();
    endResetModel();
}

void ResultTableModel::refreshEditability()
{
    editable_ = provider_->isEditable(resultId_);
    notEditableReason_ = QString(provider_->notEditableReason(resultId_));
}

QString ResultTableModel::columnNameAt(int column) const
{
    if (column < 0 || column >= int(columns_.size())) {
        return {};
    }
    return QString(columns_[column].name);
}

void ResultTableModel::rowsAppended(quint64 first, quint64 count)
{
    Q_UNUSED(first);
    if (count == 0) {
        return;
    }
    const quint64 newTotal = provider_->rowCount(resultId_);
    if (newTotal <= rowCount_) {
        return;
    }
    beginInsertRows(QModelIndex(), int(rowCount_), int(newTotal) - 1);
    rowCount_ = newTotal;
    endInsertRows();
}

int ResultTableModel::rowCount(const QModelIndex &parent) const
{
    return parent.isValid() ? 0 : int(rowCount_);
}

int ResultTableModel::columnCount(const QModelIndex &parent) const
{
    return parent.isValid() ? 0 : int(columns_.size());
}

const ResultTableModel::Page *ResultTableModel::pageFor(int row) const
{
    for (const Page &page : pages_) {
        if (quint64(row) >= page.first && quint64(row) < page.first + quint64(page.rows.size())) {
            return &page;
        }
    }
    return nullptr;
}

void ResultTableModel::ensurePage(int row) const
{
    if (pageFor(row) != nullptr) {
        return;
    }
    const quint64 first = (quint64(row) / kPageSize) * kPageSize;
    // E2E only (`crates/app/tests/e2e_database_console.rs`): the NFR
    // table's "UI thread never blocked > 16 ms per FFI call" target, one
    // `db_row_page{ms}` mark per page — the test takes the max over the
    // whole scroll.
    QElapsedTimer timer;
    timer.start();
    Page page{first, provider_->rowPage(resultId_, first, kPageSize)};
    e2eMark(QStringLiteral("{\"ev\":\"db_row_page\",\"resultId\":%1,\"first\":%2,\"ms\":%3}")
              .arg(resultId_)
              .arg(first)
              .arg(timer.elapsed()));
    if (pages_.size() >= kMaxCachedPages) {
        pages_.removeFirst();
    }
    pages_.append(std::move(page));
}

QVariant ResultTableModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid()) {
        return {};
    }
    if (role != Qt::DisplayRole && role != Qt::EditRole && role != Qt::BackgroundRole) {
        return {};
    }
    ensurePage(index.row());
    const Page *page = pageFor(index.row());
    if (page == nullptr) {
        return {};
    }
    const int offset = index.row() - int(page->first);
    if (offset < 0 || offset >= int(page->rows.size())) {
        return {};
    }
    const FfiDbRow &row = page->rows[offset];
    if (role == Qt::BackgroundRole) {
        if (row.flags & kFlagDeleted) {
            return QColor(255, 205, 210); // light red
        }
        if (row.flags & kFlagInserted) {
            return QColor(200, 230, 201); // light green
        }
        if (row.flags & kFlagEdited) {
            return QColor(255, 249, 196); // light yellow
        }
        return {};
    }
    const QStringList cells = splitField(QString(row.cells));
    if (index.column() >= cells.size()) {
        return {};
    }
    return cells.at(index.column());
}

bool ResultTableModel::setData(const QModelIndex &index, const QVariant &value, int role)
{
    if (!index.isValid() || role != Qt::EditRole) {
        return false;
    }
    const QString column = columnNameAt(index.column());
    if (column.isEmpty()) {
        return false;
    }
    const FfiResult result =
      provider_->setCell(resultId_, quint64(index.row()), column, value.toString());
    if (result.code != 0) {
        return false;
    }
    // The buffer's own state changed server-side; this cell's row (row
    // highlight) and value both need a fresh read next time they are
    // shown — evicting just this row's cached page is enough, no full
    // reset (unlike `refreshRows`, which also picks up a row-count change).
    pages_.clear();
    emit dataChanged(index, index, {Qt::DisplayRole, Qt::BackgroundRole});
    return true;
}

Qt::ItemFlags ResultTableModel::flags(const QModelIndex &index) const
{
    Qt::ItemFlags base = QAbstractTableModel::flags(index);
    if (index.isValid() && editable_) {
        base |= Qt::ItemIsEditable;
    }
    return base;
}

QVariant ResultTableModel::headerData(int section, Qt::Orientation orientation, int role) const
{
    if (role != Qt::DisplayRole) {
        return QAbstractTableModel::headerData(section, orientation, role);
    }
    if (orientation == Qt::Horizontal && section >= 0 && section < int(columns_.size())) {
        return QString(columns_[section].name);
    }
    return QAbstractTableModel::headerData(section, orientation, role);
}

bool ResultTableModel::canFetchMore(const QModelIndex &parent) const
{
    if (parent.isValid()) {
        return false;
    }
    return provider_->fetchMoreAvailable(resultId_);
}

void ResultTableModel::fetchMore(const QModelIndex &parent)
{
    if (parent.isValid()) {
        return;
    }
    provider_->fetchMore(resultId_);
}

} // namespace ui_shell
