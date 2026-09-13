#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QPoint>

class QWidget;

namespace ui_shell {

// F2-11: the signature-help popup driven by `(` and `,` while typing an
// argument list. R3 moved it onto `EditorPopup` — the same one hover and
// its diagnostics use — so all three share one popup's placement, sizing
// and dismissal rules instead of each chasing their own.
void showSignatureTip(QWidget *editor, const QPoint &globalPos, const FfiSignatureHelp &help);

void hideSignatureTip();

} // namespace ui_shell
