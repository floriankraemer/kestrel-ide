#include "commit_log_panel.h"

#include "branch_popup.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "history_list_view.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QCheckBox>
#include <QClipboard>
#include <QDateEdit>
#include <QDateTime>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QInputDialog>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QPushButton>
#include <QShowEvent>
#include <QVBoxLayout>

#include <limits>

namespace ui_shell {

namespace {
// A panel only ever shows one page of commits at a time; "Load more" grows
// by this much rather than the whole repository's history at once.
constexpr quint32 kPageSize = 200;

constexpr int vcsErrorCode(FfiVcsErrorCode code)
{
    return static_cast<int>(code);
}

// `QDateEdit`'s value, as seconds since the Unix epoch at local midnight —
// `commitLogFiltered`'s `since`/`until` are dates, not instants, so this
// treats the whole day as included the same way `git log --since=<date>`
// does.
qint64 toEpochSeconds(const QDate &date)
{
    return QDateTime(date, QTime(0, 0), Qt::LocalTime).toSecsSinceEpoch();
}

} // namespace

CommitLogPanel::CommitLogPanel(VcsService *vcsService, std::function<void(const QString &)>
                                                          openCommit,
                                QWidget *parent)
  : QWidget(parent)
  , vcsService_(vcsService)
  , openCommit_(std::move(openCommit))
  , currentMax_(kPageSize)
{
    list_ = new HistoryListView(vcsService_, this);
    connect(list_, &HistoryListView::commitActivated, this,
            [this](const QString &commitId) { openCommit_(commitId); });
    connect(list_, &HistoryListView::contextMenuRequestedFor, this,
            &CommitLogPanel::showRowContextMenu);

    // R7: filter bar — text, author, path, date range. Every field empty/
    // disabled means "no filter", the same as `commitLogFiltered`'s own
    // contract.
    auto *filterBar = new QWidget(this);
    auto *filterLayout = new QHBoxLayout(filterBar);
    filterLayout->setContentsMargins(4, 4, 4, 4);

    textFilter_ = new QLineEdit(filterBar);
    textFilter_->setPlaceholderText(tr("Message contains..."));
    filterLayout->addWidget(textFilter_, 2);

    authorFilter_ = new QLineEdit(filterBar);
    authorFilter_->setPlaceholderText(tr("Author..."));
    filterLayout->addWidget(authorFilter_, 1);

    pathFilter_ = new QLineEdit(filterBar);
    pathFilter_->setPlaceholderText(tr("Path..."));
    filterLayout->addWidget(pathFilter_, 1);

    sinceEnabled_ = new QCheckBox(tr("Since"), filterBar);
    filterLayout->addWidget(sinceEnabled_);
    sinceDate_ = new QDateEdit(QDate::currentDate().addMonths(-1), filterBar);
    sinceDate_->setCalendarPopup(true);
    sinceDate_->setEnabled(false);
    filterLayout->addWidget(sinceDate_);
    connect(sinceEnabled_, &QCheckBox::toggled, sinceDate_, &QDateEdit::setEnabled);

    untilEnabled_ = new QCheckBox(tr("Until"), filterBar);
    filterLayout->addWidget(untilEnabled_);
    untilDate_ = new QDateEdit(QDate::currentDate(), filterBar);
    untilDate_->setCalendarPopup(true);
    untilDate_->setEnabled(false);
    filterLayout->addWidget(untilDate_);
    connect(untilEnabled_, &QCheckBox::toggled, untilDate_, &QDateEdit::setEnabled);

    for (auto *edit : {textFilter_, authorFilter_, pathFilter_}) {
        connect(edit, &QLineEdit::returnPressed, this, &CommitLogPanel::refresh);
    }
    for (auto *check : {sinceEnabled_, untilEnabled_}) {
        connect(check, &QCheckBox::toggled, this, &CommitLogPanel::refresh);
    }
    for (auto *date : {sinceDate_, untilDate_}) {
        connect(date, &QDateEdit::dateChanged, this, &CommitLogPanel::refresh);
    }

    loadMoreButton_ = new QPushButton(tr("Load more"), this);
    connect(loadMoreButton_, &QPushButton::clicked, this, &CommitLogPanel::loadMore);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(filterBar);
    layout->addWidget(list_, 1);
    layout->addWidget(loadMoreButton_);

    connect(vcsService_, &VcsService::commitLogReady, this, &CommitLogPanel::onCommitLogReady);
    // The log changes on every commit/checkout/pull — re-ask rather than
    // trying to patch the existing list in place.
    connect(vcsService_, &VcsService::statusChanged, this, &CommitLogPanel::refresh);
    connect(vcsService_, &VcsService::branchChanged, this, &CommitLogPanel::refresh);
    connect(vcsService_, &VcsService::repositoryChanged, this, &CommitLogPanel::refresh);

    refresh();
}

void CommitLogPanel::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    refresh();
}

void CommitLogPanel::refresh()
{
    // `commitLogFiltered` takes every field, empty/`i64::MIN` meaning
    // unset — when none of the filter widgets carry a value, this is
    // exactly what plain `commitLog` already did.
    if (textFilter_->text().isEmpty() && authorFilter_->text().isEmpty()
        && pathFilter_->text().isEmpty() && !sinceEnabled_->isChecked()
        && !untilEnabled_->isChecked()) {
        vcsService_->commitLog(currentMax_);
        return;
    }
    vcsService_->commitLogFiltered(
      authorFilter_->text(), pathFilter_->text(), textFilter_->text(),
      sinceEnabled_->isChecked() ? toEpochSeconds(sinceDate_->date())
                                  : std::numeric_limits<qint64>::min(),
      untilEnabled_->isChecked() ? toEpochSeconds(untilDate_->date())
                                  : std::numeric_limits<qint64>::min(),
      currentMax_);
}

void CommitLogPanel::loadMore()
{
    currentMax_ += kPageSize;
    refresh();
}

void CommitLogPanel::onCommitLogReady(const ::rust::Vec<FfiLogEntry> &entries)
{
    list_->setEntries(entries);
    // Nothing more to load once the answer came back shorter than what was
    // asked for — the repository's whole history is already shown.
    loadMoreButton_->setEnabled(static_cast<quint32>(entries.size()) >= currentMax_);
    for (int row = 0; row < list_->commitCount(); ++row) {
        const QRect rect = list_->globalRectForRow(row);
        e2eMark(QStringLiteral("{\"ev\":\"commit_log_row\",\"commit\":%1,"
                                "\"row\":%2,\"rect\":[%3,%4,%5,%6]}")
                  .arg(e2eJson(list_->commitIdAt(row)))
                  .arg(row)
                  .arg(rect.x())
                  .arg(rect.y())
                  .arg(rect.width())
                  .arg(rect.height()));
    }
}

void CommitLogPanel::showRowContextMenu(const QPoint &globalPos, const QStringList &selectedIds)
{
    if (selectedIds.isEmpty()) {
        return;
    }
    const QString id = selectedIds.first();
    const QString shortId = id.left(8);

    auto *menu = new QMenu(this);
    menu->setAttribute(Qt::WA_DeleteOnClose);

    QAction *checkoutAction = menu->addAction(tr("Checkout Revision"));
    QAction *newBranchAction = menu->addAction(tr("New Branch Here..."));
    menu->addSeparator();
    QAction *cherryPickAction = menu->addAction(tr("Cherry-pick"));
    QAction *revertAction = menu->addAction(tr("Revert Commit"));
    menu->addSeparator();
    QMenu *resetMenu = menu->addMenu(tr("Reset Current Branch Here"));
    QAction *resetSoftAction = resetMenu->addAction(tr("Soft"));
    QAction *resetMixedAction = resetMenu->addAction(tr("Mixed"));
    QAction *resetHardAction = resetMenu->addAction(tr("Hard..."));
    menu->addSeparator();
    QAction *copyHashAction = menu->addAction(tr("Copy Hash"));

    connect(checkoutAction, &QAction::triggered, vcsService_,
            [this, id]() { vcsService_->checkout(id); });
    connect(newBranchAction, &QAction::triggered, this, [this, id]() {
        const QString name =
          QInputDialog::getText(this, tr("New Branch"), tr("Branch name:"));
        if (!name.isEmpty()) {
            vcsService_->createBranch(name, id);
        }
    });
    connect(cherryPickAction, &QAction::triggered, vcsService_, [this, id]() {
        watchIntegrationResult(this, vcsService_, tr("Cherry-pick"));
        vcsService_->cherryPick(id);
    });
    connect(revertAction, &QAction::triggered, vcsService_, [this, id]() {
        watchIntegrationResult(this, vcsService_, tr("Revert Commit"));
        vcsService_->revertCommit(id);
    });
    connect(resetSoftAction, &QAction::triggered, vcsService_,
            [this, id]() { vcsService_->resetTo(id, FfiResetMode::Soft); });
    connect(resetMixedAction, &QAction::triggered, vcsService_,
            [this, id]() { vcsService_->resetTo(id, FfiResetMode::Mixed); });
    connect(resetHardAction, &QAction::triggered, this, [this, id, shortId]() {
        const auto choice = QMessageBox::warning(
          this, tr("Reset Current Branch"),
          tr("Reset the current branch to %1 (hard)? Uncommitted changes and every "
             "commit after it on this branch are discarded.")
            .arg(shortId),
          QMessageBox::Cancel | QMessageBox::Yes, QMessageBox::Cancel);
        if (choice == QMessageBox::Yes) {
            vcsService_->resetTo(id, FfiResetMode::Hard);
        }
    });
    connect(copyHashAction, &QAction::triggered, this,
            [id]() { QGuiApplication::clipboard()->setText(id); });

    menu->popup(globalPos);
}

CommitLogPanel *buildCommitLogDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                    ads::CDockAreaWidget *relativeTo, VcsService *vcsService,
                                    std::function<void(const QString &)> openCommit)
{
    auto *panel = new CommitLogPanel(vcsService, std::move(openCommit), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Commit Log"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("commitLog"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("commitLog"));
    return panel;
}

} // namespace ui_shell
