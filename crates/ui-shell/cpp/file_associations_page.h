#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ui_shell {

// Settings > File Associations: which handler (text, image,
// binary, ...) a file pattern opens with.
//
// Live-effect like `plugins_page.cpp`/`languages_page.cpp` (ADR-0002): no
// draft, no OK-shaped promise — every row added, removed or edited through
// the table writes straight through `FileAssociationsEditor`. What a
// pattern matches, what a handler name means, and what the shipped defaults
// are, are all `settings_model::file_associations`'s answers; this file
// only lays out two tables (global, and — when a project is open — its
// override) and an Add/Remove strip under each.
QWidget *buildFileAssociationsPage(QWidget *parent, FileAssociationsEditor *editor);

} // namespace ui_shell
