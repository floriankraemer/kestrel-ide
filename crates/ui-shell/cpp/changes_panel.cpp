#include "changes_panel.h"

#include "changes_toolbar.h"
#include "e2e_mark.h"
#include "git_dialogs.h"
#include "theme.h"

#include <QAction>
#include <QFileInfo>
#include <QFont>
#include <QGuiApplication>
#include <QClipboard>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QMenu>
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

// Column 0 (the letter) is fixed-width; the checkbox and the row's identity
// data both live on it, same as the single "File" column did before G7.
constexpr int kLetterColumn = 0;
constexpr int kNameColumn = 1;
constexpr int kLocationColumn = 2;
// A file row is one level deep under its group, and Qt reserves *two*
// indentation widths (PM_TreeViewIndentation, pinned by ChromeStyle) for it
// — one for the group's own chevron, one for the elbow into the row — inside
// this fixed-width column. A narrower column leaves no room left over for
// the checkbox and letter it is supposed to show.
constexpr int kLetterColumnWidth = 72;

constexpr int kPathRole = Qt::UserRole;
// Whether checking this item stages (true) or unstages (false) its path —
// the two groups' checkboxes mean opposite things, so the toggle handler
// can't infer it from which group the item's parent is once items move
// around during a refresh. Unused (left false) on a Merge Conflicts row,
// which is never checkable.
constexpr int kChecksToStageRole = Qt::UserRole + 1;
// Which group a row belongs to ("staged"/"unstaged"/"untracked"/
// "conflicts") — the context menu's "Show File History" reads it to know
// an untracked row has no history to show, the same thing
// `markChangesRow`'s own `group` parameter already reports to E2E.
constexpr int kGroupRole = Qt::UserRole + 2;

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
    case FfiChangeKind::Renamed:
        return QObject::tr("Renamed");
    case FfiChangeKind::Copied:
        return QObject::tr("Copied");
    case FfiChangeKind::Conflicted:
        return QObject::tr("Conflicted");
    case FfiChangeKind::None:
        break;
    }
    return QString();
}

// The git/VS Code single-letter status codes (the plan's own table).
// `changeKindLabel` above stays the tooltip; this is the cell text.
QString changeKindLetter(FfiChangeKind kind)
{
    switch (kind) {
    case FfiChangeKind::Added:
        return QObject::tr("A", "status letter: Added");
    case FfiChangeKind::Modified:
        return QObject::tr("M", "status letter: Modified");
    case FfiChangeKind::Deleted:
        return QObject::tr("D", "status letter: Deleted");
    case FfiChangeKind::TypeChanged:
        return QObject::tr("T", "status letter: Type Changed");
    case FfiChangeKind::Untracked:
        return QObject::tr("U", "status letter: Untracked");
    case FfiChangeKind::Renamed:
        return QObject::tr("R", "status letter: Renamed");
    case FfiChangeKind::Copied:
    case FfiChangeKind::Conflicted:
        // Git's own single-letter convention has no separate glyph for a
        // copy, and the plan's status-letter table names only Conflict for
        // "C" — a copy is the far rarer of the two (this app's `status`
        // porcelain call does not even pass `--find-copies`, ADR-0053) and
        // reads apart from a conflict on colour and weight alone
        // (`changeKindColor`/bold below).
        return QObject::tr("C", "status letter: Conflicted/Copied");
    case FfiChangeKind::None:
        break;
    }
    return QString();
}

// The plan's colour table, read from the theme rather than hardcoded so a
// colour picked here and one picked in QSS never drift apart.
QColor changeKindColor(FfiChangeKind kind)
{
    const DiffColors diff = diffColorsForTheme(activeThemeName());
    switch (kind) {
    case FfiChangeKind::Added:
    case FfiChangeKind::Untracked:
        return diff.addedMarker;
    case FfiChangeKind::Modified:
    case FfiChangeKind::Renamed:
    case FfiChangeKind::Copied:
    case FfiChangeKind::TypeChanged:
        return diff.modifiedMarker;
    case FfiChangeKind::Deleted:
        return diff.deletedMarker;
    case FfiChangeKind::Conflicted:
        return semanticColorsForTheme(activeThemeName()).error;
    case FfiChangeKind::None:
        break;
    }
    return QColor();
}

QTreeWidgetItem *makeGroup(QTreeWidget *tree, const QString &title)
{
    auto *group = new QTreeWidgetItem(tree, {title});
    QFont font = group->font(0);
    font.setBold(true);
    group->setFont(0, font);
    group->setFlags(Qt::ItemIsEnabled);
    group->setExpanded(true);
    // Otherwise the title is squeezed into the letter column's own narrow
    // width, which fits a one-character status letter but not a group
    // title like "Staged Changes (2)".
    group->setFirstColumnSpanned(true);
    return group;
}

// One row's worth of inputs bundled together (Clean Code: a 7-parameter
// `makeFileRow` is a parameter list asking to be a value object) — `kind`
// is whichever of `FfiChangedFile::staged`/`unstaged` this row represents,
// not the whole file's status.
struct FileRowSpec
{
    QString path;
    QString origPath;
    FfiChangeKind kind;
    bool checkable;
    bool checked;
    bool checksToStage;
    QString group;
};

QTreeWidgetItem *makeFileRow(QTreeWidgetItem *group, const FileRowSpec &spec)
{
    const QFileInfo info(spec.path);
    // `QFileInfo::path()` answers "." for a file with no directory
    // component — not a directory this dock should ever print.
    const QString directory = info.path() == QStringLiteral(".") ? QString() : info.path();

    auto *row = new QTreeWidgetItem(group);
    row->setText(kLetterColumn, changeKindLetter(spec.kind));
    row->setTextAlignment(kLetterColumn, Qt::AlignCenter);
    row->setToolTip(kLetterColumn, changeKindLabel(spec.kind));
    const QColor color = changeKindColor(spec.kind);
    if (color.isValid()) {
        row->setForeground(kLetterColumn, color);
    }
    if (spec.kind == FfiChangeKind::Conflicted) {
        QFont font = row->font(kLetterColumn);
        font.setBold(true);
        row->setFont(kLetterColumn, font);
    }

    row->setText(kNameColumn, info.fileName());

    QString location = directory;
    if (!spec.origPath.isEmpty()) {
        location = location.isEmpty()
                     ? QObject::tr("← was: %1").arg(spec.origPath)
                     : QObject::tr("%1  ← was: %2").arg(location, spec.origPath);
    }
    row->setText(kLocationColumn, location);
    row->setForeground(kLocationColumn, chromePaletteForTheme(activeThemeName()).textDim);

    if (spec.checkable) {
        row->setFlags(row->flags() | Qt::ItemIsUserCheckable);
        row->setCheckState(kLetterColumn, spec.checked ? Qt::Checked : Qt::Unchecked);
    }
    row->setData(kLetterColumn, kPathRole, spec.path);
    row->setData(kLetterColumn, kChecksToStageRole, spec.checksToStage);
    row->setData(kLetterColumn, kGroupRole, spec.group);
    return row;
}

// `rect` is this row's own label on screen, in global coordinates — the
// same reason `EditorTabs::markTab` reports a tab's rect: an E2E flow that
// has to click a specific file's checkbox would otherwise compute a row's
// position from the tree's font metrics and row height, which move for
// reasons unrelated to whatever it is testing. `status` is the single-letter
// code (`changeKindLetter`'s own text) so a test can assert it without
// decoding a colour.
void markChangesRow(QTreeWidget *tree, QTreeWidgetItem *row, const QString &path,
                     const QString &group, const QString &status)
{
    const QRect rect = tree->visualItemRect(row);
    const QPoint origin =
      rect.isEmpty() ? QPoint() : tree->viewport()->mapToGlobal(rect.topLeft());
    e2eMark(QStringLiteral("{\"ev\":\"changes_row\",\"path\":%1,\"group\":%2,\"status\":%3,"
                            "\"rect\":[%4,%5,%6,%7]}")
              .arg(e2eJson(path), e2eJson(group), e2eJson(status))
              .arg(origin.x())
              .arg(origin.y())
              .arg(rect.width())
              .arg(rect.height()));
}

} // namespace

ChangesPanel::ChangesPanel(VcsService *vcsService, std::function<void(const QString &)> showDiff,
                             std::function<void(const QString &)> showFileHistory,
                             QWidget *parent)
  : QWidget(parent)
  , vcsService_(vcsService)
  , showDiff_(std::move(showDiff))
  , showFileHistory_(std::move(showFileHistory))
{
    toolbar_ = new ChangesToolbar(vcsService_, this);
    connect(toolbar_, &ChangesToolbar::refreshRequested, vcsService_,
            [this]() { vcsService_->refreshStatus(); });
    connect(toolbar_, &ChangesToolbar::fetchRequested, vcsService_,
            [this]() { vcsService_->fetch(QStringLiteral("origin")); });
    connect(toolbar_, &ChangesToolbar::pullRequested, vcsService_, [this]() {
        const FfiBranchStatus status = vcsService_->branchStatus();
        if (!status.branch.isEmpty()) {
            vcsService_->pull(QStringLiteral("origin"), status.branch);
        }
    });
    connect(toolbar_, &ChangesToolbar::pushRequested, vcsService_, [this]() {
        const FfiBranchStatus status = vcsService_->branchStatus();
        if (!status.branch.isEmpty()) {
            vcsService_->push(QStringLiteral("origin"), status.branch,
                               /*setUpstream=*/!status.has_upstream);
        }
    });
    connect(toolbar_, &ChangesToolbar::stageAllRequested, vcsService_,
            [this]() { vcsService_->stageAll(); });
    connect(toolbar_, &ChangesToolbar::unstageAllRequested, vcsService_,
            [this]() { vcsService_->unstageAll(); });

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(3);
    tree_->setHeaderLabels({tr("Status"), tr("File"), tr("Location")});
    tree_->header()->setSectionResizeMode(kLetterColumn, QHeaderView::Fixed);
    tree_->setColumnWidth(kLetterColumn, kLetterColumnWidth);
    tree_->header()->setSectionResizeMode(kNameColumn, QHeaderView::Stretch);
    tree_->header()->setSectionResizeMode(kLocationColumn, QHeaderView::ResizeToContents);
    tree_->setUniformRowHeights(true);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(tree_, &QTreeWidget::itemDoubleClicked, this, [this](QTreeWidgetItem *item, int) {
        // Only a file row carries a path (kPathRole) — a group header
        // ("Staged"/"Unstaged"/"Untracked"/"Merge Conflicts") double-click
        // does nothing.
        //
        // The row's path is repository-relative, which is what the checkbox
        // handler below needs; opening the file needs a filesystem path, and
        // `absolutePath` is the one place that knows the repository root.
        const QString path = item->data(kLetterColumn, kPathRole).toString();
        if (path.isEmpty()) {
            return;
        }
        const QString absolute = vcsService_->absolutePath(path);
        if (!absolute.isEmpty()) {
            showDiff_(absolute);
        }
    });
    connect(tree_, &QTreeWidget::customContextMenuRequested, this, &ChangesPanel::showContextMenu);

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
    repoLayout->addWidget(toolbar_);
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

    // Merge Conflicts first, per the plan's mid-merge mockup; all four are
    // hidden when empty (`makeGroup` below), the existing pattern this only
    // grows from one entry to four.
    auto *conflicts = makeGroup(tree_, tr("Merge Conflicts"));
    auto *staged = makeGroup(tree_, tr("Staged Changes"));
    auto *unstaged = makeGroup(tree_, tr("Unstaged Changes"));
    auto *untracked = makeGroup(tree_, tr("Untracked Files"));

    // Row, path and group name, marked only once every row exists and every
    // empty group is hidden below — `visualItemRect` answers with whatever
    // the tree's *current* layout is, and an empty "Staged Changes" still
    // taking up a header row above "draft.txt" at the moment a row is
    // inserted is not the layout an E2E flow clicking that row's marked
    // rect will find on screen once `setHidden` below collapses it.
    QVector<std::tuple<QTreeWidgetItem *, QString, QString, QString>> rows;

    const ::rust::Vec<FfiChangedFile> files = vcsService_->changedFiles();
    for (const FfiChangedFile &file : files) {
        const QString path = file.path;
        const QString origPath = file.orig_path;
        // A file can be both staged-modified and unstaged-modified (staged,
        // then edited again) — it shows up in both groups rather than
        // picking one, since both are true at once.
        if (file.staged != FfiChangeKind::None) {
            QTreeWidgetItem *row = makeFileRow(
              staged, {path, origPath, file.staged, /*checkable=*/true, /*checked=*/true,
                       /*checksToStage=*/false, QStringLiteral("staged")});
            rows.append({row, path, QStringLiteral("staged"), changeKindLetter(file.staged)});
        }
        if (file.unstaged == FfiChangeKind::Conflicted) {
            // No checkbox: staging a conflict is not a thing this panel
            // offers (the plan's own wording) — resolving one is a "Discard
            // Changes…"/manual-edit-then-commit flow, not a checkbox toggle.
            QTreeWidgetItem *row = makeFileRow(
              conflicts, {path, origPath, file.unstaged, /*checkable=*/false, /*checked=*/false,
                          /*checksToStage=*/false, QStringLiteral("conflicts")});
            rows.append(
              {row, path, QStringLiteral("conflicts"), changeKindLetter(file.unstaged)});
        } else if (file.unstaged == FfiChangeKind::Untracked) {
            QTreeWidgetItem *row = makeFileRow(
              untracked, {path, origPath, file.unstaged, /*checkable=*/true, /*checked=*/false,
                          /*checksToStage=*/true, QStringLiteral("untracked")});
            rows.append(
              {row, path, QStringLiteral("untracked"), changeKindLetter(file.unstaged)});
        } else if (file.unstaged != FfiChangeKind::None) {
            QTreeWidgetItem *row = makeFileRow(
              unstaged, {path, origPath, file.unstaged, /*checkable=*/true, /*checked=*/false,
                         /*checksToStage=*/true, QStringLiteral("unstaged")});
            rows.append({row, path, QStringLiteral("unstaged"), changeKindLetter(file.unstaged)});
        }
    }

    for (QTreeWidgetItem *group : {conflicts, staged, unstaged, untracked}) {
        const int count = group->childCount();
        group->setText(0, tr("%1 (%2)").arg(group->text(0)).arg(count));
        group->setHidden(count == 0);
    }

    for (const auto &[row, path, group, status] : rows) {
        markChangesRow(tree_, row, path, group, status);
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
    if (populating_ || column != kLetterColumn || item->data(kLetterColumn, kPathRole).isNull()) {
        return;
    }
    const QString path = item->data(kLetterColumn, kPathRole).toString();
    const bool checksToStage = item->data(kLetterColumn, kChecksToStageRole).toBool();
    const bool checked = item->checkState(kLetterColumn) == Qt::Checked;
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

void ChangesPanel::showContextMenu(const QPoint &pos)
{
    QTreeWidgetItem *item = tree_->itemAt(pos);
    const QString path = item ? item->data(kLetterColumn, kPathRole).toString() : QString();
    if (path.isEmpty()) {
        // No item, or a group header (which carries no path) — nothing to
        // act on.
        return;
    }
    const bool checkable = item->flags().testFlag(Qt::ItemIsUserCheckable);
    const bool checksToStage = item->data(kLetterColumn, kChecksToStageRole).toBool();
    const QString group = item->data(kLetterColumn, kGroupRole).toString();

    QMenu menu(tree_);
    QAction *showDiffAction = menu.addAction(tr("Show Diff"));
    menu.addSeparator();
    // A conflicted row is never checkable (see `refresh()`): staging or
    // unstaging one is not offered here either, the same rule the missing
    // checkbox already states.
    QAction *stageToggleAction =
      checkable ? menu.addAction(checksToStage ? tr("Stage File") : tr("Unstage File")) : nullptr;
    QAction *discardAction = menu.addAction(tr("Discard Changes…"));
    menu.addSeparator();
    QAction *historyAction = menu.addAction(tr("Show File History"));
    // An untracked file has no commits to show history for.
    historyAction->setEnabled(group != QStringLiteral("untracked"));
    QAction *copyPathAction = menu.addAction(tr("Copy Path"));
    QAction *copyRelativePathAction = menu.addAction(tr("Copy Relative Path"));

    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == showDiffAction) {
        const QString absolute = vcsService_->absolutePath(path);
        if (!absolute.isEmpty()) {
            showDiff_(absolute);
        }
    } else if (chosen == stageToggleAction) {
        if (checksToStage) {
            vcsService_->stageFile(path);
        } else {
            vcsService_->unstageFile(path);
        }
    } else if (chosen == discardAction) {
        const QString name = QFileInfo(path).fileName();
        if (confirmDiscardChanges(this, name, "discard_changes_confirm")) {
            vcsService_->revertFile(path);
        }
    } else if (chosen == historyAction) {
        const QString absolute = vcsService_->absolutePath(path);
        if (!absolute.isEmpty()) {
            showFileHistory_(absolute);
        }
    } else if (chosen == copyPathAction) {
        const QString absolute = vcsService_->absolutePath(path);
        QGuiApplication::clipboard()->setText(absolute.isEmpty() ? path : absolute);
    } else if (chosen == copyRelativePathAction) {
        QGuiApplication::clipboard()->setText(path);
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
