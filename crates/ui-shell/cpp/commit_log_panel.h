#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>
#include <functional>

class QCheckBox;
class QDateEdit;
class QLineEdit;
class QPushButton;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class HistoryListView;

// The repo-wide Commit Log dock: every commit reachable from `HEAD`, newest
// first, via `VcsService::commitLog`/`commitLogFiltered`/`commitLogReady` —
// the same `HistoryListView` the File History panel uses, with a filter bar
// (R7: text, author, path, date range) above it and a "Load more" button
// growing the page instead of a per-file title.
//
// Humble view: what the log is, how a filter narrows it, and how many
// commits a given `max` returns are all `vcs-core`'s; this only re-asks
// with the current filter/page and re-renders what comes back.
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
    // R7: the row context menu — Checkout Revision, New Branch Here,
    // Cherry-pick, Revert Commit, Reset Current Branch Here (soft/mixed/
    // hard, with confirmation), Copy Hash.
    void showRowContextMenu(const QPoint &globalPos, const QStringList &selectedIds);

    VcsService *vcsService_;
    std::function<void(const QString &)> openCommit_;
    HistoryListView *list_ = nullptr;
    QPushButton *loadMoreButton_ = nullptr;
    quint32 currentMax_ = 0;

    // Filter bar (R7).
    QLineEdit *authorFilter_ = nullptr;
    QLineEdit *pathFilter_ = nullptr;
    QLineEdit *textFilter_ = nullptr;
    QCheckBox *sinceEnabled_ = nullptr;
    QDateEdit *sinceDate_ = nullptr;
    QCheckBox *untilEnabled_ = nullptr;
    QDateEdit *untilDate_ = nullptr;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"commitLog"`, the same one-call pattern `buildRunConsoleDock`
// uses.
CommitLogPanel *buildCommitLogDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                    ads::CDockAreaWidget *relativeTo, VcsService *vcsService,
                                    std::function<void(const QString &)> openCommit);

} // namespace ui_shell
