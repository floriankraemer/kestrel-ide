#include "project_tree_git_menu.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "file_history_panel.h"
#include "project_tree_dock.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAction>
#include <QFileInfo>
#include <QMainWindow>
#include <QMenu>
#include <QMessageBox>
#include <QPushButton>

namespace ui_shell {

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

    git->addSeparator();
    QAction *compareAction = git->addAction(QObject::tr("Compare with HEAD"));
    compareAction->setEnabled(tracked);
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
    QObject::connect(compareAction, &QAction::triggered, git, [actions, absolutePath]() {
        actions.showDiffAgainstHead(absolutePath);
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
        // time, naming the file, with Cancel as the default button.
        const QString name = QFileInfo(absolutePath).fileName();
        QMessageBox confirm(QMessageBox::Warning, QObject::tr("Revert File Changes"),
                             QObject::tr("Discard all changes to \"%1\"?").arg(name),
                             QMessageBox::Cancel, window);
        confirm.setInformativeText(
          QObject::tr("The file goes back to its last committed state. This cannot be undone."));
        QAbstractButton *revert =
          confirm.addButton(QObject::tr("Revert"), QMessageBox::DestructiveRole);
        confirm.setDefaultButton(QMessageBox::Cancel);
        e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"revert_file_confirm\"}");
        confirm.exec();
        const bool accepted = confirm.clickedButton() == revert;
        e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"revert_file_confirm\","
                                "\"accepted\":%1}")
                  .arg(accepted ? "true" : "false"));
        if (accepted) {
            vcs->revertFile(absolutePath);
        }
    });
}

} // namespace ui_shell
