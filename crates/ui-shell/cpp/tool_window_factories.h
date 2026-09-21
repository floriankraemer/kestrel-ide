#pragma once

#include "containers_panel.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <functional>

class QAction;
class QMenu;
class QWidget;

namespace ads {
class CDockAreaWidget;
class CDockManager;
class CDockWidget;
} // namespace ads

namespace ui_shell {

class BuildToolsPanel;
class DatabasePanel;
class DockRegistry;
class EditorTabs;

// A dock factory builds and registers its dock (inside `relativeTo`'s area,
// exactly as its hand-written call site always has) and returns the panel
// widget — declared once so `buildContributedToolWindows`'s table can hold
// both existing factories (`wireBuildToolsDock`, `buildContainersDock`)
// under one type despite their unrelated parameter lists: each table entry
// is a closure over whatever that one factory actually needs.
using DockFactory = std::function<QWidget *(ads::CDockAreaWidget *relativeTo)>;

// The typed panels a caller still needs after the generic loop below: the
// extra, per-dock wiring (`wireBuildToolsSettings`'s settings-button
// handler, `wireContainersProjectHook`'s project-lifecycle hook) is not
// generic and stays a direct call, which needs its panel back with a real
// type — `qobject_cast` recovers it once, here, rather than at every call
// site that needs one.
struct ContributedToolWindows
{
    BuildToolsPanel *buildTools = nullptr;
    ContainersPanel *containers = nullptr;
    DatabasePanel *database = nullptr;
};

// Replaces the literal `wireBuildToolsDock`/`buildContainersDock` calls
// `main_window.cpp` used to make with one loop over
// `AppSettings::contributedToolWindows()` (database-tools plan G1): row ->
// factory lookup in the table this function builds -> the factory's own
// `registerDock` call, keyed by the same id the row names. `rightArea`/
// `bottomArea` are the only placements today's docks actually use;
// `left`/`center` fall back to `bottomArea` until a contribution needs a
// real area of its own. An id with no factory — a wasm plugin declaring a
// tool window, which cannot render one natively yet (the plan's decision
// 11) — is logged and skipped, never a hard failure (fail-soft, ADR-0026).
ContributedToolWindows buildContributedToolWindows(
    AppSettings *appSettings, ads::CDockManager *dockManager, DockRegistry *docks,
    ads::CDockAreaWidget *rightArea, ads::CDockAreaWidget *bottomArea,
    ads::CDockWidget *editorDock, BuildToolsService *buildToolsService, RunService *runService,
    ProjectTreeModel *treeModel, EditorTabs *editorTabs, ContainerService *containerService,
    TerminalSupervisor *terminalSupervisor, ContainersPanel::OpenAt containersOpenAt,
    DatabaseService *databaseService);

// The View menu's `view.<id>` toggle actions for every contributed tool
// window, with the contributed title passed through as-is (it is plugin
// text, not a literal, so it is not `tr()`-wrapped here). Replaces the
// action creation that used to live inside `wireBuildToolsSettings`/
// `wireContainersProjectHook` — see those functions' own headers for what
// they still do.
void wireContributedToolWindowMenus(AppSettings *appSettings, QHash<QString, QAction *> &actions,
                                    DockRegistry *docks, QMenu *viewMenu);

} // namespace ui_shell
