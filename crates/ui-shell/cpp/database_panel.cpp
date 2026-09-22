#include "database_panel.h"

#include "database_exchange_actions.h"
#include "db_object_dialogs.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "theme.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QActionGroup>
#include <QClipboard>
#include <QDebug>
#include <QEvent>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QInputDialog>
#include <QKeyEvent>
#include <QLabel>
#include <QLinearGradient>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QTimer>
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

// The row's own `kind` string (`bridge::database::tree::kind_id`'s
// vocabulary) to the `.a8` mask that paints it — the same lookup-table
// precedent `containers_panel.cpp`'s `maskForIconKey` sets, not a
// decision of this view's own. A kind with no dedicated glyph (`type`,
// `role`, `user`, `trigger`, `constraint`, `sequence`, `group`) falls back
// to the plain table glyph rather than growing the icon set for a row this
// dock's own actions matrix barely distinguishes today.
const char *maskForIconKey(const QString &kind)
{
    static const QHash<QString, const char *> masks = {
      {QStringLiteral("source"), ":/ui/icons/database/database.a8"},
      {QStringLiteral("catalog"), ":/ui/icons/database/schema.a8"},
      {QStringLiteral("schema"), ":/ui/icons/database/schema.a8"},
      {QStringLiteral("table"), ":/ui/icons/database/table.a8"},
      {QStringLiteral("view"), ":/ui/icons/database/view.a8"},
      {QStringLiteral("materialized-view"), ":/ui/icons/database/view.a8"},
      {QStringLiteral("column"), ":/ui/icons/database/column.a8"},
      {QStringLiteral("field"), ":/ui/icons/database/column.a8"},
      {QStringLiteral("index"), ":/ui/icons/database/index.a8"},
      {QStringLiteral("routine"), ":/ui/icons/database/routine.a8"},
      {QStringLiteral("collection"), ":/ui/icons/database/collection.a8"},
      // Keyspace/KeyNamespace (F7b): both are schema-like grouping nodes —
      // a Cassandra keyspace groups tables the way a folder does, a Redis
      // key namespace groups keys the same `:`-delimited way.
      {QStringLiteral("keyspace"), ":/ui/icons/database/keyspace.a8"},
      {QStringLiteral("key-namespace"), ":/ui/icons/database/keyspace.a8"},
      {QStringLiteral("key"), ":/ui/icons/database/redis-key.a8"},
      {QStringLiteral("folder-tables"), ":/ui/icons/database/folder.a8"},
      {QStringLiteral("folder-views"), ":/ui/icons/database/folder.a8"},
      {QStringLiteral("folder-materialized-views"), ":/ui/icons/database/folder.a8"},
      {QStringLiteral("folder-procedures"), ":/ui/icons/database/folder.a8"},
      {QStringLiteral("folder-functions"), ":/ui/icons/database/folder.a8"},
      {QStringLiteral("folder-sequences"), ":/ui/icons/database/folder.a8"},
      {QStringLiteral("folder-types"), ":/ui/icons/database/folder.a8"},
    };
    return masks.value(kind, ":/ui/icons/database/table.a8");
}

QIcon rowIcon(const QString &kind, bool primaryKey)
{
    // A primary-key column gets the dedicated key glyph in place of the
    // plain column one — `db_core::tree::TreeRow::primary_key`'s own
    // answer, never re-derived here.
    const char *mask = (kind == QLatin1String("column") && primaryKey)
      ? ":/ui/icons/database/key.a8"
      : maskForIconKey(kind);
    return maskIcon(mask, chromePaletteForTheme(activeThemeName()).textDim);
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

    // View options (FY.3): grouping, whether Procedures/Functions split,
    // and sort — three independent knobs on one menu rather than three
    // toolbar toggles, the same way a crowded toolbar sheds a rarely-hit
    // control anywhere else in this codebase.
    viewOptionsButton_ = new QToolButton(this);
    viewOptionsButton_->setText(tr("View options"));
    viewOptionsButton_->setPopupMode(QToolButton::InstantPopup);
    auto *viewOptionsMenu = new QMenu(viewOptionsButton_);

    auto *groupAction = viewOptionsMenu->addAction(tr("Group by Object Type"));
    groupAction->setCheckable(true);
    groupAction->setChecked(!flatGrouping_);
    auto *flatAction = viewOptionsMenu->addAction(tr("Flat"));
    flatAction->setCheckable(true);
    flatAction->setChecked(flatGrouping_);
    auto *groupModeGroup = new QActionGroup(viewOptionsMenu);
    groupModeGroup->setExclusive(true);
    groupModeGroup->addAction(groupAction);
    groupModeGroup->addAction(flatAction);
    connect(flatAction, &QAction::toggled, this, [this](bool flat) {
        flatGrouping_ = flat;
        databaseService_->setGrouping(flat);
    });

    viewOptionsMenu->addSeparator();
    auto *separateRoutinesAction = viewOptionsMenu->addAction(tr("Separate Procedures and Functions"));
    separateRoutinesAction->setCheckable(true);
    separateRoutinesAction->setChecked(separateRoutines_);
    connect(separateRoutinesAction, &QAction::toggled, this, [this](bool value) {
        separateRoutines_ = value;
        databaseService_->setSeparateRoutines(value);
    });

    viewOptionsMenu->addSeparator();
    auto *naturalSortAction = viewOptionsMenu->addAction(tr("Sort Natural"));
    naturalSortAction->setCheckable(true);
    naturalSortAction->setChecked(!alphabeticalSort_);
    auto *alphaSortAction = viewOptionsMenu->addAction(tr("Sort Alphabetical"));
    alphaSortAction->setCheckable(true);
    alphaSortAction->setChecked(alphabeticalSort_);
    auto *sortGroup = new QActionGroup(viewOptionsMenu);
    sortGroup->setExclusive(true);
    sortGroup->addAction(naturalSortAction);
    sortGroup->addAction(alphaSortAction);
    connect(alphaSortAction, &QAction::toggled, this, [this](bool alphabetical) {
        alphabeticalSort_ = alphabetical;
        databaseService_->setSort(alphabetical);
    });

    viewOptionsButton_->setMenu(viewOptionsMenu);
    toolbar->addWidget(viewOptionsButton_);

    filterEdit_ = new QLineEdit(this);
    filterEdit_->setPlaceholderText(tr("Filter (e.g. table:-payment_.*)"));
    filterEdit_->installEventFilter(this);
    connect(filterEdit_, &QLineEdit::textChanged, this,
            [this](const QString &text) { databaseService_->setFilter(text); });
    toolbar->addWidget(filterEdit_, 1);
    layout->addLayout(toolbar);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(2);
    tree_->setHeaderLabels({tr("Name"), tr("Detail")});
    // Without an explicit resize mode, Qt's default column widths (each
    // section's own `sizeHintForColumn`, computed against whatever size
    // this widget had before ADS ever placed it in a narrow right-area
    // dock) can add up to more than the dock ever actually shows — a
    // `QTreeWidget` happily lets a row's columns overflow its own
    // viewport, no horizontal scrollbar needed to notice, since nothing
    // before this reads a row's rect back. `ContainersPanel`'s tree sets
    // the same pair for the same reason (`Stretch` on the label column,
    // `ResizeToContents` on the rest), so total column width can never
    // exceed the viewport's own.
    tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    tree_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(tree_, &QTreeWidget::customContextMenuRequested, this, &DatabasePanel::showContextMenu);
    connect(tree_, &QTreeWidget::itemExpanded, this, &DatabasePanel::onItemExpanded);
    connect(tree_, &QTreeWidget::itemDoubleClicked, this, &DatabasePanel::onItemDoubleClicked);
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

bool DatabasePanel::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == filterEdit_ && event->type() == QEvent::KeyPress) {
        auto *keyEvent = static_cast<QKeyEvent *>(event);
        if (keyEvent->key() == Qt::Key_Escape && !filterEdit_->text().isEmpty()) {
            filterEdit_->clear();
            return true;
        }
    }
    return QWidget::eventFilter(watched, event);
}

void DatabasePanel::rebuildTree()
{
    QHash<QString, bool> expandedById;
    for (auto it = itemsById_.constBegin(); it != itemsById_.constEnd(); ++it) {
        expandedById.insert(it.key(), it.value()->isExpanded());
    }
    const QString selected = selectedNodeId();

    sourceColorById_.clear();
    for (const FfiDbSourceRow &source : databaseService_->sources()) {
        const QString color = QString(source.color);
        if (!color.isEmpty() && QColor::isValidColorName(color)) {
            sourceColorById_.insert(QString(source.id), QColor(color));
        }
    }

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
        item->setIcon(0, rowIcon(QString(row.kind), row.primaryKey));
        item->setData(0, Qt::UserRole, nodeId);
        // Colour tag (F2 polish): the source's own `color` (Data Source
        // dialog, F1.6) as a thin bar down the left edge of column 0 — a
        // left-anchored gradient on that column's background brush,
        // `containers_panel.cpp` having no per-row tint precedent of its
        // own to follow instead. `QTreeWidgetItem` offers no per-pixel
        // paint short of a delegate, and a whole-cell tint would fight
        // the selection highlight this dock already uses; a few percent
        // of the column's own width reads as a bar without either.
        const auto colorIt = sourceColorById_.constFind(QString(row.sourceId));
        if (colorIt != sourceColorById_.constEnd()) {
            QLinearGradient gradient(0, 0, 1, 0);
            gradient.setCoordinateMode(QGradient::ObjectMode);
            gradient.setColorAt(0.0, *colorIt);
            gradient.setColorAt(0.04, *colorIt);
            gradient.setColorAt(0.041, Qt::transparent);
            item->setBackground(0, QBrush(gradient));
        }
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
        info.kind = QString(row.kind);
        info.depth = row.depth;
        info.isSourceRoot = info.kind == QLatin1String("source");
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

    e2eMark(QStringLiteral("{\"ev\":\"database_tree_changed\",\"nodes\":%1,\"rows\":[%2]}")
              .arg(itemsById_.size())
              .arg(rowRectsJson().join(QLatin1Char(','))));
}

// See `ContainersPanel::rowRectsJson`'s own doc comment for why this is
// filtered the way it is (collapsed/scrolled-away rows report a rect an
// E2E click could never land on).
QStringList DatabasePanel::rowRectsJson() const
{
    QStringList rects;
    for (auto it = itemsById_.constBegin(); it != itemsById_.constEnd(); ++it) {
        QTreeWidgetItem *item = it.value();
        const QRect rect = tree_->visualItemRect(item);
        if (!rect.isValid() || rect.isEmpty() || !tree_->viewport()->rect().contains(rect)) {
            continue;
        }
        const QPoint origin = tree_->viewport()->mapToGlobal(rect.topLeft());
        const auto info = rowInfoById_.constFind(it.key());
        const QString kind = info == rowInfoById_.constEnd() ? QString() : info->kind;
        rects << QStringLiteral("{\"id\":%1,\"kind\":%2,\"rect\":[%3,%4,%5,%6]}")
                    .arg(e2eJson(it.key()), e2eJson(kind))
                    .arg(origin.x())
                    .arg(origin.y())
                    .arg(rect.width())
                    .arg(rect.height());
    }
    return rects;
}

void DatabasePanel::refreshE2eRects() const
{
    e2eMark(QStringLiteral("{\"ev\":\"database_tree_changed\",\"nodes\":%1,\"rows\":[%2]}")
              .arg(itemsById_.size())
              .arg(rowRectsJson().join(QLatin1Char(','))));
}

void DatabasePanel::onItemExpanded(QTreeWidgetItem *item)
{
    const QString nodeId = item->data(0, Qt::UserRole).toString();
    if (!nodeId.isEmpty()) {
        report(databaseService_->expand(nodeId));
    }
    // E2E only (`crates/app/tests/e2e_database.rs`): a synthetic grouping
    // folder (`kind` starting `folder-`, `db_core::tree::flatten`'s own
    // by-object-type grouping) has every child item already built —
    // expanding it is a plain `QTreeWidgetItem::setExpanded`, no backend
    // round trip and so no `rows_changed`/`database_tree_changed` of its
    // own to reveal the children's rects. A node that *does* still need
    // fetching harmlessly gets this same remark twice — once here, once
    // more from `rebuildTree` once that fetch's reply lands.
    QTimer::singleShot(0, this, [this]() { refreshE2eRects(); });
}

void DatabasePanel::onItemDoubleClicked(QTreeWidgetItem *item)
{
    const QString nodeId = item ? item->data(0, Qt::UserRole).toString() : QString();
    const auto found = rowInfoById_.constFind(nodeId);
    if (found == rowInfoById_.constEnd() || !found->isSourceRoot) {
        return;
    }
    report(databaseService_->connectSource(found->sourceId));
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

    // E2E only (`crates/app/tests/e2e_database.rs`): the tree's own
    // context menu has no keymap shortcut for most of its actions (Go to
    // DDL included) — the same problem `e2eMarkMenuActions` solves for
    // every other on-demand popup in this codebase.
    e2eMarkMenuActions(&menu, "database_context_menu_action");
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == nullptr) {
        return;
    }
    if (chosen == openConsole) {
        // `DatabaseService::openConsole` resolves/creates the console
        // file and reports it through `consoleFileReady`, already wired
        // straight to `EditorTabs::openFile` (`editor_tabs.cpp`) — this
        // call was the only piece F2.5's own handoff note left undone.
        report(databaseService_->openConsole(nodeId));
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
            // Renaming changes what the parent's own children list shows —
            // that scope, not the (about to be stale) node id itself, is
            // what needs refreshing on success.
            confirmPreviewAndRunAction(nodeId, QStringLiteral("rename:%1").arg(text), tr("Rename"),
                                       parentNodeId(nodeId, info.depth));
        }
    } else if (chosen == dropAction) {
        confirmPreviewAndRunAction(nodeId, QStringLiteral("drop"), tr("Drop"),
                                   parentNodeId(nodeId, info.depth));
    } else if (chosen == truncateAction) {
        confirmPreviewAndRunAction(nodeId, QStringLiteral("truncate"), tr("Truncate"), nodeId);
    } else if (chosen == commentAction) {
        bool ok = false;
        const QString text =
          QInputDialog::getMultiLineText(this, tr("Comment"), tr("Comment:"), QString(), &ok);
        if (ok) {
            confirmPreviewAndRunAction(nodeId, QStringLiteral("comment:%1").arg(text), tr("Comment"),
                                       nodeId);
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

namespace {
QString connectionStateName(FfiDbConnectionState state)
{
    switch (state) {
    case FfiDbConnectionState::Disconnected:
        return QStringLiteral("disconnected");
    case FfiDbConnectionState::Connecting:
        return QStringLiteral("connecting");
    case FfiDbConnectionState::Connected:
        return QStringLiteral("connected");
    case FfiDbConnectionState::Error:
        return QStringLiteral("error");
    }
    return QString();
}
} // namespace

void DatabasePanel::onConnectionStateChanged(const QString &id, FfiDbConnectionState state,
                                             const QString &message)
{
    if (state == FfiDbConnectionState::Error && !message.isEmpty()) {
        statusLabel_->setText(message);
    } else {
        statusLabel_->clear();
    }
    // E2E only (`crates/app/tests/e2e_database.rs`): the tree's own
    // `database_tree_changed` rows carry no connection state, so a flow
    // that double-clicks a `source` row to connect it needs its own
    // marker to wait on — the same `containers_connection_state` shape
    // `ContainersPanel` already reports.
    e2eMark(QStringLiteral("{\"ev\":\"database_connection_state\",\"id\":%1,\"state\":%2}")
              .arg(e2eJson(id), e2eJson(connectionStateName(state))));
}

void DatabasePanel::onActionFinished(bool ok, const QString &message)
{
    if (!ok) {
        statusLabel_->setText(message);
        ddlRefreshNodeId_.clear();
        return;
    }
    statusLabel_->clear();
    // Refresh the scope a just-run mutating action affected — F4.4's own
    // object-DDL dialogs set this right before dispatch;
    // `confirmPreviewAndRunAction` does the same for rename/drop/
    // truncate/comment (F2 polish, generalised from F4.4's precedent
    // rather than each path growing its own refresh call).
    if (!ddlRefreshNodeId_.isEmpty()) {
        report(databaseService_->refresh(ddlRefreshNodeId_, false));
        ddlRefreshNodeId_.clear();
    }
}

void DatabasePanel::confirmPreviewAndRunAction(const QString &nodeId, const QString &actionId,
                                               const QString &title, const QString &refreshScopeId)
{
    const FfiResult preview = databaseService_->actionPreview(nodeId, actionId);
    if (preview.code != 0) {
        statusLabel_->setText(QString(preview.message));
        return;
    }
    if (QMessageBox::question(this, title,
                              tr("Run this statement?\n\n%1").arg(QString(preview.message)))
        != QMessageBox::Yes) {
        return;
    }
    const FfiResult result = databaseService_->runAction(nodeId, actionId);
    // Only arm the refresh when dispatch itself succeeded — a synchronous
    // failure (bad node id, not connected) never reaches `actionFinished`
    // at all, and a stale armed id would refresh the wrong node the next
    // time some *other* action finishes.
    ddlRefreshNodeId_ = result.code == 0 ? refreshScopeId : QString();
    report(result);
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
    // Tabbed into the existing right area (Structure/AI Chat) rather than
    // splitting a second right column — same reason as `databaseResults`.
    docks->registerDock(QStringLiteral("database"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("database"));
    // E2E only — see `ContainersPanel::refreshE2eRects`'s own doc comment
    // for why this waits a turn of the event loop past `visibilityChanged`.
    QObject::connect(dock, &ads::CDockWidget::visibilityChanged, panel, [panel](bool visible) {
        if (visible) {
            QTimer::singleShot(0, panel, [panel]() { panel->refreshE2eRects(); });
        }
    });
    return panel;
}

} // namespace ui_shell
