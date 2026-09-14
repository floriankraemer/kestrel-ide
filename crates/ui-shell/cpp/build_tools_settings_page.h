#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ui_shell {

// Settings > Build Tools (the jvm-build-tools plan's B5): a Gradle group
// box and a Maven group box, each covering every field of its own
// `[build_tools.gradle]`/`[build_tools.maven]` sub-table. Global only —
// see `bridge::build_tools`'s own module doc for why trusted roots are not
// shown here.
//
// Humble view: every field's meaning and every validation sentence come
// from `BuildToolsEditor`/`settings_model::build_tools`; this only lays
// out widgets and forwards edits.
QWidget *buildBuildToolsSettingsPage(QWidget *parent, BuildToolsEditor *editor);

} // namespace ui_shell
