//! `FfiSqlScriptOptions` <-> `RunConfig::sql_script`, both directions — the
//! `sql-script` kind's own sub-table (database-tools-plan F3.6), the same
//! shape `container_form` gives the three container kinds.

use cxx_qt_lib::QString;

use app_config::sql_script_run::SqlScriptRunSetting;

use crate::bridge::ffi;

pub(super) fn to_ffi_options(config: &run_core::RunConfig) -> ffi::FfiSqlScriptOptions {
    let sql = config.sql_script.clone().unwrap_or_default();
    ffi::FfiSqlScriptOptions {
        source_id: QString::from(sql.source_id.as_str()),
        file: QString::from(sql.file.as_str()),
        tx_mode: QString::from(sql.tx_mode.as_str()),
        stop_on_error: sql.stop_on_error,
    }
}

/// Sets `config.sql_script` from `options` when `kind == "sql-script"`,
/// clears it otherwise — a configuration switched away from this kind
/// keeps no stale sub-table around (`container_form::apply_options`'s own
/// rule for its three kinds).
pub(super) fn apply_options(
    config: &mut run_core::RunConfig,
    kind: &str,
    options: &ffi::FfiSqlScriptOptions,
) {
    if kind != "sql-script" {
        config.sql_script = None;
        return;
    }
    config.sql_script = Some(SqlScriptRunSetting {
        source_id: options.source_id.to_string(),
        file: options.file.to_string(),
        tx_mode: options.tx_mode.to_string(),
        stop_on_error: options.stop_on_error,
    });
}
