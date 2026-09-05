#include "editor_tabs.h"

#include "code_editor.h"
#include "diff_view.h"
#include "diff_view_page.h"
#include "e2e_mark.h"
#include "find_bar.h"
#include "hex_viewer.h"
#include "icon_cache.h"
#include "syntax_highlighter.h"

#include <QApplication>
#include <QByteArray>
#include <QColor>
#include <QCursor>
#include <QFileDialog>
#include <QFileInfo>
#include <QFont>
#include <QHash>
#include <QInputDialog>
#include <QLabel>
#include <QMenu>
#include <QMessageBox>
#include <QPalette>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QRect>
#include <QSplitter>
#include <QTabBar>
#include <QTabWidget>
#include <QTextBlock>
#include <QTextCursor>
#include <QToolTip>
#include <QVariant>
#include <QVector>
#include <QtGui/QTextDocument>
#include <algorithm>

namespace ui_shell {

namespace {

// The SyntaxHighlighter onTabOpened attached to `document`, if any.
//
// QSyntaxHighlighter parents itself to the document it attaches to, so that —
// not the editor widget — is where the instance can be found again.
// SyntaxHighlighter deliberately has no Q_OBJECT (it only overrides a plain
// virtual), and Qt 6.7+ static_asserts that findChild's type has one, so the
// lookup is a plain dynamic_cast over the document's children instead.
SyntaxHighlighter *highlighterOf(QTextDocument *document)
{
    for (QObject *child : document->children()) {
        if (auto *highlighter = dynamic_cast<SyntaxHighlighter *>(child)) {
            return highlighter;
        }
    }
    return nullptr;
}

// Moves `editor`'s caret to (1-based) `line`, `column` characters into it,
// and centres the view on it — the shared tail of every jump the IDE makes
// (Find in Files, Class View, Go to Line).
//
// `line` is clamped to the document: QTextDocument::findBlockByNumber returns
// an invalid block past the end, which silently lands the caret at position 0
// instead of the last line. Any fold hiding the target is expanded first, so
// the caret never ends up on an invisible line.
void moveCursorToLine(QPlainTextEdit *editor, int line, int column)
{
    const int blockNumber = qBound(0, line - 1, editor->blockCount() - 1);
    if (auto *codeEditor = qobject_cast<CodeEditor *>(editor)) {
        codeEditor->ensureBlockVisible(blockNumber);
    }
    const QTextBlock block = editor->document()->findBlockByNumber(blockNumber);
    QTextCursor cursor(block);
    cursor.movePosition(QTextCursor::StartOfBlock);
    // Clamped to the block: QTextCursor::Right walks on into the *next* line
    // rather than stopping at the end of this one, so a column past the end
    // of the line would land the caret on the line below.
    cursor.movePosition(QTextCursor::Right, QTextCursor::MoveAnchor,
                         qBound(0, column, block.length() - 1));
    editor->setTextCursor(cursor);
    editor->centerCursor();
    editor->setFocus();
}

// UTF-16 column for a UTF-8 *byte* column into `lineText`.
//
// The project index and tree-sitter both report columns as byte offsets;
// QTextCursor counts UTF-16 code units. The two agree exactly on ASCII and
// diverge on everything else, which is why passing one for the other looks
// correct until a line contains an accent.
//
// A byte column landing inside a character snaps back to that character's
// start, and one past the end of the line clamps to it.
int utf16ColumnForByteColumn(const QString &lineText, int byteColumn)
{
    if (byteColumn <= 0) {
        return 0;
    }
    const QByteArray utf8 = lineText.toUtf8();
    int at = std::clamp(byteColumn, 0, static_cast<int>(utf8.size()));
    // Continuation bytes are 0b10xxxxxx; walking back off them puts `at` on a
    // character boundary, so decoding the prefix cannot produce a replacement
    // character and miscount the column by one.
    while (at > 0 && (static_cast<unsigned char>(utf8.at(at)) & 0xC0) == 0x80) {
        --at;
    }
    return QString::fromUtf8(utf8.constData(), at).size();
}

// Jump to a column reported in bytes rather than UTF-16 units — the form the
// project index and tree-sitter use. Every byte-column caller goes through
// here; `moveCursorToLine` keeps taking UTF-16 columns, because the jump
// history stores what QTextCursor::columnNumber() gave it and must round-trip
// unchanged.
void moveCursorToByteColumn(QPlainTextEdit *editor, int line, int byteColumn)
{
    const int blockNumber = qBound(0, line - 1, editor->blockCount() - 1);
    const QTextBlock block = editor->document()->findBlockByNumber(blockNumber);
    moveCursorToLine(editor, line, utf16ColumnForByteColumn(block.text(), byteColumn));
}

} // namespace

EditorTabs::EditorTabs(DocumentManager *docManager, LanguageService *languageService,
                       QSplitter *root, QWidget *window)
  : docManager_(docManager)
  , languageService_(languageService)
  , editorOps_(new EditorOps(this))
  , root_(root)
  , window_(window)
{
    connect(docManager_, &DocumentManager::tabOpened, this, &EditorTabs::onTabOpened);
    connect(docManager_, &DocumentManager::tabClosed, this, &EditorTabs::onTabClosed);
    connect(docManager_,
            &DocumentManager::tabModifiedChanged,
            this,
            &EditorTabs::onTabModifiedChanged);

    // Clicking anywhere inside a group — its tab bar or its editor —
    // makes that group the active one. One application-wide hook beats
    // per-widget focus plumbing on every page added later.
    connect(qApp, &QApplication::focusChanged, this, [this](QWidget *, QWidget *now) {
        for (QWidget *widget = now; widget; widget = widget->parentWidget()) {
            auto *group = qobject_cast<QTabWidget *>(widget);
            if (!group || !groups_.contains(group)) {
                continue;
            }
            if (group != activeGroup_ && group->currentIndex() >= 0) {
                setActiveGroup(group, group->currentIndex());
            }
            return;
        }
    });

    // L3: one tooltip for the whole window. The answer is asynchronous,
    // so it is shown where the pointer is when it arrives — safe only
    // because `lsp_core::HoverTracker` has already dropped everything
    // the user has moved on from, so whatever reaches here is still
    // about the word under the cursor.
    connect(languageService_, &LanguageService::hoverReady, this, [](const QString &html) {
        QToolTip::showText(QCursor::pos(), html);
    });

    // L5: a completion answer landed. Only a still-current one is ever
    // signalled (`lsp_core::CompletionTracker`), so the visible editor
    // simply re-reads the candidates for the word it is on.
    connect(languageService_, &LanguageService::completionReady, this, [this]() {
        auto *editor = qobject_cast<CodeEditor *>(
            activeGroup_ ? activeGroup_->currentWidget() : nullptr);
        if (editor) {
            editor->refreshCompletions();
        }
    });

    // C7: the splice for the last acceptCompletion — the item's own edit,
    // merged with a resolved `additionalTextEdits` when there was one to
    // merge — spliced through the same one-edit-block path every other
    // buffer change uses.
    connect(languageService_,
            &LanguageService::completionEditReady,
            this,
            [this](const ::rust::Vec<FfiTextEdit> &edits) {
                auto *editor = qobject_cast<CodeEditor *>(
                    activeGroup_ ? activeGroup_->currentWidget() : nullptr);
                if (editor) {
                    applyEditsTo(editor, edits);
                }
            });

    // C7: a completion-item preview resolution landed for the popup's
    // currently highlighted row.
    connect(languageService_,
            &LanguageService::completionPreviewReady,
            this,
            [this](const QString &detail, const QString &documentation) {
                auto *editor = qobject_cast<CodeEditor *>(
                    activeGroup_ ? activeGroup_->currentWidget() : nullptr);
                if (editor) {
                    editor->updateCompletionPreview(detail, documentation);
                }
            });

    // F2-10/F2-11: every answer landed. Only a still-current one is ever
    // signalled (`lsp_core::RequestTracker`), so each of these only has to
    // decide where to put it — never whether it is still wanted.
    connect(languageService_, &LanguageService::intentionsReady, this,
            &EditorTabs::onIntentionsReady);
    connect(languageService_, &LanguageService::documentHighlightsReady, this,
            &EditorTabs::onDocumentHighlightsReady);
    connect(languageService_, &LanguageService::signatureHelpReady, this,
            &EditorTabs::onSignatureHelpReady);
    connect(languageService_, &LanguageService::inlayHintsReady, this,
            &EditorTabs::onInlayHintsReady);
    connect(languageService_, &LanguageService::semanticTokensReady, this,
            &EditorTabs::onSemanticTokensReady);
    connect(languageService_, &LanguageService::codeLensesReady, this,
            &EditorTabs::onCodeLensesReady);
    // C12-followup: a decompiled/generated source fetch landed. Reuses
    // onTabOpened — the same build-the-widget path DocumentManager::tabOpened
    // drives for a normal file — for a new tab, then always focuses it
    // (mirroring EditorTabs::openFile's own focus-regardless-of-new rule).
    connect(languageService_, &LanguageService::virtualDocumentOpened, this,
            [this](quint64 tabId, const QString &title, bool newlyOpened) {
                if (newlyOpened) {
                    onTabOpened(tabId, title);
                }
                focusTab(tabId);
            });

    activeGroup_ = makeGroup();
    root_->addWidget(activeGroup_);
}

void EditorTabs::setPreviewProvider(PreviewProvider *previewProvider)
{
    previewProvider_ = previewProvider;
}

void EditorTabs::setActiveTabChangedCallback(std::function<void()> callback)
{
    activeTabChanged_ = std::move(callback);
}

void EditorTabs::setPreviewChangedCallback(std::function<void(quint64)> callback)
{
    previewChanged_ = std::move(callback);
}

void EditorTabs::openFile(const QString &path)
{
    const auto result = docManager_->openFile(path);
    if (result.code != 0) {
        QMessageBox::critical(window_, tr("Cannot open file"), result.message);
        return;
    }
    focusTab(result.tab_id);
}

void EditorTabs::openFileAtLine(const QString &path, int line, int column)
{
    recordCurrentPosition();
    openFile(path);
    auto *editor = currentEditor();
    if (!editor) {
        return;
    }
    moveCursorToByteColumn(editor, line, column);
    recordCurrentPosition();
    navigationChanged();
}

void EditorTabs::jumpWithinCurrentTab(int line, int column)
{
    auto *editor = currentEditor();
    if (!editor) {
        return;
    }
    recordCurrentPosition();
    moveCursorToByteColumn(editor, line, column);
    recordCurrentPosition();
    navigationChanged();
}

void EditorTabs::setNavigationChangedCallback(std::function<void()> callback)
{
    navigationChanged_ = std::move(callback);
}

quint64 EditorTabs::byteOffsetAt(int documentPosition) const
{
    auto *editor = currentEditor();
    if (!editor) {
        return 0;
    }
    return static_cast<quint64>(
      editor->toPlainText().left(documentPosition).toUtf8().size());
}

int EditorTabs::caretPosition() const
{
    auto *editor = currentEditor();
    return editor ? editor->textCursor().position() : 0;
}

bool EditorTabs::hasUnsavedChanges() const
{
    for (QTabWidget *group : groups_) {
        for (int i = 0; i < group->count(); ++i) {
            auto *editor = qobject_cast<CodeEditor *>(group->widget(i));
            if (editor && editor->document()->isModified()) {
                return true;
            }
        }
    }
    return false;
}

void EditorTabs::setContextMenuCallback(std::function<void(QMenu *)> callback)
{
    contextMenu_ = std::move(callback);
}

bool EditorTabs::saveAllModified()
{
    bool allSaved = true;
    for (QTabWidget *group : groups_) {
        for (int i = 0; i < group->count(); ++i) {
            auto *editor = qobject_cast<QPlainTextEdit *>(group->widget(i));
            if (editor && editor->document()->isModified() && !saveTab(group, i)) {
                allSaved = false;
            }
        }
    }
    return allSaved;
}

QStringList EditorTabs::openPaths() const
{
    QStringList paths;
    for (QTabWidget *group : groups_) {
        for (int i = 0; i < group->count(); ++i) {
            auto *editor = qobject_cast<CodeEditor *>(group->widget(i));
            const QString path = editor ? editor->property("lspPath").toString() : QString();
            if (!path.isEmpty() && !paths.contains(path)) {
                paths.append(path);
            }
        }
    }
    return paths;
}

CodeEditor *EditorTabs::editorForPath(const QString &path) const
{
    for (QTabWidget *group : groups_) {
        for (int i = 0; i < group->count(); ++i) {
            auto *editor = qobject_cast<CodeEditor *>(group->widget(i));
            if (editor && editor->property("lspPath").toString() == path) {
                return editor;
            }
        }
    }
    return nullptr;
}

QString EditorTabs::wordUnderCursor() const
{
    auto *editor = currentEditor();
    if (!editor) {
        return QString();
    }
    QTextCursor cursor = editor->textCursor();
    cursor.select(QTextCursor::WordUnderCursor);
    return cursor.selectedText();
}

QString EditorTabs::selectedText() const
{
    auto *editor = currentEditor();
    if (!editor) {
        return QString();
    }
    return editor->textCursor().selectedText().replace(QChar(0x2029), QLatin1Char('\n'));
}

QString EditorTabs::currentContent() const
{
    auto *editor = currentEditor();
    return editor ? editor->toPlainText() : QString();
}

void EditorTabs::requestDeclarationAtCaret()
{
    auto *editor = currentEditor();
    if (!editor || !declarationRequested_) {
        return;
    }
    declarationRequested_(editor->textCursor().position());
}

void EditorTabs::setDeclarationRequestedCallback(std::function<void(int)> callback)
{
    declarationRequested_ = std::move(callback);
}

QPlainTextEdit *EditorTabs::currentEditor() const
{
    return activeGroup_ ? qobject_cast<QPlainTextEdit *>(activeGroup_->currentWidget())
                        : nullptr;
}

void EditorTabs::recordCurrentPosition()
{
    auto *editor = currentEditor();
    if (!editor) {
        return;
    }
    const QString path = docManager_->tabPath(currentTabId());
    if (path.isEmpty()) {
        return;
    }
    const QTextCursor cursor = editor->textCursor();
    docManager_->recordJump(path, static_cast<quint32>(cursor.blockNumber() + 1),
                             static_cast<quint32>(cursor.columnNumber()));
}

void EditorTabs::applyHistoryLocation(const FfiLocation &location)
{
    if (!location.found) {
        return;
    }
    // Deliberately not openFileAtLine: walking the history must not
    // record new entries, or Back would push the place it just left
    // and the stack would never move.
    openFile(location.path);
    if (auto *editor = currentEditor()) {
        // moveCursorToLine, not moveCursorToByteColumn: recordCurrentPosition
        // stored QTextCursor::columnNumber(), which is already UTF-16.
        // Converting here would corrupt Back/Forward on non-ASCII lines.
        moveCursorToLine(editor, static_cast<int>(location.line),
                          static_cast<int>(location.column));
    }
    navigationChanged();
}

void EditorTabs::navigationChanged()
{
    if (navigationChanged_) {
        navigationChanged_();
    }
}

void EditorTabs::withFindBar(const std::function<void(FindBar *)> &action)
{
    auto *editor = currentEditor();
    if (!editor) {
        return;
    }
    if (auto *bar = editor->findChild<FindBar *>()) {
        action(bar);
    }
}

quint64 EditorTabs::currentTabId() const
{
    return activeGroup_ ? tabIdAt(activeGroup_, activeGroup_->currentIndex()) : 0;
}

void EditorTabs::jumpToByteOffset(quint64 byteOffset)
{
    auto *editor = currentEditor();
    if (!editor) {
        return;
    }
    recordCurrentPosition();
    const QByteArray utf8 = docManager_->tabContent(currentTabId()).toUtf8();
    const qsizetype clamped = qMin<qsizetype>(static_cast<qsizetype>(byteOffset), utf8.size());
    int line = 0;
    qsizetype lineStart = 0;
    for (qsizetype i = 0; i < clamped; ++i) {
        if (utf8.at(i) == '\n') {
            ++line;
            lineStart = i + 1;
        }
    }
    moveCursorToByteColumn(editor, line + 1,
                            static_cast<int>(qMax<qsizetype>(0, clamped - lineStart)));
    recordCurrentPosition();
    navigationChanged();
}

void EditorTabs::showFindBar()
{ withFindBar([](FindBar *bar) { bar->showFind(); }); }


void EditorTabs::showReplaceBar()
{ withFindBar([](FindBar *bar) { bar->showReplace(); }); }


void EditorTabs::findNext()
{ withFindBar([](FindBar *bar) { bar->findNext(); }); }


void EditorTabs::findPrevious()
{ withFindBar([](FindBar *bar) { bar->findPrevious(); }); }


void EditorTabs::goToLine()
{
    auto *editor = currentEditor();
    if (!editor) {
        return;
    }
    bool ok = false;
    const int line = QInputDialog::getInt(window_, tr("Go to Line"), tr("Line number:"),
                                           editor->textCursor().blockNumber() + 1, 1,
                                           editor->blockCount(), 1, &ok);
    if (ok) {
        jumpWithinCurrentTab(line, 0);
    }
}

void EditorTabs::saveCurrentTab()
{
    if (activeGroup_) {
        saveTab(activeGroup_, activeGroup_->currentIndex());
    }
}

void EditorTabs::saveCurrentTabAs()
{
    if (!activeGroup_) {
        return;
    }
    const int index = activeGroup_->currentIndex();
    if (index < 0) {
        return;
    }
    auto *editor = qobject_cast<QPlainTextEdit *>(activeGroup_->widget(index));
    if (!editor) {
        return;
    }
    const QString path = QFileDialog::getSaveFileName(window_, tr("Save As"));
    if (path.isEmpty()) {
        return;
    }
    const quint64 tabId = tabIdAt(activeGroup_, index);
    const auto result = docManager_->saveTabAs(tabId, path, editor->toPlainText());
    if (result.code != 0) {
        QMessageBox::critical(window_, tr("Cannot save file"), result.message);
        return;
    }
    // The tab now backs a different file: the server has to be told about
    // both halves of that move.
    const QString previous = editor->property("lspPath").toString();
    if (!previous.isEmpty()) {
        languageService_->documentClosed(previous);
    }
    editor->setProperty("lspPath", path);
    languageService_->documentOpened(path, editor->toPlainText());
    editor->document()->setModified(false);
    renderTabText(activeGroup_, index, docManager_->tabTitle(tabId), false);
}

void EditorTabs::attachStatusBar(QLabel *positionLabel, QLabel *languageLabel)
{
    positionLabel_ = positionLabel;
    languageLabel_ = languageLabel;
    updateStatusBar();
}

bool EditorTabs::confirmCloseAllTabs()
{
    for (QTabWidget *group : std::as_const(groups_)) {
        for (int i = 0; i < group->count(); ++i) {
            if (!confirmCloseTab(group, i)) {
                return false;
            }
        }
    }
    return true;
}

void EditorTabs::setEditorFont(const QFont &font)
{
    editorFont_ = font;
    forEachEditor([&font](QPlainTextEdit *editor) { editor->setFont(font); });
    forEachHexViewer([&font](HexViewer *viewer) {
        viewer->setFont(font);
        viewer->refreshMetrics();
    });
}

void EditorTabs::setWhitespaceOptions(const WhitespaceOptions &options)
{
    whitespaceOptions_ = options;
    forEachEditor([&options](QPlainTextEdit *editor) {
        if (auto *codeEditor = qobject_cast<CodeEditor *>(editor)) {
            codeEditor->setWhitespaceOptions(options);
        }
    });
}

void EditorTabs::setInlayHintsEnabled(bool enabled)
{
    inlayHintsEnabled_ = enabled;
    forEachEditor([this, enabled](QPlainTextEdit *editor) {
        auto *codeEditor = qobject_cast<CodeEditor *>(editor);
        if (!codeEditor) {
            return;
        }
        codeEditor->setInlayHintsEnabled(enabled);
        if (enabled) {
            requestInlayHintsFor(codeEditor);
        }
    });
}

void EditorTabs::collapseAllFolds()
{
    if (auto *codeEditor = qobject_cast<CodeEditor *>(currentEditor())) {
        codeEditor->collapseAll();
    }
}

void EditorTabs::expandAllFolds()
{
    if (auto *codeEditor = qobject_cast<CodeEditor *>(currentEditor())) {
        codeEditor->expandAll();
    }
}

void EditorTabs::setEditorColors(const QString &backgroundHex, const QString &foregroundHex,
                      const QString &currentLineHex)
{
    editorBackground_ = backgroundHex;
    editorForeground_ = foregroundHex;
    editorCurrentLine_ = currentLineHex;
    forEachEditor([this](QPlainTextEdit *editor) { applyEditorAppearance(editor); });
    forEachHexViewer([this](HexViewer *viewer) { applyEditorPalette(viewer); });
}

void EditorTabs::reloadHighlighterLanguages()
{
    forEachEditor([](QPlainTextEdit *editor) {
        if (auto *highlighter = highlighterOf(editor->document())) {
            highlighter->reloadLanguage();
            highlighter->rehighlight();
        }
    });
}

void EditorTabs::refreshTabIcons()
{
    for (QTabWidget *group : std::as_const(groups_)) {
        for (int index = 0; index < group->count(); ++index) {
            const quint64 tabId = tabIdAt(group, index);
            renderTabText(group, index, docManager_->tabTitle(tabId),
                          docManager_->tabIsModified(tabId));
        }
    }
}

void EditorTabs::onSemanticTokensReady(const QString &path)
{
    CodeEditor *editor = editorForPath(path);
    if (!editor) {
        return;
    }
    auto *highlighter = highlighterOf(editor->document());
    if (!highlighter) {
        return;
    }
    highlighter->applySemanticTokens(languageService_->semanticTokenSpans(path));
}

void EditorTabs::onCodeLensesReady(const QString &path)
{
    CodeEditor *editor = editorForPath(path);
    if (!editor) {
        return;
    }
    QVector<CodeLensSpan> lenses;
    for (const FfiCodeLens &lens : languageService_->codeLenses(path)) {
        lenses.append(
          CodeLensSpan{ static_cast<int>(lens.line), QString(lens.label), lens.clickable });
    }
    editor->setCodeLenses(lenses);
}

void EditorTabs::refreshHighlighting()
{
    forEachEditor([](QPlainTextEdit *editor) {
        if (auto *highlighter = highlighterOf(editor->document())) {
            highlighter->invalidatePalette();
            highlighter->rehighlight();
        }
    });
}

void EditorTabs::onTabTitleChanged(quint64 tabId, const QString &title)
{
    const TabLoc loc = locate(tabId);
    if (!loc.group) {
        return;
    }
    renderTabText(loc.group, loc.index, title, docManager_->tabIsModified(tabId));
}

void EditorTabs::onBufferEditedExternally(quint64 tabId, const QString &content)
{
    auto *editor = editorForTab(tabId);
    if (!editor) {
        return;
    }
    editor->setPlainText(content);
    editor->document()->setModified(true);
}

void EditorTabs::handleExternalChange(quint64 tabId, const QString &path)
{
    auto *editor = editorForTab(tabId);
    if (!editor) {
        return;
    }

    QMessageBox box(QMessageBox::Warning,
                     tr("File changed on disk"),
                     tr("\"%1\" was modified outside the editor.")
                       .arg(QFileInfo(path).fileName()),
                     QMessageBox::NoButton,
                     window_);
    QPushButton *reloadButton = box.addButton(tr("Reload"), QMessageBox::AcceptRole);
    box.addButton(tr("Keep My Version"), QMessageBox::RejectRole);
    box.setDefaultButton(reloadButton);
    box.exec();

    if (box.clickedButton() == reloadButton) {
        const auto result = docManager_->reloadTabFromDisk(tabId);
        if (result.code != 0) {
            QMessageBox::critical(window_, tr("Cannot reload file"), result.message);
            return;
        }
        editor->setPlainText(docManager_->tabContent(tabId));
        editor->document()->setModified(false);
    } else {
        editor->document()->setModified(true);
    }
}

void EditorTabs::renderTabText(QTabWidget *group, int index, const QString &title, bool modified)
{
    group->setTabText(index, modified ? title + QStringLiteral(" •") : title);
    group->setTabIcon(index,
                      fileIcon(docManager_->tabPath(tabIdAt(group, index)),
                               group->iconSize().width()));
}

bool EditorTabs::saveTab(QTabWidget *group, int index)
{
    auto *codeEditor = qobject_cast<CodeEditor *>(group->widget(index));
    auto *editor = qobject_cast<QPlainTextEdit *>(group->widget(index));
    if (!editor) {
        return false;
    }
    return saveEditor(tabIdAt(group, index), codeEditor, editor);
}

bool EditorTabs::saveEditor(quint64 tabId, CodeEditor *codeEditor, QPlainTextEdit *editor)
{
    // C12-followup: a read-only tab (virtual document, hex, diff) has
    // nothing to save — Save is a no-op rather than a click that always
    // fails against `AppSession::save_tab`'s own refusal.
    if (editor->isReadOnly()) {
        return true;
    }
    // F1-11: trim, final newline and line-ending normalisation, applied
    // *before* the file is read for writing — so they are one undo entry,
    // separate from whatever the user's last edit was, and the caret lands
    // wherever the splice's own cursor adjustment puts it rather than
    // jumping to column 0. Only real editors have this (a hex tab has no
    // language and nothing to tidy).
    if (codeEditor) {
        const ::rust::Vec<FfiTextEdit> tidyEdits =
          editorOps_->saveRuleEdits(tabId, editor->toPlainText());
        if (!tidyEdits.empty()) {
            applyEditsTo(editor, tidyEdits);
        }
    }
    const auto result = docManager_->saveTab(tabId, editor->toPlainText());
    if (result.code != 0) {
        QMessageBox::critical(window_, tr("Cannot save file"), result.message);
        return false;
    }
    editor->document()->setModified(false);
    if (codeEditor) {
        refreshCarets(codeEditor);
    }
    const QString path = editor->property("lspPath").toString();
    if (!path.isEmpty()) {
        // Servers that only re-analyse on save (and linters behind them)
        // need this; the buffer itself already went across as didChange.
        languageService_->documentSaved(path);
    }
    if (vcsService_ && vcsService_->isRepository()) {
        // A save is exactly what `changedFiles()` (the Changes dock, the
        // status bar's branch widget) is supposed to answer about; nothing
        // else asks `VcsService` to look again after one. The gutter's own
        // hunks already track the live buffer via `requestHunksFor`'s
        // didChange debounce — this is the same freshness rule for the
        // whole-repo status, which only ever changes relative to disk.
        vcsService_->refreshStatus();
    }
    return true;
}

bool EditorTabs::confirmCloseTab(QTabWidget *group, int index)
{
    if (!docManager_->tabIsModified(tabIdAt(group, index))) {
        return true;
    }

    const auto choice = QMessageBox::question(
      window_,
      tr("Unsaved changes"),
      tr("\"%1\" has unsaved changes. Save before closing?").arg(group->tabText(index)),
      QMessageBox::Save | QMessageBox::Discard | QMessageBox::Cancel,
      QMessageBox::Save);

    if (choice == QMessageBox::Cancel) {
        return false;
    }
    if (choice == QMessageBox::Save) {
        return saveTab(group, index);
    }
    return true; // Discard.
}

void EditorTabs::requestCloseTab(QTabWidget *group, int index)
{
    if (!confirmCloseTab(group, index)) {
        return;
    }
    docManager_->closeTab(tabIdAt(group, index));
}

void EditorTabs::updateStatusBar()
{
    if (!positionLabel_ || !languageLabel_) {
        return;
    }
    auto *editor = currentEditor();
    if (!editor) {
        const quint64 tabId = currentTabId();
        if (tabId != 0 && docManager_->tabKind(tabId) == kTabKindBinary) {
            positionLabel_->setText(
              QObject::tr("%L1 bytes").arg(docManager_->binaryLength(tabId)));
            languageLabel_->setText(QObject::tr("Binary"));
            return;
        }
        positionLabel_->clear();
        languageLabel_->clear();
        return;
    }
    const QTextCursor cursor = editor->textCursor();
    positionLabel_->setText(QObject::tr("Ln %1, Col %2")
                               .arg(cursor.blockNumber() + 1)
                               .arg(cursor.columnNumber() + 1));
    languageLabel_->setText(docManager_->tabLanguageName(currentTabId()));
}

void EditorTabs::applyEditorAppearance(QPlainTextEdit *editor)
{
    applyEditorPalette(editor);
    // The current-line band is not a QPalette role, so it can't ride
    // along with the palette and is pushed to the editor separately.
    if (auto *codeEditor = qobject_cast<CodeEditor *>(editor)) {
        codeEditor->setCurrentLineColor(editorCurrentLine_);
    }
}

void EditorTabs::applyEditorPalette(QWidget *editor)
{
    QPalette pal = qApp->palette();
    if (!editorBackground_.isEmpty()) {
        pal.setColor(QPalette::Base, QColor(editorBackground_));
    }
    if (!editorForeground_.isEmpty()) {
        pal.setColor(QPalette::Text, QColor(editorForeground_));
    }
    editor->setPalette(pal);
}

void EditorTabs::markTab(const char *event, quint64 tabId, QTabWidget *group, int index,
              const QString &title)
{
    // `rect` is the tab's own label on screen, in global coordinates. A
    // test that has to click a tab would otherwise compute it from the
    // window geometry, the style's metrics and the font — three things
    // that move for reasons unrelated to whatever it is testing.
    const QRect rect = group->tabBar()->tabRect(index);
    const QPoint origin = rect.isEmpty() ? QPoint() : group->tabBar()->mapToGlobal(rect.topLeft());
    e2eMark(QStringLiteral("{\"ev\":\"%1\",\"index\":%2,\"tab_id\":%3,\"pane\":%4,"
                            "\"rect\":[%5,%6,%7,%8],\"title\":%9}")
              .arg(QLatin1String(event))
              .arg(index)
              .arg(tabId)
              .arg(groups_.indexOf(group))
              .arg(origin.x())
              .arg(origin.y())
              .arg(rect.width())
              .arg(rect.height())
              .arg(e2eJson(title)));
}

void EditorTabs::addHexTab(QTabWidget *group, quint64 tabId, const QString &title)
{
    auto *viewer = new HexViewer(group);
    viewer->setProperty("tabId", QVariant::fromValue(tabId));
    viewer->setFont(editorFont_);
    viewer->setRowCount(docManager_->binaryRowCount(tabId));
    viewer->setRowProvider([this, tabId](quint64 firstRow, int count) {
        QVector<HexRow> rows;
        const auto ffiRows = docManager_->hexRows(tabId, firstRow, static_cast<quint64>(count));
        rows.reserve(static_cast<int>(ffiRows.size()));
        for (const auto &row : ffiRows) {
            rows.append(HexRow{ row.offset, row.hex, row.ascii });
        }
        return rows;
    });
    group->addTab(viewer, title);
    renderTabText(group, group->indexOf(viewer), title, false);
    markTab("tab_added", tabId, group, group->indexOf(viewer), title);
}

void EditorTabs::addDiffTab(QTabWidget *group, quint64 tabId, const QString &title)
{
    const QString path = docManager_->tabPath(tabId);
    auto *diffView = new DiffView(docManager_->diffLeftText(tabId), docManager_->diffRightText(tabId),
                                    docManager_->diffHunks(tabId), docManager_->diffSpans(tabId), path);
    auto *page = new DiffViewPage(diffView, docManager_->diffLeftLabel(tabId),
                                    docManager_->diffRightLabel(tabId), group);
    page->setProperty("tabId", QVariant::fromValue(tabId));
    page->onIgnoreWhitespaceToggled = [this, diffView, tabId](bool ignore) {
        const QString left = docManager_->diffLeftText(tabId);
        const QString right = docManager_->diffRightText(tabId);
        diffView->setHunks(docManager_->diffHunksBetween(left, right, ignore),
                             docManager_->diffSpansBetween(left, right, ignore));
    };
    group->addTab(page, title);
    renderTabText(group, group->indexOf(page), title, false);
    markTab("tab_added", tabId, group, group->indexOf(page), title);
}

void EditorTabs::onTabClosed(quint64 tabId)
{
    // F1-13: drop the carets and the expand/shrink stack this tab
    // accumulated, or the map grows for the life of the process.
    editorOps_->forgetTab(tabId);

    // The tab's own page is a placeholder while its editor lives in a
    // floating diff window (F3-14) — the file is going away regardless of
    // whether its diff was ever closed, so the window (and the editor still
    // inside it) closes with it rather than leaking a window over a tab
    // that no longer exists.
    if (const auto it = diffWindows_.constFind(tabId); it != diffWindows_.constEnd()) {
        delete it->window;
        diffWindows_.remove(tabId);
    }

    const TabLoc loc = locate(tabId);
    if (!loc.group) {
        return;
    }
    QWidget *widget = loc.group->widget(loc.index);
    const QString path = widget->property("lspPath").toString();
    if (!path.isEmpty()) {
        languageService_->documentClosed(path);
        if (vcsService_) {
            // Same reasoning as documentClosed above, for the gutter's own
            // per-path state: the cached hunks hold this file's whole HEAD
            // text and whole working text.
            vcsService_->forgetPath(path);
        }
    }
    loc.group->removeTab(loc.index);
    delete widget;
    markTab("tab_closed", tabId, loc.group, loc.index, QString());
    if (loc.group->count() == 0) {
        collapseGroup(loc.group);
    }
    markPaneCount();
}

void EditorTabs::onTabModifiedChanged(quint64 tabId, bool modified)
{
    const TabLoc loc = locate(tabId);
    if (!loc.group) {
        return;
    }
    renderTabText(loc.group, loc.index, docManager_->tabTitle(tabId), modified);
    e2eMark(QStringLiteral("{\"ev\":\"tab_dirty\",\"index\":%1,\"tab_id\":%2,\"dirty\":%3}")
              .arg(loc.index)
              .arg(tabId)
              .arg(modified ? QLatin1String("true") : QLatin1String("false")));
}

} // namespace ui_shell
