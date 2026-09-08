#include "history_list_view.h"

#include "e2e_mark.h"

#include <QDateTime>
#include <QHeaderView>
#include <QLabel>
#include <QLocale>
#include <QMenu>
#include <QPointer>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QVBoxLayout>

namespace ui_shell {

namespace {
constexpr int kCommitIdRole = Qt::UserRole;
// Set on the one child item a row starts with (a stand-in so the expand
// arrow shows before the real body is fetched); cleared once
// `populateBody` has replaced its text with the real one, so a later
// collapse/expand does not re-fetch.
constexpr int kPlaceholderRole = Qt::UserRole + 1;

enum Column
{
    kDateColumn,
    kIdColumn,
    kAuthorColumn,
    kSummaryColumn,
    kColumnCount,
};
} // namespace

HistoryListView::HistoryListView(VcsService *vcsService, QWidget *parent)
  : QWidget(parent), vcsService_(vcsService)
{
    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(kColumnCount);
    tree_->setHeaderHidden(true);
    tree_->setRootIsDecorated(true);
    tree_->setUniformRowHeights(false);
    tree_->setSelectionMode(QAbstractItemView::ExtendedSelection);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);
    tree_->header()->setSectionResizeMode(kSummaryColumn, QHeaderView::Stretch);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(tree_);

    connect(tree_, &QTreeWidget::itemExpanded, this, &HistoryListView::onItemExpanded);
    connect(tree_, &QTreeWidget::itemActivated, this, &HistoryListView::onItemActivated);
    connect(tree_, &QTreeWidget::customContextMenuRequested, this,
            &HistoryListView::showContextMenu);
}

void HistoryListView::setEntries(const ::rust::Vec<FfiLogEntry> &entries)
{
    tree_->clear();
    const QLocale locale;
    for (const FfiLogEntry &entry : entries) {
        const QDateTime when = QDateTime::fromSecsSinceEpoch(entry.author_time);
        const QString commitId = QString(entry.id);

        auto *item = new QTreeWidgetItem(tree_);
        item->setText(kDateColumn, locale.toString(when, QLocale::ShortFormat));
        item->setText(kIdColumn, commitId.left(8));
        item->setText(kAuthorColumn, QString(entry.author_name));
        item->setText(kSummaryColumn, QString(entry.summary));
        item->setData(kIdColumn, kCommitIdRole, commitId);

        // A placeholder child so the expand arrow shows before the body is
        // fetched — replaced with the real wrapped text on first expand.
        auto *placeholder = new QTreeWidgetItem(item);
        placeholder->setFlags(placeholder->flags() & ~Qt::ItemIsSelectable);
        placeholder->setData(kDateColumn, kPlaceholderRole, true);
    }
    for (int i = 0; i < kColumnCount - 1; ++i) {
        tree_->resizeColumnToContents(i);
    }
}

int HistoryListView::commitCount() const
{
    return tree_->topLevelItemCount();
}

QString HistoryListView::commitIdAt(int row) const
{
    QTreeWidgetItem *item = tree_->topLevelItem(row);
    return item ? item->data(kIdColumn, kCommitIdRole).toString() : QString();
}

QRect HistoryListView::globalRectForRow(int row) const
{
    QTreeWidgetItem *item = tree_->topLevelItem(row);
    if (!item) {
        return QRect();
    }
    const QRect rect = tree_->visualItemRect(item);
    if (rect.isEmpty()) {
        return QRect();
    }
    return QRect(tree_->viewport()->mapToGlobal(rect.topLeft()), rect.size());
}

QStringList HistoryListView::selectedCommitIds() const
{
    QStringList ids;
    for (QTreeWidgetItem *item : tree_->selectedItems()) {
        // Skip a selected placeholder/body child — only top-level (commit)
        // rows count as a selection here.
        if (item->parent() != nullptr) {
            continue;
        }
        ids.append(item->data(kIdColumn, kCommitIdRole).toString());
    }
    return ids;
}

QString HistoryListView::firstSelectedCommitId() const
{
    const QStringList ids = selectedCommitIds();
    return ids.isEmpty() ? QString() : ids.first();
}

void HistoryListView::onItemExpanded(QTreeWidgetItem *item)
{
    if (item->childCount() != 1) {
        return;
    }
    QTreeWidgetItem *child = item->child(0);
    if (!child->data(kDateColumn, kPlaceholderRole).toBool()) {
        return;
    }
    populateBody(child, item->data(kIdColumn, kCommitIdRole).toString());
}

void HistoryListView::populateBody(QTreeWidgetItem *placeholder, const QString &commitId)
{
    auto *label = new QLabel(tr("Loading…"), tree_);
    label->setWordWrap(true);
    label->setContentsMargins(24, 2, 8, 6);
    tree_->setItemWidget(placeholder, kDateColumn, label);
    tree_->setFirstColumnSpanned(0, tree_->indexFromItem(placeholder->parent()), true);
    placeholder->setData(kDateColumn, kPlaceholderRole, false);

    const FfiCommitDetail cached = vcsService_->commitDetail(commitId);
    if (!QString(cached.id).isEmpty()) {
        label->setText(QString(cached.body).isEmpty() ? tr("(no further description)")
                                                         : QString(cached.body));
        return;
    }

    // Not fetched yet: ask, and fill the label in once the answer arrives.
    // Left connected rather than a one-shot disconnect — a body already
    // shown just gets the same text again if this id's detail is
    // requested a second time elsewhere (the commit-detail dock does),
    // which is harmless.
    connect(vcsService_, &VcsService::commitDetailReady, tree_,
            [this, label = QPointer<QLabel>(label), commitId](const QString &id) {
                if (id != commitId || !label) {
                    return;
                }
                const FfiCommitDetail detail = vcsService_->commitDetail(commitId);
                label->setText(QString(detail.body).isEmpty() ? tr("(no further description)")
                                                                : QString(detail.body));
            });
    vcsService_->requestCommitDetail(commitId);
}

void HistoryListView::onItemActivated(QTreeWidgetItem *item, int /*column*/)
{
    if (item->parent() != nullptr) {
        // The body child, not a commit row.
        return;
    }
    const QString commitId = item->data(kIdColumn, kCommitIdRole).toString();
    e2eMark(QStringLiteral("{\"ev\":\"commit_activated\",\"commit\":%1}").arg(e2eJson(commitId)));
    emit commitActivated(commitId);
}

void HistoryListView::showContextMenu(const QPoint &pos)
{
    const QStringList ids = selectedCommitIds();
    if (ids.isEmpty()) {
        return;
    }
    emit contextMenuRequestedFor(tree_->viewport()->mapToGlobal(pos), ids);
}

} // namespace ui_shell
