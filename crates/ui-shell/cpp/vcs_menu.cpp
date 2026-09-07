#include "vcs_menu.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "editor_tabs.h"
#include "file_history_panel.h"
#include "keymap_page.h"

#include "DockWidget.h"

#include <QAction>
#include <QCursor>
#include <QInputDialog>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QMessageBox>
#include <QPushButton>
#include <QStatusBar>
#include <QStringList>
#include <QToolButton>

namespace ui_shell {

namespace {

/// One `vcs-core` error code as the plain `int` `FfiResult::code` carries.
/// The view names the constant rather than writing the number (ADR-0003 §4);
/// the enum itself is declared in the bridge, beside the struct it labels.
constexpr int vcsErrorCode(FfiVcsErrorCode code)
{
    return static_cast<int>(code);
}

void showBranchMenu(VcsService *vcsService, QWidget *anchor, const QPoint &globalPos)
{
    auto *menu = new QMenu(anchor);
    menu->setAttribute(Qt::WA_DeleteOnClose);

    // Names copied out to a plain QStringList up front: `rust::Vec<T>` is a
    // borrowed view over Rust-owned memory, not a value this code should
    // hold onto past this function, and the delete-branch lambda below
    // needs the list again after the branch that filled it has closed.
    const QString current = vcsService->currentBranch();
    QStringList names;
    for (const FfiBranch &branch : vcsService->branches()) {
        names.append(branch.name);
    }
    for (const QString &name : names) {
        QAction *action = menu->addAction(name);
        action->setCheckable(true);
        action->setChecked(name == current);
        QObject::connect(action, &QAction::triggered, vcsService,
                          [vcsService, name]() { vcsService->checkout(name); });
    }
    menu->addSeparator();

    QAction *newBranchAction = menu->addAction(QObject::tr("New Branch..."));
    QObject::connect(newBranchAction, &QAction::triggered, vcsService, [vcsService, anchor]() {
        const QString name =
          QInputDialog::getText(anchor, QObject::tr("New Branch"), QObject::tr("Branch name:"));
        if (!name.isEmpty()) {
            vcsService->createBranch(name, QString());
        }
    });

    QAction *deleteBranchAction = menu->addAction(QObject::tr("Delete Branch..."));
    QObject::connect(
      deleteBranchAction, &QAction::triggered, vcsService, [vcsService, anchor, names]() {
          if (names.isEmpty()) {
              return;
          }
          bool ok = false;
          const QString name = QInputDialog::getItem(anchor, QObject::tr("Delete Branch"),
                                                       QObject::tr("Branch:"), names, 0, false, &ok);
          if (!ok || name.isEmpty()) {
              return;
          }
          // A refusal (unmerged commits) is shown, not silently retried —
          // force is a deliberate second click, never an automatic fallback
          // (Repository::delete_branch's own doc comment on why).
          QObject::connect(
            vcsService, &VcsService::vcsFailed, vcsService,
            [vcsService, anchor, name](FfiResult error) {
                QObject::disconnect(vcsService, &VcsService::vcsFailed, vcsService, nullptr);
                if (error.code != vcsErrorCode(FfiVcsErrorCode::UnmergedBranch)) {
                    QMessageBox::warning(anchor, QObject::tr("Delete Branch"), error.message);
                    return;
                }
                const auto choice = QMessageBox::warning(
                  anchor, QObject::tr("Delete Branch"),
                  QObject::tr("'%1' has commits not merged anywhere else. Delete anyway?")
                    .arg(name),
                  QMessageBox::Cancel | QMessageBox::Yes, QMessageBox::Cancel);
                if (choice == QMessageBox::Yes) {
                    vcsService->deleteBranch(name, /*force=*/true);
                }
            });
          vcsService->deleteBranch(name, /*force=*/false);
      });

    menu->popup(globalPos);
}

} // namespace

QToolButton *buildBranchWidget(VcsService *vcsService, QWidget *window, QStatusBar *statusBar)
{
    auto *button = new QToolButton(statusBar);
    button->setAutoRaise(true);
    button->setVisible(false);

    const auto refresh = [button, vcsService]() {
        const bool isRepo = vcsService->isRepository();
        button->setVisible(isRepo);
        if (isRepo) {
            const QString branch = vcsService->currentBranch();
            button->setText(branch.isEmpty() ? QObject::tr("(detached)") : branch);
        }
    };
    QObject::connect(vcsService, &VcsService::branchChanged, button, refresh);
    QObject::connect(vcsService, &VcsService::repositoryChanged, button, [vcsService, refresh]() {
        if (vcsService->isRepository()) {
            vcsService->refreshBranches();
        }
        refresh();
    });
    QObject::connect(button, &QToolButton::clicked, vcsService, [vcsService, button]() {
        showBranchMenu(vcsService, button, button->mapToGlobal(QPoint(0, 0)));
    });

    return button;
}

void buildVcsMenu(QMainWindow *window, VcsService *vcsService, AppSettings *appSettings,
                   QHash<QString, QAction *> &actions, EditorTabs *editorTabs,
                   DockRegistry *docks, FileHistoryPanel *fileHistoryPanel, QMenu *viewMenu)
{
    // No caller of `VcsService` anywhere in `ui-shell` showed `vcsFailed` to
    // the user before this menu existed (the gutter's stage/revert and the
    // Changes dock's stage/commit all rely on it being rare) — this is the
    // first surface broad enough that it should stop being silent.
    // `UnmergedBranch` is excluded: the branch-delete flow above shows its
    // own actionable dialog for that one.
    QObject::connect(vcsService, &VcsService::vcsFailed, window, [window, vcsService](FfiResult error) {
        // Before any early return and before any modal: a `vcsFailed` that
        // ends in an `exec()` blocks the application for the rest of a flow,
        // so an E2E run that hangs afterwards has to be able to see that
        // this is what happened (#210).
        e2eMark(QStringLiteral("{\"ev\":\"vcs_failed\",\"code\":%1,\"message\":%2}")
                  .arg(error.code)
                  .arg(e2eJson(error.message)));
        if (error.code == vcsErrorCode(FfiVcsErrorCode::UnmergedBranch)) {
            return;
        }
        // Git refuses
        // to touch this project root because its ownership looks dubious to
        // it (common on WSL `//wsl.localhost/...` paths and networked
        // drives). The path is already embedded in `error.message`, which
        // `vcs_core::VcsError::DubiousOwnership`'s `Display` impl formats as
        // "Git doesn't trust the ownership of <path>" — reused as-is here
        // rather than re-parsing the path back out of it.
        if (error.code == vcsErrorCode(FfiVcsErrorCode::DubiousOwnership)) {
            QMessageBox box(window);
            box.setWindowTitle(QObject::tr("Git"));
            box.setIcon(QMessageBox::Warning);
            box.setText(error.message);
            box.setInformativeText(
              QObject::tr("This can happen on WSL or networked drives. Mark it as safe to continue."));
            QPushButton *trustButton =
              box.addButton(QObject::tr("Trust This Folder"), QMessageBox::AcceptRole);
            box.addButton(QMessageBox::Cancel);
            box.setDefaultButton(QMessageBox::Cancel);
            box.exec();
            if (box.clickedButton() == trustButton) {
                vcsService->trustDirectory();
            }
            return;
        }
        QMessageBox::warning(window, QObject::tr("Git"), error.message);
    });

    // "V&CS", not "&VCS": "&View" already claims Alt+V, and a menu bar's
    // ambiguous-mnemonic fallback (cycling on a repeated press) is not
    // something either a user or an E2E flow should have to rely on to
    // reach this menu.
    QMenu *vcsMenu = window->menuBar()->addMenu(QObject::tr("V&CS"));
    // A top-level menu bar entry never goes through `exec()` (unlike the
    // tab and hunk-popup context menus), so `aboutToShow`/`aboutToHide` are
    // the only signal an E2E flow has that it is safe to send keystrokes
    // into what is, in X11 terms, a brand new toplevel.
    QObject::connect(vcsMenu, &QMenu::aboutToShow, vcsMenu,
                      []() { e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"vcs_menu\"}"); });
    QObject::connect(vcsMenu, &QMenu::aboutToHide, vcsMenu,
                      []() { e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"vcs_menu\"}"); });
    // Each entry's on-screen rect, so a flow clicks "Show Diff" by name
    // rather than by counting arrow presses.
    e2eMarkMenuActions(vcsMenu, "vcs_menu_action");

    QAction *commitAction = registerAction(vcsMenu, QStringLiteral("vcs.commit"),
                                            QObject::tr("Commit..."), appSettings, actions);
    QObject::connect(commitAction, &QAction::triggered, window,
                      [docks]() { docks->show(QStringLiteral("changes")); });

    QAction *pushAction = registerAction(vcsMenu, QStringLiteral("vcs.push"),
                                          QObject::tr("Push"), appSettings, actions);
    QObject::connect(pushAction, &QAction::triggered, vcsService, [vcsService]() {
        const QString branch = vcsService->currentBranch();
        if (!branch.isEmpty()) {
            vcsService->push(QStringLiteral("origin"), branch, /*setUpstream=*/false);
        }
    });

    QAction *pullAction = registerAction(vcsMenu, QStringLiteral("vcs.pull"),
                                          QObject::tr("Pull"), appSettings, actions);
    QObject::connect(pullAction, &QAction::triggered, vcsService, [vcsService]() {
        const QString branch = vcsService->currentBranch();
        if (!branch.isEmpty()) {
            vcsService->pull(QStringLiteral("origin"), branch);
        }
    });

    QAction *fetchAction = registerAction(vcsMenu, QStringLiteral("vcs.fetch"),
                                           QObject::tr("Fetch"), appSettings, actions);
    QObject::connect(fetchAction, &QAction::triggered, vcsService,
                      [vcsService]() { vcsService->fetch(QStringLiteral("origin")); });

    QAction *branchesAction = registerAction(vcsMenu, QStringLiteral("vcs.branches"),
                                              QObject::tr("Branches..."), appSettings, actions);
    QObject::connect(branchesAction, &QAction::triggered, window, [vcsService, window]() {
        showBranchMenu(vcsService, window, QCursor::pos());
    });

    vcsMenu->addSeparator();

    QAction *showDiffAction = registerAction(vcsMenu, QStringLiteral("vcs.showDiff"),
                                              QObject::tr("Show Diff"), appSettings, actions);
    QObject::connect(showDiffAction, &QAction::triggered, editorTabs,
                      [editorTabs]() { editorTabs->showDiffAgainstHead(); });

    QAction *rollbackAction = registerAction(vcsMenu, QStringLiteral("vcs.rollbackHunk"),
                                              QObject::tr("Rollback Hunk"), appSettings, actions);
    QObject::connect(rollbackAction, &QAction::triggered, editorTabs,
                      [editorTabs]() { editorTabs->rollbackHunkAtCaret(); });

    QAction *nextChangeAction = registerAction(vcsMenu, QStringLiteral("vcs.nextChange"),
                                                QObject::tr("Next Change"), appSettings, actions);
    QObject::connect(nextChangeAction, &QAction::triggered, editorTabs,
                      [editorTabs]() { editorTabs->jumpToChange(/*forward=*/true); });

    QAction *previousChangeAction =
      registerAction(vcsMenu, QStringLiteral("vcs.previousChange"),
                      QObject::tr("Previous Change"), appSettings, actions);
    QObject::connect(previousChangeAction, &QAction::triggered, editorTabs,
                      [editorTabs]() { editorTabs->jumpToChange(/*forward=*/false); });

    QAction *annotateAction = registerAction(vcsMenu, QStringLiteral("vcs.annotate"),
                                              QObject::tr("Annotate with Blame"), appSettings,
                                              actions);
    annotateAction->setCheckable(true);
    QObject::connect(annotateAction, &QAction::toggled, editorTabs,
                      [editorTabs](bool checked) { editorTabs->setAnnotateEnabled(checked); });

    QAction *viewChangesAction = registerAction(viewMenu, QStringLiteral("view.changes"),
                                                 QObject::tr("Changes"), appSettings, actions);
    QObject::connect(viewChangesAction, &QAction::triggered, window,
                      [docks]() { docks->show(QStringLiteral("changes")); });

    QAction *viewHistoryAction = registerAction(viewMenu, QStringLiteral("view.vcsHistory"),
                                                 QObject::tr("File History"), appSettings, actions);
    QObject::connect(viewHistoryAction, &QAction::triggered, window,
                      [docks, editorTabs, fileHistoryPanel]() {
                          docks->show(QStringLiteral("fileHistory"));
                          fileHistoryPanel->setCurrentFile(editorTabs->currentPath());
                      });

    // Not built at all for a non-repository project, per the plan — reached
    // here by disabling rather than omitting (see vcs_menu.h's note): a
    // project's Git-ness is unknown until well after this menu exists.
    const auto refreshEnabled = [vcsMenu, viewChangesAction, viewHistoryAction, vcsService]() {
        const bool isRepo = vcsService->isRepository();
        vcsMenu->menuAction()->setVisible(isRepo);
        viewChangesAction->setVisible(isRepo);
        viewHistoryAction->setVisible(isRepo);
    };
    QObject::connect(vcsService, &VcsService::repositoryChanged, vcsMenu, refreshEnabled);
    refreshEnabled();

    // The Changes dock follows the same rule Problems does (dock_layout.h):
    // a plain `toggleView(true)` rather than `docks->show()` when a project
    // turns out to be a repository, so its tab reappears without raising
    // over whatever the user is already looking at. File History stays
    // hidden until `view.vcsHistory` deliberately opens it.
    QObject::connect(vcsService, &VcsService::repositoryChanged, window, [docks, vcsService]() {
        if (vcsService->isRepository()) {
            docks->dock(QStringLiteral("changes"))->toggleView(true);
        } else {
            docks->hide(QStringLiteral("changes"));
        }
        docks->hide(QStringLiteral("fileHistory"));
    });
}

} // namespace ui_shell
