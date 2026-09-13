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

    ContainersPanel(ContainerService *containerService, QWidget *parent);

    void setOpenSettingsHandler(OpenSettings handler);

private:
    void onTreeChanged();
    void onSelectionChanged();
    void showContextMenu(const QPoint &pos);
    void updateToolbarEnablement();
    void report(const FfiResult &result);
    QString selectedConnectionId() const;
    QTreeWidgetItem *selectedItem() const;

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

    // Rebuilt wholesale on every `treeChanged`, like `TestsPanel`; the
    // expansion and selection are carried across by row id.
    QHash<QString, QTreeWidgetItem *> itemsById_;
    QString selectedNodeId_;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"containers"` — the same one-call pattern `buildTestsDock` uses.
ContainersPanel *buildContainersDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                     ads::CDockAreaWidget *relativeTo,
                                     ContainerService *containerService);

} // namespace ui_shell
