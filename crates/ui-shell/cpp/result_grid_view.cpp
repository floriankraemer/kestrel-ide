#include "result_grid_view.h"

#include "result_table_model.h"
#include "value_editor_dialog.h"

#include <cstdint>
#include <functional>

#include <QAction>
#include <QEvent>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QKeyEvent>
#include <QKeySequence>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QTableView>
#include <QToolButton>
#include <QVBoxLayout>

namespace ui_shell {

namespace {
// Window-scoped shortcut wiring, same reasoning as `DatabaseConsoleBar`'s
// own (its doc comment): the grid is rarely focused when the shortcut is
// pressed, so the action must fire regardless of which widget has focus.
QAction *windowShortcut(QWidget *owner, AppSettings *appSettings, const QString &actionId,
                        const std::function<void()> &triggered)
{
    auto *action = new QAction(owner);
    action->setShortcut(
      QKeySequence(appSettings->shortcutFor(actionId), QKeySequence::PortableText));
    QObject::connect(action, &QAction::triggered, owner, triggered);
    owner->addAction(action);
    return action;
}
} // namespace

ResultGridView::ResultGridView(ConsoleService *consoleService, ResultProvider *provider,
                               AppSettings *appSettings, QWidget *parent)
  : QWidget(parent)
  , consoleService_(consoleService)
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

    submitButton_ = new QToolButton(this);
    submitButton_->setText(tr("Submit"));
    connect(submitButton_, &QToolButton::clicked, this, &ResultGridView::submit);
    toolbar->addWidget(submitButton_);

    revertButton_ = new QToolButton(this);
    revertButton_->setText(tr("Revert"));
    connect(revertButton_, &QToolButton::clicked, this, &ResultGridView::revert);
    toolbar->addWidget(revertButton_);

    addRowButton_ = new QToolButton(this);
    addRowButton_->setText(tr("Add Row"));
    connect(addRowButton_, &QToolButton::clicked, this, &ResultGridView::addRow);
    toolbar->addWidget(addRowButton_);

    deleteRowButton_ = new QToolButton(this);
    deleteRowButton_->setText(tr("Delete Row"));
    connect(deleteRowButton_, &QToolButton::clicked, this, &ResultGridView::deleteSelectedRows);
    toolbar->addWidget(deleteRowButton_);

    cloneRowButton_ = new QToolButton(this);
    cloneRowButton_->setText(tr("Clone Row"));
    connect(cloneRowButton_, &QToolButton::clicked, this, &ResultGridView::cloneSelectedRow);
    toolbar->addWidget(cloneRowButton_);

    previewDmlButton_ = new QToolButton(this);
    previewDmlButton_->setText(tr("Preview DML"));
    connect(previewDmlButton_, &QToolButton::clicked, this, &ResultGridView::previewDml);
    toolbar->addWidget(previewDmlButton_);

    layout->addLayout(toolbar);

    editableBanner_ = new QLabel(this);
    editableBanner_->setWordWrap(true);
    editableBanner_->setVisible(false);
    layout->addWidget(editableBanner_);

    model_ = new ResultTableModel(provider_, this);
    tableView_ = new QTableView(this);
    tableView_->setModel(model_);
    tableView_->horizontalHeader()->setStretchLastSection(true);
    tableView_->setSelectionBehavior(QAbstractItemView::SelectRows);
    tableView_->installEventFilter(this);
    connect(tableView_, &QTableView::doubleClicked, this, &ResultGridView::openValueEditor);
    layout->addWidget(tableView_, 1);

    statusLabel_ = new QLabel(this);
    layout->addWidget(statusLabel_);

    windowShortcut(this, appSettings, QStringLiteral("database.submit"), [this]() { submit(); });
    windowShortcut(this, appSettings, QStringLiteral("database.revert"), [this]() { revert(); });
    windowShortcut(this, appSettings, QStringLiteral("database.addRow"), [this]() { addRow(); });
    windowShortcut(this, appSettings, QStringLiteral("database.deleteRow"),
                  [this]() { deleteSelectedRows(); });
    windowShortcut(this, appSettings, QStringLiteral("database.cloneRow"),
                  [this]() { cloneSelectedRow(); });
    windowShortcut(this, appSettings, QStringLiteral("database.previewDml"),
                  [this]() { previewDml(); });

    updateActionsEnabled();
}

void ResultGridView::setResultId(quint64 resultId)
{
    resultId_ = resultId;
    model_->setResultId(resultId);
    statusLabel_->setText(tr("Running…"));
    editableBanner_->setVisible(false);
    updateActionsEnabled();
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

void ResultGridView::editabilityChanged(quint64 resultId, bool editable, const QString &reason)
{
    if (resultId != resultId_) {
        return;
    }
    model_->refreshEditability();
    editableBanner_->setVisible(!editable);
    editableBanner_->setText(reason);
    updateActionsEnabled();
}

void ResultGridView::submitFinished(quint64 resultId, bool ok, const QString &message)
{
    if (resultId != resultId_) {
        return;
    }
    statusLabel_->setText(message);
    if (!ok) {
        QMessageBox::warning(this, tr("Submit"), message);
    }
    model_->refreshRows();
    updateActionsEnabled();
}

void ResultGridView::resultRefreshed(quint64 oldResultId, quint64 newResultId)
{
    if (oldResultId != resultId_) {
        return;
    }
    setResultId(newResultId);
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

void ResultGridView::submit()
{
    if (resultId_ == 0 || !model_->isEditable()) {
        return;
    }
    const FfiResult result = consoleService_->submit(resultId_);
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
    }
}

void ResultGridView::revert()
{
    if (resultId_ == 0) {
        return;
    }
    provider_->revert(resultId_);
    model_->refreshRows();
}

void ResultGridView::addRow()
{
    if (resultId_ == 0 || !model_->isEditable()) {
        return;
    }
    const FfiResult result = provider_->addRow(resultId_);
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    model_->refreshRows();
}

void ResultGridView::deleteSelectedRows()
{
    if (resultId_ == 0 || !model_->isEditable()) {
        return;
    }
    const QModelIndexList selected = tableView_->selectionModel()->selectedRows();
    if (selected.isEmpty()) {
        return;
    }
    ::rust::Vec<::std::uint64_t> rows;
    for (const QModelIndex &index : selected) {
        rows.push_back(::std::uint64_t(index.row()));
    }
    const FfiResult result = provider_->deleteRows(resultId_, rows);
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    model_->refreshRows();
}

void ResultGridView::cloneSelectedRow()
{
    if (resultId_ == 0 || !model_->isEditable()) {
        return;
    }
    const QModelIndexList selected = tableView_->selectionModel()->selectedRows();
    if (selected.isEmpty()) {
        return;
    }
    const FfiResult result = provider_->cloneRow(resultId_, quint64(selected.first().row()));
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    model_->refreshRows();
}

void ResultGridView::previewDml()
{
    if (resultId_ == 0) {
        return;
    }
    const FfiResult result = consoleService_->dmlPreview(resultId_);
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
    }
}

void ResultGridView::openValueEditor(const QModelIndex &index)
{
    if (!index.isValid() || !model_->isEditable()) {
        return;
    }
    const QString column = model_->columnNameAt(index.column());
    if (column.isEmpty()) {
        return;
    }
    const QString text = model_->data(index, Qt::EditRole).toString();
    auto *dialog =
      new ValueEditorDialog(provider_, resultId_, quint64(index.row()), column, text, this);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    connect(dialog, &QDialog::accepted, this, [this]() { model_->refreshRows(); });
    dialog->exec();
}

bool ResultGridView::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == tableView_ && event->type() == QEvent::KeyPress) {
        auto *keyEvent = static_cast<QKeyEvent *>(event);
        if ((keyEvent->modifiers() & Qt::ShiftModifier)
            && (keyEvent->key() == Qt::Key_Return || keyEvent->key() == Qt::Key_Enter)) {
            openValueEditor(tableView_->currentIndex());
            return true;
        }
    }
    return QWidget::eventFilter(watched, event);
}

void ResultGridView::updateActionsEnabled()
{
    const bool editable = resultId_ != 0 && model_->isEditable();
    submitButton_->setEnabled(editable);
    revertButton_->setEnabled(editable);
    addRowButton_->setEnabled(editable);
    deleteRowButton_->setEnabled(editable);
    cloneRowButton_->setEnabled(editable);
    previewDmlButton_->setEnabled(editable);
}

} // namespace ui_shell
