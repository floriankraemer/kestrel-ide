// Image/network/volume context menus, dialogs, and the toolbar's Pull
// Image action (C4) — split out from `containers_panel.cpp`/
// `containers_actions.cpp` under the file-size ratchet, same reasoning
// `containers_detail.cpp` was split out for C3. Humble view: which actions
// apply to a node is still `ContainerService::nodeActions`'s answer; this
// file only builds menus/dialogs from those flags and dispatches clicks.

#include "containers_panel.h"

#include "containers_detail.h"

#include <QAction>
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QInputDialog>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kConnectionRole = Qt::UserRole + 2;
constexpr int kResourceIdRole = Qt::UserRole + 5;

// A key/value block in a `QPlainTextEdit`, one `key=value` per line — the
// same convention `FfiNetworkSpec::labels`/`FfiVolumeSpec::labels` and
// `FfiVolumeSpec::options` cross the seam with, so no per-row widget is
// needed for what is, in practice, always a short list.
QString labelsText(QPlainTextEdit *edit)
{
    return edit->toPlainText();
}

} // namespace

void ContainersPanel::triggerPullImage()
{
    const QString connectionId = selectedConnectionId();
    if (connectionId.isEmpty()) {
        statusLabel_->setText(tr("Select a connection first."));
        return;
    }
    bool ok = false;
    const QString reference = QInputDialog::getText(this, tr("Pull Image"), tr("Image reference"),
                                                     QLineEdit::Normal, QString(), &ok);
    if (!ok || reference.trimmed().isEmpty()) {
        return;
    }
    detail_->openPullTab(connectionId, reference.trimmed());
}

void ContainersPanel::showImageContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString id = item->data(0, kIdRole).toString();
    const FfiNodeActions actions = containerService_->nodeActions(id);

    QMenu menu(tree_);
    QAction *createContainer = menu.addAction(tr("Create Container..."));
    createContainer->setEnabled(actions.canCreateContainer);
    QAction *pull = menu.addAction(tr("Pull"));
    pull->setEnabled(actions.canPull);
    QAction *push = menu.addAction(tr("Push Image..."));
    push->setEnabled(false);
    push->setToolTip(tr("Push Image is not available until registries (C7) land."));
    QAction *copyTo = menu.addAction(tr("Copy Image to..."));
    copyTo->setEnabled(actions.canCopy);
    QAction *tag = menu.addAction(tr("Tag..."));
    tag->setEnabled(actions.canTag);
    menu.addSeparator();
    QAction *showLayers = menu.addAction(tr("Show Layers"));
    QAction *showLabels = menu.addAction(tr("Show Labels"));
    QAction *inspect = menu.addAction(tr("Inspect"));
    menu.addSeparator();
    QAction *copyImageId = menu.addAction(tr("Copy Image ID"));
    QAction *remove = menu.addAction(tr("Remove..."));
    remove->setEnabled(actions.canRemove);

    QAction *chosen = menu.exec(globalPos);
    if (chosen == createContainer) {
        openCreateContainerQuickDialog(id);
    } else if (chosen == pull) {
        const QString connectionId = item->data(0, kConnectionRole).toString();
        const QString reference = item->text(0);
        detail_->openPullTab(connectionId, reference);
    } else if (chosen == copyTo) {
        openCopyImageDialog(id);
    } else if (chosen == tag) {
        openTagDialog(id);
    } else if (chosen == showLayers) {
        detail_->showLayers();
    } else if (chosen == showLabels) {
        detail_->showLabels();
    } else if (chosen == inspect) {
        report(containerService_->openInspect(id));
    } else if (chosen == copyImageId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    } else if (chosen == remove) {
        QMessageBox confirm(this);
        confirm.setIcon(QMessageBox::Warning);
        confirm.setWindowTitle(tr("Remove Image"));
        confirm.setText(tr("Remove image \"%1\"?").arg(item->text(0)));
        auto *forceCheck = new QCheckBox(tr("Force"), &confirm);
        confirm.setCheckBox(forceCheck);
        confirm.setStandardButtons(QMessageBox::Cancel);
        QPushButton *removeButton = confirm.addButton(tr("Remove"), QMessageBox::AcceptRole);
        confirm.exec();
        if (confirm.clickedButton() == removeButton) {
            report(containerService_->removeImage(id, forceCheck->isChecked()));
        }
    }
}

void ContainersPanel::showNetworkContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString id = item->data(0, kIdRole).toString();
    const FfiNodeActions actions = containerService_->nodeActions(id);

    QMenu menu(tree_);
    QAction *inspect = menu.addAction(tr("Inspect"));
    QAction *showLabels = menu.addAction(tr("Show Labels"));
    QAction *copyId = menu.addAction(tr("Copy ID"));
    menu.addSeparator();
    QAction *remove = menu.addAction(tr("Remove..."));
    remove->setEnabled(actions.canRemove);

    QAction *chosen = menu.exec(globalPos);
    if (chosen == inspect) {
        report(containerService_->openInspect(id));
    } else if (chosen == showLabels) {
        detail_->showLabels();
    } else if (chosen == copyId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    } else if (chosen == remove) {
        if (QMessageBox::question(this, tr("Remove Network"),
                                  tr("Remove network \"%1\"?").arg(item->text(0)))
            == QMessageBox::Yes) {
            report(containerService_->removeNetwork(id));
        }
    }
}

void ContainersPanel::showVolumeContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString id = item->data(0, kIdRole).toString();
    const FfiNodeActions actions = containerService_->nodeActions(id);

    QMenu menu(tree_);
    QAction *inspect = menu.addAction(tr("Inspect"));
    QAction *showLabels = menu.addAction(tr("Show Labels"));
    QAction *copyId = menu.addAction(tr("Copy ID"));
    menu.addSeparator();
    QAction *remove = menu.addAction(tr("Remove..."));
    remove->setEnabled(actions.canRemove);

    QAction *chosen = menu.exec(globalPos);
    if (chosen == inspect) {
        report(containerService_->openInspect(id));
    } else if (chosen == showLabels) {
        detail_->showLabels();
    } else if (chosen == copyId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    } else if (chosen == remove) {
        QMessageBox confirm(this);
        confirm.setIcon(QMessageBox::Warning);
        confirm.setWindowTitle(tr("Remove Volume"));
        confirm.setText(tr("Remove volume \"%1\"?").arg(item->text(0)));
        auto *forceCheck = new QCheckBox(tr("Force"), &confirm);
        confirm.setCheckBox(forceCheck);
        confirm.setStandardButtons(QMessageBox::Cancel);
        QPushButton *removeButton = confirm.addButton(tr("Remove"), QMessageBox::AcceptRole);
        confirm.exec();
        if (confirm.clickedButton() == removeButton) {
            report(containerService_->removeVolume(id, forceCheck->isChecked()));
        }
    }
}

void ContainersPanel::showImagesGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString connectionId = item->data(0, kConnectionRole).toString();
    QMenu menu(tree_);
    QAction *pull = menu.addAction(tr("Pull Image..."));
    QAction *cleanDangling = menu.addAction(tr("Clean Up: Dangling Images"));
    QAction *cleanAllUnused = menu.addAction(tr("Clean Up: All Unused Images"));
    QAction *chosen = menu.exec(globalPos);
    if (chosen == pull) {
        triggerPullImage();
    } else if (chosen == cleanDangling) {
        report(containerService_->pruneImages(connectionId, false));
    } else if (chosen == cleanAllUnused) {
        if (QMessageBox::question(this, tr("Clean Up"), tr("Remove every unused image, not just dangling ones?"))
            == QMessageBox::Yes) {
            report(containerService_->pruneImages(connectionId, true));
        }
    }
}

void ContainersPanel::showNetworksGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString connectionId = item->data(0, kConnectionRole).toString();
    QMenu menu(tree_);
    QAction *create = menu.addAction(tr("Create Network..."));
    QAction *prune = menu.addAction(tr("Clean Up"));
    QAction *chosen = menu.exec(globalPos);
    if (chosen == create) {
        openCreateNetworkDialog(connectionId);
    } else if (chosen == prune) {
        if (QMessageBox::question(this, tr("Clean Up"), tr("Remove every unused network?"))
            == QMessageBox::Yes) {
            report(containerService_->pruneNetworks(connectionId));
        }
    }
}

void ContainersPanel::showVolumesGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString connectionId = item->data(0, kConnectionRole).toString();
    QMenu menu(tree_);
    QAction *create = menu.addAction(tr("Create Volume..."));
    QAction *prune = menu.addAction(tr("Clean Up"));
    QAction *chosen = menu.exec(globalPos);
    if (chosen == create) {
        openCreateVolumeDialog(connectionId);
    } else if (chosen == prune) {
        if (QMessageBox::question(this, tr("Clean Up"), tr("Remove every unused volume?"))
            == QMessageBox::Yes) {
            report(containerService_->pruneVolumes(connectionId));
        }
    }
}

void ContainersPanel::openCreateNetworkDialog(const QString &connectionId)
{
    if (connectionId.isEmpty()) {
        return;
    }
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Create Network"));
    auto *form = new QFormLayout(&dialog);
    auto *name = new QLineEdit(&dialog);
    auto *driver = new QComboBox(&dialog);
    driver->setEditable(true);
    driver->addItems({QString(), QStringLiteral("bridge"), QStringLiteral("host"),
                      QStringLiteral("overlay"), QStringLiteral("macvlan"), QStringLiteral("none")});
    auto *subnet = new QLineEdit(&dialog);
    auto *gateway = new QLineEdit(&dialog);
    auto *internal = new QCheckBox(&dialog);
    auto *attachable = new QCheckBox(&dialog);
    auto *labels = new QPlainTextEdit(&dialog);
    labels->setPlaceholderText(QStringLiteral("key=value"));
    form->addRow(tr("Name"), name);
    form->addRow(tr("Driver"), driver);
    form->addRow(tr("Subnet"), subnet);
    form->addRow(tr("Gateway"), gateway);
    form->addRow(tr("Internal"), internal);
    form->addRow(tr("Attachable"), attachable);
    form->addRow(tr("Labels"), labels);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    form->addRow(buttons);
    if (dialog.exec() != QDialog::Accepted || name->text().trimmed().isEmpty()) {
        return;
    }
    FfiNetworkSpec spec;
    spec.name = name->text().trimmed();
    spec.driver = driver->currentText().trimmed();
    spec.subnet = subnet->text().trimmed();
    spec.gateway = gateway->text().trimmed();
    spec.internal = internal->isChecked();
    spec.attachable = attachable->isChecked();
    spec.labels = labelsText(labels);
    report(containerService_->createNetwork(connectionId, spec));
}

void ContainersPanel::openCreateVolumeDialog(const QString &connectionId)
{
    if (connectionId.isEmpty()) {
        return;
    }
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Create Volume"));
    auto *form = new QFormLayout(&dialog);
    auto *name = new QLineEdit(&dialog);
    auto *driver = new QLineEdit(&dialog);
    auto *labels = new QPlainTextEdit(&dialog);
    labels->setPlaceholderText(QStringLiteral("key=value"));
    auto *options = new QPlainTextEdit(&dialog);
    options->setPlaceholderText(QStringLiteral("key=value"));
    form->addRow(tr("Name"), name);
    form->addRow(tr("Driver"), driver);
    form->addRow(tr("Labels"), labels);
    form->addRow(tr("Options"), options);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    form->addRow(buttons);
    if (dialog.exec() != QDialog::Accepted || name->text().trimmed().isEmpty()) {
        return;
    }
    FfiVolumeSpec spec;
    spec.name = name->text().trimmed();
    spec.driver = driver->text().trimmed();
    spec.labels = labelsText(labels);
    spec.options = labelsText(options);
    report(containerService_->createVolume(connectionId, spec));
}

void ContainersPanel::openTagDialog(const QString &nodeId)
{
    bool ok = false;
    const QString newReference =
      QInputDialog::getText(this, tr("Tag Image"), tr("New tag"), QLineEdit::Normal, QString(), &ok);
    if (!ok || newReference.trimmed().isEmpty()) {
        return;
    }
    report(containerService_->tagImage(nodeId, newReference.trimmed()));
}

void ContainersPanel::openCopyImageDialog(const QString &nodeId)
{
    const QString ownConnectionId = nodeId.section(QLatin1Char('/'), 0, 0);
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Copy Image To"));
    auto *form = new QFormLayout(&dialog);
    auto *target = new QComboBox(&dialog);
    for (const FfiConnectionSummary &connection : containerService_->connections()) {
        if (QString(connection.id) == ownConnectionId) {
            continue;
        }
        target->addItem(tr("%1 (%2)").arg(QString(connection.name), QString(connection.engine)),
                        QString(connection.id));
    }
    form->addRow(tr("Target connection"), target);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    form->addRow(buttons);
    if (target->count() == 0) {
        statusLabel_->setText(tr("No other connection is configured to copy this image to."));
        return;
    }
    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    report(containerService_->copyImageTo(nodeId, target->currentData().toString()));
}

void ContainersPanel::openCreateContainerQuickDialog(const QString &nodeId)
{
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Create Container"));
    auto *form = new QFormLayout(&dialog);
    auto *name = new QLineEdit(&dialog);
    auto *publishAll = new QCheckBox(tr("Publish all exposed ports"), &dialog);
    form->addRow(tr("Name"), name);
    form->addRow(QString(), publishAll);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    form->addRow(buttons);
    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    report(containerService_->createContainerQuick(nodeId, name->text().trimmed(),
                                                    publishAll->isChecked()));
}

} // namespace ui_shell
