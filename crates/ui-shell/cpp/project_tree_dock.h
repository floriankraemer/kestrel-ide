#pragma once

#include <QHash>
#include <QString>
#include <functional>

class AiChat;
class ProjectTreeModel;
class VcsService;

class QAction;
class QMainWindow;
class QMenu;
class QTreeView;
class AppSettings;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class AiChatPanel;
class DockRegistry;
class FileHistoryPanel;

// Everything the project tree's context menu acts on.
//
// A struct rather than several parameters: the list is long because the menu
// reaches the AI chat, and a positional call of that length is one
// transposition away from a bug the compiler cannot see.
struct ProjectTreeActions
{
    QMainWindow *window;
    AiChat *aiChat;
    AiChatPanel *aiChatPanel;
    // Reveals the AI Chat dock (F0-7): re-adds it to its default placement
    // first if a restored layout left it homeless, same as every other dock.
    DockRegistry *docks;
    // Opening a file is EditorTabs' job, and the tree does not depend on
    // editor_tabs.h to say so — a callback keeps it that way, the same
    // shape the search results panel already takes.
    std::function<void(const QString &)> openFile;
    // The active editor tab's path ("" if none), for the locate-in-tree
    // button — same reasoning as `openFile`: EditorTabs' job, reached
    // through a callback rather than a dependency on editor_tabs.h.
    std::function<QString()> currentEditorPath;
    // F3-14: "Compare with…" — opens a read-only diff tab over two files'
    // current contents, no Git involved. Same callback shape as `openFile`.
    std::function<void(const QString &, const QString &)> compareFiles;
    // The Git submenu (project_tree_git_menu.cpp). Null when the app has no
    // VCS service at all; the submenu also asks `isRepository()` before it
    // offers anything.
    VcsService *vcsService;
    // Where "Show File History" points the dock. The dock itself is revealed
    // through `docks`, like every other one.
    FileHistoryPanel *fileHistoryPanel;
    // "Compare with HEAD" — an editable working-tree-vs-HEAD diff for one
    // path. A callback for the same reason `openFile` is: it is EditorTabs'
    // job, and the tree does not depend on editor_tabs.h to say so.
    std::function<void(const QString &)> showDiffAgainstHead;
};

// The tree view plus the toolbar's locate action, which the active-tab-
// changed callback (main_window.cpp) enables only while a tab is open.
struct ProjectTreeDock
{
    QTreeView *view;
    QAction *locateAction;
};

// Builds the Project dock: the toolbar (sort, locate), the tree view, the
// icon-decoration proxy between it and the model, and the dock widget
// itself, docked left of `editorArea`.
ProjectTreeDock createProjectTreeDock(ads::CDockManager *dockManager,
                                      ads::CDockAreaWidget *editorArea,
                                      ProjectTreeModel *treeModel,
                                      DockRegistry *docks);

// Wires the tree's gestures: click to open, right-click for the
// create/rename/delete/attach menu (US-2b), and the locate action's reveal-
// in-tree behavior. Separate from construction only because the panels the
// menu reaches are built after the dock is.
void wireProjectTree(QTreeView *treeView,
                     QAction *locateAction,
                     ProjectTreeModel *treeModel,
                     const ProjectTreeActions &actions);

// Adds the `view.projectTree` action to `viewMenu` so the dock can be raised
// again after its "x" is closed (#117) — same registerAction/DockRegistry
// shape as every other dock's View-menu entry, split out here rather than
// main_window.cpp only to stay under that file's line ceiling.
void wireProjectTreeViewAction(QMenu *viewMenu,
                               DockRegistry *docks,
                               AppSettings *appSettings,
                               QHash<QString, QAction *> &actions);

} // namespace ui_shell
