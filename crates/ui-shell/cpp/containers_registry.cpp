// Registry tree context menus, the Add ▾ menu's "Registry…" entry (wired
// in containers_panel.cpp's constructor, next to the other Add ▾ items),
// and the Push Image dialog (C7) — split out from containers_panel.cpp/
// containers_resources.cpp under the file-size ratchet, same reasoning
// containers_compose.cpp was split out for C5. Humble view: which
// repositories/tags a registry has is `ContainerService::nodes()`'s
// answer once loaded; a destination reference's exact text is
// `container_core::registry_ref::format_reference`'s (through
// `pushImageCommand`) — this file only builds menus/dialogs and dispatches
// clicks.

#include "containers_panel.h"

#include "containers_detail.h"

#include <QApplication>
#include <QClipboard>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QTreeWidgetItem>

namespace ui_shell {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kDetailRole = Qt::UserRole + 4;

} // namespace

void ContainersPanel::showRegistryContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString id = item->data(0, kIdRole).toString();
    const QString name = item->text(0);
    const FfiNodeActions actions = containerService_->nodeActions(id);

    QMenu menu(tree_);
    QAction *refresh = menu.addAction(tr("Refresh"));
    QAction *edit = menu.addAction(tr("Edit..."));
    menu.addSeparator();
    QAction *remove = menu.addAction(tr("Remove..."));
    remove->setEnabled(actions.canRemove);
    QAction *chosen = menu.exec(globalPos);
    if (chosen == refresh) {
        item->takeChildren();
        containerService_->loadRegistryRepositories(id);
    } else if (chosen == edit && openSettings_) {
        openSettings_(tr("Registries"));
    } else if (chosen == remove) {
        if (QMessageBox::question(this, tr("Remove Registry"),
                                  tr("Remove registry \"%1\"? Its stored password/token is "
                                     "deleted too.")
                                    .arg(name))
            == QMessageBox::Yes) {
            report(appSettings_->removeRegistry(id));
            containerService_->refreshAll();
        }
    }
}

void ContainersPanel::showRegistryRepoContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString id = item->data(0, kIdRole).toString();
    QMenu menu(tree_);
    QAction *refresh = menu.addAction(tr("Refresh"));
    QAction *copyRepo = menu.addAction(tr("Copy repository name"));
    QAction *chosen = menu.exec(globalPos);
    if (chosen == refresh) {
        item->takeChildren();
        containerService_->loadRegistryTags(id);
    } else if (chosen == copyRepo) {
        QApplication::clipboard()->setText(item->text(0));
    }
}

void ContainersPanel::showRegistryTagContextMenu(QTreeWidgetItem *item, const QPoint &globalPos)
{
    const QString tagNodeId = item->data(0, kIdRole).toString();
    const QString reference = item->data(0, kDetailRole).toString();

    QMenu menu(tree_);
    QAction *pull = menu.addAction(tr("Pull Image..."));
    QAction *copyReference = menu.addAction(tr("Copy reference"));
    QAction *chosen = menu.exec(globalPos);
    if (chosen == copyReference) {
        QApplication::clipboard()->setText(reference);
        return;
    }
    if (chosen != pull) {
        return;
    }

    const auto connections = containerService_->connections();
    QString connectionId;
    if (connections.size() == 1) {
        connectionId = QString(connections[0].id);
    } else if (connections.size() > 1) {
        QDialog dialog(this);
        dialog.setWindowTitle(tr("Pull Image"));
        auto *form = new QFormLayout(&dialog);
        auto *target = new QComboBox(&dialog);
        for (const FfiConnectionSummary &connection : connections) {
            target->addItem(tr("%1 (%2)").arg(QString(connection.name), QString(connection.engine)),
                            QString(connection.id));
        }
        form->addRow(tr("Pull into"), target);
        auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
        connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
        connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
        form->addRow(buttons);
        if (dialog.exec() != QDialog::Accepted) {
            return;
        }
        connectionId = target->currentData().toString();
    } else {
        statusLabel_->setText(tr("No connection is configured to pull into."));
        return;
    }

    const FfiCommand command = containerService_->pullFromRegistryCommand(tagNodeId, connectionId);
    if (QString(command.program).isEmpty()) {
        statusLabel_->setText(tr("Could not build a pull command for this tag."));
        return;
    }
    detail_->openTerminalCommandTab(command, tr("Pull: %1").arg(reference));
}

void ContainersPanel::openPushImageDialog(const QString &imageNodeId)
{
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Push Image"));
    auto *form = new QFormLayout(&dialog);

    auto *registryCombo = new QComboBox(&dialog);
    // Registries have no live "connected" state of their own (C7: they are
    // configuration, not a watched engine) — every configured registry is
    // offered, same as the Copy Image To... dialog offers every configured
    // connection regardless of whether it is currently connected.
    for (const FfiContainerNode &node : containerService_->nodes()) {
        if (QString(node.kind) == QStringLiteral("registry")) {
            registryCombo->addItem(QString(node.name), QString(node.resourceId));
        }
    }
    if (registryCombo->count() == 0) {
        statusLabel_->setText(tr("No registry is configured yet — add one in Settings > Containers."));
        return;
    }
    form->addRow(tr("Registry"), registryCombo);

    auto *repository = new QLineEdit(&dialog);
    if (QTreeWidgetItem *imageItem = itemsById_.value(imageNodeId)) {
        // The image row's own text is `repo:tag` (or a bare `<none>` for
        // an untagged one) — prefill just the repository half.
        repository->setText(imageItem->text(0).section(QLatin1Char(':'), 0, 0));
    }
    form->addRow(tr("Repository"), repository);

    auto *tag = new QLineEdit(&dialog);
    tag->setText(QStringLiteral("latest"));
    form->addRow(tr("Tag"), tag);

    auto *preview = new QLabel(&dialog);
    preview->setWordWrap(true);
    preview->setTextInteractionFlags(Qt::TextSelectableByMouse);
    form->addRow(tr("Reference"), preview);

    auto updatePreview = [preview, registryCombo, repository, tag]() {
        preview->setText(tr("%1/%2:%3")
                            .arg(registryCombo->currentText(), repository->text(),
                                 tag->text().isEmpty() ? tr("<tag>") : tag->text()));
    };
    connect(registryCombo, &QComboBox::currentIndexChanged, &dialog, updatePreview);
    connect(repository, &QLineEdit::textChanged, &dialog, updatePreview);
    connect(tag, &QLineEdit::textChanged, &dialog, updatePreview);
    updatePreview();

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    form->addRow(buttons);
    if (dialog.exec() != QDialog::Accepted || repository->text().trimmed().isEmpty()) {
        return;
    }

    const FfiCommand command = containerService_->pushImageCommand(
      imageNodeId, registryCombo->currentData().toString(), repository->text().trimmed(), tag->text().trimmed());
    if (QString(command.program).isEmpty()) {
        statusLabel_->setText(tr("Could not build a push command for this image."));
        return;
    }
    detail_->openTerminalCommandTab(command, tr("Push: %1/%2").arg(registryCombo->currentText(), repository->text()));
}

} // namespace ui_shell
