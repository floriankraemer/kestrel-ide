#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ui_shell {

// Settings > Project Scope (T5, ADR-0064): the per-project *Excluded*
// folders list and the global *Ignored names* list, plus a note stating
// the index's content rules.
//
// Humble view (ADR-0002): every rule — what a folder normalizes to, what an
// entry is rejected for, whether a change rescopes the open project — is a
// `ProjectTreeModel` call into `app_config`/`project_model`. This page only
// builds widgets and relays clicks.
QWidget *buildProjectScopeSettingsPage(QWidget *parent, ProjectTreeModel *treeModel);

} // namespace ui_shell
