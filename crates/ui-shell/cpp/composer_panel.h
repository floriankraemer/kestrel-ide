#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;

// The Composer dock (PHP parity plan, I8): the project's scripts and
// packages from `ComposerService::rows()`, and a toolbar and context menu
// whose actions run in the Run console. Humble view (ADR-0002): the rows,
// the argv of every action and the refusal of a malformed package name are
// `php_core::composer_view`'s; this builds a dock, paints rows and forwards
// an action's wire name and argument to `ComposerService::actionConfig`,
// then launches the answer through `RunService::runTemporary`.
//
// Q_OBJECT-free (everything is a lambda over the widgets), so no header
// needs moc registration.
QWidget *buildComposerDock(ads::CDockManager *dockManager, DockRegistry *docks,
                           ads::CDockAreaWidget *relativeTo, ComposerService *composerService,
                           RunService *runService, ProjectTreeModel *treeModel);

} // namespace ui_shell
