#pragma once

#include <QString>
#include <QWidget>

#include <functional>

class QTabWidget;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class EditorTabs;

// The Diff dock: one closable tab per file currently diffed against HEAD
// (F3-14) — the same shape `CommitDetailPanel` uses for its per-commit
// tabs, chosen because ADS's `DockRegistry` has no support for a dynamic
// dock per diff and `restoreState()` needs a fixed, known-at-startup id set
// (see that panel's own doc comment). A diff used to open in its own
// top-level `Qt::Window`; that left it outside the dock layout entirely,
// unable to be docked, tabbed or split like every other panel.
//
// Content-agnostic on purpose: this widget knows nothing about `DiffView`/
// `DiffViewPage`, only that each tab is a `QWidget` identified by the
// `tabId` of the editor tab it stands in for. `EditorTabs` (editor_tabs_vcs.cpp)
// owns everything diff-specific.
class DiffPanel : public QWidget
{
public:
    // Called with the (tabId, page) of a tab the user closed via its own
    // close button, before `page` is deleted — the same "borrow it back out
    // before it goes away" contract the old floating window's `onClosing`
    // gave `EditorTabs::restoreEditorFromDiffWindow`.
    using DiffClosed = std::function<void(quint64 tabId, QWidget *page)>;

    DiffPanel(DiffClosed onClosed, QWidget *parent);

    // Opens a tab for `tabId` titled `title`, taking ownership of `page`,
    // and raises it. Caller's job to check `raiseIfOpen` first — this
    // never checks for an existing tab itself.
    void openDiff(quint64 tabId, const QString &title, QWidget *page);

    // Raises `tabId`'s tab if one is open. Returns whether it was.
    bool raiseIfOpen(quint64 tabId);

    // Removes and deletes `tabId`'s tab without running `DiffClosed` —
    // for when the underlying file tab itself is closing (editor_tabs.cpp's
    // `onTabClosed`), so there is nothing left to restore into.
    void discardDiff(quint64 tabId);

private:
    void closeTab(int index);

    DiffClosed onClosed_;
    QTabWidget *tabs_ = nullptr;
};

// Builds the panel, wraps it in a dock widget, registers it with `docks`
// under id `"diff"` (a fixed dock registered once at startup like
// `"commitDetail"` — not a dynamic dock per diff, for the reason above),
// and wires it into `editorTabs` (`EditorTabs::setDiffPanel`) — one call so
// wiring this dock into `main_window.cpp` costs one line, not the whole
// callback pair (`buildCommitDetailDock` and friends follow the same
// shape). Starts hidden; opening a diff reveals it.
void buildDiffDock(ads::CDockManager *dockManager, DockRegistry *docks,
                    ads::CDockAreaWidget *relativeTo, EditorTabs *editorTabs);

} // namespace ui_shell
