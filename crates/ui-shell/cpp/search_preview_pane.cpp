#include "search_preview_pane.h"

#include <QFile>
#include <QFontDatabase>
#include <QLabel>
#include <QPlainTextEdit>
#include <QTextBlock>
#include <QTextCursor>
#include <QVBoxLayout>

namespace ui_shell {

SearchPreviewPane::SearchPreviewPane(QWidget *parent)
  : QWidget(parent)
{
    pathLabel_ = new QLabel(this);
    pathLabel_->setStyleSheet(QStringLiteral("font-weight: bold;"));
    pathLabel_->setWordWrap(true);

    editor_ = new QPlainTextEdit(this);
    editor_->setReadOnly(true);
    editor_->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    editor_->setLineWrapMode(QPlainTextEdit::NoWrap);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(pathLabel_);
    layout->addWidget(editor_, 1);
}

void SearchPreviewPane::showMatch(const QString &path, int line, int start, int end)
{
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly | QIODevice::Text)) {
        clearPreview();
        return;
    }
    pathLabel_->setText(path);
    editor_->setPlainText(QString::fromUtf8(file.readAll()));

    QTextCursor cursor(editor_->document()->findBlockByNumber(qMax(0, line - 1)));
    const int lineStart = cursor.position();
    if (end > start) {
        cursor.setPosition(lineStart + start);
        cursor.setPosition(lineStart + end, QTextCursor::KeepAnchor);
    }
    editor_->setTextCursor(cursor);
    editor_->centerCursor();
}

void SearchPreviewPane::clearPreview()
{
    pathLabel_->clear();
    editor_->clear();
}

} // namespace ui_shell
