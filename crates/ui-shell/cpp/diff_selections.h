#pragma once

#include <QColor>
#include <QList>
#include <QTextEdit>
#include <QVector>

class QTextDocument;

namespace ui_shell {

// A changed line's full-width background — `block` is a 0-based
// `QTextBlock` number in the pane's own document.
struct DiffLineBackground
{
    int block;
    QColor color;
};

// A changed span within a line — `start`/`end` are UTF-16 offsets into
// `block`, the units `FfiInlineSpan` already carries.
struct DiffInlineSpan
{
    int block;
    int start;
    int end;
    QColor color;
};

// The `QTextEdit::ExtraSelection`s that paint a diff over `document`:
// line backgrounds first, inline spans after, so the stronger shade wins
// where both apply. One builder for every pane that shows a diff — the
// read-only `DiffPane`s and the live `CodeEditor` in the editable window —
// so the two can never paint the same hunk differently.
QList<QTextEdit::ExtraSelection> diffSelections(const QTextDocument *document,
                                                const QVector<DiffLineBackground> &backgrounds,
                                                const QVector<DiffInlineSpan> &spans);

} // namespace ui_shell
