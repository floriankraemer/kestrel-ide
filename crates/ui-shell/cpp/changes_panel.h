#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QPoint>
#include <QString>
#include <QWidget>
#include <functional>

class QLabel;
class QPlainTextEdit;
class QPushButton;
class QShowEvent;
class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {

class ChangesToolbar;

// The Changes dock (F3-17, rebuilt G6-G8 to read like VS/JetBrains): a
// toolbar (branch chip, Refresh/Fetch/Pull/Push, Stage all/Unstage all),
// Merge Conflicts / Staged / Unstaged / Untracked trees with per-file
// checkboxes and a row context menu, a commit message box, and Commit /
// Commit and Push / Amend.
//
// Humble view per CLAUDE.md: what is staged, what changed, a file's rename
// origin, and the branch/ahead-behind picture are all `vcs-core`'s rules
// (`VcsService`'s translation of them); this widget only builds the trees
// from `changedFiles()`/`branchStatus()` and turns a checkbox toggle or a
// context-menu entry into the matching `VcsService` call.
//
// Deliberately per-file only, not per-hunk: `VcsService::stageHunk`/
// `unstageHunk`'s own doc comment already flags that they diff against
// `HEAD`, not the index, and are "increasingly wrong the more of the file is
// already staged" — correct per-hunk staging needs an index-blob read this
// dock does not have. The gutter's hunk popup (F3-16) already covers the
// per-hunk case for an open file; this dock covers the whole-file case for
// every changed file, open or not.
class ChangesPanel : public QWidget
{
public:
    // `showDiff` is F3-14's entry point into `EditorTabs`' editable diff
    // window, reached by callback rather than a dependency on
    // editor_tabs.h — same shape `ProjectTreeActions::openFile` uses.
    // Double-clicking a changed file's row (or its context menu's "Show
    // Diff") calls it.
    //
    // A row's path is repository-relative; turning it into the absolute
    // path "Copy Path" and "Show File History" need goes through
    // `vcsService->absolutePath()` (the bridge is the one place that knows
    // the repository root — see the double-click handler in the .cpp).
    //
    // `showFileHistory` reveals the File History dock for an absolute path
    // — `ProjectTreeActions::fileHistoryPanel`'s call shape, reached by
    // callback here because `FileHistoryPanel` is built after this panel in
    // `main_window.cpp`.
    ChangesPanel(VcsService *vcsService, std::function<void(const QString &)> showDiff,
                  std::function<void(const QString &)> showFileHistory, QWidget *parent);

protected:
    void showEvent(QShowEvent *event) override;

private:
    void refresh();
    void onItemChanged(QTreeWidgetItem *item, int column);
    void doCommit(bool amend, bool push);
    void refreshEmptyState();
    void showContextMenu(const QPoint &pos);

    VcsService *vcsService_;
    std::function<void(const QString &)> showDiff_;
    std::function<void(const QString &)> showFileHistory_;
    ChangesToolbar *toolbar_ = nullptr;
    QTreeWidget *tree_ = nullptr;
    QPlainTextEdit *messageEdit_ = nullptr;
    QPushButton *commitButton_ = nullptr;
    QPushButton *commitAndPushButton_ = nullptr;
    QPushButton *amendButton_ = nullptr;
    QWidget *repoWidgets_ = nullptr;
    // Shown instead of `repoWidgets_` when `!vcsService_->isRepository()`:
    // no tree, no commit box, just a label and an "Initialize Git
    // Repository" button (plus a "Not now" link the first time it is
    // asked) — see `refreshEmptyState`.
    QWidget *emptyState_ = nullptr;
    QLabel *emptyStateLabel_ = nullptr;
    QPushButton *initButton_ = nullptr;
    QPushButton *notNowButton_ = nullptr;
    // Set while refresh() repopulates the tree, so the checkbox toggles it
    // performs don't loop back into stageFile/unstageFile calls.
    bool populating_ = false;
};

} // namespace ui_shell
