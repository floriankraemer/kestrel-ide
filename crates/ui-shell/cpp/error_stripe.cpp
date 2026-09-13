#include "error_stripe.h"

#include "code_editor.h"
#include "editor_popup.h"

#include <QMouseEvent>
#include <QPainter>
#include <QScrollBar>
#include <QTextBlock>
#include <QTextCursor>
#include <QTextDocument>

namespace ui_shell {

namespace {

constexpr int kWidth = 8;
constexpr int kTickHeight = 3;
// The file-level summary badge at the very top of the strip, clear of any
// tick a line-0 diagnostic would otherwise paint under it.
constexpr int kSummaryHeight = 6;

// Same visible-row counting ADR-0044's minimap uses for its overlay rows
// (`Minimap::visibleRowForBlock`): a fold hides blocks, so a raw block
// number is not a row position on screen.
//
// ponytail: O(document blocks) per call, same accepted cost the minimap's
// own copy carries — a file whose diagnostics number in the thousands would
// pay for a walk per tick; upgrade path is the same one ADR-0044 names for
// its own copy, a single per-paint block-number -> visible-row array.
int visibleRowForBlock(const QTextDocument *document, int blockNumber)
{
    const QTextBlock target = document->findBlockByNumber(blockNumber);
    if (!target.isValid() || !target.isVisible()) {
        return -1;
    }
    int visibleIndex = 0;
    for (QTextBlock block = document->begin(); block.isValid() && block != target;
         block = block.next()) {
        if (block.isVisible()) {
            ++visibleIndex;
        }
    }
    return visibleIndex;
}

// `value()`/`maximum()+pageStep()` already count visible lines (ADR-0044
// #2), so the strip needs no second total.
int totalVisibleRows(CodeEditor *editor)
{
    QScrollBar *scrollBar = editor->verticalScrollBar();
    return qMax(1, scrollBar->maximum() + scrollBar->pageStep());
}

int yForVisibleRow(CodeEditor *editor, int visibleRow, int paintableHeight)
{
    return kSummaryHeight
      + visibleRow * (paintableHeight - kSummaryHeight) / totalVisibleRows(editor);
}

// The diagnostic mark whose tick is closest to `y`, within half a tick's
// height — the same "closest, not exact" hit test a file taller than the
// strip needs, since several lines can then share one on-screen pixel.
bool markNear(CodeEditor *editor, int y, int paintableHeight, int *outLine, QString *outMessage)
{
    const QHash<int, DiagnosticMark> &marks = editor->diagnosticMarks();
    int bestLine = -1;
    int bestDistance = kTickHeight;
    for (auto it = marks.constBegin(); it != marks.constEnd(); ++it) {
        const int row = visibleRowForBlock(editor->document(), it.key());
        if (row < 0) {
            continue;
        }
        const int tickY = yForVisibleRow(editor, row, paintableHeight);
        const int distance = qAbs(tickY - y);
        if (distance <= bestDistance) {
            bestDistance = distance;
            bestLine = it.key();
            *outMessage = it->message;
        }
    }
    if (bestLine < 0) {
        return false;
    }
    *outLine = bestLine;
    return true;
}

} // namespace

ErrorStripe::ErrorStripe(CodeEditor *editor)
  : QWidget(editor)
  , editor_(editor)
{
    setMouseTracking(true);
}

int ErrorStripe::preferredWidth()
{
    return kWidth;
}

void ErrorStripe::paintEvent(QPaintEvent * /*event*/)
{
    QPainter painter(this);
    painter.fillRect(rect(), palette().color(QPalette::Base));

    const DiagnosticSummary &summary = editor_->diagnosticSummary();
    if (summary.worstColor.isValid()) {
        painter.fillRect(1, 1, width() - 2, kSummaryHeight - 1, summary.worstColor);
    }

    const QHash<int, DiagnosticMark> &marks = editor_->diagnosticMarks();
    for (auto it = marks.constBegin(); it != marks.constEnd(); ++it) {
        const int row = visibleRowForBlock(editor_->document(), it.key());
        if (row < 0) {
            continue;
        }
        const int y = yForVisibleRow(editor_, row, height());
        painter.fillRect(1, y, width() - 2, kTickHeight, it->color);
    }
}

void ErrorStripe::mousePressEvent(QMouseEvent *event)
{
    const int y = static_cast<int>(event->position().y());
    if (y < kSummaryHeight) {
        return;
    }
    int line = -1;
    QString message;
    if (!markNear(editor_, y, height(), &line, &message)) {
        return;
    }
    QTextCursor cursor(editor_->document()->findBlockByNumber(line));
    editor_->setTextCursor(cursor);
    editor_->centerCursor();
}

void ErrorStripe::mouseMoveEvent(QMouseEvent *event)
{
    const int y = static_cast<int>(event->position().y());
    const DiagnosticSummary &summary = editor_->diagnosticSummary();
    if (y < kSummaryHeight) {
        if (summary.errors == 0 && summary.warnings == 0 && summary.infos == 0
            && summary.hints == 0) {
            hideEditorPopup();
            return;
        }
        showEditorPopup(mapToGlobal(event->pos()),
                         tr("%1 error(s), %2 warning(s)")
                           .arg(summary.errors)
                           .arg(summary.warnings)
                           .toHtmlEscaped());
        return;
    }

    int line = -1;
    QString message;
    if (!markNear(editor_, y, height(), &line, &message)) {
        hideEditorPopup();
        return;
    }
    showEditorPopup(mapToGlobal(event->pos()), message.toHtmlEscaped());
}

void ErrorStripe::leaveEvent(QEvent * /*event*/)
{
    hideEditorPopup();
}

} // namespace ui_shell
