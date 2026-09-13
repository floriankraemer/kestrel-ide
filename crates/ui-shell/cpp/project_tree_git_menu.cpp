#include "project_tree_git_menu.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "file_history_panel.h"
#include "git_dialogs.h"
#include "project_tree_dock.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAction>
#include <QFileInfo>
#include <QInputDialog>
#include <QMainWindow>
#include <QMenu>
#include <QMetaObject>
#include <QStringList>

#include <memory>

namespace ui_shell {

namespace {

// One editable picker for both the file and the project flows (R6): local
// branches and tags as the offered items (`refNames()`, from the last
// `refreshBranches`), with anything `git rev-parse` understands typeable
// into the same box — a commit id, `HEAD~2`, `origin/main`. Empty when
// cancelled.
QString pickRevision(QMainWindow *window, VcsService *vcs)
{
    QStringList names;
    for (const FfiBranch &ref : vcs->refNames()) {
        names.append(QString(ref.name));
    }
    bool ok = false;
    const QString revision =
      QInputDialog::getItem(window, QObject::tr("Compare with Branch, Tag or Revision"),
                             QObject::tr("Branch, tag or revision:"), names, 0,
                             /*editable=*/true, &ok);
    return ok ? revision.trimmed() : QString();
}

} // namespace

void appendProjectGitSubmenu(QMenu &menu, const ProjectTreeActions &actions)
{
    VcsService *vcs = actions.vcsService;
    if (!vcs || !vcs->isRepository()) {
        return;
    }
    menu.addSeparator();
    QMenu *git = menu.addMenu(QObject::tr("Git"));
    e2eMarkMenuActions(git, "project_tree_git_action");
    QAction *compareAction =
      git->addAction(QObject::tr("Compare Project with Branch, Tag or Revision…"));

    QMainWindow *window = actions.window;
    QObject::connect(compareAction, &QAction::triggered, git, [actions, vcs, window]() {
        const QString revision = pickRevision(window, vcs);
        if (revision.isEmpty()) {
            return;
        }
        // The file list comes back from the worker; the menu this lambda
        // belongs to is gone by then, so the one-shot connection hangs off
        // the window and disconnects itself — the same shape
        // `EditorTabs::openCompareRevisions` uses for `blobReady`.
        auto connection = std::make_shared<QMetaObject::Connection>();
        *connection = QObject::connect(
          vcs, &VcsService::changedPathsReady, window,
          [actions, vcs, window, revision, connection](const QString &readyRevision,
                                                       const ::rust::Vec<FfiRepoPath> &paths) {
              if (readyRevision != revision) {
                  return;
              }
              QObject::disconnect(*connection);
              QStringList names;
              for (const FfiRepoPath &path : paths) {
                  names.append(QString(path.path));
              }
              if (names.isEmpty()) {
                  QInputDialog::getItem(window, QObject::tr("Compare Project with %1").arg(revision),
                                        QObject::tr("No files differ from %1.").arg(revision),
                                        QStringList{QObject::tr("(no changes)")}, 0, false);
                  return;
              }
              bool ok = false;
              const QString picked =
                QInputDialog::getItem(window, QObject::tr("Compare Project with %1").arg(revision),
                                      QObject::tr("File to compare:"), names, 0,
                                      /*editable=*/false, &ok);
              if (ok && !picked.isEmpty()) {
                  actions.compareWithRevision(vcs->absolutePath(picked), revision);
              }
          });
        vcs->requestChangedPathsAgainst(revision);
    });
}

void appendGitSubmenu(QMenu &menu, const QString &absolutePath,
                       const ProjectTreeActions &actions)
{
    VcsService *vcs = actions.vcsService;
    if (!vcs || !vcs->isRepository()) {
        return;
    }

    // One status lookup for the whole submenu, and `VcsService` resolves the
    // absolute path against the repository root itself — the view never does
    // path arithmetic on a repository-relative path.
    const FfiChangedFile status = vcs->fileStatus(absolutePath);
    const bool hasStaged = status.staged != FfiChangeKind::None;
    const bool hasUnstaged = status.unstaged != FfiChangeKind::None;
    const bool untracked = status.unstaged == FfiChangeKind::Untracked;
    // `HEAD` has nothing to diff against or restore for a file it has never
    // seen, so those two entries are about tracked-ness, not dirtiness.
    const bool tracked = !untracked;

    menu.addSeparator();
    QMenu *git = menu.addMenu(QObject::tr("Git"));
    e2eMarkMenuActions(git, "project_tree_git_action");

    QAction *stageAction = git->addAction(QObject::tr("Stage File"));
    stageAction->setEnabled(hasUnstaged);
    QAction *unstageAction = git->addAction(QObject::tr("Unstage File"));
    unstageAction->setEnabled(hasStaged);
    // Untracked only (R6): ignoring a tracked file changes nothing git
    // already knows about, which would read as the entry doing nothing.
    QAction *ignoreAction = git->addAction(QObject::tr("Add to .gitignore"));
    ignoreAction->setEnabled(untracked);

    git->addSeparator();
    QAction *compareAction = git->addAction(QObject::tr("Compare with HEAD"));
    compareAction->setEnabled(tracked);
    QAction *compareRevisionAction =
      git->addAction(QObject::tr("Compare with Branch, Tag or Revision…"));
    compareRevisionAction->setEnabled(tracked);
    QAction *historyAction = git->addAction(QObject::tr("Show File History"));
    historyAction->setEnabled(tracked);

    git->addSeparator();
    QAction *revertAction = git->addAction(QObject::tr("Revert File Changes…"));
    revertAction->setEnabled(tracked && (hasStaged || hasUnstaged));

    QMainWindow *window = actions.window;
    QObject::connect(stageAction, &QAction::triggered, git,
                      [vcs, absolutePath]() { vcs->stageFile(absolutePath); });
    QObject::connect(unstageAction, &QAction::triggered, git,
                      [vcs, absolutePath]() { vcs->unstageFile(absolutePath); });
    QObject::connect(ignoreAction, &QAction::triggered, git,
                      [vcs, absolutePath]() { vcs->addToGitignore(absolutePath); });
    QObject::connect(compareAction, &QAction::triggered, git, [actions, absolutePath]() {
        actions.showDiffAgainstHead(absolutePath);
    });
    QObject::connect(compareRevisionAction, &QAction::triggered, git,
                      [actions, vcs, absolutePath]() {
                          const QString revision = pickRevision(actions.window, vcs);
                          if (!revision.isEmpty()) {
                              actions.compareWithRevision(absolutePath, revision);
                          }
                      });
    QObject::connect(historyAction, &QAction::triggered, git, [actions, absolutePath]() {
        actions.docks->show(QStringLiteral("fileHistory"));
        // `fileHistory()` takes the absolute path and strips it itself, the
        // same as every other per-file VCS read.
        actions.fileHistoryPanel->setCurrentFile(absolutePath);
    });
    QObject::connect(revertAction, &QAction::triggered, git, [vcs, window, absolutePath]() {
        // The working-tree content is gone once `git checkout HEAD --` has
        // run, and nothing in the IDE can bring it back — so this asks, every
        // time, naming the file (git_dialogs.h, shared with the Changes
        // dock's own Discard Changes… entry, G8).
        const QString name = QFileInfo(absolutePath).fileName();
        if (confirmDiscardChanges(window, name, "revert_file_confirm")) {
            vcs->revertFile(absolutePath);
        }
    });
}

} // namespace ui_shell
