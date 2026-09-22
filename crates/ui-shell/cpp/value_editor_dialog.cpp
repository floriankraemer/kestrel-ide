#include "value_editor_dialog.h"

#include <QDialogButtonBox>
#include <QFile>
#include <QFileDialog>
#include <QHBoxLayout>
#include <QLabel>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTextStream>
#include <QVBoxLayout>

namespace ui_shell {

ValueEditorDialog::ValueEditorDialog(ResultProvider *provider, quint64 resultId, quint64 row,
                                     const QString &column, const QString &initialText,
                                     QWidget *parent)
  : QDialog(parent)
  , provider_(provider)
  , resultId_(resultId)
  , row_(row)
  , column_(column)
  , rawText_(initialText)
{
    setWindowTitle(tr("Edit Value — %1").arg(column));
    resize(560, 420);

    auto *layout = new QVBoxLayout(this);

    editor_ = new QPlainTextEdit(this);
    editor_->setPlainText(initialText);
    layout->addWidget(editor_, 1);

    statusLabel_ = new QLabel(this);
    statusLabel_->setWordWrap(true);
    layout->addWidget(statusLabel_);

    auto *buttonRow = new QHBoxLayout();
    auto *nullButton = new QPushButton(tr("NULL"), this);
    connect(nullButton, &QPushButton::clicked, this, &ValueEditorDialog::setNull);
    buttonRow->addWidget(nullButton);

    auto *defaultButton = new QPushButton(tr("DEFAULT"), this);
    connect(defaultButton, &QPushButton::clicked, this, &ValueEditorDialog::setDefault);
    buttonRow->addWidget(defaultButton);

    auto *prettyButton = new QPushButton(tr("Pretty JSON"), this);
    connect(prettyButton, &QPushButton::clicked, this, &ValueEditorDialog::togglePrettyJson);
    buttonRow->addWidget(prettyButton);

    auto *loadButton = new QPushButton(tr("Load from File..."), this);
    connect(loadButton, &QPushButton::clicked, this, &ValueEditorDialog::loadFromFile);
    buttonRow->addWidget(loadButton);

    auto *saveToFileButton = new QPushButton(tr("Save to File..."), this);
    connect(saveToFileButton, &QPushButton::clicked, this, &ValueEditorDialog::saveToFile);
    buttonRow->addWidget(saveToFileButton);

    buttonRow->addStretch(1);
    layout->addLayout(buttonRow);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel, this);
    connect(buttons, &QDialogButtonBox::accepted, this, &ValueEditorDialog::save);
    connect(buttons, &QDialogButtonBox::rejected, this, &ValueEditorDialog::reject);
    layout->addWidget(buttons);
}

void ValueEditorDialog::save()
{
    const FfiResult result =
      provider_->setCell(resultId_, row_, column_, editor_->toPlainText());
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    accept();
}

void ValueEditorDialog::setNull()
{
    const FfiResult result = provider_->setNull(resultId_, row_, column_);
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    accept();
}

void ValueEditorDialog::setDefault()
{
    const FfiResult result = provider_->setDefault(resultId_, row_, column_);
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    accept();
}

void ValueEditorDialog::togglePrettyJson()
{
    if (prettyShown_) {
        editor_->setPlainText(rawText_);
        prettyShown_ = false;
        return;
    }
    const FfiResult result = provider_->prettyJson(editor_->toPlainText());
    if (result.code != 0) {
        statusLabel_->setText(QString(result.message));
        return;
    }
    rawText_ = editor_->toPlainText();
    editor_->setPlainText(QString(result.message));
    prettyShown_ = true;
}

void ValueEditorDialog::loadFromFile()
{
    const QString path = QFileDialog::getOpenFileName(this, tr("Load Value From File"));
    if (path.isEmpty()) {
        return;
    }
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly | QIODevice::Text)) {
        QMessageBox::warning(this, tr("Load from File"), file.errorString());
        return;
    }
    QTextStream stream(&file);
    editor_->setPlainText(stream.readAll());
    prettyShown_ = false;
}

void ValueEditorDialog::saveToFile()
{
    const QString path = QFileDialog::getSaveFileName(this, tr("Save Value To File"));
    if (path.isEmpty()) {
        return;
    }
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Text)) {
        QMessageBox::warning(this, tr("Save to File"), file.errorString());
        return;
    }
    QTextStream stream(&file);
    stream << editor_->toPlainText();
}

} // namespace ui_shell
