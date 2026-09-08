#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>

class QAction;
class QMainWindow;

namespace ui_shell {

// The PHP tooling plan's B9: the "&Analysis" menu's one action, "Inspect
// Project" (`analysis.inspectProject`), wired to `AnalysisService::
// inspectProject`. Its own translation unit for the reason `build_menu.cpp`
// and `vcs_menu.cpp` are: `main_window.cpp` sits at its 1200-line ceiling
// (ADR-0025).
//
// A single action rather than a full menu of per-analyzer entries: which
// analyzers exist and whether each is enabled are Settings > Analysis's
// job (B7), and a project-wide run always runs every enabled one at once
// — there is nothing else to choose from a menu.
void buildAnalysisMenu(QMainWindow *window, AnalysisService *analysisService,
                       AppSettings *appSettings, QHash<QString, QAction *> &actions);

} // namespace ui_shell
