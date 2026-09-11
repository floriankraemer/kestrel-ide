#include "editor_tabs.h"

#include "code_editor.h"
#include "diff_panel.h"
#include "diff_view.h"
#include "diff_view_page.h"
#include "e2e_mark.h"
#include "vcs_gutter.h"

#include <QFile>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QLabel>
#include <QMessageBox>
#include <QPushButton>
#include <QTabWidget>
#include <QTextDocument>
#include <QTimer>
#include <QVBoxLayout>
#include <QVector>

namespace ui_shell {

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

    // Until this relay existed, nothing outside the app could move the
    // Changes dock: a `git pull`, a rebase, a branch switch or an edit made
    // in a terminal changed the worktree and the dock went on showing what
    // it last read. The tree's watcher already reports every one of those
    // events on the Qt thread, and the search index already coalesces the
    // same signal the same way (`main_window.cpp`'s reindex timer) — one
    // save produces several events, and a checkout produces thousands, so
    // the window matters more here than the individual paths do. The paths
    // are not collected at all: `refreshStatus` reads the whole worktree
    // regardless of which file rang the bell, and `VcsService` drops a
    // request that duplicates one already queued.
    auto *statusTimer = new QTimer(vcsService);
    statusTimer->setSingleShot(true);
    statusTimer->setInterval(300);
    QObject::connect(statusTimer, &QTimer::timeout, vcsService, [vcsService]() {
        if (vcsService->isRepository()) {
            vcsService->refreshStatus();
        }
    });
    QObject::connect(treeModel, &ProjectTreeModel::filesChangedExternally, statusTimer,
                      [statusTimer](const QString &) { statusTimer->start(); });
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

void EditorTabs::setDiffPanel(DiffPanel *diffPanel, std::function<void()> revealDiffDock)
{
    diffPanel_ = diffPanel;
    revealDiffDock_ = std::move(revealDiffDock);
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
    if (diffPanel_ && diffPanel_->raiseIfOpen(tabId)) {
        if (revealDiffDock_) {
            revealDiffDock_();
        }
        return;
    }
    if (!vcsService_ || !diffPanel_) {
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
    // editor is showing in the Diff dock instead, and a blank page reads as
    // a bug rather than "look elsewhere".
    auto *placeholder = new QWidget(loc.group);
    placeholder->setProperty("tabId", QVariant::fromValue(tabId));
    auto *placeholderLayout = new QVBoxLayout(placeholder);
    placeholderLayout->addStretch(1);
    auto *label = new QLabel(tr("This file's diff is open in the Diff dock."), placeholder);
    label->setAlignment(Qt::AlignCenter);
    placeholderLayout->addWidget(label);
    auto *showButton = new QPushButton(tr("Show Diff"), placeholder);
    connect(showButton, &QPushButton::clicked, this, [this, tabId] {
        if (diffPanel_ && diffPanel_->raiseIfOpen(tabId) && revealDiffDock_) {
            revealDiffDock_();
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
    diffPlaceholders_.insert(tabId, placeholder);

    auto *diffView =
      new DiffView(headText, editor, vcsService_->hunks(path), ::rust::Vec<FfiInlineSpan>(), path);
    // The right text is the live buffer, read at every recompute — a
    // keystroke, a chevron, a toolbar option all see what the editor holds
    // now, not what it held when the window opened.
    auto recompute = [this, headText, editor](FfiWhitespaceMode whitespace,
                                              FfiHighlightMode highlight) {
        DiffData data;
        data.leftText = headText;
        data.rightText = editor->toPlainText();
        data.hunks = docManager_->diffHunksBetween(data.leftText, data.rightText, whitespace);
        data.spans =
          docManager_->diffSpansBetween(data.leftText, data.rightText, whitespace, highlight);
        data.rows = docManager_->diffRowsBetween(data.leftText, data.rightText, whitespace);
        return data;
    };
    auto *page = new DiffViewPage(diffView, path, tr("HEAD"), tr("Working Tree"), recompute);
    page->setApplyHandler([this, page, headText, editor, path](const FfiHunk &hunk) {
        FfiTextEdit edit = docManager_->hunkRevertEdit(headText, hunk);
        edit.path = path;
        ::rust::Vec<FfiTextEdit> edits;
        edits.push_back(edit);
        applyEditsTo(editor, edits);
        e2eMark(QStringLiteral("{\"ev\":\"diff_hunk_applied\",\"path\":%1,\"new_start\":%2}")
                  .arg(e2eJson(path))
                  .arg(hunk.new_start));
        page->refresh();
    });
    // Typing in the right pane changes the diff; recompute once the burst
    // settles rather than on every keystroke. The timer is the page's
    // child, so the window closing drops the connection with it.
    auto *refreshTimer = new QTimer(page);
    refreshTimer->setSingleShot(true);
    refreshTimer->setInterval(250);
    connect(editor->document(), &QTextDocument::contentsChanged, refreshTimer,
            qOverload<>(&QTimer::start));
    connect(refreshTimer, &QTimer::timeout, page, &DiffViewPage::refresh);

    diffPages_.insert(tabId, page);

    // Revealed before the page is added, not after: `DiffToolbar`'s
    // `showEvent` marks its own on-screen rect via `mapToGlobal` for the
    // E2E harness, and a page added to a still-hidden (or not yet current)
    // dock gets that `showEvent` while its ancestor chain has no real
    // screen position yet — the rect it reports then is wrong, though
    // nothing else about the dock's own rendering seemed to notice.
    if (revealDiffDock_) {
        revealDiffDock_();
    }
    diffPanel_->openDiff(tabId, tr("Diff — %1").arg(path), page);
}

void EditorTabs::restoreEditorFromDiffWindow(quint64 tabId, QWidget *page)
{
    const auto it = diffPlaceholders_.find(tabId);
    if (it == diffPlaceholders_.end()) {
        return;
    }
    QWidget *placeholder = it.value();
    diffPlaceholders_.erase(it);
    diffPages_.remove(tabId);

    auto *diffPage = qobject_cast<DiffViewPage *>(page);
    QPlainTextEdit *editor = diffPage ? diffPage->diffView()->releaseRightPane() : nullptr;
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
    delete placeholder;
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
    // `path` by value, not by reference: `build` below outlives this call —
    // it runs from a `blobReady` slot, long after the caller's own `path`
    // (a local in `FileHistoryPanel::showContextMenu`) is gone. Capturing
    // the reference left every lookup reading freed memory, so both panes
    // came out empty when it survived and the flow hung when it did not
    // (#210).
    auto textAt = [this, path](const QString &revision) -> QString {
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
