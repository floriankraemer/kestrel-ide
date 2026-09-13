#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

#include <functional>

class QWidget;

namespace ui_shell {

// R5: Ctrl+Shift+F8 — every breakpoint in the project, its condition, hit
// count, log message and enabled/temporary flags editable in place, Remove,
// and a double-click that opens the source line.
//
// Humble view: the rows are `DebugService::allBreakpoints`'s answer and
// every edit is one `configureBreakpoint`/`toggleBreakpoint` call — which
// fields a breakpoint has and what "temporary" means are `dap_core`'s.
void showBreakpointsWindow(QWidget *parent, DebugService *debugService,
                            std::function<void(const QString &, int, int)> openAt);

} // namespace ui_shell
