#include "dock_layout.h"

#include <QSplitter>
#include <QTimer>

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
    // G1: a caller (the status bar's Build Tools button, a View-menu action)
    // can name a dock whose contributing plugin is now disabled — `docks_`
    // never held it, or held it and lost it on the next reload. Silently
    // doing nothing beats `docks_[id]` default-constructing an `Entry` whose
    // `dock` is a null pointer and crashing on the very next line.
    const auto found = docks_.constFind(id);
    if (found == docks_.constEnd()) {
        return;
    }
    reseat(*found);
    found->dock->toggleView(true);
    found->dock->raise();
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

void DockRegistry::applyRowSplit(ads::CDockAreaWidget *rightArea)
{
    auto *splitter = qobject_cast<QSplitter *>(rightArea->parentWidget());
    if (splitter == nullptr) {
        return;
    }
    // `CDockManager::setSplitterSizes` silently ignores a list whose length
    // differs from the splitter's child count — and this row holds a third
    // dock area (the Build Tools dock) beside the editor and the right
    // column, so the two-entry list this used to pass never applied and the
    // column was sized from its panels' hints alone (#321). Weights: the
    // editor (first) 680, the right column 360. An area with no open dock
    // is skipped by the layout, but QSplitter hands it its stored size as
    // *pixels* the moment a dock opens there, taken from the editor — so a
    // closed area gets the ~260px such a column paints at, not a weight.
    QList<int> sizes;
    for (int i = 0; i < splitter->count(); ++i) {
        auto *area = qobject_cast<ads::CDockAreaWidget *>(splitter->widget(i));
        const bool closed = area != nullptr && area->openDockWidgetsCount() == 0;
        sizes << (i == 0 ? 680 : (closed ? 260 : 360));
    }
    dockManager_->setSplitterSizes(rightArea, sizes);
}

void DockRegistry::seedDefaultSplits(ads::CDockAreaWidget *bottomArea,
                                     ads::CDockAreaWidget *rightArea, const QString &base64State)
{
    if (!base64State.isEmpty()) {
        return;
    }
    // Weights, not pixels: QSplitter scales them to the real extent. At
    // 520:200 the Tests dock showed one tree row above its failure pane;
    // 70:30 is about IntelliJ's default. Re-splitting the column vertically
    // makes ADS re-flow the row above, so the row's own weights are
    // re-applied in the same turn.
    // Context is the manager, which owns every dock area the lambda touches.
    QTimer::singleShot(0, dockManager_, [this, bottomArea, rightArea]() {
        dockManager_->setSplitterSizes(bottomArea, {700, 300});
        applyRowSplit(rightArea);
    });
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
    const auto found = docks_.constFind(id);
    if (found != docks_.constEnd()) {
        found->dock->toggleView(false);
    }
}

bool DockRegistry::isClosed(const QString &id) const
{
    const auto found = docks_.constFind(id);
    // A dock nothing registered is, from every caller's point of view,
    // already closed.
    return found == docks_.constEnd() || found->dock->isClosed();
}

ads::CDockWidget *DockRegistry::dock(const QString &id) const
{
    const auto found = docks_.constFind(id);
    return found == docks_.constEnd() ? nullptr : found->dock;
}

} // namespace ui_shell
