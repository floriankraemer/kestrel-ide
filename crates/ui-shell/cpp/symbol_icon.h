#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QIcon>

namespace ui_shell {

// A small colored glyph for a symbol's kind or its Structure category
// (Task 4c) — same alpha-mask-plus-tint mechanism `theme.cpp`'s
// `tabCloseIcon()` uses, except the tint is a fixed per-kind/per-category
// color rather than one looked up from the active theme: unlike a chrome
// icon that must blend into whichever theme is active, a symbol-kind
// glyph's color *is* the thing that tells kinds apart (JetBrains/VS Code
// convention), so it stays constant across a light/dark switch. The
// palette was chosen to read clearly on both a light and a dark dock
// background; see symbol_icon.cpp for the concrete values.
//
// Each icon is built once (the color never changes) and cached for the
// life of the process.
QIcon symbolKindIcon(FfiSymbolKind kind);
QIcon symbolCategoryIcon(FfiSymbolCategory category);

} // namespace ui_shell
