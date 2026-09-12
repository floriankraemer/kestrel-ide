// The glyphs this app draws in code — alpha masks tinted to the active
// theme (no Qt6Svg in this build) and the proxy style that puts the close
// glyph on every plain QTabBar. Declared in theme.h; split out of
// theme.cpp to keep that file's stylesheet builders and theme plumbing
// under the size ceiling.
#include "theme.h"

#include <QFile>
#include <QImage>
#include <QPixmap>
#include <QProxyStyle>

namespace ui_shell {

QIcon maskIcon(const char *maskResource, QColor tint)
{
    constexpr int kSide = 32;
    QFile file(QString::fromLatin1(maskResource));
    file.open(QIODevice::ReadOnly);
    const QByteArray mask = file.readAll();
    Q_ASSERT(mask.size() == kSide * kSide);

    QImage image(kSide, kSide, QImage::Format_ARGB32_Premultiplied);
    image.fill(Qt::transparent);
    for (int y = 0; y < kSide; ++y) {
        for (int x = 0; x < kSide; ++x) {
            QColor pixel = tint;
            pixel.setAlpha(static_cast<uchar>(mask[y * kSide + x]));
            image.setPixelColor(x, y, pixel);
        }
    }
    return QIcon(QPixmap::fromImage(image));
}

namespace {

// Swaps Qt's platform close glyph for tabCloseIcon() below, so every plain
// QTabWidget's close button — editor tab groups, terminal session tabs —
// matches ADS's own dock/tab close buttons, themed the same way (see
// applyTheme()'s ads::CIconProvider registration for that half of the swap),
// and widens that button so the glyph is not flush against the tab's edge.
// Everything else falls through to the base style unchanged.
class TabCloseStyle : public QProxyStyle
{
public:
    using QProxyStyle::QProxyStyle;

    QIcon standardIcon(StandardPixmap standardIcon, const QStyleOption *option,
                        const QWidget *widget) const override
    {
        if (standardIcon == QStyle::SP_TabCloseButton) {
            return tabCloseIcon();
        }
        return QProxyStyle::standardIcon(standardIcon, option, widget);
    }

    // Breathing room around the [x], and the same amount of it on every
    // platform. QCommonStyle centres the close glyph — 8px of ink at this
    // icon size — in a button PM_TabCloseIndicatorWidth wide, and the base
    // styles disagree wildly about that width (20 under Fusion, 12 under the
    // Windows style), so the slack around the glyph was 6px on one platform
    // and 2px on the other. Nothing in the stylesheet can correct it:
    // QStyleSheetStyle answers SE_TabBarTabRightButton itself and ignores
    // both `QTabBar::close-button`'s margins and a subElementRect override
    // (see the QTabBar rules in chromeStyleSheet() above). This metric is
    // still delegated, so pinning it to 16 gives the glyph 4px of its own on
    // each side everywhere. The button's own offset from the tab's edge is
    // what the base styles still disagree about — flush under Windows, 4px
    // in under Fusion — and PaneTabBar pins that itself, so the glyph ends
    // up 8px clear of both the label and the border on either platform.
    int pixelMetric(PixelMetric metric, const QStyleOption *option,
                     const QWidget *widget) const override
    {
        if (metric == QStyle::PM_TabCloseIndicatorWidth
            || metric == QStyle::PM_TabCloseIndicatorHeight) {
            return 16;
        }
        return QProxyStyle::pixelMetric(metric, option, widget);
    }
};

} // namespace

QIcon tabCloseIcon()
{
    return maskIcon(":/ui/icons/close_mask_32.a8", chromePaletteForTheme(activeThemeName()).textDim);
}

QStyle *makeTabCloseStyle(QStyle *base)
{
    return new TabCloseStyle(base);
}

QIcon searchIcon()
{
    return maskIcon(":/ui/icons/search_mask_32.a8",
                     chromePaletteForTheme(activeThemeName()).textDim);
}

} // namespace ui_shell
