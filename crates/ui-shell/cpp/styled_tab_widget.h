#pragma once

#include <QSize>
#include <QTabBar>
#include <QTabWidget>

class QEvent;
class QObject;
class QWidget;

namespace ui_shell {

// A QTabBar whose close button ([x]) is pinned to the tab's own right edge.
//
// Qt's stylesheet style positions QTabBar::close-button itself and ignores
// both stylesheet padding and QProxyStyle overrides of it, so the only seam
// that reaches the button's placement is this event filter. See the
// tabSizeHint/tabInserted/eventFilter/placeCloseButton members below for the
// exact traps this works around.
class StyledTabBar : public QTabBar
{
public:
    explicit StyledTabBar(QWidget *parent);

    QSize tabSizeHint(int index) const override;

protected:
    void tabInserted(int index) override;
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    void placeCloseButton(int index, QWidget *button) const;
};

// A QTabWidget installing a StyledTabBar. QTabWidget::setTabBar is
// protected, so installing one takes a subclass — this is the whole reason
// this class exists.
class StyledTabWidget : public QTabWidget
{
public:
    explicit StyledTabWidget(QWidget *parent);
};

} // namespace ui_shell
