#include "find_usages_panel.h"

#include "editor_tabs.h"
#include "search_preview_pane.h"

#include <QFileInfo>
#include <QLabel>
#include <QString>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QSplitter>
#include <QVBoxLayout>

namespace {

constexpr int kPathRole = Qt::UserRole;
constexpr int kLineRole = Qt::UserRole + 1;
constexpr int kColumnRole = Qt::UserRole + 2;

} // namespace

namespace ui_shell {

FindUsagesPanel::FindUsagesPanel(SearchModel *searchModel, EditorTabs *editorTabs, QWidget *parent)
  : QWidget(parent)
  , searchModel_(searchModel)
  , editorTabs_(editorTabs)
{
    statusLabel_ = new QLabel(this);
    results_ = new QTreeWidget(this);
    results_->setColumnCount(1);
    results_->setHeaderHidden(true);
    results_->setUniformRowHeights(true);
    preview_ = new SearchPreviewPane(this);

    auto *splitter = new QSplitter(Qt::Horizontal, this);
    splitter->addWidget(results_);
    splitter->addWidget(preview_);
    splitter->setStretchFactor(0, 1);
    splitter->setStretchFactor(1, 1);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(statusLabel_);
    layout->addWidget(splitter, 1);

    connect(results_, &QTreeWidget::itemDoubleClicked, this, &FindUsagesPanel::openSelected);
    connect(results_, &QTreeWidget::currentItemChanged, this,
            &FindUsagesPanel::previewSelection);
    connect(searchModel_, &SearchModel::usagesFound, this, &FindUsagesPanel::addUsage);
    connect(searchModel_, &SearchModel::usagesFinished, this, [this]() {
        int count = 0;
        for (int g = 0; g < results_->topLevelItemCount(); ++g) {
            count += results_->topLevelItem(g)->childCount();
        }
        statusLabel_->setText(tr("%1 result(s).").arg(count));
    });
    connect(searchModel_, &SearchModel::usagesFailed, this, [this](const QString &message) {
        statusLabel_->setText(tr("Search failed: %1").arg(message));
    });
}

void FindUsagesPanel::findUsages(const QString &name)
{
    beginQuery(tr("Searching usages of \"%1\"...").arg(name));
    searchModel_->findUsages(name);
}

void FindUsagesPanel::findUsagesAt(const QString &name, const QString &path, quint32 line,
                                    quint32 character)
{
    beginQuery(tr("Searching usages of \"%1\"...").arg(name));
    searchModel_->usagesAt(name, path, line, character);
}

void FindUsagesPanel::findImplementations(const QString &name)
{
    beginQuery(tr("Searching implementations of \"%1\"...").arg(name));
    searchModel_->findImplementations(name);
}

void FindUsagesPanel::findSupertypes(const QString &name)
{
    beginQuery(tr("Searching supertypes of \"%1\"...").arg(name));
    searchModel_->findSupertypes(name);
}

void FindUsagesPanel::beginQuery(const QString &status)
{
    results_->clear();
    preview_->clearPreview();
    statusLabel_->setText(status);
}

QTreeWidgetItem *FindUsagesPanel::fileGroup(const QString &path)
{
    // Rows arrive grouped by file already (`index_core::find_usages` sorts
    // by (path, line), and an LSP `references` answer is grouped the same
    // way by `lsp_core::group_by_uri` before it reaches here), so the last
    // top-level row is the right group unless the file just changed.
    if (results_->topLevelItemCount() > 0) {
        QTreeWidgetItem *last = results_->topLevelItem(results_->topLevelItemCount() - 1);
        if (last->data(0, kPathRole).toString() == path) {
            return last;
        }
    }
    auto *group = new QTreeWidgetItem(results_);
    group->setData(0, kPathRole, path);
    group->setText(0, QFileInfo(path).fileName());
    group->setToolTip(0, path);
    group->setExpanded(true);
    return group;
}

void FindUsagesPanel::addUsage(const FfiSymbolMatch &row)
{
    QTreeWidgetItem *group = fileGroup(row.path);
    const QString kindLabel = row.is_definition ? tr("def") : tr("ref");
    const QString label = row.container.isEmpty()
      ? tr("%1 [%2]").arg(row.line).arg(kindLabel)
      : tr("%1 [%2] in %3").arg(row.line).arg(kindLabel, row.container);
    auto *item = new QTreeWidgetItem(group);
    item->setText(0, label);
    item->setData(0, kPathRole, row.path);
    item->setData(0, kLineRole, row.line);
    item->setData(0, kColumnRole, row.column);
}

void FindUsagesPanel::previewSelection()
{
    QTreeWidgetItem *item = results_->currentItem();
    if (!item || item->childCount() > 0) {
        return;
    }
    const int column = item->data(0, kColumnRole).toInt();
    preview_->showMatch(item->data(0, kPathRole).toString(), item->data(0, kLineRole).toInt(),
                        column, column);
}

void FindUsagesPanel::openSelected(QTreeWidgetItem *item, int column)
{
    Q_UNUSED(column);
    if (!item || item->childCount() > 0) {
        return;
    }
    editorTabs_->openFileAtLine(item->data(0, kPathRole).toString(),
                                item->data(0, kLineRole).toInt(),
                                item->data(0, kColumnRole).toInt());
}

} // namespace ui_shell
