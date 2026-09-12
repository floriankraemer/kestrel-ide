#include "dock_tab_factory.h"

#include "DockWidgetTab.h"
#include "ui_tokens.h"

#include <QLayout>
#include <QLayoutItem>

namespace ui_shell {

ads::CDockWidgetTab *DockTabFactory::createDockWidgetTab(ads::CDockWidget *dockWidget) const
{
    ads::CDockWidgetTab *tab = CDockComponentsFactory::createDockWidgetTab(dockWidget);
    QLayout *layout = tab->layout();
    if (layout == nullptr) {
        return tab;
    }
    layout->setContentsMargins(0, 0, 0, 0);
    for (int i = 0; i < layout->count(); ++i) {
        if (QSpacerItem *spacer = layout->itemAt(i)->spacerItem()) {
            spacer->changeSize(tokens::kSp1, 0, QSizePolicy::Fixed, QSizePolicy::Minimum);
        }
    }
    return tab;
}

} // namespace ui_shell
