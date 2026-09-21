#include "run_config_sql_page.h"

#include <QCheckBox>
#include <QComboBox>
#include <QFileDialog>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QSignalBlocker>
#include <QToolButton>
#include <QVBoxLayout>

namespace ui_shell {

SqlScriptOptionsPage::SqlScriptOptionsPage(ConsoleService *consoleService, QWidget *parent)
  : QWidget(parent)
  , consoleService_(consoleService)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);

    const auto addRow = [this, layout](const QString &label, QWidget *field) {
        auto *row = new QHBoxLayout();
        auto *labelWidget = new QLabel(label, this);
        labelWidget->setMinimumWidth(90);
        row->addWidget(labelWidget);
        row->addWidget(field, 1);
        layout->addLayout(row);
    };

    sourceCombo_ = new QComboBox(this);
    if (consoleService_ != nullptr) {
        for (const FfiDbSourceRow &source : consoleService_->availableSources()) {
            sourceCombo_->addItem(QString(source.name), QString(source.id));
        }
    }
    connect(sourceCombo_, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            &SqlScriptOptionsPage::changed);
    addRow(tr("Data source:"), sourceCombo_);

    auto *fileRow = new QWidget(this);
    auto *fileRowLayout = new QHBoxLayout(fileRow);
    fileRowLayout->setContentsMargins(0, 0, 0, 0);
    fileEdit_ = new QLineEdit(fileRow);
    connect(fileEdit_, &QLineEdit::textChanged, this, &SqlScriptOptionsPage::changed);
    fileRowLayout->addWidget(fileEdit_, 1);
    browseButton_ = new QToolButton(fileRow);
    browseButton_->setText(tr("..."));
    connect(browseButton_, &QToolButton::clicked, this, &SqlScriptOptionsPage::browseForFile);
    fileRowLayout->addWidget(browseButton_);
    addRow(tr("SQL file:"), fileRow);

    txModeCombo_ = new QComboBox(this);
    txModeCombo_->addItem(tr("Auto-commit each statement"), QStringLiteral(""));
    txModeCombo_->addItem(tr("Single transaction"), QStringLiteral("single_transaction"));
    connect(txModeCombo_, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            &SqlScriptOptionsPage::changed);
    addRow(tr("Transaction mode:"), txModeCombo_);

    stopOnErrorCheck_ = new QCheckBox(tr("Stop on first error"), this);
    stopOnErrorCheck_->setChecked(true);
    connect(stopOnErrorCheck_, &QCheckBox::toggled, this, &SqlScriptOptionsPage::changed);
    layout->addWidget(stopOnErrorCheck_);
}

void SqlScriptOptionsPage::setOptions(const FfiSqlScriptOptions &options)
{
    const QSignalBlocker sourceBlocker(sourceCombo_);
    const QSignalBlocker fileBlocker(fileEdit_);
    const QSignalBlocker txBlocker(txModeCombo_);
    const QSignalBlocker stopBlocker(stopOnErrorCheck_);
    const int sourceIndex = sourceCombo_->findData(QString(options.source_id));
    sourceCombo_->setCurrentIndex(sourceIndex >= 0 ? sourceIndex : -1);
    fileEdit_->setText(QString(options.file));
    const int txIndex = txModeCombo_->findData(QString(options.tx_mode));
    txModeCombo_->setCurrentIndex(txIndex >= 0 ? txIndex : 0);
    stopOnErrorCheck_->setChecked(options.stop_on_error);
}

FfiSqlScriptOptions SqlScriptOptionsPage::options() const
{
    FfiSqlScriptOptions options{};
    options.source_id = sourceCombo_->currentData().toString();
    options.file = fileEdit_->text();
    options.tx_mode = txModeCombo_->currentData().toString();
    options.stop_on_error = stopOnErrorCheck_->isChecked();
    return options;
}

void SqlScriptOptionsPage::browseForFile()
{
    const QString path =
      QFileDialog::getOpenFileName(this, tr("Select SQL File"), fileEdit_->text(),
                                   tr("SQL files (*.sql);;All files (*)"));
    if (!path.isEmpty()) {
        fileEdit_->setText(path);
    }
}

} // namespace ui_shell
