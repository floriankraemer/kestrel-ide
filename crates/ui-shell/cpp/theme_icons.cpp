// The glyphs this app draws in code — alpha masks tinted to the active
// theme (no Qt6Svg in this build) and the proxy style that puts the close
// glyph on every plain QTabBar and paints every tree view's branch column.
// Declared in theme.h; split out of
// theme.cpp to keep that file's stylesheet builders and theme plumbing
// under the size ceiling.
#include "theme.h"

#include <QFile>
#include <QImage>
#include <QPainter>
#include <QPixmap>
#include <QProxyStyle>
#include <QStyleOption>

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

// Paints the branch column of one tree row: the guide lines that join a
// parent to its children and the chevron on rows that have children.
//
// Qt hands the column one cell per indentation level, left to right, and
// says through the state flags what that cell is: State_Item — the cell
// right before the row's own label (with State_Children when the row has
// children, State_Open when they are shown); any cell left of it stands
// for one ancestor. State_Sibling means the cell's item — the row itself,
// or that ancestor — has another sibling further down.
//
// Qt's own styles run a row's connecting line through its State_Item cell,
// one indentation step right of the parent's chevron. The guide is drawn
// one cell to the left of that instead, so it hangs straight down from the
// parent's chevron the way IDE trees usually draw it. Root rows have no
// parent to hang from — their column would lie left of the viewport — so
// they get no lines at all, only the chevron.
void paintTreeBranch(const QStyleOption &option, QPainter &painter)
{
    const ChromePalette palette = chromePaletteForTheme(activeThemeName());
    const QRect rect = option.rect;
    const QStyle::State state = option.state;
    const int centreX = rect.left() + rect.width() / 2;
    const int centreY = rect.top() + rect.height() / 2;
    const int parentX = centreX - rect.width();
    const bool hasParentColumn = parentX >= 0;
    // Half the chevron's height; also how far the guide lines keep clear
    // of it.
    const int half = qMax(2, rect.height() / 5);
    const bool hasChildren = state & QStyle::State_Children;

    // Lines are dim text at a fraction of its alpha rather than the border
    // colour: the border is meant to vanish against a surface, a guide is
    // meant to be followed by eye.
    QColor lineColor = palette.textDim;
    lineColor.setAlpha(96);

    painter.save();
    painter.setRenderHint(QPainter::Antialiasing, false);
    painter.setPen(QPen(lineColor, 1));
    if (hasChildren && (state & QStyle::State_Open)) {
        painter.drawLine(centreX, centreY + half + 2, centreX, rect.bottom());
    }
    if (hasParentColumn && (state & QStyle::State_Item)) {
        // Elbow from the parent's column into this row: down from the top
        // (on to the bottom when a sibling follows), then right to the
        // chevron or the label.
        const int bottom = (state & QStyle::State_Sibling) ? rect.bottom() : centreY;
        const int right = hasChildren ? centreX - half - 2 : rect.right();
        painter.drawLine(parentX, rect.top(), parentX, bottom);
        painter.drawLine(parentX, centreY, right, centreY);
    } else if (hasParentColumn && (state & QStyle::State_Sibling)) {
        painter.drawLine(parentX, rect.top(), parentX, rect.bottom());
    }

    if (hasChildren) {
        // Chevron pointing right when closed and down when open, tinted like
        // every other secondary glyph.
        painter.setRenderHint(QPainter::Antialiasing, true);
        painter.setPen(QPen(palette.textDim, 1.5, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin));
        const QPointF centre(centreX, centreY);
        const QPolygonF chevron = (state & QStyle::State_Open)
            ? QPolygonF{centre + QPointF(-half, -half / 2.0), centre + QPointF(0, half / 2.0),
                        centre + QPointF(half, -half / 2.0)}
            : QPolygonF{centre + QPointF(-half / 2.0, -half), centre + QPointF(half / 2.0, 0),
                        centre + QPointF(-half / 2.0, half)};
        painter.drawPolyline(chevron);
    }
    painter.restore();
}

// The application-wide QProxyStyle. Everything not listed here falls
// through to the base style unchanged.
//
// Close buttons: swaps Qt's platform close glyph for tabCloseIcon() below,
// so every plain QTabWidget's close button — editor tab groups, terminal
// session tabs — matches ADS's own dock/tab close buttons, themed the same
// way (see applyTheme()'s ads::CIconProvider registration for that half of
// the swap), and widens that button so the glyph is not flush against the
// tab's edge.
//
// Tree branches: paints every QTreeView's branch column (paintTreeBranch
// above) so all trees — project, structure, search results, changes, ... —
// share one themed look on every platform instead of Fusion's arrows on
// Linux and the Windows style's dotted lines on Windows. This works because
// QStyleSheetStyle only takes PE_IndicatorBranch over when the stylesheet
// has a `QTreeView::branch` rule; keep chromeStyleSheet() free of one.
class ChromeStyle : public QProxyStyle
{
public:
    using QProxyStyle::QProxyStyle;

    void drawPrimitive(PrimitiveElement element, const QStyleOption *option, QPainter *painter,
                       const QWidget *widget) const override
    {
        if (element == QStyle::PE_IndicatorBranch) {
            paintTreeBranch(*option, *painter);
            return;
        }
        QProxyStyle::drawPrimitive(element, option, painter, widget);
    }

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
        // One indentation step per tree level — room for the chevron and
        // guide lines above without the 20px the base styles default to.
        if (metric == QStyle::PM_TreeViewIndentation) {
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

QStyle *makeChromeStyle(QStyle *base)
{
    return new ChromeStyle(base);
}

QIcon searchIcon()
{
    return maskIcon(":/ui/icons/search_mask_32.a8",
                     chromePaletteForTheme(activeThemeName()).textDim);
}

} // namespace ui_shell
