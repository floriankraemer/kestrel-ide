#include "result_table_model.h"

namespace ui_shell {

namespace {
constexpr QChar kCellSeparator(0x1f); // unit separator — matches console.rs's `CELL_SEP`.

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
    Page page{first, provider_->rowPage(resultId_, first, kPageSize)};
    if (pages_.size() >= kMaxCachedPages) {
        pages_.removeFirst();
    }
    pages_.append(std::move(page));
}

QVariant ResultTableModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || role != Qt::DisplayRole) {
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
    const QStringList cells = splitField(QString(row.cells));
    if (index.column() >= cells.size()) {
        return {};
    }
    return cells.at(index.column());
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
