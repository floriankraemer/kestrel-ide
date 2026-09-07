#include "commit_detail_panel.h"

#include "commit_detail_view.h"
#include "dock_layout.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QTabWidget>
#include <QVBoxLayout>

namespace ui_shell {

CommitDetailPanel::CommitDetailPanel(VcsService *vcsService, QWidget *parent)
  : QWidget(parent), vcsService_(vcsService)
{
    tabs_ = new QTabWidget(this);
    tabs_->setTabsClosable(true);
    tabs_->setDocumentMode(true);
    connect(tabs_, &QTabWidget::tabCloseRequested, this, &CommitDetailPanel::closeTab);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(tabs_);
}

void CommitDetailPanel::openCommit(const QString &commitId)
{
    if (CommitDetailView *existing = viewsByCommit_.value(commitId)) {
        tabs_->setCurrentWidget(existing);
        return;
    }
    auto *view = new CommitDetailView(vcsService_, commitId, tabs_);
    viewsByCommit_.insert(commitId, view);
    const int index = tabs_->addTab(view, commitId.left(8));
    tabs_->setCurrentIndex(index);
}

void CommitDetailPanel::closeTab(int index)
{
    auto *view = qobject_cast<CommitDetailView *>(tabs_->widget(index));
    tabs_->removeTab(index);
    if (view) {
        viewsByCommit_.remove(view->commitId());
        view->deleteLater();
    }
}

CommitDetailPanel *buildCommitDetailDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                          ads::CDockAreaWidget *relativeTo,
                                          VcsService *vcsService)
{
    auto *panel = new CommitDetailPanel(vcsService, dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Commit Detail"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("commitDetail"), dock, ads::CenterDockWidgetArea,
                        relativeTo);
    docks->hide(QStringLiteral("commitDetail"));
    return panel;
}

} // namespace ui_shell
