#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <QWidget>

#include <functional>

class QAction;
class QCompleter;
class QLabel;
class QLineEdit;
class QMenu;
class QTabWidget;
class QToolButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;
class ContainerDetailArea;
class EditorTabs;

// The Containers dock (containers plan C2): every configured Docker/Podman
// connection as a tree of Containers / Images / Networks / Volumes /
// Compose / Pods, with a Dashboard for the selected row.
//
// Humble view per CLAUDE.md: which rows exist, what they say and which
// icon they carry are `container_core::tree`'s answers, read back through
// `ContainerService::nodes()` exactly as `TestsPanel` reads `TestService`.
// This widget builds `QTreeWidgetItem`s from those rows (keeping selection
// and expansion by row id across a rebuild), and turns toolbar, search and
// context-menu input into calls back into `ContainerService`.
class ContainersPanel : public QWidget
{
public:
    // Opens Settings on the Containers page — "New connection...", "Edit
    // configuration...", "Registry...", and a registry node's "Edit...".
    // The argument is the (translated) tab to additionally select on that
    // page — `tr("Registries")` for the last two, empty for the first two
    // (C7 review follow-up: previously every one of these opened on
    // whichever tab was last shown). Wired by the main window once the
    // settings context exists.
    using OpenSettings = std::function<void(const QString &initialTab)>;
    using OpenAt = std::function<void(const QString &, int, int)>;

    ContainersPanel(ContainerService *containerService, TerminalSupervisor *terminalSupervisor,
                    AppSettings *appSettings, OpenAt openAt, QWidget *parent);

    void setOpenSettingsHandler(OpenSettings handler);

    // C5 (ADR-0056): the compose tree's Start All/Stop/Down/Scale and
    // "Create Container..." (replacing C4's `createContainerQuick`) all
    // need the run-config machinery, which is constructed after this panel
    // (`main_window.cpp`'s `runConfigEditor`) — set once, right after both
    // exist, the same "setter after construction" shape
    // `setOpenSettingsHandler` already uses.
    // `editorTabs` is Compose "Jump to Source" (C5, ADR-0056)'s: the one
    // Containers-dock action that opens an editor tab rather than calling a
    // container/run service.
    void setRunContext(RunService *runService, RunConfigEditor *runConfigEditor,
                       EditorTabs *editorTabs);

    // C6: a compose code lens was clicked — select `nodeId` in the tree
    // (which opens its Log tab) and bring that tab to the front.
    void revealContainerLog(const QString &nodeId);
    // C6: the editor's "Pull image" intention — the same Pull tab the
    // Images console opens.
    void openPullTab(const QString &connectionId, const QString &reference);

private:
    void onTreeChanged();
    void onSelectionChanged();
    void showContextMenu(const QPoint &pos);
    // C9 polish: "Add from contexts..." opens a checkbox dialog of
    // discovered-but-not-yet-configured connections instead of only
    // opening Settings on the discovery button.
    void openAddFromContextsDialog();
    void updateToolbarEnablement();
    void report(const FfiResult &result);
    QString selectedConnectionId() const;
    QTreeWidgetItem *selectedItem() const;

    // containers_actions.cpp: lifecycle toolbar buttons + the container/
    // containers-group context menus (C3).
    void buildLifecycleToolbar();
    void updateLifecycleButtons();
    void triggerStart();
    void triggerStop();
    void triggerRestart();
    void triggerPauseOrUnpause();
    void triggerRemove();
    void triggerCleanUp();
    void showContainerContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showContainersGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);

    // containers_resources.cpp: image/network/volume context menus, the
    // live Pull Image/Clean Up ▾ toolbar buttons, and the Create Network/
    // Create Volume/Tag/Copy Image/Create Container dialogs (C4).
    void buildResourceToolbar();
    void showImageContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showNetworkContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showVolumeContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showImagesGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showNetworksGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showVolumesGroupContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void triggerPullImage();
    void openCreateNetworkDialog(const QString &connectionId);
    void openCreateVolumeDialog(const QString &connectionId);
    void openTagDialog(const QString &nodeId);
    void openCopyImageDialog(const QString &nodeId);
    void openCreateContainerDialog(const QString &nodeId);

    // containers_compose.cpp: the Compose group/project/service context
    // menus (Start All/Stop/Down/Scale/Jump to Source) and the project
    // node's services-and-counts Dashboard (C5, ADR-0056).
    void showComposeProjectContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showComposeServiceContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void triggerComposeStartAll(const QString &nodeId);
    void triggerComposeStop(const QString &nodeId);
    void triggerComposeDown(const QString &nodeId);
    void triggerComposeScale(const QString &serviceNodeId);
    void triggerComposeJumpToSource(const QString &nodeId);
    void showComposeProjectDashboard(const QString &nodeId);

    // containers_registry.cpp (C7): registry/registry-repo/registry-tag
    // context menus and the Push Image dialog, split out under the same
    // file-size ratchet as the files above.
    void showRegistryContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showRegistryRepoContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void showRegistryTagContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void openPushImageDialog(const QString &imageNodeId);

    // containers_podman.cpp (C9): a pod node's own lifecycle context menu,
    // and a Podman connection's Start machine/Stop machine entries added
    // onto the connection context menu built in showContextMenu().
    void showPodContextMenu(QTreeWidgetItem *item, const QPoint &globalPos);
    void addMachineActions(QMenu &menu, const QString &connectionId, QAction *&startAction,
                           QAction *&stopAction);

    ContainerService *containerService_;
    // C7 review follow-up: "Remove" on a registry node writes straight
    // through `AppSettings::removeRegistry` (settings + keychain), the
    // same collaborator `buildContainersPage`/`buildRegistriesPage` already
    // edit registries through — stored here only because this is the first
    // registry mutation to originate from the tree rather than the
    // Settings page.
    AppSettings *appSettings_;
    RunService *runService_ = nullptr;
    RunConfigEditor *runConfigEditor_ = nullptr;
    EditorTabs *editorTabs_ = nullptr;
    OpenSettings openSettings_;

    QToolButton *addButton_ = nullptr;
    QToolButton *refreshButton_ = nullptr;
    QToolButton *connectButton_ = nullptr;
    QToolButton *disconnectButton_ = nullptr;
    QToolButton *pullButton_ = nullptr;
    QToolButton *cleanUpButton_ = nullptr;
    QToolButton *filterButton_ = nullptr;
    QAction *showStoppedAction_ = nullptr;
    QAction *showUntaggedAction_ = nullptr;
    QLineEdit *searchEdit_ = nullptr;
    QLabel *statusLabel_ = nullptr;
    QTreeWidget *tree_ = nullptr;
    QTabWidget *detailTabs_ = nullptr;
    QLabel *dashboardName_ = nullptr;
    QLabel *dashboardId_ = nullptr;
    QLabel *dashboardStatus_ = nullptr;
    QLabel *dashboardDetail_ = nullptr;

    // C3: Start/Stop/Restart/Pause-Unpause/Remove, enabled per
    // `ContainerService::nodeActions` — the rule lives in Rust, these five
    // buttons only read its flags.
    QToolButton *startButton_ = nullptr;
    QToolButton *stopButton_ = nullptr;
    QToolButton *restartButton_ = nullptr;
    QToolButton *pauseButton_ = nullptr;
    QToolButton *removeButton_ = nullptr;

    ContainerDetailArea *detail_ = nullptr;

    // C4: the Images console row, shown at the top of the detail area only
    // when the Images group is selected.
    QWidget *imagesConsole_ = nullptr;
    QLineEdit *pullEdit_ = nullptr;
    QCompleter *pullCompleter_ = nullptr;

    // Rebuilt wholesale on every `treeChanged`, like `TestsPanel`; the
    // expansion and selection are carried across by row id.
    QHash<QString, QTreeWidgetItem *> itemsById_;
    QString selectedNodeId_;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"containers"` — the same one-call pattern `buildTestsDock` uses.
ContainersPanel *buildContainersDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *relativeTo,
                                     ContainerService *containerService,
                                     TerminalSupervisor *terminalSupervisor,
                                     AppSettings *appSettings,
                                     ContainersPanel::OpenAt openAt);

} // namespace ui_shell
