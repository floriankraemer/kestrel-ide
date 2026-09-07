#include "diff_divider.h"

#include "diff_pane.h"
#include "e2e_mark.h"
#include "theme.h"

#include <QPainter>
#include <QPainterPath>
#include <QPlainTextEdit>
#include <QToolButton>

namespace ui_shell {

namespace {

constexpr int kChevronSide = 20;

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
    qDeleteAll(chevrons_);
    chevrons_.clear();
    markedRects_.clear();
    const QIcon icon =
      maskIcon(":/ui/icons/diff/chevron_apply.a8", palette().color(QPalette::Text));
    for (int i = 0; i < hunks_.size(); ++i) {
        auto *chevron = new QToolButton(this);
        chevron->setIcon(icon);
        chevron->setIconSize(QSize(14, 14));
        chevron->setFixedSize(kChevronSide, kChevronSide);
        chevron->setAutoRaise(true);
        chevron->setCursor(Qt::PointingHandCursor);
        chevron->setFocusPolicy(Qt::NoFocus);
        chevron->setToolTip(tr("Replace with left side"));
        chevron->setObjectName(QStringLiteral("diffChevron"));
        chevron->hide();
        connect(chevron, &QToolButton::clicked, this, [this, i] {
            if (onApplyHunk) {
                onApplyHunk(i);
            }
        });
        chevrons_.append(chevron);
    }
    update();
}

void DiffDivider::setEditable(bool editable)
{
    editable_ = editable;
    update();
}

int DiffDivider::paneYToLocal(const QPlainTextEdit *edit, int viewportY) const
{
    return mapFromGlobal(edit->viewport()->mapToGlobal(QPoint(0, viewportY))).y();
}

void DiffDivider::placeChevron(int index, int y, bool visible)
{
    if (index >= chevrons_.size()) {
        return;
    }
    QToolButton *chevron = chevrons_[index];
    if (!visible || !editable_) {
        chevron->hide();
        return;
    }
    chevron->move((width() - kChevronSide) / 2, y - kChevronSide / 2);
    chevron->show();
    // Only when it moved: repaint runs on every scroll step, and the E2E
    // stream is for state changes, not frames.
    const QRect global(chevron->mapToGlobal(QPoint(0, 0)), chevron->size());
    if (markedRects_.value(chevron) != global) {
        markedRects_.insert(chevron, global);
        e2eMark(QStringLiteral("{\"ev\":\"diff_chevron\",\"hunk_index\":%1,"
                               "\"rect\":[%2,%3,%4,%5]}")
                  .arg(index)
                  .arg(global.x())
                  .arg(global.y())
                  .arg(global.width())
                  .arg(global.height()));
    }
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

    for (int i = 0; i < hunks_.size(); ++i) {
        const Hunk &hunk = hunks_[i];
        // Off-screen on both sides: `cursorRect` walks from the first
        // visible block, so a hunk far away is not worth asking about.
        const bool aboveBoth = hunk.oldStart + hunk.oldLen < leftFirst
          && hunk.newStart + hunk.newLen < rightFirst;
        const bool belowBoth = hunk.oldStart > leftLast + 1 && hunk.newStart > rightLast + 1;
        if (aboveBoth || belowBoth) {
            placeChevron(i, 0, false);
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

        const int centreY = (leftTop + rightTop) / 2 + kChevronSide / 2;
        placeChevron(i, centreY, centreY >= 0 && centreY <= height());
    }
}

} // namespace ui_shell
