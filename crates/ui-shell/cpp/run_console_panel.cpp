#include "run_console_panel.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "run_toolbar.h"
#include "styled_tab_widget.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QCheckBox>
#include <QColor>
#include <QCursor>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QAction>
#include <QMenu>
#include <QMouseEvent>
#include <QShortcut>
#include <QSignalBlocker>
#include <QPlainTextEdit>
#include <QScrollBar>
#include <QTabBar>
#include <QTabWidget>
#include <QTextCharFormat>
#include <QTextCursor>
#include <QTextEdit>
#include <QToolButton>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

// How many matches the find bar highlights at once. Beyond this the
// highlight is dropped and only the current match is shown — a console can
// hold megabytes, and painting a hundred thousand extra selections freezes
// the widget for longer than the search saved.
constexpr int kMaxHighlightedMatches = 2000;

// Read-only console text plus Ctrl+hover/Ctrl+Click link activation. No
// Q_OBJECT: it has no signals/slots of its own, only two callbacks set once
// at construction, so it needs no moc registration.
class ConsoleTextEdit : public QPlainTextEdit
{
public:
    using HoverCallback = std::function<void(int, bool)>;
    using ActivateCallback = std::function<void(int)>;

    explicit ConsoleTextEdit(QWidget *parent) : QPlainTextEdit(parent)
    {
        setReadOnly(true);
        // Deliberately unbounded (R2-3): `RunService` bounds the text and
        // says so through `consoleTrimmed`, and a second, tighter limit
        // here would silently drop lines it still holds offsets into.
        setMaximumBlockCount(0);
        setMouseTracking(true);
        setLineWrapMode(QPlainTextEdit::NoWrap);
    }

    void setHoverCallback(HoverCallback callback) { hover_ = std::move(callback); }
    void setActivateCallback(ActivateCallback callback) { activate_ = std::move(callback); }

protected:
    void mouseMoveEvent(QMouseEvent *event) override
    {
        QPlainTextEdit::mouseMoveEvent(event);
        if (hover_) {
            hover_(cursorForPosition(event->pos()).position(),
                   event->modifiers().testFlag(Qt::ControlModifier));
        }
    }

    void mousePressEvent(QMouseEvent *event) override
    {
        if (event->button() == Qt::LeftButton
            && event->modifiers().testFlag(Qt::ControlModifier) && activate_) {
            activate_(cursorForPosition(event->pos()).position());
            event->accept();
            return;
        }
        QPlainTextEdit::mousePressEvent(event);
    }

private:
    HoverCallback hover_;
    ActivateCallback activate_;
};

// Appends `text` at the end of the document and returns the position it
// starts at — what `applyStyledRuns` offsets into. The view follows the tail
// only if it was already there and the user has not locked the scroll
// (R2-3).
int appendLine(QPlainTextEdit *edit, const QString &text, bool followTail)
{
    QScrollBar *scrollBar = edit->verticalScrollBar();
    const bool wasAtBottom = scrollBar->value() >= scrollBar->maximum();

    QTextCursor cursor = edit->textCursor();
    cursor.movePosition(QTextCursor::End);
    const int base = cursor.position();
    cursor.insertText(text);

    if (wasAtBottom && followTail) {
        scrollBar->setValue(scrollBar->maximum());
    }
    return base;
}

// Paints one chunk's SGR styling (R2-1). Which spans are styled, and how,
// is `run-core`'s answer via `consoleStyleRuns`; this only turns each run
// into a `QTextCharFormat`. A run with no colour of its own keeps the
// palette's — `has_fg`/`has_bg` false means "the view's default", so the
// format simply leaves that half unset.
void applyStyledRuns(QPlainTextEdit *edit, int base, const rust::Vec<FfiStyledRun> &runs)
{
    for (const FfiStyledRun &run : runs) {
        QTextCharFormat format;
        if (run.has_fg) {
            format.setForeground(QColor(run.fg_r, run.fg_g, run.fg_b));
        }
        if (run.has_bg) {
            format.setBackground(QColor(run.bg_r, run.bg_g, run.bg_b));
        }
        if (run.bold) {
            format.setFontWeight(QFont::Bold);
        }
        if (run.italic) {
            format.setFontItalic(true);
        }
        if (run.underline) {
            format.setFontUnderline(true);
        }

        QTextCursor cursor(edit->document());
        cursor.setPosition(base + static_cast<int>(run.start));
        cursor.setPosition(base + static_cast<int>(run.start) + static_cast<int>(run.length),
                           QTextCursor::KeepAnchor);
        cursor.mergeCharFormat(format);
    }
}

} // namespace

RunConsolePanel::RunConsolePanel(RunService *runService, RunToolbar *toolbar, OpenAt openAt,
                                 QWidget *parent)
  : QWidget(parent)
  , runService_(runService)
  , openAt_(std::move(openAt))
  , toolbar_(toolbar)
{
    tabs_ = new StyledTabWidget(this);
    tabs_->setTabsClosable(true);
    connect(tabs_, &QTabWidget::tabCloseRequested, this, &RunConsolePanel::closeTab);
    connect(tabs_, &QTabWidget::currentChanged, this,
            [this](int) { updateTabControls(); });

    auto *controls = new QWidget(this);
    auto *controlsLayout = new QHBoxLayout(controls);
    controlsLayout->setContentsMargins(4, 2, 4, 2);
    controlsLayout->setSpacing(8);

    pinButton_ = new QToolButton(controls);
    pinButton_->setText(tr("Pin"));
    pinButton_->setCheckable(true);
    pinButton_->setToolTip(tr("Keep this tab: it gets no close button"));
    connect(pinButton_, &QToolButton::toggled, this, &RunConsolePanel::togglePinned);

    scrollLock_ = new QCheckBox(tr("Scroll lock"), controls);
    scrollLock_->setToolTip(tr("Stop following the end of the output"));

    auto *clearButton = new QToolButton(controls);
    clearButton->setText(tr("Clear"));
    clearButton->setToolTip(tr("Discard this console's output"));
    connect(clearButton, &QToolButton::clicked, this, &RunConsolePanel::clearCurrentConsole);

    controlsLayout->addWidget(pinButton_);
    controlsLayout->addWidget(scrollLock_);
    controlsLayout->addWidget(clearButton);
    controlsLayout->addStretch(1);

    // The find bar, hidden until Ctrl+F. Enter and Shift+Enter step through
    // matches the way every other find bar in this app does.
    findBar_ = new QWidget(this);
    auto *findLayout = new QHBoxLayout(findBar_);
    findLayout->setContentsMargins(4, 2, 4, 2);
    findLayout->setSpacing(6);
    findField_ = new QLineEdit(findBar_);
    findField_->setPlaceholderText(tr("Find in console"));
    findStatus_ = new QLabel(findBar_);
    auto *findNext = new QToolButton(findBar_);
    findNext->setText(tr("Next"));
    auto *findPrevious = new QToolButton(findBar_);
    findPrevious->setText(tr("Previous"));
    auto *findClose = new QToolButton(findBar_);
    findClose->setText(tr("Close"));
    findLayout->addWidget(findField_, 1);
    findLayout->addWidget(findStatus_);
    findLayout->addWidget(findPrevious);
    findLayout->addWidget(findNext);
    findLayout->addWidget(findClose);
    findBar_->hide();

    connect(findField_, &QLineEdit::textChanged, this, [this](const QString &) { runFind(0); });
    connect(findField_, &QLineEdit::returnPressed, this, [this]() { runFind(1); });
    connect(findNext, &QToolButton::clicked, this, [this]() { runFind(1); });
    connect(findPrevious, &QToolButton::clicked, this, [this]() { runFind(-1); });
    connect(findClose, &QToolButton::clicked, this, [this]() {
        findBar_->hide();
        if (ConsoleTab *tab = currentTab()) {
            tab->edit->setExtraSelections({});
        }
    });

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(controls);
    layout->addWidget(tabs_, 1);
    layout->addWidget(findBar_);

    // Widget-scoped, so it beats the editor's window-level Find while the
    // console has focus and leaves it alone otherwise.
    auto *findShortcut = new QShortcut(QKeySequence::Find, this);
    findShortcut->setContext(Qt::WidgetWithChildrenShortcut);
    connect(findShortcut, &QShortcut::activated, this, &RunConsolePanel::showFindBar);

    connect(runService_, &RunService::consoleStarted, this, &RunConsolePanel::onConsoleStarted);
    connect(runService_, &RunService::consoleOutput, this, &RunConsolePanel::onConsoleOutput);
    connect(runService_, &RunService::consoleTrimmed, this, &RunConsolePanel::onConsoleTrimmed);
    connect(runService_, &RunService::consoleFinished, this, &RunConsolePanel::onConsoleFinished);
    updateTabControls();
}

RunConsolePanel::ConsoleTab *RunConsolePanel::currentTab()
{
    const quint64 id = currentConsoleId();
    const auto it = consoles_.find(id);
    return it == consoles_.end() ? nullptr : &it.value();
}

quint64 RunConsolePanel::currentConsoleId() const
{
    QWidget *current = tabs_->currentWidget();
    if (current == nullptr) {
        return 0;
    }
    for (auto it = consoles_.constBegin(); it != consoles_.constEnd(); ++it) {
        if (it->edit == current) {
            return it.key();
        }
    }
    return 0;
}

void RunConsolePanel::onConsoleStarted(quint64 consoleId, const QString &configId)
{
    auto *edit = new ConsoleTextEdit(tabs_);
    edit->setHoverCallback([this, edit, consoleId](int position, bool ctrlHeld) {
        const bool linked =
          ctrlHeld && runService_->resolveLink(consoleId, static_cast<quint32>(position)).found;
        edit->viewport()->setCursor(linked ? Qt::PointingHandCursor : Qt::IBeamCursor);
    });
    edit->setActivateCallback(
      [this, consoleId](int position) { onLinkActivated(consoleId, position); });

    consoles_.insert(consoleId, ConsoleTab{edit, /*pinned=*/false, /*finished=*/false});
    tabs_->addTab(edit, configId);
    tabs_->setCurrentWidget(edit);
    updateTabControls();

    e2eMark(QStringLiteral("{\"ev\":\"run_console_tab_added\",\"console_id\":%1,\"config_id\":%2}")
              .arg(consoleId)
              .arg(e2eJson(configId)));
}

void RunConsolePanel::onConsoleOutput(quint64 consoleId, const QString &text)
{
    const auto it = consoles_.constFind(consoleId);
    if (it != consoles_.constEnd()) {
        const int base = appendLine(it->edit, text, !scrollLock_->isChecked());
        applyStyledRuns(it->edit, base, runService_->consoleStyleRuns(consoleId));
    }
    e2eMark(QStringLiteral("{\"ev\":\"run_console_output\",\"console_id\":%1,\"text\":%2}")
              .arg(consoleId)
              .arg(e2eJson(text)));
}

void RunConsolePanel::onConsoleTrimmed(quint64 consoleId, quint32 utf16Units)
{
    const auto it = consoles_.constFind(consoleId);
    if (it == consoles_.constEnd()) {
        return;
    }
    // Exactly as many code units as the cache dropped, off the same end:
    // the two documents are the same text or the offsets mean nothing.
    QTextCursor cursor(it->edit->document());
    cursor.setPosition(0);
    cursor.setPosition(static_cast<int>(utf16Units), QTextCursor::KeepAnchor);
    cursor.removeSelectedText();
}

void RunConsolePanel::onConsoleFinished(quint64 consoleId, int exitCode, bool escaped)
{
    const auto it = consoles_.find(consoleId);
    if (it == consoles_.end()) {
        return;
    }
    it->finished = true;
    QString line = exitCode >= 0
      ? tr("\nProcess finished with exit code %1\n").arg(exitCode)
      : tr("\nProcess stopped\n");
    if (escaped) {
        line += tr("Some child processes could not be terminated.\n");
    }
    // The one piece of text in this document `RunService` does not hold.
    // It is safe precisely because it is last: a finished console receives
    // no further output, so it shifts no offset anything will ask about.
    appendLine(it->edit, line, !scrollLock_->isChecked());
    updateTabControls();

    e2eMark(QStringLiteral(
              "{\"ev\":\"run_console_finished\",\"console_id\":%1,\"exit_code\":%2,\"escaped\":%3}")
              .arg(consoleId)
              .arg(exitCode)
              .arg(escaped ? "true" : "false"));
}

void RunConsolePanel::onLinkActivated(quint64 consoleId, int textPosition)
{
    const FfiResolvedLink link =
      runService_->resolveLink(consoleId, static_cast<quint32>(textPosition));
    e2eMark(QStringLiteral("{\"ev\":\"run_console_link_activated\",\"position\":%1,"
                            "\"found\":%2,\"path\":%3,\"line\":%4}")
              .arg(textPosition)
              .arg(link.found ? "true" : "false")
              .arg(e2eJson(link.path))
              .arg(link.line));
    if (link.found && openAt_) {
        openAt_(link.path, static_cast<int>(link.line),
                link.has_column ? static_cast<int>(link.column) : 0);
    }
}

void RunConsolePanel::clearCurrentConsole()
{
    ConsoleTab *tab = currentTab();
    if (tab == nullptr) {
        return;
    }
    // Both sides forget together — see `RunService::clearConsole`.
    runService_->clearConsole(currentConsoleId());
    tab->edit->clear();
    e2eMark(QStringLiteral("{\"ev\":\"run_console_cleared\",\"console_id\":%1}")
              .arg(currentConsoleId()));
}

void RunConsolePanel::togglePinned(bool pinned)
{
    ConsoleTab *tab = currentTab();
    if (tab == nullptr) {
        return;
    }
    tab->pinned = pinned;
    updateTabControls();
}

void RunConsolePanel::closeTab(int index)
{
    QWidget *widget = tabs_->widget(index);
    quint64 consoleId = 0;
    for (auto it = consoles_.constBegin(); it != consoles_.constEnd(); ++it) {
        if (it->edit == widget) {
            consoleId = it.key();
            break;
        }
    }
    if (consoleId == 0) {
        return;
    }
    if (!consoles_[consoleId].finished) {
        // Stop first, close when it is actually over: a tab that vanished
        // while its process kept running would leave the user with no way
        // back to it. `consoleFinished` is what re-enables the button.
        runService_->stop(consoleId);
        return;
    }
    runService_->closeConsole(consoleId);
    consoles_.remove(consoleId);
    tabs_->removeTab(index);
    delete widget;
    updateTabControls();
}

void RunConsolePanel::updateTabControls()
{
    ConsoleTab *tab = currentTab();
    pinButton_->setEnabled(tab != nullptr);
    scrollLock_->setEnabled(tab != nullptr);
    if (tab != nullptr) {
        QSignalBlocker blocker(pinButton_);
        pinButton_->setChecked(tab->pinned);
    }

    // A pinned tab has no close button; an unfinished one closes by being
    // stopped first, which is why it keeps its button.
    QTabBar *bar = tabs_->tabBar();
    for (int index = 0; index < tabs_->count(); ++index) {
        QWidget *widget = tabs_->widget(index);
        bool pinned = false;
        for (auto it = consoles_.constBegin(); it != consoles_.constEnd(); ++it) {
            if (it->edit == widget) {
                pinned = it->pinned;
                break;
            }
        }
        QWidget *button = bar->tabButton(index, QTabBar::RightSide);
        if (button != nullptr) {
            button->setVisible(!pinned);
        }
    }
}

void RunConsolePanel::showFindBar()
{
    if (currentTab() == nullptr) {
        return;
    }
    findBar_->show();
    findField_->setFocus();
    findField_->selectAll();
    runFind(0);
}

void RunConsolePanel::runFind(int direction)
{
    ConsoleTab *tab = currentTab();
    if (tab == nullptr) {
        return;
    }
    const QString pattern = findField_->text();
    if (pattern.isEmpty()) {
        tab->edit->setExtraSelections({});
        findStatus_->clear();
        return;
    }

    // Where the matches are is `editor_core::search`'s answer, reached
    // through `RunService` — the console does not run a matcher of its own.
    const rust::Vec<FfiTextMatch> matches =
      runService_->findInConsole(currentConsoleId(), pattern, /*case_sensitive=*/false);
    findStatus_->setText(matches.empty()
                           ? tr("No matches")
                           : tr("%1 matches").arg(static_cast<int>(matches.size())));
    if (matches.empty()) {
        tab->edit->setExtraSelections({});
        return;
    }

    QList<QTextEdit::ExtraSelection> highlights;
    if (static_cast<int>(matches.size()) <= kMaxHighlightedMatches) {
        QTextCharFormat format;
        format.setBackground(tab->edit->palette().highlight().color().lighter(140));
        for (const FfiTextMatch &match : matches) {
            QTextEdit::ExtraSelection selection;
            selection.format = format;
            selection.cursor = QTextCursor(tab->edit->document());
            selection.cursor.setPosition(static_cast<int>(match.start));
            selection.cursor.setPosition(static_cast<int>(match.end), QTextCursor::KeepAnchor);
            highlights.append(selection);
        }
    }
    tab->edit->setExtraSelections(highlights);

    const int caret = tab->edit->textCursor().position();
    int target = -1;
    if (direction == 0) {
        // Typing in the field: show the first match rather than walking
        // away from where the user is looking.
        target = static_cast<int>(matches.front().start);
    } else if (direction > 0) {
        for (const FfiTextMatch &match : matches) {
            if (static_cast<int>(match.start) > caret) {
                target = static_cast<int>(match.start);
                break;
            }
        }
        if (target < 0) {
            target = static_cast<int>(matches.front().start);
        }
    } else {
        for (const FfiTextMatch &match : matches) {
            if (static_cast<int>(match.start) < caret) {
                target = static_cast<int>(match.start);
            }
        }
        if (target < 0) {
            target = static_cast<int>(matches.back().start);
        }
    }

    QTextCursor cursor(tab->edit->document());
    cursor.setPosition(target);
    tab->edit->setTextCursor(cursor);
    tab->edit->ensureCursorVisible();
    e2eMark(QStringLiteral("{\"ev\":\"run_console_find\",\"pattern\":%1,\"matches\":%2}")
              .arg(e2eJson(pattern))
              .arg(matches.size()));
}

void RunConsolePanel::showRunningList()
{
    QMenu menu(this);
    const rust::Vec<FfiRunningConsole> rows = runService_->activeConsoles();
    if (rows.empty()) {
        menu.addAction(tr("Nothing has been run yet"))->setEnabled(false);
    }
    for (const FfiRunningConsole &row : rows) {
        const QString label = row.running
          ? tr("%1 — running").arg(QString(row.config_id))
          : tr("%1 — finished").arg(QString(row.config_id));
        const quint64 consoleId = row.console_id;
        QAction *action = menu.addAction(label);
        connect(action, &QAction::triggered, this, [this, consoleId]() {
            const auto it = consoles_.constFind(consoleId);
            if (it != consoles_.constEnd()) {
                tabs_->setCurrentWidget(it->edit);
            }
        });
    }
    e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"running_list\",\"rows\":%1}")
              .arg(rows.size()));
    menu.exec(QCursor::pos());
    e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"running_list\"}");
}

void RunConsolePanel::runSelected()
{
    toolbar_->runSelected();
}

void RunConsolePanel::stopSelected()
{
    toolbar_->stopSelected();
}

void RunConsolePanel::killSelected()
{
    toolbar_->killSelected();
}

void RunConsolePanel::rerunSelected()
{
    toolbar_->rerunSelected();
}

void RunConsolePanel::debugSelected()
{
    toolbar_->debugSelected();
}

void RunConsolePanel::focusConfigSelector()
{
    toolbar_->focusConfigSelector();
}

RunConsolePanel *buildRunConsoleDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *relativeTo, RunToolbar *toolbar,
                                     RunConsolePanel::OpenAt openAt)
{
    auto *panel = new RunConsolePanel(toolbar->runService(), toolbar, std::move(openAt), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Run"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("runConsole"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("runConsole"));
    return panel;
}

} // namespace ui_shell
