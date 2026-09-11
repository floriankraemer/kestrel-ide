#pragma once

#include <QAbstractScrollArea>
#include <QImage>
#include <QPixmap>

class QPaintEvent;
class QResizeEvent;
class QWheelEvent;
class QKeyEvent;

namespace ui_shell {

// Read-only view of one already-decoded image: 1:1 when it
// fits the viewport, scaled down to fit otherwise, with Ctrl+wheel/keyboard
// zoom on top of that base scale and panning via the inherited scrollbars —
// the same `QAbstractScrollArea` shape `HexViewer` uses, so panning a large
// image costs nothing extra to implement.
//
// Decoding is the caller's job (ADR-0002): raster formats come from
// `QImageReader` reading `tabPath()` directly, SVG from the FFI's
// `renderSvgImage` (icon-theme's `resvg` pipeline) — see
// `EditorTabs::addImageTab`. This widget only ever paints a `QImage` it is
// handed.
class ImageViewer : public QAbstractScrollArea
{
    Q_OBJECT

public:
    explicit ImageViewer(QWidget *parent = nullptr);

    // A null `image` (decode failed) paints a short message instead of a
    // blank viewport, so a broken file reads as broken rather than empty.
    void setImage(QImage image);

protected:
    void paintEvent(QPaintEvent *event) override;
    void resizeEvent(QResizeEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;
    void keyPressEvent(QKeyEvent *event) override;

private:
    // Scale that fits the image inside the viewport, never upscaling past
    // 1:1 — the "1:1 if it fits, else scaled" rule.
    qreal baseScale() const;
    qreal effectiveScale() const { return baseScale() * zoom_; }
    void setZoom(qreal zoom);
    void rebuildScaledPixmap();
    void updateScrollRange();

    QImage image_;
    // Cached at `effectiveScale()`, rebuilt only on resize/zoom/image
    // change rather than on every paint (`Minimap`'s own `QPixmap`-caching
    // reasoning).
    QPixmap scaled_;
    qreal zoom_ = 1.0;
};

} // namespace ui_shell
