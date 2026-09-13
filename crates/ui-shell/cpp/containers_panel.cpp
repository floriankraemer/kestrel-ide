#include "containers_panel.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "theme.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QApplication>
#include <QClipboard>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QPoint>
#include <QSplitter>
#include <QTabWidget>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kKindRole = Qt::UserRole + 1;
constexpr int kConnectionRole = Qt::UserRole + 2;
constexpr int kStatusRole = Qt::UserRole + 3;
constexpr int kDetailRole = Qt::UserRole + 4;
constexpr int kResourceIdRole = Qt::UserRole + 5;

// The `icon` key a row carries (decided in `container_core::tree`) to the
// mask that paints it — a lookup table, not a decision.
const char *maskForIconKey(const QString &key)
{
    static const QHash<QString, const char *> masks = {
      {QStringLiteral("docker"), ":/ui/icons/containers/docker.a8"},
      {QStringLiteral("podman"), ":/ui/icons/containers/podman.a8"},
      {QStringLiteral("container-running"), ":/ui/icons/containers/container-running.a8"},
      {QStringLiteral("container-stopped"), ":/ui/icons/containers/container-stopped.a8"},
      {QStringLiteral("image"), ":/ui/icons/containers/image.a8"},
      {QStringLiteral("network"), ":/ui/icons/containers/network.a8"},
      {QStringLiteral("volume"), ":/ui/icons/containers/volume.a8"},
      {QStringLiteral("compose"), ":/ui/icons/containers/compose.a8"},
      {QStringLiteral("pod"), ":/ui/icons/containers/pod.a8"},
      {QStringLiteral("registry"), ":/ui/icons/containers/registry.a8"},
    };
    return masks.value(key, ":/ui/icons/containers/container-stopped.a8");
}

QIcon rowIcon(const QString &key)
{
    const SemanticColors colors = semanticColors();
    const ChromePalette chrome = chromePaletteForTheme(activeThemeName());
    const QColor tint = key == QStringLiteral("container-running") ? colors.ok : chrome.textDim;
    return maskIcon(maskForIconKey(key), tint);
}

QToolButton *iconButton(const char *mask, const QString &toolTip, QWidget *parent)
{
    auto *button = new QToolButton(parent);
    button->setIcon(maskIcon(mask, chromePaletteForTheme(activeThemeName()).textDim));
    button->setIconSize(QSize(16, 16));
    button->setAutoRaise(true);
    button->setFocusPolicy(Qt::NoFocus);
    button->setToolTip(toolTip);
    return button;
}

QLabel *readOnlyValue(QWidget *parent)
{
    auto *label = new QLabel(parent);
    label->setTextInteractionFlags(Qt::TextSelectableByMouse);
    label->setWordWrap(true);
    return label;
}

} // namespace

ContainersPanel::ContainersPanel(ContainerService *containerService, QWidget *parent)
  : QWidget(parent)
  , containerService_(containerService)
{
    addButton_ = iconButton(":/ui/icons/containers/add.a8", tr("Add"), this);
    addButton_->setPopupMode(QToolButton::InstantPopup);
    auto *addMenu = new QMenu(addButton_);
    QAction *newConnection = addMenu->addAction(tr("New connection..."));
    QAction *addFromContexts = addMenu->addAction(tr("Add from contexts..."));
    addButton_->setMenu(addMenu);
    connect(newConnection, &QAction::triggered, this, [this]() {
        if (openSettings_) {
            openSettings_();
        }
    });
    // Discovery lives on the Settings page (C1) — both entries open it.
    connect(addFromContexts, &QAction::triggered, this, [this]() {
        if (openSettings_) {
            openSettings_();
        }
    });

    refreshButton_ = iconButton(":/ui/icons/diff/sync.a8", tr("Refresh"), this);
    connect(refreshButton_, &QToolButton::clicked, this,
            [this]() { containerService_->refreshAll(); });

    connectButton_ = iconButton(":/ui/icons/containers/connect.a8", tr("Connect"), this);
    connect(connectButton_, &QToolButton::clicked, this, [this]() {
        const QString id = selectedConnectionId();
        if (!id.isEmpty()) {
            report(containerService_->connectEngine(id));
        }
    });

    disconnectButton_ =
      iconButton(":/ui/icons/containers/disconnect.a8", tr("Disconnect"), this);
    connect(disconnectButton_, &QToolButton::clicked, this, [this]() {
        const QString id = selectedConnectionId();
        if (!id.isEmpty()) {
            report(containerService_->disconnectEngine(id));
        }
    });

    // Placeholders until C4 (Pull Image) and C3/C4 (Clean Up) give them
    // behaviour; disabled so the toolbar already has its final shape.
    pullButton_ = iconButton(":/ui/icons/containers/registry.a8", tr("Pull Image..."), this);
    pullButton_->setEnabled(false);
    cleanUpButton_ = iconButton(":/ui/icons/containers/cleanup.a8", tr("Clean Up"), this);
    cleanUpButton_->setEnabled(false);

    filterButton_ = iconButton(":/ui/icons/containers/filter.a8", tr("Filter"), this);
    filterButton_->setPopupMode(QToolButton::InstantPopup);
    auto *filterMenu = new QMenu(filterButton_);
    showStoppedAction_ = filterMenu->addAction(tr("Stopped containers"));
    showStoppedAction_->setCheckable(true);
    showUntaggedAction_ = filterMenu->addAction(tr("Untagged images"));
    showUntaggedAction_->setCheckable(true);
    const FfiContainerFilter initial = containerService_->filter();
    showStoppedAction_->setChecked(initial.showStopped);
    showUntaggedAction_->setChecked(initial.showUntagged);
    filterButton_->setMenu(filterMenu);
    const auto applyFilter = [this]() {
        report(containerService_->setFilter(showStoppedAction_->isChecked(),
                                            showUntaggedAction_->isChecked()));
    };
    connect(showStoppedAction_, &QAction::toggled, this, applyFilter);
    connect(showUntaggedAction_, &QAction::toggled, this, applyFilter);

    searchEdit_ = new QLineEdit(this);
    searchEdit_->setPlaceholderText(tr("Type to filter"));
    searchEdit_->setClearButtonEnabled(true);
    searchEdit_->addAction(searchIcon(), QLineEdit::LeadingPosition);
    connect(searchEdit_, &QLineEdit::textChanged, this,
            [this](const QString &text) { containerService_->setSearch(text); });

    statusLabel_ = new QLabel(this);

    auto *toolbar = new QHBoxLayout();
    toolbar->addWidget(addButton_);
    toolbar->addWidget(refreshButton_);
    toolbar->addWidget(connectButton_);
    toolbar->addWidget(disconnectButton_);
    toolbar->addWidget(pullButton_);
    toolbar->addWidget(cleanUpButton_);
    toolbar->addWidget(filterButton_);
    toolbar->addWidget(statusLabel_, 1);
    toolbar->addWidget(searchEdit_);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(3);
    tree_->setHeaderLabels({tr("Name"), tr("Status"), tr("Details")});
    tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    tree_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    tree_->header()->setSectionResizeMode(2, QHeaderView::ResizeToContents);
    tree_->setUniformRowHeights(true);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);

    detailTabs_ = new QTabWidget(this);
    auto *dashboard = new QWidget(detailTabs_);
    auto *form = new QFormLayout(dashboard);
    form->setLabelAlignment(Qt::AlignRight | Qt::AlignTop);
    dashboardName_ = readOnlyValue(dashboard);
    dashboardId_ = readOnlyValue(dashboard);
    dashboardStatus_ = readOnlyValue(dashboard);
    dashboardDetail_ = readOnlyValue(dashboard);
    form->addRow(tr("Name"), dashboardName_);
    form->addRow(tr("ID"), dashboardId_);
    form->addRow(tr("Status"), dashboardStatus_);
    form->addRow(tr("Details"), dashboardDetail_);
    detailTabs_->addTab(dashboard, tr("Dashboard"));

    auto *splitter = new QSplitter(Qt::Horizontal, this);
    splitter->addWidget(tree_);
    splitter->addWidget(detailTabs_);
    splitter->setStretchFactor(0, 1);
    splitter->setStretchFactor(1, 2);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addLayout(toolbar);
    layout->addWidget(splitter, 1);

    connect(tree_, &QTreeWidget::itemSelectionChanged, this,
            &ContainersPanel::onSelectionChanged);
    connect(tree_, &QTreeWidget::customContextMenuRequested, this,
            &ContainersPanel::showContextMenu);
    connect(tree_, &QTreeWidget::itemDoubleClicked, this, [this](QTreeWidgetItem *item, int) {
        if (item->data(0, kKindRole).toString() == QStringLiteral("connection")) {
            report(containerService_->connectEngine(item->data(0, kIdRole).toString()));
        }
    });
    connect(containerService_, &ContainerService::treeChanged, this,
            &ContainersPanel::onTreeChanged);
    connect(containerService_, &ContainerService::connectionStateChanged, this,
            [this](const QString &) { updateToolbarEnablement(); });

    onTreeChanged();
}

void ContainersPanel::setOpenSettingsHandler(OpenSettings handler)
{
    openSettings_ = std::move(handler);
}

void ContainersPanel::report(const FfiResult &result)
{
    if (result.code == 0) {
        return;
    }
    statusLabel_->setText(QString(result.message));
    e2eMark(QStringLiteral("{\"ev\":\"containers_refused\",\"code\":%1}").arg(result.code));
}

QTreeWidgetItem *ContainersPanel::selectedItem() const
{
    return tree_->currentItem();
}

QString ContainersPanel::selectedConnectionId() const
{
    QTreeWidgetItem *item = selectedItem();
    if (item == nullptr) {
        // With nothing selected, a single configured connection is the
        // obvious target; otherwise the user has to pick one.
        return tree_->topLevelItemCount() == 1
          ? tree_->topLevelItem(0)->data(0, kIdRole).toString()
          : QString();
    }
    return item->data(0, kConnectionRole).toString();
}

void ContainersPanel::onTreeChanged()
{
    QHash<QString, bool> expandedById;
    for (auto it = itemsById_.constBegin(); it != itemsById_.constEnd(); ++it) {
        expandedById.insert(it.key(), it.value()->isExpanded());
    }

    tree_->blockSignals(true);
    tree_->clear();
    itemsById_.clear();

    const ::rust::Vec<FfiContainerNode> nodes = containerService_->nodes();
    for (const FfiContainerNode &node : nodes) {
        const QString id = QString(node.id);
        const QString parentId = QString(node.parentId);
        QTreeWidgetItem *parentItem = parentId.isEmpty() ? nullptr : itemsById_.value(parentId);
        auto *item = parentItem ? new QTreeWidgetItem(parentItem) : new QTreeWidgetItem(tree_);
        item->setText(0, QString(node.name));
        item->setText(1, QString(node.status));
        item->setText(2, QString(node.detail));
        item->setIcon(0, rowIcon(QString(node.icon)));
        const QString tooltip = QString(node.tooltip);
        if (!tooltip.isEmpty()) {
            item->setToolTip(0, tooltip);
        }
        item->setData(0, kIdRole, id);
        item->setData(0, kKindRole, QString(node.kind));
        item->setData(0, kConnectionRole, QString(node.connectionId));
        item->setData(0, kStatusRole, QString(node.status));
        item->setData(0, kDetailRole, QString(node.detail));
        item->setData(0, kResourceIdRole, QString(node.resourceId));
        // A connection row opens by default so a fresh connection shows
        // its groups without a click; groups and deeper rows start
        // collapsed. A row seen before keeps what the user chose.
        item->setExpanded(expandedById.value(id, parentItem == nullptr));
        itemsById_.insert(id, item);
    }

    if (!selectedNodeId_.isEmpty()) {
        if (QTreeWidgetItem *item = itemsById_.value(selectedNodeId_)) {
            tree_->setCurrentItem(item);
        }
    }
    tree_->blockSignals(false);
    onSelectionChanged();

    const int total = itemsById_.size();
    statusLabel_->setText(tree_->topLevelItemCount() == 0
                            ? tr("No connections configured. Use Add to create one.")
                            : QString());
    e2eMark(QStringLiteral("{\"ev\":\"containers_tree_changed\",\"nodes\":%1}").arg(total));
}

void ContainersPanel::onSelectionChanged()
{
    QTreeWidgetItem *item = selectedItem();
    selectedNodeId_ = item ? item->data(0, kIdRole).toString() : QString();
    if (item == nullptr) {
        dashboardName_->clear();
        dashboardId_->clear();
        dashboardStatus_->clear();
        dashboardDetail_->clear();
    } else {
        dashboardName_->setText(item->text(0));
        dashboardId_->setText(item->data(0, kResourceIdRole).toString());
        dashboardStatus_->setText(item->data(0, kStatusRole).toString());
        dashboardDetail_->setText(item->data(0, kDetailRole).toString());
    }
    updateToolbarEnablement();
}

void ContainersPanel::updateToolbarEnablement()
{
    const QString id = selectedConnectionId();
    if (id.isEmpty()) {
        connectButton_->setEnabled(false);
        disconnectButton_->setEnabled(false);
        return;
    }
    const FfiConnectionState state = containerService_->connectionState(id);
    const bool disconnected = QString(state.state) == QStringLiteral("disconnected");
    connectButton_->setEnabled(disconnected);
    disconnectButton_->setEnabled(!disconnected);
}

void ContainersPanel::showContextMenu(const QPoint &pos)
{
    QTreeWidgetItem *item = tree_->itemAt(pos);
    if (item == nullptr) {
        return;
    }
    tree_->setCurrentItem(item);
    const QString id = item->data(0, kIdRole).toString();
    const QString kind = item->data(0, kKindRole).toString();

    QMenu menu(tree_);
    if (kind == QStringLiteral("connection")) {
        const FfiConnectionState state = containerService_->connectionState(id);
        const bool disconnected = QString(state.state) == QStringLiteral("disconnected");
        QAction *connectAction = menu.addAction(tr("Connect"));
        connectAction->setEnabled(disconnected);
        QAction *disconnectAction = menu.addAction(tr("Disconnect"));
        disconnectAction->setEnabled(!disconnected);
        QAction *refreshAction = menu.addAction(tr("Refresh"));
        refreshAction->setEnabled(!disconnected);
        menu.addSeparator();
        QAction *editAction = menu.addAction(tr("Edit configuration..."));
        QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
        if (chosen == connectAction) {
            report(containerService_->connectEngine(id));
        } else if (chosen == disconnectAction) {
            report(containerService_->disconnectEngine(id));
        } else if (chosen == refreshAction) {
            report(containerService_->refresh(id));
        } else if (chosen == editAction && openSettings_) {
            openSettings_();
        }
        return;
    }

    // Every other row: the skeleton later tasks (C3/C4) fill in.
    QAction *copyId = menu.addAction(tr("Copy ID"));
    copyId->setEnabled(!item->data(0, kResourceIdRole).toString().isEmpty());
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == copyId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    }
}

ContainersPanel *buildContainersDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *relativeTo,
                                     ContainerService *containerService)
{
    auto *panel = new ContainersPanel(containerService, dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Containers"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("containers"), dock, ads::CenterDockWidgetArea,
                        relativeTo);
    docks->hide(QStringLiteral("containers"));
    return panel;
}

} // namespace ui_shell
