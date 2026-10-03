#include "editor_tabs.h"

#include "code_editor.h"
#include "diff_view.h"
#include "diff_view_page.h"

#include <QApplication>
#include <QFileDialog>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QTabWidget>

// Saving: Ctrl+S, Save As, the save a close or a quit asks for, and the
// write itself — a leg of EditorTabs in its own translation unit, like the
// pane tree, the language-server leg and the VCS/run/debug ones.

namespace ui_shell {

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

void EditorTabs::saveCurrentTab()
{
    // Ctrl+S with focus inside an open diff tab saves *that* diff's editor,
    // not whatever `activeGroup_` says is current — the file.save QAction
    // (main_window.cpp) is the only Ctrl+S left registered on this window
    // since the Diff dock stopped being a separate top-level window
    // (F3-14): a second `QShortcut` scoped to the diff page would only
    // have raced this one for the same key sequence, which Qt resolves by
    // firing neither.
    QWidget *focused = QApplication::focusWidget();
    for (auto it = diffPages_.constBegin(); focused && it != diffPages_.constEnd(); ++it) {
        DiffViewPage *page = it.value();
        if (page && (page == focused || page->isAncestorOf(focused))) {
            auto *editor = qobject_cast<CodeEditor *>(page->diffView()->rightPane());
            if (editor) {
                beginSave(it.key(), editor, editor);
            }
            return;
        }
    }
    if (!activeGroup_) {
        return;
    }
    const int index = activeGroup_->currentIndex();
    auto *editor = qobject_cast<QPlainTextEdit *>(activeGroup_->widget(index));
    if (editor) {
        beginSave(tabIdAt(activeGroup_, index), qobject_cast<CodeEditor *>(editor), editor);
    }
}

void EditorTabs::beginSave(quint64 tabId, CodeEditor *codeEditor, QPlainTextEdit *editor)
{
    if (!codeEditor || editor->isReadOnly()) {
        saveEditor(tabId, codeEditor, editor);
        return;
    }
    // A formatter can take seconds (a container `run`), so it runs on a
    // worker and the file is written when `saveFormatted` comes back.
    const FfiSaveEdits start = editorOps_->beginSave(
      tabId, static_cast<qint64>(editor->document()->revision()), editor->toPlainText());
    if (start.pending) {
        showStatusNotice(tr("Formatting before saving..."));
        return;
    }
    writeSave(tabId, codeEditor, editor, start);
}

void EditorTabs::onSaveFormatted(quint64 tabId)
{
    QPlainTextEdit *editor = nullptr;
    if (DiffViewPage *page = diffPages_.value(tabId)) {
        editor = qobject_cast<QPlainTextEdit *>(page->diffView()->rightPane());
    }
    if (!editor) {
        editor = editorForTab(tabId);
    }
    if (!editor) {
        return;
    }
    const FfiSaveEdits done = editorOps_->finishSave(
      tabId, static_cast<qint64>(editor->document()->revision()), editor->toPlainText());
    if (!done.pending) {
        writeSave(tabId, qobject_cast<CodeEditor *>(editor), editor, done);
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
    // A save that cannot wait (closing, quitting, Save All): the tidy rules
    // only, never the formatter. Real editors only (a hex tab has no
    // language and nothing to tidy).
    FfiSaveEdits tidy{};
    if (codeEditor) {
        tidy = editorOps_->saveRuleEdits(tabId, editor->toPlainText());
    }
    return writeSave(tabId, codeEditor, editor, tidy);
}

bool EditorTabs::writeSave(quint64 tabId, CodeEditor *codeEditor, QPlainTextEdit *editor,
                           const FfiSaveEdits &tidy)
{
    // F1-11: the formatter's text, trim, final newline and line-ending
    // normalisation, applied *before* the file is read for writing — one
    // undo entry apart from the user's last edit, the caret where the
    // splice's own cursor adjustment puts it rather than column 0.
    if (!tidy.edits.empty()) {
        applyEditsTo(editor, tidy.edits);
    }
    switch (tidy.skipped) {
    case FfiFormatSkipped::Failed:
        showStatusNotice(tr("Saved without formatting: %1 failed: %2")
                           .arg(QString(tidy.failed_tool), QString(tidy.failure)));
        break;
    case FfiFormatSkipped::BufferChanged:
        showStatusNotice(tr("Saved without formatting: the file changed while it was being formatted"));
        break;
    case FfiFormatSkipped::NotWaited:
        showStatusNotice(tr("Saved without formatting"));
        break;
    case FfiFormatSkipped::None:
        break;
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
        if (analysisService_) {
            analysisService_->fileSaved(path, editor->toPlainText());
        }
        if (documentSavedCallback_) {
            documentSavedCallback_(path);
        }
    }
    if (vcsService_ && vcsService_->isRepository()) {
        // A save is exactly what `changedFiles()` (the Changes dock, the
        // status bar's branch widget) is supposed to answer about. The
        // filesystem watcher now reports this write too (`wireVcsService`'s
        // relay), so this call is usually a duplicate — kept anyway,
        // because `VcsService` drops a request that duplicates one already
        // queued, and because it does not depend on the watcher having
        // started. The gutter's own hunks already track the live buffer via
        // `requestHunksFor`'s didChange debounce — this is the same
        // freshness rule for the whole-repo status, which only ever changes
        // relative to disk.
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

} // namespace ui_shell
