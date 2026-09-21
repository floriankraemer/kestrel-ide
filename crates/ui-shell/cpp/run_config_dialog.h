#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// F4-12: "Edit Configurations..." — a list of the project's run
// configurations on the left, a name/program/args/cwd/env form on the
// right, Add/Remove, and Save/Cancel.
//
// Same commit/discard convention `showSettingsDialog` uses for its pages
// (`KeymapEditor`, `LanguageServerEditor`, ...): `beginEdit()` loads the
// draft when the dialog opens, Save calls `validate()` then `commit()` (a
// validation refusal keeps the dialog open with the message shown, rather
// than closing on invalid data), Cancel calls `revert()`. Modal and
// standalone rather than a Settings page, since editing run configurations
// is an action reached from the Run menu, not a persistent preference.
//
// `containerService` feeds the container-kind pages' Server combo
// (`ContainerService::connections()`) and Services picker (C5, ADR-0056).
// `selectConfigId`, when non-empty, selects that configuration on open
// instead of the first row — the Dockerfile/compose gutter's "New
// configuration..." and "Create Container..." (replacing C4's
// `createContainerQuick`) both add an entry first and then open the dialog
// already pointed at it.
void showRunConfigDialog(QWidget *parent, RunConfigEditor *editor,
                         ContainerService *containerService,
                         const QString &selectConfigId = QString(),
                         ConsoleService *consoleService = nullptr);

} // namespace ui_shell
