#include "editor_tabs.h"

#include "code_editor.h"
#include "e2e_mark.h"
#include "find_bar.h"
#include "intention_bulb.h"
#include "problems_panel.h"
#include "signature_tip.h"
#include "syntax_highlighter.h"

#include <QAction>
#include <QCursor>
#include <QHash>
#include <QMenu>
#include <QPlainTextEdit>
#include <QScrollBar>
#include <QTabWidget>
#include <QTextBlock>
#include <QTextCursor>
#include <QTimer>
#include <QVariant>
#include <QVector>
#include <QtGui/QTextDocument>

namespace ui_shell {

namespace {

// A document (UTF-16) position as the line/character pair LSP speaks: both
// 0-based, characters counted in UTF-16 code units — which is exactly what
// QTextCursor counts, so this is a re-expression, not a conversion (unlike
// byteOffsetAt, which the byte-addressed index needs).
QPair<quint32, quint32> lspPosition(QPlainTextEdit *editor, int documentPosition)
{
    QTextCursor cursor(editor->document());
    cursor.setPosition(documentPosition);
    return {static_cast<quint32>(cursor.blockNumber()),
            static_cast<quint32>(cursor.positionInBlock())};
}

} // namespace

QPair<quint32, quint32> EditorTabs::lspPositionAt(int documentPosition) const
{
    auto *editor = currentEditor();
    return editor ? lspPosition(editor, documentPosition) : QPair<quint32, quint32>{0, 0};
}

int EditorTabs::documentRevision() const
{
    auto *editor = currentEditor();
    return editor ? static_cast<int>(editor->document()->revision()) : 0;
}

QPair<QPair<quint32, quint32>, QPair<quint32, quint32>> EditorTabs::selectionRange() const
{
    auto *editor = currentEditor();
    if (!editor) {
        return {{0, 0}, {0, 0}};
    }
    const QTextCursor cursor = editor->textCursor();
    return {lspPosition(editor, cursor.selectionStart()),
            lspPosition(editor, cursor.selectionEnd())};
}

namespace {

// Splice one edit through an already-open cursor. Positions resolve against
// the document as it stands, which is why every producer hands its edits
// over descending.
void spliceEdit(QTextCursor &cursor, const FfiTextEdit &edit)
{
    cursor.setPosition(
      EditorTabs::positionAt(cursor.document(), edit.start_line, edit.start_character));
    cursor.setPosition(EditorTabs::positionAt(cursor.document(), edit.end_line, edit.end_character),
                       QTextCursor::KeepAnchor);
    cursor.insertText(edit.new_text);
}

} // namespace

void EditorTabs::applyBufferEdits(const ::rust::Vec<FfiTextEdit> &edits)
{
    // One cursor per file, held open for the whole splice: `beginEditBlock`
    // and `endEditBlock` run on the *same* QTextCursor, which is what makes
    // every edit to a file one Ctrl+Z (ADR-0019, ADR-0023). Hence the
    // insert-then-begin order — beginning on a temporary and storing a copy
    // of it would leave the two halves on different cursor objects.
    QHash<QString, QTextCursor> cursors;
    for (const FfiTextEdit &edit : edits) {
        if (!edit.in_buffer) {
            continue;
        }
        const QString path = edit.path;
        if (!cursors.contains(path)) {
            CodeEditor *editor = editorForPath(path);
            if (!editor) {
                continue;
            }
            cursors.insert(path, QTextCursor(editor->document()));
            cursors[path].beginEditBlock();
        }
        spliceEdit(cursors[path], edit);
    }
    for (QTextCursor &cursor : cursors) {
        cursor.endEditBlock();
    }
}

void EditorTabs::applyEditsTo(QPlainTextEdit *editor, const ::rust::Vec<FfiTextEdit> &edits)
{
    // The whole transaction inside one edit block, which is what makes a
    // keystroke at 200 carets one Ctrl+Z (ADR-0023). The edits name no file
    // because they are all this buffer's.
    QTextCursor cursor(editor->document());
    cursor.beginEditBlock();
    for (const FfiTextEdit &edit : edits) {
        spliceEdit(cursor, edit);
    }
    cursor.endEditBlock();
}

void EditorTabs::refreshCarets(CodeEditor *editor)
{
    if (!editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString text = editor->toPlainText();

    QVector<SecondaryCaret> secondary;
    QTextCursor primary = editor->textCursor();
    bool sawPrimary = false;
    for (const FfiCaret &caret : editorOps_->carets(tabId, text)) {
        if (caret.primary) {
            primary.setPosition(static_cast<int>(caret.anchor));
            primary.setPosition(static_cast<int>(caret.head), QTextCursor::KeepAnchor);
            sawPrimary = true;
            continue;
        }
        secondary.append(SecondaryCaret{static_cast<int>(caret.anchor),
                                        static_cast<int>(caret.head)});
    }

    // Guarded, because setTextCursor emits cursorPositionChanged and that
    // handler pushes the widget's caret back to Rust — which would replace
    // the set that was just computed with a single caret.
    syncingCarets_ = true;
    if (sawPrimary) {
        editor->setTextCursor(primary);
    }
    editor->setSecondaryCarets(secondary);
    syncingCarets_ = false;
}

void EditorTabs::withCurrentEditor(const std::function<void(quint64, const QString &)> &op)
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    op(editor->property("tabId").toULongLong(), editor->toPlainText());
    refreshCarets(editor);
}

void EditorTabs::jumpToMatchingBracket()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const qint64 target = editorOps_->matchingBracket(
      tabId, editor->toPlainText(), static_cast<quint32>(editor->textCursor().position()));
    if (target < 0) {
        return;
    }
    QTextCursor cursor = editor->textCursor();
    cursor.setPosition(static_cast<int>(target));
    editor->setTextCursor(cursor);
}

void EditorTabs::showIntentionsNow()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    requestIntentionsFor(editor, true);
}

void EditorTabs::requestIntentionsFor(CodeEditor *editor, bool explicitRequest)
{
    const QString path = editor->property("lspPath").toString();
    if (path.isEmpty()) {
        return;
    }
    const int position = editor->textCursor().position();
    const QPair<quint32, quint32> at = lspPosition(editor, position);
    intentionsEditor_ = editor;
    intentionsDocPos_ = position;
    intentionsPending_ = explicitRequest;
    languageService_->requestIntentions(path, at.first, at.second);
    languageService_->requestDocumentHighlights(path, at.first, at.second);
}

void EditorTabs::onDocumentHighlightsReady()
{
    if (!intentionsEditor_) {
        return;
    }
    QVector<OccurrenceSpan> spans;
    const QTextDocument *document = intentionsEditor_->document();
    for (const FfiDocumentHighlight &highlight : languageService_->documentHighlights()) {
        const int start = positionAt(document, highlight.start_line, highlight.start_character);
        const int end = positionAt(document, highlight.end_line, highlight.end_character);
        if (end <= start) {
            continue;
        }
        spans.append(OccurrenceSpan{start, end, highlight.kind == FfiHighlightKind::Write});
    }
    intentionsEditor_->setOccurrenceSpans(spans);
}

void EditorTabs::requestSignatureHelpFor(CodeEditor *editor, bool explicitRequest)
{
    const QString path = editor->property("lspPath").toString();
    if (path.isEmpty()) {
        return;
    }
    signatureHelpEditor_ = editor;
    languageService_->requestSignatureHelp(path, editor->toPlainText(),
                                           static_cast<quint64>(editor->textCursor().position()),
                                           explicitRequest, signatureTipVisible_);
}

void EditorTabs::requestSignatureHelpNow()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    requestSignatureHelpFor(editor, true);
}

void EditorTabs::organizeImports()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    const QString path = currentPath();
    if (!editor || path.isEmpty()) {
        return;
    }
    languageService_->organizeImports(
      path, static_cast<quint32>(editor->document()->blockCount() - 1), documentRevision());
}

void EditorTabs::onSignatureHelpReady()
{
    if (!signatureHelpEditor_) {
        return;
    }
    const FfiSignatureHelp help = languageService_->signatureHelp();
    signatureTipVisible_ = help.has_signature;
    if (!help.has_signature) {
        hideSignatureTip();
        return;
    }
    const QRect rect = signatureHelpEditor_->cursorRect();
    showSignatureTip(signatureHelpEditor_,
                     signatureHelpEditor_->viewport()->mapToGlobal(QPoint(rect.left(), rect.top())),
                     help);
}

void EditorTabs::requestInlayHintsFor(CodeEditor *editor)
{
    if (!editor->inlayHintsEnabled()) {
        return;
    }
    const QString path = editor->property("lspPath").toString();
    if (path.isEmpty()) {
        return;
    }
    inlayHintsEditor_ = editor;
    const int firstLine = editor->cursorForPosition(QPoint(0, 0)).blockNumber();
    const int lastLine =
      editor->cursorForPosition(QPoint(0, editor->viewport()->height())).blockNumber();
    languageService_->requestInlayHints(path, static_cast<quint32>(firstLine),
                                        static_cast<quint32>(lastLine));
}

void EditorTabs::onInlayHintsReady()
{
    if (!inlayHintsEditor_) {
        return;
    }
    QVector<InlayHintSpan> hints;
    const QTextDocument *document = inlayHintsEditor_->document();
    for (const FfiInlayHint &hint : languageService_->inlayHints()) {
        hints.append(InlayHintSpan{positionAt(document, hint.line, hint.character),
                                   QString(hint.label), hint.padding_left, hint.padding_right});
    }
    inlayHintsEditor_->setInlayHints(hints);
}

void EditorTabs::requestSemanticTokensFor(CodeEditor *editor)
{
    const QString path = editor->property("lspPath").toString();
    if (path.isEmpty()) {
        return;
    }
    languageService_->requestSemanticTokens(path, editor->toPlainText());
}

void EditorTabs::requestCodeLensesFor(CodeEditor *editor)
{
    const QString path = editor->property("lspPath").toString();
    if (path.isEmpty()) {
        return;
    }
    languageService_->requestCodeLenses(path);
}

void EditorTabs::onIntentionsReady()
{
    const bool wasPending = intentionsPending_;
    intentionsPending_ = false;
    if (!intentionsEditor_) {
        return;
    }
    const ::rust::Vec<FfiIntention> items = languageService_->intentions();
    if (items.empty()) {
        if (intentionBulb_) {
            intentionBulb_->hide();
        }
        return;
    }

    if (!intentionBulb_) {
        intentionBulb_ = new IntentionBulb(intentionsEditor_->viewport());
        connect(intentionBulb_, &IntentionBulb::activated, this,
                [this]() { showIntentionsMenu(); });
    } else {
        intentionBulb_->setParent(intentionsEditor_->viewport());
    }
    QTextCursor cursor(intentionsEditor_->document());
    cursor.setPosition(intentionsDocPos_);
    const QRect caretRect = intentionsEditor_->cursorRect(cursor);
    intentionBulb_->move(2, caretRect.top());
    intentionBulb_->show();
    intentionBulb_->raise();

    if (wasPending) {
        showIntentionsMenu();
    }
}

void EditorTabs::showIntentionsMenu()
{
    if (!intentionsEditor_) {
        return;
    }
    const ::rust::Vec<FfiIntention> items = languageService_->intentions();
    if (items.empty()) {
        return;
    }
    QMenu menu(window_);
    FfiIntentionGroup lastGroup = items[0].group;
    for (std::size_t i = 0; i < items.size(); ++i) {
        if (i > 0 && items[i].group != lastGroup) {
            menu.addSeparator();
        }
        lastGroup = items[i].group;
        const QString reason = items[i].disabled_reason;
        QAction *entry = menu.addAction(reason.isEmpty()
                                          ? QString(items[i].title)
                                          : tr("%1 — %2").arg(QString(items[i].title), reason));
        entry->setEnabled(reason.isEmpty());
        const quint32 index = static_cast<quint32>(i);
        connect(entry, &QAction::triggered, this, [this, index]() {
            languageService_->applyIntention(index, documentRevision());
        });
    }
    const QPoint pos = intentionBulb_ && intentionBulb_->isVisible()
      ? intentionBulb_->mapToGlobal(QPoint(0, intentionBulb_->height()))
      : QCursor::pos();
    // A popup menu takes a keyboard grab rather than the input focus, so
    // this mark is the only way anything outside the process can know it
    // is up — same reasoning as the tab context menu's.
    e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"intentions_menu\",\"count\":%1}")
              .arg(items.size()));
    QAction *chosen = menu.exec(pos);
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"intentions_menu\","
                            "\"accepted\":%1}")
              .arg(chosen != nullptr ? QLatin1String("true") : QLatin1String("false")));
}

void EditorTabs::runEditorOp(
  const std::function<::rust::Vec<FfiTextEdit>(quint64, const QString &)> &op)
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const ::rust::Vec<FfiTextEdit> edits = op(tabId, editor->toPlainText());
    if (!edits.empty()) {
        applyEditsTo(editor, edits);
    }
    refreshCarets(editor);
}

void EditorTabs::setHoverFallbackCallback(std::function<void()> callback)
{
    hoverFallback_ = std::move(callback);
}

void EditorTabs::setHoverCanceledCallback(std::function<void()> callback)
{
    hoverCanceled_ = std::move(callback);
}

void EditorTabs::hoverFallback()
{
    if (hoverFallback_) {
        hoverFallback_();
    }
}

void EditorTabs::hoverCanceled()
{
    if (hoverCanceled_) {
        hoverCanceled_();
    }
}

int EditorTabs::positionAt(const QTextDocument *document, quint32 line, quint32 character)
{
    const QTextBlock block = document->findBlockByNumber(static_cast<int>(line));
    if (!block.isValid()) {
        return document->characterCount() - 1;
    }
    const int within = qMin(static_cast<int>(character), block.length() - 1);
    return block.position() + within;
}

void EditorTabs::applyDiagnostics()
{
    forEachEditor([this](QPlainTextEdit *editor) {
        auto *codeEditor = qobject_cast<CodeEditor *>(editor);
        const QString path = editor->property("lspPath").toString();
        if (!codeEditor) {
            return;
        }
        QVector<DiagnosticSpan> spans;
        if (!path.isEmpty()) {
            const QTextDocument *document = editor->document();
            for (const FfiDiagnostic &row : diagnosticsService_->diagnosticsForFile(path)) {
                // LSP line/character are UTF-16 code units, which is what
                // QTextBlock/QTextCursor count too — so this is arithmetic,
                // not a re-encoding (contrast SyntaxHighlighter, which has
                // to map tree-sitter's UTF-8 byte offsets).
                const QTextBlock startBlock =
                  document->findBlockByNumber(static_cast<int>(row.line) - 1);
                if (!startBlock.isValid()) {
                    continue;
                }
                const QTextBlock endBlock =
                  document->findBlockByNumber(static_cast<int>(row.end_line) - 1);
                const int start =
                  startBlock.position()
                  + qMin(static_cast<int>(row.column), startBlock.length() - 1);
                int end = start;
                if (endBlock.isValid()) {
                    end = endBlock.position()
                          + qMin(static_cast<int>(row.end_column), endBlock.length() - 1);
                }
                if (end <= start) {
                    // A zero-width range still has to be visible.
                    end = qMin(start + 1, startBlock.position() + startBlock.length() - 1);
                }
                spans.append(DiagnosticSpan{start, end, severityColor(row.severity)});
            }
        }
        codeEditor->setDiagnosticSpans(spans);
        if (!path.isEmpty()) {
            // The only way anything outside the process can know this
            // file's squiggles caught up with whichever source just
            // changed — the same reason `ProblemsPanel::applyFilter`
            // reports `problems_refreshed` for its own dock.
            e2eMark(QStringLiteral("{\"ev\":\"diagnostics_applied\",\"path\":%1,\"count\":%2}")
                      .arg(e2eJson(path))
                      .arg(spans.size()));
        }
    });
}

void EditorTabs::setDiagnosticsService(DiagnosticsService *diagnosticsService)
{
    diagnosticsService_ = diagnosticsService;
}

DiagnosticsService *wireDiagnosticsService(QObject *parent, LanguageService *languageService,
                                           BuildService *buildService,
                                           AnalysisService *analysisService,
                                           EditorTabs *editorTabs)
{
    // ADR-0046: the single QObject the Problems dock and the editor read;
    // any source's `diagnosticsChanged` means this file's squiggles (and
    // the panel's rows, wired the same way from `problemsPanel.cpp`) may
    // have changed.
    auto *diagnosticsService = new DiagnosticsService(parent);
    editorTabs->setDiagnosticsService(diagnosticsService);
    QObject::connect(languageService, &LanguageService::diagnosticsChanged, editorTabs,
                      [editorTabs]() { editorTabs->applyDiagnostics(); });
    QObject::connect(buildService, &BuildService::diagnosticsChanged, editorTabs,
                      [editorTabs]() { editorTabs->applyDiagnostics(); });
    QObject::connect(analysisService, &AnalysisService::diagnosticsChanged, editorTabs,
                      [editorTabs]() { editorTabs->applyDiagnostics(); });
    return diagnosticsService;
}

void EditorTabs::reannounceDocuments()
{
    forEachEditor([this](QPlainTextEdit *editor) {
        const QString path = editor->property("lspPath").toString();
        if (!path.isEmpty()) {
            languageService_->reopenDocument(path, editor->toPlainText());
        }
    });
}

void EditorTabs::onTabOpened(quint64 tabId, const QString &title)
{
    QTabWidget *group = activeGroup_ ? activeGroup_ : groups_.first();
    // Which page a tab needs is the session's answer, not a guess made
    // here from the path or the bytes (ADR-0002, ADR-0020).
    if (docManager_->tabKind(tabId) == kTabKindBinary) {
        addHexTab(group, tabId, title);
        return;
    }
    if (docManager_->tabKind(tabId) == kTabKindDiff) {
        addDiffTab(group, tabId, title);
        return;
    }
    if (docManager_->tabKind(tabId) == kTabKindImage) {
        addImageTab(group, tabId, title);
        return;
    }
    auto *editor = new CodeEditor(group);
    editor->setProperty("tabId", QVariant::fromValue(tabId));
    editor->setPlainText(docManager_->tabContent(tabId));
    editor->document()->setModified(false);
    // C12-followup: a virtual document (decompiled/generated source) has
    // nowhere to save to. `saveEditor` also refuses on `isReadOnly()`, so
    // Save is a no-op rather than a click that always fails.
    editor->setReadOnly(docManager_->tabIsReadOnly(tabId));
    editor->setFont(editorFont_);
    editor->setInlayHintsEnabled(inlayHintsEnabled_);
    applyEditorAppearance(editor);
    // Show-whitespace-characters task: the classifier is a plain callback
    // into editorOps_ (kept off the header so CodeEditor stays decoupled
    // from the cxx-qt generated types, same as every other FFI-to-view
    // conversion in this file), and the tab width is resolved once per
    // tab open — it depends on the tab's language, which does not change
    // for the tab's lifetime.
    editor->setWhitespaceOptions(whitespaceOptions_);
    editor->setMinimapOptions(minimapOptions_);
    editor->setWhitespaceClassifier([this](const QString &text) {
        QVector<WhitespaceSpan> spans;
        for (const FfiWhitespaceSpan &span : editorOps_->whitespaceSpans(text)) {
            spans.append(WhitespaceSpan{static_cast<int>(span.line),
                                        static_cast<int>(span.column), span.is_tab,
                                        static_cast<int>(span.category)});
        }
        return spans;
    });
    editor->setEditorTabWidth(static_cast<int>(editorOps_->tabWidthForTab(tabId)));
    // Y2: self-parents to editor->document(), no manual lifetime
    // management needed. Plain text (a file no language claims) yields
    // no spans from the incremental highlighter, so this is a
    // harmless no-op then. `editor` (Task C) lets it push fold ranges
    // to the gutter on the same revision-change hook that already
    // drives highlighting.
    new SyntaxHighlighter(editor->document(), docManager_->tabFileName(tabId), editor);
    // Find (F3): one bar per editor, floated over it, hidden until
    // Ctrl+F/Ctrl+R. Parented to the editor, so it dies with the tab.
    new FindBar(editor, docManager_);

    // N7: Ctrl+Click on an identifier. The widget reports the gesture
    // and its document position; everything about what that
    // identifier *means* is decided outside the view.
    connect(editor, &CodeEditor::declarationRequested, this, [this](int position) {
        if (declarationRequested_) {
            declarationRequested_(position);
        }
    });

    // L3: the pointer dwelled over an identifier. Asking is free of the
    // UI thread (LanguageService answers from its worker), and a file
    // with no server never got an lspPath, so nothing is asked for it.
    connect(editor, &CodeEditor::hoverRequested, this, [this, editor](int position) {
        hoverPosition_ = position;
        const QString path = editor->property("lspPath").toString();
        // RF12: a file whose language has no server never got an
        // lspPath, so there is nobody to ask — which is precisely when
        // the index's declaration answers instead.
        if (path.isEmpty()) {
            hoverFallback();
            return;
        }
        const QPair<quint32, quint32> at = lspPosition(editor, position);
        languageService_->hoverAt(path, at.first, at.second);
    });
    // Right-click: the window decides what goes in beyond Qt's own
    // entries, so this only forwards the menu.
    connect(editor, &CodeEditor::contextMenuAboutToShow, this, [this](QMenu *menu) {
        if (contextMenu_) {
            contextMenu_(menu);
        }
    });
    connect(editor, &CodeEditor::hoverCanceled, this, [this]() {
        // Two legs, two trackers: the server's answer and the index's
        // are separate round trips, and both must stop being wanted.
        languageService_->cancelHover();
        hoverCanceled();
    });

    // L5: completion. The editor reports keystrokes and the caret; every
    // decision about them — whether a request is worth making, which
    // candidates match, in what order, and what each one types — is
    // `lsp_core::completion`'s. All that happens here is the position
    // conversion and the FFI-to-view struct copy.
    connect(editor,
            &CodeEditor::completionRequested,
            this,
            [this, editor](int position, const QString &textBefore, bool explicitRequest) {
                const QString path = editor->property("lspPath").toString();
                if (path.isEmpty()) {
                    return;
                }
                const QPair<quint32, quint32> at = lspPosition(editor, position);
                languageService_->completionAt(path, at.first, at.second, textBefore,
                                               explicitRequest);
            });
    connect(editor,
            &CodeEditor::completionFilterChanged,
            this,
            [this, editor](const QString &textBefore) {
                QVector<CompletionEntry> entries;
                for (const FfiCompletionItem &item :
                     languageService_->completionItems(textBefore)) {
                    entries.append(CompletionEntry{
                        item.label,
                        item.kind,
                        item.detail,
                        item.documentation,
                        item.insert,
                        item.has_range,
                        static_cast<int>(item.start_line),
                        static_cast<int>(item.start_character),
                        static_cast<int>(item.end_line),
                        static_cast<int>(item.end_character),
                        static_cast<int>(item.prefix_length),
                        item.resolve_data,
                    });
                }
                editor->showCompletions(entries);
            });
    connect(editor, &CodeEditor::completionCanceled, this, [this]() {
        languageService_->cancelCompletion();
        // C7: a documentation preview for the popup that just closed is no
        // longer wanted either.
        languageService_->cancelCompletionPreview();
    });
    // C7: the popup's selection moved — ask for a resolved preview of the
    // newly highlighted row.
    connect(editor,
            &CodeEditor::completionPreviewRequested,
            this,
            [this](const QString &resolveData) {
                languageService_->resolveCompletionPreview(resolveData);
            });
    // F0-18/C7: accepting an item is a buffer edit like any other. The
    // widget says which item and where the caret is; `lsp_core::completion`
    // works out the span that gets replaced, and — when the server offers
    // `completionItem/resolve` — merges in whatever `additionalTextEdits`
    // resolving it adds (the `using` an unimported type brings with it).
    // The splice arrives on completionEditReady rather than as a return
    // value, since the resolve round trip means it is never immediate.
    connect(editor,
            &CodeEditor::completionChosen,
            this,
            [this, editor](const CompletionEntry &entry) {
                const QPair<quint32, quint32> caret =
                  lspPosition(editor, editor->textCursor().position());
                const FfiCompletionItem item{entry.label,
                                             entry.kind,
                                             entry.detail,
                                             entry.documentation,
                                             entry.insert,
                                             entry.hasRange,
                                             static_cast<quint32>(entry.startLine),
                                             static_cast<quint32>(entry.startCharacter),
                                             static_cast<quint32>(entry.endLine),
                                             static_cast<quint32>(entry.endCharacter),
                                             static_cast<quint32>(entry.prefixLength),
                                             entry.resolveData};
                languageService_->acceptCompletion(item, caret.first, caret.second);
            });

    // F1-15: the multi-caret gestures. The widget reports what happened;
    // every one of these asks `editor_ops` for a transaction and splices
    // what comes back, so nothing about what an edit means is decided here.
    connect(editor, &CodeEditor::multiCaretTyped, this, [this, editor, tabId](const QString &typed) {
        const ::rust::Vec<FfiTextEdit> edits =
          editorOps_->typeText(tabId, editor->toPlainText(), typed);
        applyEditsTo(editor, edits);
        refreshCarets(editor);
    });
    connect(editor, &CodeEditor::multiCaretBackspace, this, [this, editor, tabId]() {
        applyEditsTo(editor, editorOps_->backspace(tabId, editor->toPlainText()));
        refreshCarets(editor);
    });
    connect(editor, &CodeEditor::multiCaretDelete, this, [this, editor, tabId]() {
        applyEditsTo(editor, editorOps_->deleteForward(tabId, editor->toPlainText()));
        refreshCarets(editor);
    });
    connect(editor, &CodeEditor::multiCaretNewline, this, [this, editor, tabId]() {
        applyEditsTo(editor, editorOps_->newline(tabId, editor->toPlainText()));
        refreshCarets(editor);
    });
    connect(editor, &CodeEditor::caretAddRequested, this, [this, editor, tabId](int position) {
        editorOps_->addCaretAt(tabId, editor->toPlainText(), static_cast<quint32>(position));
        refreshCarets(editor);
    });
    connect(editor,
            &CodeEditor::columnSelectRequested,
            this,
            [this, editor, tabId](int anchor, int head) {
                editorOps_->columnSelect(tabId, editor->toPlainText(),
                                         static_cast<quint32>(anchor),
                                         static_cast<quint32>(head));
                refreshCarets(editor);
            });
    connect(editor, &CodeEditor::secondaryCaretsDropped, this, [this, editor, tabId]() {
        editorOps_->clearSecondaryCarets(tabId);
        editor->setSecondaryCarets({});
    });
    connect(editor, &CodeEditor::pasteRequested, this, [this, editor, tabId](const QString &text) {
        const ::rust::Vec<FfiTextEdit> edits =
          editorOps_->pasteText(tabId, editor->toPlainText(), text);
        applyEditsTo(editor, edits);
        refreshCarets(editor);
    });

    // L3: only the visible tab's cursor should move the status bar —
    // guards against a background tab's programmatic cursor change
    // (e.g. a reload) touching labels that describe a different tab.
    // M4: unlike the status bar, every cursor move is forwarded to
    // AppSession regardless of visibility, so get_cursor_position stays
    // accurate for a tab MCP asks about while it's in the background.
    // F2-10: intentions are caret-scoped, so every move invalidates
    // whatever was asked for the caret's previous spot — a stale bulb
    // sitting on a line the caret already left is a bug, not a cosmetic
    // nit — and, once the caret settles, is worth asking about again.
    // 150ms matches §5's debounce policy; not asked at all for a tab that
    // is not the one visible.
    auto *intentionsTimer = new QTimer(editor);
    intentionsTimer->setSingleShot(true);
    intentionsTimer->setInterval(150);
    connect(intentionsTimer, &QTimer::timeout, this, [this, editor]() {
        requestIntentionsFor(editor, false);
    });
    connect(editor, &QPlainTextEdit::cursorPositionChanged, this,
            [this, editor, tabId, intentionsTimer]() {
        const QTextCursor cursor = editor->textCursor();
        // F1-15: keep `editor_ops` told where the one caret is, so Ctrl+D
        // and the line operations act on what the user can see. Skipped
        // while this class is the one moving the caret (it would overwrite
        // a multi-caret set with a single caret) and while the widget is
        // showing extra carets, which Rust owns.
        if (!syncingCarets_ && !editor->hasSecondaryCarets()) {
            ::rust::Vec<FfiCaret> carets;
            carets.push_back(FfiCaret{static_cast<quint32>(cursor.anchor()),
                                      static_cast<quint32>(cursor.position()), true});
            editorOps_->setCarets(tabId, editor->toPlainText(), carets);
        }
        docManager_->setCursorPosition(tabId, static_cast<quint32>(cursor.blockNumber()),
                                        static_cast<quint32>(cursor.columnNumber()));
        if (activeGroup_ && activeGroup_->currentWidget() == editor) {
            updateStatusBar();
            languageService_->cancelIntentions();
            if (intentionBulb_ && intentionsEditor_ == editor) {
                intentionBulb_->hide();
            }
            intentionsTimer->start();
            // F2-11: signature help tracks typing live rather than waiting
            // for the caret to settle — `should_request`/`should_dismiss`
            // decide whether this keystroke means anything to it.
            requestSignatureHelpFor(editor);
        }
    });

    // Forward QPlainTextEdit's own modified state into the session's
    // authoritative dirty flag (ADR-0003) rather than marshalling
    // keystrokes. The stable id is captured by value — unlike a tab
    // index, it never shifts when other tabs close.
    connect(editor->document(),
            &QTextDocument::modificationChanged,
            docManager_,
            [this, tabId](bool modified) { docManager_->setTabModified(tabId, modified); });

    // Task L2: tell the language server about the document, and keep it
    // told. The path rides on the widget (like `tabId`) so a close can
    // still name the document after the session has forgotten the tab.
    const QString path = docManager_->tabPath(tabId);
    editor->setProperty("lspPath", path);
    if (!path.isEmpty()) {
        languageService_->documentOpened(path, editor->toPlainText());
    }
    // didChange is full-text (ADR-0016), so it is debounced rather than
    // sent per keystroke — the delay is a view-side rate limit, not a
    // rule about when a server should re-analyse.
    auto *changeTimer = new QTimer(editor);
    changeTimer->setSingleShot(true);
    changeTimer->setInterval(300);
    connect(changeTimer, &QTimer::timeout, this, [this, editor]() {
        const QString current = editor->property("lspPath").toString();
        if (!current.isEmpty()) {
            languageService_->documentChanged(current, editor->toPlainText());
        }
        // F2-11: a content change shifts every hint position on the lines
        // after it, so the visible set is asked for again on the same
        // debounce rather than a fourth timer.
        requestInlayHintsFor(editor);
        // F3-16: the gutter's markers are stale the moment the buffer
        // changes, exactly like an inlay hint's position is — same
        // debounce, same reasoning.
        requestHunksFor(editor);
        // C9-followup: the server's last answer was for the previous
        // revision, so ask again on the same settle tick.
        requestSemanticTokensFor(editor);
        // C10-followup: same reasoning — a lens's line/label can be stale
        // the moment the buffer changes.
        requestCodeLensesFor(editor);
    });

    // F3-16: a click on a change marker. `window_` (not `editor`) owns the
    // popup so it survives whichever tab is current changing under it.
    connect(editor, &CodeEditor::changeMarkerClicked, this,
            [this, editor](int hunkIndex, const QPoint &globalPos) {
                onChangeMarkerClicked(editor, hunkIndex, globalPos);
            });
    connect(editor->document(), &QTextDocument::contentsChanged, changeTimer,
             qOverload<>(&QTimer::start));

    // C10-followup: a click on a code lens pill runs it — resolving it
    // first if needed, then dispatching its command through the
    // gated-refactor path (`LanguageService::runCodeLens`'s own doc).
    connect(editor, &CodeEditor::codeLensClicked, this, [this, editor](int index) {
        const QString path = editor->property("lspPath").toString();
        if (!path.isEmpty()) {
            languageService_->runCodeLens(path, static_cast<quint32>(index));
        }
    });

    // The Preview dock's own debounce (ADR-0033), separate from the LSP
    // one above: a `didChange` and a re-render answer different questions
    // at different costs, and tying the dock's cadence to the language
    // server's would make one plugin's slow re-render throttle the
    // other's fast one for no reason. Fires only when this editor is the
    // *current* one — the dock has nothing to show for a background tab's
    // edits.
    auto *previewTimer = new QTimer(editor);
    previewTimer->setSingleShot(true);
    previewTimer->setInterval(300);
    connect(previewTimer, &QTimer::timeout, this, [this, editor, tabId]() {
        if (activeGroup_ && activeGroup_->currentWidget() == editor) {
            // Both preview surfaces ride this one timer: the dock through
            // the callback, the in-tab view mode directly. Whichever is not
            // showing this tab ignores what it is handed.
            refreshPreviewMode(editor);
            if (previewChanged_) {
                previewChanged_(tabId);
            }
        }
    });
    connect(editor->document(), &QTextDocument::contentsChanged, previewTimer,
             qOverload<>(&QTimer::start));
    // F2-11: scrolling changes which lines are visible without touching the
    // document or the caret, so it needs its own trigger.
    connect(editor->verticalScrollBar(), &QScrollBar::valueChanged, this,
            [this, editor]() { requestInlayHintsFor(editor); });

    group->addTab(editor, title);
    renderTabText(group, group->indexOf(editor), title, false);
    markTab("tab_added", tabId, group, group->indexOf(editor), title);
    applyDiagnostics();
    requestInlayHintsFor(editor);
    requestHunksFor(editor);
    refreshRunMarker(editor);
    refreshBreakpointsFor(editor);
    watchLineCountFor(editor);
    connect(editor, &CodeEditor::runRequested, this, [this, editor]() { requestRunFor(editor); });
    connect(editor, &CodeEditor::breakpointToggled, this,
            [this, editor](int blockNumber) { toggleBreakpointAt(editor, blockNumber); });
    requestSemanticTokensFor(editor);
    requestCodeLensesFor(editor);
}

} // namespace ui_shell
