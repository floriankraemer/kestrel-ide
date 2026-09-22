#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// Database Tools plan F8.5: the ADBC driver install consent dialog —
// names the publisher (the pinned artifact's own download domain), its
// URL and sha256, then a progress bar while `DriverInstallService::install`
// runs on its own worker thread, then the result.
//
// Humble view: every status line, every consent field and the pass/fail
// outcome come from `DriverInstallService`; this only lays out widgets and
// forwards the click.
void showDriverInstallDialog(QWidget *parent, DriverInstallService *service,
                              const QString &driverId, const QString &driverName);

} // namespace ui_shell
