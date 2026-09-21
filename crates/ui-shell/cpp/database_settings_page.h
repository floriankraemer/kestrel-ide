#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ui_shell {

// Settings > Database (Database Tools plan F1.6): the data-source list —
// Add/Edit/Duplicate/Remove, each row showing its name, driver and which
// layer (global/project) it lives in. Add/Edit open `showDataSourceDialog`;
// Duplicate/Remove act immediately, the same "no separate OK to press"
// shape the Registries list already uses for its own row actions.
//
// F8.5: a "Drivers..." button opens a list of every `adbc`-backend row with
// its install status and Install/Re-enable action, plus the
// `allow_third_party_drivers` checkbox (ADR-0061 §4) — both driven by
// `driverInstallService`/`appSettings`, nothing decided here.
//
// Humble view: every row's data and every mutation come from `AppSettings`/
// `DataSourceEditor`/`DriverInstallService`; this only lays out the list and
// forwards clicks.
QWidget *buildDatabaseSettingsPage(QWidget *parent, AppSettings *appSettings,
                                    DataSourceEditor *editor,
                                    DriverInstallService *driverInstallService);

} // namespace ui_shell
