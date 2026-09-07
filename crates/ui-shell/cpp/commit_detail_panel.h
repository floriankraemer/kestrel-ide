#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <QWidget>

class QTabWidget;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class CommitDetailView;

// The commit-detail dock: a `QTabWidget`, one closable tab per commit
// currently open, `CommitDetailPanel::openCommit` focusing an existing tab
// rather than duplicating it — the same shape `RunConsolePanel` uses for
// its per-run tabs.
class CommitDetailPanel : public QWidget
{
public:
    CommitDetailPanel(VcsService *vcsService, QWidget *parent);

    // Opens a tab for `commitId`, creating one if none is open yet, and
    // raises it. The entry point `HistoryListView::commitActivated`
    // (via `FileHistoryPanel`/`CommitLogPanel`) is wired to.
    void openCommit(const QString &commitId);

private:
    void closeTab(int index);

    VcsService *vcsService_;
    QTabWidget *tabs_ = nullptr;
    QHash<QString, CommitDetailView *> viewsByCommit_;
};

// Builds the panel, wraps it in a dock widget and registers it with
// `docks` under id `"commitDetail"`, a fixed dock registered once at
// startup like `"runConsole"`/`"build"` — not a dynamic dock per commit,
// since ADS's `DockRegistry` has no support for one and `restoreState()`
// needs a fixed, known-at-startup id set.
CommitDetailPanel *buildCommitDetailDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                          ads::CDockAreaWidget *relativeTo,
                                          VcsService *vcsService);

} // namespace ui_shell
