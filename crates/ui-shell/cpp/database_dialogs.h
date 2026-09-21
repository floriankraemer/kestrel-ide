#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// The `Ask` script policy's confirmation dialog (database-tools-plan F3e):
// a failing statement in a `StopOnError`/`Ask`-policy script stops here
// with Continue/Stop, answered through `ConsoleService::resume`. A
// separate file/humble free function, not a method on
// `DatabaseResultsPanel`, so a dialog never grows into a class of its own
// (`cpp/`'s own "no `if` that encodes a business decision" rule stays
// trivially true: this asks a question and forwards the answer, nothing
// else).
void showAskContinueDialog(ConsoleService *consoleService, quint64 resultId,
                           const QString &message, QWidget *parent);

} // namespace ui_shell
