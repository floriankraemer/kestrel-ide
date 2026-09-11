#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <functional>

class QWidget;

namespace ui_shell {

// Settings > Tabs: air around an editor tab's label, per side.
//
// Commits on OK, like Terminal: a padding a user is halfway through typing
// is not worth applying keystroke by keystroke, and the spin boxes already
// clamp to a sane range as the user types, so there is nothing to preview.
//
// Project-scoped (ADR-0022): the page edits whichever layer the dialog's
// scope selector names, and knows nothing about that itself — it reads
// `AppSettings::tabPadding()` when it is built and writes `saveTabPadding()`
// on OK, both of which follow the scope.
struct TabPaddingPage
{
    QWidget *widget;
    std::function<bool()> commit;
};

// Humble view (ADR-0002): which side means what, the bound, and how a
// project's override interacts with the global layer are all decided
// behind `AppSettings`. This file renders four spin boxes.
TabPaddingPage buildTabPaddingPage(QWidget *parent, AppSettings *appSettings);

} // namespace ui_shell
