#include "containers_detail.h"

#include "terminal_widget.h"

#include <QAbstractItemView>
#include <QFileDialog>
#include <QFont>
#include <QFormLayout>
#include <QHash>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QInputDialog>
#include <QLabel>
#include <QListWidget>
#include <QLocale>
#include <QMenu>
#include <QPoint>
#include <QPushButton>
#include <QTabBar>
#include <QTableWidget>
#include <QTabWidget>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kFilesPathRole = Qt::UserRole;
constexpr int kFilesKindRole = Qt::UserRole + 1;
// A single placeholder child, added under every directory entry so it
// paints an expand arrow before its real children are known — the common
// "lazy tree" trick, removed the moment `itemExpanded` asks for the real
// listing.
constexpr int kFilesLoadedRole = Qt::UserRole + 2;

// The Log tab's tail: generous scrollback without asking the engine to
// replay a container's entire history on every "Restart".
constexpr quint32 kLogTail = 2000;

constexpr int kContainerNodeIdRole = Qt::UserRole;

QLabel *readOnlyValue(QWidget *parent)
{
    auto *label = new QLabel(parent);
    label->setTextInteractionFlags(Qt::TextSelectableByMouse);
    label->setWordWrap(true);
    return label;
}

} // namespace

ContainerDetailArea::ContainerDetailArea(ContainerService *containerService,
                                        TerminalSupervisor *terminalSupervisor,
                                        AppSettings *appSettings, OpenAt openAt, QTabWidget *tabs,
                                        QObject *parent)
  : QObject(parent)
  , containerService_(containerService)
  , terminalSupervisor_(terminalSupervisor)
  , appSettings_(appSettings)
  , openAt_(std::move(openAt))
  , tabs_(tabs)
{
    // Closable globally; `addTerminalTab` is the only place that adds a
    // closable tab in practice — Dashboard/Log/Processes/Files each strip
    // their own close button off right after `addTab`/`insertTab`.
    tabs_->setTabsClosable(true);
    connect(tabs_, &QTabWidget::tabCloseRequested, this, [this](int index) {
        closeTerminalTab(index);
    });

    connect(containerService_, &ContainerService::processesReady, this,
            [this](const QString &nodeId, const QString &titles, const ::rust::Vec<FfiProcessRow> &rows) {
                if (nodeId != nodeId_ || processesTable_ == nullptr) {
                    return;
                }
                const QStringList columns = titles.split(QLatin1Char('\t'));
                processesTable_->clear();
                processesTable_->setColumnCount(columns.size());
                processesTable_->setHorizontalHeaderLabels(columns);
                processesTable_->setRowCount(static_cast<int>(rows.size()));
                int row = 0;
                for (const FfiProcessRow &entry : rows) {
                    const QStringList cells = QString(entry.cells).split(QLatin1Char('\t'));
                    for (int column = 0; column < cells.size() && column < columns.size(); ++column) {
                        processesTable_->setItem(row, column, new QTableWidgetItem(cells.at(column)));
                    }
                    ++row;
                }
                processesTable_->resizeColumnsToContents();
            });

    connect(containerService_, &ContainerService::filesReady, this,
            [this](const QString &nodeId, const QString &dir, const ::rust::Vec<FfiFileEntry> &entries) {
                if (nodeId != nodeId_ || filesTree_ == nullptr) {
                    return;
                }
                QTreeWidgetItem *target = dir == QStringLiteral("/")
                  ? filesTree_->invisibleRootItem()
                  : nullptr;
                if (target == nullptr) {
                    // Find the item awaiting this directory's children by
                    // its stored path — the tree is small enough that a
                    // linear walk per response is not worth a side map.
                    QList<QTreeWidgetItem *> stack;
                    for (int i = 0; i < filesTree_->invisibleRootItem()->childCount(); ++i) {
                        stack.append(filesTree_->invisibleRootItem()->child(i));
                    }
                    while (!stack.isEmpty() && target == nullptr) {
                        QTreeWidgetItem *item = stack.takeLast();
                        if (item->data(0, kFilesPathRole).toString() == dir) {
                            target = item;
                            break;
                        }
                        for (int i = 0; i < item->childCount(); ++i) {
                            stack.append(item->child(i));
                        }
                    }
                }
                if (target == nullptr) {
                    return;
                }
                target->takeChildren();
                for (const FfiFileEntry &entry : entries) {
                    const QString name = entry.name;
                    const QString kind = entry.kind;
                    const QString path = dir.endsWith(QLatin1Char('/')) ? dir + name : dir + QLatin1Char('/') + name;
                    auto *child = new QTreeWidgetItem(target);
                    child->setText(0, name);
                    child->setData(0, kFilesPathRole, path);
                    child->setData(0, kFilesKindRole, kind);
                    child->setData(0, kFilesLoadedRole, true);
                    if (!QString(entry.target).isEmpty()) {
                        child->setText(1, tr("-> %1").arg(QString(entry.target)));
                        QFont italic = child->font(1);
                        italic.setItalic(true);
                        child->setFont(1, italic);
                    } else if (kind == QStringLiteral("file")) {
                        child->setText(1, QString::number(entry.size));
                    }
                    if (kind == QStringLiteral("dir")) {
                        // Placeholder child so the expand arrow shows up
                        // before the real listing is known.
                        auto *placeholder = new QTreeWidgetItem(child);
                        placeholder->setData(0, kFilesLoadedRole, false);
                    }
                }
                if (target != filesTree_->invisibleRootItem()) {
                    target->setData(0, kFilesLoadedRole, true);
                }
            });

    connect(containerService_, &ContainerService::actionFinished, this,
            [this](const QString &, bool, const QString &) {
                // Failures already surface through `ContainersPanel`'s own
                // banner (it connects to the same signal); this class has
                // nothing further to show, but stays subscribed so a
                // future per-tab status line has somewhere to hook in.
            });

    connect(containerService_, &ContainerService::layersReady, this,
            [this](const QString &nodeId, const ::rust::Vec<FfiLayer> &layers) {
                if (nodeId != nodeId_ || layersTable_ == nullptr) {
                    return;
                }
                layersTable_->setRowCount(static_cast<int>(layers.size()));
                int row = 0;
                for (const FfiLayer &layer : layers) {
                    layersTable_->setItem(row, 0, new QTableWidgetItem(QString(layer.id)));
                    layersTable_->setItem(
                      row, 1, new QTableWidgetItem(QLocale().formattedDataSize(layer.sizeBytes)));
                    layersTable_->setItem(row, 2, new QTableWidgetItem(QString(layer.created)));
                    auto *createdBy = new QTableWidgetItem(QString(layer.createdBy));
                    createdBy->setToolTip(QString(layer.createdBy));
                    layersTable_->setItem(row, 3, createdBy);
                    ++row;
                }
                layersTable_->resizeColumnsToContents();
            });

    connect(containerService_, &ContainerService::layerFsReady, this,
            [this](const QString &nodeId, const QString &lines) {
                if (nodeId != nodeId_ || layerFsTree_ == nullptr) {
                    return;
                }
                layerFsTree_->clear();
                QHash<QString, QTreeWidgetItem *> layerItems;
                for (const QString &line : lines.split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
                    const QStringList fields = line.split(QLatin1Char('\t'));
                    if (fields.size() != 4) {
                        continue;
                    }
                    const QString &layerId = fields.at(0);
                    QTreeWidgetItem *layerItem = layerItems.value(layerId);
                    if (layerItem == nullptr) {
                        layerItem = new QTreeWidgetItem(layerFsTree_, {layerId});
                        layerItems.insert(layerId, layerItem);
                    }
                    const qint64 size = fields.at(2).toLongLong();
                    const QString kind = fields.at(3);
                    const QString glyph = kind == QStringLiteral("deleted")   ? tr("- ")
                                          : kind == QStringLiteral("modified") ? tr("~ ")
                                                                               : tr("+ ");
                    new QTreeWidgetItem(layerItem, {glyph + fields.at(1),
                                                    kind == QStringLiteral("deleted")
                                                      ? QString()
                                                      : QLocale().formattedDataSize(size),
                                                    kind});
                }
                layerFsTree_->expandAll();
                layerFsTree_->resizeColumnToContents(0);
            });
}

void ContainerDetailArea::onSelectionChanged(const QString &nodeId, const QString &kind)
{
    nodeId_ = nodeId;
    kind_ = kind;
    isContainer_ = kind == QStringLiteral("container");
    if (isContainer_) {
        openOrReplaceLogTab();
    } else {
        closeLogTab();
    }
    if (processesPage_ != nullptr) {
        tabs_->removeTab(tabs_->indexOf(processesPage_));
        processesPage_->deleteLater();
        processesPage_ = nullptr;
        processesTable_ = nullptr;
    }
    if (filesPage_ != nullptr) {
        tabs_->removeTab(tabs_->indexOf(filesPage_));
        filesPage_->deleteLater();
        filesPage_ = nullptr;
        filesTree_ = nullptr;
    }
    if (layersPage_ != nullptr) {
        tabs_->removeTab(tabs_->indexOf(layersPage_));
        layersPage_->deleteLater();
        layersPage_ = nullptr;
        layersTable_ = nullptr;
        analyzeImageButton_ = nullptr;
        layerFsTree_ = nullptr;
    }
    if (labelsPage_ != nullptr) {
        tabs_->removeTab(tabs_->indexOf(labelsPage_));
        labelsPage_->deleteLater();
        labelsPage_ = nullptr;
        labelsTable_ = nullptr;
    }
    updateDashboardTab();
}

void ContainerDetailArea::clearSelection()
{
    nodeId_.clear();
    kind_.clear();
    isContainer_ = false;
    closeLogTab();
    updateDashboardTab();
}

void ContainerDetailArea::setGenericDashboardPage(QWidget *page)
{
    genericDashboardPage_ = page;
}

void ContainerDetailArea::updateDashboardTab()
{
    QWidget *wanted = genericDashboardPage_;
    if (kind_ == QStringLiteral("image")) {
        wanted = ensureImageDashboardPage();
        populateImageDashboard();
    } else if (kind_ == QStringLiteral("network")) {
        wanted = ensureNetworkDashboardPage();
        populateNetworkDashboard();
    } else if (kind_ == QStringLiteral("volume")) {
        wanted = ensureVolumeDashboardPage();
        populateVolumeDashboard();
    }
    if (tabs_->widget(0) == wanted) {
        return;
    }
    const QString title = tabs_->tabText(0);
    tabs_->removeTab(0);
    tabs_->insertTab(0, wanted, title.isEmpty() ? tr("Dashboard") : title);
    tabs_->tabBar()->setTabButton(0, QTabBar::RightSide, nullptr);
}

void ContainerDetailArea::fillContainersList(QListWidget *list, const QString &containers)
{
    list->clear();
    for (const QString &line : containers.split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        const int tab = line.indexOf(QLatin1Char('\t'));
        if (tab < 0) {
            continue;
        }
        auto *item = new QListWidgetItem(line.left(tab), list);
        item->setData(kContainerNodeIdRole, line.mid(tab + 1));
    }
}

QWidget *ContainerDetailArea::ensureImageDashboardPage()
{
    if (imageDashboardPage_ != nullptr) {
        return imageDashboardPage_;
    }
    imageDashboardPage_ = new QWidget(tabs_);
    auto *form = new QFormLayout(imageDashboardPage_);
    form->setLabelAlignment(Qt::AlignRight | Qt::AlignTop);
    imageDashName_ = readOnlyValue(imageDashboardPage_);
    imageDashId_ = readOnlyValue(imageDashboardPage_);
    imageDashSize_ = readOnlyValue(imageDashboardPage_);
    imageDashCreated_ = readOnlyValue(imageDashboardPage_);
    imageDashTags_ = new QListWidget(imageDashboardPage_);
    imageDashDigests_ = new QListWidget(imageDashboardPage_);
    imageDashContainers_ = new QListWidget(imageDashboardPage_);
    connect(imageDashContainers_, &QListWidget::itemClicked, this, [this](QListWidgetItem *item) {
        emit containerNodeRequested(item->data(kContainerNodeIdRole).toString());
    });
    form->addRow(tr("Name"), imageDashName_);
    form->addRow(tr("ID"), imageDashId_);
    form->addRow(tr("Size"), imageDashSize_);
    form->addRow(tr("Created"), imageDashCreated_);
    form->addRow(tr("Tags"), imageDashTags_);
    form->addRow(tr("Digests"), imageDashDigests_);
    form->addRow(tr("Containers using this image"), imageDashContainers_);
    return imageDashboardPage_;
}

void ContainerDetailArea::populateImageDashboard()
{
    const FfiImageDashboard dashboard = containerService_->imageDashboard(nodeId_);
    imageDashName_->setText(QString(dashboard.name));
    imageDashId_->setText(QString(dashboard.id));
    imageDashSize_->setText(dashboard.sizeBytes >= 0
                              ? QLocale().formattedDataSize(dashboard.sizeBytes)
                              : QString());
    imageDashCreated_->setText(QString(dashboard.created));
    imageDashTags_->clear();
    imageDashTags_->addItems(
      QString(dashboard.tags).split(QLatin1Char('\n'), Qt::SkipEmptyParts));
    imageDashDigests_->clear();
    imageDashDigests_->addItems(
      QString(dashboard.digests).split(QLatin1Char('\n'), Qt::SkipEmptyParts));
    fillContainersList(imageDashContainers_, dashboard.containers);
}

QWidget *ContainerDetailArea::ensureNetworkDashboardPage()
{
    if (networkDashboardPage_ != nullptr) {
        return networkDashboardPage_;
    }
    networkDashboardPage_ = new QWidget(tabs_);
    auto *form = new QFormLayout(networkDashboardPage_);
    form->setLabelAlignment(Qt::AlignRight | Qt::AlignTop);
    networkDashName_ = readOnlyValue(networkDashboardPage_);
    networkDashId_ = readOnlyValue(networkDashboardPage_);
    networkDashDriver_ = readOnlyValue(networkDashboardPage_);
    networkDashScope_ = readOnlyValue(networkDashboardPage_);
    networkDashSubnets_ = new QListWidget(networkDashboardPage_);
    networkDashContainers_ = new QListWidget(networkDashboardPage_);
    networkDashLabels_ = new QTableWidget(networkDashboardPage_);
    networkDashLabels_->setEditTriggers(QAbstractItemView::NoEditTriggers);
    networkDashLabels_->verticalHeader()->setVisible(false);
    networkDashLabels_->setColumnCount(2);
    networkDashLabels_->setHorizontalHeaderLabels({tr("Key"), tr("Value")});
    connect(networkDashContainers_, &QListWidget::itemClicked, this,
            [this](QListWidgetItem *item) {
                emit containerNodeRequested(item->data(kContainerNodeIdRole).toString());
            });
    form->addRow(tr("Name"), networkDashName_);
    form->addRow(tr("ID"), networkDashId_);
    form->addRow(tr("Driver"), networkDashDriver_);
    form->addRow(tr("Scope"), networkDashScope_);
    form->addRow(tr("Subnets / Gateways"), networkDashSubnets_);
    form->addRow(tr("Connected containers"), networkDashContainers_);
    form->addRow(tr("Labels"), networkDashLabels_);
    return networkDashboardPage_;
}

void ContainerDetailArea::populateNetworkDashboard()
{
    const FfiNetworkDashboard dashboard = containerService_->networkDashboard(nodeId_);
    networkDashName_->setText(QString(dashboard.name));
    networkDashId_->setText(QString(dashboard.id));
    networkDashDriver_->setText(QString(dashboard.driver));
    networkDashScope_->setText(QString(dashboard.scope));
    networkDashSubnets_->clear();
    for (const QString &line :
         QString(dashboard.subnets).split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        const int tab = line.indexOf(QLatin1Char('\t'));
        const QString gateway = tab >= 0 ? line.mid(tab + 1) : QString();
        const QString subnet = tab >= 0 ? line.left(tab) : line;
        networkDashSubnets_->addItem(gateway.isEmpty() ? subnet
                                                        : tr("%1 (gateway %2)").arg(subnet, gateway));
    }
    fillContainersList(networkDashContainers_, dashboard.containers);
    const QStringList labelLines =
      QString(dashboard.labels).split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    networkDashLabels_->setRowCount(labelLines.size());
    int row = 0;
    for (const QString &line : labelLines) {
        const int at = line.indexOf(QLatin1Char('='));
        networkDashLabels_->setItem(row, 0, new QTableWidgetItem(at >= 0 ? line.left(at) : line));
        networkDashLabels_->setItem(row, 1,
                                    new QTableWidgetItem(at >= 0 ? line.mid(at + 1) : QString()));
        ++row;
    }
    networkDashLabels_->resizeColumnsToContents();
}

QWidget *ContainerDetailArea::ensureVolumeDashboardPage()
{
    if (volumeDashboardPage_ != nullptr) {
        return volumeDashboardPage_;
    }
    volumeDashboardPage_ = new QWidget(tabs_);
    auto *form = new QFormLayout(volumeDashboardPage_);
    form->setLabelAlignment(Qt::AlignRight | Qt::AlignTop);
    volumeDashName_ = readOnlyValue(volumeDashboardPage_);
    volumeDashDriver_ = readOnlyValue(volumeDashboardPage_);
    volumeDashMountpoint_ = readOnlyValue(volumeDashboardPage_);
    volumeDashContainers_ = new QListWidget(volumeDashboardPage_);
    volumeDashLabels_ = new QTableWidget(volumeDashboardPage_);
    volumeDashLabels_->setEditTriggers(QAbstractItemView::NoEditTriggers);
    volumeDashLabels_->verticalHeader()->setVisible(false);
    volumeDashLabels_->setColumnCount(2);
    volumeDashLabels_->setHorizontalHeaderLabels({tr("Key"), tr("Value")});
    connect(volumeDashContainers_, &QListWidget::itemClicked, this,
            [this](QListWidgetItem *item) {
                emit containerNodeRequested(item->data(kContainerNodeIdRole).toString());
            });
    form->addRow(tr("Name"), volumeDashName_);
    form->addRow(tr("Driver"), volumeDashDriver_);
    form->addRow(tr("Mountpoint"), volumeDashMountpoint_);
    form->addRow(tr("Containers using this volume"), volumeDashContainers_);
    form->addRow(tr("Labels"), volumeDashLabels_);
    return volumeDashboardPage_;
}

void ContainerDetailArea::populateVolumeDashboard()
{
    const FfiVolumeDashboard dashboard = containerService_->volumeDashboard(nodeId_);
    volumeDashName_->setText(QString(dashboard.name));
    volumeDashDriver_->setText(QString(dashboard.driver));
    volumeDashMountpoint_->setText(QString(dashboard.mountpoint));
    fillContainersList(volumeDashContainers_, dashboard.containers);
    const QStringList labelLines =
      QString(dashboard.labels).split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    volumeDashLabels_->setRowCount(labelLines.size());
    int row = 0;
    for (const QString &line : labelLines) {
        const int at = line.indexOf(QLatin1Char('='));
        volumeDashLabels_->setItem(row, 0, new QTableWidgetItem(at >= 0 ? line.left(at) : line));
        volumeDashLabels_->setItem(row, 1,
                                   new QTableWidgetItem(at >= 0 ? line.mid(at + 1) : QString()));
        ++row;
    }
    volumeDashLabels_->resizeColumnsToContents();
}

void ContainerDetailArea::openOrReplaceLogTab()
{
    closeLogTab();

    logPage_ = new QWidget(tabs_);
    auto *layout = new QVBoxLayout(logPage_);
    layout->setContentsMargins(0, 0, 0, 0);
    auto *toolbar = new QHBoxLayout();
    auto *restartButton = new QToolButton(logPage_);
    restartButton->setText(tr("Restart"));
    toolbar->addWidget(restartButton);
    toolbar->addStretch(1);
    layout->addLayout(toolbar);

    const quint64 sessionId = terminalSupervisor_->newSession();
    const FfiCommand command = containerService_->logSessionCommand(nodeId_, kLogTail);
    terminalSupervisor_->setCommand(sessionId, command.program, command.args, command.env);
    logWidget_ = new TerminalWidget(terminalSupervisor_, sessionId, QString(), appSettings_,
                                    openAt_, logPage_);
    layout->addWidget(logWidget_, 1);
    connect(restartButton, &QToolButton::clicked, this, [this]() { openOrReplaceLogTab(); });

    const int logIndex = tabs_->insertTab(1, logPage_, tr("Log"));
    tabs_->tabBar()->setTabButton(logIndex, QTabBar::RightSide, nullptr);
}

void ContainerDetailArea::closeLogTab()
{
    if (logPage_ == nullptr) {
        return;
    }
    const quint64 sessionId = logWidget_->sessionId();
    tabs_->removeTab(tabs_->indexOf(logPage_));
    logPage_->deleteLater();
    logPage_ = nullptr;
    logWidget_ = nullptr;
    terminalSupervisor_->closeSession(sessionId);
}

void ContainerDetailArea::addTerminalTab(const FfiCommand &command, const QString &title)
{
    const quint64 sessionId = terminalSupervisor_->newSession();
    terminalSupervisor_->setCommand(sessionId, command.program, command.args, command.env);
    auto *widget =
      new TerminalWidget(terminalSupervisor_, sessionId, QString(), appSettings_, openAt_, tabs_);
    const int index = tabs_->addTab(widget, title);
    tabs_->setTabsClosable(true);
    tabs_->setCurrentIndex(index);
    widget->setFocus();
}

void ContainerDetailArea::closeTerminalTab(int index)
{
    if (auto *widget = qobject_cast<TerminalWidget *>(tabs_->widget(index))) {
        // The Dashboard/Log/Processes/Files tabs are never closed through
        // this path — only a `TerminalWidget` (Terminal/Exec/Attach) is,
        // matching `TerminalSessionsPanel::closeTab`'s own rule.
        const quint64 sessionId = widget->sessionId();
        tabs_->removeTab(index);
        widget->deleteLater();
        terminalSupervisor_->closeSession(sessionId);
    }
}

void ContainerDetailArea::openTerminal(bool asRoot)
{
    if (nodeId_.isEmpty()) {
        return;
    }
    const FfiCommand command = containerService_->terminalSessionCommand(nodeId_, asRoot);
    if (QString(command.program).isEmpty()) {
        return;
    }
    addTerminalTab(command, asRoot ? tr("Terminal (root)") : tr("Terminal"));
}

void ContainerDetailArea::openExecDialog(QWidget *dialogParent)
{
    if (nodeId_.isEmpty()) {
        return;
    }
    const QStringList history = containerService_->execHistory(nodeId_).split(
      QLatin1Char('\n'), Qt::SkipEmptyParts);
    QInputDialog dialog(dialogParent);
    dialog.setWindowTitle(tr("Exec Command"));
    dialog.setLabelText(tr("Command"));
    dialog.setComboBoxEditable(true);
    dialog.setComboBoxItems(history);
    dialog.setOption(QInputDialog::UseListViewForComboBoxItems, false);
    if (dialog.exec() != QDialog::Accepted) {
        return;
    }
    const QString commandText = dialog.textValue().trimmed();
    if (commandText.isEmpty()) {
        return;
    }
    const FfiCommand command = containerService_->execSessionCommand(nodeId_, commandText);
    if (QString(command.program).isEmpty()) {
        return;
    }
    addTerminalTab(command, tr("Exec: %1").arg(commandText));
}

void ContainerDetailArea::openAttach()
{
    if (nodeId_.isEmpty()) {
        return;
    }
    const FfiCommand command = containerService_->attachSessionCommand(nodeId_);
    if (QString(command.program).isEmpty()) {
        return;
    }
    addTerminalTab(command, tr("Attach"));
}

void ContainerDetailArea::showProcesses()
{
    if (nodeId_.isEmpty()) {
        return;
    }
    if (processesPage_ == nullptr) {
        processesPage_ = new QWidget(tabs_);
        auto *layout = new QVBoxLayout(processesPage_);
        auto *toolbar = new QHBoxLayout();
        auto *refreshButton = new QToolButton(processesPage_);
        refreshButton->setText(tr("Refresh"));
        connect(refreshButton, &QToolButton::clicked, this, [this]() { refreshProcesses(); });
        toolbar->addWidget(refreshButton);
        toolbar->addStretch(1);
        layout->addLayout(toolbar);
        processesTable_ = new QTableWidget(processesPage_);
        processesTable_->setEditTriggers(QAbstractItemView::NoEditTriggers);
        processesTable_->verticalHeader()->setVisible(false);
        layout->addWidget(processesTable_, 1);
        const int processesIndex = tabs_->addTab(processesPage_, tr("Processes"));
        tabs_->tabBar()->setTabButton(processesIndex, QTabBar::RightSide, nullptr);
    }
    tabs_->setCurrentWidget(processesPage_);
    refreshProcesses();
}

void ContainerDetailArea::refreshProcesses()
{
    containerService_->processes(nodeId_);
}

void ContainerDetailArea::showFiles()
{
    if (nodeId_.isEmpty()) {
        return;
    }
    ensureFilesTab();
    tabs_->setCurrentWidget(filesPage_);
    populateFilesRoot();
}

void ContainerDetailArea::ensureFilesTab()
{
    if (filesPage_ != nullptr) {
        return;
    }
    filesPage_ = new QWidget(tabs_);
    auto *layout = new QVBoxLayout(filesPage_);
    layout->setContentsMargins(0, 0, 0, 0);
    filesTree_ = new QTreeWidget(filesPage_);
    filesTree_->setColumnCount(2);
    filesTree_->setHeaderLabels({tr("Name"), tr("Size / Target")});
    filesTree_->setContextMenuPolicy(Qt::CustomContextMenu);
    layout->addWidget(filesTree_);
    const int filesIndex = tabs_->addTab(filesPage_, tr("Files"));
    tabs_->tabBar()->setTabButton(filesIndex, QTabBar::RightSide, nullptr);

    connect(filesTree_, &QTreeWidget::itemExpanded, this, [this](QTreeWidgetItem *item) {
        if (item->childCount() == 1 && !item->child(0)->data(0, kFilesLoadedRole).toBool()) {
            requestChildren(item, item->data(0, kFilesPathRole).toString());
        }
    });
    connect(filesTree_, &QTreeWidget::itemDoubleClicked, this,
            [this](QTreeWidgetItem *item, int) {
                if (item->data(0, kFilesKindRole).toString() == QStringLiteral("file")) {
                    containerService_->openFile(nodeId_, item->data(0, kFilesPathRole).toString());
                }
            });
    connect(filesTree_, &QTreeWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        QTreeWidgetItem *item = filesTree_->itemAt(pos);
        if (item == nullptr || item->data(0, kFilesKindRole).toString() != QStringLiteral("file")) {
            return;
        }
        QMenu menu(filesTree_);
        QAction *download = menu.addAction(tr("Download..."));
        if (menu.exec(filesTree_->viewport()->mapToGlobal(pos)) == download) {
            downloadPrompt(item->data(0, kFilesPathRole).toString(), filesTree_);
        }
    });
}

void ContainerDetailArea::populateFilesRoot()
{
    filesTree_->clear();
    containerService_->listFiles(nodeId_, QStringLiteral("/"));
}

void ContainerDetailArea::requestChildren(QTreeWidgetItem *dirItem, const QString &dir)
{
    dirItem->takeChildren();
    containerService_->listFiles(nodeId_, dir);
}

void ContainerDetailArea::downloadPrompt(const QString &path, QWidget *dialogParent)
{
    const QString fileName = path.section(QLatin1Char('/'), -1);
    const QString hostDest =
      QFileDialog::getSaveFileName(dialogParent, tr("Download"), fileName);
    if (hostDest.isEmpty()) {
        return;
    }
    containerService_->downloadFile(nodeId_, path, hostDest);
}

void ContainerDetailArea::showLayers()
{
    if (nodeId_.isEmpty() || kind_ != QStringLiteral("image")) {
        return;
    }
    if (layersPage_ == nullptr) {
        layersPage_ = new QWidget(tabs_);
        auto *layout = new QVBoxLayout(layersPage_);
        layout->setContentsMargins(0, 0, 0, 0);
        layersTable_ = new QTableWidget(layersPage_);
        layersTable_->setEditTriggers(QAbstractItemView::NoEditTriggers);
        layersTable_->verticalHeader()->setVisible(false);
        layersTable_->setColumnCount(4);
        layersTable_->setHorizontalHeaderLabels(
          {tr("Layer ID"), tr("Size"), tr("Created"), tr("Created By")});
        layout->addWidget(layersTable_);

        analyzeImageButton_ = new QPushButton(tr("Analyze image"), layersPage_);
        connect(analyzeImageButton_, &QPushButton::clicked, this,
                &ContainerDetailArea::triggerAnalyzeImage);
        layout->addWidget(analyzeImageButton_, 0, Qt::AlignLeft);

        layerFsTree_ = new QTreeWidget(layersPage_);
        layerFsTree_->setEditTriggers(QAbstractItemView::NoEditTriggers);
        layerFsTree_->setColumnCount(3);
        layerFsTree_->setHeaderLabels({tr("Path"), tr("Size"), tr("Kind")});
        layout->addWidget(layerFsTree_, 1);

        const int layersIndex = tabs_->addTab(layersPage_, tr("Layers"));
        tabs_->tabBar()->setTabButton(layersIndex, QTabBar::RightSide, nullptr);
    }
    tabs_->setCurrentWidget(layersPage_);
    containerService_->imageLayers(nodeId_);
}

void ContainerDetailArea::triggerAnalyzeImage()
{
    if (nodeId_.isEmpty() || kind_ != QStringLiteral("image") || layerFsTree_ == nullptr) {
        return;
    }
    layerFsTree_->clear();
    containerService_->analyzeImage(nodeId_);
}

void ContainerDetailArea::showLabels()
{
    if (nodeId_.isEmpty()) {
        return;
    }
    if (labelsPage_ == nullptr) {
        labelsPage_ = new QWidget(tabs_);
        auto *layout = new QVBoxLayout(labelsPage_);
        layout->setContentsMargins(0, 0, 0, 0);
        labelsTable_ = new QTableWidget(labelsPage_);
        labelsTable_->setEditTriggers(QAbstractItemView::NoEditTriggers);
        labelsTable_->verticalHeader()->setVisible(false);
        labelsTable_->setColumnCount(2);
        labelsTable_->setHorizontalHeaderLabels({tr("Key"), tr("Value")});
        layout->addWidget(labelsTable_);
        const int labelsIndex = tabs_->addTab(labelsPage_, tr("Labels"));
        tabs_->tabBar()->setTabButton(labelsIndex, QTabBar::RightSide, nullptr);
    }
    tabs_->setCurrentWidget(labelsPage_);

    ::rust::Vec<FfiKeyValue> labels;
    if (kind_ == QStringLiteral("image")) {
        labels = containerService_->imageLabels(nodeId_);
    } else if (kind_ == QStringLiteral("network")) {
        const FfiNetworkDashboard dashboard = containerService_->networkDashboard(nodeId_);
        for (const QString &line :
             QString(dashboard.labels).split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
            const int at = line.indexOf(QLatin1Char('='));
            if (at >= 0) {
                labels.push_back(FfiKeyValue{QString(line.left(at)), QString(line.mid(at + 1))});
            }
        }
    } else if (kind_ == QStringLiteral("volume")) {
        const FfiVolumeDashboard dashboard = containerService_->volumeDashboard(nodeId_);
        for (const QString &line :
             QString(dashboard.labels).split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
            const int at = line.indexOf(QLatin1Char('='));
            if (at >= 0) {
                labels.push_back(FfiKeyValue{QString(line.left(at)), QString(line.mid(at + 1))});
            }
        }
    }
    labelsTable_->setRowCount(static_cast<int>(labels.size()));
    int row = 0;
    for (const FfiKeyValue &entry : labels) {
        labelsTable_->setItem(row, 0, new QTableWidgetItem(QString(entry.key)));
        labelsTable_->setItem(row, 1, new QTableWidgetItem(QString(entry.value)));
        ++row;
    }
    labelsTable_->resizeColumnsToContents();
}

void ContainerDetailArea::openPullTab(const QString &connectionId, const QString &reference)
{
    const FfiCommand command = containerService_->pullSessionCommand(connectionId, reference);
    if (QString(command.program).isEmpty()) {
        return;
    }
    addTerminalTab(command, tr("Pull: %1").arg(reference));
}

void ContainerDetailArea::openTerminalCommandTab(const FfiCommand &command, const QString &title)
{
    addTerminalTab(command, title);
}

} // namespace ui_shell
