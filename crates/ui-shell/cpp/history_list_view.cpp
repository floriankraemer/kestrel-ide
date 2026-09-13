#include "history_list_view.h"

#include "e2e_mark.h"

#include <QDateTime>
#include <QFont>
#include <QHeaderView>
#include <QLabel>
#include <QLocale>
#include <QMenu>
#include <QPainter>
#include <QPointer>
#include <QStyledItemDelegate>
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
// R7: this row's lane-graph column (int) and the columns its parent lines
// continue into (`QVariantList` of int) — `LaneGraphDelegate::paint`'s own
// data.
constexpr int kLaneRole = Qt::UserRole + 2;
constexpr int kParentLanesRole = Qt::UserRole + 3;

enum Column
{
    kLaneColumn,
    kDateColumn,
    kIdColumn,
    kAuthorColumn,
    kRefsColumn,
    kSummaryColumn,
    kColumnCount,
};

// R7: a simple lane graph — one coloured dot per commit, a vertical line
// through every lane still "live" at this row, and a short diagonal for a
// merge's second-and-later parent lines. Not a full curve renderer (out of
// scope per the plan): each cell is painted independently, so a line only
// ever spans this row's own height, which is enough to read as continuous
// once rows are stacked.
class LaneGraphDelegate : public QStyledItemDelegate
{
public:
    explicit LaneGraphDelegate(QObject *parent)
      : QStyledItemDelegate(parent)
    {
    }

    void paint(QPainter *painter, const QStyleOptionViewItem &option,
               const QModelIndex &index) const override
    {
        painter->save();
        painter->setRenderHint(QPainter::Antialiasing, true);
        painter->fillRect(option.rect, option.palette.base());

        const int lane = index.data(kLaneRole).toInt();
        const QVariantList parentLanes = index.data(kParentLanesRole).toList();
        const int cx = option.rect.left() + kLaneSpacing / 2 + lane * kLaneSpacing;
        const int cy = option.rect.center().y();
        const QColor color = laneColor(lane);

        painter->setPen(QPen(color, 2));
        // This lane continues through the row: a line top-to-bottom.
        painter->drawLine(cx, option.rect.top(), cx, option.rect.bottom());
        // Every other parent lane (a merge's second-and-later parent) gets
        // a short line from this dot down to its own column.
        for (const QVariant &parentLaneVariant : parentLanes) {
            const int parentLane = parentLaneVariant.toInt();
            if (parentLane == lane) {
                continue;
            }
            const int px = option.rect.left() + kLaneSpacing / 2 + parentLane * kLaneSpacing;
            painter->setPen(QPen(laneColor(parentLane), 2));
            painter->drawLine(cx, cy, px, option.rect.bottom());
        }

        painter->setPen(Qt::NoPen);
        painter->setBrush(color);
        painter->drawEllipse(QPoint(cx, cy), 4, 4);
        painter->restore();
    }

    QSize sizeHint(const QStyleOptionViewItem &option, const QModelIndex &index) const override
    {
        const int lanes = qMax(1, index.data(kLaneRole).toInt() + 1);
        return QSize(lanes * kLaneSpacing + kLaneSpacing, option.rect.height());
    }

private:
    static constexpr int kLaneSpacing = 14;

    static QColor laneColor(int lane)
    {
        // A small fixed palette, cycled — distinguishing adjacent lanes is
        // the only job this does, not carrying meaning per index.
        static const QColor kPalette[] = {
            QColor(0x4c, 0xaf, 0x50), QColor(0x21, 0x96, 0xf3), QColor(0xff, 0x98, 0x00),
            QColor(0x9c, 0x27, 0xb0), QColor(0xf4, 0x43, 0x36), QColor(0x00, 0xbc, 0xd4),
        };
        constexpr int kPaletteSize = sizeof(kPalette) / sizeof(kPalette[0]);
        return kPalette[lane % kPaletteSize];
    }
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
    tree_->setItemDelegateForColumn(kLaneColumn, new LaneGraphDelegate(tree_));

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
        item->setData(kLaneColumn, kLaneRole, static_cast<int>(entry.lane));
        QVariantList parentLanes;
        for (std::size_t i = 0; i < entry.parent_lanes.size(); ++i) {
            parentLanes.append(static_cast<int>(entry.parent_lanes[i]));
        }
        item->setData(kLaneColumn, kParentLanesRole, parentLanes);

        // R7: ref chips — `HEAD`/branches/tags decorating this commit,
        // comma-joined; a bold, tinted font marks the branch `HEAD` sits
        // on so it reads apart from every other decoration at a glance.
        QStringList refNames;
        bool hasHead = false;
        for (const FfiRefDecoration &ref : vcsService_->commitRefs(commitId)) {
            refNames.append(QString(ref.name));
            hasHead = hasHead || ref.kind == FfiRefKind::Head;
        }
        if (!refNames.isEmpty()) {
            item->setText(kRefsColumn, refNames.join(QStringLiteral(", ")));
            QColor refColor = palette().color(QPalette::Link);
            if (hasHead) {
                QFont font = item->font(kRefsColumn);
                font.setBold(true);
                item->setFont(kRefsColumn, font);
            }
            item->setForeground(kRefsColumn, refColor);
        }

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
