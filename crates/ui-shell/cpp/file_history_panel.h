#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>
#include <functional>

class QLabel;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class HistoryListView;

// The File History dock (F3-18): `fileHistory(path)`'s commits for whatever
// file is named by `setCurrentFile`, newest first.
//
// Humble view: what commits touched a file, and in what order, is
// `vcs-core`'s (`Repository::file_history`/`HistoryCache`); this only lists
// `historyReady`'s answer, via the shared `HistoryListView`. `historyReady`
// carries the path it answers for (F3-18's own bridge fix — `fileHistory`
// has no request id otherwise), so a reply that arrives after the active
// file changed again is dropped here rather than shown under the wrong
// title.
class FileHistoryPanel : public QWidget
{
public:
    // F3-14's entry point into `EditorTabs`, reached by callback rather than
    // a dependency on editor_tabs.h (the same shape
    // `ProjectTreeActions::compareFiles` uses): path, left revision + label,
    // right revision + label. An empty revision string means "the live
    // working text", which `EditorTabs::openCompareRevisions` already treats
    // specially.
    using CompareRevisions = std::function<void(const QString &, const QString &, const QString &,
                                                 const QString &, const QString &)>;
    // Opens the commit-detail dock for one commit id — double-click on a
    // row, or Enter.
    using OpenCommit = std::function<void(const QString &)>;

    FileHistoryPanel(VcsService *vcsService, CompareRevisions compareRevisions,
                      OpenCommit openCommit, QWidget *parent);

    // Which file to show history for — asks `VcsService::fileHistory`
    // immediately; empty clears the list (no file, or an unsaved buffer).
    void setCurrentFile(const QString &path);

private:
    void onHistoryReady(const QString &path, const ::rust::Vec<FfiLogEntry> &entries);
    void onHistoryUnavailable(const QString &path);
    void showContextMenu(const QPoint &globalPos, const QStringList &selectedIds);

    VcsService *vcsService_;
    CompareRevisions compareRevisions_;
    OpenCommit openCommit_;
    QString currentPath_;
    QLabel *titleLabel_ = nullptr;
    HistoryListView *list_ = nullptr;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"fileHistory"`, the same one-call pattern `buildRunConsoleDock`
// uses.
FileHistoryPanel *buildFileHistoryDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                        ads::CDockAreaWidget *relativeTo, VcsService *vcsService,
                                        FileHistoryPanel::CompareRevisions compareRevisions,
                                        FileHistoryPanel::OpenCommit openCommit);

} // namespace ui_shell
