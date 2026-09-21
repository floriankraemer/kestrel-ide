#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// Database Tools plan F1.6: the Add/Edit Data Source dialog — General,
// SSH, SSL and Options tabs, a Test button, and the fields
// `DataSourceEditor` already validates. `id` empty means "Add" (a fresh
// draft in `scope`, `"global"` or `"project"`); otherwise "Edit" reopens
// that source from whichever layer it actually lives in.
//
// Humble view: every field's meaning and every validation sentence come
// from `DataSourceEditor`/`settings_model::database`; this only lays out
// widgets, forwards edits, and calls `commit()` on OK.
// `driverInstallService` drives F8.5's status line/Install button for an
// `adbc`-backend driver row (status/consent/install/re-enable); `odbc`-backend
// rows instead show a hint to use the URL field as a DSN or connection
// string, since `db-driver-odbc` already reads it that way
// (`db_driver_odbc::connect::build_connection_string`).
void showDataSourceDialog(QWidget *parent, AppSettings *appSettings, DataSourceEditor *editor,
                           DriverInstallService *driverInstallService, const QString &id,
                           const QString &scope);

} // namespace ui_shell
