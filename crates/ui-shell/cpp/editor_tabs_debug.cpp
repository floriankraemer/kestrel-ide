// D2-5/D3: the editor's half of debugging — the breakpoint column and the
// execution point.
//
// Its own translation unit for the same reason `editor_tabs_run.cpp` is one.
//
// Humble view throughout: whether toggling a line adds or removes a
// breakpoint is `DebugService::toggleBreakpoint`'s answer, and where
// execution stopped is what `debugStopped` reported. Nothing here decides
// either.

#include "editor_tabs.h"

#include "breakpoint_dialog.h"
#include "code_editor.h"
#include "e2e_mark.h"

#include <QAction>
#include <QMenu>
#include <QPlainTextEdit>
#include <QTextBlock>
#include <QTextDocument>
#include <QSet>

namespace ui_shell {

namespace {
// The lines `DebugService` reports for a file, as the widget wants them:
// 0-based block numbers, because `QTextDocument` counts blocks from zero
// while everything a user sees counts lines from one.
QSet<int> blocksFromLines(const QString &newlineSeparated)
{
    QSet<int> blocks;
    for (const QString &line : newlineSeparated.split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        bool ok = false;
        const int number = line.toInt(&ok);
        if (ok && number > 0) {
            blocks.insert(number - 1);
        }
    }
    return blocks;
}
} // namespace

void wireDebugService(DebugService *debugService, EditorTabs *editorTabs)
{
    editorTabs->setDebugService(debugService);
    QObject::connect(debugService, &DebugService::breakpointsChanged, editorTabs,
                      [editorTabs]() { editorTabs->refreshBreakpoints(); });
    QObject::connect(debugService, &DebugService::debugStopped, editorTabs,
                      [editorTabs](quint64, const QString &, const QString &path, quint32 line) {
                          editorTabs->showExecutionPoint(path, static_cast<int>(line));
                          editorTabs->refreshInlineValues();
                      });
    // The scopes arrive after the stop, one fetch per level, so the values
    // are painted again as they land rather than once when nothing is
    // known yet (D3-7).
    QObject::connect(debugService, &DebugService::variablesChanged, editorTabs,
                      [editorTabs](quint64, qint64) { editorTabs->refreshInlineValues(); });
    QObject::connect(debugService, &DebugService::debugResumed, editorTabs,
                      [editorTabs](quint64) {
                          editorTabs->showExecutionPoint(QString(), 0);
                          editorTabs->refreshInlineValues();
                      });
    QObject::connect(debugService, &DebugService::debugTerminated, editorTabs,
                      [editorTabs](quint64, int) {
                          editorTabs->showExecutionPoint(QString(), 0);
                          editorTabs->refreshInlineValues();
                      });
}

void EditorTabs::setDebugService(DebugService *debugService)
{
    debugService_ = debugService;
}

void EditorTabs::refreshBreakpointsFor(CodeEditor *editor)
{
    if (!debugService_ || !editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        return;
    }
    const QSet<int> lines = blocksFromLines(debugService_->breakpointLines(path));
    editor->setBreakpointLines(lines);
    // The only way anything outside the process can know the gutter has
    // caught up with a toggle — an E2E flow that starts a session right
    // after setting a breakpoint would otherwise race it (D3-9).
    e2eMark(QStringLiteral("{\"ev\":\"breakpoints_applied\",\"path\":%1,\"count\":%2}")
              .arg(e2eJson(path))
              .arg(lines.size()));
}

void EditorTabs::refreshBreakpoints()
{
    forEachEditor([this](QPlainTextEdit *editor) {
        refreshBreakpointsFor(qobject_cast<CodeEditor *>(editor));
    });
}

void EditorTabs::toggleBreakpointAt(CodeEditor *editor, int blockNumber)
{
    if (!debugService_ || !editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        return;
    }
    // 1-based on the wire: `dap-core` counts lines the way DAP and the user
    // do, and the conversion belongs at this edge rather than in the store.
    debugService_->toggleBreakpoint(path, static_cast<quint32>(blockNumber + 1));
}

void EditorTabs::setTemporaryBreakpointAt(CodeEditor *editor, int blockNumber)
{
    if (!debugService_ || !editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        return;
    }
    // A fresh, unconditional, enabled, temporary breakpoint — replacing
    // whatever configuration was already on that line, which is what
    // Alt+click means in IntelliJ too.
    FfiBreakpoint breakpoint{};
    breakpoint.path = path;
    breakpoint.line = static_cast<quint32>(blockNumber + 1);
    breakpoint.enabled = true;
    breakpoint.temporary = true;
    debugService_->configureBreakpoint(breakpoint);
}

void EditorTabs::showBreakpointContextMenu(CodeEditor *editor, int blockNumber,
                                            const QPoint &globalPos)
{
    if (!debugService_ || !editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        return;
    }
    const quint32 line = static_cast<quint32>(blockNumber + 1);
    const bool hasBreakpoint =
      blocksFromLines(debugService_->breakpointLines(path)).contains(blockNumber);

    QMenu menu(editor);
    QAction *edit = menu.addAction(hasBreakpoint ? tr("Edit Breakpoint...")
                                                  : tr("Add Breakpoint"));
    QAction *remove = hasBreakpoint ? menu.addAction(tr("Remove Breakpoint")) : nullptr;
    menu.addSeparator();
    const quint64 sessionId = debugService_->currentSessionId();
    QAction *runToCursor = menu.addAction(tr("Run to Cursor"));
    runToCursor->setEnabled(sessionId != 0);

    QAction *chosen = menu.exec(globalPos);
    if (chosen == edit) {
        if (!hasBreakpoint) {
            debugService_->toggleBreakpoint(path, line);
        }
        showBreakpointDialog(editor, debugService_, path, line);
    } else if (chosen == remove) {
        debugService_->toggleBreakpoint(path, line);
    } else if (chosen == runToCursor && sessionId != 0) {
        debugService_->runToCursor(sessionId, path, line);
    }
}

void EditorTabs::watchLineCountFor(CodeEditor *editor)
{
    if (!editor) {
        return;
    }
    // D2-3: breakpoints follow edits. The seam is the document's own
    // `contentsChange`, which the editor already emits — the debugger gets
    // no hook of its own (ADR-0041).
    //
    // Block count before and after is enough: what a breakpoint needs is how
    // many lines moved and from where, not what the text was.
    auto *previous = new int(editor->document()->blockCount());
    connect(editor->document(), &QTextDocument::contentsChange, editor,
            [this, editor, previous](int position, int, int) {
                const int now = editor->document()->blockCount();
                const int delta = now - *previous;
                *previous = now;
                if (delta == 0 || !debugService_) {
                    return;
                }
                const quint64 tabId = editor->property("tabId").toULongLong();
                const QString path = docManager_->tabPath(tabId);
                if (path.isEmpty()) {
                    return;
                }
                const int block = editor->document()->findBlock(position).blockNumber();
                debugService_->shiftBreakpoints(path, static_cast<quint32>(block + 1), delta);
            });
    connect(editor, &QObject::destroyed, editor, [previous]() { delete previous; });
}

void EditorTabs::refreshInlineValues()
{
    if (!debugService_) {
        return;
    }
    forEachEditor([this](QPlainTextEdit *editor) {
        auto *codeEditor = qobject_cast<CodeEditor *>(editor);
        if (!codeEditor) {
            return;
        }
        const quint64 tabId = codeEditor->property("tabId").toULongLong();
        const QString path = docManager_->tabPath(tabId);
        QVector<InlineValueSpan> spans;
        if (!path.isEmpty()) {
            // Whether this file is the one execution stopped in is
            // `DebugService`'s answer: it returns nothing for every other
            // file, so this loop asks the same question of each editor and
            // decides nothing itself.
            for (const FfiInlineValue &value :
                 debugService_->inlineValues(path, codeEditor->toPlainText())) {
                spans.append(InlineValueSpan{static_cast<int>(value.line) - 1, value.text});
            }
        }
        codeEditor->setInlineValues(spans);
    });
}

void EditorTabs::showExecutionPoint(const QString &path, int line)
{
    forEachEditor([this, &path, line](QPlainTextEdit *editor) {
        auto *codeEditor = qobject_cast<CodeEditor *>(editor);
        if (!codeEditor) {
            return;
        }
        const quint64 tabId = codeEditor->property("tabId").toULongLong();
        const QString editorPath = docManager_->tabPath(tabId);
        const bool isTheOne = !path.isEmpty() && editorPath == path && line > 0;
        codeEditor->setExecutionLine(isTheOne ? line - 1 : -1);
    });

    if (!path.isEmpty() && line > 0) {
        // Bring the suspended line into view, which is the whole point of
        // stopping there. `openAt` is the same jump a diagnostic or a search
        // hit uses, so a file that is not open yet opens.
        openFileAtLine(path, line, 1);
    }
}

} // namespace ui_shell
