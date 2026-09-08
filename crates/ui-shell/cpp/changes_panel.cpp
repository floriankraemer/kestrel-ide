#include "changes_panel.h"

#include "e2e_mark.h"

#include <QFont>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QShowEvent>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QVBoxLayout>
#include <QVector>

#include <tuple>

namespace ui_shell {

namespace {

constexpr int kPathRole = Qt::UserRole;
// Whether checking this item stages (true) or unstages (false) its path —
// the two groups' checkboxes mean opposite things, so the toggle handler
// can't infer it from which group the item's parent is once items move
// around during a refresh.
constexpr int kChecksToStageRole = Qt::UserRole + 1;

QString changeKindLabel(FfiChangeKind kind)
{
    switch (kind) {
    case FfiChangeKind::Added:
        return QObject::tr("Added");
    case FfiChangeKind::Modified:
        return QObject::tr("Modified");
    case FfiChangeKind::Deleted:
        return QObject::tr("Deleted");
    case FfiChangeKind::TypeChanged:
        return QObject::tr("Type Changed");
    case FfiChangeKind::Untracked:
        return QObject::tr("Untracked");
    case FfiChangeKind::None:
        break;
    }
    return QString();
}

QTreeWidgetItem *makeGroup(QTreeWidget *tree, const QString &title)
{
    auto *group = new QTreeWidgetItem(tree, {title});
    QFont font = group->font(0);
    font.setBold(true);
    group->setFont(0, font);
    group->setFlags(Qt::ItemIsEnabled);
    group->setExpanded(true);
    return group;
}

QTreeWidgetItem *makeFileRow(QTreeWidgetItem *group, const QString &path, FfiChangeKind kind,
                             bool checked, bool checksToStage)
{
    auto *row = new QTreeWidgetItem(group, {path, changeKindLabel(kind)});
    row->setFlags(row->flags() | Qt::ItemIsUserCheckable);
    row->setCheckState(0, checked ? Qt::Checked : Qt::Unchecked);
    row->setData(0, kPathRole, path);
    row->setData(0, kChecksToStageRole, checksToStage);
    return row;
}

// `rect` is this row's own label on screen, in global coordinates — the
// same reason `EditorTabs::markTab` reports a tab's rect: an E2E flow that
// has to click a specific file's checkbox would otherwise compute a row's
// position from the tree's font metrics and row height, which move for
// reasons unrelated to whatever it is testing.
void markChangesRow(QTreeWidget *tree, QTreeWidgetItem *row, const QString &path,
                     const QString &group)
{
    const QRect rect = tree->visualItemRect(row);
    const QPoint origin =
      rect.isEmpty() ? QPoint() : tree->viewport()->mapToGlobal(rect.topLeft());
    e2eMark(QStringLiteral("{\"ev\":\"changes_row\",\"path\":%1,\"group\":%2,"
                            "\"rect\":[%3,%4,%5,%6]}")
              .arg(e2eJson(path), e2eJson(group))
              .arg(origin.x())
              .arg(origin.y())
              .arg(rect.width())
              .arg(rect.height()));
}

} // namespace

ChangesPanel::ChangesPanel(VcsService *vcsService, std::function<void(const QString &)> showDiff,
                            QWidget *parent)
  : QWidget(parent)
  , vcsService_(vcsService)
  , showDiff_(std::move(showDiff))
{
    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(2);
    tree_->setHeaderLabels({tr("File"), tr("Status")});
    tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    tree_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    tree_->setUniformRowHeights(true);
    connect(tree_, &QTreeWidget::itemDoubleClicked, this, [this](QTreeWidgetItem *item, int) {
        // Only a file row carries a path (kPathRole) — a group header
        // ("Staged"/"Unstaged"/"Untracked") double-click does nothing.
        //
        // The row's path is repository-relative, which is what the checkbox
        // handler below needs; opening the file needs a filesystem path, and
        // `absolutePath` is the one place that knows the repository root.
        const QString path = item->data(0, kPathRole).toString();
        if (path.isEmpty()) {
            return;
        }
        const QString absolute = vcsService_->absolutePath(path);
        if (!absolute.isEmpty()) {
            showDiff_(absolute);
        }
    });

    messageEdit_ = new QPlainTextEdit(this);
    messageEdit_->setPlaceholderText(tr("Commit message"));
    messageEdit_->setMaximumHeight(80);

    commitButton_ = new QPushButton(tr("Commit"), this);
    commitAndPushButton_ = new QPushButton(tr("Commit and Push"), this);
    amendButton_ = new QPushButton(tr("Amend"), this);

    auto *buttonRow = new QHBoxLayout();
    buttonRow->addWidget(commitButton_);
    buttonRow->addWidget(commitAndPushButton_);
    buttonRow->addWidget(amendButton_);
    buttonRow->addStretch(1);

    repoWidgets_ = new QWidget(this);
    auto *repoLayout = new QVBoxLayout(repoWidgets_);
    repoLayout->setContentsMargins(0, 0, 0, 0);
    repoLayout->addWidget(tree_, 1);
    repoLayout->addWidget(messageEdit_);
    repoLayout->addLayout(buttonRow);

    // Shown instead of `repoWidgets_` for a project that isn't a Git
    // repository at all (F3-2's "not a repository" outcome) — `refresh()`
    // never used to give this case any feedback, leaving an empty tree with
    // no way to get from "no repository" to "I can commit" without a
    // terminal. Humble view: this widget only calls `initRepository`/
    // `setDeclinedGitInit` and re-reads `declinedGitInit()`/`isRepository()`
    // to pick its own wording — it does not decide what either means.
    emptyState_ = new QWidget(this);
    emptyStateLabel_ = new QLabel(emptyState_);
    emptyStateLabel_->setAlignment(Qt::AlignCenter);
    emptyStateLabel_->setWordWrap(true);
    initButton_ = new QPushButton(tr("Initialize Git Repository"), emptyState_);
    notNowButton_ = new QPushButton(tr("Not now"), emptyState_);
    notNowButton_->setFlat(true);

    auto *emptyLayout = new QVBoxLayout(emptyState_);
    emptyLayout->addStretch(1);
    emptyLayout->addWidget(emptyStateLabel_);
    auto *initRow = new QHBoxLayout();
    initRow->addStretch(1);
    initRow->addWidget(initButton_);
    initRow->addStretch(1);
    emptyLayout->addLayout(initRow);
    auto *notNowRow = new QHBoxLayout();
    notNowRow->addStretch(1);
    notNowRow->addWidget(notNowButton_);
    notNowRow->addStretch(1);
    emptyLayout->addLayout(notNowRow);
    emptyLayout->addStretch(1);

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(repoWidgets_, 1);
    layout->addWidget(emptyState_, 1);

    connect(tree_, &QTreeWidget::itemChanged, this, &ChangesPanel::onItemChanged);
    connect(commitButton_, &QPushButton::clicked, this,
            [this]() { doCommit(/*amend=*/false, /*push=*/false); });
    connect(commitAndPushButton_, &QPushButton::clicked, this,
            [this]() { doCommit(/*amend=*/false, /*push=*/true); });
    connect(amendButton_, &QPushButton::clicked, this,
            [this]() { doCommit(/*amend=*/true, /*push=*/false); });
    connect(initButton_, &QPushButton::clicked, vcsService_,
            [this]() { vcsService_->initRepository(); });
    connect(notNowButton_, &QPushButton::clicked, vcsService_, [this]() {
        vcsService_->setDeclinedGitInit(true);
        refreshEmptyState();
    });

    connect(vcsService_, &VcsService::statusChanged, this, &ChangesPanel::refresh);
    connect(vcsService_, &VcsService::repositoryChanged, this, &ChangesPanel::refresh);

    refresh();
}

void ChangesPanel::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    // Same reasoning as `markChangesRow`: the commit message box and the
    // Commit button move with the dock's own layout, so an E2E flow reads
    // their on-screen rects here instead of guessing them from the main
    // window's geometry.
    const QRect messageRect(messageEdit_->mapToGlobal(QPoint(0, 0)), messageEdit_->size());
    const QRect commitRect(commitButton_->mapToGlobal(QPoint(0, 0)), commitButton_->size());
    e2eMark(QStringLiteral("{\"ev\":\"changes_panel_shown\","
                            "\"message_rect\":[%1,%2,%3,%4],"
                            "\"commit_rect\":[%5,%6,%7,%8]}")
              .arg(messageRect.x())
              .arg(messageRect.y())
              .arg(messageRect.width())
              .arg(messageRect.height())
              .arg(commitRect.x())
              .arg(commitRect.y())
              .arg(commitRect.width())
              .arg(commitRect.height()));
}

void ChangesPanel::refresh()
{
    refreshEmptyState();
    if (!vcsService_->isRepository()) {
        return;
    }

    populating_ = true;
    tree_->clear();

    auto *staged = makeGroup(tree_, tr("Staged Changes"));
    auto *unstaged = makeGroup(tree_, tr("Unstaged Changes"));
    auto *untracked = makeGroup(tree_, tr("Untracked Files"));

    // Row, path and group name, marked only once every row exists and every
    // empty group is hidden below — `visualItemRect` answers with whatever
    // the tree's *current* layout is, and an empty "Staged Changes" still
    // taking up a header row above "draft.txt" at the moment a row is
    // inserted is not the layout an E2E flow clicking that row's marked
    // rect will find on screen once `setHidden` below collapses it.
    QVector<std::tuple<QTreeWidgetItem *, QString, QString>> rows;

    const ::rust::Vec<FfiChangedFile> files = vcsService_->changedFiles();
    for (const FfiChangedFile &file : files) {
        const QString path = file.path;
        // A file can be both staged-modified and unstaged-modified (staged,
        // then edited again) — it shows up in both groups rather than
        // picking one, since both are true at once.
        if (file.staged != FfiChangeKind::None) {
            QTreeWidgetItem *row =
              makeFileRow(staged, path, file.staged, /*checked=*/true, /*checksToStage=*/false);
            rows.append({row, path, QStringLiteral("staged")});
        }
        if (file.unstaged == FfiChangeKind::Untracked) {
            QTreeWidgetItem *row =
              makeFileRow(untracked, path, file.unstaged, /*checked=*/false, /*checksToStage=*/true);
            rows.append({row, path, QStringLiteral("untracked")});
        } else if (file.unstaged != FfiChangeKind::None) {
            QTreeWidgetItem *row =
              makeFileRow(unstaged, path, file.unstaged, /*checked=*/false, /*checksToStage=*/true);
            rows.append({row, path, QStringLiteral("unstaged")});
        }
    }

    for (QTreeWidgetItem *group : {staged, unstaged, untracked}) {
        group->setHidden(group->childCount() == 0);
    }

    for (const auto &[row, path, group] : rows) {
        markChangesRow(tree_, row, path, group);
    }

    populating_ = false;
}

void ChangesPanel::refreshEmptyState()
{
    const bool isRepo = vcsService_->isRepository();
    repoWidgets_->setVisible(isRepo);
    emptyState_->setVisible(!isRepo);
    if (isRepo) {
        return;
    }
    const bool declined = vcsService_->declinedGitInit();
    emptyStateLabel_->setText(declined ? tr("No Git Repository initialized.")
                                        : tr("This folder is not a Git repository."));
    notNowButton_->setVisible(!declined);
}

void ChangesPanel::onItemChanged(QTreeWidgetItem *item, int column)
{
    if (populating_ || column != 0 || item->data(0, kPathRole).isNull()) {
        return;
    }
    const QString path = item->data(0, kPathRole).toString();
    const bool checksToStage = item->data(0, kChecksToStageRole).toBool();
    const bool checked = item->checkState(0) == Qt::Checked;
    // A checkbox in the unstaged/untracked group means "stage me" when
    // checked; one in the staged group means "unstage me" when unchecked.
    // The other two combinations (staged-and-checked, unstaged-and-
    // unchecked) are each group's resting state and never fire this slot.
    if (checksToStage && checked) {
        vcsService_->stageFile(path);
    } else if (!checksToStage && !checked) {
        vcsService_->unstageFile(path);
    }
}

void ChangesPanel::doCommit(bool amend, bool push)
{
    const QString message = messageEdit_->toPlainText().trimmed();
    // `commit(message, amend)` always writes `message` as the commit
    // message, amend included — there is no "keep the previous message"
    // path across the seam, so an empty box is refused even for Amend
    // rather than silently committing an empty message.
    if (message.isEmpty()) {
        return;
    }
    vcsService_->commit(message, amend);
    if (push) {
        // Queued right behind the commit above on the same worker job
        // queue (VcsService's jobs run FIFO on one thread), so this always
        // pushes the commit just made, not a stale HEAD.
        const QString branch = vcsService_->currentBranch();
        if (!branch.isEmpty()) {
            vcsService_->push(QStringLiteral("origin"), branch, /*setUpstream=*/false);
        }
    }
    messageEdit_->clear();
}

} // namespace ui_shell
