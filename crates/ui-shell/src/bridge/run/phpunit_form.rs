//! `FfiPhpUnitOptions` <-> `RunConfig::phpunit`, both directions — the
//! `phpunit` kind's own sub-table (PHPUnit editor runs plan, E4), the same
//! shape as `php_server_form`.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use app_config::php::{PhpUnitRunSetting, PHPUNIT_KIND};

use crate::bridge::ffi;

fn join(words: &[String]) -> QString {
    QString::from(words.join(", ").as_str())
}

fn split(text: &QString) -> Vec<String> {
    text.to_string()
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn to_ffi_options(config: &run_core::RunConfig) -> ffi::FfiPhpUnitOptions {
    let setting = config.phpunit.clone().unwrap_or_default();
    ffi::FfiPhpUnitOptions {
        scope: QString::from(setting.scope.as_str()),
        path: QString::from(setting.path.as_str()),
        class_name: QString::from(setting.class.as_str()),
        method: QString::from(setting.method.as_str()),
        data_set: QString::from(setting.data_set.as_str()),
        filter: QString::from(setting.filter.as_str()),
        testsuite: QString::from(setting.testsuite.as_str()),
        configuration: QString::from(setting.configuration.as_str()),
        groups: join(&setting.groups),
        exclude_groups: join(&setting.exclude_groups),
        runner_options: QString::from(setting.runner_options.as_str()),
        interpreter: QString::from(setting.interpreter.as_str()),
    }
}

/// Sets `config.phpunit` from `options` when `kind` is `phpunit`, clears it
/// otherwise — a configuration switched away from this kind keeps no stale
/// sub-table.
pub(super) fn apply_options(
    config: &mut run_core::RunConfig,
    kind: &str,
    options: &ffi::FfiPhpUnitOptions,
) {
    if kind != PHPUNIT_KIND {
        config.phpunit = None;
        return;
    }
    config.phpunit = Some(PhpUnitRunSetting {
        scope: options.scope.to_string(),
        path: options.path.to_string(),
        class: options.class_name.to_string(),
        method: options.method.to_string(),
        data_set: options.data_set.to_string(),
        filter: options.filter.to_string(),
        testsuite: options.testsuite.to_string(),
        configuration: options.configuration.to_string(),
        groups: split(&options.groups),
        exclude_groups: split(&options.exclude_groups),
        runner_options: options.runner_options.to_string(),
        interpreter: options.interpreter.to_string(),
    });
}

/// The configuration's table, if it is a `phpunit` one.
fn setting_of(config: &run_core::RunConfig) -> Option<&PhpUnitRunSetting> {
    (config.kind.as_deref() == Some(PHPUNIT_KIND))
        .then_some(config.phpunit.as_ref())
        .flatten()
}

/// The editor's refusal to save a `phpunit` configuration, worded to follow
/// its quoted name.
pub(super) fn problem(config: &run_core::RunConfig) -> Option<String> {
    php_core::phpunit_run::problem(setting_of(config)?)
}

/// The command line the Run Configurations dialog previews: the PHPUnit
/// dialect's argv, or the validation problem. `None` for any other kind.
pub(super) fn preview(config: &run_core::RunConfig) -> Option<String> {
    let setting = setting_of(config)?;
    let root = super::current_project_root().unwrap_or_default();
    let context = run_core::MacroContext::for_project(&root);
    let line = php_core::phpunit_run::plan(
        setting,
        |text| run_core::macros::expand(text, &context),
        &root.to_string_lossy(),
        cfg!(windows),
    )
    .map_err(|error| error.to_string())
    .and_then(|plan| {
        test_core::scope::scope_args(
            &["--teamcity".to_string()],
            test_core::filter::FilterDialect::PhpUnitRegex,
            Some("--filter"),
            None,
            &plan.scope,
            &plan.options,
        )
        .map_err(|error| error.to_string())
    });
    Some(match line {
        Ok(args) => {
            let mut argv = vec!["vendor/bin/phpunit".to_string()];
            argv.extend(args);
            container_core::run_config::preview(&argv)
        }
        Err(message) => message,
    })
}

/// Read the PHPUnit configuration `configuration` names (blank: the
/// project's own) and scan its suites for groups on a worker thread,
/// answering through `phpUnitModelReady`. The scan walks the filesystem, so
/// it never runs on the Qt thread.
pub(super) fn request_model(editor: Pin<&mut ffi::RunConfigEditor>, configuration: &QString) {
    let configuration = configuration.to_string();
    let root = super::current_project_root();
    let qt_thread = editor.qt_thread();
    std::thread::spawn(move || {
        let model = root.map(|root| {
            let context = run_core::MacroContext::for_project(&root);
            let explicit = run_core::macros::expand(&configuration, &context);
            php_core::phpunit_xml::model(&root, &explicit)
        });
        let model = model.unwrap_or_default();
        let lines = |list: &[String]| QString::from(list.join("\n").as_str());
        let answer = ffi::FfiPhpUnitModel {
            configuration: QString::from(configuration.as_str()),
            file: QString::from(model.file.unwrap_or_default().as_str()),
            suites: lines(&model.suites),
            groups: lines(&model.groups),
            problem: QString::from(model.problem.unwrap_or_default().as_str()),
        };
        let _ = qt_thread.queue(move |mut editor: Pin<&mut ffi::RunConfigEditor>| {
            editor.as_mut().php_unit_model_ready(answer);
        });
    });
}
