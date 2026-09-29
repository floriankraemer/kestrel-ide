#include "intention_bulb.h"

#include "theme.h"

#include <QPainter>
#include <QPainterPath>
#include <QPixmap>

namespace ui_shell {

namespace {
// The design's 14x14 box.
constexpr qreal kBox = 14.0;

QPixmap blankPixmap(int logicalSize, qreal dpr)
{
    QPixmap pixmap(QSize(logicalSize, logicalSize) * dpr);
    pixmap.setDevicePixelRatio(dpr);
    pixmap.fill(Qt::transparent);
    return pixmap;
}
} // namespace

void paintBulb(QPainter &painter, const QRectF &rect, BulbKind kind)
{
    const SemanticColors colors = semanticColors();
    const QColor fill = kind == BulbKind::Fix ? colors.error : colors.warning;
    painter.save();
    painter.setRenderHint(QPainter::Antialiasing);
    painter.translate(rect.topLeft());
    painter.scale(rect.width() / kBox, rect.height() / kBox);
    painter.setPen(Qt::NoPen);
    painter.setBrush(fill);

    QPainterPath glass;
    // Glass: the mockup's path `M7 1a4.3 4.3 0 0 0-2.6 7.7c.4.3.6.8.6 1.3V11h4v-1
    // c0-.5.2-1 .6-1.3A4.3 4.3 0 0 0 7 1z`, traced with the same arcs.
    glass.moveTo(7, 1);
    glass.arcTo(QRectF(2.7, 1, 8.6, 8.6), 90, 142.6);
    glass.cubicTo(QPointF(4.8, 9.0), QPointF(5.0, 9.5), QPointF(5.0, 10.0));
    glass.lineTo(5, 11);
    glass.lineTo(9, 11);
    glass.lineTo(9, 10);
    glass.cubicTo(QPointF(9.0, 9.5), QPointF(9.2, 9.0), QPointF(9.6, 8.7));
    glass.arcTo(QRectF(2.7, 1, 8.6, 8.6), 307.4, 142.6);
    glass.closeSubpath();
    painter.drawPath(glass);
    painter.drawRoundedRect(QRectF(5, 11.6, 4, 1.6), 0.5, 0.5);

    if (kind == BulbKind::Fix) {
        // Meaning must not rest on hue alone: a dark "!" inside the glass.
        painter.setBrush(fill.darker(300));
        painter.drawRoundedRect(QRectF(6.4, 2.9, 1.2, 3.4), 0.5, 0.5);
        painter.drawEllipse(QPointF(7.0, 7.7), 0.75, 0.75);
    }
    painter.restore();
}

QIcon bulbIcon(BulbKind kind, int logicalSize, qreal devicePixelRatio)
{
    QPixmap pixmap = blankPixmap(logicalSize, devicePixelRatio);
    QPainter painter(&pixmap);
    paintBulb(painter, QRectF(0, 0, logicalSize, logicalSize), kind);
    painter.end();
    return QIcon(pixmap);
}

QIcon blankBulbIcon(int logicalSize, qreal devicePixelRatio)
{
    return QIcon(blankPixmap(logicalSize, devicePixelRatio));
}

} // namespace ui_shell
