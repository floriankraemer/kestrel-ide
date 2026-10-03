//! `FfiPhpServerOptions` <-> `RunConfig::php_server`, both directions — the
//! `php-builtin-server` kind's own sub-table (PHP parity plan, I6), the
//! same shape as `sql_script_form`.

use cxx_qt_lib::QString;

use app_config::php::PhpBuiltinServerRunSetting;
use run_core::php_run::KIND_BUILTIN_SERVER;

use crate::bridge::ffi;

pub(super) fn to_ffi_options(config: &run_core::RunConfig) -> ffi::FfiPhpServerOptions {
    let server = config.php_server.clone().unwrap_or_default();
    ffi::FfiPhpServerOptions {
        host: QString::from(server.host.as_str()),
        port: u32::from(server.port),
        document_root: QString::from(server.document_root.as_str()),
        router: QString::from(server.router.as_str()),
    }
}

/// Sets `config.php_server` from `options` when `kind` is
/// `php-builtin-server`, clears it otherwise — a configuration switched
/// away from this kind keeps no stale sub-table.
pub(super) fn apply_options(
    config: &mut run_core::RunConfig,
    kind: &str,
    options: &ffi::FfiPhpServerOptions,
) {
    if kind != KIND_BUILTIN_SERVER {
        config.php_server = None;
        return;
    }
    config.php_server = Some(PhpBuiltinServerRunSetting {
        host: options.host.to_string(),
        // A spin box cannot exceed u16; 0 stays "default".
        port: u16::try_from(options.port).unwrap_or_default(),
        document_root: options.document_root.to_string(),
        router: options.router.to_string(),
    });
}
