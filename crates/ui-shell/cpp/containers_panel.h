#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <QWidget>

#include <functional>

class QAction;
class QLabel;
class QLineEdit;
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
    // Opens Settings on the Containers page — "New connection..." and "Edit
    // configuration...". Wired by the main window once the settings context
    // exists.
    using OpenSettings = std::function<void()>;
    using OpenAt = std::function<void(const QString &, int, int)>;

    ContainersPanel(ContainerService *containerService, TerminalSupervisor *terminalSupervisor,
                    AppSettings *appSettings, OpenAt openAt, QWidget *parent);

    void setOpenSettingsHandler(OpenSettings handler);

private:
    void onTreeChanged();
    void onSelectionChanged();
    void showContextMenu(const QPoint &pos);
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

    ContainerService *containerService_;
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
