//! `RunConfig` <-> `FfiRunConfig`, both directions — split out of `mod.rs`
//! under its file-size ratchet when the `sql-script` kind (F3.6) needed a
//! fourth per-kind sub-table alongside `container_form`'s three.

use cxx_qt_lib::QString;

use super::{container_form, env_from_string, env_to_string, sql_script_form, tasks_from_string};
use super::{ffi, tasks_to_string};

pub(crate) fn to_ffi_run_config(config: &run_core::RunConfig) -> ffi::FfiRunConfig {
    ffi::FfiRunConfig {
        id: QString::from(config.id.as_str()),
        name: QString::from(config.name.as_str()),
        program: QString::from(config.program.as_str()),
        args: QString::from(config.args.join(" ").as_str()),
        cwd: QString::from(config.cwd.clone().unwrap_or_default().as_str()),
        env: QString::from(env_to_string(&config.env).as_str()),
        toolchain: QString::from(config.toolchain.clone().unwrap_or_default().as_str()),
        target: QString::from(config.target.clone().unwrap_or_default().as_str()),
        temporary: config.temporary,
        allow_parallel: config.allow_parallel,
        before_launch: QString::from(tasks_to_string(config).as_str()),
        kind: QString::from(config.kind.clone().unwrap_or_default().as_str()),
        container: container_form::to_ffi_options(config),
        sql_script: sql_script_form::to_ffi_options(config),
        run_on: QString::from(config.run_on.clone().unwrap_or_default().as_str()),
    }
}

/// The inverse of [`to_ffi_run_config`] — a fresh [`run_core::RunConfig`]
/// from a form the caller built rather than one drawn from the draft, the
/// same shape `RunConfigEditor::command_preview`'s own "scratch" config
/// uses. `BuildToolsService::runTemporary` (the jvm-build-tools plan's B1)
/// is this function's only caller: it already has a full `FfiRunConfig`
/// from `BuildToolsService::taskConfig` and needs it back as a
/// `RunConfig` to launch.
pub(crate) fn from_ffi_run_config(form: &ffi::FfiRunConfig) -> run_core::RunConfig {
    let mut config = run_core::RunConfig {
        id: form.id.to_string(),
        name: form.name.to_string(),
        program: form.program.to_string(),
        args: form
            .args
            .to_string()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        toolchain: (!form.toolchain.to_string().is_empty()).then(|| form.toolchain.to_string()),
        target: (!form.target.to_string().is_empty()).then(|| form.target.to_string()),
        temporary: true,
        allow_parallel: form.allow_parallel,
        before_launch: tasks_from_string(&form.before_launch.to_string()),
        kind: (!form.kind.to_string().is_empty()).then(|| form.kind.to_string()),
        ..run_core::RunConfig::default()
    };
    let cwd = form.cwd.to_string();
    config.cwd = if cwd.trim().is_empty() {
        None
    } else {
        Some(cwd)
    };
    config.env = env_from_string(&form.env.to_string());
    container_form::apply_options(&mut config, &form.kind.to_string(), &form.container);
    sql_script_form::apply_options(&mut config, &form.kind.to_string(), &form.sql_script);
    let run_on = form.run_on.to_string();
    config.run_on = (!run_on.trim().is_empty()).then_some(run_on);
    config
}
