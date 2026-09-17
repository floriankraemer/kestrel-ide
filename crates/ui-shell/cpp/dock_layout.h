#pragma once

#include "ads_globals.h"

#include <QHash>
#include <QPointer>
#include <QString>

namespace ads {
class CDockAreaWidget;
class CDockManager;
class CDockWidget;
} // namespace ads

namespace ui_shell {

// Every dock widget hanging off the main window's one CDockManager, and the
// one place that knows how to show/hide one of them.
//
// Before this existed, each of the six side/bottom docks got its own
// `dockManager->addDockWidget(...)` at construction and its own scattered
// `dock->toggleView(true); dock->raise();` pair at every call site that
// wanted to reveal it (View menu actions, Find Usages' raise-and-focus, the
// Problems panel's first-diagnostic auto-show...). One of the six (AI Chat)
// additionally needed a one-off `showAiChatDock()` free function because a
// dock a restored layout never mentioned comes back from
// `CDockManager::restoreState()` unassigned — closed, un-parented, no dock
// area (see `CDockManager::restoreDockWidgetsOpenState`) — and showing it in
// that state takes ADS's floating path instead of returning it to its
// tab strip. That risk is not actually specific to AI Chat: it is whatever
// dock existed in code but not in a since-superseded saved layout, so
// `show()` below applies the same "re-add if homeless" recovery to every
// dock, not just the one that happened to need it first.
class DockRegistry
{
public:
    explicit DockRegistry(ads::CDockManager *dockManager);

    // Registers `dock` under `id` and adds it to the layout now, in `area`
    // relative to `relativeTo`'s dock area. Returns the dock area
    // `addDockWidget` created/extended, exactly as `CDockManager` does, so a
    // caller placing a second dock relative to this one can chain off it.
    ads::CDockAreaWidget *registerDock(const QString &id, ads::CDockWidget *dock,
                                       ads::DockWidgetArea area, ads::CDockAreaWidget *relativeTo);

    // Reveals dock `id`: puts it back at its registered placement first if a
    // restored layout left it homeless, then `toggleView(true)` + `raise()`.
    void show(const QString &id);

    // Applies a base64 `CDockManager::saveState()` blob, then puts every dock
    // the blob never mentioned back at its registered placement. An empty
    // blob does nothing, which is what "no layout has been saved" means.
    //
    // The two steps are one operation because they are never correct apart.
    // A restored state leaves any dock it predates un-parented, and `show()`
    // only rescues one at the moment something asks for it — enough at
    // startup, where nothing looks at a dock before then, but not when a
    // named layout is applied to a window the user is already looking at:
    // there, an un-rescued dock simply vanishes. Routing every restore
    // through here means no call site has to remember that.
    void restoreState(const QString &base64State);

    // The default splits once the window is up, unless `base64State` names
    // a saved layout, whose own splits then stand: the bottom area at 30% of
    // the column (about IntelliJ's default), the right column re-applied
    // alongside it. Deferred a turn because the same weights set before the
    // first layout change how ADS splits the editor from the right column
    // (#321).
    void seedDefaultSplits(ads::CDockAreaWidget *bottomArea, ads::CDockAreaWidget *rightArea,
                           const QString &base64State);

    // The editor/right-column row's default weights (editor ~65%, right
    // column ~35%), spelled out for every child of that row's splitter —
    // a two-entry list is silently ignored once a third, closed dock area
    // sits in the row.
    void applyRowSplit(ads::CDockAreaWidget *rightArea);

    // `toggleView(false)` on dock `id`.
    void hide(const QString &id);

    // `CDockWidget::isClosed()` for dock `id`.
    bool isClosed(const QString &id) const;

    // The raw dock widget registered under `id`, for the call sites that
    // need more than show/hide/isClosed (focusing a child widget after
    // raising it, for instance).
    ads::CDockWidget *dock(const QString &id) const;

private:
    struct Entry
    {
        ads::CDockWidget *dock;
        ads::DockWidgetArea area;
        // A dock widget that lived in `registerDock`'s `relativeTo` area,
        // never that area itself: `CDockManager::restoreState()` deletes
        // every dock area and builds new ones, so an area pointer taken at
        // construction is dangling after the first layout restore, and
        // `show()`'s re-add handed ADS exactly that (issue: File History
        // crashed on every launch once a layout without it had been saved).
        // Dock widgets survive a restore; the area to re-add next to is
        // looked up through this one at show time. `QPointer`, so a dock
        // ADS deletes reads as "no anchor" rather than as another stale
        // pointer.
        QPointer<ads::CDockWidget> anchor;
    };

    // Re-adds `entry`'s dock at its registered placement if it currently has
    // no dock area. Visibility is deliberately untouched: `show()` reveals
    // afterwards, `reseatHomeless()` does not.
    void reseat(const Entry &entry);

    ads::CDockManager *dockManager_;
    QHash<QString, Entry> docks_;
};

} // namespace ui_shell
