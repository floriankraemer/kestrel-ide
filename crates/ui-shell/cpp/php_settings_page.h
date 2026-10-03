#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ui_shell {

// Settings > PHP (PHP parity plan, I7): the interpreter (and the container
// it may run in), its probed version and Xdebug state, the language level,
// include paths, stubs, the Intelephense licence key and which language
// servers run.
//
// Humble view (ADR-0002): every field is read from and written to
// `PhpSettingsEditor`; `settings_model::php::PhpForm` decides what a blank
// or malformed value means, and `php_core::probe` what the interpreter
// reports. This page only lays the fields out and words the probe result.
QWidget *buildPhpSettingsPage(QWidget *parent, PhpSettingsEditor *editor,
                              RunConfigEditor *runConfigEditor);

} // namespace ui_shell
