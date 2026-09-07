// The editor minimap (code map, issue #199): a scaled, syntax-coloured
// rendering of the whole file on the right edge of a CodeEditor, its own
// translation unit for the same reason code_editor_gutter.cpp is one — a
// self-contained strip, pushed decorations from outside and deciding none
// of them itself.

#include "minimap.h"

#include "code_editor.h"
#include "e2e_mark.h"
#include "theme.h"

#include <QCoreApplication>
#include <QMouseEvent>
#include <QPainter>
#include <QPaintEvent>
#include <QResizeEvent>
#include <QScrollBar>
#include <QTextBlock>
#include <QTextLayout>
#include <QWheelEvent>

namespace ui_shell {

namespace {

// One rendered pixel row per source line, at one pixel per column — a scaled
// map, not a legible second copy of the text. `kMaxColumns` bounds both the
// per-row work and the strip's width, matching VS Code's `minimap.maxColumn`.
constexpr int kRowHeight = 2;
constexpr int kCharWidth = 1;
constexpr int kMaxColumns = 110;
constexpr int kWidth = kMaxColumns * kCharWidth;

} // namespace

Minimap::Minimap(CodeEditor *editor)
  : QWidget(editor)
  , editor_(editor)
{
    setMouseTracking(true);
}

int Minimap::preferredWidth()
{
    return kWidth;
}

void Minimap::setOptions(const MinimapOptions &options)
{
    if (options_.enabled == options.enabled && options_.searchMatches == options.searchMatches
        && options_.diagnostics == options.diagnostics && options_.vcsChanges == options.vcsChanges
        && options_.breakpoints == options.breakpoints
        && options_.caretLine == options.caretLine) {
        return;
    }
    options_ = options;
    update();
}

int Minimap::totalRows() const
{
    QScrollBar *scrollBar = editor_->verticalScrollBar();
    return scrollBar->maximum() + scrollBar->pageStep();
}

int Minimap::visibleRows() const
{
    return qMax(1, height() / kRowHeight);
}

int Minimap::firstRow() const
{
    const int total = totalRows();
    const int visible = visibleRows();
    if (total <= visible) {
        return 0;
    }
    QScrollBar *scrollBar = editor_->verticalScrollBar();
    const int maxScroll = qMax(1, scrollBar->maximum());
    const double fraction = static_cast<double>(scrollBar->value()) / maxScroll;
    return static_cast<int>((total - visible) * fraction);
}

int Minimap::sliderHeightPx() const
{
    return qMax(kRowHeight, editor_->verticalScrollBar()->pageStep() * kRowHeight);
}

QRect Minimap::sliderRect() const
{
    QScrollBar *scrollBar = editor_->verticalScrollBar();
    const int first = firstRow();
    const int top = (scrollBar->value() - first) * kRowHeight;
    return QRect(0, top, width(), sliderHeightPx());
}

int Minimap::sliderTravelPx() const
{
    // How far sliderRect().top() moves between scroll value 0 and
    // maximum(): the range a press or drag maps pixels onto. A file taller
    // than the strip scrolls the map under the slider, so the slider stops
    // at `height() - sliderHeightPx()`. A file that fits the strip whole
    // has a fixed map, and the slider stops at that file's last row
    // instead — mapping such a file over the full track made the slider
    // move slower than the pointer and drift away from it over a drag,
    // which is why a short file's minimap looked broken next to a long
    // file's in the same split.
    QScrollBar *scrollBar = editor_->verticalScrollBar();
    const int rows = qMin(scrollBar->maximum(), visibleRows() - scrollBar->pageStep());
    return qMax(1, rows * kRowHeight);
}

void Minimap::scrollToPixelY(int y)
{
    QScrollBar *scrollBar = editor_->verticalScrollBar();
    const int maxScroll = scrollBar->maximum();
    if (maxScroll <= 0) {
        return;
    }
    const int travel = sliderTravelPx();
    const int clampedY = qBound(0, y, travel);
    const double fraction = static_cast<double>(clampedY) / travel;
    scrollBar->setValue(static_cast<int>(fraction * maxScroll));
}

int Minimap::visibleRowForBlock(int blockNumber) const
{
    QTextBlock target = editor_->document()->findBlockByNumber(blockNumber);
    if (!target.isValid() || !target.isVisible()) {
        return -1;
    }
    int visibleIndex = 0;
    for (QTextBlock block = editor_->document()->begin(); block.isValid() && block != target;
         block = block.next()) {
        if (block.isVisible()) {
            ++visibleIndex;
        }
    }
    return visibleIndex;
}

void Minimap::ensureCodeCache(int firstRow, int rowCount)
{
    const QColor base = palette().color(QPalette::Base);
    const unsigned int baseColor = base.rgba();
    const int revision = editor_->document()->revision();
    if (revision == cacheRevision_ && firstRow == cacheFirstRow_ && width() == cacheWidth_
        && height() == cacheHeight_ && baseColor == cacheBaseColor_) {
        return;
    }

    codeCache_ = QPixmap(qMax(1, width()), qMax(1, height()));
    codeCache_.fill(tinted(base, 130, 106));
    QPainter painter(&codeCache_);
    renderCode(painter, firstRow, rowCount);

    cacheRevision_ = revision;
    cacheFirstRow_ = firstRow;
    cacheWidth_ = width();
    cacheHeight_ = height();
    cacheBaseColor_ = baseColor;
}

void Minimap::renderCode(QPainter &painter, int firstRow, int rowCount)
{
    QTextBlock block = editor_->document()->begin();
    int visibleIndex = 0;
    while (block.isValid() && visibleIndex < firstRow) {
        if (block.isVisible()) {
            ++visibleIndex;
        }
        block = block.next();
    }

    const QColor defaultColor = editor_->palette().color(QPalette::Text);
    int paintedRow = 0;
    while (block.isValid() && paintedRow < rowCount) {
        if (block.isVisible()) {
            const QString text = block.text();
            const int columns = qMin(text.size(), kMaxColumns);
            const QVector<QTextLayout::FormatRange> formats =
              block.layout() != nullptr ? block.layout()->formats()
                                         : QVector<QTextLayout::FormatRange>();

            auto colorAt = [&](int column) {
                for (const QTextLayout::FormatRange &range : formats) {
                    if (column >= range.start && column < range.start + range.length) {
                        const QBrush foreground = range.format.foreground();
                        if (foreground.style() != Qt::NoBrush) {
                            return foreground.color();
                        }
                        break;
                    }
                }
                return defaultColor;
            };

            int column = 0;
            const int y = paintedRow * kRowHeight;
            while (column < columns) {
                if (text.at(column).isSpace()) {
                    ++column;
                    continue;
                }
                const QColor color = colorAt(column);
                const int runStart = column;
                ++column;
                while (column < columns && !text.at(column).isSpace()
                       && colorAt(column) == color) {
                    ++column;
                }
                QColor painted = color;
                painted.setAlpha(200);
                painter.fillRect(runStart * kCharWidth, y, (column - runStart) * kCharWidth,
                                  kRowHeight, painted);
            }
            ++paintedRow;
        }
        block = block.next();
    }
}

void Minimap::paintOverlays(QPainter &painter, int firstRow, int rowCount)
{
    // Every mark is alpha-blended over the code pixmap rather than painted
    // opaque: a dense overlay (a find with hundreds of matches, a file that
    // is mostly changed) must still read as a density map of the code
    // underneath, not a solid bar that erases the shape decision #3 exists
    // for. Caret line is deliberately the faintest — it is a "you are here"
    // hint, not a fact about the file the way a diagnostic or a change is.
    auto paintRowMark = [&](int blockNumber, QColor color, int alpha) {
        const int row = visibleRowForBlock(blockNumber) - firstRow;
        if (row < 0 || row >= rowCount) {
            return;
        }
        color.setAlpha(alpha);
        painter.fillRect(0, row * kRowHeight, width(), kRowHeight, color);
    };

    if (options_.vcsChanges) {
        const QHash<int, ChangeMarker> &markers = editor_->changeMarkers();
        for (auto it = markers.constBegin(); it != markers.constEnd(); ++it) {
            paintRowMark(it.key(), changeMarkerColor(it.value().kind), 170);
        }
    }

    if (options_.diagnostics) {
        for (const DiagnosticSpan &span : editor_->diagnosticSpans()) {
            const int blockNumber = editor_->document()->findBlock(span.start).blockNumber();
            paintRowMark(blockNumber, span.color, 170);
        }
    }

    if (options_.searchMatches) {
        const QColor matchColor = palette().color(QPalette::Highlight);
        for (const QPair<int, int> &match : editor_->matchSelections()) {
            const int blockNumber = editor_->document()->findBlock(match.first).blockNumber();
            paintRowMark(blockNumber, matchColor, 130);
        }
    }

    if (options_.breakpoints) {
        const QColor breakpointColor(220, 60, 60);
        for (int blockNumber : editor_->breakpointLines()) {
            paintRowMark(blockNumber, breakpointColor, 200);
        }
    }

    if (options_.caretLine) {
        paintRowMark(editor_->textCursor().blockNumber(), palette().color(QPalette::Highlight),
                     90);
    }
}

void Minimap::paintSlider(QPainter &painter)
{
    QColor color = palette().color(QPalette::Highlight);
    color.setAlpha(dragging_ || hovered_ ? 76 : 46);
    painter.fillRect(sliderRect(), color);
}

void Minimap::paintEvent(QPaintEvent * /*event*/)
{
    QPainter painter(this);

    if (!options_.enabled) {
        painter.fillRect(rect(), tinted(palette().color(QPalette::Base), 130, 106));
        return;
    }

    const int first = firstRow();
    const int rowCount = qMin(visibleRows(), totalRows() - first);
    ensureCodeCache(first, rowCount);
    painter.drawPixmap(0, 0, codeCache_);

    paintOverlays(painter, first, rowCount);
    paintSlider(painter);
}

void Minimap::mousePressEvent(QMouseEvent *event)
{
    if (!options_.enabled) {
        return;
    }
    const QRect slider = sliderRect();
    if (slider.contains(event->pos())) {
        dragging_ = true;
        dragGrabOffsetY_ = event->pos().y() - slider.top();
    } else {
        dragging_ = true;
        dragGrabOffsetY_ = slider.height() / 2;
        scrollToPixelY(event->pos().y() - dragGrabOffsetY_);
    }
    update();
}

void Minimap::mouseMoveEvent(QMouseEvent *event)
{
    const bool wasHovered = hovered_;
    hovered_ = sliderRect().contains(event->pos());
    if (dragging_) {
        scrollToPixelY(event->pos().y() - dragGrabOffsetY_);
    }
    if (dragging_ || hovered_ != wasHovered) {
        update();
    }
}

void Minimap::mouseReleaseEvent(QMouseEvent * /*event*/)
{
    if (dragging_) {
        dragging_ = false;
        e2eMark(QStringLiteral("{\"ev\":\"minimap_scrolled\",\"first_line\":%1}")
                  .arg(editor_->verticalScrollBar()->value()));
        update();
    }
}

void Minimap::wheelEvent(QWheelEvent *event)
{
    QCoreApplication::sendEvent(editor_->verticalScrollBar(), event);
}

void Minimap::resizeEvent(QResizeEvent * /*event*/)
{
    const QRect screenRect(mapToGlobal(QPoint(0, 0)), size());
    e2eMark(QStringLiteral("{\"ev\":\"minimap_shown\","
                            "\"rect\":[%1,%2,%3,%4]}")
              .arg(screenRect.x())
              .arg(screenRect.y())
              .arg(screenRect.width())
              .arg(screenRect.height()));
}

void Minimap::leaveEvent(QEvent * /*event*/)
{
    if (hovered_) {
        hovered_ = false;
        update();
    }
}

} // namespace ui_shell
