#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// New Target wizard (C8, ADR-0056's "Run targets" section): pull/use an
// existing image, build from a Dockerfile, or pick a compose service, then
// the mount/env/ports page with a live command preview
// (`RunConfigEditor::targetCommandPreview`).
//
// Returns `true` (with `target` filled in) if the user finished the wizard,
// `false` on Cancel. Persistence (`RunConfigEditor::addContainerTarget`/
// `updateContainerTarget`) is the caller's job — the run-config dialog's
// "New target..." combo entry and Settings > Containers > Run targets' Edit
// both call this, but they save to different places (a fresh id vs. an
// existing row), which this function has no opinion about.
//
// `prefill`, when its `id` is non-empty, opens the wizard already populated
// with that target's fields (Edit); an empty `id` starts from
// `FfiContainerTarget{}`'s defaults (New target...).
bool showContainerTargetWizard(QWidget *parent, ContainerService *containerService,
                               RunConfigEditor *editor, const FfiContainerTarget &prefill,
                               FfiContainerTarget &target);

} // namespace ui_shell
