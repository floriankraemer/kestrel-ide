#include "build_tools_panel.h"

#include "dock_layout.h"
#include "e2e_mark.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QHash>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QSignalBlocker>
#include <QStyle>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kToolRole = Qt::UserRole + 1;
constexpr int kRunnableRole = Qt::UserRole + 2;
constexpr int kBuildFileRole = Qt::UserRole + 3;
constexpr int kIsProfileRole = Qt::UserRole + 4;

// A row icon per kind (review fix 3, pixel scrutiny): plain platform-style
// icons, the same "no vendored asset for a handful of kinds" call
// `tests_panel.cpp`'s own status dot makes, rather than the project tree's
// icon-theme pipeline — that pipeline resolves a *file's* icon from its
// path/language, which a Gradle task or a Maven scope has neither of.
QIcon iconForKind(FfiBuildToolNodeKind kind)
{
    QStyle *style = QApplication::style();
    switch (kind) {
    case FfiBuildToolNodeKind::ToolRoot:
        return style->standardIcon(QStyle::SP_DriveHDIcon);
    case FfiBuildToolNodeKind::Group:
        return style->standardIcon(QStyle::SP_DirIcon);
    case FfiBuildToolNodeKind::Task:
        return style->standardIcon(QStyle::SP_MediaPlay);
    case FfiBuildToolNodeKind::Module:
        return style->standardIcon(QStyle::SP_DirClosedIcon);
    case FfiBuildToolNodeKind::SourceRoot:
        return style->standardIcon(QStyle::SP_FileIcon);
    case FfiBuildToolNodeKind::Dependency:
        return style->standardIcon(QStyle::SP_FileDialogDetailedView);
    case FfiBuildToolNodeKind::Profile:
        return QIcon();
    }
    return QIcon();
}

QString titleFor(FfiBuildToolTitleKind kind)
{
    switch (kind) {
    case FfiBuildToolTitleKind::Gradle:
        return QObject::tr("Gradle");
    case FfiBuildToolTitleKind::Maven:
        return QObject::tr("Maven");
    case FfiBuildToolTitleKind::Both:
        return QObject::tr("Gradle & Maven");
    case FfiBuildToolTitleKind::None:
        break;
    }
    return QObject::tr("Build Tools");
}

} // namespace

BuildToolsPanel::BuildToolsPanel(BuildToolsService *buildToolsService, RunService *runService,
                                  OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , buildToolsService_(buildToolsService)
  , runService_(runService)
  , openAt_(std::move(openAt))
{
    auto *reloadButton = new QToolButton(this);
    reloadButton->setText(tr("Reload"));
    executeEdit_ = new QLineEdit(this);
    executeEdit_->setPlaceholderText(tr("Execute…"));
    // Keeps the placeholder readable regardless of how many toggle buttons
    // this toolbar ends up with (Maven's own Skip Tests makes one more than
    // Gradle's) — the same `find_bar.cpp` rule: the field gets a floor, the
    // buttons give up space first.
    executeEdit_->setMinimumWidth(90);
    auto *runButton = new QToolButton(this);
    runButton->setText(tr("Run"));
    offlineCheck_ = new QCheckBox(tr("Offline"), this);
    skipTestsCheck_ = new QCheckBox(tr("Skip Tests"), this);
    auto *settingsButton = new QToolButton(this);
    settingsButton->setText(tr("Settings…"));

    auto *toolbar = new QHBoxLayout();
    toolbar->addWidget(reloadButton);
    toolbar->addWidget(executeEdit_, 1);
    toolbar->addWidget(runButton);
    toolbar->addWidget(offlineCheck_);
    toolbar->addWidget(skipTestsCheck_);
    toolbar->addWidget(settingsButton);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(2);
    tree_->setHeaderLabels({tr("Name"), tr("Detail")});
    tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);

    statusLabel_ = new QLabel(this);
    statusLabel_->setWordWrap(true);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addLayout(toolbar);
    layout->addWidget(statusLabel_);
    layout->addWidget(tree_, 1);

    connect(reloadButton, &QToolButton::clicked, this,
            [this]() { buildToolsService_->sync(); });
    connect(runButton, &QToolButton::clicked, this, [this]() {
        if (QTreeWidgetItem *item = tree_->currentItem()) {
            runNode(item->data(0, kIdRole).toString(), executeEdit_->text());
        }
    });
    connect(tree_, &QTreeWidget::itemDoubleClicked, this, [this](QTreeWidgetItem *item, int) {
        if (item->data(0, kRunnableRole).toBool()) {
            runNode(item->data(0, kIdRole).toString(), QString());
        }
    });
    connect(tree_, &QTreeWidget::itemChanged, this, [this](QTreeWidgetItem *item, int column) {
        if (column != 0 || !item->data(0, kIsProfileRole).toBool()) {
            return;
        }
        buildToolsService_->setProfileChecked(item->text(0), item->checkState(0) == Qt::Checked);
    });
    connect(tree_, &QTreeWidget::customContextMenuRequested, this,
            &BuildToolsPanel::showContextMenu);
    connect(offlineCheck_, &QCheckBox::toggled, this,
            [this](bool on) { buildToolsService_->setOffline(on); });
    connect(skipTestsCheck_, &QCheckBox::toggled, this,
            [this](bool on) { buildToolsService_->setSkipTests(on); });
    connect(settingsButton, &QToolButton::clicked, this, [this]() {
        if (openSettings_) {
            openSettings_();
        }
    });

    connect(buildToolsService_, &BuildToolsService::modelChanged, this, [this]() {
        refreshTree();
        refreshTitle();
    });
    connect(buildToolsService_, &BuildToolsService::syncStateChanged, this,
            &BuildToolsPanel::refreshTitle);
    connect(buildToolsService_, &BuildToolsService::bannerChanged, this,
            &BuildToolsPanel::refreshBanner);

    refreshTree();
    refreshTitle();
    refreshBanner();
}

void BuildToolsPanel::refreshTitle()
{
    const FfiBuildToolTitleKind title = buildToolsService_->titleKind();
    // Skip Tests only means something for Maven (`-DskipTests`); Gradle's
    // own toggle above already covers the "skip test task" case through
    // `-x test` regardless, so hiding this one for a pure-Gradle project
    // avoids a control that reads as redundant.
    skipTestsCheck_->setVisible(title == FfiBuildToolTitleKind::Maven
                                 || title == FfiBuildToolTitleKind::Both);
}

void BuildToolsPanel::refreshBanner()
{
    // The editor banner (B4) owns the trust/reload prompts; this label only
    // reports a sync failure inline, since it is already visible when the
    // dock is open and the banner's own dismissal should not hide that.
    if (buildToolsService_->syncStateKind() == FfiSyncStateKind::Failed) {
        statusLabel_->setText(buildToolsService_->syncMessage());
        statusLabel_->setVisible(true);
    } else {
        statusLabel_->setVisible(false);
    }
}

void BuildToolsPanel::refreshTree()
{
    // Rebuilding sets every profile row's check state from the model, which
    // would otherwise re-fire `itemChanged` back into `setProfileChecked`
    // and loop: that slot calls `modelChanged`, which is exactly the signal
    // that reaches this function.
    const QSignalBlocker blocker(tree_);
    QHash<QString, QTreeWidgetItem *> itemsById;
    QHash<QString, bool> expandedById;
    for (auto *item : tree_->findItems(QString(), Qt::MatchContains | Qt::MatchRecursive)) {
        expandedById.insert(item->data(0, kIdRole).toString(), item->isExpanded());
    }
    tree_->clear();

    const ::rust::Vec<FfiBuildToolNode> rows = buildToolsService_->rows();
    if (rows.empty()) {
        statusLabel_->setText(
          tr("No Gradle or Maven project detected, or it has not been loaded yet."));
        statusLabel_->setVisible(buildToolsService_->syncStateKind() != FfiSyncStateKind::Failed);
        return;
    }

    for (const FfiBuildToolNode &node : rows) {
        const QString id = QString(node.id);
        const QString parentId = QString(node.parentId);
        QTreeWidgetItem *parentItem = parentId.isEmpty() ? nullptr : itemsById.value(parentId);
        auto *item = parentItem ? new QTreeWidgetItem(parentItem) : new QTreeWidgetItem(tree_);
        item->setText(0, QString(node.label));
        item->setText(1, QString(node.detail));
        item->setIcon(0, iconForKind(node.kind));
        item->setData(0, kIdRole, id);
        item->setData(0, kToolRole, QString(node.tool));
        item->setData(0, kRunnableRole, node.kind == FfiBuildToolNodeKind::Task);
        item->setData(0, kBuildFileRole, QString(node.buildFile));
        if (node.kind == FfiBuildToolNodeKind::Profile) {
            item->setData(0, kIsProfileRole, true);
            item->setFlags(item->flags() | Qt::ItemIsUserCheckable);
            item->setCheckState(0, node.checked ? Qt::Checked : Qt::Unchecked);
        }
        item->setExpanded(expandedById.value(id, node.kind == FfiBuildToolNodeKind::ToolRoot));
        itemsById.insert(id, item);
    }
    e2eMark(QStringLiteral("{\"ev\":\"build_tools_model_changed\",\"nodes\":%1}")
              .arg(static_cast<int>(rows.size())));
}

void BuildToolsPanel::runNode(const QString &nodeId, const QString &extraArgs)
{
    const FfiRunConfig config = extraArgs.trimmed().isEmpty()
                                  ? buildToolsService_->taskConfig(nodeId)
                                  : buildToolsService_->taskConfigWithArgs(nodeId, extraArgs);
    if (QString(config.program).isEmpty()) {
        return;
    }
    const FfiResult result = runService_->runTemporary(config);
    if (result.code != 0) {
        QMessageBox::warning(this, tr("Run"), result.message);
    }
}

void BuildToolsPanel::showContextMenu(const QPoint &pos)
{
    QTreeWidgetItem *item = tree_->itemAt(pos);
    if (!item) {
        return;
    }
    const QString nodeId = item->data(0, kIdRole).toString();
    const bool runnable = item->data(0, kRunnableRole).toBool();
    const QString buildFile = item->data(0, kBuildFileRole).toString();
    if (!runnable && buildFile.isEmpty()) {
        return;
    }

    QMenu menu(tree_);
    QAction *run = runnable ? menu.addAction(tr("Run")) : nullptr;
    QAction *runWithArgs = runnable ? menu.addAction(tr("Run with Arguments…")) : nullptr;
    if (runnable) {
        menu.addSeparator();
    }
    QAction *openBuildFile =
      !buildFile.isEmpty() ? menu.addAction(tr("Open Build File")) : nullptr;
    QAction *copy = menu.addAction(tr("Copy Coordinate"));
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == run) {
        runNode(nodeId, QString());
    } else if (chosen == runWithArgs) {
        runNode(nodeId, executeEdit_->text());
    } else if (chosen == openBuildFile) {
        if (openAt_) {
            openAt_(buildFile, 1, 0);
        }
    } else if (chosen == copy) {
        QGuiApplication::clipboard()->setText(item->text(0));
    }
}

BuildToolsPanel *buildBuildToolsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                      ads::CDockAreaWidget *relativeTo,
                                      BuildToolsService *buildToolsService, RunService *runService,
                                      BuildToolsPanel::OpenAt openAt)
{
    auto *panel = new BuildToolsPanel(buildToolsService, runService, std::move(openAt), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Build Tools"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("buildTools"), dock, ads::RightDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("buildTools"));

    // The dock's own title tracks which tool(s) synced and the sync state —
    // set from here, where `dock` is a plain local pointer, rather than
    // walking up `panel`'s parent chain to find it.
    const auto updateTitle = [dock, buildToolsService]() {
        QString text = titleFor(buildToolsService->titleKind());
        switch (buildToolsService->syncStateKind()) {
        case FfiSyncStateKind::Syncing:
            text = QObject::tr("%1 — syncing…").arg(text);
            break;
        case FfiSyncStateKind::Failed:
            text = QObject::tr("%1 — sync failed").arg(text);
            break;
        case FfiSyncStateKind::Idle:
            break;
        }
        dock->setWindowTitle(text);
    };
    updateTitle();
    QObject::connect(buildToolsService, &BuildToolsService::modelChanged, dock, updateTitle);
    QObject::connect(buildToolsService, &BuildToolsService::syncStateChanged, dock, updateTitle);

    return panel;
}

} // namespace ui_shell
