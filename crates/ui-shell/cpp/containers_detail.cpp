#include "containers_detail.h"

#include "terminal_widget.h"

#include <QAbstractItemView>
#include <QFileDialog>
#include <QFont>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QInputDialog>
#include <QMenu>
#include <QPoint>
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
}

void ContainerDetailArea::onSelectionChanged(const QString &nodeId, const QString &kind)
{
    nodeId_ = nodeId;
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
}

void ContainerDetailArea::clearSelection()
{
    nodeId_.clear();
    isContainer_ = false;
    closeLogTab();
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

} // namespace ui_shell
