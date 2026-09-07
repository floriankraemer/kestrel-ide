#include "dock_layout.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

namespace ui_shell {

DockRegistry::DockRegistry(ads::CDockManager *dockManager) : dockManager_(dockManager) {}

ads::CDockAreaWidget *DockRegistry::registerDock(const QString &id, ads::CDockWidget *dock,
                                                 ads::DockWidgetArea area,
                                                 ads::CDockAreaWidget *relativeTo)
{
    ads::CDockWidget *anchor =
      relativeTo && !relativeTo->dockWidgets().isEmpty() ? relativeTo->dockWidgets().first() : nullptr;
    docks_.insert(id, Entry{dock, area, anchor});
    return dockManager_->addDockWidget(area, dock, relativeTo);
}

void DockRegistry::show(const QString &id)
{
    const Entry &entry = docks_[id];
    reseat(entry);
    entry.dock->toggleView(true);
    entry.dock->raise();
}

void DockRegistry::restoreState(const QString &base64State)
{
    if (base64State.isEmpty()) {
        return;
    }
    dockManager_->restoreState(QByteArray::fromBase64(base64State.toLatin1()));
    for (const Entry &entry : std::as_const(docks_)) {
        reseat(entry);
    }
}

void DockRegistry::reseat(const Entry &entry)
{
    if (entry.dock->dockAreaWidget()) {
        return;
    }
    // The anchor's *current* area. An anchor the restored layout left
    // homeless too gives `nullptr`, which ADS takes as "a new area on
    // the root container" — a dock in a plain place beats no dock.
    ads::CDockAreaWidget *relativeTo = entry.anchor ? entry.anchor->dockAreaWidget() : nullptr;
    dockManager_->addDockWidget(entry.area, entry.dock, relativeTo);
}

void DockRegistry::hide(const QString &id)
{
    docks_[id].dock->toggleView(false);
}

bool DockRegistry::isClosed(const QString &id) const
{
    return docks_[id].dock->isClosed();
}

ads::CDockWidget *DockRegistry::dock(const QString &id) const
{
    return docks_[id].dock;
}

} // namespace ui_shell
