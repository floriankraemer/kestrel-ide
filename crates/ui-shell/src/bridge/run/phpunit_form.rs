//! `FfiPhpUnitOptions` <-> `RunConfig::phpunit`, both directions — the
//! `phpunit` kind's own sub-table (PHPUnit editor runs plan, E4), the same
//! shape as `php_server_form`.

use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::time::SystemTime;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use app_config::php::{PhpUnitRunSetting, PHPUNIT_KIND};
use php_core::phpunit_run::setting_of;

use crate::bridge::errors;
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

/// The editor's refusal to save a `phpunit` configuration, worded to follow
/// its quoted name.
pub(super) fn problem(config: &run_core::RunConfig) -> Option<String> {
    let root = super::current_project_root().unwrap_or_default();
    php_core::phpunit_run::problem(&setting_of(config)?, &root.to_string_lossy(), cfg!(windows))
}

/// The command line the Run Configurations dialog previews for a `phpunit`
/// configuration (`TestService`'s rule); `None` for any other kind.
pub(super) fn preview(config: &run_core::RunConfig) -> Option<String> {
    setting_of(config)?;
    let root = super::current_project_root().unwrap_or_default();
    Some(crate::bridge::testing::configuration_preview(config, &root))
}

/// The `phpunit` page's fields `scope` reads.
pub(super) fn scope_fields(scope: &QString) -> ffi::FfiPhpUnitScopeFields {
    let fields = php_core::phpunit_run::fields_of(&scope.to_string());
    ffi::FfiPhpUnitScopeFields {
        path: fields.path,
        class_name: fields.class,
        method: fields.method,
        data_set: fields.data_set,
        filter: fields.filter,
        testsuite: fields.testsuite,
    }
}

/// When the configuration a request names last changed, so a cached answer
/// is reused only while the file is unchanged. `None` when there is none.
type Stamp = Option<(std::path::PathBuf, Option<SystemTime>)>;

/// The editor's `phpunit.xml` answers: one cached answer per configuration
/// field text while its file is unchanged, and one scan in flight at a time
/// (the page asks on every show, load and typing pause). Cleared when the
/// dialog opens, so a group added to a test file since shows up.
#[derive(Default)]
pub(super) struct ModelRequests {
    in_flight: bool,
    /// The newest request made while a scan ran.
    waiting: Option<String>,
    cache: HashMap<String, (Stamp, ffi::FfiPhpUnitModel)>,
}

impl ModelRequests {
    pub(super) fn clear(&mut self) {
        self.cache.clear();
    }
}

fn stamp(root: &Path, explicit: &str) -> Stamp {
    let file = php_core::phpunit_xml::discover(root, explicit)?;
    let modified = std::fs::metadata(&file)
        .and_then(|meta| meta.modified())
        .ok();
    Some((file, modified))
}

fn to_ffi_model(
    configuration: &str,
    model: php_core::phpunit_xml::PhpUnitModel,
) -> ffi::FfiPhpUnitModel {
    use php_core::phpunit_xml::ModelProblem;
    let lines = |list: &[String]| QString::from(list.join("\n").as_str());
    let problem = match &model.problem {
        None => ffi::FfiResult::default(),
        Some(problem @ ModelProblem::NotFound(_)) => {
            errors::failure(errors::CODE_PHPUNIT_CONFIG_NOT_FOUND, problem.to_string())
        }
        Some(problem) => {
            errors::failure(errors::CODE_PHPUNIT_CONFIG_UNREADABLE, problem.to_string())
        }
    };
    ffi::FfiPhpUnitModel {
        configuration: QString::from(configuration),
        file: QString::from(model.file.unwrap_or_default().as_str()),
        suites: lines(&model.suites),
        groups: lines(&model.groups),
        problem,
    }
}

/// Answer with the PHPUnit configuration `configuration` names (blank: the
/// project's own) and its suites' groups through `phpUnitModelReady`. A scan
/// walks the filesystem, so it runs on a worker thread, never on the Qt
/// thread.
pub(super) fn request_model(mut editor: Pin<&mut ffi::RunConfigEditor>, configuration: &QString) {
    let configuration = configuration.to_string();
    let Some(root) = super::current_project_root() else {
        let answer = to_ffi_model(
            &configuration,
            php_core::phpunit_xml::PhpUnitModel::default(),
        );
        editor.as_mut().php_unit_model_ready(answer);
        return;
    };
    let context = run_core::MacroContext::for_project(&root);
    let explicit = run_core::macros::expand(&configuration, &context);
    let stamp = stamp(&root, &explicit);
    let cached = {
        let mut requests = editor.phpunit_model.borrow_mut();
        let cached = requests
            .cache
            .get(&configuration)
            .filter(|(cached_stamp, _)| *cached_stamp == stamp)
            .map(|(_, answer)| answer.clone());
        if cached.is_none() && requests.in_flight {
            requests.waiting = Some(configuration);
            return;
        }
        requests.in_flight |= cached.is_none();
        cached
    };
    if let Some(answer) = cached {
        editor.as_mut().php_unit_model_ready(answer);
        return;
    }
    let qt_thread = editor.qt_thread();
    std::thread::spawn(move || {
        let answer = to_ffi_model(
            &configuration,
            php_core::phpunit_xml::model(&root, &explicit),
        );
        let _ = qt_thread.queue(move |mut editor: Pin<&mut ffi::RunConfigEditor>| {
            let next = {
                let mut requests = editor.phpunit_model.borrow_mut();
                requests.in_flight = false;
                requests
                    .cache
                    .insert(configuration.clone(), (stamp, answer.clone()));
                requests.waiting.take()
            };
            match next {
                Some(next) if next != configuration => {
                    request_model(editor, &QString::from(next.as_str()));
                }
                _ => editor.as_mut().php_unit_model_ready(answer),
            }
        });
    });
}
