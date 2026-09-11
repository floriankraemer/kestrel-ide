#include "structure_panel.h"

#include "editor_tabs.h"
#include "symbol_icon.h"
#include "symbol_kind_label.h"

#include <QAction>
#include <QComboBox>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QMenu>
#include <QPoint>
#include <QString>
#include <QStringList>
#include <QToolButton>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QVBoxLayout>
#include <QVariant>
#include <QVector>

namespace ui_shell {

namespace {

// Non-recursive per-level sort, applied down the whole tree (Task 4b).
// `QTreeWidget::sortByColumn`/`setSortingEnabled` sort every level by the
// same comparator, which would alphabetize the category group items
// themselves ("Constants" before "Fields" before "Methods"...) and
// destroy their fixed order — so each item's direct children are sorted
// in isolation instead, top level included via `tree_->invisibleRootItem()`.
void sortEachLevelAlphabetically(QTreeWidgetItem *item)
{
    item->sortChildren(0, Qt::AscendingOrder);
    for (int i = 0; i < item->childCount(); ++i) {
        sortEachLevelAlphabetically(item->child(i));
    }
}

} // namespace

StructurePanel::StructurePanel(DocumentManager *docManager, SearchModel *searchModel,
                               EditorTabs *editorTabs,
                               std::function<void(const QString &)> onFindUsagesRequested,
                               QWidget *parent)
  : QWidget(parent)
  , docManager_(docManager)
  , searchModel_(searchModel)
  , editorTabs_(editorTabs)
  , onFindUsagesRequested_(std::move(onFindUsagesRequested))
{
    modeCombo_ = new QComboBox(this);
    modeCombo_->addItem(tr("Current File"));
    modeCombo_->addItem(tr("Project"));

    // PhpStorm-style toggle: off (default) shows definition order, on
    // sorts each tree level alphabetically by its item text — the symbol
    // name is the leading token so text sort already reads as name sort,
    // no per-item comparator needed. Per-level (not `setSortingEnabled`,
    // which sorts every level, category groups included) so the fixed
    // category order survives the toggle — see sortEachLevelAlphabetically.
    // Off has no saved "natural order" snapshot to restore, so it just
    // re-issues whichever tier is active.
    sortButton_ = new QToolButton(this);
    sortButton_->setText(tr("A→Z"));
    sortButton_->setToolTip(tr("Sort Alphabetically"));
    sortButton_->setCheckable(true);
    connect(sortButton_, &QToolButton::toggled, this, [this](bool on) {
        if (on) {
            sortEachLevelAlphabetically(tree_->invisibleRootItem());
        } else if (projectMode_) {
            refreshProject();
        } else {
            refresh(editorTabs_->currentTabId());
        }
    });

    tree_ = new QTreeWidget(this);
    tree_->setHeaderHidden(true);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);
    auto *topLayout = new QHBoxLayout();
    topLayout->addWidget(modeCombo_, 1);
    topLayout->addWidget(sortButton_);
    auto *layout = new QVBoxLayout(this);
    layout->addLayout(topLayout);
    layout->addWidget(tree_);

    connect(tree_, &QTreeWidget::itemDoubleClicked, this, &StructurePanel::onItemDoubleClicked);
    connect(tree_, &QTreeWidget::customContextMenuRequested, this,
            &StructurePanel::onContextMenuRequested);
    connect(modeCombo_, &QComboBox::currentIndexChanged, this, [this](int index) {
        projectMode_ = (index == 1);
        if (projectMode_) {
            refreshProject();
        } else {
            refresh(editorTabs_->currentTabId());
        }
    });

    connect(searchModel_, &SearchModel::projectSymbolFound, this, &StructurePanel::addProjectSymbol);
    connect(searchModel_, &SearchModel::projectSymbolsFinished, this,
            [this]() { tree_->expandAll(); });
    connect(searchModel_, &SearchModel::projectSymbolsFailed, this,
            [this](const QString &message) {
                tree_->clear();
                new QTreeWidgetItem(tree_, QStringList { tr("Project symbols unavailable: %1").arg(message) });
            });
}

void StructurePanel::refresh(quint64 tabId)
{
    if (projectMode_) {
        return;
    }
    tree_->clear();
    if (tabId == 0) {
        return;
    }
    const rust::Vec<FfiSymbolNode> symbols = docManager_->tabOutline(tabId);

    // `depth` reconstructs the tree from this pre-order-flattened list
    // (see FfiSymbolNode's doc comment): `parents[d]` is the open
    // QTreeWidgetItem at depth d-1 that the next depth-d item attaches
    // under, or nullptr for a root (attaches to the QTreeWidget itself).
    // A depth-0 symbol attaches straight to the tree — it has no class/
    // container above it to group under (Task 4b) — every deeper one goes
    // through its parent's category group instead.
    QVector<QTreeWidgetItem *> parents;
    QHash<QTreeWidgetItem *, QHash<int, QTreeWidgetItem *>> categoryGroups;
    for (const auto &symbol : symbols) {
        const int depth = static_cast<int>(symbol.depth);
        parents.resize(depth + 1);
        auto *item = new QTreeWidgetItem();
        item->setText(0, symbol.name + QStringLiteral(" (") + symbolKindLabel(symbol.kind)
                            + QStringLiteral(")"));
        item->setIcon(0, symbolKindIcon(symbol.kind));
        item->setData(0, Qt::UserRole, static_cast<quint64>(symbol.name_start));
        // Task J: the bare name, for "Find Usages" — kept separate from
        // the display text above, which has the "(kind)" suffix baked in.
        item->setData(0, Qt::UserRole + 2, symbol.name);
        if (depth == 0) {
            tree_->addTopLevelItem(item);
        } else {
            QTreeWidgetItem *group = categoryGroup(categoryGroups, parents[depth - 1], symbol.category);
            group->addChild(item);
        }
        parents[depth] = item;
    }
    tree_->expandAll();
}

void StructurePanel::refreshProject()
{
    tree_->clear();
    fileItems_.clear();
    containerItems_.clear();
    categoryGroups_.clear();
    searchModel_->projectSymbols();
}

void StructurePanel::addProjectSymbol(const FfiSymbolMatch &row)
{
    QTreeWidgetItem *fileItem = fileItems_.value(row.path, nullptr);
    if (!fileItem) {
        fileItem = new QTreeWidgetItem(tree_, QStringList { QFileInfo(row.path).fileName() });
        fileItems_.insert(row.path, fileItem);
    }
    QTreeWidgetItem *parent = fileItem;
    if (!row.container.isEmpty()) {
        const QString key = row.path + QChar(0x1f) + row.container;
        QTreeWidgetItem *containerItem = containerItems_.value(key, nullptr);
        if (!containerItem) {
            containerItem = new QTreeWidgetItem(fileItem, QStringList { row.container });
            containerItems_.insert(key, containerItem);
        }
        parent = containerItem;
    }
    // Task 4b: every leaf nests under its category group, file-direct
    // leaves (no container) included — see categoryGroup()'s doc comment.
    QTreeWidgetItem *group = categoryGroup(categoryGroups_, parent, row.category);
    auto *item = new QTreeWidgetItem(
      group,
      QStringList { row.name + QStringLiteral(" (") + symbolKindLabel(row.kind)
                    + QStringLiteral(")") });
    item->setIcon(0, symbolKindIcon(row.kind));
    item->setData(0, Qt::UserRole, row.path);
    item->setData(0, Qt::UserRole + 1, row.line);
    // Task J: bare name for "Find Usages" — group nodes (file/container/
    // category, built above with QStringList-only constructors) never get
    // this role set, so the context menu naturally has nothing to offer
    // them.
    item->setData(0, Qt::UserRole + 2, row.name);
    item->setData(0, Qt::UserRole + 3, row.column);
}

QTreeWidgetItem *StructurePanel::categoryGroup(
  QHash<QTreeWidgetItem *, QHash<int, QTreeWidgetItem *>> &groups, QTreeWidgetItem *parent,
  FfiSymbolCategory category)
{
    QHash<int, QTreeWidgetItem *> &byCategory = groups[parent];
    const int key = static_cast<int>(category);
    const auto existing = byCategory.constFind(key);
    if (existing != byCategory.constEnd()) {
        return existing.value();
    }
    auto *group = new QTreeWidgetItem();
    group->setText(0, symbolCategoryLabel(category));
    group->setIcon(0, symbolCategoryIcon(category));
    // `byCategory` holds exactly `parent`'s existing children (groups are
    // the only thing ever parented directly under a class/container), so
    // counting the ones with a smaller ordinal gives this group's sorted
    // insertion index — keeping `FfiSymbolCategory`'s declared order even
    // though groups are created in whatever order their first member is
    // first seen.
    int insertAt = 0;
    for (auto it = byCategory.constBegin(); it != byCategory.constEnd(); ++it) {
        if (it.key() < key) {
            ++insertAt;
        }
    }
    parent->insertChild(insertAt, group);
    byCategory.insert(key, group);
    return group;
}

void StructurePanel::onItemDoubleClicked(QTreeWidgetItem *item)
{
    if (!item) {
        return;
    }
    if (projectMode_) {
        // File/container group nodes carry no data — only leaf symbol
        // items do (see addProjectSymbol above).
        const QVariant pathData = item->data(0, Qt::UserRole);
        if (!pathData.isValid()) {
            return;
        }
        editorTabs_->openFileAtLine(pathData.toString(),
                                     item->data(0, Qt::UserRole + 1).toInt(),
                                     item->data(0, Qt::UserRole + 3).toInt());
    } else {
        editorTabs_->jumpToByteOffset(item->data(0, Qt::UserRole).toULongLong());
    }
}

void StructurePanel::onContextMenuRequested(const QPoint &pos)
{
    QTreeWidgetItem *item = tree_->itemAt(pos);
    if (!item) {
        return;
    }
    const QVariant nameData = item->data(0, Qt::UserRole + 2);
    if (!nameData.isValid() || !onFindUsagesRequested_) {
        return;
    }
    QMenu menu(tree_);
    QAction *findUsagesAction = menu.addAction(tr("Find Usages"));
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == findUsagesAction) {
        onFindUsagesRequested_(nameData.toString());
    }
}

} // namespace ui_shell
