// Podman extras (C9): a pod node's own lifecycle context menu, and a
// Podman machine connection's Start machine/Stop machine entries — split
// out from `containers_panel.cpp` under the same file-size ratchet as
// `containers_actions.cpp`/`containers_resources.cpp`/`containers_compose.cpp`/
// `containers_registry.cpp`. Humble view: which actions apply is
// `ContainerService::nodeActions`'s/`isPodmanMachineConnection`'s answer;
// this file only enables/disables entries and dispatches clicks.

#include "containers_panel.h"

#include <QAction>
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QMenu>
#include <QMessageBox>
#include <QPushButton>
#include <QTreeWidget>

namespace ui_shell {

namespace {

// Mirrors `containers_panel.cpp`'s anonymous-namespace item-data roles —
// see `containers_actions.cpp`'s own copy of this comment.
constexpr int kIdRole = Qt::UserRole;
constexpr int kResourceIdRole = Qt::UserRole + 5;

} // namespace

void ContainersPanel::showPodContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString id = item->data(0, kIdRole).toString();
    const FfiNodeActions actions = containerService_->nodeActions(id);

    QMenu menu(tree_);
    QAction *start = menu.addAction(tr("Start"));
    start->setEnabled(actions.canStart);
    QAction *stop = menu.addAction(tr("Stop"));
    stop->setEnabled(actions.canStop);
    QAction *restart = menu.addAction(tr("Restart"));
    restart->setEnabled(actions.canRestart);
    QAction *remove = menu.addAction(tr("Remove..."));
    remove->setEnabled(actions.canRemove);
    menu.addSeparator();
    QAction *inspect = menu.addAction(tr("Inspect"));
    menu.addSeparator();
    QAction *copyId = menu.addAction(tr("Copy Pod ID"));
    copyId->setEnabled(!item->data(0, kResourceIdRole).toString().isEmpty());

    QAction *chosen = menu.exec(globalPos);
    if (chosen == start) {
        report(containerService_->startPod(id));
    } else if (chosen == stop) {
        report(containerService_->stopPod(id));
    } else if (chosen == restart) {
        report(containerService_->restartPod(id));
    } else if (chosen == remove) {
        const QString name = item->text(0);
        QMessageBox confirm(this);
        confirm.setIcon(QMessageBox::Warning);
        confirm.setWindowTitle(tr("Remove Pod"));
        confirm.setText(tr("Remove pod \"%1\" and every container in it?").arg(name));
        auto *forceCheck = new QCheckBox(tr("Force (kill if running)"), &confirm);
        confirm.setCheckBox(forceCheck);
        confirm.setStandardButtons(QMessageBox::Cancel);
        QPushButton *removeButton = confirm.addButton(tr("Remove"), QMessageBox::AcceptRole);
        confirm.exec();
        if (confirm.clickedButton() == removeButton) {
            report(containerService_->removePod(id, forceCheck->isChecked()));
        }
    } else if (chosen == inspect) {
        report(containerService_->openInspect(id));
    } else if (chosen == copyId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    }
}

void ContainersPanel::addMachineActions(QMenu &menu, const QString &connectionId,
                                        QAction *&startAction, QAction *&stopAction)
{
    if (!containerService_->isPodmanMachineConnection(connectionId)) {
        return;
    }
    menu.addSeparator();
    startAction = menu.addAction(tr("Start machine"));
    stopAction = menu.addAction(tr("Stop machine"));
}

} // namespace ui_shell
