#include "editor_tabs.h"

#include "code_editor.h"
#include "diff_view.h"
#include "diff_view_page.h"
#include "e2e_mark.h"
#include "vcs_gutter.h"

#include <QCloseEvent>
#include <QFile>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QKeySequence>
#include <QLabel>
#include <QMessageBox>
#include <QPushButton>
#include <QShortcut>
#include <QTabWidget>
#include <QVBoxLayout>
#include <QVector>

namespace ui_shell {

namespace {

// The floating window `openEditableDiffWindow` shows. `closeEvent` (not
// `destroyed`, and not `WA_DeleteOnClose` alone) is what runs `onClosing`:
// `QObject::destroyed` fires only once `~QWidget` has already torn down its
// children, by which point the real `CodeEditor` this window borrowed would
// already be gone. Deferring the actual delete to `deleteLater()` keeps
// this safe to close from within its own event handling.
class DiffWindow : public QWidget
{
public:
    using QWidget::QWidget;
    std::function<void()> onClosing;

protected:
    void closeEvent(QCloseEvent *event) override
    {
        if (onClosing) {
            onClosing();
        }
        QWidget::closeEvent(event);
        deleteLater();
    }
};

} // namespace

namespace {

// The block a hunk's marker paints on — mirrors applyVcsHunks's own rule for
// a pure deletion (no line of its own on the new side, so it marks the line
// the deletion happened in front of). Shared so rollback-at-caret and
// next/previous-change agree with what the gutter actually shows.
quint32 hunkMarkerLine(const FfiHunk &hunk)
{
    if (hunk.kind == FfiHunkKind::Removed) {
        return hunk.new_start > 0 ? hunk.new_start - 1 : 0;
    }
    return hunk.new_start;
}

} // namespace

void wireVcsService(VcsService *vcsService, ProjectTreeModel *treeModel, EditorTabs *editorTabs)
{
    // Same project-open lifecycle event the tree/watcher and the language
    // servers already join; isRepository()/changedFiles() answer
    // asynchronously once discovery replies (VcsService::openProject).
    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, vcsService,
                      [vcsService](const QString &rootPath) { vcsService->openProject(rootPath); });
    QObject::connect(vcsService, &VcsService::repositoryChanged, vcsService, [vcsService]() {
        if (vcsService->isRepository()) {
            vcsService->refreshStatus();
        }
    });
    editorTabs->setVcsService(vcsService);
    QObject::connect(vcsService, &VcsService::hunksChanged, editorTabs,
                      [editorTabs](const QString &path) { editorTabs->applyVcsHunks(path); });
    QObject::connect(vcsService, &VcsService::blameReady, editorTabs,
                      [editorTabs](const QString &path, const ::rust::Vec<FfiBlameLine> &lines) {
                          editorTabs->applyVcsBlame(path, lines);
                      });
}

void EditorTabs::setVcsService(VcsService *vcsService)
{
    vcsService_ = vcsService;
}

void EditorTabs::requestHunksFor(CodeEditor *editor)
{
    if (!vcsService_) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        // An unsaved buffer has no path and therefore nothing in HEAD to
        // gutter against.
        return;
    }
    // The document's own revision, not a counter bumped per call: a counter
    // that changes on every request can never equal the one that produced
    // the cached answer, so `HunkCache`'s hit branch was unreachable and a
    // tab switch or a settle tick after the buffer stopped changing rediffed
    // the whole file. `QTextDocument::revision()` is the `doc_revision` the
    // plan specified, Qt maintains it, and it makes the key correctly
    // per-file instead of shared across every open tab.
    vcsService_->requestHunks(
      path, editor->toPlainText(), static_cast<qint64>(editor->document()->revision()));
}

void EditorTabs::applyVcsHunks(const QString &path)
{
    if (!vcsService_) {
        return;
    }
    CodeEditor *editor = editorForPath(path);
    if (!editor) {
        return;
    }

    QVector<ChangeMarker> markers;
    const ::rust::Vec<FfiHunk> hunks = vcsService_->hunks(path);
    for (std::size_t i = 0; i < hunks.size(); ++i) {
        const FfiHunk &hunk = hunks[i];
        const int hunkIndex = static_cast<int>(i);
        ChangeMarkerKind kind = hunk.kind == FfiHunkKind::Added   ? ChangeMarkerKind::Added
                                 : hunk.kind == FfiHunkKind::Removed ? ChangeMarkerKind::Removed
                                                                      : ChangeMarkerKind::Modified;
        if (hunk.kind == FfiHunkKind::Removed) {
            // An empty new-side range has no line of its own to sit on;
            // mark the line the deletion happened in front of (or the
            // first line, for a deletion at the very top of the file).
            const int block = hunk.new_start > 0 ? static_cast<int>(hunk.new_start) - 1 : 0;
            markers.append(ChangeMarker{block, kind, hunkIndex});
            continue;
        }
        for (quint32 line = hunk.new_start; line < hunk.new_start + hunk.new_len; ++line) {
            markers.append(ChangeMarker{static_cast<int>(line), kind, hunkIndex});
        }
    }
    editor->setChangeMarkers(markers);

    // The only way anything outside the process can know the gutter has
    // caught up with a buffer edit — an E2E flow that reverts a hunk right
    // after typing one needs this, or it races the 300ms didChange debounce
    // (editor_tabs_lsp.cpp) that got it here.
    e2eMark(QStringLiteral("{\"ev\":\"vcs_hunks_applied\",\"path\":%1,\"count\":%2}")
              .arg(e2eJson(path))
              .arg(markers.size()));
}

void EditorTabs::setAnnotateEnabled(bool enabled)
{
    annotateEnabled_ = enabled;
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    editor->setBlameEnabled(enabled);
    if (!enabled || !vcsService_) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (!path.isEmpty()) {
        vcsService_->blame(path);
    }
}

void EditorTabs::applyVcsBlame(const QString &path, const ::rust::Vec<FfiBlameLine> &lines)
{
    CodeEditor *editor = editorForPath(path);
    if (!editor) {
        return;
    }
    QVector<BlameAnnotation> annotations;
    annotations.reserve(static_cast<int>(lines.size()));
    for (const FfiBlameLine &line : lines) {
        const QString shortId = QString(line.commit).left(8);
        annotations.append(BlameAnnotation{
          static_cast<int>(line.line) - 1,
          QStringLiteral("%1 %2 %3").arg(shortId, QString(line.author_name), QString(line.summary))});
    }
    editor->setBlameAnnotations(annotations);
    editor->setBlameEnabled(annotateEnabled_);
}

void EditorTabs::showDiffAgainstHead()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor || !vcsService_) {
        return;
    }
    const QString path = currentPath();
    if (path.isEmpty()) {
        return;
    }
    openEditableDiffWindow(currentTabId(), editor, path);
}

void EditorTabs::showDiffForPath(const QString &path)
{
    if (!vcsService_) {
        return;
    }
    openFile(path);
    CodeEditor *editor = editorForPath(path);
    if (!editor) {
        // A binary file, or something `openFile` couldn't open at all —
        // either way, no editable diff to show.
        return;
    }
    openEditableDiffWindow(editor->property("tabId").toULongLong(), editor, path);
}

void EditorTabs::openEditableDiffWindow(quint64 tabId, CodeEditor *editor, const QString &path)
{
    if (const auto it = diffWindows_.constFind(tabId); it != diffWindows_.constEnd()) {
        it->window->show();
        it->window->raise();
        it->window->activateWindow();
        return;
    }
    if (!vcsService_) {
        return;
    }
    const TabLoc loc = locate(tabId);
    if (!loc.group) {
        return;
    }
    const QString title = loc.group->tabText(loc.index);
    const QString headText = vcsService_->headText(path);

    loc.group->removeTab(loc.index);

    // A placeholder, not a blank page: the tab still exists (it can be
    // renamed by a file rename, closed, dragged into a split) while its
    // editor is off in the diff window, and a blank page reads as a bug
    // rather than "look elsewhere".
    auto *placeholder = new QWidget(loc.group);
    placeholder->setProperty("tabId", QVariant::fromValue(tabId));
    auto *placeholderLayout = new QVBoxLayout(placeholder);
    placeholderLayout->addStretch(1);
    auto *label = new QLabel(tr("This file's diff is open in a separate window."), placeholder);
    label->setAlignment(Qt::AlignCenter);
    placeholderLayout->addWidget(label);
    auto *showButton = new QPushButton(tr("Show Diff Window"), placeholder);
    connect(showButton, &QPushButton::clicked, this, [this, tabId] {
        if (const auto it = diffWindows_.constFind(tabId); it != diffWindows_.constEnd()) {
            it->window->show();
            it->window->raise();
            it->window->activateWindow();
        }
    });
    auto *buttonRow = new QHBoxLayout;
    buttonRow->addStretch(1);
    buttonRow->addWidget(showButton);
    buttonRow->addStretch(1);
    placeholderLayout->addLayout(buttonRow);
    placeholderLayout->addStretch(1);
    loc.group->insertTab(loc.index, placeholder, title);
    loc.group->setCurrentIndex(loc.index);

    auto *diffView =
      new DiffView(headText, editor, vcsService_->hunks(path), ::rust::Vec<FfiInlineSpan>(), path);
    auto *page = new DiffViewPage(diffView, tr("HEAD"), tr("Working Tree"));
    page->onIgnoreWhitespaceToggled = [this, diffView, headText, editor](bool ignore) {
        const QString workingText = editor->toPlainText();
        diffView->setHunks(docManager_->diffHunksBetween(headText, workingText, ignore),
                             docManager_->diffSpansBetween(headText, workingText, ignore));
    };

    auto *window = new DiffWindow(nullptr, Qt::Window);
    window->setWindowTitle(tr("Diff — %1").arg(path));
    auto *layout = new QVBoxLayout(window);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(page);
    window->resize(1100, 750);
    window->onClosing = [this, tabId] { restoreEditorFromDiffWindow(tabId); };

    auto *saveShortcut = new QShortcut(QKeySequence::Save, window);
    connect(saveShortcut, &QShortcut::activated, this,
            [this, tabId, editor] { saveEditor(tabId, editor, editor); });

    diffWindows_.insert(tabId, DiffWindowState{window, placeholder});
    window->show();
}

void EditorTabs::restoreEditorFromDiffWindow(quint64 tabId)
{
    const auto it = diffWindows_.find(tabId);
    if (it == diffWindows_.end()) {
        return;
    }
    DiffWindowState state = it.value();
    diffWindows_.erase(it);

    auto *page = state.window->findChild<DiffViewPage *>();
    QPlainTextEdit *editor = page ? page->diffView()->releaseRightPane() : nullptr;
    if (!editor) {
        return;
    }

    const TabLoc loc = locate(tabId);
    if (!loc.group) {
        // The tab is gone by some other path than `onTabClosed` (which
        // handles its own teardown without going through this method at
        // all) — nothing to put the editor back into, so it goes with it.
        delete editor;
        return;
    }
    const QString title = loc.group->tabText(loc.index);
    loc.group->removeTab(loc.index);
    delete state.placeholder;
    loc.group->insertTab(loc.index, editor, title);
    loc.group->setCurrentIndex(loc.index);
    editor->setFocus();
}

void EditorTabs::openCompareFiles(const QString &leftPath, const QString &rightPath)
{
    QFile left(leftPath);
    QFile right(rightPath);
    if (!left.open(QIODevice::ReadOnly) || !right.open(QIODevice::ReadOnly)) {
        QMessageBox::critical(window_, tr("Cannot compare files"),
                                tr("One of the selected files could not be read."));
        return;
    }
    const QString leftText = QString::fromUtf8(left.readAll());
    const QString rightText = QString::fromUtf8(right.readAll());
    const quint64 tabId =
      docManager_->openDiffTab(leftPath, QFileInfo(leftPath).fileName(),
                                 QFileInfo(rightPath).fileName(), leftText, rightText);
    focusTab(tabId);
}

void EditorTabs::openCompareRevisions(const QString &path,
                                       const QString &leftRevision,
                                       const QString &leftLabel,
                                       const QString &rightRevision,
                                       const QString &rightLabel)
{
    if (!vcsService_) {
        return;
    }
    // An empty revision means "the live working text" — the open buffer if
    // there is one (so an unsaved edit is what gets compared, matching
    // what the user actually sees), the file on disk otherwise.
    auto textAt = [this, &path](const QString &revision) -> QString {
        if (!revision.isEmpty()) {
            return vcsService_->blobAt(path, revision);
        }
        if (CodeEditor *editor = editorForPath(path)) {
            return editor->toPlainText();
        }
        QFile file(path);
        return file.open(QIODevice::ReadOnly) ? QString::fromUtf8(file.readAll()) : QString();
    };
    // `blobAt` answers from a worker-thread cache filled by `requestBlobAt`
    // — ask for both sides, wait for `blobReady` to say the cache has them,
    // then build the tab. A revision the caller already resolved from a
    // real log entry, so no "still loading" state is needed beyond this.
    auto build = [this, path, leftRevision, leftLabel, rightRevision, rightLabel, textAt]() {
        const QString leftText = textAt(leftRevision);
        const QString rightText = textAt(rightRevision);
        const quint64 tabId =
          docManager_->openDiffTab(path, leftLabel, rightLabel, leftText, rightText);
        focusTab(tabId);
    };
    const bool leftNeedsFetch = !leftRevision.isEmpty();
    const bool rightNeedsFetch = !rightRevision.isEmpty();
    if (!leftNeedsFetch && !rightNeedsFetch) {
        build();
        return;
    }
    auto pending = std::make_shared<int>((leftNeedsFetch ? 1 : 0) + (rightNeedsFetch ? 1 : 0));
    auto connection = std::make_shared<QMetaObject::Connection>();
    *connection = connect(
      vcsService_, &VcsService::blobReady, this,
      [this, connection, pending, build, path, leftRevision,
       rightRevision](const QString &readyPath, const QString &readyRevision) {
          // Filtered by the exact (path, revision) pair this call asked
          // for — `blobReady` is process-wide, and an unrelated "compare
          // revisions" started while this one is still fetching must not
          // be counted against it.
          if (readyPath != path
              || (readyRevision != leftRevision && readyRevision != rightRevision)) {
              return;
          }
          if (--(*pending) <= 0) {
              QObject::disconnect(*connection);
              build();
          }
      });
    if (leftNeedsFetch) {
        vcsService_->requestBlobAt(path, leftRevision);
    }
    if (rightNeedsFetch) {
        vcsService_->requestBlobAt(path, rightRevision);
    }
}

void EditorTabs::rollbackHunkAtCaret()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor || !vcsService_) {
        return;
    }
    const QString path = currentPath();
    if (path.isEmpty()) {
        return;
    }
    const int caretLine = editor->textCursor().blockNumber();
    const ::rust::Vec<FfiHunk> hunks = vcsService_->hunks(path);
    for (std::size_t i = 0; i < hunks.size(); ++i) {
        const FfiHunk &hunk = hunks[i];
        const quint32 start = hunkMarkerLine(hunk);
        const quint32 end =
          hunk.kind == FfiHunkKind::Removed ? start + 1 : hunk.new_start + hunk.new_len;
        if (static_cast<quint32>(caretLine) >= start && static_cast<quint32>(caretLine) < end) {
            const ::rust::Vec<FfiTextEdit> edits =
              vcsService_->revertHunk(path, static_cast<quint32>(i));
            if (!edits.empty()) {
                applyEditsTo(editor, edits);
                // Proof the revert went through the buffer's own undo stack
                // (F3-11's whole design point) rather than the file on
                // disk — nothing else marks the moment `vcs.rollbackHunk`
                // actually found and spliced a hunk.
                e2eMark(QStringLiteral("{\"ev\":\"vcs_hunk_reverted\",\"path\":%1,"
                                        "\"hunk_index\":%2}")
                          .arg(e2eJson(path))
                          .arg(i));
            }
            return;
        }
    }
}

void EditorTabs::jumpToChange(bool forward)
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor || !vcsService_) {
        return;
    }
    const QString path = currentPath();
    if (path.isEmpty()) {
        return;
    }
    const ::rust::Vec<FfiHunk> hunks = vcsService_->hunks(path);
    if (hunks.empty()) {
        return;
    }
    const int caretLine = editor->textCursor().blockNumber();
    int target = -1;
    if (forward) {
        for (std::size_t i = 0; i < hunks.size(); ++i) {
            const int line = static_cast<int>(hunkMarkerLine(hunks[i]));
            if (line > caretLine) {
                target = line;
                break;
            }
        }
        if (target < 0) {
            target = static_cast<int>(hunkMarkerLine(hunks[0]));
        }
    } else {
        for (std::size_t i = hunks.size(); i-- > 0;) {
            const int line = static_cast<int>(hunkMarkerLine(hunks[i]));
            if (line < caretLine) {
                target = line;
                break;
            }
        }
        if (target < 0) {
            target = static_cast<int>(hunkMarkerLine(hunks[hunks.size() - 1]));
        }
    }

    QTextCursor cursor = editor->textCursor();
    cursor.movePosition(QTextCursor::Start);
    cursor.movePosition(QTextCursor::Down, QTextCursor::MoveAnchor, target);
    editor->setTextCursor(cursor);
    editor->centerCursor();
}

void EditorTabs::onChangeMarkerClicked(CodeEditor *editor, int hunkIndex, const QPoint &globalPos)
{
    if (!vcsService_) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString path = docManager_->tabPath(tabId);
    if (path.isEmpty()) {
        return;
    }

    HunkPopupActions actions;
    actions.revert = [this, editor, path, hunkIndex]() {
        const ::rust::Vec<FfiTextEdit> edits = vcsService_->revertHunk(path, hunkIndex);
        if (!edits.empty()) {
            applyEditsTo(editor, edits);
        }
    };
    actions.stage = [this, path]() {
        // Whole-file staging: precise per-hunk staging needs the hunk
        // between the index and the worktree, and this gutter only ever
        // has the hunk between HEAD and the worktree (see
        // VcsService::stageHunk's own doc comment). Correct per-hunk
        // staging belongs to F3-17's Changes dock.
        vcsService_->stageFile(path);
    };
    actions.showDiff = [this, editor, tabId, path]() { openEditableDiffWindow(tabId, editor, path); };

    showHunkPopup(window_, globalPos, actions);
}

} // namespace ui_shell
