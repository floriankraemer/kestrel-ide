#include "database_console_bar.h"

#include "editor_tabs.h"

#include <QApplication>
#include <QComboBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QToolButton>
#include <QVariant>

namespace ui_shell {

namespace {
bool isSqlPath(const QString &path)
{
    return path.endsWith(QStringLiteral(".sql"), Qt::CaseInsensitive);
}
} // namespace

DatabaseConsoleBar::DatabaseConsoleBar(EditorTabs *editorTabs, ConsoleService *consoleService,
                                       QWidget *parent)
  : QWidget(parent)
  , editorTabs_(editorTabs)
  , consoleService_(consoleService)
{
    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(4, 2, 4, 2);

    sourceCombo_ = new QComboBox(this);
    sourceCombo_->setMinimumWidth(140);
    connect(sourceCombo_, &QComboBox::currentTextChanged, this,
            [this](const QString &) { attachCurrentTab(); });
    layout->addWidget(sourceCombo_);

    txModeCombo_ = new QComboBox(this);
    txModeCombo_->addItem(tr("Auto-commit"), QVariant::fromValue(int(FfiDbTxMode::Auto)));
    txModeCombo_->addItem(tr("Manual"), QVariant::fromValue(int(FfiDbTxMode::Manual)));
    connect(txModeCombo_, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            [this](int index) {
                if (currentTabId_ == 0) {
                    return;
                }
                const auto mode = static_cast<FfiDbTxMode>(txModeCombo_->itemData(index).toInt());
                consoleService_->setTxMode(currentTabId_, mode);
            });
    layout->addWidget(txModeCombo_);

    runButton_ = new QToolButton(this);
    runButton_->setText(tr("Run"));
    connect(runButton_, &QToolButton::clicked, this, &DatabaseConsoleBar::runClicked);
    layout->addWidget(runButton_);

    cancelButton_ = new QToolButton(this);
    cancelButton_->setText(tr("Cancel"));
    connect(cancelButton_, &QToolButton::clicked, this, &DatabaseConsoleBar::cancelClicked);
    layout->addWidget(cancelButton_);

    commitButton_ = new QToolButton(this);
    commitButton_->setText(tr("Commit"));
    connect(commitButton_, &QToolButton::clicked, this,
            [this]() { consoleService_->commit(currentTabId_); });
    layout->addWidget(commitButton_);

    rollbackButton_ = new QToolButton(this);
    rollbackButton_->setText(tr("Rollback"));
    connect(rollbackButton_, &QToolButton::clicked, this,
            [this]() { consoleService_->rollback(currentTabId_); });
    layout->addWidget(rollbackButton_);

    statusLabel_ = new QLabel(this);
    layout->addWidget(statusLabel_, 1);

    refreshSources();
    refreshForCurrentTab();
}

void DatabaseConsoleBar::refreshSources()
{
    sourceCombo_->clear();
    const ::rust::Vec<FfiDbSourceRow> sources = consoleService_->availableSources();
    for (const FfiDbSourceRow &source : sources) {
        sourceCombo_->addItem(QString(source.name), QString(source.id));
    }
}

void DatabaseConsoleBar::setStatus(const QString &text)
{
    statusLabel_->setText(text);
}

void DatabaseConsoleBar::refreshForCurrentTab()
{
    const quint64 tabId = editorTabs_->currentTabId();
    const QString path = editorTabs_->currentPath();
    if (!isSqlPath(path)) {
        setVisible(false);
        return;
    }
    setVisible(true);
    if (tabId == currentTabId_ && path == currentPath_) {
        return;
    }
    currentTabId_ = tabId;
    currentPath_ = path;
    attachedSourceId_.clear();
    setStatus(QString());

    const QString defaultSource = consoleService_->sourceForPath(path);
    if (!defaultSource.isEmpty()) {
        const int index = sourceCombo_->findData(defaultSource);
        if (index >= 0) {
            sourceCombo_->setCurrentIndex(index);
        }
    }
    attachCurrentTab();
}

void DatabaseConsoleBar::attachCurrentTab()
{
    if (currentTabId_ == 0) {
        return;
    }
    const QString sourceId = sourceCombo_->currentData().toString();
    if (sourceId.isEmpty() || sourceId == attachedSourceId_) {
        return;
    }
    attachedSourceId_ = sourceId;
    const FfiResult result = consoleService_->attach(currentTabId_, sourceId);
    if (result.code != 0) {
        setStatus(QString(result.message));
    }
}

void DatabaseConsoleBar::runClicked()
{
    if (currentTabId_ == 0) {
        return;
    }
    attachCurrentTab();
    const QString selected = editorTabs_->selectedText();
    FfiResult result;
    if (!selected.isEmpty()) {
        result = consoleService_->execute(currentTabId_, selected, FfiDbExecWhat::Selection, 0);
    } else {
        // Nothing selected: run the whole buffer as a script.
        // ponytail: "run the statement at the caret" (F3.3's headline
        // Ctrl+Enter behaviour) needs a caret byte offset `EditorTabs`
        // does not expose yet — add it and switch this branch to
        // `FfiDbExecWhat::Statement` once it does.
        editorTabs_->withCurrentEditor([this, &result](quint64, const QString &text) {
            result = consoleService_->execute(currentTabId_, text, FfiDbExecWhat::Selection, 0);
        });
    }
    if (result.code != 0) {
        setStatus(QString(result.message));
    }
}

void DatabaseConsoleBar::cancelClicked()
{
    if (currentTabId_ == 0) {
        return;
    }
    const FfiResult result = consoleService_->cancel(currentTabId_);
    if (result.code != 0) {
        setStatus(QString(result.message));
    }
}

DatabaseConsoleBar *mountDatabaseConsoleBar(EditorTabs *editorTabs, ConsoleService *consoleService,
                                           QWidget *parent)
{
    auto *bar = new DatabaseConsoleBar(editorTabs, consoleService, parent);
    // A second, independent subscriber to the app-wide focus signal —
    // `EditorTabs`'s own active-group tracking (`editor_tabs.cpp`) already
    // relies on the same signal the same way; Qt signals take any number
    // of slots, so this needs no touch to that connection.
    QObject::connect(qApp, &QApplication::focusChanged, bar,
                      [bar](QWidget *, QWidget *) { bar->refreshForCurrentTab(); });
    return bar;
}

} // namespace ui_shell
