#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QApplication;

namespace ui_shell {

// Installs the active UI locale's translator (plus Qt's own shipped
// qtbase_<locale>.qm for standard dialog buttons) into `app`, and inits the
// rcc-embedded translations resource first. Call once, before any widget is
// built — a language change only takes effect after a restart (ADR-0049),
// so there is nothing to redo later. "en" installs no translator at all: it
// is the tr() source text.
void installUiTranslators(AppSettings *appSettings, QApplication &app);

} // namespace ui_shell
