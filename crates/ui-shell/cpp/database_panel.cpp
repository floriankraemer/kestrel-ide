#include "database_panel.h"

#include "database_exchange_actions.h"
#include "db_object_dialogs.h"
#include "dock_layout.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QClipboard>
#include <QDebug>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QInputDialog>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QStyle>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

// The parent row's own composite node id, derived from `nodeId`'s own
// shape (`"<source>:<path>"`, `depth` counting every rendered row
// including a grouping folder) — `FfiDbTreeRow` carries no parent id of
// its own (database-tools-plan F2.2's rows are a flat, `db_core::tree::
// flatten`-ordered list, not a parent-linked one), so the view derives
// it here rather than `db_core` growing a field only this lookup needs.
QString parentNodeId(const QString &nodeId, int depth)
{
    if (depth <= 0) {
        return QString();
    }
    const int colon = nodeId.indexOf(QLatin1Char(':'));
    if (colon < 0) {
        return QString();
    }
    const QString sourceId = nodeId.left(colon);
    if (depth == 1) {
        return sourceId + QStringLiteral(":$root");
    }
    const QString path = nodeId.mid(colon + 1);
    const int lastSlash = path.lastIndexOf(QLatin1Char('/'));
    return sourceId + QStringLiteral(":") + path.left(lastSlash);
}

QIcon rowIcon(const QWidget *widget, const QString &kind)
{
    // ponytail: standard `QStyle` icons in place of the F2.5 spec's own
    // `.a8` mask set under `resources/icons/database/` — a real icon per
    // kind (database/schema/table/view/column/key/index/routine/folder)
    // is a follow-up, not deferred for a technical reason.
    const QStyle *style = widget->style();
    if (kind == QLatin1String("source")) {
        return style->standardIcon(QStyle::SP_DriveNetIcon);
    }
    if (kind.startsWith(QLatin1String("folder-"))) {
        return style->standardIcon(QStyle::SP_DirIcon);
    }
    if (kind == QLatin1String("column") || kind == QLatin1String("field")) {
        return style->standardIcon(QStyle::SP_FileIcon);
    }
    // Keyspace/KeyNamespace (F7b): both are schema-like grouping nodes —
    // a Cassandra keyspace groups tables the way a folder does, a Redis
    // key namespace groups keys the same `:`-delimited way. Neither is
    // backend-reported the way a `Schema` is, but visually they play the
    // same "directory of objects" role.
    if (kind == QLatin1String("keyspace") || kind == QLatin1String("key-namespace")) {
        return style->standardIcon(QStyle::SP_DirIcon);
    }
    if (kind == QLatin1String("key")) {
        return style->standardIcon(QStyle::SP_FileIcon);
    }
    return style->standardIcon(QStyle::SP_FileDialogDetailedView);
}

} // namespace

DatabasePanel::DatabasePanel(DatabaseService *databaseService, ExchangeService *exchangeService,
                             DocumentManager *documentManager, QWidget *parent)
  : QWidget(parent)
  , databaseService_(databaseService)
  , exchangeService_(exchangeService)
  , documentManager_(documentManager)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(4, 4, 4, 4);

    auto *toolbar = new QHBoxLayout();
    addButton_ = new QToolButton(this);
    addButton_->setText(tr("New data source…"));
    connect(addButton_, &QToolButton::clicked, this, [this]() {
        if (openSettings_) {
            openSettings_();
        } else {
            qWarning() << "Database: no Settings handler wired yet";
        }
    });
    toolbar->addWidget(addButton_);

    refreshButton_ = new QToolButton(this);
    refreshButton_->setText(tr("Refresh"));
    connect(refreshButton_, &QToolButton::clicked, this, [this]() {
        const QString id = selectedNodeId();
        if (!id.isEmpty()) {
            report(databaseService_->refresh(id, false));
        }
    });
    toolbar->addWidget(refreshButton_);

    forceRefreshButton_ = new QToolButton(this);
    forceRefreshButton_->setText(tr("Force Refresh"));
    connect(forceRefreshButton_, &QToolButton::clicked, this, [this]() {
        const QString id = selectedNodeId();
        if (!id.isEmpty()) {
            report(databaseService_->refresh(id, true));
        }
    });
    toolbar->addWidget(forceRefreshButton_);

    goToDdlButton_ = new QToolButton(this);
    goToDdlButton_->setText(tr("Go to DDL"));
    connect(goToDdlButton_, &QToolButton::clicked, this, [this]() {
        const QString id = selectedNodeId();
        if (!id.isEmpty()) {
            report(databaseService_->goToDdl(id));
        }
    });
    toolbar->addWidget(goToDdlButton_);

    groupingButton_ = new QToolButton(this);
    groupingButton_->setText(tr("Flat"));
    groupingButton_->setCheckable(true);
    connect(groupingButton_, &QToolButton::toggled, this,
            [this](bool flat) { databaseService_->setGrouping(flat); });
    toolbar->addWidget(groupingButton_);

    filterEdit_ = new QLineEdit(this);
    filterEdit_->setPlaceholderText(tr("Filter (e.g. table:-payment_.*)"));
    connect(filterEdit_, &QLineEdit::textChanged, this,
            [this](const QString &text) { databaseService_->setFilter(text); });
    toolbar->addWidget(filterEdit_, 1);
    layout->addLayout(toolbar);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(2);
    tree_->setHeaderLabels({tr("Name"), tr("Detail")});
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(tree_, &QTreeWidget::customContextMenuRequested, this, &DatabasePanel::showContextMenu);
    connect(tree_, &QTreeWidget::itemExpanded, this, &DatabasePanel::onItemExpanded);
    layout->addWidget(tree_, 1);

    statusLabel_ = new QLabel(this);
    statusLabel_->setWordWrap(true);
    layout->addWidget(statusLabel_);

    connect(databaseService_, &DatabaseService::rowsChanged, this, &DatabasePanel::rebuildTree);
    connect(databaseService_, &DatabaseService::connectionStateChanged, this,
            &DatabasePanel::onConnectionStateChanged);
    connect(databaseService_, &DatabaseService::actionFinished, this,
            &DatabasePanel::onActionFinished);

    rebuildTree();
}

void DatabasePanel::setOpenSettingsHandler(OpenSettings handler)
{
    openSettings_ = std::move(handler);
}

void DatabasePanel::rebuildTree()
{
    QHash<QString, bool> expandedById;
    for (auto it = itemsById_.constBegin(); it != itemsById_.constEnd(); ++it) {
        expandedById.insert(it.key(), it.value()->isExpanded());
    }
    const QString selected = selectedNodeId();

    tree_->blockSignals(true);
    tree_->clear();
    itemsById_.clear();
    rowInfoById_.clear();

    const ::rust::Vec<FfiDbTreeRow> rows = databaseService_->rows();
    for (const FfiDbTreeRow &row : rows) {
        const QString nodeId = QString(row.nodeId);
        const QString parentId = parentNodeId(nodeId, row.depth);
        QTreeWidgetItem *parentItem = parentId.isEmpty() ? nullptr : itemsById_.value(parentId);
        auto *item = parentItem ? new QTreeWidgetItem(parentItem) : new QTreeWidgetItem(tree_);
        item->setText(0, QString(row.label));
        item->setText(1, QString(row.detail));
        item->setIcon(0, rowIcon(this, QString(row.kind)));
        item->setData(0, Qt::UserRole, nodeId);
        // A node the bridge already fetched shows its children right
        // away; one that has not been asked for yet still reports
        // `childIndicatorPolicy` so the user can expand it, which is what
        // triggers `onItemExpanded`'s `expand()` call.
        if (row.expandable) {
            item->setChildIndicatorPolicy(row.loaded ? QTreeWidgetItem::DontShowIndicatorWhenChildless
                                                     : QTreeWidgetItem::ShowIndicator);
        }
        item->setExpanded(expandedById.value(nodeId, row.depth == 0));
        itemsById_.insert(nodeId, item);

        RowInfo info;
        info.label = QString(row.label);
        info.sourceId = QString(row.sourceId);
        info.isSourceRoot = QString(row.kind) == QLatin1String("source");
        info.canOpenConsole = row.actions.canOpenConsole;
        info.canEditData = row.actions.canEditData;
        info.canGoToDdl = row.actions.canGoToDdl;
        info.canCopyName = row.actions.canCopyName;
        info.canRefresh = row.actions.canRefresh;
        info.canRename = row.actions.canRename;
        info.canDrop = row.actions.canDrop;
        info.canTruncate = row.actions.canTruncate;
        info.canComment = row.actions.canComment;
        info.canErDiagram = row.actions.canErDiagram;
        info.canExportData = row.actions.canExportData;
        info.canImportData = row.actions.canImportData;
        info.canCopyTable = row.actions.canCopyTable;
        info.canDump = row.actions.canDump;
        info.canCompare = row.actions.canCompare;
        info.canDeleteKey = row.actions.canDeleteKey;
        info.canTtlSet = row.actions.canTtlSet;
        info.canCreateTable = row.actions.canCreateTable;
        info.canModifyTable = row.actions.canModifyTable;
        info.canAddColumn = row.actions.canAddColumn;
        info.canCreateIndex = row.actions.canCreateIndex;
        info.canCreateUser = row.actions.canCreateUser;
        rowInfoById_.insert(nodeId, info);
    }

    if (!selected.isEmpty()) {
        if (QTreeWidgetItem *item = itemsById_.value(selected)) {
            tree_->setCurrentItem(item);
        }
    }
    tree_->blockSignals(false);
}

void DatabasePanel::onItemExpanded(QTreeWidgetItem *item)
{
    const QString nodeId = item->data(0, Qt::UserRole).toString();
    if (!nodeId.isEmpty()) {
        report(databaseService_->expand(nodeId));
    }
}

QString DatabasePanel::selectedNodeId() const
{
    QTreeWidgetItem *item = tree_->currentItem();
    return item ? item->data(0, Qt::UserRole).toString() : QString();
}

void DatabasePanel::showContextMenu(const QPoint &pos)
{
    QTreeWidgetItem *item = tree_->itemAt(pos);
    if (item == nullptr) {
        return;
    }
    tree_->setCurrentItem(item);
    const QString nodeId = item->data(0, Qt::UserRole).toString();
    const auto found = rowInfoById_.constFind(nodeId);
    if (found == rowInfoById_.constEnd()) {
        return;
    }
    const RowInfo &info = *found;

    QMenu menu(this);
    QAction *openConsole = info.canOpenConsole ? menu.addAction(tr("Open Console")) : nullptr;
    QAction *editData = info.canEditData ? menu.addAction(tr("Edit Data…")) : nullptr;
    QAction *goToDdl = info.canGoToDdl ? menu.addAction(tr("Go to DDL")) : nullptr;
    QAction *copyName = info.canCopyName ? menu.addAction(tr("Copy Name")) : nullptr;
    QAction *refreshAction = info.canRefresh ? menu.addAction(tr("Refresh")) : nullptr;
    if (info.canRename || info.canDrop || info.canTruncate || info.canComment) {
        menu.addSeparator();
    }
    QAction *rename = info.canRename ? menu.addAction(tr("Rename…")) : nullptr;
    QAction *dropAction = info.canDrop ? menu.addAction(tr("Drop…")) : nullptr;
    QAction *truncateAction = info.canTruncate ? menu.addAction(tr("Truncate…")) : nullptr;
    QAction *commentAction = info.canComment ? menu.addAction(tr("Comment…")) : nullptr;
    if (info.canExportData || info.canImportData || info.canCopyTable || info.canErDiagram
        || info.canDump || info.canCompare) {
        menu.addSeparator();
    }
    QAction *exportData = info.canExportData ? menu.addAction(tr("Export Data…")) : nullptr;
    QAction *importData = info.canImportData ? menu.addAction(tr("Import Data…")) : nullptr;
    QAction *copyTableTo = info.canCopyTable ? menu.addAction(tr("Copy Table to…")) : nullptr;
    QAction *erDiagram = info.canErDiagram ? menu.addAction(tr("ER Diagram")) : nullptr;
    // No `ActionSet::COMPARE_DATA` bit of its own: exactly the same
    // "this row is a real table" condition `COPY_TABLE` already encodes,
    // reused rather than adding a second flag that would always equal
    // the first.
    QAction *compareData = info.canCopyTable ? menu.addAction(tr("Compare Data with…")) : nullptr;
    QAction *dumpAction = info.canDump ? menu.addAction(tr("Dump…")) : nullptr;
    // Restore shares Dump's own `canDump` gate (F6c): both write to this
    // exact source, one direction each, and no backend distinguishes
    // "can be dumped" from "can be restored into" the way it does for,
    // say, comparison (`canCompare`) versus data copy (`canCopyTable`).
    QAction *restoreAction = info.canDump ? menu.addAction(tr("Restore…")) : nullptr;
    QAction *compareStructure = info.canCompare ? menu.addAction(tr("Compare Structure with…")) : nullptr;
    if (info.canDeleteKey || info.canTtlSet) {
        menu.addSeparator();
    }
    QAction *deleteKey = info.canDeleteKey ? menu.addAction(tr("Delete Key…")) : nullptr;
    QAction *setTtl = info.canTtlSet ? menu.addAction(tr("Set TTL…")) : nullptr;
    if (info.canCreateTable || info.canModifyTable || info.canAddColumn || info.canCreateIndex
        || info.canCreateUser) {
        menu.addSeparator();
    }
    QAction *createTable = info.canCreateTable ? menu.addAction(tr("Create Table…")) : nullptr;
    QAction *createUser = info.canCreateUser ? menu.addAction(tr("Create User…")) : nullptr;
    QAction *modifyTable = info.canModifyTable ? menu.addAction(tr("Modify Column…")) : nullptr;
    QAction *addColumn = info.canAddColumn ? menu.addAction(tr("Add Column…")) : nullptr;
    QAction *dropColumn = info.canModifyTable ? menu.addAction(tr("Drop Column…")) : nullptr;
    QAction *createIndex = info.canCreateIndex ? menu.addAction(tr("Create Index…")) : nullptr;
    if (menu.actions().isEmpty()) {
        return;
    }

    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == nullptr) {
        return;
    }
    if (chosen == openConsole) {
        // F3 wires this to the console dock; left as a stub per the
        // database-tools-plan F2.5 handoff note.
        qWarning() << "Database: Open Console is not wired yet (F3)";
    } else if (chosen == editData) {
        qWarning() << "Database: Edit Data is not wired yet (F4)";
    } else if (chosen == goToDdl) {
        report(databaseService_->goToDdl(nodeId));
    } else if (chosen == copyName) {
        QGuiApplication::clipboard()->setText(info.label);
    } else if (chosen == refreshAction) {
        report(databaseService_->refresh(nodeId, false));
    } else if (chosen == rename) {
        bool ok = false;
        const QString text = QInputDialog::getText(this, tr("Rename"), tr("New name:"),
                                                    QLineEdit::Normal, info.label, &ok);
        if (ok && !text.isEmpty()) {
            report(databaseService_->runAction(nodeId, QStringLiteral("rename:%1").arg(text)));
        }
    } else if (chosen == dropAction) {
        if (QMessageBox::question(this, tr("Drop"),
                                  tr("Drop '%1'? This cannot be undone.").arg(info.label))
            == QMessageBox::Yes) {
            report(databaseService_->runAction(nodeId, QStringLiteral("drop")));
        }
    } else if (chosen == truncateAction) {
        if (QMessageBox::question(this, tr("Truncate"),
                                  tr("Truncate '%1'? Every row will be deleted.").arg(info.label))
            == QMessageBox::Yes) {
            report(databaseService_->runAction(nodeId, QStringLiteral("truncate")));
        }
    } else if (chosen == commentAction) {
        bool ok = false;
        const QString text =
          QInputDialog::getMultiLineText(this, tr("Comment"), tr("Comment:"), QString(), &ok);
        if (ok) {
            report(databaseService_->runAction(nodeId, QStringLiteral("comment:%1").arg(text)));
        }
    } else if (chosen == exportData) {
        showExportDataDialog(this, exchangeService_, info.sourceId, info.label);
    } else if (chosen == importData) {
        showImportDataDialog(this, exchangeService_, info.sourceId, info.label);
    } else if (chosen == copyTableTo) {
        showCopyTableDialog(this, exchangeService_, info.sourceId, info.label, databaseService_->sources());
    } else if (chosen == erDiagram) {
        // A table row's own name scopes the diagram to it and its FK
        // neighbours; the data source's root row (no table name of its
        // own) diagrams the whole schema — `ExchangeService::
        // erDiagramMermaid`'s own `tableScope` contract.
        const QString scope = info.isSourceRoot ? QString() : info.label;
        showErDiagramDialog(this, exchangeService_, documentManager_, info.sourceId, scope);
    } else if (chosen == compareData) {
        showDataCompareDialog(this, exchangeService_, documentManager_, info.sourceId, info.label,
                              databaseService_->sources());
    } else if (chosen == dumpAction) {
        showDumpDialog(this, exchangeService_, info.sourceId);
    } else if (chosen == restoreAction) {
        showRestoreDialog(this, exchangeService_, info.sourceId);
    } else if (chosen == compareStructure) {
        showSchemaCompareDialog(this, exchangeService_, documentManager_, info.sourceId,
                                databaseService_->sources());
    } else if (chosen == deleteKey) {
        if (QMessageBox::question(this, tr("Delete Key"),
                                  tr("Delete key '%1'? This cannot be undone.").arg(info.label))
            == QMessageBox::Yes) {
            report(databaseService_->runAction(nodeId, QStringLiteral("delete-key")));
        }
    } else if (chosen == setTtl) {
        bool ok = false;
        const int seconds = QInputDialog::getInt(this, tr("Set TTL"),
                                                  tr("Expire '%1' after how many seconds:")
                                                    .arg(info.label),
                                                  60, 1, 2147483647, 1, &ok);
        if (ok) {
            report(databaseService_->runAction(nodeId,
                                               QStringLiteral("ttl:%1").arg(seconds)));
        }
    } else if (chosen == createTable) {
        ddlRefreshNodeId_ = showCreateTableDialog(this, databaseService_, nodeId) ? nodeId
                                                                                 : QString();
    } else if (chosen == createUser) {
        ddlRefreshNodeId_ =
          showCreateUserDialog(this, databaseService_, nodeId) ? nodeId : QString();
    } else if (chosen == modifyTable) {
        ddlRefreshNodeId_ =
          showModifyColumnDialog(this, databaseService_, nodeId) ? nodeId : QString();
    } else if (chosen == addColumn) {
        ddlRefreshNodeId_ =
          showAddColumnDialog(this, databaseService_, nodeId) ? nodeId : QString();
    } else if (chosen == dropColumn) {
        ddlRefreshNodeId_ =
          showDropColumnDialog(this, databaseService_, nodeId) ? nodeId : QString();
    } else if (chosen == createIndex) {
        ddlRefreshNodeId_ =
          showCreateIndexDialog(this, databaseService_, nodeId) ? nodeId : QString();
    }
}

void DatabasePanel::onConnectionStateChanged(const QString &id, FfiDbConnectionState state,
                                             const QString &message)
{
    Q_UNUSED(id);
    if (state == FfiDbConnectionState::Error && !message.isEmpty()) {
        statusLabel_->setText(message);
    } else {
        statusLabel_->clear();
    }
}

void DatabasePanel::onActionFinished(bool ok, const QString &message)
{
    if (!ok) {
        statusLabel_->setText(message);
        ddlRefreshNodeId_.clear();
        return;
    }
    statusLabel_->clear();
    // F4.4: refresh the scope a just-run object-DDL dialog affected —
    // `showContextMenu`'s own dispatch sets this right before the dialog
    // ran; every other `runAction` path (rename/drop/…) leaves it empty,
    // unchanged (`database-tools.md` §11's own tracked debt on those).
    if (!ddlRefreshNodeId_.isEmpty()) {
        report(databaseService_->refresh(ddlRefreshNodeId_, false));
        ddlRefreshNodeId_.clear();
    }
}

void DatabasePanel::report(const FfiResult &result)
{
    if (result.code == 0) {
        return;
    }
    statusLabel_->setText(QString(result.message));
}

DatabasePanel *buildDatabaseDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                 ads::CDockAreaWidget *relativeTo, DatabaseService *databaseService,
                                 ExchangeService *exchangeService, DocumentManager *documentManager)
{
    auto *panel = new DatabasePanel(databaseService, exchangeService, documentManager, dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Database"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("database"), dock, ads::RightDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("database"));
    return panel;
}

} // namespace ui_shell
