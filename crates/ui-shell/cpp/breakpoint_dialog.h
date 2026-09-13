#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// R5: "Edit Breakpoint..." — condition, hit count, log message, temporary
// and enabled, for the breakpoint at `path:line`. Opened from the gutter's
// context menu and from the Breakpoints window's own Edit button.
//
// Humble view: the fields it shows are `DebugService::breakpointAt`'s
// answer and Save is one `configureBreakpoint` call — whether a condition
// is well-formed, what "temporary" does, and how many hits the adapter
// counts before it fires are all `dap_core::breakpoints`' rules, not this
// dialog's.
void showBreakpointDialog(QWidget *parent, DebugService *debugService, const QString &path,
                           quint32 line);

} // namespace ui_shell
