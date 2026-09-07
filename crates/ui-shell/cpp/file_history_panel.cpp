#include "file_history_panel.h"

#include "e2e_mark.h"
#include "history_list_view.h"

#include <QAction>
#include <QLabel>
#include <QMenu>
#include <QVBoxLayout>

namespace ui_shell {

FileHistoryPanel::FileHistoryPanel(
  VcsService *vcsService,
  std::function<void(const QString &, const QString &, const QString &, const QString &,
                      const QString &)>
    compareRevisions,
  std::function<void(const QString &)> openCommit,
  QWidget *parent)
  : QWidget(parent)
  , vcsService_(vcsService)
  , compareRevisions_(std::move(compareRevisions))
  , openCommit_(std::move(openCommit))
{
    titleLabel_ = new QLabel(this);
    titleLabel_->setWordWrap(true);
    list_ = new HistoryListView(vcsService_, this);
    connect(list_, &HistoryListView::commitActivated, this,
            [this](const QString &commitId) { openCommit_(commitId); });
    connect(list_, &HistoryListView::contextMenuRequestedFor, this,
            &FileHistoryPanel::showContextMenu);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(titleLabel_);
    layout->addWidget(list_, 1);

    connect(vcsService_, &VcsService::historyReady, this, &FileHistoryPanel::onHistoryReady);
    connect(vcsService_, &VcsService::historyUnavailable, this,
            &FileHistoryPanel::onHistoryUnavailable);
}

void FileHistoryPanel::setCurrentFile(const QString &path)
{
    currentPath_ = path;
    list_->setEntries(::rust::Vec<FfiLogEntry>());
    if (path.isEmpty()) {
        titleLabel_->setText(tr("No file selected"));
        return;
    }
    titleLabel_->setText(path);
    vcsService_->fileHistory(path);
}

void FileHistoryPanel::onHistoryReady(const QString &path, const ::rust::Vec<FfiLogEntry> &entries)
{
    if (path != currentPath_) {
        // A reply for a file that is no longer the active one — the user
        // switched tabs while it was in flight.
        return;
    }
    list_->setEntries(entries);
    // Each row's own rect, so an E2E flow can right-click a specific commit
    // without computing its position from row height/font metrics — same
    // reasoning as `markChangesRow` (changes_panel.cpp).
    for (int row = 0; row < list_->commitCount(); ++row) {
        const QRect rect = list_->globalRectForRow(row);
        e2eMark(QStringLiteral("{\"ev\":\"history_row\",\"path\":%1,\"commit\":%2,"
                                "\"row\":%3,\"rect\":[%4,%5,%6,%7]}")
                  .arg(e2eJson(path), e2eJson(list_->commitIdAt(row)))
                  .arg(row)
                  .arg(rect.x())
                  .arg(rect.y())
                  .arg(rect.width())
                  .arg(rect.height()));
    }
    // F3-18's own bridge fix carries the path so a race between two
    // requests is observable too, not just the final count.
    e2eMark(QStringLiteral("{\"ev\":\"history_ready\",\"path\":%1,\"count\":%2}")
              .arg(e2eJson(path))
              .arg(entries.size()));
}

void FileHistoryPanel::onHistoryUnavailable(const QString &path)
{
    if (path != currentPath_) {
        return;
    }
    list_->setEntries(::rust::Vec<FfiLogEntry>());
    titleLabel_->setText(tr("%1 — not a version-controlled file").arg(path));
    e2eMark(QStringLiteral("{\"ev\":\"history_unavailable\",\"path\":%1}").arg(e2eJson(path)));
}

void FileHistoryPanel::showContextMenu(const QPoint &globalPos, const QStringList &selectedIds)
{
    if (selectedIds.isEmpty() || currentPath_.isEmpty()) {
        return;
    }

    QString firstRevision = selectedIds.first();
    QString leftRevision;
    QString rightRevision;
    if (selectedIds.size() == 2) {
        // `HistoryListView::selectedCommitIds` returns rows in their
        // on-screen (newest-first) order, so the diff reads left-to-right
        // as old-to-new either way the two rows were picked.
        leftRevision = selectedIds.at(1);
        rightRevision = selectedIds.at(0);
    }
    const QString path = currentPath_;

    QMenu menu(this);
    QAction *compareWithWorkingTree = nullptr;
    QAction *compareSelected = nullptr;
    if (selectedIds.size() == 1) {
        compareWithWorkingTree = menu.addAction(tr("Compare with Working Tree"));
    } else if (selectedIds.size() == 2) {
        compareSelected = menu.addAction(tr("Compare Selected Revisions"));
    }
    if (menu.isEmpty()) {
        // More than two selected: nothing meaningful to compare.
        return;
    }

    // Same reasoning as `EditorTabs::showTabContextMenu`: a popup menu grabs
    // the keyboard rather than input focus, so this mark is the only way an
    // E2E flow driving `xdotool` from outside the process can know the menu
    // is up — and it has to fire before `exec()`, which does not return
    // until the menu is gone.
    e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"file_history_context_menu\"}");
    QAction *chosen = menu.exec(globalPos);
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"file_history_context_menu\","
                            "\"accepted\":%1}")
              .arg(chosen != nullptr ? QLatin1String("true") : QLatin1String("false")));
    if (!chosen) {
        return;
    }
    if (chosen == compareWithWorkingTree) {
        compareRevisions_(path, firstRevision, firstRevision.left(8), QString(),
                            tr("Working Tree"));
    } else if (chosen == compareSelected) {
        compareRevisions_(path, leftRevision, leftRevision.left(8), rightRevision,
                            rightRevision.left(8));
    }
}

} // namespace ui_shell
