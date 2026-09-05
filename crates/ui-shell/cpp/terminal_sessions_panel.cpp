#include "terminal_sessions_panel.h"

#include "terminal_widget.h"

#include <QAction>
#include <QKeySequence>
#include <QMenu>
#include <QTabWidget>
#include <QToolButton>
#include <QVBoxLayout>

namespace ui_shell {

TerminalSessionsPanel::TerminalSessionsPanel(TerminalSupervisor *supervisor,
                                              AppSettings *appSettings, OpenAt openAt,
                                              QWidget *parent)
  : QWidget(parent)
  , supervisor_(supervisor)
  , appSettings_(appSettings)
  , openAt_(std::move(openAt))
{
    tabs_ = new QTabWidget(this);
    tabs_->setTabsClosable(true);
    connect(tabs_, &QTabWidget::tabCloseRequested, this, &TerminalSessionsPanel::closeTab);

    newTabButton_ = new QToolButton(tabs_);
    newTabButton_->setText(QStringLiteral("+"));
    newTabButton_->setToolTip(tr("New Terminal Tab"));
    // MenuButtonPopup, not InstantPopup: the common case is "another one of
    // what I already have", which stays a single click, and the dropdown
    // arrow beside it is where picking a different shell lives.
    newTabButton_->setPopupMode(QToolButton::MenuButtonPopup);
    connect(newTabButton_, &QToolButton::clicked, this, [this]() { addSession(); });

    shellMenu_ = new QMenu(newTabButton_);
    // Rebuilt on every open: a WSL distro installed while the IDE is
    // running should show up without a restart.
    connect(shellMenu_, &QMenu::aboutToShow, this, &TerminalSessionsPanel::refreshShellMenu);
    newTabButton_->setMenu(shellMenu_);

    tabs_->setCornerWidget(newTabButton_, Qt::TopRightCorner);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(tabs_);

    newSessionAction_ = new QAction(tr("New Terminal Tab"), this);
    newSessionAction_->setShortcut(QKeySequence(
      appSettings_->shortcutFor(QStringLiteral("terminal.newSession")), QKeySequence::PortableText));
    // WithChildren: focus normally lives on a tab's TerminalWidget, a child
    // of this panel, not the panel itself.
    newSessionAction_->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    connect(newSessionAction_, &QAction::triggered, this, [this]() { addSession(); });
    addAction(newSessionAction_);

    selectShellAction_ = new QAction(tr("Select Shell..."), this);
    selectShellAction_->setShortcut(QKeySequence(
      appSettings_->shortcutFor(QStringLiteral("terminal.selectShell")),
      QKeySequence::PortableText));
    selectShellAction_->setShortcutContext(Qt::WidgetWithChildrenShortcut);
    connect(selectShellAction_, &QAction::triggered, newTabButton_, &QToolButton::showMenu);
    addAction(selectShellAction_);

    // A terminal dock is never empty: exactly like the single-session
    // predecessor of this class, there is always at least one shell ready
    // to use as soon as the dock is shown.
    addSession();
}

void TerminalSessionsPanel::refreshShellMenu()
{
    shellMenu_->clear();
    for (const FfiShellCandidate &shell : supervisor_->availableShells()) {
        const QString id = shell.id;
        const QString label = shell.label;
        connect(shellMenu_->addAction(label), &QAction::triggered, this,
                [this, id, label]() { addSession(id, label); });
    }
}

void TerminalSessionsPanel::addSession(const QString &shellId, const QString &label)
{
    const quint64 sessionId = supervisor_->newSession();
    auto *widget =
      new TerminalWidget(supervisor_, sessionId, shellId, appSettings_, openAt_, tabs_);
    ++sessionCounter_;
    // A tab opened from the dropdown is named after the shell it runs,
    // which is the only thing distinguishing it from its neighbours; the
    // default tab keeps the plain counter.
    const QString title = label.isEmpty() ? tr("Terminal %1").arg(sessionCounter_) : label;
    const int index = tabs_->addTab(widget, title);
    tabs_->setCurrentIndex(index);
    widget->setFocus();
}

void TerminalSessionsPanel::focusCurrent()
{
    if (auto *widget = qobject_cast<TerminalWidget *>(tabs_->currentWidget())) {
        widget->setFocus();
    }
}

void TerminalSessionsPanel::reapplyKeymap()
{
    // newSessionAction_ and selectShellAction_ need no update here: both
    // live in the app-wide `actions` map (see `newSessionAction()`'s doc
    // comment), so `applyKeymap()` already re-reads their shortcuts on the
    // same OK click.
    for (int i = 0; i < tabs_->count(); ++i) {
        if (auto *widget = qobject_cast<TerminalWidget *>(tabs_->widget(i))) {
            widget->reapplyKeymap();
        }
    }
}

void TerminalSessionsPanel::closeTab(int index)
{
    auto *widget = qobject_cast<TerminalWidget *>(tabs_->widget(index));
    if (!widget) {
        return;
    }
    const quint64 sessionId = widget->sessionId();
    tabs_->removeTab(index);
    widget->deleteLater();
    supervisor_->closeSession(sessionId);

    // Never leave the dock with no tabs at all — same "always one shell
    // ready" rule the constructor's initial `addSession()` establishes.
    if (tabs_->count() == 0) {
        addSession();
    }
}

} // namespace ui_shell
