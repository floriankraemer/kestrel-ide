// Live templates in the editor (ADR-0072): the pickers behind Ctrl+J and
// Ctrl+Alt+T, and the splice that follows any expansion. Which template
// fits and what it expands to is decided in Rust (`EditorOps`).

#include "editor_tabs.h"
#include "code_editor.h"

#include <QAction>
#include <QMenu>
#include <QTextCursor>
#include <QVariant>

namespace ui_shell {

bool EditorTabs::applyTemplateExpansion(CodeEditor *editor, const FfiTemplateExpansion &expansion)
{
    if (!expansion.applied) {
        return false;
    }
    applyEditsTo(editor, expansion.edits);
    QTextCursor cursor = editor->textCursor();
    cursor.setPosition(static_cast<int>(expansion.stop.start));
    cursor.setPosition(static_cast<int>(expansion.stop.end), QTextCursor::KeepAnchor);
    editor->setTextCursor(cursor);
    editor->setSnippetActive(expansion.stop.more);
    return true;
}

QString EditorTabs::pickTemplate(CodeEditor *editor, const ::rust::Vec<FfiTemplateItem> &items)
{
    if (items.empty()) {
        return {};
    }
    QMenu menu(window_);
    menu.setObjectName(QStringLiteral("liveTemplatesMenu"));
    for (const FfiTemplateItem &item : items) {
        QAction *entry = menu.addAction(
          tr("%1 — %2").arg(QString(item.abbreviation), QString(item.description)));
        entry->setData(QString(item.abbreviation));
    }
    const QAction *chosen = menu.exec(editor->mapToGlobal(editor->cursorRect().bottomLeft()));
    return chosen != nullptr ? chosen->data().toString() : QString();
}

void EditorTabs::insertLiveTemplateNow()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString text = editor->toPlainText();
    const QString abbreviation = pickTemplate(editor, editorOps_->insertableTemplates(tabId, text));
    if (!abbreviation.isEmpty()) {
        applyTemplateExpansion(editor, editorOps_->insertTemplate(tabId, text, abbreviation));
    }
}

void EditorTabs::surroundWithTemplateNow()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString text = editor->toPlainText();
    const QString abbreviation = pickTemplate(editor, editorOps_->surroundTemplates(tabId));
    if (!abbreviation.isEmpty()) {
        applyTemplateExpansion(editor, editorOps_->surroundWith(tabId, text, abbreviation));
    }
}

} // namespace ui_shell
