// The Compose tree's own actions (C5, ADR-0056): Start All/Stop/Down on a
// project node, Scale.../Jump to Source on a service node — split out of
// containers_panel.cpp under the file-size ratchet, same reasoning
// containers_resources.cpp/containers_actions.cpp were split out for C3/C4.
//
// Humble view: every action here reads only what the tree already carries
// on the item (connection id, project/service name, the project's compose
// files — the same `tooltip` `containers_panel.cpp`'s own `onTreeChanged`
// already sets from `FfiContainerNode::tooltip`) and hands it to
// `RunService`, which does the actual argv compilation and launching
// (`run_core::container_run::compose_project_*`). No business decision
// lives here.

#include "containers_panel.h"

#include "editor_tabs.h"

#include <QAction>
#include <QApplication>
#include <QClipboard>
#include <QDir>
#include <QFileInfo>
#include <QInputDialog>
#include <QMenu>
#include <QMessageBox>
#include <QTreeWidget>

namespace ui_shell {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kConnectionRole = Qt::UserRole + 2;
constexpr int kResourceIdRole = Qt::UserRole + 5;

// The project item's own compose files — `\n`-separated, from the tooltip
// `onTreeChanged` set from `FfiContainerNode::tooltip`
// (`container_core::tree`'s own `project.config_files.join("\n")`).
QString composeFilesOf(QTreeWidgetItem *projectItem)
{
    return projectItem->toolTip(0);
}

} // namespace

void ContainersPanel::showComposeProjectContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString nodeId = item->data(0, kIdRole).toString();

    QMenu menu(tree_);
    QAction *startAll = menu.addAction(tr("Start All"));
    QAction *stop = menu.addAction(tr("Stop"));
    QAction *down = menu.addAction(tr("Down"));
    menu.addSeparator();
    QAction *jumpToSource = menu.addAction(tr("Jump to Source"));
    jumpToSource->setEnabled(!composeFilesOf(item).isEmpty());
    menu.addSeparator();
    QAction *copyId = menu.addAction(tr("Copy ID"));

    QAction *chosen = menu.exec(globalPos);
    if (chosen == startAll) {
        triggerComposeStartAll(nodeId);
    } else if (chosen == stop) {
        triggerComposeStop(nodeId);
    } else if (chosen == down) {
        triggerComposeDown(nodeId);
    } else if (chosen == jumpToSource) {
        triggerComposeJumpToSource(nodeId);
    } else if (chosen == copyId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    }
}

void ContainersPanel::showComposeServiceContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString nodeId = item->data(0, kIdRole).toString();
    QTreeWidgetItem *projectItem = item->parent();

    QMenu menu(tree_);
    QAction *scale = menu.addAction(tr("Scale..."));
    QAction *jumpToSource = menu.addAction(tr("Jump to Source"));
    jumpToSource->setEnabled(projectItem != nullptr && !composeFilesOf(projectItem).isEmpty());
    menu.addSeparator();
    QAction *copyId = menu.addAction(tr("Copy ID"));

    QAction *chosen = menu.exec(globalPos);
    if (chosen == scale) {
        triggerComposeScale(nodeId);
    } else if (chosen == jumpToSource) {
        triggerComposeJumpToSource(projectItem->data(0, kIdRole).toString());
    } else if (chosen == copyId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    }
}

void ContainersPanel::triggerComposeStartAll(const QString &nodeId)
{
    QTreeWidgetItem *item = itemsById_.value(nodeId);
    if (item == nullptr || runService_ == nullptr) {
        return;
    }
    report(runService_->runComposeProject(item->data(0, kConnectionRole).toString(),
                                          composeFilesOf(item),
                                          item->data(0, kResourceIdRole).toString()));
}

void ContainersPanel::triggerComposeStop(const QString &nodeId)
{
    QTreeWidgetItem *item = itemsById_.value(nodeId);
    if (item == nullptr || runService_ == nullptr) {
        return;
    }
    report(runService_->stopComposeProject(item->data(0, kConnectionRole).toString(),
                                           composeFilesOf(item),
                                           item->data(0, kResourceIdRole).toString()));
}

void ContainersPanel::triggerComposeDown(const QString &nodeId)
{
    QTreeWidgetItem *item = itemsById_.value(nodeId);
    if (item == nullptr || runService_ == nullptr) {
        return;
    }
    report(runService_->downComposeProject(item->data(0, kConnectionRole).toString(),
                                           composeFilesOf(item),
                                           item->data(0, kResourceIdRole).toString()));
}

void ContainersPanel::triggerComposeScale(const QString &serviceNodeId)
{
    QTreeWidgetItem *item = itemsById_.value(serviceNodeId);
    if (item == nullptr || runService_ == nullptr) {
        return;
    }
    QTreeWidgetItem *projectItem = item->parent();
    if (projectItem == nullptr) {
        return;
    }
    bool ok = false;
    const int count = QInputDialog::getInt(this, tr("Scale"),
                                           tr("Number of containers for \"%1\":")
                                             .arg(item->data(0, kResourceIdRole).toString()),
                                           1, 0, 100, 1, &ok);
    if (!ok) {
        return;
    }
    report(runService_->scaleComposeService(item->data(0, kConnectionRole).toString(),
                                            composeFilesOf(projectItem),
                                            item->data(0, kResourceIdRole).toString(),
                                            static_cast<quint32>(count)));
}

void ContainersPanel::triggerComposeJumpToSource(const QString &nodeId)
{
    QTreeWidgetItem *item = itemsById_.value(nodeId);
    if (item == nullptr || editorTabs_ == nullptr) {
        return;
    }
    const QStringList files =
      composeFilesOf(item).split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    if (files.isEmpty()) {
        return;
    }
    // The project's own `working_dir` (`kDetailRole`, set from
    // `ComposeProject::working_dir`) makes a relative compose file path
    // absolute — opening a bare relative path would resolve against the
    // IDE process's own working directory instead.
    const QString workingDir = item->data(0, Qt::UserRole + 4).toString();
    QString first = files.first();
    if (QFileInfo(first).isRelative() && !workingDir.isEmpty()) {
        first = QDir(workingDir).filePath(first);
    }
    editorTabs_->openFile(first);
}

} // namespace ui_shell
