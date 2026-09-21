#include "result_grid_view.h"

#include "result_table_model.h"

#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QTableView>
#include <QToolButton>
#include <QVBoxLayout>

namespace ui_shell {

ResultGridView::ResultGridView(ResultProvider *provider, QWidget *parent)
  : QWidget(parent)
  , provider_(provider)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);

    auto *toolbar = new QHBoxLayout();
    whereEdit_ = new QLineEdit(this);
    whereEdit_->setPlaceholderText(tr("WHERE"));
    toolbar->addWidget(whereEdit_, 1);
    orderByEdit_ = new QLineEdit(this);
    orderByEdit_->setPlaceholderText(tr("ORDER BY"));
    toolbar->addWidget(orderByEdit_, 1);
    applyButton_ = new QToolButton(this);
    applyButton_->setText(tr("Apply"));
    connect(applyButton_, &QToolButton::clicked, this, &ResultGridView::applyClauses);
    toolbar->addWidget(applyButton_);
    layout->addLayout(toolbar);

    model_ = new ResultTableModel(provider_, this);
    tableView_ = new QTableView(this);
    tableView_->setModel(model_);
    tableView_->horizontalHeader()->setStretchLastSection(true);
    tableView_->setSelectionBehavior(QAbstractItemView::SelectRows);
    layout->addWidget(tableView_, 1);

    statusLabel_ = new QLabel(this);
    layout->addWidget(statusLabel_);
}

void ResultGridView::setResultId(quint64 resultId)
{
    resultId_ = resultId;
    model_->setResultId(resultId);
    statusLabel_->setText(tr("Running…"));
}

void ResultGridView::rowsAppended(quint64 resultId, quint64 first, quint64 count)
{
    if (resultId != resultId_) {
        return;
    }
    model_->rowsAppended(first, count);
}

void ResultGridView::executionFinished(quint64 resultId, bool ok, quint64 affected,
                                       quint64 elapsedMs)
{
    if (resultId != resultId_) {
        return;
    }
    statusLabel_->setText(ok ? tr("%1 row(s) — %2 ms").arg(affected).arg(elapsedMs)
                             : tr("Failed — %1 ms").arg(elapsedMs));
}

void ResultGridView::applyClauses()
{
    if (resultId_ == 0) {
        return;
    }
    const FfiResult result =
      provider_->applyClauses(resultId_, whereEdit_->text(), orderByEdit_->text());
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    // The new result id travels back in `message` — see
    // `ResultProviderRust::apply_clauses`'s own doc comment.
    bool parsed = false;
    const quint64 newId = QString(result.message).toULongLong(&parsed);
    if (parsed) {
        setResultId(newId);
    }
}

} // namespace ui_shell
