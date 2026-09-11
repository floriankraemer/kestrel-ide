#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

namespace ui_shell {

// The user-facing "class"/"method"/... word for a symbol kind, spelled once.
//
// Shared by the Structure dock, Search Everywhere and the declaration
// navigator, which all label a symbol row the same way. It used to be spelled
// twice — a QStringLiteral copy beside the Structure dock and a tr() copy on
// DeclarationNavigator — so the same seven words were translatable in one
// half of the UI and not the other.
//
// An out-of-range kind yields an empty string rather than a plausible word:
// FfiSymbolKind is a closed FFI enum, so a value outside it is a bug on the
// Rust side, and labelling it "field" would hide that.
QString symbolKindLabel(FfiSymbolKind kind);

// The Structure group heading for a symbol category (Task 4b) — "Fields",
// "Methods", etc. Same closed-enum convention as symbolKindLabel: an
// out-of-range category yields an empty string rather than a plausible
// word.
QString symbolCategoryLabel(FfiSymbolCategory category);

} // namespace ui_shell
