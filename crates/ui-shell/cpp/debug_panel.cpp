#include "debug_panel.h"

#include "dock_layout.h"
#include "e2e_mark.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QClipboard>
#include <QCompleter>
#include <QComboBox>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMenu>
#include <QPlainTextEdit>
#include <QScrollBar>
#include <QSignalBlocker>
#include <QSplitter>
#include <QStringListModel>
#include <QStyledItemDelegate>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {
// Which variables reference a tree row stands for, and whether its children
// have been fetched. Kept on the item rather than in a side table so a
// cleared tree takes its bookkeeping with it.
constexpr int kReferenceRole = Qt::UserRole + 1;
constexpr int kFetchedRole = Qt::UserRole + 2;
// R5: the reference of the row's *container* — what `setVariable` needs
// alongside the row's own name, since DAP names a variable within its
// parent rather than by its own reference.
constexpr int kContainerReferenceRole = Qt::UserRole + 3;

QToolButton *makeButton(const QString &text, const QString &tip, QWidget *parent)
{
    auto *button = new QToolButton(parent);
    button->setText(text);
    button->setToolTip(tip);
    button->setEnabled(false);
    return button;
}

// R5: restricts inline editing to one column — column 0 (the name) must
// stay read-only on the Variables tree, and the Watches tree is the mirror
// image. `QTreeWidgetItem::setFlags` is per-row, not per-column, so which
// column is actually editable is this delegate's one job; there is only
// one editing rule here, so a class per tree would exist for a hypothetical
// second implementation rather than an actual one.
class ColumnEditDelegate : public QStyledItemDelegate
{
public:
    ColumnEditDelegate(int editableColumn, QObject *parent)
      : QStyledItemDelegate(parent)
      , column_(editableColumn)
    {
    }

    QWidget *createEditor(QWidget *parent, const QStyleOptionViewItem &option,
                           const QModelIndex &index) const override
    {
        if (index.column() != column_) {
            return nullptr;
        }
        return QStyledItemDelegate::createEditor(parent, option, index);
    }

private:
    int column_;
};

// One row, shared by the Variables, Watches and Evaluate trees — a variable
// is a variable regardless of which one asked for it (R5).
QTreeWidgetItem *addVariableRow(QTreeWidgetItem *under, QTreeWidget *tree,
                                 const FfiVariable &variable, qint64 containerReference,
                                 bool editableValue)
{
    auto *row = under ? new QTreeWidgetItem(under) : new QTreeWidgetItem(tree);
    row->setText(0, QString(variable.name));
    row->setText(1, QString(variable.value));
    row->setToolTip(1, QString(variable.type_name));
    row->setData(0, kReferenceRole, static_cast<qlonglong>(variable.variables_reference));
    row->setData(0, kContainerReferenceRole, static_cast<qlonglong>(containerReference));
    if (variable.variables_reference != 0) {
        // A placeholder child is what makes the row expandable before its
        // children exist; the tree's own expand handler replaces it on
        // demand.
        row->setChildIndicatorPolicy(QTreeWidgetItem::ShowIndicator);
    }
    if (editableValue) {
        row->setFlags(row->flags() | Qt::ItemIsEditable);
    }
    return row;
}
} // namespace

DebugPanel::DebugPanel(DebugService *debugService, OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , debugService_(debugService)
  , openAt_(std::move(openAt))
{
    resumeButton_ = makeButton(tr("Resume"), tr("Resume Program"), this);
    pauseButton_ = makeButton(tr("Pause"), tr("Pause Program"), this);
    stopButton_ = makeButton(tr("Stop"), tr("Stop Debugging"), this);
    stepOverButton_ = makeButton(tr("Over"), tr("Step Over"), this);
    stepIntoButton_ = makeButton(tr("Into"), tr("Step Into"), this);
    stepOutButton_ = makeButton(tr("Out"), tr("Step Out"), this);

    // D4-5: several sessions can run at once — a debugged test suite while
    // a server stays suspended — so which one the views show is a choice,
    // not an assumption.
    sessionPicker_ = new QComboBox(this);
    sessionPicker_->setMinimumWidth(120);
    // R5: which of the stopped adapter's threads the frames/variables below
    // belong to.
    threadPicker_ = new QComboBox(this);
    threadPicker_->setMinimumWidth(120);

    auto *toolbar = new QHBoxLayout();
    toolbar->addWidget(sessionPicker_);
    toolbar->addWidget(threadPicker_);
    for (QToolButton *button :
         {resumeButton_, pauseButton_, stepOverButton_, stepIntoButton_, stepOutButton_,
          stopButton_}) {
        toolbar->addWidget(button);
    }
    toolbar->addStretch(1);

    frames_ = new QListWidget(this);

    variableFilter_ = new QLineEdit(this);
    variableFilter_->setPlaceholderText(tr("Filter variables"));
    variables_ = new QTreeWidget(this);
    variables_->setColumnCount(2);
    variables_->setHeaderLabels({tr("Name"), tr("Value")});
    variables_->setItemDelegate(new ColumnEditDelegate(1, variables_));
    variables_->setContextMenuPolicy(Qt::CustomContextMenu);
    auto *variablesColumn = new QVBoxLayout();
    variablesColumn->addWidget(variableFilter_);
    variablesColumn->addWidget(variables_, 1);
    auto *variablesWidget = new QWidget(this);
    variablesWidget->setLayout(variablesColumn);

    watches_ = new QTreeWidget(this);
    watches_->setColumnCount(2);
    watches_->setHeaderLabels({tr("Expression"), tr("Value")});
    watches_->setItemDelegate(new ColumnEditDelegate(0, watches_));
    watches_->setContextMenuPolicy(Qt::CustomContextMenu);
    watchInput_ = new QLineEdit(this);
    watchInput_->setPlaceholderText(tr("Add watch expression"));

    auto *watchColumn = new QVBoxLayout();
    watchColumn->addWidget(new QLabel(tr("Watches"), this));
    watchColumn->addWidget(watches_, 1);
    watchColumn->addWidget(watchInput_);
    auto *watchWidget = new QWidget(this);
    watchWidget->setLayout(watchColumn);

    console_ = new QPlainTextEdit(this);
    console_->setReadOnly(true);
    console_->setMaximumBlockCount(5000);
    evaluateTree_ = new QTreeWidget(this);
    evaluateTree_->setColumnCount(2);
    evaluateTree_->setHeaderLabels({tr("Expression"), tr("Value")});
    evaluateTree_->setContextMenuPolicy(Qt::CustomContextMenu);
    evaluateInput_ = new QLineEdit(this);
    evaluateInput_->setPlaceholderText(tr("Evaluate expression"));
    // R5: an expression typed before recalls what was typed in earlier
    // sessions too — the history lives in `DebugService`, not in this box.
    auto *evaluateCompleter = new QCompleter(this);
    evaluateCompleter->setCaseSensitivity(Qt::CaseInsensitive);
    evaluateInput_->setCompleter(evaluateCompleter);

    auto *consoleColumn = new QVBoxLayout();
    consoleColumn->addWidget(new QLabel(tr("Console"), this));
    consoleColumn->addWidget(console_, 1);
    consoleColumn->addWidget(new QLabel(tr("Evaluate"), this));
    consoleColumn->addWidget(evaluateTree_, 1);
    consoleColumn->addWidget(evaluateInput_);
    auto *consoleWidget = new QWidget(this);
    consoleWidget->setLayout(consoleColumn);

    auto *splitter = new QSplitter(Qt::Horizontal, this);
    splitter->addWidget(frames_);
    splitter->addWidget(variablesWidget);
    splitter->addWidget(watchWidget);
    splitter->addWidget(consoleWidget);
    splitter->setStretchFactor(1, 2);
    splitter->setStretchFactor(3, 2);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addLayout(toolbar);
    layout->addWidget(splitter, 1);

    connect(sessionPicker_, &QComboBox::currentIndexChanged, this, [this](int index) {
        if (index < 0) {
            return;
        }
        const quint64 chosen = sessionPicker_->itemData(index).toULongLong();
        if (chosen != 0 && chosen != sessionId_) {
            sessionId_ = chosen;
            refreshThreads();
            refreshFrames();
        }
    });
    connect(threadPicker_, &QComboBox::currentIndexChanged, this, [this](int index) {
        if (index < 0 || sessionId_ == 0) {
            return;
        }
        debugService_->selectThread(sessionId_, threadPicker_->itemData(index).toLongLong());
    });
    connect(resumeButton_, &QToolButton::clicked, this, &DebugPanel::resume);
    connect(pauseButton_, &QToolButton::clicked, this, &DebugPanel::pause);
    connect(stopButton_, &QToolButton::clicked, this, &DebugPanel::stopSession);
    connect(stepOverButton_, &QToolButton::clicked, this, &DebugPanel::stepOver);
    connect(stepIntoButton_, &QToolButton::clicked, this, &DebugPanel::stepInto);
    connect(stepOutButton_, &QToolButton::clicked, this, &DebugPanel::stepOut);

    connect(frames_, &QListWidget::currentRowChanged, this, [this](int row) {
        if (sessionId_ == 0 || row < 0) {
            return;
        }
        const ::rust::Vec<FfiStackFrame> frames = debugService_->frames();
        if (row < static_cast<int>(frames.size())) {
            debugService_->selectFrame(sessionId_, frames[row].id);
        }
    });
    // R5: double-click a frame to look at its source, the same jump a
    // suspended breakpoint's own top frame already gets.
    connect(frames_, &QListWidget::itemDoubleClicked, this, [this]() {
        const int row = frames_->currentRow();
        const ::rust::Vec<FfiStackFrame> frames = debugService_->frames();
        if (row < 0 || row >= static_cast<int>(frames.size()) || !openAt_) {
            return;
        }
        const FfiStackFrame &frame = frames[static_cast<size_t>(row)];
        if (!QString(frame.path).isEmpty()) {
            openAt_(QString(frame.path), static_cast<int>(frame.line), 1);
        }
    });

    connect(variables_, &QTreeWidget::itemExpanded, this, &DebugPanel::expandItem);
    connect(variables_, &QTreeWidget::itemChanged, this, [this](QTreeWidgetItem *item, int column) {
        // R5: an inline value edit — guarded against the `setText` calls
        // this same panel makes while (re)populating the tree, which fire
        // the identical signal.
        if (populating_ || column != 1 || sessionId_ == 0) {
            return;
        }
        const qint64 container = item->data(0, kContainerReferenceRole).toLongLong();
        debugService_->setVariable(sessionId_, container, item->text(0), item->text(1));
    });
    connect(variables_, &QTreeWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        showTreeContextMenu(variables_, variables_->viewport()->mapToGlobal(pos));
    });
    connect(variableFilter_, &QLineEdit::textChanged, this, &DebugPanel::applyVariableFilter);

    connect(watches_, &QTreeWidget::itemExpanded, this,
            [this](QTreeWidgetItem *item) { expandWatchItem(item, watches_); });
    connect(watches_, &QTreeWidget::itemChanged, this, [this](QTreeWidgetItem *item, int column) {
        // R5: renaming a watch's expression — guarded the same way the
        // Variables tree's value edit is.
        if (populating_ || column != 0) {
            return;
        }
        const int index = watches_->indexOfTopLevelItem(item);
        if (index >= 0) {
            debugService_->editWatch(static_cast<quint32>(index), item->text(0));
        }
    });
    connect(watches_, &QTreeWidget::customContextMenuRequested, this, [this](const QPoint &pos) {
        showTreeContextMenu(watches_, watches_->viewport()->mapToGlobal(pos));
    });
    connect(watchInput_, &QLineEdit::returnPressed, this, [this]() {
        debugService_->addWatch(watchInput_->text());
        watchInput_->clear();
    });

    connect(evaluateTree_, &QTreeWidget::itemExpanded, this,
            [this](QTreeWidgetItem *item) { expandWatchItem(item, evaluateTree_); });
    connect(evaluateTree_, &QTreeWidget::customContextMenuRequested, this,
            [this](const QPoint &pos) {
                showTreeContextMenu(evaluateTree_, evaluateTree_->viewport()->mapToGlobal(pos));
            });
    connect(evaluateInput_, &QLineEdit::returnPressed, this, [this]() {
        if (sessionId_ != 0 && !evaluateInput_->text().trimmed().isEmpty()) {
            debugService_->evaluateToTree(sessionId_, evaluateInput_->text());
            evaluateInput_->clear();
        }
    });

    connect(debugService_, &DebugService::debugStarted, this, &DebugPanel::onStarted);
    connect(debugService_, &DebugService::debugStopped, this, &DebugPanel::onStopped);
    connect(debugService_, &DebugService::debugResumed, this, &DebugPanel::onResumed);
    connect(debugService_, &DebugService::debugTerminated, this, &DebugPanel::onTerminated);
    connect(debugService_, &DebugService::debugFailed, this, &DebugPanel::onFailed);
    connect(debugService_, &DebugService::debugOutput, this, &DebugPanel::onOutput);
    connect(debugService_, &DebugService::variablesChanged, this,
            &DebugPanel::onVariablesChanged);
    connect(debugService_, &DebugService::framesChanged, this, [this](quint64 sessionId) {
        if (sessionId == sessionId_) {
            refreshFrames();
        }
    });
    connect(debugService_, &DebugService::watchChildrenChanged, this,
            &DebugPanel::onWatchChildrenChanged);
    connect(debugService_, &DebugService::watchesChanged, this, &DebugPanel::onWatchesChanged);
    connect(debugService_, &DebugService::evaluatedToTree, this,
            &DebugPanel::onEvaluatedToTree);
}

void DebugPanel::resume()
{
    if (sessionId_ != 0) {
        debugService_->resume(sessionId_);
        setRunning(true);
    }
}

void DebugPanel::pause()
{
    if (sessionId_ != 0) {
        debugService_->pause(sessionId_);
    }
}

void DebugPanel::stepOver()
{
    if (sessionId_ != 0) {
        debugService_->stepOver(sessionId_);
        setRunning(true);
    }
}

void DebugPanel::stepInto()
{
    if (sessionId_ != 0) {
        debugService_->stepInto(sessionId_);
        setRunning(true);
    }
}

void DebugPanel::stepOut()
{
    if (sessionId_ != 0) {
        debugService_->stepOut(sessionId_);
        setRunning(true);
    }
}

void DebugPanel::stopSession()
{
    if (sessionId_ != 0) {
        debugService_->stop(sessionId_);
    }
}

void DebugPanel::onStarted(quint64 sessionId, const QString &configId)
{
    sessionId_ = sessionId;
    console_->clear();
    console_->appendPlainText(tr("Debugging %1").arg(configId));
    frames_->clear();
    variables_->clear();
    threadPicker_->clear();
    setRunning(true);
    stopButton_->setEnabled(true);
    refreshSessions();
    e2eMark(QStringLiteral("{\"ev\":\"debug_started\",\"session_id\":%1}").arg(sessionId));
}

void DebugPanel::onStopped(quint64 sessionId, const QString &reason, const QString &path,
                            quint32 line)
{
    if (sessionId != sessionId_) {
        return;
    }
    setRunning(false);
    refreshThreads();
    refreshFrames();
    e2eMark(QStringLiteral("{\"ev\":\"debug_stopped\",\"session_id\":%1,\"reason\":%2,\"line\":%3}")
              .arg(sessionId)
              .arg(e2eJson(reason))
              .arg(line));
    Q_UNUSED(path);
}

void DebugPanel::onResumed(quint64 sessionId)
{
    if (sessionId == sessionId_) {
        setRunning(true);
    }
}

void DebugPanel::onTerminated(quint64 sessionId, int exitCode)
{
    if (sessionId != sessionId_) {
        return;
    }
    sessionId_ = 0;
    frames_->clear();
    variables_->clear();
    threadPicker_->clear();
    for (QToolButton *button : {resumeButton_, pauseButton_, stopButton_, stepOverButton_,
                                 stepIntoButton_, stepOutButton_}) {
        button->setEnabled(false);
    }
    console_->appendPlainText(tr("Process finished with exit code %1").arg(exitCode));
    refreshSessions();
    e2eMark(QStringLiteral("{\"ev\":\"debug_terminated\",\"session_id\":%1,\"exit_code\":%2}")
              .arg(sessionId)
              .arg(exitCode));
}

void DebugPanel::onFailed(quint64 sessionId, const FfiResult &error)
{
    Q_UNUSED(sessionId);
    // Shown in the console, where the session's own output would have been:
    // "codelldb could not be started: … install it from …" is the answer to
    // the question the user just asked.
    console_->appendPlainText(QString(error.message));
    e2eMark(QStringLiteral("{\"ev\":\"debug_failed\",\"code\":%1,\"message\":%2}")
              .arg(error.code)
              .arg(e2eJson(QString(error.message))));
}

void DebugPanel::onOutput(quint64 sessionId, const QString &category, const QString &text)
{
    if (sessionId != sessionId_) {
        return;
    }
    Q_UNUSED(category);
    QTextCursor cursor = console_->textCursor();
    cursor.movePosition(QTextCursor::End);
    cursor.insertText(text);
    console_->setTextCursor(cursor);
    console_->verticalScrollBar()->setValue(console_->verticalScrollBar()->maximum());
}

void DebugPanel::onVariablesChanged(quint64 sessionId, qint64 reference)
{
    if (sessionId != sessionId_) {
        return;
    }
    const ::rust::Vec<FfiVariable> variables = debugService_->variables(reference);
    const bool editable = debugService_->canSetVariable(sessionId_);

    // The row this reference belongs under, or the tree's root for a scope.
    QTreeWidgetItem *parent = nullptr;
    QList<QTreeWidgetItem *> pending;
    for (int i = 0; i < variables_->topLevelItemCount(); ++i) {
        pending.append(variables_->topLevelItem(i));
    }
    while (!pending.isEmpty()) {
        QTreeWidgetItem *item = pending.takeFirst();
        if (item->data(0, kReferenceRole).toLongLong() == reference) {
            parent = item;
            break;
        }
        for (int i = 0; i < item->childCount(); ++i) {
            pending.append(item->child(i));
        }
    }

    populating_ = true;
    if (parent) {
        parent->takeChildren();
        parent->setData(0, kFetchedRole, true);
        for (const FfiVariable &variable : variables) {
            addVariableRow(parent, variables_, variable, reference, editable);
        }
    } else {
        // Scope-level population only: a reference from the Watches or
        // Evaluate tree that this tree does not recognise belongs to one of
        // them, not to a fresh scope dump, so it is left alone rather than
        // dumped in here as bogus top-level rows.
        for (const FfiVariable &variable : variables) {
            addVariableRow(nullptr, variables_, variable, reference, editable);
        }
    }
    populating_ = false;
    if (!parent) {
        e2eMark(QStringLiteral("{\"ev\":\"debug_variables\",\"count\":%1}").arg(variables.size()));
    }
}

void DebugPanel::onWatchChildrenChanged(quint64 sessionId, qint64 reference)
{
    if (sessionId != sessionId_) {
        return;
    }
    const ::rust::Vec<FfiVariable> children = debugService_->variables(reference);
    for (QTreeWidget *tree : {watches_, evaluateTree_}) {
        QList<QTreeWidgetItem *> pending;
        for (int i = 0; i < tree->topLevelItemCount(); ++i) {
            pending.append(tree->topLevelItem(i));
        }
        while (!pending.isEmpty()) {
            QTreeWidgetItem *item = pending.takeFirst();
            if (item->data(0, kReferenceRole).toLongLong() == reference) {
                populating_ = true;
                item->takeChildren();
                item->setData(0, kFetchedRole, true);
                for (const FfiVariable &child : children) {
                    addVariableRow(item, tree, child, reference, false);
                }
                populating_ = false;
                break;
            }
            for (int i = 0; i < item->childCount(); ++i) {
                pending.append(item->child(i));
            }
        }
    }
}

void DebugPanel::onEvaluatedToTree(quint64 sessionId, const FfiVariable &row)
{
    if (sessionId != sessionId_) {
        return;
    }
    addVariableRow(nullptr, evaluateTree_, row, 0, false);
    evaluateTree_->scrollToBottom();
    // R5: the history box's completer, refreshed with what this evaluation
    // just added to it.
    if (auto *model =
          qobject_cast<QStringListModel *>(evaluateInput_->completer()->model())) {
        model->setStringList(
          debugService_->evaluateHistory().split(QLatin1Char('\n'), Qt::SkipEmptyParts));
    } else {
        evaluateInput_->completer()->setModel(new QStringListModel(
          debugService_->evaluateHistory().split(QLatin1Char('\n'), Qt::SkipEmptyParts),
          evaluateInput_));
    }
    e2eMark("{\"ev\":\"debug_evaluated\"}");
}

void DebugPanel::onWatchesChanged()
{
    populating_ = true;
    watches_->clear();
    for (const FfiWatch &watch : debugService_->watchesDetailed()) {
        const FfiVariable row{watch.expression, watch.value, watch.type_name,
                               watch.variables_reference};
        // ponytail: the tree is rebuilt flat on every refresh, so an
        // expanded watch collapses again on the next stop; revisit with
        // per-index expand-state tracking if that friction shows up.
        addVariableRow(nullptr, watches_, row, 0, true);
    }
    populating_ = false;
}

void DebugPanel::refreshSessions()
{
    const QSignalBlocker blocker(sessionPicker_);
    sessionPicker_->clear();
    for (const QString &line :
         debugService_->sessions().split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        const QStringList parts = line.split(QLatin1Char('\t'));
        if (parts.size() < 2) {
            continue;
        }
        sessionPicker_->addItem(parts.at(1), parts.at(0).toULongLong());
        if (parts.at(0).toULongLong() == sessionId_) {
            sessionPicker_->setCurrentIndex(sessionPicker_->count() - 1);
        }
    }
}

void DebugPanel::refreshThreads()
{
    const QSignalBlocker blocker(threadPicker_);
    threadPicker_->clear();
    if (sessionId_ == 0) {
        return;
    }
    for (const FfiDebugThread &thread : debugService_->threads()) {
        threadPicker_->addItem(QString(thread.name), static_cast<qulonglong>(thread.id));
    }
    if (threadPicker_->count() > 0) {
        threadPicker_->setCurrentIndex(0);
    }
}

void DebugPanel::refreshFrames()
{
    frames_->clear();
    for (const FfiStackFrame &frame : debugService_->frames()) {
        const QString where = QString(frame.path).isEmpty()
          ? QString(frame.name)
          : QStringLiteral("%1  (%2:%3)").arg(QString(frame.name), QString(frame.path)).arg(frame.line);
        frames_->addItem(where);
    }
    if (frames_->count() > 0) {
        frames_->setCurrentRow(0);
    }
}

void DebugPanel::expandItem(QTreeWidgetItem *item)
{
    if (sessionId_ == 0 || item->data(0, kFetchedRole).toBool()) {
        return;
    }
    const qint64 reference = item->data(0, kReferenceRole).toLongLong();
    if (reference != 0) {
        debugService_->expand(sessionId_, reference);
    }
}

void DebugPanel::expandWatchItem(QTreeWidgetItem *item, QTreeWidget *tree)
{
    Q_UNUSED(tree);
    if (sessionId_ == 0 || item->data(0, kFetchedRole).toBool()) {
        return;
    }
    const qint64 reference = item->data(0, kReferenceRole).toLongLong();
    if (reference != 0) {
        debugService_->watchChildren(sessionId_, reference);
    }
}

void DebugPanel::applyVariableFilter(const QString &text)
{
    // ponytail: only rows already fetched are considered — an unexpanded
    // child is not force-loaded just to test it against the filter.
    std::function<bool(QTreeWidgetItem *)> filterRow = [&](QTreeWidgetItem *item) {
        bool selfMatch = text.isEmpty() || item->text(0).contains(text, Qt::CaseInsensitive)
          || item->text(1).contains(text, Qt::CaseInsensitive);
        bool anyChildMatch = false;
        for (int i = 0; i < item->childCount(); ++i) {
            if (filterRow(item->child(i))) {
                anyChildMatch = true;
            }
        }
        const bool visible = selfMatch || anyChildMatch;
        item->setHidden(!visible);
        return visible;
    };
    for (int i = 0; i < variables_->topLevelItemCount(); ++i) {
        filterRow(variables_->topLevelItem(i));
    }
}

void DebugPanel::showTreeContextMenu(QTreeWidget *tree, const QPoint &globalPos)
{
    QTreeWidgetItem *item = tree->currentItem();
    if (!item) {
        return;
    }
    QMenu menu(tree);
    QAction *copyValue = menu.addAction(tr("Copy Value"));
    QAction *copyPath = menu.addAction(tr("Copy Path"));
    QAction *removeWatch = nullptr;
    if (tree == watches_ && watches_->indexOfTopLevelItem(item) >= 0) {
        removeWatch = menu.addAction(tr("Remove Watch"));
    }
    QAction *chosen = menu.exec(globalPos);
    if (chosen == copyValue) {
        QGuiApplication::clipboard()->setText(item->text(1));
    } else if (chosen == copyPath) {
        QStringList parts;
        for (QTreeWidgetItem *cur = item; cur; cur = cur->parent()) {
            parts.prepend(cur->text(0));
        }
        QGuiApplication::clipboard()->setText(parts.join(QLatin1Char('.')));
    } else if (chosen == removeWatch) {
        debugService_->removeWatch(static_cast<quint32>(watches_->indexOfTopLevelItem(item)));
    }
}

void DebugPanel::setRunning(bool running)
{
    // While the debuggee runs there is nothing to step from, and while it is
    // suspended there is nothing to pause. The buttons say so.
    resumeButton_->setEnabled(!running && sessionId_ != 0);
    stepOverButton_->setEnabled(!running && sessionId_ != 0);
    stepIntoButton_->setEnabled(!running && sessionId_ != 0);
    stepOutButton_->setEnabled(!running && sessionId_ != 0);
    pauseButton_->setEnabled(running && sessionId_ != 0);
}

DebugPanel *buildDebugDock(ads::CDockManager *dockManager, DockRegistry *docks,
                            ads::CDockAreaWidget *relativeTo, DebugService *debugService,
                            DebugPanel::OpenAt openAt)
{
    auto *panel = new DebugPanel(debugService, std::move(openAt), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Debug"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("debug"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("debug"));
    return panel;
}

} // namespace ui_shell
