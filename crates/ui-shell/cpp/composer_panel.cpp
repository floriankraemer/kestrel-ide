#include "composer_panel.h"

#include "dock_layout.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QInputDialog>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QStackedWidget>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

#include <functional>

namespace ui_shell {

namespace {

constexpr int kKindRole = Qt::UserRole;

QString groupTitle(const QString &kind)
{
    if (kind == QLatin1String("script")) {
        return QObject::tr("Scripts");
    }
    return kind == QLatin1String("dev-package") ? QObject::tr("Dev packages")
                                                : QObject::tr("Packages");
}

} // namespace

QWidget *buildComposerDock(ads::CDockManager *dockManager, DockRegistry *docks,
                           ads::CDockAreaWidget *relativeTo, ComposerService *composerService,
                           RunService *runService, ProjectTreeModel *treeModel)
{
    auto *panel = new QWidget(dockManager);
    auto *layout = new QVBoxLayout(panel);
    layout->setContentsMargins(0, 0, 0, 0);

    // Launch `action` (with `argument`) in the Run console; a refusal from
    // the model (a malformed package name) is shown as it is worded there.
    const auto run = [=](const QString &action, const QString &argument) {
        const FfiComposerAction answer = composerService->actionConfig(action, argument);
        if (answer.result.code != 0) {
            QMessageBox::warning(panel, QObject::tr("Composer"), QString(answer.result.message));
            return;
        }
        const FfiResult result = runService->runTemporary(answer.config);
        if (result.code != 0) {
            QMessageBox::warning(panel, QObject::tr("Run"), QString(result.message));
        }
    };

    auto *toolbar = new QHBoxLayout();
    toolbar->setContentsMargins(4, 4, 4, 0);
    const auto addButton = [&](const QString &text, std::function<void()> onClick) {
        auto *button = new QToolButton(panel);
        button->setText(text);
        button->setAutoRaise(true);
        QObject::connect(button, &QToolButton::clicked, panel, std::move(onClick));
        toolbar->addWidget(button);
    };
    auto *tree = new QTreeWidget(panel);
    const auto refresh = [=]() {
        tree->clear();
        QTreeWidgetItem *group = nullptr;
        QString groupKind;
        for (const FfiComposerRow &row : composerService->rows()) {
            const QString kind = QString(row.kind);
            if (group == nullptr || kind != groupKind) {
                group = new QTreeWidgetItem(tree, {groupTitle(kind)});
                groupKind = kind;
            }
            auto *item = new QTreeWidgetItem(group, {QString(row.name), QString(row.detail)});
            item->setData(0, kKindRole, kind);
            if (kind != QLatin1String("script") && !row.installed) {
                item->setText(1, QObject::tr("%1 (not installed)").arg(QString(row.detail)));
            }
        }
        tree->expandAll();
    };
    addButton(QObject::tr("Install"), [=]() { run(QStringLiteral("install"), QString()); });
    addButton(QObject::tr("Update"), [=]() { run(QStringLiteral("update"), QString()); });
    addButton(QObject::tr("Require…"), [=]() {
        bool ok = false;
        const QString package = QInputDialog::getText(
          panel, QObject::tr("Require Package"),
          QObject::tr("Package (vendor/name, optionally :constraint):"), QLineEdit::Normal,
          QString(), &ok);
        if (ok && !package.trimmed().isEmpty()) {
            run(QStringLiteral("require"), package);
        }
    });
    addButton(QObject::tr("Dump Autoload"), [=]() { run(QStringLiteral("dump-autoload"), QString()); });
    addButton(QObject::tr("Outdated"), [=]() { run(QStringLiteral("outdated"), QString()); });
    addButton(QObject::tr("Refresh"), refresh);
    toolbar->addStretch(1);
    layout->addLayout(toolbar);

    tree->setColumnCount(2);
    tree->setHeaderLabels({QObject::tr("Name"), QObject::tr("Version")});
    tree->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    tree->setContextMenuPolicy(Qt::CustomContextMenu);

    auto *empty = new QLabel(QObject::tr("No composer.json in this project."), panel);
    empty->setAlignment(Qt::AlignCenter);
    empty->setEnabled(false);
    auto *stack = new QStackedWidget(panel);
    stack->addWidget(tree);
    stack->addWidget(empty);
    layout->addWidget(stack, 1);

    const auto reload = [=]() {
        refresh();
        stack->setCurrentWidget(composerService->hasComposerJson() ? static_cast<QWidget *>(tree)
                                                                    : static_cast<QWidget *>(empty));
    };

    // A script row runs on double-click; a package row has no default.
    QObject::connect(tree, &QTreeWidget::itemDoubleClicked, panel,
                     [=](QTreeWidgetItem *item, int) {
                         if (item->data(0, kKindRole).toString() == QLatin1String("script")) {
                             run(QStringLiteral("run-script"), item->text(0));
                         }
                     });
    QObject::connect(tree, &QTreeWidget::customContextMenuRequested, panel, [=](const QPoint &pos) {
        QTreeWidgetItem *item = tree->itemAt(pos);
        if (item == nullptr || item->parent() == nullptr) {
            return;
        }
        const QString kind = item->data(0, kKindRole).toString();
        const QString name = item->text(0);
        QMenu menu(panel);
        if (kind == QLatin1String("script")) {
            menu.addAction(QObject::tr("Run"), panel,
                           [=]() { run(QStringLiteral("run-script"), name); });
        } else {
            menu.addAction(QObject::tr("Update"), panel,
                           [=]() { run(QStringLiteral("update"), name); });
            menu.addAction(QObject::tr("Remove"), panel,
                           [=]() { run(QStringLiteral("remove"), name); });
        }
        menu.exec(tree->viewport()->mapToGlobal(pos));
    });

    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Composer"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("composer"), dock, ads::RightDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("composer"));

    // The files change outside the IDE (and by the very actions this dock
    // runs), so re-read whenever the dock is shown or a project opens.
    QObject::connect(dock, &ads::CDockWidget::visibilityChanged, panel,
                     [reload](bool visible) {
                         if (visible) {
                             reload();
                         }
                     });
    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, panel,
                     [reload](const QString &) { reload(); });
    reload();
    return panel;
}

} // namespace ui_shell
