#include "commit_log_panel.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "history_list_view.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QPushButton>
#include <QShowEvent>
#include <QVBoxLayout>

namespace ui_shell {

namespace {
// A panel only ever shows one page of commits at a time; "Load more" grows
// by this much rather than the whole repository's history at once.
constexpr quint32 kPageSize = 200;
} // namespace

CommitLogPanel::CommitLogPanel(VcsService *vcsService, std::function<void(const QString &)>
                                                          openCommit,
                                QWidget *parent)
  : QWidget(parent)
  , vcsService_(vcsService)
  , openCommit_(std::move(openCommit))
  , currentMax_(kPageSize)
{
    list_ = new HistoryListView(vcsService_, this);
    connect(list_, &HistoryListView::commitActivated, this,
            [this](const QString &commitId) { openCommit_(commitId); });

    loadMoreButton_ = new QPushButton(tr("Load more"), this);
    connect(loadMoreButton_, &QPushButton::clicked, this, &CommitLogPanel::loadMore);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(list_, 1);
    layout->addWidget(loadMoreButton_);

    connect(vcsService_, &VcsService::commitLogReady, this, &CommitLogPanel::onCommitLogReady);
    // The log changes on every commit/checkout/pull — re-ask rather than
    // trying to patch the existing list in place.
    connect(vcsService_, &VcsService::statusChanged, this, &CommitLogPanel::refresh);
    connect(vcsService_, &VcsService::branchChanged, this, &CommitLogPanel::refresh);
    connect(vcsService_, &VcsService::repositoryChanged, this, &CommitLogPanel::refresh);

    refresh();
}

void CommitLogPanel::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    refresh();
}

void CommitLogPanel::refresh()
{
    vcsService_->commitLog(currentMax_);
}

void CommitLogPanel::loadMore()
{
    currentMax_ += kPageSize;
    refresh();
}

void CommitLogPanel::onCommitLogReady(const ::rust::Vec<FfiLogEntry> &entries)
{
    list_->setEntries(entries);
    // Nothing more to load once the answer came back shorter than what was
    // asked for — the repository's whole history is already shown.
    loadMoreButton_->setEnabled(static_cast<quint32>(entries.size()) >= currentMax_);
    for (int row = 0; row < list_->commitCount(); ++row) {
        const QRect rect = list_->globalRectForRow(row);
        e2eMark(QStringLiteral("{\"ev\":\"commit_log_row\",\"commit\":%1,"
                                "\"row\":%2,\"rect\":[%3,%4,%5,%6]}")
                  .arg(e2eJson(list_->commitIdAt(row)))
                  .arg(row)
                  .arg(rect.x())
                  .arg(rect.y())
                  .arg(rect.width())
                  .arg(rect.height()));
    }
}

CommitLogPanel *buildCommitLogDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                    ads::CDockAreaWidget *relativeTo, VcsService *vcsService,
                                    std::function<void(const QString &)> openCommit)
{
    auto *panel = new CommitLogPanel(vcsService, std::move(openCommit), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Commit Log"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("commitLog"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("commitLog"));
    return panel;
}

} // namespace ui_shell
