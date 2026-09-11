#include "image_viewer.h"

#include <algorithm>

#include <QKeyEvent>
#include <QPainter>
#include <QPaintEvent>
#include <QPalette>
#include <QResizeEvent>
#include <QScrollBar>
#include <QWheelEvent>

namespace ui_shell {

namespace {

constexpr qreal kZoomStep = 1.25;
constexpr qreal kMinZoom = 0.05;
constexpr qreal kMaxZoom = 20.0;

} // namespace

ImageViewer::ImageViewer(QWidget *parent)
  : QAbstractScrollArea(parent)
{
    viewport()->setBackgroundRole(QPalette::Base);
    viewport()->setAutoFillBackground(true);
    setFocusPolicy(Qt::StrongFocus);
}

void ImageViewer::setImage(QImage image)
{
    image_ = std::move(image);
    zoom_ = 1.0;
    rebuildScaledPixmap();
    updateScrollRange();
    viewport()->update();
}

qreal ImageViewer::baseScale() const
{
    if (image_.isNull() || image_.width() <= 0 || image_.height() <= 0) {
        return 1.0;
    }
    const qreal vw = std::max(1, viewport()->width());
    const qreal vh = std::max(1, viewport()->height());
    // 1:1 if it fits, else scaled down to fit — never upscale
    // past 1:1 just because the viewport happens to be roomy.
    return std::min(1.0, std::min(vw / image_.width(), vh / image_.height()));
}

void ImageViewer::setZoom(qreal zoom)
{
    zoom_ = std::clamp(zoom, kMinZoom, kMaxZoom);
    rebuildScaledPixmap();
    updateScrollRange();
    viewport()->update();
}

void ImageViewer::rebuildScaledPixmap()
{
    if (image_.isNull()) {
        scaled_ = QPixmap();
        return;
    }
    const qreal scale = effectiveScale();
    const QSize target(std::max(1, qRound(image_.width() * scale)),
                        std::max(1, qRound(image_.height() * scale)));
    scaled_ = QPixmap::fromImage(
      image_.scaled(target, Qt::KeepAspectRatio, Qt::SmoothTransformation));
}

void ImageViewer::updateScrollRange()
{
    const int vw = viewport()->width();
    const int vh = viewport()->height();
    horizontalScrollBar()->setRange(0, std::max(0, scaled_.width() - vw));
    horizontalScrollBar()->setPageStep(std::max(1, vw));
    verticalScrollBar()->setRange(0, std::max(0, scaled_.height() - vh));
    verticalScrollBar()->setPageStep(std::max(1, vh));
}

void ImageViewer::resizeEvent(QResizeEvent *event)
{
    QAbstractScrollArea::resizeEvent(event);
    // The base ("fits the viewport") scale depends on the viewport size, so
    // a resize can change it even at the same zoom level.
    rebuildScaledPixmap();
    updateScrollRange();
}

void ImageViewer::wheelEvent(QWheelEvent *event)
{
    if (event->modifiers() & Qt::ControlModifier) {
        setZoom(event->angleDelta().y() > 0 ? zoom_ * kZoomStep : zoom_ / kZoomStep);
        event->accept();
        return;
    }
    QAbstractScrollArea::wheelEvent(event);
}

void ImageViewer::keyPressEvent(QKeyEvent *event)
{
    switch (event->key()) {
    case Qt::Key_Plus:
    case Qt::Key_Equal:
        setZoom(zoom_ * kZoomStep);
        event->accept();
        return;
    case Qt::Key_Minus:
        setZoom(zoom_ / kZoomStep);
        event->accept();
        return;
    case Qt::Key_0:
        setZoom(1.0);
        event->accept();
        return;
    default:
        break;
    }

    QScrollBar *vertical = verticalScrollBar();
    QScrollBar *horizontal = horizontalScrollBar();
    switch (event->key()) {
    case Qt::Key_Up:
        vertical->triggerAction(QAbstractSlider::SliderSingleStepSub);
        break;
    case Qt::Key_Down:
        vertical->triggerAction(QAbstractSlider::SliderSingleStepAdd);
        break;
    case Qt::Key_PageUp:
        vertical->triggerAction(QAbstractSlider::SliderPageStepSub);
        break;
    case Qt::Key_PageDown:
        vertical->triggerAction(QAbstractSlider::SliderPageStepAdd);
        break;
    case Qt::Key_Left:
        horizontal->triggerAction(QAbstractSlider::SliderSingleStepSub);
        break;
    case Qt::Key_Right:
        horizontal->triggerAction(QAbstractSlider::SliderSingleStepAdd);
        break;
    default:
        QAbstractScrollArea::keyPressEvent(event);
        return;
    }
    event->accept();
}

void ImageViewer::paintEvent(QPaintEvent *)
{
    QPainter painter(viewport());

    if (image_.isNull()) {
        painter.setPen(palette().color(QPalette::Text));
        painter.drawText(viewport()->rect(), Qt::AlignCenter, tr("Couldn't load image"));
        return;
    }

    const int vw = viewport()->width();
    const int vh = viewport()->height();
    const int pw = scaled_.width();
    const int ph = scaled_.height();

    // Centered when the scaled image is smaller than the viewport in that
    // dimension; otherwise offset by the scrollbar so panning works.
    const int x = pw <= vw ? (vw - pw) / 2 : -horizontalScrollBar()->value();
    const int y = ph <= vh ? (vh - ph) / 2 : -verticalScrollBar()->value();
    painter.drawPixmap(x, y, scaled_);
}

} // namespace ui_shell
