#include "database_console_bar.h"

#include "e2e_mark.h"
#include "editor_tabs.h"

#include <QAction>
#include <QApplication>
#include <QComboBox>
#include <QHBoxLayout>
#include <QKeySequence>
#include <QLabel>
#include <QSignalBlocker>
#include <QStringList>
#include <QTimer>
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
                                       AppSettings *appSettings, QWidget *parent)
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

    policyCombo_ = new QComboBox(this);
    policyCombo_->addItem(tr("Stop on error"),
                          QVariant::fromValue(int(FfiDbScriptPolicy::StopOnError)));
    policyCombo_->addItem(tr("Continue on error"),
                          QVariant::fromValue(int(FfiDbScriptPolicy::Continue)));
    policyCombo_->addItem(tr("Ask on error"), QVariant::fromValue(int(FfiDbScriptPolicy::Ask)));
    connect(policyCombo_, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            [this](int index) {
                if (currentTabId_ == 0) {
                    return;
                }
                const auto policy =
                  static_cast<FfiDbScriptPolicy>(policyCombo_->itemData(index).toInt());
                consoleService_->setScriptPolicy(currentTabId_, policy);
            });
    layout->addWidget(policyCombo_);

    schemaCombo_ = new QComboBox(this);
    schemaCombo_->setMinimumWidth(120);
    connect(schemaCombo_, QOverload<int>::of(&QComboBox::currentIndexChanged), this,
            [this](int index) {
                if (currentTabId_ == 0 || index < 0 || schemaCombo_->count() == 0) {
                    return;
                }
                const FfiResult result =
                  consoleService_->setSchema(currentTabId_, schemaCombo_->itemText(index));
                if (result.code != 0) {
                    setStatus(QString(result.message));
                }
            });
    layout->addWidget(schemaCombo_);

    runButton_ = new QToolButton(this);
    runButton_->setText(tr("Run"));
    connect(runButton_, &QToolButton::clicked, this, &DatabaseConsoleBar::runClicked);
    layout->addWidget(runButton_);

    runScriptButton_ = new QToolButton(this);
    runScriptButton_->setText(tr("Run script"));
    connect(runScriptButton_, &QToolButton::clicked, this, &DatabaseConsoleBar::runScriptClicked);
    layout->addWidget(runScriptButton_);

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

    // Window-scoped shortcuts (Qt's own default context for a QAction
    // added to a widget): they fire regardless of which widget in the
    // main window has focus, in particular the SQL editor itself, which
    // is what the user is actually typing Ctrl+(Shift+)Enter into — the
    // same reasoning `registerAction`'s menu actions already rely on for
    // every other window-wide shortcut (`keymap_page.cpp`'s own doc
    // comment).
    auto *runAction = new QAction(this);
    runAction->setShortcut(
      QKeySequence(appSettings->shortcutFor(QStringLiteral("database.run")),
                   QKeySequence::PortableText));
    connect(runAction, &QAction::triggered, this, &DatabaseConsoleBar::runClicked);
    addAction(runAction);

    auto *runScriptAction = new QAction(this);
    runScriptAction->setShortcut(
      QKeySequence(appSettings->shortcutFor(QStringLiteral("database.runScript")),
                   QKeySequence::PortableText));
    connect(runScriptAction, &QAction::triggered, this, &DatabaseConsoleBar::runScriptClicked);
    addAction(runScriptAction);

    // `attach`'s own `Names`-level introspect (`ConsoleServiceRust`'s doc
    // comment) replies asynchronously, after the "Attached to ..."
    // `outputAppended` this bar can already observe — ponytail: piggybacks
    // on that signal rather than a dedicated "schemas changed" one, since
    // every attach already appends at least that one line.
    connect(consoleService_, &ConsoleService::outputAppended, this,
            [this](quint64 tabId, const QString &) {
                if (tabId == currentTabId_) {
                    refreshSchemas();
                }
            });

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
    const bool wasVisible = isVisible();
    setVisible(true);
    if (!wasVisible) {
        // E2E only: the bar starts (and stays, between `.sql` tabs)
        // hidden — a mark taken any earlier, e.g. from the dock's own
        // `visibilityChanged`, reports every button's pre-`setVisible`
        // geometry, identical and wrong. This is the one place the bar
        // actually becomes visible, so it is also the one place a mark
        // taken a turn later is trustworthy.
        QTimer::singleShot(0, this, [this]() { markE2eToolbarRects(); });
    }
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
    const int policyIndex =
      policyCombo_->findData(QVariant::fromValue(int(consoleService_->scriptPolicy(tabId))));
    if (policyIndex >= 0) {
        const QSignalBlocker blocker(policyCombo_);
        policyCombo_->setCurrentIndex(policyIndex);
    }
    refreshSchemas();
}

void DatabaseConsoleBar::refreshSchemas()
{
    if (currentTabId_ == 0) {
        schemaCombo_->clear();
        return;
    }
    const QSignalBlocker blocker(schemaCombo_);
    const QString current = schemaCombo_->currentText();
    schemaCombo_->clear();
    const QStringList names = consoleService_->schemas(currentTabId_);
    for (const QString &name : names) {
        schemaCombo_->addItem(name);
    }
    schemaCombo_->setVisible(schemaCombo_->count() > 0);
    const int index = schemaCombo_->findText(current);
    if (index >= 0) {
        schemaCombo_->setCurrentIndex(index);
    }
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
    if (currentTabId_ == 0 || !isSqlPath(editorTabs_->currentPath())) {
        return;
    }
    attachCurrentTab();
    const QString selected = editorTabs_->selectedText();
    FfiResult result;
    if (!selected.isEmpty()) {
        result = consoleService_->execute(currentTabId_, selected, FfiDbExecWhat::Selection, 0);
    } else {
        // Nothing selected: run only the statement the caret sits inside
        // (F3.3's headline Ctrl+Enter behaviour) — `EditorOps` already
        // tracks the live caret's byte offset for every open tab.
        const quint32 caret = static_cast<quint32>(editorTabs_->editorOps()->caretOffset(currentTabId_));
        const QString text = editorTabs_->currentContent();
        result = consoleService_->execute(currentTabId_, text, FfiDbExecWhat::Statement, caret);
    }
    if (result.code != 0) {
        setStatus(QString(result.message));
    }
}

void DatabaseConsoleBar::runScriptClicked()
{
    if (currentTabId_ == 0 || !isSqlPath(editorTabs_->currentPath())) {
        return;
    }
    attachCurrentTab();
    const QString text = editorTabs_->currentContent();
    const FfiResult result = consoleService_->execute(currentTabId_, text, FfiDbExecWhat::Selection, 0);
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

void DatabaseConsoleBar::markE2eToolbarRects() const
{
    struct ButtonEntry
    {
        const char *name;
        QToolButton *button;
    };
    const ButtonEntry entries[] = {
        { "run", runButton_ },
        { "runScript", runScriptButton_ },
        { "cancel", cancelButton_ },
        { "commit", commitButton_ },
        { "rollback", rollbackButton_ },
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
    e2eMark(QStringLiteral("{\"ev\":\"database_console_toolbar_rects\",\"buttons\":[%1]}")
              .arg(buttons.join(QLatin1Char(','))));
}

DatabaseConsoleBar *mountDatabaseConsoleBar(EditorTabs *editorTabs, ConsoleService *consoleService,
                                           AppSettings *appSettings, QWidget *parent)
{
    auto *bar = new DatabaseConsoleBar(editorTabs, consoleService, appSettings, parent);
    // A second, independent subscriber to the app-wide focus signal —
    // `EditorTabs`'s own active-group tracking (`editor_tabs.cpp`) already
    // relies on the same signal the same way; Qt signals take any number
    // of slots, so this needs no touch to that connection.
    QObject::connect(qApp, &QApplication::focusChanged, bar,
                      [bar](QWidget *, QWidget *) { bar->refreshForCurrentTab(); });
    // FX: `ConsoleService::sourcesChanged` (`projectOpened`'s own signal —
    // see its `ffi.rs` doc comment) — the combo is only ever populated
    // once, at construction, before any project is open.
    QObject::connect(consoleService, &ConsoleService::sourcesChanged, bar,
                      [bar]() { bar->refreshSources(); });
    return bar;
}

} // namespace ui_shell
