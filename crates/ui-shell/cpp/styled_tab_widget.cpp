#include "styled_tab_widget.h"

#include "ui_tokens.h"

#include <QEvent>
#include <QWidget>

namespace ui_shell {

StyledTabBar::StyledTabBar(QWidget *parent)
  : QTabBar(parent)
{
}

// Qt's stylesheet style pads every tab with an icon by a hardcoded
// 12px — `spaceForIcon = 6 + 4 + 2 /* magic */` in QStyleSheetStyle::
// sizeFromContents(CT_TabBarTab) — on top of the icon width and spacing
// QTabBar::tabSizeHint() has already counted. Nothing consumes it: the
// text rect comes out 12px wider than the text, so Qt centres the label
// in it and the slack lands as a gap on either side of the label, the
// one before the [x] most visibly. Taking it back here is the only
// seam that reaches it — the size hint is the widget's, while
// CT_TabBarTab itself is answered by the stylesheet style and ignores a
// QProxyStyle override of it (see makeChromeStyle() in theme.cpp for
// the same trap one sub-element over).
QSize StyledTabBar::tabSizeHint(int index) const
{
    constexpr int kStyleSheetIconPadding = 12;
    QSize hint = QTabBar::tabSizeHint(index);
    if (!tabIcon(index).isNull()) {
        hint.setWidth(hint.width() - kStyleSheetIconPadding);
    }
    return hint;
}

// Where the [x] ends up is otherwise platform-dependent. Qt reserves the
// same room for it everywhere — QCommonStylePrivate::tabLayout ends the
// label rect 4px plus the close button's width before the tab's content
// edge — but the button itself is positioned by QStyleSheetStyle from
// the tab's *full* rect, ignoring the sheet's padding-right that the
// label rect does honour. Under the Windows style that leaves the button
// flush against the tab's border with every spare pixel piled up on its
// left: 12px there against 4px on the right, which is the hole in the
// middle of the tab. Under Fusion the same code lands it 4px in.
//
// Pinning the button to the tab's own right edge ourselves is the one
// lever that reads the same on both. It has to happen from an event
// filter rather than tabLayoutChange(): Qt re-lays the tab widgets out
// after that hook too (on Windows the final layout runs after the last
// tabLayoutChange() of the show), and a position set there is silently
// overwritten.
void StyledTabBar::tabInserted(int index)
{
    QTabBar::tabInserted(index);
    if (QWidget *button = tabButton(index, QTabBar::RightSide)) {
        button->installEventFilter(this);
        placeCloseButton(index, button);
    }
}

bool StyledTabBar::eventFilter(QObject *watched, QEvent *event)
{
    if (event->type() == QEvent::Move || event->type() == QEvent::Resize) {
        for (int i = 0; i < count(); ++i) {
            QWidget *button = tabButton(i, QTabBar::RightSide);
            if (button == watched) {
                placeCloseButton(i, button);
                break;
            }
        }
    }
    return QTabBar::eventFilter(watched, event);
}

// 4px of air between the [x] and the tab's border, matching the 4px the
// close indicator carries on its other side (PM_TabCloseIndicatorWidth
// is pinned to 16 around an 8px glyph — see makeChromeStyle() in
// theme.cpp), so the glyph sits 8px clear of both the label and the
// border.
void StyledTabBar::placeCloseButton(int index, QWidget *button) const
{
    constexpr int kTabBorderWidth = 1;
    const int x = tabRect(index).right() + 1 - kTabBorderWidth - tokens::kSp1
                  - button->width();
    if (button->x() != x) {
        button->move(x, button->y());
    }
}

StyledTabWidget::StyledTabWidget(QWidget *parent)
  : QTabWidget(parent)
{
    setTabBar(new StyledTabBar(this));
}

} // namespace ui_shell
