#include "diff_selections.h"

#include <QTextBlock>
#include <QTextCursor>
#include <QTextDocument>

#include <algorithm>

namespace ui_shell {

QList<QTextEdit::ExtraSelection> diffSelections(const QTextDocument *document,
                                                const QVector<DiffLineBackground> &backgrounds,
                                                const QVector<DiffInlineSpan> &spans)
{
    QList<QTextEdit::ExtraSelection> selections;
    selections.reserve(backgrounds.size() + spans.size());

    for (const DiffLineBackground &background : backgrounds) {
        const QTextBlock block = document->findBlockByNumber(background.block);
        if (!block.isValid()) {
            continue;
        }
        QTextEdit::ExtraSelection selection;
        selection.cursor = QTextCursor(block);
        selection.format.setBackground(background.color);
        // Without this the band stops at the end of the text on that line.
        selection.format.setProperty(QTextFormat::FullWidthSelection, true);
        selections.append(selection);
    }

    for (const DiffInlineSpan &span : spans) {
        const QTextBlock block = document->findBlockByNumber(span.block);
        if (!block.isValid()) {
            continue;
        }
        const int last = std::max(0, block.length() - 1);
        QTextCursor cursor(block);
        cursor.setPosition(block.position() + std::min(span.start, last));
        cursor.setPosition(block.position() + std::min(span.end, last), QTextCursor::KeepAnchor);
        QTextEdit::ExtraSelection selection;
        selection.cursor = cursor;
        selection.format.setBackground(span.color);
        selections.append(selection);
    }
    return selections;
}

} // namespace ui_shell
