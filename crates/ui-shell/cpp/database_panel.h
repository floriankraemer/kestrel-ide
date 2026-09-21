#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <QWidget>

#include <functional>

class QAction;
class QLabel;
class QLineEdit;
class QToolButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;

// The Database dock (database-tools-plan F2.5): every configured data
// source as a tree of catalogs/schemas/tables/views/routines/…, built
// from `DatabaseService::rows()` — a flattened `db_core::tree::TreeRow`
// list — exactly as `ContainersPanel` builds its tree from
// `ContainerService::nodes()`.
//
// Humble view per CLAUDE.md: grouping, filtering, sorting and which
// actions a row offers are all `db_core::tree`'s answers; this widget
// only turns them into `QTreeWidgetItem`s and turns toolbar/context-menu
// input into `DatabaseService` calls. `virtualDocumentOpened` (Go to
// DDL's tab) is wired directly from `DatabaseService` to `EditorTabs` in
// `editor_tabs.cpp`, the same as `ContainerService`'s — this panel never
// touches tabs itself.
class DatabasePanel : public QWidget
{
public:
    using OpenSettings = std::function<void()>;

    DatabasePanel(DatabaseService *databaseService, QWidget *parent);

    void setOpenSettingsHandler(OpenSettings handler);

private:
    void rebuildTree();
    void onItemExpanded(QTreeWidgetItem *item);
    void showContextMenu(const QPoint &pos);
    void onConnectionStateChanged(const QString &id, FfiDbConnectionState state,
                                  const QString &message);
    void onActionFinished(bool ok, const QString &message);
    void report(const FfiResult &result);
    QString selectedNodeId() const;

    DatabaseService *databaseService_;
    OpenSettings openSettings_;

    QToolButton *addButton_ = nullptr;
    QToolButton *refreshButton_ = nullptr;
    QToolButton *forceRefreshButton_ = nullptr;
    QToolButton *goToDdlButton_ = nullptr;
    QToolButton *groupingButton_ = nullptr;
    QLineEdit *filterEdit_ = nullptr;
    QLabel *statusLabel_ = nullptr;
    QTreeWidget *tree_ = nullptr;

    // Rebuilt wholesale on every `rowsChanged`, like `ContainersPanel`;
    // expansion and selection carry across by node id.
    QHash<QString, QTreeWidgetItem *> itemsById_;

    // What the context menu and Refresh/Go to DDL toolbar buttons need
    // back out of a row, cached at rebuild time — nothing here decides
    // which actions apply (`db_core::tree::actions_for` already did).
    struct RowInfo
    {
        QString label;
        bool canOpenConsole = false;
        bool canEditData = false;
        bool canGoToDdl = false;
        bool canCopyName = false;
        bool canRefresh = false;
        bool canRename = false;
        bool canDrop = false;
        bool canTruncate = false;
        bool canComment = false;
    };
    QHash<QString, RowInfo> rowInfoById_;
};

// Builds the panel, wraps it in a dock widget and registers it with
// `docks` under id `"database"` — the same one-call pattern
// `buildContainersDock`/`buildBuildToolsDock` use.
DatabasePanel *buildDatabaseDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                ads::CDockAreaWidget *relativeTo,
                                DatabaseService *databaseService);

} // namespace ui_shell
