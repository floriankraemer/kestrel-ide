#include "branch_popup.h"

#include "e2e_mark.h"

#include <QAction>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFont>
#include <QGroupBox>
#include <QInputDialog>
#include <QLineEdit>
#include <QListWidget>
#include <QMenu>
#include <QMessageBox>
#include <QMetaObject>
#include <QPushButton>
#include <QVBoxLayout>

#include <memory>

namespace ui_shell {

namespace {

/// One `vcs-core` error code as the plain `int` `FfiResult::code` carries —
/// same convention `vcs_menu.cpp`'s own copy of this constant follows
/// (ADR-0003 §4).
constexpr int vcsErrorCode(FfiVcsErrorCode code)
{
    return static_cast<int>(code);
}

constexpr int kNameRole = Qt::UserRole;

} // namespace

QString pickRemote(QWidget *anchor, VcsService *vcsService)
{
    QStringList names;
    for (const FfiRemoteInfo &remote : vcsService->remotes()) {
        names.append(QString(remote.name));
    }
    if (names.isEmpty()) {
        QMessageBox::warning(anchor, QObject::tr("Push"),
                              QObject::tr("This repository has no remotes configured."));
        return QString();
    }
    if (names.size() == 1) {
        return names.first();
    }
    bool ok = false;
    const QString remote = QInputDialog::getItem(anchor, QObject::tr("Push"),
                                                  QObject::tr("Remote:"), names, 0, false, &ok);
    return ok ? remote : QString();
}

namespace {

/// `merge`/`rebase`/`cherryPick`/`revertCommit` all share the same outcome
/// shape: silence on success (the branch/status signals every other write
/// already fires are enough), and a conflict shown as what it is rather
/// than a bare error — the Changes dock's existing "Merge Conflicts" group
/// already reflects it via the same `statusChanged`. A full merge editor is
/// out of scope (ADR-0031 amendment); this only points the user at
/// "Resolve..." in the Changes dock.
void watchIntegrationResult(QWidget *anchor, VcsService *vcsService, const QString &verb)
{
    auto connection = std::make_shared<QMetaObject::Connection>();
    *connection = QObject::connect(
      vcsService, &VcsService::vcsFailed, vcsService,
      [anchor, verb, vcsService, connection](FfiResult error) {
          QObject::disconnect(*connection);
          if (error.code == vcsErrorCode(FfiVcsErrorCode::MergeConflict)) {
              QMessageBox::information(
                anchor, verb,
                QObject::tr("%1 left conflicts. Resolve them in the Changes dock's "
                            "\"Merge Conflicts\" group, then commit.")
                  .arg(verb));
              return;
          }
          QMessageBox::warning(anchor, verb, error.message);
      });
}

} // namespace

void showBranchMenu(VcsService *vcsService, QWidget *anchor, const QPoint & /*globalPos*/)
{
    auto *dialog = new QDialog(anchor ? anchor->window() : nullptr);
    dialog->setWindowTitle(QObject::tr("Branches"));
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->resize(420, 460);
    QObject::connect(dialog, &QDialog::finished, dialog,
                      []() { e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"branch_popup\"}"); });

    auto *layout = new QVBoxLayout(dialog);

    auto *search = new QLineEdit(dialog);
    search->setPlaceholderText(QObject::tr("Search branches..."));
    layout->addWidget(search);

    auto *localGroup = new QGroupBox(QObject::tr("Local"), dialog);
    auto *localLayout = new QVBoxLayout(localGroup);
    auto *localList = new QListWidget(localGroup);
    localList->setContextMenuPolicy(Qt::CustomContextMenu);
    localLayout->addWidget(localList);
    layout->addWidget(localGroup, 1);

    auto *remoteGroup = new QGroupBox(QObject::tr("Remote"), dialog);
    auto *remoteLayout = new QVBoxLayout(remoteGroup);
    auto *remoteList = new QListWidget(remoteGroup);
    remoteList->setContextMenuPolicy(Qt::CustomContextMenu);
    remoteLayout->addWidget(remoteList);
    layout->addWidget(remoteGroup, 1);

    const auto populate = [=]() {
        const QString filter = search->text();
        const QString current = vcsService->currentBranch();

        localList->clear();
        for (const FfiBranch &branch : vcsService->branches()) {
            const QString name = QString(branch.name);
            if (!filter.isEmpty() && !name.contains(filter, Qt::CaseInsensitive)) {
                continue;
            }
            const bool isCurrent = name == current;
            auto *item =
              new QListWidgetItem(isCurrent ? name + QObject::tr(" (current)") : name, localList);
            item->setData(kNameRole, name);
            if (isCurrent) {
                QFont font = item->font();
                font.setBold(true);
                item->setFont(font);
            }
        }

        remoteList->clear();
        for (const FfiBranch &branch : vcsService->remoteBranches()) {
            const QString name = QString(branch.name);
            if (!filter.isEmpty() && !name.contains(filter, Qt::CaseInsensitive)) {
                continue;
            }
            auto *item = new QListWidgetItem(name, remoteList);
            item->setData(kNameRole, name);
        }
    };
    populate();
    QObject::connect(search, &QLineEdit::textChanged, dialog, populate);
    QObject::connect(vcsService, &VcsService::branchChanged, dialog, populate);

    const auto compareWithCurrent = [=](const QString &name) {
        auto connection = std::make_shared<QMetaObject::Connection>();
        *connection = QObject::connect(
          vcsService, &VcsService::changedPathsReady, dialog,
          [dialog, name, connection](const QString &revision,
                                     const ::rust::Vec<FfiRepoPath> &paths) {
              if (revision != name) {
                  return;
              }
              QObject::disconnect(*connection);
              if (paths.empty()) {
                  QMessageBox::information(dialog, QObject::tr("Compare with Current"),
                                            QObject::tr("No files differ from %1.").arg(name));
                  return;
              }
              QStringList lines;
              for (const FfiRepoPath &path : paths) {
                  lines.append(QString(path.path));
              }
              QMessageBox::information(
                dialog, QObject::tr("Compare with Current"),
                QObject::tr("%1 file(s) differ from %2:\n\n%3")
                  .arg(paths.size())
                  .arg(name)
                  .arg(lines.join(QStringLiteral("\n"))));
          });
        vcsService->requestChangedPathsAgainst(name);
    };

    const auto showLocalContextMenu = [=](const QPoint &pos) {
        QListWidgetItem *item = localList->itemAt(pos);
        if (!item) {
            return;
        }
        const QString name = item->data(kNameRole).toString();
        const bool isCurrent = name == vcsService->currentBranch();

        auto *menu = new QMenu(localList);
        menu->setAttribute(Qt::WA_DeleteOnClose);

        QAction *checkoutAction = menu->addAction(QObject::tr("Checkout"));
        checkoutAction->setEnabled(!isCurrent);
        QAction *mergeAction = menu->addAction(QObject::tr("Merge into Current"));
        mergeAction->setEnabled(!isCurrent);
        QAction *rebaseAction = menu->addAction(QObject::tr("Rebase Current onto %1").arg(name));
        rebaseAction->setEnabled(!isCurrent);
        menu->addSeparator();
        QAction *renameAction = menu->addAction(QObject::tr("Rename..."));
        QAction *deleteAction = menu->addAction(QObject::tr("Delete..."));
        deleteAction->setEnabled(!isCurrent);
        menu->addSeparator();
        QAction *pushAction = menu->addAction(QObject::tr("Push"));
        QAction *compareAction = menu->addAction(QObject::tr("Compare with Current"));
        compareAction->setEnabled(!isCurrent);

        QObject::connect(checkoutAction, &QAction::triggered, vcsService,
                          [vcsService, name]() { vcsService->checkout(name); });
        QObject::connect(mergeAction, &QAction::triggered, vcsService, [=]() {
            watchIntegrationResult(dialog, vcsService, QObject::tr("Merge"));
            vcsService->merge(name);
        });
        QObject::connect(rebaseAction, &QAction::triggered, vcsService, [=]() {
            watchIntegrationResult(dialog, vcsService, QObject::tr("Rebase"));
            vcsService->rebase(name);
        });
        QObject::connect(renameAction, &QAction::triggered, dialog, [=]() {
            const QString newName = QInputDialog::getText(
              dialog, QObject::tr("Rename Branch"), QObject::tr("New name:"), QLineEdit::Normal,
              name);
            if (!newName.isEmpty() && newName != name) {
                vcsService->renameBranch(name, newName);
            }
        });
        QObject::connect(deleteAction, &QAction::triggered, vcsService, [=]() {
            auto connection = std::make_shared<QMetaObject::Connection>();
            *connection = QObject::connect(
              vcsService, &VcsService::vcsFailed, vcsService,
              [vcsService, dialog, name, connection](FfiResult error) {
                  QObject::disconnect(*connection);
                  if (error.code != vcsErrorCode(FfiVcsErrorCode::UnmergedBranch)) {
                      QMessageBox::warning(dialog, QObject::tr("Delete Branch"), error.message);
                      return;
                  }
                  const auto choice = QMessageBox::warning(
                    dialog, QObject::tr("Delete Branch"),
                    QObject::tr("'%1' has commits not merged anywhere else. Delete anyway?")
                      .arg(name),
                    QMessageBox::Cancel | QMessageBox::Yes, QMessageBox::Cancel);
                  if (choice == QMessageBox::Yes) {
                      vcsService->deleteBranch(name, /*force=*/true);
                  }
              });
            vcsService->deleteBranch(name, /*force=*/false);
        });
        QObject::connect(pushAction, &QAction::triggered, vcsService, [=]() {
            const QString remote = pickRemote(dialog, vcsService);
            if (!remote.isEmpty()) {
                vcsService->push(remote, name, /*setUpstream=*/true);
            }
        });
        QObject::connect(compareAction, &QAction::triggered, dialog,
                          [=]() { compareWithCurrent(name); });

        menu->popup(localList->viewport()->mapToGlobal(pos));
    };
    QObject::connect(localList, &QListWidget::customContextMenuRequested, dialog,
                      showLocalContextMenu);
    QObject::connect(localList, &QListWidget::itemDoubleClicked, vcsService,
                      [vcsService](QListWidgetItem *item) {
                          vcsService->checkout(item->data(kNameRole).toString());
                      });

    const auto showRemoteContextMenu = [=](const QPoint &pos) {
        QListWidgetItem *item = remoteList->itemAt(pos);
        if (!item) {
            return;
        }
        const QString name = item->data(kNameRole).toString();

        auto *menu = new QMenu(remoteList);
        menu->setAttribute(Qt::WA_DeleteOnClose);
        QAction *checkoutAction = menu->addAction(QObject::tr("Checkout"));
        QAction *compareAction = menu->addAction(QObject::tr("Compare with Current"));

        QObject::connect(checkoutAction, &QAction::triggered, vcsService,
                          [vcsService, name]() { vcsService->checkout(name); });
        QObject::connect(compareAction, &QAction::triggered, dialog,
                          [=]() { compareWithCurrent(name); });

        menu->popup(remoteList->viewport()->mapToGlobal(pos));
    };
    QObject::connect(remoteList, &QListWidget::customContextMenuRequested, dialog,
                      showRemoteContextMenu);

    auto *newBranchButton = new QPushButton(QObject::tr("New Branch..."), dialog);
    QObject::connect(newBranchButton, &QPushButton::clicked, dialog, [=]() {
        const QString name =
          QInputDialog::getText(dialog, QObject::tr("New Branch"), QObject::tr("Branch name:"));
        if (!name.isEmpty()) {
            vcsService->createBranch(name, QString());
        }
    });

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, dialog);
    buttons->addButton(newBranchButton, QDialogButtonBox::ActionRole);
    QObject::connect(buttons, &QDialogButtonBox::rejected, dialog, &QDialog::reject);
    layout->addWidget(buttons);

    e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"branch_popup\"}");
    dialog->show();
    dialog->raise();
    dialog->activateWindow();
}

} // namespace ui_shell
