#include "tests_panel.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "theme.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QPixmap>
#include <QPlainTextEdit>
#include <QPoint>
#include <QScrollBar>
#include <QSplitter>
#include <QTextCursor>
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
    }
    return QString();
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

TestsPanel::TestsPanel(TestService *testService, OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , testService_(testService)
  , openAt_(std::move(openAt))
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
          && testService_->resolveFailureLink(selectedNodeId_, static_cast<quint32>(position))
               .found;
        failureEdit->viewport()->setCursor(linked ? Qt::PointingHandCursor : Qt::IBeamCursor);
    });
    failureEdit->setActivateCallback(
      [this](int position) { onFailureLinkActivated(position); });

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

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addLayout(toolbar);
    layout->addWidget(splitter, 1);

    connect(runAllButton_, &QToolButton::clicked, this, &TestsPanel::runAllTests);
    connect(runFailedButton_, &QToolButton::clicked, this, &TestsPanel::runFailedTests);
    connect(stopButton_, &QToolButton::clicked, this, &TestsPanel::stopTests);
    connect(tree_, &QTreeWidget::itemSelectionChanged, this, &TestsPanel::onSelectionChanged);
    connect(tree_, &QTreeWidget::customContextMenuRequested, this, &TestsPanel::showContextMenu);
    connect(testService_, &TestService::testRunStarted, this, &TestsPanel::onTestRunStarted);
    connect(testService_, &TestService::testTreeChanged, this, &TestsPanel::onTestTreeChanged);
    connect(testService_, &TestService::testOutputAppended, this,
            &TestsPanel::onTestOutputAppended);
    connect(testService_, &TestService::testRunFinished, this, &TestsPanel::onTestRunFinished);

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
    statusLabel_->setText(QString(result.message));
    e2eMark(QStringLiteral("{\"ev\":\"test_run_refused\",\"code\":%1}").arg(result.code));
}

void TestsPanel::onTestRunStarted()
{
    stopButton_->setEnabled(true);
    output_->clear();
    statusLabel_->setText(tr("Running..."));
    e2eMark(QStringLiteral("{\"ev\":\"test_run_started\"}"));
}

void TestsPanel::onTestTreeChanged()
{
    // Every id currently expanded, so a long run's live progress does not
    // collapse the tree the user is watching — restored below by id rather
    // than by row, since row order can change as new siblings arrive.
    QHash<QString, bool> expandedById;
    for (auto it = itemsById_.constBegin(); it != itemsById_.constEnd(); ++it) {
        expandedById.insert(it.key(), it.value()->isExpanded());
    }

    tree_->clear();
    itemsById_.clear();

    const ::rust::Vec<FfiTestNode> nodes = testService_->nodes();
    const int iconPx = tree_->fontMetrics().height();
    for (const FfiTestNode &node : nodes) {
        const QString id = QString(node.id);
        const QString parentId = QString(node.parentId);
        QTreeWidgetItem *parentItem = parentId.isEmpty() ? nullptr : itemsById_.value(parentId);
        auto *item = parentItem ? new QTreeWidgetItem(parentItem) : new QTreeWidgetItem(tree_);
        item->setText(0, QString(node.name));
        item->setIcon(0, statusIcon(node.status, iconPx));
        item->setText(1, durationText(node.durationMs));
        item->setData(0, kIdRole, id);
        item->setData(0, kKindRole, static_cast<int>(node.kind));
        item->setExpanded(expandedById.value(id, true));
        itemsById_.insert(id, item);
    }

    if (!selectedNodeId_.isEmpty()) {
        if (QTreeWidgetItem *item = itemsById_.value(selectedNodeId_)) {
            item->setSelected(true);
        }
    }
    onSelectionChanged();

    const int total = itemsById_.size();
    statusLabel_->setText(tr("%n test node(s).", nullptr, total));
    e2eMark(QStringLiteral("{\"ev\":\"test_tree_changed\",\"nodes\":%1}").arg(total));
}

void TestsPanel::onTestOutputAppended(const QString &text)
{
    QTextCursor cursor = output_->textCursor();
    cursor.movePosition(QTextCursor::End);
    cursor.insertText(text);
    output_->setTextCursor(cursor);
    output_->verticalScrollBar()->setValue(output_->verticalScrollBar()->maximum());
}

void TestsPanel::onTestRunFinished(bool ok, const QString &message)
{
    stopButton_->setEnabled(false);
    statusLabel_->setText(ok ? tr("Run finished.") : message);
    e2eMark(QStringLiteral("{\"ev\":\"test_run_finished\",\"ok\":%1,\"message\":%2}")
              .arg(ok ? "true" : "false")
              .arg(e2eJson(message)));
}

void TestsPanel::onSelectionChanged()
{
    QTreeWidgetItem *item = tree_->currentItem();
    selectedNodeId_ = item ? item->data(0, kIdRole).toString() : QString();
    if (selectedNodeId_.isEmpty()) {
        failureHeader_->clear();
        failureDetails_->clear();
        return;
    }
    const QString message = testService_->failureMessage(selectedNodeId_);
    const QString details = testService_->failureDetails(selectedNodeId_);
    failureHeader_->setText(message);
    failureDetails_->setPlainText(details);
}

void TestsPanel::onFailureLinkActivated(int textPosition)
{
    if (selectedNodeId_.isEmpty()) {
        return;
    }
    const FfiResolvedLink link =
      testService_->resolveFailureLink(selectedNodeId_, static_cast<quint32>(textPosition));
    if (link.found && openAt_) {
        openAt_(link.path, static_cast<int>(link.line),
                link.has_column ? static_cast<int>(link.column) : 0);
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
    QAction *rerun = menu.addAction(label);
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == rerun) {
        report(testService_->runNode(id));
    }
}

TestsPanel *buildTestsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                           ads::CDockAreaWidget *relativeTo, TestService *testService,
                           TestsPanel::OpenAt openAt)
{
    auto *panel = new TestsPanel(testService, std::move(openAt), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Tests"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("tests"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("tests"));
    return panel;
}

} // namespace ui_shell
