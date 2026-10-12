#include "tests_panel.h"

#include "keymap_page.h"
#include "code_editor.h"
#include "coverage_panel.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "editor_tabs.h"
#include "theme.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QKeySequence>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QMainWindow>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QPixmap>
#include <QPlainTextEdit>
#include <QPoint>
#include <QScrollBar>
#include <QKeyEvent>
#include <QSplitter>
#include <QStatusBar>
#include <QTextBlock>
#include <QTextCursor>
#include <QTextDocument>
#include <QTimer>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kKindRole = Qt::UserRole + 1;

// The display-side bound for the raw output pane, same reasoning
// `BuildPanel` gives its own: a chatty run must not grow the widget without
// limit.
constexpr int kMaxDisplayBlocks = 5000;

QString statusText(FfiTestStatusKind status)
{
    switch (status) {
    case FfiTestStatusKind::Failed:
        return QObject::tr("Failed");
    case FfiTestStatusKind::Running:
        return QObject::tr("Running");
    case FfiTestStatusKind::Pending:
        return QObject::tr("Pending");
    case FfiTestStatusKind::Passed:
        return QObject::tr("Passed");
    case FfiTestStatusKind::Skipped:
        return QObject::tr("Skipped");
    case FfiTestStatusKind::Terminated:
        return QObject::tr("Not finished");
    }
    return QString();
}

// A stable, English, machine-readable name for a status — never
// `statusText`'s `tr()`-wrapped display string, which a non-English UI
// locale would translate out from under anything reading the marker
// stream (ADR-0049).
const char *statusName(FfiTestStatusKind status)
{
    switch (status) {
    case FfiTestStatusKind::Failed:
        return "failed";
    case FfiTestStatusKind::Running:
        return "running";
    case FfiTestStatusKind::Pending:
        return "pending";
    case FfiTestStatusKind::Passed:
        return "passed";
    case FfiTestStatusKind::Skipped:
        return "skipped";
    case FfiTestStatusKind::Terminated:
        return "terminated";
    }
    return "";
}

QColor statusColor(FfiTestStatusKind status)
{
    // The product-wide semantic set (`theme.h`), the same rule
    // `severityColor` follows in `problems_panel.cpp`: a red test failure
    // and a red error diagnostic have to mean the same red.
    const SemanticColors colors = semanticColors();
    switch (status) {
    case FfiTestStatusKind::Failed:
        return colors.error;
    case FfiTestStatusKind::Running:
        return colors.info;
    case FfiTestStatusKind::Pending:
        return colors.muted;
    case FfiTestStatusKind::Passed:
        return colors.ok;
    case FfiTestStatusKind::Skipped:
        return colors.muted;
    case FfiTestStatusKind::Terminated:
        return colors.warning;
    }
    return QColor();
}

// A small filled circle in the status's colour — cheaper than a vendored
// icon set for five states, and it can never drift from `statusColor`'s
// palette the way a separate SVG per state could.
QIcon statusIcon(FfiTestStatusKind status, int logicalPx)
{
    QPixmap pixmap(logicalPx, logicalPx);
    pixmap.fill(Qt::transparent);
    QPainter painter(&pixmap);
    painter.setRenderHint(QPainter::Antialiasing);
    painter.setPen(Qt::NoPen);
    painter.setBrush(statusColor(status));
    const int margin = logicalPx / 4;
    painter.drawEllipse(margin, margin, logicalPx - 2 * margin, logicalPx - 2 * margin);
    return QIcon(pixmap);
}

QString durationText(qint64 durationMs)
{
    if (durationMs < 0) {
        return QString();
    }
    if (durationMs < 1000) {
        return QObject::tr("%1 ms").arg(durationMs);
    }
    return QObject::tr("%1 s").arg(durationMs / 1000.0, 0, 'f', 2);
}

// Read-only failure text plus Ctrl+hover/Ctrl+Click link activation — the
// same interaction `ConsoleTextEdit` (`run_console_panel.cpp`) gives run
// output, kept as its own small copy here rather than shared across a
// header: neither trims scrollback nor tracks a console id, so the two
// have nothing else in common to factor out.
class FailureTextEdit : public QPlainTextEdit
{
public:
    using HoverCallback = std::function<void(int, bool)>;
    using ActivateCallback = std::function<void(int)>;

    explicit FailureTextEdit(QWidget *parent) : QPlainTextEdit(parent)
    {
        setReadOnly(true);
        setMouseTracking(true);
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
        QPlainTextEdit::mousePressEvent(event);
        if (activate_ && event->modifiers().testFlag(Qt::ControlModifier)) {
            activate_(cursorForPosition(event->pos()).position());
        }
    }

private:
    HoverCallback hover_;
    ActivateCallback activate_;
};

} // namespace

TestsPanel::TestsPanel(TestService *testService, AppSettings *appSettings, OpenAt openAt,
                       OpenText openText, QWidget *parent)
  : QWidget(parent)
  , testService_(testService)
  , appSettings_(appSettings)
  , openAt_(std::move(openAt))
  , openText_(std::move(openText))
{
    runAllButton_ = new QToolButton(this);
    runAllButton_->setText(tr("Run All"));
    runFailedButton_ = new QToolButton(this);
    runFailedButton_->setText(tr("Run Failed"));
    stopButton_ = new QToolButton(this);
    stopButton_->setText(tr("Stop"));
    stopButton_->setEnabled(false);
    statusLabel_ = new QLabel(this);

    auto *toolbar = new QHBoxLayout();
    toolbar->addWidget(runAllButton_);
    toolbar->addWidget(runFailedButton_);
    toolbar->addWidget(stopButton_);
    toolbar->addWidget(statusLabel_, 1);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(2);
    tree_->setHeaderLabels({tr("Test"), tr("Duration")});
    tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    tree_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    tree_->setUniformRowHeights(true);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);

    failureHeader_ = new QLabel(this);
    failureHeader_->setTextInteractionFlags(Qt::TextSelectableByMouse);
    auto *failureEdit = new FailureTextEdit(this);
    failureDetails_ = failureEdit;
    failureEdit->setHoverCallback([this, failureEdit](int position, bool ctrlHeld) {
        const bool linked = ctrlHeld && selectedNodeId_ != QString()
          && failureLinkAt(position).found;
        failureEdit->viewport()->setCursor(linked ? Qt::PointingHandCursor : Qt::IBeamCursor);
    });
    failureEdit->setActivateCallback(
      [this](int position) { onFailureLinkActivated(position); });

    failureDetails_->setPlaceholderText(tr("Select a failed test to see why it failed."));

    output_ = new QPlainTextEdit(this);
    output_->setReadOnly(true);
    output_->setMaximumBlockCount(kMaxDisplayBlocks);
    output_->setLineWrapMode(QPlainTextEdit::NoWrap);

    auto *bottomTabs = new QSplitter(Qt::Horizontal, this);
    auto *failureBox = new QWidget(bottomTabs);
    auto *failureLayout = new QVBoxLayout(failureBox);
    failureLayout->setContentsMargins(0, 0, 0, 0);
    failureLayout->addWidget(failureHeader_);
    failureLayout->addWidget(failureDetails_, 1);
    bottomTabs->addWidget(failureBox);
    bottomTabs->addWidget(output_);
    bottomTabs->setStretchFactor(0, 1);
    bottomTabs->setStretchFactor(1, 1);

    auto *splitter = new QSplitter(Qt::Vertical, this);
    splitter->addWidget(tree_);
    splitter->addWidget(bottomTabs);
    splitter->setStretchFactor(0, 2);
    splitter->setStretchFactor(1, 1);
    // Stretch factors only govern *extra* space; the first layout still
    // comes from size hints, which split the panel about evenly and left
    // one visible tree row (#321). Seed the same 2:1 as the initial split.
    splitter->setSizes({200, 100});

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addLayout(toolbar);
    layout->addWidget(splitter, 1);

    connect(runAllButton_, &QToolButton::clicked, this, &TestsPanel::runAllTests);
    connect(runFailedButton_, &QToolButton::clicked, this, &TestsPanel::runFailedTests);
    connect(stopButton_, &QToolButton::clicked, this, &TestsPanel::stopTests);
    connect(tree_, &QTreeWidget::itemSelectionChanged, this, &TestsPanel::onSelectionChanged);
    connect(tree_, &QTreeWidget::customContextMenuRequested, this, &TestsPanel::showContextMenu);
    connect(tree_, &QTreeWidget::itemActivated, this,
            [this](QTreeWidgetItem *item) { jumpToSource(item->data(0, kIdRole).toString()); });
    tree_->installEventFilter(this);
    if (e2eMarksEnabled()) {
        // Rows move when a suite folds: report where they went, once per
        // turn of the event loop (a rebuild expands every suite).
        auto *remark = new QTimer(this);
        remark->setSingleShot(true);
        remark->setInterval(0);
        connect(remark, &QTimer::timeout, this, [this]() { markE2eRows(); });
        const auto start = [remark]() { remark->start(); };
        connect(tree_, &QTreeWidget::itemCollapsed, remark, start);
        connect(tree_, &QTreeWidget::itemExpanded, remark, start);
    }
    connect(testService_, &TestService::testRunPreparing, this, &TestsPanel::onTestRunPreparing);
    connect(testService_, &TestService::testRunStarted, this, &TestsPanel::onTestRunStarted);
    connect(testService_, &TestService::testRunStopped, this, &TestsPanel::onTestRunStopped);
    connect(testService_, &TestService::testTreeChanged, this, &TestsPanel::onTestTreeChanged);
    connect(testService_, &TestService::testOutputAppended, this,
            &TestsPanel::onTestOutputAppended);
    connect(testService_, &TestService::testRunFinished, this, &TestsPanel::onTestRunFinished);
    connect(testService_, &TestService::testRunRefused, this, &TestsPanel::onTestRunRefused);

    statusLabel_->setText(tr("No tests have run yet."));
}

void TestsPanel::runAllTests()
{
    report(testService_->runAll());
}

void TestsPanel::runFailedTests()
{
    report(testService_->runFailed());
}

void TestsPanel::stopTests()
{
    testService_->stop();
}

void TestsPanel::report(const FfiResult &result)
{
    if (result.code == 0) {
        return;
    }
    // A refusal is shown where a run's own status would have been —
    // "no test framework is contributed for this project" is the answer to
    // the same question a modal would ask, per `BuildPanel::report`.
    onTestRunRefused(result);
}

void TestsPanel::onTestRunPreparing()
{
    // Finding the framework may wait on WSL or a container: Stop ends it.
    stopButton_->setEnabled(true);
    statusLabel_->setText(tr("Preparing…"));
    e2eMark(QStringLiteral("{\"ev\":\"test_run_preparing\"}"));
}

void TestsPanel::onTestRunStopped()
{
    stopButton_->setEnabled(false);
    statusLabel_->setText(tr("Run stopped."));
    e2eMark(QStringLiteral("{\"ev\":\"test_run_stopped\"}"));
}

void TestsPanel::onTestRunStarted()
{
    stopButton_->setEnabled(true);
    output_->clear();
    statusLabel_->setText(tr("Running…"));
    e2eMark(QStringLiteral("{\"ev\":\"test_run_started\"}"));
}

void TestsPanel::onTestTreeChanged()
{
    // Only the rows that changed since the last call (#462): a rebuild per
    // streamed event was quadratic in the suite's size.
    const FfiTestNodeChanges changes = testService_->takeNodeChanges();
    bool selectedChanged = changes.reset;
    if (changes.reset) {
        // Every id currently expanded, so a new run of the same tests does
        // not collapse the tree the user is watching.
        expandedById_.clear();
        for (auto it = itemsById_.constBegin(); it != itemsById_.constEnd(); ++it) {
            expandedById_.insert(it.key(), it.value()->isExpanded());
        }
        tree_->clear();
        itemsById_.clear();
    }

    const int iconPx = tree_->fontMetrics().height();
    for (const FfiTestNode &node : changes.nodes) {
        const QString id = QString(node.id);
        QTreeWidgetItem *item = itemsById_.value(id);
        if (item == nullptr) {
            const QString parentId = QString(node.parentId);
            QTreeWidgetItem *parentItem = parentId.isEmpty() ? nullptr : itemsById_.value(parentId);
            item = new QTreeWidgetItem();
            item->setData(0, kIdRole, id);
            if (parentItem != nullptr) {
                parentItem->insertChild(qMin(int(node.index), parentItem->childCount()), item);
            } else {
                tree_->insertTopLevelItem(qMin(int(node.index), tree_->topLevelItemCount()), item);
            }
            item->setExpanded(expandedById_.value(id, true));
            itemsById_.insert(id, item);
        }
        item->setText(0, QString(node.name));
        item->setIcon(0, statusIcon(node.status, iconPx));
        item->setText(1, durationText(node.durationMs));
        item->setData(0, kKindRole, static_cast<int>(node.kind));
        selectedChanged = selectedChanged || id == selectedNodeId_;
        if (e2eMarksEnabled()) {
            e2eMark(QStringLiteral("{\"ev\":\"test_tree_row\",\"id\":%1,\"name\":%2,\"status\":%3,"
                                   "\"suite\":%4}")
                      .arg(e2eJson(id), e2eJson(QString(node.name)),
                           e2eJson(QString::fromUtf8(statusName(node.status))),
                           node.kind == FfiTestNodeKind::Suite ? QStringLiteral("true")
                                                               : QStringLiteral("false")));
        }
    }

    if (changes.reset && !selectedNodeId_.isEmpty()) {
        if (QTreeWidgetItem *item = itemsById_.value(selectedNodeId_)) {
            item->setSelected(true);
        }
    }
    // The failure pane follows its node: refreshed only when that node
    // changed, not on every streamed event.
    if (selectedChanged) {
        onSelectionChanged();
    }

    const int total = itemsById_.size();
    statusLabel_->setText(tr("%n test node(s).", nullptr, total));
    e2eMark(QStringLiteral("{\"ev\":\"test_tree_changed\",\"nodes\":%1}").arg(total));
}

void TestsPanel::markE2eRows() const
{
    QStringList rows;
    for (auto it = itemsById_.constBegin(); it != itemsById_.constEnd(); ++it) {
        const QRect rect = tree_->visualItemRect(it.value());
        const QPoint origin = tree_->viewport()->mapToGlobal(rect.topLeft());
        // Visible: inside the scrolled viewport, where a click lands on it.
        const bool visible = tree_->viewport()->rect().contains(rect.center());
        rows << QStringLiteral("{\"id\":%1,\"name\":%2,\"rect\":[%3,%4,%5,%6],\"visible\":%7}")
                  .arg(e2eJson(it.key()), e2eJson(it.value()->text(0)))
                  .arg(origin.x())
                  .arg(origin.y())
                  .arg(rect.width())
                  .arg(rect.height())
                  .arg(visible ? QStringLiteral("true") : QStringLiteral("false"));
    }
    // Shown: the dock is the current tab of its area, so the rects are on
    // screen rather than where a hidden tab would be.
    e2eMark(QStringLiteral("{\"ev\":\"test_tree_rects\",\"shown\":%1,\"rows\":[%2]}")
              .arg(tree_->isVisible() ? QStringLiteral("true") : QStringLiteral("false"))
              .arg(rows.join(QLatin1Char(','))));
}

void TestsPanel::markE2eToolbar() const
{
    struct ButtonEntry
    {
        const char *name;
        QToolButton *button;
    };
    const ButtonEntry entries[] = {
        { "run_all", runAllButton_ },
        { "run_failed", runFailedButton_ },
        { "stop", stopButton_ },
    };
    QStringList buttons;
    for (const ButtonEntry &entry : entries) {
        const QPoint origin = entry.button->mapToGlobal(QPoint(0, 0));
        const QSize size = entry.button->size();
        buttons << QStringLiteral("{\"name\":%1,\"rect\":[%2,%3,%4,%5]}")
                      .arg(e2eJson(QString::fromUtf8(entry.name)))
                      .arg(origin.x())
                      .arg(origin.y())
                      .arg(size.width())
                      .arg(size.height());
    }
    e2eMark(QStringLiteral("{\"ev\":\"tests_toolbar_rects\",\"buttons\":[%1]}")
              .arg(buttons.join(QLatin1Char(','))));
}

void TestsPanel::onTestOutputAppended(const QString &text)
{
    e2eMark(QStringLiteral("{\"ev\":\"test_output\",\"text\":%1}").arg(e2eJson(text)));
    QTextCursor cursor = output_->textCursor();
    cursor.movePosition(QTextCursor::End);
    cursor.insertText(text);
    output_->setTextCursor(cursor);
    output_->verticalScrollBar()->setValue(output_->verticalScrollBar()->maximum());
}

void TestsPanel::onTestRunFinished(bool ok, bool matchedNothing, const QString &message)
{
    stopButton_->setEnabled(false);
    statusLabel_->setText(!ok ? message : matchedNothing ? tr("No tests matched.") : tr("Run finished."));
    // The rows' rects once the finished tree is laid out, for a flow that
    // clicks a row the run just reported.
    if (e2eMarksEnabled()) {
        QTimer::singleShot(0, this, [this]() { markE2eRows(); });
    }
    e2eMark(QStringLiteral("{\"ev\":\"test_run_finished\",\"ok\":%1,\"matched_nothing\":%2,\"message\":%3}")
              .arg(ok ? "true" : "false")
              .arg(matchedNothing ? "true" : "false")
              .arg(e2eJson(message)));
}

void TestsPanel::onTestRunRefused(const FfiResult &result)
{
    stopButton_->setEnabled(testService_->isRunning());
    statusLabel_->setText(QString(result.message));
    e2eMark(QStringLiteral("{\"ev\":\"test_run_refused\",\"code\":%1,\"message\":%2}")
              .arg(result.code)
              .arg(e2eJson(QString(result.message))));
}

void TestsPanel::onSelectionChanged()
{
    QTreeWidgetItem *item = tree_->currentItem();
    selectedNodeId_ = item ? item->data(0, kIdRole).toString() : QString();
    if (selectedNodeId_.isEmpty()) {
        failureHeader_->clear();
        failureDetails_->clear();
        detailsOffset_ = 0;
        return;
    }
    const QString message = testService_->failureMessage(selectedNodeId_);
    const QString details = testService_->failureDetails(selectedNodeId_);
    failureHeader_->setText(message);
    // An equality assertion's two sides lead, as PhpStorm's Expected/Actual
    // do; the trace's links are resolved past them (`detailsOffset_`).
    const FfiTestComparison comparison = testService_->failureComparison(selectedNodeId_);
    const QString lead = comparison.found
      ? tr("Expected: %1\nActual:   %2\n\n").arg(QString(comparison.expected), QString(comparison.actual))
      : QString();
    detailsOffset_ = static_cast<int>(lead.size());
    failureDetails_->setPlainText(lead + details);
    if (e2eMarksEnabled()) {
        markE2eFailureLink();
        // A keyboard move scrolls the tree: report where the rows are now.
        QTimer::singleShot(0, this, [this]() { markE2eRows(); });
    }
}

void TestsPanel::markE2eFailureLink() const
{
    // The first line of the details that is a link, where a Ctrl+Click on it
    // lands: the failure pane has no other handle an E2E flow could click.
    QString rect = QStringLiteral("[]");
    for (QTextBlock block = failureDetails_->document()->begin(); block.isValid();
         block = block.next()) {
        const int position = block.position() + block.length() / 2;
        if (failureLinkAt(position).found) {
            QTextCursor cursor(failureDetails_->document());
            cursor.setPosition(position);
            const QRect local = failureDetails_->cursorRect(cursor);
            const QPoint origin = failureDetails_->viewport()->mapToGlobal(local.topLeft());
            rect = QStringLiteral("[%1,%2,%3,%4]")
                     .arg(origin.x())
                     .arg(origin.y())
                     .arg(qMax(local.width(), 2))
                     .arg(local.height());
            break;
        }
    }
    e2eMark(QStringLiteral("{\"ev\":\"test_failure_details\",\"id\":%1,\"message\":%2,"
                           "\"details\":%3,\"link_rect\":%4}")
              .arg(e2eJson(selectedNodeId_), e2eJson(failureHeader_->text()),
                   e2eJson(failureDetails_->toPlainText()), rect));
}

void TestsPanel::onFailureLinkActivated(int textPosition)
{
    if (selectedNodeId_.isEmpty()) {
        return;
    }
    const FfiResolvedLink link = failureLinkAt(textPosition);
    if (link.found && openAt_) {
        openAt_(link.path, static_cast<int>(link.line),
                link.has_column ? static_cast<int>(link.column) : 0);
    }
}

FfiResolvedLink TestsPanel::failureLinkAt(int textPosition) const
{
    if (textPosition < detailsOffset_) {
        return FfiResolvedLink{};
    }
    return testService_->resolveFailureLink(selectedNodeId_,
                                            static_cast<quint32>(textPosition - detailsOffset_));
}

QKeySequence TestsPanel::jumpToSourceKey() const
{
    return scopedShortcutKey(QStringLiteral("tests.jumpToSource"), appSettings_);
}

bool TestsPanel::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == tree_
        && (event->type() == QEvent::ShortcutOverride || event->type() == QEvent::KeyPress)) {
        const auto *key = static_cast<QKeyEvent *>(event);
        const QKeySequence bound = jumpToSourceKey();
        if (!bound.isEmpty() && QKeySequence(key->keyCombination()) == bound) {
            // Claiming the override keeps a window action on the same key
            // from taking it; the press itself then does the jump.
            if (event->type() == QEvent::KeyPress) {
                jumpToSource(selectedNodeId_);
            }
            event->accept();
            return true;
        }
    }
    return QWidget::eventFilter(watched, event);
}

void TestsPanel::jumpToSource(const QString &nodeId)
{
    if (nodeId.isEmpty() || !openAt_) {
        return;
    }
    const QString path = testService_->nodeSourcePath(nodeId);
    if (path.isEmpty()) {
        return;
    }
    const std::optional<QString> open = openText_ ? openText_(path) : std::nullopt;
    const FfiResolvedLink source =
      testService_->nodeSource(nodeId, open.value_or(QString()), open.has_value());
    if (source.found) {
        openAt_(source.path, static_cast<int>(source.line), 0);
    }
}

void TestsPanel::showContextMenu(const QPoint &pos)
{
    QTreeWidgetItem *item = tree_->itemAt(pos);
    if (!item) {
        return;
    }
    const QString id = item->data(0, kIdRole).toString();
    const auto kind = static_cast<FfiTestNodeKind>(item->data(0, kKindRole).toInt());
    const QString label =
      kind == FfiTestNodeKind::Suite ? tr("Rerun Suite") : tr("Rerun Test");

    QMenu menu(tree_);
    menu.setToolTipsVisible(true);
    QAction *jump = menu.addAction(tr("Jump to Source"));
    jump->setShortcut(jumpToSourceKey());
    jump->setEnabled(!testService_->nodeSourcePath(id).isEmpty());
    QAction *rerun = menu.addAction(label);
    // The framework cannot narrow a rerun to a tree node (decided in Rust):
    // show the action greyed out with the reason rather than refuse a click.
    const QString block = testService_->rerunBlock();
    rerun->setEnabled(block.isEmpty());
    rerun->setToolTip(block);
    e2eMarkMenuActions(&menu, "tests_menu_action");
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == jump) {
        jumpToSource(id);
    } else if (chosen == rerun) {
        report(testService_->runNode(id));
    }
}

std::optional<QString> openEditorText(const EditorTabs *editorTabs, const QString &path)
{
    const CodeEditor *editor = editorTabs->editorForPath(path);
    return editor != nullptr ? std::optional<QString>(editor->toPlainText()) : std::nullopt;
}

TestsPanel *buildTestsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                           ads::CDockAreaWidget *relativeTo, TestService *testService,
                           AppSettings *appSettings, TestsPanel::OpenAt openAt,
                           EditorTabs *editorTabs)
{
    const auto openText = [editorTabs](const QString &path) {
        return openEditorText(editorTabs, path);
    };
    auto *panel = new TestsPanel(testService, appSettings, openAt, openText, dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Tests"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("tests"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("tests"));
    // Every test run shows its dock, as PhpStorm's Run window comes forward
    // for a run started from the editor, the tree or a configuration (#446).
    QObject::connect(testService, &TestService::testRunPreparing, panel,
                     [docks]() { docks->show(QStringLiteral("tests")); });
    // Every refusal of a test run, whichever service found it, shows the
    // same way: in the Tests dock, raised, and on the status bar.
    QObject::connect(testService, &TestService::testRunRefused, panel,
                     [docks, dockManager](const FfiResult &result) {
                         docks->show(QStringLiteral("tests"));
                         if (auto *window = qobject_cast<QMainWindow *>(dockManager->window())) {
                             window->statusBar()->showMessage(QString(result.message), 6000);
                         }
                     });
    buildCoverageDock(dockManager, docks, relativeTo, testService, openAt);
    // E2E only: same reasoning as `containers_panel.cpp`'s identical
    // `visibilityChanged` connect on its own dock — the toolbar has no
    // real geometry until this dock is actually on screen and laid out.
    QObject::connect(dock, &ads::CDockWidget::visibilityChanged, panel, [panel](bool visible) {
        if (visible) {
            QTimer::singleShot(0, panel, [panel]() {
                panel->markE2eToolbar();
                panel->markE2eRows();
            });
        }
    });
    return panel;
}

} // namespace ui_shell
