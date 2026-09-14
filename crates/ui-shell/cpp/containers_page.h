#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <functional>

class QWidget;

namespace ui_shell {

// Settings > Containers (C1, ADR-0055): named Docker/Podman connections,
// plus the two dock filters and the SELinux relabel toggle.
//
// Commits on OK, like Terminal and Tabs: a connection is edited as a draft
// list in this page and only written back through
// `AppSettings::saveContainerConnections`/`saveContainerSettings` when the
// dialog is accepted — there is no live effect to preview for a connection
// that has not been tested yet.
//
// Project-scoped (ADR-0022): the page edits whichever layer the dialog's
// scope selector names, and knows nothing about that itself — it reads
// `AppSettings::containerConnections()`/`containerSettings()` when it is
// built and writes both back on OK, both of which follow the scope.
struct ContainersPage
{
    QWidget *widget;
    std::function<void()> commit;
};

// Humble view (ADR-0002): what a connection kind's fields mean, how a
// connection turns into a runnable command, and what "Test connection"'s
// success/failure text says are all decided behind `container_core`
// (through `AppSettings`). This file renders a connection list, a
// per-kind form, and the two dock-filter checkboxes.
// Run targets (C8): `runConfigEditor`/`containerService` feed the Run
// targets section's New Target wizard (Server combo, Services picker,
// command preview) — the same two collaborators the run-config dialog
// already threads to it.
ContainersPage buildContainersPage(QWidget *parent, AppSettings *appSettings,
                                   RunConfigEditor *runConfigEditor,
                                   ContainerService *containerService);

} // namespace ui_shell
