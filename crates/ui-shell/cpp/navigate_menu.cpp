#include "navigate_menu.h"

#include "declaration_navigator.h"
#include "dock_layout.h"
#include "editor_tabs.h"
#include "find_usages_panel.h"
#include "hierarchy_panel.h"
#include "keymap_page.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>

namespace ui_shell {

void buildNavigateMenu(QMainWindow *window, LanguageService *languageService,
                        SearchModel *searchModel, EditorTabs *editorTabs, AppSettings *appSettings,
                        QHash<QString, QAction *> &actions, DockRegistry *docks,
                        FindUsagesPanel *findUsagesPanel, HierarchyPanel *hierarchyPanel)
{
    // N8: code navigation. The Ctrl+Click gesture and every action below
    // route through the one DeclarationNavigator, so there is a single
    // place that turns a resolution result into a jump.
    auto *navigator = new DeclarationNavigator(languageService, searchModel, editorTabs, window);
    editorTabs->setDeclarationRequestedCallback(
      [navigator](int position) { navigator->resolveAt(position); });

    QMenu *navigateMenu = window->menuBar()->addMenu(QObject::tr("&Navigate"));
    QAction *goToDeclarationAction =
      registerAction(navigateMenu, QStringLiteral("navigate.goToDeclaration"),
                      QObject::tr("Go to Declaration"), appSettings, actions);
    QObject::connect(goToDeclarationAction, &QAction::triggered, window,
                      [editorTabs]() { editorTabs->requestDeclarationAtCaret(); });

    QAction *findUsagesAction =
      registerAction(navigateMenu, QStringLiteral("navigate.findUsages"),
                      QObject::tr("Find Usages"), appSettings, actions);
    QObject::connect(findUsagesAction, &QAction::triggered, window,
                      [docks, findUsagesPanel, editorTabs]() {
        const QString name = editorTabs->wordUnderCursor();
        const QString path = editorTabs->currentPath();
        if (name.isEmpty() || path.isEmpty()) {
            return;
        }
        // R8: from the caret, so a running language server's own
        // `textDocument/references` answer can be tried first.
        const auto at = editorTabs->lspPositionAt(editorTabs->caretPosition());
        docks->show(QStringLiteral("findUsages"));
        findUsagesPanel->findUsagesAt(name, path, at.first, at.second);
    });

    QAction *goToImplementationAction =
      registerAction(navigateMenu, QStringLiteral("navigate.goToImplementation"),
                      QObject::tr("Go to Implementation"), appSettings, actions);
    QObject::connect(goToImplementationAction, &QAction::triggered, window,
                      [docks, findUsagesPanel, editorTabs]() {
                          const QString name = editorTabs->wordUnderCursor();
                          if (name.isEmpty()) {
                              return;
                          }
                          docks->show(QStringLiteral("findUsages"));
                          findUsagesPanel->findImplementations(name);
                      });

    QAction *goToInterfaceAction =
      registerAction(navigateMenu, QStringLiteral("navigate.goToInterface"),
                      QObject::tr("Go to Interface"), appSettings, actions);
    QObject::connect(goToInterfaceAction, &QAction::triggered, window,
                      [docks, findUsagesPanel, editorTabs]() {
        const QString name = editorTabs->wordUnderCursor();
        if (name.isEmpty()) {
            return;
        }
        docks->show(QStringLiteral("findUsages"));
        findUsagesPanel->findSupertypes(name);
    });

    // C11-followup: the caret's own file and LSP position — same convention
    // requestIntentions/requestSignatureHelp already send.
    auto atCaret = [editorTabs]() {
        return editorTabs->lspPositionAt(editorTabs->caretPosition());
    };
    QAction *callHierarchyAction =
      registerAction(navigateMenu, QStringLiteral("navigate.showCallHierarchy"),
                      QObject::tr("Show Call Hierarchy"), appSettings, actions);
    QObject::connect(callHierarchyAction, &QAction::triggered, window,
                      [docks, hierarchyPanel, editorTabs, atCaret]() {
        const QString path = editorTabs->currentPath();
        if (path.isEmpty()) {
            return;
        }
        const auto at = atCaret();
        docks->show(QStringLiteral("hierarchy"));
        hierarchyPanel->showCallHierarchyAt(path, at.first, at.second);
    });

    QAction *typeHierarchyAction =
      registerAction(navigateMenu, QStringLiteral("navigate.showTypeHierarchy"),
                      QObject::tr("Show Type Hierarchy"), appSettings, actions);
    QObject::connect(typeHierarchyAction, &QAction::triggered, window,
                      [docks, hierarchyPanel, editorTabs, atCaret]() {
        const QString path = editorTabs->currentPath();
        if (path.isEmpty()) {
            return;
        }
        const auto at = atCaret();
        docks->show(QStringLiteral("hierarchy"));
        hierarchyPanel->showTypeHierarchyAt(path, at.first, at.second);
    });

    navigateMenu->addSeparator();
    // R4: F2 / Shift+F2. `EditorTabs::goToDiagnostic` reads the caret's own
    // file and position, the same convention every other caret-relative
    // action here follows.
    QAction *nextDiagnosticAction =
      registerAction(navigateMenu, QStringLiteral("code.nextDiagnostic"),
                      QObject::tr("Next Highlighted Error"), appSettings, actions);
    QObject::connect(nextDiagnosticAction, &QAction::triggered, window,
                      [editorTabs]() { editorTabs->goToDiagnostic(/*forward=*/true); });
    QAction *previousDiagnosticAction =
      registerAction(navigateMenu, QStringLiteral("code.previousDiagnostic"),
                      QObject::tr("Previous Highlighted Error"), appSettings, actions);
    QObject::connect(previousDiagnosticAction, &QAction::triggered, window,
                      [editorTabs]() { editorTabs->goToDiagnostic(/*forward=*/false); });

    navigateMenu->addSeparator();
    QAction *backAction = registerAction(navigateMenu, QStringLiteral("navigate.back"),
                                          QObject::tr("Back"), appSettings, actions);
    QObject::connect(backAction, &QAction::triggered, window,
                      [editorTabs]() { editorTabs->jumpBack(); });
    QAction *forwardAction = registerAction(navigateMenu, QStringLiteral("navigate.forward"),
                                             QObject::tr("Forward"), appSettings, actions);
    QObject::connect(forwardAction, &QAction::triggered, window,
                      [editorTabs]() { editorTabs->jumpForward(); });

    // Enabled state comes from the session's stack, not from a second copy
    // kept here. Applied once now and again after every jump.
    auto refreshNavigationActions = [editorTabs, backAction, forwardAction]() {
        backAction->setEnabled(editorTabs->canJumpBack());
        forwardAction->setEnabled(editorTabs->canJumpForward());
    };
    editorTabs->setNavigationChangedCallback(refreshNavigationActions);
    refreshNavigationActions();
}

} // namespace ui_shell
