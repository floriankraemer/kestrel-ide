#pragma once

#include "DockComponentsFactory.h"

namespace ui_shell {

// ADS's stock CDockWidgetTab lays its contents out with font-derived air of
// its own — `2 * Spacing` of left margin, `Spacing` between label and [x],
// `Spacing * 4/3` after it, `Spacing = round(fontHeight / 4)` — on top of
// whatever the stylesheet pads the tab with, so a dock tab never lines up
// with an editor tab, whose air is the sheet's alone. This factory hands out
// tabs with those layout gaps pinned to the editor tab's geometry (see
// StyledTabBar and TabCloseStyle: 4px between label and a 16px [x], 4px
// between the [x] and the border), leaving the sheet's `padding` the one
// lever for the rest — the same lever Settings > Tabs drives for editor tabs.
class DockTabFactory : public ads::CDockComponentsFactory
{
public:
    ads::CDockWidgetTab *createDockWidgetTab(ads::CDockWidget *dockWidget) const override;
};

} // namespace ui_shell
