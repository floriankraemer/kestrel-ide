#include "containers_panel.h"

#include "containers_detail.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "theme.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QApplication>
#include <QClipboard>
#include <QCompleter>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QLocale>
#include <QMenu>
#include <QMessageBox>
#include <QPoint>
#include <QPushButton>
#include <QSplitter>
#include <QStringListModel>
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

// Group rows carry no name: the label is the view's, by kind.
QString kindLabel(const QString &kind)
{
    if (kind == QStringLiteral("containers-group")) {
        return QObject::tr("Containers");
    }
    if (kind == QStringLiteral("images-group")) {
        return QObject::tr("Images");
    }
    if (kind == QStringLiteral("networks-group")) {
        return QObject::tr("Networks");
    }
    if (kind == QStringLiteral("volumes-group")) {
        return QObject::tr("Volumes");
    }
    if (kind == QStringLiteral("compose-group")) {
        return QObject::tr("Compose");
    }
    if (kind == QStringLiteral("pods-group")) {
        return QObject::tr("Pods");
    }
    return QString();
}

QString ageText(FfiAgeUnit unit, qint64 value)
{
    const int n = static_cast<int>(value);
    switch (unit) {
    case FfiAgeUnit::None:
        return QString();
    case FfiAgeUnit::Seconds:
        return QObject::tr("%n s", "age in seconds", n);
    case FfiAgeUnit::Minutes:
        return QObject::tr("%n min", "age in minutes", n);
    case FfiAgeUnit::Hours:
        return QObject::tr("%n h", "age in hours", n);
    case FfiAgeUnit::Days:
        return QObject::tr("%n d", "age in days", n);
    case FfiAgeUnit::Months:
        return QObject::tr("%n mo", "age in months", n);
    case FfiAgeUnit::Years:
        return QObject::tr("%n y", "age in years", n);
    }
    return QString();
}

// The status word for a row. Which status applies was decided in Rust;
// only the wording happens here.
QString statusWord(const FfiContainerNode &node)
{
    switch (node.status) {
    case FfiContainerNodeStatus::None:
        return QString();
    case FfiContainerNodeStatus::Disconnected:
        return QObject::tr("disconnected");
    case FfiContainerNodeStatus::Connecting:
        return QObject::tr("connecting...");
    case FfiContainerNodeStatus::Connected:
        // `detail` is the engine's product name, `statusText` its version.
        return QObject::tr("%1 Engine %2").arg(QString(node.detail), QString(node.statusText));
    case FfiContainerNodeStatus::Error:
        // One row, one line: the hint lines stay in the tooltip/Dashboard.
        return QObject::tr("error: %1").arg(QString(node.statusText).section(QLatin1Char('\n'), 0, 0));
    case FfiContainerNodeStatus::Running:
        return QObject::tr("running");
    case FfiContainerNodeStatus::Paused:
        return QObject::tr("paused");
    case FfiContainerNodeStatus::Restarting:
        return QObject::tr("restarting");
    case FfiContainerNodeStatus::Exited:
        return QObject::tr("exited (%1)").arg(node.exitCode);
    case FfiContainerNodeStatus::Created:
        return QObject::tr("created");
    case FfiContainerNodeStatus::Dead:
        return QObject::tr("dead");
    case FfiContainerNodeStatus::Other:
        return QString(node.statusText);
    }
    return QString();
}

// The Status column: the status word with its age, a compose row's
// running/total, a group's or network's count, or an image's/volume's age.
QString statusColumn(const FfiContainerNode &node)
{
    const QString word = statusWord(node);
    const QString age = ageText(node.ageUnit, node.ageValue);
    if (!word.isEmpty()) {
        return age.isEmpty() ? word : QObject::tr("%1 (%2)").arg(word, age);
    }
    if (node.running >= 0) {
        return QObject::tr("%1/%2 running").arg(node.running).arg(node.total);
    }
    if (node.kind == QStringLiteral("network")) {
        return QObject::tr("%n container(s)", nullptr, static_cast<int>(node.count));
    }
    if (node.count >= 0) {
        return QString::number(node.count);
    }
    return age;
}

// The Details column: engine text as-is, or an image's size in the
// locale's own SI units, or a pod's container count.
QString detailColumn(const FfiContainerNode &node)
{
    if (node.sizeBytes >= 0) {
        return QLocale().formattedDataSize(node.sizeBytes, 1, QLocale::DataSizeSIFormat);
    }
    if (node.kind == QStringLiteral("pod")) {
        return QObject::tr("%n container(s)", nullptr, static_cast<int>(node.count));
    }
    return QString(node.detail);
}

QLabel *readOnlyValue(QWidget *parent)
{
    auto *label = new QLabel(parent);
    label->setTextInteractionFlags(Qt::TextSelectableByMouse);
    label->setWordWrap(true);
    return label;
}

} // namespace

ContainersPanel::ContainersPanel(ContainerService *containerService,
                                 TerminalSupervisor *terminalSupervisor, AppSettings *appSettings,
                                 OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , containerService_(containerService)
{
    addButton_ = iconButton(":/ui/icons/containers/add.a8", tr("Add"), this);
    addButton_->setPopupMode(QToolButton::InstantPopup);
    auto *addMenu = new QMenu(addButton_);
    QAction *newConnection = addMenu->addAction(tr("New connection..."));
    QAction *addFromContexts = addMenu->addAction(tr("Add from contexts..."));
    QAction *newRegistry = addMenu->addAction(tr("Registry..."));
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
    // C7: a registry is added on the Registries tab of the same page —
    // there is no per-tab-selection hook on `openSettings_` yet, so this
    // opens the page on whichever tab was last shown, same as the two
    // entries above did before C7.
    connect(newRegistry, &QAction::triggered, this, [this]() {
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

    pullButton_ = iconButton(":/ui/icons/containers/registry.a8", tr("Pull Image..."), this);
    connect(pullButton_, &QToolButton::clicked, this, &ContainersPanel::triggerPullImage);

    // C4: every "Clean Up" kind on the selected connection, gated per
    // engine by `nodeActions`'s answer for whichever group is selected —
    // unavailable kinds (build cache on Podman) just aren't offered.
    cleanUpButton_ = iconButton(":/ui/icons/containers/cleanup.a8", tr("Clean Up..."), this);
    cleanUpButton_->setPopupMode(QToolButton::InstantPopup);
    auto *cleanUpMenu = new QMenu(cleanUpButton_);
    const auto cleanUp = [this](const QString &kind, const QString &confirmText) {
        const QString connectionId = selectedConnectionId();
        if (connectionId.isEmpty()) {
            return;
        }
        if (QMessageBox::question(this, tr("Clean Up"), confirmText, QMessageBox::Yes | QMessageBox::Cancel,
                                  QMessageBox::Cancel)
            != QMessageBox::Yes) {
            return;
        }
        report(containerService_->cleanUp(connectionId, kind));
    };
    connect(cleanUpMenu->addAction(tr("All (stopped containers, unused networks/volumes, dangling images)")),
            &QAction::triggered, this,
            [cleanUp]() { cleanUp(QStringLiteral("all"), tr("Clean up everything unused on this connection?")); });
    connect(cleanUpMenu->addAction(tr("Stopped Containers")), &QAction::triggered, this,
            [cleanUp]() {
                cleanUp(QStringLiteral("stopped-containers"), tr("Remove every stopped container?"));
            });
    connect(cleanUpMenu->addAction(tr("Unused Networks")), &QAction::triggered, this, [cleanUp]() {
        cleanUp(QStringLiteral("unused-networks"), tr("Remove every unused network?"));
    });
    connect(cleanUpMenu->addAction(tr("Unused Volumes")), &QAction::triggered, this, [cleanUp]() {
        cleanUp(QStringLiteral("unused-volumes"), tr("Remove every unused volume?"));
    });
    connect(cleanUpMenu->addAction(tr("Dangling Images")), &QAction::triggered, this, [cleanUp]() {
        cleanUp(QStringLiteral("dangling-images"), tr("Remove every dangling image?"));
    });
    connect(cleanUpMenu->addAction(tr("Build Cache")), &QAction::triggered, this, [cleanUp]() {
        cleanUp(QStringLiteral("build-cache"), tr("Remove the build cache?"));
    });
    cleanUpButton_->setMenu(cleanUpMenu);

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
    buildLifecycleToolbar(); // C3: fills start/stop/restart/pause/removeButton_.

    toolbar->addWidget(addButton_);
    toolbar->addWidget(refreshButton_);
    toolbar->addWidget(connectButton_);
    toolbar->addWidget(disconnectButton_);
    toolbar->addWidget(startButton_);
    toolbar->addWidget(stopButton_);
    toolbar->addWidget(restartButton_);
    toolbar->addWidget(pauseButton_);
    toolbar->addWidget(removeButton_);
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

    // C4: the Images console row lives above the detail tabs, visible only
    // when the Images group is selected (`onSelectionChanged`).
    imagesConsole_ = new QWidget(this);
    auto *consoleLayout = new QHBoxLayout(imagesConsole_);
    consoleLayout->setContentsMargins(0, 0, 0, 0);
    consoleLayout->addWidget(new QLabel(tr("Image to pull:"), imagesConsole_));
    pullEdit_ = new QLineEdit(imagesConsole_);
    pullCompleter_ = new QCompleter(imagesConsole_);
    pullCompleter_->setCaseSensitivity(Qt::CaseInsensitive);
    pullEdit_->setCompleter(pullCompleter_);
    connect(pullEdit_, &QLineEdit::textEdited, this, [this](const QString &text) {
        const QStringList items = containerService_->imageCompletions(selectedConnectionId(), text)
                                     .split(QLatin1Char('\n'), Qt::SkipEmptyParts);
        pullCompleter_->setModel(new QStringListModel(items, pullCompleter_));
    });
    consoleLayout->addWidget(pullEdit_, 1);
    auto *pullGoButton = new QPushButton(tr("Pull"), imagesConsole_);
    consoleLayout->addWidget(pullGoButton);
    const auto pullFromConsole = [this]() {
        const QString reference = pullEdit_->text().trimmed();
        const QString connectionId = selectedConnectionId();
        if (reference.isEmpty() || connectionId.isEmpty()) {
            return;
        }
        detail_->openPullTab(connectionId, reference);
        pullEdit_->clear();
    };
    connect(pullGoButton, &QPushButton::clicked, this, pullFromConsole);
    connect(pullEdit_, &QLineEdit::returnPressed, this, pullFromConsole);
    imagesConsole_->setVisible(false);

    auto *detailContainer = new QWidget(this);
    auto *detailLayout = new QVBoxLayout(detailContainer);
    detailLayout->setContentsMargins(0, 0, 0, 0);
    detailLayout->addWidget(imagesConsole_);
    detailLayout->addWidget(detailTabs_, 1);

    auto *splitter = new QSplitter(Qt::Horizontal, this);
    splitter->addWidget(tree_);
    splitter->addWidget(detailContainer);
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
    // C7: a registry/registry-repo node's children are fetched on first
    // expand rather than eagerly — `nodes()` already includes whatever is
    // cached, so an empty `childCount()` means "not fetched yet" (or
    // fetched-and-genuinely-empty, in which case this re-asks once more
    // per expand, a redundant network call rather than a correctness bug).
    connect(tree_, &QTreeWidget::itemExpanded, this, [this](QTreeWidgetItem *item) {
        if (item->childCount() > 0) {
            return;
        }
        const QString kind = item->data(0, kKindRole).toString();
        const QString id = item->data(0, kIdRole).toString();
        if (kind == QStringLiteral("registry")) {
            containerService_->loadRegistryRepositories(id);
        } else if (kind == QStringLiteral("registry-repo")) {
            containerService_->loadRegistryTags(id);
        }
    });
    connect(containerService_, &ContainerService::registryChildrenReady, this,
            [this](const QString &) { onTreeChanged(); });
    connect(containerService_, &ContainerService::connectionStateChanged, this,
            [this](const QString &) { updateToolbarEnablement(); });
    connect(containerService_, &ContainerService::actionFinished, this,
            [this](const QString &, bool ok, const QString &message) {
                if (!ok) {
                    statusLabel_->setText(message);
                }
            });

    detail_ = new ContainerDetailArea(containerService_, terminalSupervisor, appSettings,
                                      std::move(openAt), detailTabs_, this);
    detail_->setGenericDashboardPage(dashboard);
    // C4: a clicked "containers using it" row on an image/network/volume
    // Dashboard selects that container node in the tree.
    connect(detail_, &ContainerDetailArea::containerNodeRequested, this,
            [this](const QString &nodeId) {
                if (QTreeWidgetItem *item = itemsById_.value(nodeId)) {
                    tree_->setCurrentItem(item);
                }
            });

    onTreeChanged();
}

void ContainersPanel::revealContainerLog(const QString &nodeId)
{
    QTreeWidgetItem *item = itemsById_.value(nodeId);
    if (item == nullptr) {
        return;
    }
    tree_->setCurrentItem(item);
    // The Log tab sits right after the Dashboard for a container node
    // (`ContainerDetailArea::openOrReplaceLogTab` inserts it at 1).
    detailTabs_->setCurrentIndex(1);
}

void ContainersPanel::openPullTab(const QString &connectionId, const QString &reference)
{
    detail_->openPullTab(connectionId, reference);
}

void ContainersPanel::setOpenSettingsHandler(OpenSettings handler)
{
    openSettings_ = std::move(handler);
}

void ContainersPanel::setRunContext(RunService *runService, RunConfigEditor *runConfigEditor,
                                    EditorTabs *editorTabs)
{
    runService_ = runService;
    runConfigEditor_ = runConfigEditor;
    editorTabs_ = editorTabs;
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
        const QString name = QString(node.name);
        const QString status = statusColumn(node);
        const QString detail = detailColumn(node);
        item->setText(0, name.isEmpty() ? kindLabel(QString(node.kind)) : name);
        item->setText(1, status);
        item->setText(2, detail);
        item->setIcon(0, rowIcon(QString(node.icon)));
        const QString tooltip = QString(node.tooltip);
        if (!tooltip.isEmpty()) {
            item->setToolTip(0, tooltip);
        }
        item->setData(0, kIdRole, id);
        item->setData(0, kKindRole, QString(node.kind));
        item->setData(0, kConnectionRole, QString(node.connectionId));
        // The Dashboard has room for the whole error, hint lines included.
        item->setData(0, kStatusRole,
                      node.status == FfiContainerNodeStatus::Error
                        ? QObject::tr("error: %1").arg(QString(node.statusText))
                        : status);
        item->setData(0, kDetailRole, detail);
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
        detail_->clearSelection();
    } else {
        dashboardName_->setText(item->text(0));
        dashboardId_->setText(item->data(0, kResourceIdRole).toString());
        dashboardStatus_->setText(item->data(0, kStatusRole).toString());
        dashboardDetail_->setText(item->data(0, kDetailRole).toString());
        // A compose project's Dashboard is its services and their
        // container counts (C5, ADR-0056) — already sitting right there as
        // this item's own children in the tree (each one's own status
        // column already reads "running/total"), so this is display, not a
        // new query: no counts are computed here that `ContainerService`
        // did not already put on the child rows.
        if (item->data(0, kKindRole).toString() == QStringLiteral("compose-project")) {
            QStringList lines;
            for (int i = 0; i < item->childCount(); ++i) {
                QTreeWidgetItem *service = item->child(i);
                lines << QStringLiteral("%1: %2").arg(service->text(0), service->text(1));
            }
            dashboardDetail_->setText(lines.join(QLatin1Char('\n')));
        }
        // Image/network/volume nodes get their own Dashboard tab, built and
        // populated by `ContainerDetailArea::onSelectionChanged` below; the
        // generic labels above are only shown for every other kind.
        detail_->onSelectionChanged(selectedNodeId_, item->data(0, kKindRole).toString());
    }
    imagesConsole_->setVisible(item != nullptr
                               && item->data(0, kKindRole).toString()
                                    == QStringLiteral("images-group"));
    updateToolbarEnablement();
}

void ContainersPanel::updateToolbarEnablement()
{
    updateLifecycleButtons();
    const QString id = selectedConnectionId();
    if (id.isEmpty()) {
        connectButton_->setEnabled(false);
        disconnectButton_->setEnabled(false);
        cleanUpButton_->setEnabled(false);
        return;
    }
    const FfiConnectionState state = containerService_->connectionState(id);
    const bool disconnected = QString(state.state) == QStringLiteral("disconnected");
    connectButton_->setEnabled(disconnected);
    disconnectButton_->setEnabled(!disconnected);
    cleanUpButton_->setEnabled(!disconnected);
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

    if (kind == QStringLiteral("container")) {
        showContainerContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("containers-group")) {
        showContainersGroupContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("image")) {
        showImageContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("network")) {
        showNetworkContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("volume")) {
        showVolumeContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("images-group")) {
        showImagesGroupContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("networks-group")) {
        showNetworksGroupContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("volumes-group")) {
        showVolumesGroupContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("compose-project")) {
        showComposeProjectContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("compose-service")) {
        showComposeServiceContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("registry")) {
        showRegistryContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("registry-repo")) {
        showRegistryRepoContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }
    if (kind == QStringLiteral("registry-tag")) {
        showRegistryTagContextMenu(item, tree_->viewport()->mapToGlobal(pos));
        return;
    }

    // Every other row (pod, ...): a later task's.
    QAction *copyId = menu.addAction(tr("Copy ID"));
    copyId->setEnabled(!item->data(0, kResourceIdRole).toString().isEmpty());
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == copyId) {
        QApplication::clipboard()->setText(item->data(0, kResourceIdRole).toString());
    }
}

ContainersPanel *buildContainersDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *relativeTo,
                                     ContainerService *containerService,
                                     TerminalSupervisor *terminalSupervisor,
                                     AppSettings *appSettings,
                                     ContainersPanel::OpenAt openAt)
{
    auto *panel = new ContainersPanel(containerService, terminalSupervisor, appSettings,
                                      std::move(openAt), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Containers"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("containers"), dock, ads::CenterDockWidgetArea,
                        relativeTo);
    docks->hide(QStringLiteral("containers"));
    // C6: the editor's compose lenses and "Pull image" land in this dock.
    QObject::connect(containerService, &ContainerService::containerLogRequested, panel,
                     [panel, docks](const QString &nodeId) {
                         docks->show(QStringLiteral("containers"));
                         panel->revealContainerLog(nodeId);
                     });
    QObject::connect(containerService, &ContainerService::pullRequested, panel,
                     [panel, docks](const QString &connectionId, const QString &reference) {
                         docks->show(QStringLiteral("containers"));
                         panel->openPullTab(connectionId, reference);
                     });
    return panel;
}

} // namespace ui_shell
