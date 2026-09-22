#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QColor>
#include <QHash>
#include <QString>
#include <QStringList>
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

    DatabasePanel(DatabaseService *databaseService, ExchangeService *exchangeService,
                 DocumentManager *documentManager, QWidget *parent);

    void setOpenSettingsHandler(OpenSettings handler);

    // E2E only (`crates/app/tests/e2e_database.rs`): re-report the tree's
    // row rects now that the dock is actually on screen and laid out —
    // `buildDatabaseDock` calls this once per `visibilityChanged(true)`,
    // the same reasoning `ContainersPanel::refreshE2eRects` gives.
    void refreshE2eRects() const;

protected:
    // Esc clears the speed-search filter edit without leaving this
    // widget's own key handling to notice a focus-scoped shortcut buried
    // in `QLineEdit` — installed on `filterEdit_` only.
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    // Every row's own screen rect, id- and kind-keyed — a collapsed or
    // scrolled-away row is filtered out, the same contract
    // `ContainersPanel::rowRectsJson` documents.
    QStringList rowRectsJson() const;
    void rebuildTree();
    void onItemExpanded(QTreeWidgetItem *item);
    // A `source` row has no toolbar/menu affordance of its own to connect
    // it — double-click is the same gesture `ContainersPanel` uses for its
    // `connection`-kind rows, and the only one a `source` row's own
    // `expandable` flag does not already cover (expanding it is a no-op
    // per `DatabaseService::expand`'s own `$root` short-circuit).
    void onItemDoubleClicked(QTreeWidgetItem *item);
    void showContextMenu(const QPoint &pos);
    void onConnectionStateChanged(const QString &id, FfiDbConnectionState state,
                                  const QString &message);
    void onActionFinished(bool ok, const QString &message);
    void report(const FfiResult &result);
    QString selectedNodeId() const;
    // Rename/Drop/Truncate/Comment (F2 polish): show the exact statement
    // `DatabaseService::actionPreview` generated and only dispatch
    // `runAction` once the user has confirmed that literal text — the
    // same "never run what the user has not seen" gate
    // `db_object_dialogs.cpp::previewConfirmAndRun` already gives the
    // F4.4 dialogs. `refreshScopeId` is the node whose subtree is stale
    // once the action succeeds (the row's own parent for rename/drop,
    // since those change what that parent's children list shows; the row
    // itself for truncate/comment, which change no name).
    void confirmPreviewAndRunAction(const QString &nodeId, const QString &actionId,
                                    const QString &title, const QString &refreshScopeId);

    DatabaseService *databaseService_;
    ExchangeService *exchangeService_;
    DocumentManager *documentManager_;
    OpenSettings openSettings_;

    QToolButton *addButton_ = nullptr;
    QToolButton *refreshButton_ = nullptr;
    QToolButton *forceRefreshButton_ = nullptr;
    QToolButton *goToDdlButton_ = nullptr;
    QToolButton *viewOptionsButton_ = nullptr;
    QLineEdit *filterEdit_ = nullptr;
    QLabel *statusLabel_ = nullptr;
    QTreeWidget *tree_ = nullptr;

    // View options menu state (FY.3) — `db_core::tree::FlattenOptions`'
    // own knobs, mirrored here only so a menu reopen shows the same
    // checkmarks it last closed with.
    bool flatGrouping_ = false;
    bool separateRoutines_ = false;
    bool alphabeticalSort_ = false;

    // A connected source's own colour tag (`FfiDbSourceRow::color`,
    // Data Source dialog F1.6), refreshed whenever `sources()` changes —
    // `rebuildTree` paints it on column 0 of every row that source owns.
    QHash<QString, QColor> sourceColorById_;

    // Rebuilt wholesale on every `rowsChanged`, like `ContainersPanel`;
    // expansion and selection carry across by node id.
    QHash<QString, QTreeWidgetItem *> itemsById_;

    // What the context menu and Refresh/Go to DDL toolbar buttons need
    // back out of a row, cached at rebuild time — nothing here decides
    // which actions apply (`db_core::tree::actions_for` already did).
    struct RowInfo
    {
        QString label;
        QString sourceId;
        QString kind;
        int depth = 0;
        bool isSourceRoot = false;
        bool canOpenConsole = false;
        bool canEditData = false;
        bool canGoToDdl = false;
        bool canCopyName = false;
        bool canRefresh = false;
        bool canRename = false;
        bool canDrop = false;
        bool canTruncate = false;
        bool canComment = false;
        bool canErDiagram = false;
        bool canExportData = false;
        bool canImportData = false;
        bool canCopyTable = false;
        bool canDump = false;
        bool canCompare = false;
        bool canDeleteKey = false;
        bool canTtlSet = false;
        bool canCreateTable = false;
        bool canModifyTable = false;
        bool canAddColumn = false;
        bool canCreateIndex = false;
        bool canCreateUser = false;
    };
    QHash<QString, RowInfo> rowInfoById_;

    // F4.4: the node id whose scope a just-dispatched object-DDL action
    // (`objectDdlPreview`/`runObjectDdl`) affects — set right before
    // dispatch, consumed by `onActionFinished` to refresh that one node
    // on success. `runAction`'s older rename/drop/truncate/comment paths
    // are unchanged (still manual-refresh, `database-tools.md` §11's own
    // tracked debt) — this is scoped to the new dialogs only.
    QString ddlRefreshNodeId_;
};

// Builds the panel, wraps it in a dock widget and registers it with
// `docks` under id `"database"` — the same one-call pattern
// `buildContainersDock`/`buildBuildToolsDock` use.
DatabasePanel *buildDatabaseDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                ads::CDockAreaWidget *relativeTo,
                                DatabaseService *databaseService, ExchangeService *exchangeService,
                                DocumentManager *documentManager);

} // namespace ui_shell
