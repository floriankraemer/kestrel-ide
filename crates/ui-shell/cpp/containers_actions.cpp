// Container lifecycle toolbar buttons and the container/containers-group
// context menus (C3) — split out from `containers_panel.cpp` under the
// file-size ratchet, the same way `containers_detail.cpp` holds the
// per-container tab area. Humble view: which actions apply to the
// selected node is `ContainerService::nodeActions`'s answer; this file
// only enables/disables buttons and dispatches clicks.

#include "containers_panel.h"

#include "containers_detail.h"
#include "theme.h"

#include <QAction>
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QMenu>
#include <QMessageBox>
#include <QPushButton>
#include <QToolButton>
#include <QTreeWidget>

namespace ui_shell {

namespace {

// Mirrors `containers_panel.cpp`'s anonymous-namespace item-data roles —
// duplicated rather than shared through the header, since both files are
// the only readers and a shared constants header would outweigh six ints.
constexpr int kIdRole = Qt::UserRole;
constexpr int kKindRole = Qt::UserRole + 1;
constexpr int kDetailRole = Qt::UserRole + 4;
constexpr int kResourceIdRole = Qt::UserRole + 5;

QToolButton *lifecycleButton(const char *mask, const QString &toolTip, QWidget *parent)
{
    auto *button = new QToolButton(parent);
    button->setIcon(maskIcon(mask, chromePaletteForTheme(activeThemeName()).textDim));
    button->setIconSize(QSize(16, 16));
    button->setAutoRaise(true);
    button->setFocusPolicy(Qt::NoFocus);
    button->setToolTip(toolTip);
    button->setEnabled(false);
    return button;
}

} // namespace

void ContainersPanel::buildLifecycleToolbar()
{
    startButton_ = lifecycleButton(":/ui/icons/containers/connect.a8", tr("Start"), this);
    connect(startButton_, &QToolButton::clicked, this, &ContainersPanel::triggerStart);

    stopButton_ = lifecycleButton(":/ui/icons/containers/disconnect.a8", tr("Stop"), this);
    connect(stopButton_, &QToolButton::clicked, this, &ContainersPanel::triggerStop);

    restartButton_ = lifecycleButton(":/ui/icons/diff/sync.a8", tr("Restart"), this);
    connect(restartButton_, &QToolButton::clicked, this, &ContainersPanel::triggerRestart);

    pauseButton_ = lifecycleButton(":/ui/icons/containers/cleanup.a8", tr("Pause"), this);
    connect(pauseButton_, &QToolButton::clicked, this, &ContainersPanel::triggerPauseOrUnpause);

    removeButton_ = lifecycleButton(":/ui/icons/containers/disconnect.a8", tr("Remove..."), this);
    connect(removeButton_, &QToolButton::clicked, this, &ContainersPanel::triggerRemove);
}

void ContainersPanel::updateLifecycleButtons()
{
    QTreeWidgetItem *item = selectedItem();
    const bool isContainer =
      item != nullptr && item->data(0, kKindRole).toString() == QStringLiteral("container");
    if (!isContainer) {
        startButton_->setEnabled(false);
        stopButton_->setEnabled(false);
        restartButton_->setEnabled(false);
        pauseButton_->setEnabled(false);
        pauseButton_->setText(tr("Pause"));
        removeButton_->setEnabled(false);
        return;
    }
    const FfiNodeActions actions = containerService_->nodeActions(item->data(0, kIdRole).toString());
    startButton_->setEnabled(actions.canStart);
    stopButton_->setEnabled(actions.canStop);
    restartButton_->setEnabled(actions.canRestart);
    pauseButton_->setEnabled(actions.canPause || actions.canUnpause);
    pauseButton_->setText(actions.canUnpause ? tr("Unpause") : tr("Pause"));
    removeButton_->setEnabled(actions.canRemove);
}

void ContainersPanel::triggerStart()
{
    QTreeWidgetItem *item = selectedItem();
    if (item != nullptr) {
        report(containerService_->startContainer(item->data(0, kIdRole).toString()));
    }
}

void ContainersPanel::triggerStop()
{
    QTreeWidgetItem *item = selectedItem();
    if (item != nullptr) {
        report(containerService_->stopContainer(item->data(0, kIdRole).toString()));
    }
}

void ContainersPanel::triggerRestart()
{
    QTreeWidgetItem *item = selectedItem();
    if (item != nullptr) {
        report(containerService_->restartContainer(item->data(0, kIdRole).toString()));
    }
}

void ContainersPanel::triggerPauseOrUnpause()
{
    QTreeWidgetItem *item = selectedItem();
    if (item == nullptr) {
        return;
    }
    const QString id = item->data(0, kIdRole).toString();
    if (containerService_->nodeActions(id).canUnpause) {
        report(containerService_->unpauseContainer(id));
    } else {
        report(containerService_->pauseContainer(id));
    }
}

void ContainersPanel::triggerRemove()
{
    QTreeWidgetItem *item = selectedItem();
    if (item == nullptr) {
        return;
    }
    const QString id = item->data(0, kIdRole).toString();
    const QString name = item->text(0);

    QMessageBox confirm(this);
    confirm.setIcon(QMessageBox::Warning);
    confirm.setWindowTitle(tr("Remove Container"));
    confirm.setText(tr("Remove container \"%1\"?").arg(name));
    auto *forceCheck = new QCheckBox(tr("Force (kill if running)"), &confirm);
    confirm.setCheckBox(forceCheck);
    confirm.setStandardButtons(QMessageBox::Cancel);
    QPushButton *removeButton = confirm.addButton(tr("Remove"), QMessageBox::AcceptRole);
    confirm.exec();
    if (confirm.clickedButton() != removeButton) {
        return;
    }
    report(containerService_->removeContainer(id, forceCheck->isChecked()));
}

void ContainersPanel::triggerCleanUp()
{
    const QString connectionId = selectedConnectionId();
    if (connectionId.isEmpty()) {
        return;
    }
    const QMessageBox::StandardButton chosen = QMessageBox::question(
      this, tr("Clean Up"),
      tr("Remove every stopped container on this connection? This cannot be undone."),
      QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel);
    if (chosen != QMessageBox::Yes) {
        return;
    }
    report(containerService_->pruneContainers(connectionId));
}

void ContainersPanel::showContainerContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
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
    QAction *pauseOrUnpause = menu.addAction(actions.canUnpause ? tr("Unpause") : tr("Pause"));
    pauseOrUnpause->setEnabled(actions.canPause || actions.canUnpause);
    QAction *remove = menu.addAction(tr("Remove..."));
    remove->setEnabled(actions.canRemove);
    menu.addSeparator();
    QMenu *terminalMenu = menu.addMenu(tr("Terminal"));
    QAction *terminalUser = terminalMenu->addAction(tr("As current user"));
    QAction *terminalRoot = terminalMenu->addAction(tr("As root"));
    QAction *exec = menu.addAction(tr("Exec..."));
    QAction *attach = menu.addAction(tr("Attach"));
    QAction *log = menu.addAction(tr("Log"));
    menu.addSeparator();
    QAction *inspect = menu.addAction(tr("Inspect"));
    QAction *processes = menu.addAction(tr("Show Processes"));
    QAction *files = menu.addAction(tr("Show Files"));
    menu.addSeparator();
    QAction *copyContainerId = menu.addAction(tr("Copy Container ID"));
    QAction *copyImageId = menu.addAction(tr("Copy Image ID"));

    QAction *chosen = menu.exec(globalPos);
    if (chosen == start) {
        triggerStart();
    } else if (chosen == stop) {
        triggerStop();
    } else if (chosen == restart) {
        triggerRestart();
    } else if (chosen == pauseOrUnpause) {
        triggerPauseOrUnpause();
    } else if (chosen == remove) {
        triggerRemove();
    } else if (chosen == terminalUser) {
        detail_->openTerminal(false);
    } else if (chosen == terminalRoot) {
        detail_->openTerminal(true);
    } else if (chosen == exec) {
        detail_->openExecDialog(this);
    } else if (chosen == attach) {
        detail_->openAttach();
    } else if (chosen == log) {
        // The Log tab already auto-opened on selection; this just brings
        // it to the front for someone who closed or lost track of it.
        detailTabs_->setCurrentIndex(1);
    } else if (chosen == inspect) {
        report(containerService_->openInspect(id));
    } else if (chosen == processes) {
        detail_->showProcesses();
    } else if (chosen == files) {
        detail_->showFiles();
    } else if (chosen == copyContainerId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    } else if (chosen == copyImageId) {
        // The Details column carries the image reference for a container
        // row (`detailColumn`); the engine-side image id itself is only
        // in `inspect`, so this copies the reference, which is what a
        // user pastes into `docker pull`/`docker run` anyway.
        QApplication::clipboard()->setText(item->data(0, kDetailRole).toString());
    }
}

void ContainersPanel::showContainersGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    Q_UNUSED(item);
    QMenu menu(tree_);
    QAction *cleanUp = menu.addAction(tr("Clean Up..."));
    if (menu.exec(globalPos) == cleanUp) {
        triggerCleanUp();
    }
}

} // namespace ui_shell
