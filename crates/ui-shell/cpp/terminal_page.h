#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <functional>

class QWidget;

namespace ui_shell {

// Settings > Terminal: which shell a new terminal tab spawns, where it
// starts, and what it adds to the environment.
//
// Commits on OK, like MCP and Keymap and unlike Appearance: there is
// nothing to apply live — a shell is chosen when a tab opens, and already
// running tabs keep the shell they started with, exactly as they do in
// IntelliJ. Cancel therefore needs no counterpart, which is why this
// struct carries a `commit` and no `revert`.
//
// Project-scoped (ADR-0022): the page edits whichever layer the dialog's
// scope selector names, and knows nothing about that itself — it reads
// `AppSettings::terminalSettings()` when it is built and writes
// `saveTerminalSettings()` on OK, both of which follow the scope.
struct TerminalPage
{
    QWidget *widget;
    std::function<void()> commit;
};

// Humble view (ADR-0002): which shells exist, what they are called, which
// of the fields wins and what an empty start directory means are all
// decided behind `AppSettings`. This file renders a form.
TerminalPage buildTerminalPage(QWidget *parent, AppSettings *appSettings);

} // namespace ui_shell
