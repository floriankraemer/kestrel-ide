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
// Humble view: every row's data and every mutation come from `AppSettings`/
// `DataSourceEditor`; this only lays out the list and forwards clicks.
QWidget *buildDatabaseSettingsPage(QWidget *parent, AppSettings *appSettings,
                                    DataSourceEditor *editor);

} // namespace ui_shell
