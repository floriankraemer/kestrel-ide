#include "diff_divider.h"

#include "diff_pane.h"
#include "theme.h"

#include <QPainter>
#include <QPainterPath>
#include <QPlainTextEdit>

namespace ui_shell {

namespace {

struct HunkShade
{
    QColor fill;
    QColor edge;
};

HunkShade shadeFor(FfiHunkKind kind)
{
    const DiffColors colors = diffColors();
    switch (kind) {
    case FfiHunkKind::Removed:
        return {colors.deletedLine, colors.deletedInline};
    case FfiHunkKind::Modified:
        return {colors.modifiedLine, colors.modifiedInline};
    case FfiHunkKind::Added:
    default:
        return {colors.addedLine, colors.addedInline};
    }
}

} // namespace

DiffDivider::DiffDivider(QPlainTextEdit *leftEdit, QPlainTextEdit *rightEdit, QWidget *parent)
  : QWidget(parent)
  , leftEdit_(leftEdit)
  , rightEdit_(rightEdit)
{
    setFixedWidth(kWidth);
}

void DiffDivider::setHunks(const QVector<Hunk> &hunks)
{
    hunks_ = hunks;
    update();
}

int DiffDivider::paneYToLocal(const QPlainTextEdit *edit, int viewportY) const
{
    return mapFromGlobal(edit->viewport()->mapToGlobal(QPoint(0, viewportY))).y();
}

void DiffDivider::paintEvent(QPaintEvent *)
{
    QPainter painter(this);
    painter.fillRect(rect(), palette().color(QPalette::Base));
    painter.setRenderHint(QPainter::Antialiasing);

    const int leftFirst = firstVisibleBlockIn(leftEdit_);
    const int leftLast = lastVisibleBlockIn(leftEdit_);
    const int rightFirst = firstVisibleBlockIn(rightEdit_);
    const int rightLast = lastVisibleBlockIn(rightEdit_);
    const int leftLineHeight = leftEdit_->fontMetrics().height();
    const int rightLineHeight = rightEdit_->fontMetrics().height();

    for (const Hunk &hunk : hunks_) {
        // Off-screen on both sides: `cursorRect` walks from the first
        // visible block, so a hunk far away is not worth asking about.
        const bool aboveBoth = hunk.oldStart + hunk.oldLen < leftFirst
          && hunk.newStart + hunk.newLen < rightFirst;
        const bool belowBoth = hunk.oldStart > leftLast + 1 && hunk.newStart > rightLast + 1;
        if (aboveBoth || belowBoth) {
            continue;
        }

        const int leftTop = paneYToLocal(leftEdit_, blockTopIn(leftEdit_, hunk.oldStart));
        const int leftBottom = hunk.oldLen > 0
          ? paneYToLocal(leftEdit_, blockTopIn(leftEdit_, hunk.oldStart + hunk.oldLen - 1))
            + leftLineHeight
          : leftTop;
        const int rightTop = paneYToLocal(rightEdit_, blockTopIn(rightEdit_, hunk.newStart));
        const int rightBottom = hunk.newLen > 0
          ? paneYToLocal(rightEdit_, blockTopIn(rightEdit_, hunk.newStart + hunk.newLen - 1))
            + rightLineHeight
          : rightTop;

        QPainterPath path;
        const qreal midX = width() / 2.0;
        path.moveTo(0, leftTop);
        path.cubicTo(midX, leftTop, midX, rightTop, width(), rightTop);
        path.lineTo(width(), rightBottom);
        path.cubicTo(midX, rightBottom, midX, leftBottom, 0, leftBottom);
        path.closeSubpath();

        const HunkShade shade = shadeFor(hunk.kind);
        painter.fillPath(path, shade.fill);
        painter.setPen(QPen(shade.edge, 1));
        painter.drawPath(path);
    }
}

} // namespace ui_shell
