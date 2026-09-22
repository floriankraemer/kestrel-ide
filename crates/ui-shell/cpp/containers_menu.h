#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

namespace ui_shell {

// C10: the one project-lifecycle hook the Containers feature needs —
// `ContainerService` is built (and its dock's tree populated, empty) before
// any project is open, so refreshing once a project opens is what lets a
// project's own `[containers]` override (ADR-0022) take effect without a
// manual Refresh. The `view.containers` action itself is no longer created
// here: `tool_window_factories.cpp`'s `wireContributedToolWindowMenus`
// covers every contributed tool window generically now (database-tools
// plan G1) — the dock's own toolbar and context menus (`ContainersPanel`)
// already cover every other action.
void wireContainersProjectHook(ProjectTreeModel *treeModel, ContainerService *containerService);

} // namespace ui_shell
