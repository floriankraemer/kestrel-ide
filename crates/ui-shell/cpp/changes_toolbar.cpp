#include "changes_toolbar.h"

#include "e2e_mark.h"
#include "theme.h"
#include "ui_tokens.h"
#include "vcs_menu.h"

#include <QHBoxLayout>
#include <QPoint>
#include <QShowEvent>
#include <QToolButton>

namespace ui_shell {

namespace {

QToolButton *iconButton(const char *mask, const QString &text, QWidget *parent)
{
    auto *button = new QToolButton(parent);
    button->setIcon(maskIcon(mask, chromePaletteForTheme(activeThemeName()).textDim));
    button->setIconSize(QSize(16, 16));
    button->setAutoRaise(true);
    button->setFocusPolicy(Qt::NoFocus);
    if (!text.isEmpty()) {
        button->setText(text);
        button->setToolButtonStyle(Qt::ToolButtonTextBesideIcon);
    }
    return button;
}

} // namespace

ChangesToolbar::ChangesToolbar(VcsService *vcsService, QWidget *parent)
  : QWidget(parent)
  , vcsService_(vcsService)
{
    setObjectName(QStringLiteral("changesToolbar"));

    // "⎇" (U+2387), the same branch glyph the mockup and every other Git UI
    // this plan measured against (VS/JetBrains) use in place of an icon.
    branchChip_ = new QToolButton(this);
    branchChip_->setAutoRaise(true);
    branchChip_->setFocusPolicy(Qt::NoFocus);
    connect(branchChip_, &QToolButton::clicked, vcsService_, [vcsService, this]() {
        showBranchMenu(vcsService, branchChip_,
                        branchChip_->mapToGlobal(QPoint(0, branchChip_->height())));
    });

    refreshButton_ = iconButton(":/ui/icons/diff/sync.a8", QString(), this);
    refreshButton_->setToolTip(tr("Refresh (git status)"));
    connect(refreshButton_, &QToolButton::clicked, this, [this]() {
        // Re-enabled the moment an answer lands, whether it is this click's
        // own refresh or one already queued behind it — `VcsService` runs
        // its jobs FIFO on one worker thread, so there is never more than
        // one outstanding request to wait for either way.
        refreshButton_->setEnabled(false);
        emit refreshRequested();
    });
    connect(vcsService_, &VcsService::statusChanged, refreshButton_,
            [this]() { refreshButton_->setEnabled(true); });
    connect(vcsService_, &VcsService::vcsFailed, refreshButton_,
            [this]() { refreshButton_->setEnabled(true); });

    fetchButton_ = iconButton(":/ui/icons/diff/sync.a8", tr("Fetch"), this);
    connect(fetchButton_, &QToolButton::clicked, this, &ChangesToolbar::fetchRequested);

    pullButton_ = iconButton(":/ui/icons/diff/arrow_down.a8", tr("Pull"), this);
    connect(pullButton_, &QToolButton::clicked, this, &ChangesToolbar::pullRequested);

    pushButton_ = iconButton(":/ui/icons/diff/arrow_up.a8", tr("Push"), this);
    connect(pushButton_, &QToolButton::clicked, this, &ChangesToolbar::pushRequested);

    // Text buttons, `tests_panel.cpp`'s Run All/Run Failed/Stop convention.
    stageAllButton_ = new QToolButton(this);
    stageAllButton_->setText(tr("Stage all"));
    connect(stageAllButton_, &QToolButton::clicked, this, &ChangesToolbar::stageAllRequested);

    unstageAllButton_ = new QToolButton(this);
    unstageAllButton_->setText(tr("Unstage all"));
    connect(unstageAllButton_, &QToolButton::clicked, this, &ChangesToolbar::unstageAllRequested);

    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(tokens::kSp2, tokens::kSp1, tokens::kSp2, tokens::kSp1);
    layout->setSpacing(tokens::kSp1);
    layout->addWidget(branchChip_);
    layout->addSpacing(tokens::kSp3);
    layout->addWidget(refreshButton_);
    layout->addWidget(fetchButton_);
    layout->addWidget(pullButton_);
    layout->addWidget(pushButton_);
    layout->addSpacing(tokens::kSp3);
    layout->addWidget(stageAllButton_);
    layout->addWidget(unstageAllButton_);
    layout->addStretch(1);

    connect(vcsService_, &VcsService::statusChanged, this, &ChangesToolbar::refresh);
    connect(vcsService_, &VcsService::branchChanged, this, &ChangesToolbar::refresh);
    connect(vcsService_, &VcsService::repositoryChanged, this, &ChangesToolbar::refresh);
    refresh();
}

void ChangesToolbar::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    markShown();
}

void ChangesToolbar::refresh()
{
    const bool isRepo = vcsService_->isRepository();
    setVisible(isRepo);
    if (!isRepo) {
        return;
    }

    const FfiBranchStatus status = vcsService_->branchStatus();
    const QString branch = status.detached || status.branch.isEmpty() ? tr("(detached)")
                                                                        : status.branch;
    branchChip_->setText(tr("⎇ %1  ↓%2 ↑%3")
                            .arg(branch)
                            .arg(status.behind)
                            .arg(status.ahead));

    // Tooltips spell the actual git command, the same convention the plan
    // asks this toolbar to introduce.
    static const QString kRemote = QStringLiteral("origin");
    fetchButton_->setToolTip(tr("git fetch %1").arg(kRemote));

    pullButton_->setText(status.behind > 0 ? tr("Pull %1").arg(status.behind) : tr("Pull"));
    pullButton_->setToolTip(tr("git pull %1 %2").arg(kRemote, branch));
    pullButton_->setEnabled(!status.detached && status.has_upstream);

    pushButton_->setText(status.ahead > 0 ? tr("Push %1").arg(status.ahead) : tr("Push"));
    pushButton_->setToolTip(status.has_upstream ? tr("git push %1 %2").arg(kRemote, branch)
                                                 : tr("git push -u %1 %2").arg(kRemote, branch));
    pushButton_->setEnabled(!status.detached && (status.ahead > 0 || !status.has_upstream));

    // Stage all / Unstage all disable only when there is genuinely nothing
    // to do — the same `changedFiles()` the tree itself reads, so this
    // widget answers from the one list rather than keeping a second count.
    bool hasStaged = false;
    bool hasUnstaged = false;
    for (const FfiChangedFile &file : vcsService_->changedFiles()) {
        hasStaged = hasStaged || file.staged != FfiChangeKind::None;
        hasUnstaged = hasUnstaged || file.unstaged != FfiChangeKind::None;
    }
    stageAllButton_->setEnabled(hasUnstaged);
    unstageAllButton_->setEnabled(hasStaged);

    markShown();
}

void ChangesToolbar::markShown()
{
    // Same reasoning as `ChangesPanel::showEvent`/`markChangesRow`: an E2E
    // flow reads a control's on-screen rect from here rather than computing
    // one from the toolbar's own layout metrics. Re-emitted on every
    // `refresh()`, not only the first `showEvent`, so a flow can watch
    // `ahead`/`behind` move — after a commit, a pull, or a push — the same
    // way `changes_row` is re-marked on every `ChangesPanel::refresh()`.
    const FfiBranchStatus status = vcsService_->branchStatus();
    const QRect refreshRect(refreshButton_->mapToGlobal(QPoint(0, 0)), refreshButton_->size());
    const QRect pullRect(pullButton_->mapToGlobal(QPoint(0, 0)), pullButton_->size());
    const QRect pushRect(pushButton_->mapToGlobal(QPoint(0, 0)), pushButton_->size());
    e2eMark(QStringLiteral("{\"ev\":\"changes_toolbar_shown\",\"ahead\":%1,\"behind\":%2,"
                            "\"refresh_rect\":[%3,%4,%5,%6],"
                            "\"pull_rect\":[%7,%8,%9,%10],"
                            "\"push_rect\":[%11,%12,%13,%14]}")
              .arg(status.ahead)
              .arg(status.behind)
              .arg(refreshRect.x())
              .arg(refreshRect.y())
              .arg(refreshRect.width())
              .arg(refreshRect.height())
              .arg(pullRect.x())
              .arg(pullRect.y())
              .arg(pullRect.width())
              .arg(pullRect.height())
              .arg(pushRect.x())
              .arg(pushRect.y())
              .arg(pushRect.width())
              .arg(pushRect.height()));
}

} // namespace ui_shell
