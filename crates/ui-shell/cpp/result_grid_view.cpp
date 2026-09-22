#include "result_grid_view.h"

#include "result_table_model.h"
#include "result_view_modes.h"
#include "value_editor_dialog.h"

#include <cstdint>
#include <functional>

#include <QAction>
#include <QComboBox>
#include <QEvent>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QKeyEvent>
#include <QKeySequence>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QStackedWidget>
#include <QStandardItemModel>
#include <QTableView>
#include <QToolButton>
#include <QTreeView>
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
                               AppSettings *appSettings, QWidget *parent,
                               std::function<void(quint64)> onResultAdopted)
  : QWidget(parent)
  , consoleService_(consoleService)
  , provider_(provider)
  , onResultAdopted_(std::move(onResultAdopted))
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

    // F4d: view modes + column visibility.
    auto *viewToolbar = new QHBoxLayout();
    tableModeButton_ = new QToolButton(this);
    tableModeButton_->setText(tr("Table"));
    tableModeButton_->setCheckable(true);
    tableModeButton_->setChecked(true);
    connect(tableModeButton_, &QToolButton::clicked, this,
           [this]() { setViewMode(FfiDbViewMode::Table); });
    viewToolbar->addWidget(tableModeButton_);

    transposeModeButton_ = new QToolButton(this);
    transposeModeButton_->setText(tr("Transpose"));
    transposeModeButton_->setCheckable(true);
    connect(transposeModeButton_, &QToolButton::clicked, this,
           [this]() { setViewMode(FfiDbViewMode::Transpose); });
    viewToolbar->addWidget(transposeModeButton_);

    textModeButton_ = new QToolButton(this);
    textModeButton_->setText(tr("Text"));
    textModeButton_->setCheckable(true);
    connect(textModeButton_, &QToolButton::clicked, this,
           [this]() { setViewMode(FfiDbViewMode::Text); });
    viewToolbar->addWidget(textModeButton_);

    recordModeButton_ = new QToolButton(this);
    recordModeButton_->setText(tr("Record"));
    recordModeButton_->setCheckable(true);
    connect(recordModeButton_, &QToolButton::clicked, this,
           [this]() { setViewMode(FfiDbViewMode::Record); });
    viewToolbar->addWidget(recordModeButton_);

    textFormatCombo_ = new QComboBox(this);
    textFormatCombo_->addItem(tr("CSV"), int(FfiDbTextFormat::Csv));
    textFormatCombo_->addItem(tr("TSV"), int(FfiDbTextFormat::Tsv));
    textFormatCombo_->addItem(tr("JSON"), int(FfiDbTextFormat::Json));
    textFormatCombo_->addItem(tr("Markdown"), int(FfiDbTextFormat::Markdown));
    connect(textFormatCombo_, &QComboBox::currentIndexChanged, this,
           [this]() { updateTextView(); });
    viewToolbar->addWidget(textFormatCombo_);

    viewToolbar->addStretch(1);

    columnsButton_ = new QToolButton(this);
    columnsButton_->setText(tr("Columns…"));
    viewToolbar->addWidget(columnsButton_);
    layout->addLayout(viewToolbar);

    model_ = new ResultTableModel(provider_, this);
    tableView_ = new QTableView(this);
    tableView_->setModel(model_);
    tableView_->horizontalHeader()->setStretchLastSection(true);
    tableView_->setSelectionBehavior(QAbstractItemView::SelectRows);
    tableView_->installEventFilter(this);
    connect(tableView_, &QTableView::doubleClicked, this, &ResultGridView::openValueEditor);
    tableView_->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(tableView_, &QTableView::customContextMenuRequested, this,
           &ResultGridView::showCellContextMenu);
    connect(tableView_->selectionModel(), &QItemSelectionModel::currentColumnChanged, this,
           [this]() { updateAggregateFooter(); });
    connect(tableView_->selectionModel(), &QItemSelectionModel::currentRowChanged, this,
           [this]() { updateRecordView(); });

    transposeModel_ = new TransposeTableModel(model_, this);
    transposeView_ = new QTableView(this);
    transposeView_->setModel(transposeModel_);

    textView_ = new QPlainTextEdit(this);
    textView_->setReadOnly(true);
    textView_->setLineWrapMode(QPlainTextEdit::NoWrap);

    recordView_ = new QTreeView(this);
    recordView_->setModel(nullptr);

    viewStack_ = new QStackedWidget(this);
    viewStack_->addWidget(tableView_);
    viewStack_->addWidget(transposeView_);
    viewStack_->addWidget(textView_);
    viewStack_->addWidget(recordView_);
    layout->addWidget(viewStack_, 1);

    auto *footer = new QHBoxLayout();
    aggregateOpCombo_ = new QComboBox(this);
    aggregateOpCombo_->addItem(tr("Sum"), int(FfiDbAggOp::Sum));
    aggregateOpCombo_->addItem(tr("Avg"), int(FfiDbAggOp::Avg));
    aggregateOpCombo_->addItem(tr("Min"), int(FfiDbAggOp::Min));
    aggregateOpCombo_->addItem(tr("Max"), int(FfiDbAggOp::Max));
    aggregateOpCombo_->addItem(tr("Count"), int(FfiDbAggOp::Count));
    connect(aggregateOpCombo_, &QComboBox::currentIndexChanged, this,
           [this]() { updateAggregateFooter(); });
    footer->addWidget(aggregateOpCombo_);
    aggregateLabel_ = new QLabel(this);
    footer->addWidget(aggregateLabel_, 1);
    layout->addLayout(footer);

    statusLabel_ = new QLabel(this);
    layout->addWidget(statusLabel_);

    connect(consoleService_, &ConsoleService::aggregateComputed, this,
           [this](quint64 resultId, FfiDbAggOp op, const FfiDbAggregateOutcome &outcome) {
               Q_UNUSED(op);
               if (resultId != resultId_) {
                   return;
               }
               if (outcome.scope == FfiDbAggScope::FetchedRowsOnly) {
                   const QString column =
                     model_->columnNameAt(tableView_->currentIndex().column());
                   const QString fallback = provider_->aggregate(
                     resultId_, column, FfiDbAggOp(aggregateOpCombo_->currentData().toInt()));
                   aggregateLabel_->setText(
                     tr("%1 (from fetched rows only — %2)").arg(fallback, QString(outcome.reason)));
                   return;
               }
               aggregateLabel_->setText(outcome.ok
                                          ? tr("%1 (over all %2 rows)")
                                              .arg(QString(outcome.value))
                                              .arg(outcome.rowCount)
                                          : QString(outcome.value));
           });
    connect(consoleService_, &ConsoleService::referencingTargetsReady, this,
           [this](quint64 resultId, const QString &column, const ::rust::Vec<FfiDbNavTarget> &targets) {
               Q_UNUSED(column);
               if (resultId != resultId_ || targets.empty()) {
                   return;
               }
               QMenu menu(this);
               for (const FfiDbNavTarget &target : targets) {
                   QAction *action = menu.addAction(QString(target.label));
                   connect(action, &QAction::triggered, this,
                          [this, target]() { goToNavTarget(target); });
               }
               menu.exec(tableView_->viewport()->mapToGlobal(
                 tableView_->visualRect(tableView_->currentIndex()).center()));
           });

    windowShortcut(this, appSettings, QStringLiteral("database.submit"), [this]() { submit(); });
    windowShortcut(this, appSettings, QStringLiteral("database.revert"), [this]() { revert(); });
    windowShortcut(this, appSettings, QStringLiteral("database.addRow"), [this]() { addRow(); });
    windowShortcut(this, appSettings, QStringLiteral("database.deleteRow"),
                  [this]() { deleteSelectedRows(); });
    windowShortcut(this, appSettings, QStringLiteral("database.cloneRow"),
                  [this]() { cloneSelectedRow(); });
    windowShortcut(this, appSettings, QStringLiteral("database.previewDml"),
                  [this]() { previewDml(); });
    windowShortcut(this, appSettings, QStringLiteral("database.goToReferencedRow"),
                  [this]() { goToReferencedRow(); });
    windowShortcut(this, appSettings, QStringLiteral("database.showReferencingRows"),
                  [this]() { showReferencingRows(); });
    windowShortcut(this, appSettings, QStringLiteral("database.viewTable"),
                  [this]() { setViewMode(FfiDbViewMode::Table); });
    windowShortcut(this, appSettings, QStringLiteral("database.viewTranspose"),
                  [this]() { setViewMode(FfiDbViewMode::Transpose); });
    windowShortcut(this, appSettings, QStringLiteral("database.viewText"),
                  [this]() { setViewMode(FfiDbViewMode::Text); });
    windowShortcut(this, appSettings, QStringLiteral("database.viewRecord"),
                  [this]() { setViewMode(FfiDbViewMode::Record); });

    updateActionsEnabled();
}

void ResultGridView::setResultId(quint64 resultId)
{
    resultId_ = resultId;
    model_->setResultId(resultId);
    statusLabel_->setText(tr("Running…"));
    editableBanner_->setVisible(false);
    aggregateLabel_->clear();
    updateActionsEnabled();
    setViewMode(FfiDbViewMode::Table);
    if (onResultAdopted_) {
        onResultAdopted_(resultId);
    }
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
    refreshViewModes();
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

void ResultGridView::showCellContextMenu(const QPoint &pos)
{
    navigateFromIndex(tableView_->indexAt(pos), tableView_->viewport()->mapToGlobal(pos));
}

void ResultGridView::goToReferencedRow()
{
    const QModelIndex index = tableView_->currentIndex();
    navigateFromIndex(index, tableView_->viewport()->mapToGlobal(
                              tableView_->visualRect(index).center()));
}

void ResultGridView::navigateFromIndex(const QModelIndex &index, const QPoint &globalPos)
{
    if (resultId_ == 0 || !index.isValid()) {
        return;
    }
    const QString column = model_->columnNameAt(index.column());
    if (column.isEmpty()) {
        return;
    }
    const auto targets = consoleService_->cellNavigation(resultId_, quint64(index.row()), column);
    QMenu menu(this);
    for (const FfiDbNavTarget &target : targets) {
        QAction *action = menu.addAction(QString(target.label));
        connect(action, &QAction::triggered, this, [this, target]() { goToNavTarget(target); });
    }
    // F4d: reverse navigation is always offered — whether this column is
    // referenced by anything else is only known once
    // `referencingTargetsReady` answers (a whole-schema introspect, not a
    // cheap synchronous check like `cellNavigation`'s forward case).
    QAction *reverseAction = menu.addAction(tr("Show Referencing Rows…"));
    const quint64 row = quint64(index.row());
    connect(reverseAction, &QAction::triggered, this,
           [this, row, column]() { consoleService_->referencingTargets(resultId_, row, column); });
    if (menu.isEmpty()) {
        return;
    }
    menu.exec(globalPos);
}

void ResultGridView::goToNavTarget(const FfiDbNavTarget &target)
{
    const FfiResult result = consoleService_->goToNavTarget(resultId_, target);
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    // The new result id travels back in `message` — same convention as
    // `applyClauses`. Replaces this grid's own page with the target row,
    // still inside the same console tab.
    bool parsed = false;
    const quint64 newId = QString(result.message).toULongLong(&parsed);
    if (parsed) {
        setResultId(newId);
    }
}

void ResultGridView::showReferencingRows()
{
    if (resultId_ == 0) {
        return;
    }
    const QModelIndex index = tableView_->currentIndex();
    if (!index.isValid()) {
        return;
    }
    const QString column = model_->columnNameAt(index.column());
    if (column.isEmpty()) {
        return;
    }
    // Answered asynchronously through `referencingTargetsReady` — see the
    // constructor's own connection.
    consoleService_->referencingTargets(resultId_, quint64(index.row()), column);
}

void ResultGridView::setViewMode(FfiDbViewMode mode)
{
    tableModeButton_->setChecked(mode == FfiDbViewMode::Table);
    transposeModeButton_->setChecked(mode == FfiDbViewMode::Transpose);
    textModeButton_->setChecked(mode == FfiDbViewMode::Text);
    recordModeButton_->setChecked(mode == FfiDbViewMode::Record);
    textFormatCombo_->setVisible(mode == FfiDbViewMode::Text);
    switch (mode) {
    case FfiDbViewMode::Transpose:
        viewStack_->setCurrentWidget(transposeView_);
        break;
    case FfiDbViewMode::Text:
        viewStack_->setCurrentWidget(textView_);
        updateTextView();
        break;
    case FfiDbViewMode::Record:
        viewStack_->setCurrentWidget(recordView_);
        updateRecordView();
        break;
    case FfiDbViewMode::Table:
    default:
        viewStack_->setCurrentWidget(tableView_);
        break;
    }
}

void ResultGridView::refreshViewModes()
{
    if (resultId_ == 0) {
        return;
    }
    const FfiResultModes modes = provider_->resultModes(resultId_);
    transposeModeButton_->setEnabled(modes.canTranspose);
    textModeButton_->setEnabled(modes.canText);
    recordModeButton_->setEnabled(modes.canRecord);
    installColumnVisibilityMenu(columnsButton_, tableView_, provider_->columns(resultId_));
    setViewMode(modes.defaultMode);
}

void ResultGridView::updateTextView()
{
    if (resultId_ == 0 || viewStack_->currentWidget() != textView_) {
        return;
    }
    const auto format = FfiDbTextFormat(textFormatCombo_->currentData().toInt());
    textView_->setPlainText(provider_->textView(resultId_, format));
}

void ResultGridView::updateRecordView()
{
    if (resultId_ == 0 || viewStack_->currentWidget() != recordView_) {
        return;
    }
    const QModelIndex current = tableView_->currentIndex();
    const quint64 row = current.isValid() ? quint64(current.row()) : 0;
    QAbstractItemModel *old = recordView_->model();
    recordView_->setModel(buildRecordModel(provider_->recordRows(resultId_, row), recordView_));
    delete old;
    recordView_->expandAll();
    for (int i = 0; i < 3; ++i) {
        recordView_->resizeColumnToContents(i);
    }
}

void ResultGridView::updateAggregateFooter()
{
    if (resultId_ == 0) {
        aggregateLabel_->clear();
        return;
    }
    const QString column = model_->columnNameAt(tableView_->currentIndex().column());
    if (column.isEmpty()) {
        aggregateLabel_->clear();
        return;
    }
    aggregateLabel_->setText(tr("computing…"));
    const auto op = FfiDbAggOp(aggregateOpCombo_->currentData().toInt());
    consoleService_->aggregateExact(resultId_, column, op);
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
