#include "layouts_menu.h"

#include "dock_layout.h"
#include "e2e_mark.h"
#include "editor_tabs.h"
#include "keymap_page.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include "DockManager.h"

#include <QAction>
#include <QInputDialog>
#include <QMainWindow>
#include <QMenu>
#include <QMessageBox>
#include <QStringList>

namespace ui_shell {

namespace {

// The two layers a layout can be saved into, as the words the seam expects.
const QLatin1String kScopeGlobal("global");
const QLatin1String kScopeProject("project");

// Applies one saved layout: the docks first, then the editor grid.
//
// Order matters. `restoreState()` rebuilds every dock area, which includes
// the one holding the editor, so reshaping the splitter tree inside it first
// would only have it torn down again a line later.
void applyLayout(QMainWindow *window, AppSettings *appSettings, DockRegistry *docks,
                 EditorTabs *editorTabs, const QString &name)
{
    const auto layout = appSettings->namedLayout(name);
    if (layout.code != 0) {
        // The layout went away between this menu being built and the entry
        // being chosen — another window saved over the file, or the project
        // that defined it closed. The menu rebuilds itself on aboutToShow,
        // so there is nothing to repair, only something to say.
        QMessageBox::warning(window, QObject::tr("Apply Layout"), layout.message);
        return;
    }

    docks->restoreState(layout.window_state);
    if (!layout.editor_grid.isEmpty()) {
        editorTabs->applyGrid(layout.editor_grid);
    }
    e2eMark(QStringLiteral("{\"ev\":\"layout_applied\",\"name\":%1}").arg(e2eJson(name)));
}

// Asks which layer to save into — but only when there is a project to save
// into. With none open, the person's own layer is the only answer there is,
// and a dialog offering one option is a dialog worth not showing.
//
// Returns an empty string when the user cancelled.
QString askScope(QMainWindow *window, AppSettings *appSettings)
{
    if (!appSettings->isProjectOpen()) {
        return kScopeGlobal;
    }
    const QStringList choices{QObject::tr("Just for me"), QObject::tr("Share with this project")};
    bool accepted = false;
    const QString choice =
      QInputDialog::getItem(window, QObject::tr("Save Layout"), QObject::tr("Save this layout:"),
                            choices, 0, false, &accepted);
    if (!accepted) {
        return QString();
    }
    return choice == choices.at(1) ? QString(kScopeProject) : QString(kScopeGlobal);
}

void saveCurrentLayout(QMainWindow *window, AppSettings *appSettings,
                       ads::CDockManager *dockManager, EditorTabs *editorTabs)
{
    bool accepted = false;
    const QString name =
      QInputDialog::getText(window, QObject::tr("Save Layout"), QObject::tr("Layout name:"),
                            QLineEdit::Normal, QString(), &accepted)
        .trimmed();
    if (!accepted || name.isEmpty()) {
        return;
    }
    const QString scope = askScope(window, appSettings);
    if (scope.isEmpty()) {
        return;
    }

    // Base64 for the same reason the session's dock blob is: saveState()
    // returns raw bytes and the Rust field is a UTF-8 String.
    const QString dockState = QString::fromLatin1(dockManager->saveState().toBase64());
    const auto result =
      appSettings->saveNamedLayout(name, scope, dockState, editorTabs->saveGrid());
    if (result.code != 0) {
        QMessageBox::critical(window, QObject::tr("Save Layout"), result.message);
        return;
    }
    e2eMark(QStringLiteral("{\"ev\":\"layout_saved\",\"name\":%1}").arg(e2eJson(name)));
}

void deleteLayout(QMainWindow *window, AppSettings *appSettings)
{
    const QStringList names = appSettings->layoutNames();
    if (names.isEmpty()) {
        QMessageBox::information(window, QObject::tr("Delete Layout"),
                                  QObject::tr("There are no saved layouts."));
        return;
    }
    bool accepted = false;
    const QString name =
      QInputDialog::getItem(window, QObject::tr("Delete Layout"),
                            QObject::tr("Layout to delete:"), names, 0, false, &accepted);
    if (!accepted || name.isEmpty()) {
        return;
    }
    const auto result = appSettings->deleteNamedLayout(name);
    if (result.code != 0) {
        QMessageBox::critical(window, QObject::tr("Delete Layout"), result.message);
    }
}

} // namespace

void buildLayoutsMenu(QMenu *viewMenu, QMainWindow *window, AppSettings *appSettings,
                       ads::CDockManager *dockManager, DockRegistry *docks,
                       EditorTabs *editorTabs, QHash<QString, QAction *> &actions)
{
    QMenu *layoutsMenu = viewMenu->addMenu(QObject::tr("Layouts"));

    QAction *saveAction = registerAction(layoutsMenu, QStringLiteral("view.saveLayout"),
                                          QObject::tr("Save Current Layout..."), appSettings,
                                          actions);
    QAction *deleteAction = registerAction(layoutsMenu, QStringLiteral("view.deleteLayout"),
                                            QObject::tr("Delete Layout..."), appSettings, actions);

    QObject::connect(saveAction, &QAction::triggered, window,
                      [window, appSettings, dockManager, editorTabs]() {
                          saveCurrentLayout(window, appSettings, dockManager, editorTabs);
                      });
    QObject::connect(deleteAction, &QAction::triggered, window,
                      [window, appSettings]() { deleteLayout(window, appSettings); });

    // Repopulated every time the menu opens rather than kept in step with a
    // change signal: the set of layouts moves when a project opens or closes
    // as well as when one is saved, and `recentProjectsMenu` already
    // establishes that rebuilding a short list on demand beats wiring three
    // notifications to it.
    QObject::connect(layoutsMenu, &QMenu::aboutToShow, layoutsMenu,
                      [layoutsMenu, saveAction, deleteAction, window, appSettings, docks,
                       editorTabs]() {
                          // Only the generated entries go; the two commands
                          // are owned by the keymap and must survive.
                          for (QAction *action : layoutsMenu->actions()) {
                              if (action != saveAction && action != deleteAction) {
                                  layoutsMenu->removeAction(action);
                                  action->deleteLater();
                              }
                          }

                          const QStringList names = appSettings->layoutNames();
                          deleteAction->setEnabled(!names.isEmpty());
                          if (names.isEmpty()) {
                              return;
                          }

                          QAction *before = layoutsMenu->actions().constFirst();
                          for (const QString &name : names) {
                              auto *entry = new QAction(name, layoutsMenu);
                              QObject::connect(entry, &QAction::triggered, window,
                                                [window, appSettings, docks, editorTabs,
                                                 name]() {
                                                    applyLayout(window, appSettings, docks,
                                                                editorTabs, name);
                                                });
                              layoutsMenu->insertAction(before, entry);
                          }
                          layoutsMenu->insertSeparator(before);
                      });
}

} // namespace ui_shell
