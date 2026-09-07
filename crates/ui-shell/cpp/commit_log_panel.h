#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>
#include <functional>

class QPushButton;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class HistoryListView;

// The repo-wide Commit Log dock: every commit reachable from `HEAD`, newest
// first, via `VcsService::commitLog`/`commitLogReady` — the same
// `HistoryListView` the File History panel uses, with a "Load more" button
// growing the page instead of a per-file title.
//
// Humble view: what the log is, and how many commits `commitLog(max)`
// returns for a given `max`, are `vcs-core`'s; this only re-asks with a
// bigger `max` and re-renders what comes back.
class CommitLogPanel : public QWidget
{
public:
    // `openCommit` opens the commit-detail dock for one commit id —
    // double-click on a row, or Enter.
    CommitLogPanel(VcsService *vcsService, std::function<void(const QString &)> openCommit,
                   QWidget *parent);

protected:
    // Re-asks every time this dock is raised, not just at construction (when
    // it is still hidden, tabbed behind Terminal/Run/etc. — its rows' rects
    // at that point describe geometry nobody sees, which an E2E flow acting
    // on the marks from `onCommitLogReady` would click into blind). Cheap:
    // `HistoryCache::log` answers from cache unless `HEAD` moved.
    void showEvent(QShowEvent *event) override;

private:
    void onCommitLogReady(const ::rust::Vec<FfiLogEntry> &entries);
    void refresh();
    void loadMore();

    VcsService *vcsService_;
    std::function<void(const QString &)> openCommit_;
    HistoryListView *list_ = nullptr;
    QPushButton *loadMoreButton_ = nullptr;
    quint32 currentMax_ = 0;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"commitLog"`, the same one-call pattern `buildRunConsoleDock`
// uses.
CommitLogPanel *buildCommitLogDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                    ads::CDockAreaWidget *relativeTo, VcsService *vcsService,
                                    std::function<void(const QString &)> openCommit);

} // namespace ui_shell
