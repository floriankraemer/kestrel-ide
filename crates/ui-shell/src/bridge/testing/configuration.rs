//! `phpunit` run configurations on `TestService` (PHPUnit editor runs plan,
//! E4, ADR-0073): loading one, resolving it into a scope, and the
//! temporary configurations editor runs create.
//!
//! Translation only: the table's rules are `php_core::phpunit_run`'s, the
//! argv is `test_core::scope`'s, and the run itself is the ordinary
//! `TestService::start` path, so the Tests dock, coverage, the interpreter
//! host and Xdebug staging all behave as they do for Run All.

use std::path::Path;
use std::pin::Pin;

use cxx_qt_lib::QString;

use super::{MarkerSelection, RerunSelection};
use crate::bridge::errors;
use crate::bridge::ffi;

/// A `phpunit` configuration resolved for a run in the project root.
pub(super) struct ConfigRun {
    pub plan: php_core::phpunit_run::Plan,
    /// The configuration's environment, macros expanded.
    pub env: Vec<(String, String)>,
}

pub(super) fn is_phpunit(config: &run_core::RunConfig) -> bool {
    config.kind.as_deref() == Some(app_config::php::PHPUNIT_KIND)
}

/// Validate and resolve `config`; the error is a sentence for the user.
pub(super) fn resolve(config: &run_core::RunConfig, root: &Path) -> Result<ConfigRun, String> {
    // A `phpunit` kind with no table runs the configuration file's scope.
    let setting = php_core::phpunit_run::setting_of(config)
        .ok_or_else(|| format!("\"{}\" is not a PHPUnit configuration", config.name))?;
    let context = run_core::MacroContext::for_project(root);
    let plan = php_core::phpunit_run::plan(
        &setting,
        |text| run_core::macros::expand(text, &context),
        &root.to_string_lossy(),
        cfg!(windows),
    )
    .map_err(|error| format!("\"{}\" {error}", config.name))?;
    let env = config
        .env
        .iter()
        .map(|(key, value)| (key.clone(), run_core::macros::expand(value, &context)))
        .collect();
    Ok(ConfigRun { plan, env })
}

/// `file` (as the editor spells it) relative to the project root.
fn relative(file: &str, root: &Path) -> Option<String> {
    test_core::filter::relative_to_root(file, &root.to_string_lossy(), cfg!(windows))
}

/// The temporary configuration a gutter or caret run of `selection` stands for.
pub(super) fn marker_config(
    selection: &MarkerSelection,
    root: &Path,
) -> Result<run_core::RunConfig, String> {
    php_core::phpunit_run::marker_config(
        &selection.marker,
        relative(&selection.file, root).as_deref(),
    )
    .map_err(|error| format!("This test cannot be run on its own: it {error}."))
}

/// The temporary configuration that reruns the data-set row `id` of `file`;
/// `None` when `id` is not a data-set row.
pub(super) fn data_set_config(
    id: &str,
    file: Option<&Path>,
    root: &Path,
) -> Option<run_core::RunConfig> {
    let relative = file.and_then(|file| relative(&file.to_string_lossy(), root));
    php_core::phpunit_run::data_set_config(id, relative.as_deref())
}

/// Persist `config` as a temporary configuration (the cap evicts older
/// ones); a saved or already-known id is left untouched.
pub(super) fn remember(config: &run_core::RunConfig, root: &Path) -> bool {
    let config = config.clone();
    app_config::project_settings::update(root, move |settings| {
        let mut configs = settings.run_configs.clone().unwrap_or_default();
        run_core::remember_temporary(&mut configs, config.clone());
        settings.run_configs = Some(configs);
    })
    .is_ok()
}

/// Discards a run's results: a before-launch test run only needs its exit.
struct Discard;

impl test_core::TestSink for Discard {
    fn output(&mut self, _text: &str) {}
    fn event(&mut self, _event: test_core::TeamCityEvent) {}
    fn junit(&mut self, _cases: Vec<test_core::JUnitTestCase>) {}
}

/// Run `config` to completion on the calling thread, for a before-launch
/// task: `Ok` only when the framework exits 0. The scope, interpreter and
/// host are the ordinary run's.
pub(crate) fn run_blocking(config: &run_core::RunConfig, root: &Path) -> Result<(), String> {
    let run = resolve(config, root)?;
    let mut settings = crate::bridge::convert::load_resolved_settings();
    if let Some(interpreter) = run.plan.interpreter.clone() {
        settings.php.interpreter = Some(interpreter);
    }
    let (plugin, framework, program, host) = super::detect_framework(root, &settings)
        .ok_or("no installed test framework is contributed for this project")?;
    let asset_dir = plugin
        .asset_dir(&app_core::resolve_config_dir())
        .map_err(|_| "could not materialise this test framework's plugin assets")?;
    let dialect = test_core::filter::parse_filter_dialect(framework.filter_dialect.as_deref())
        .map_err(|error| error.to_string())?;
    let args = test_core::scope::scope_args(
        &plugin_host::expand_asset_dir(&framework.args, &asset_dir),
        dialect,
        framework.filter_flag.as_deref(),
        framework.filter_template.as_deref(),
        &run.plan.scope,
        &run.plan.options,
    )
    .map_err(|error| error.to_string())?;
    let (program, args) = match framework.requires_interpreter.as_deref() {
        Some("php") => {
            let interpreter = settings_model::php::resolve(&settings).interpreter;
            let (program, prefix) = analysis_core::php_invocation(&program, Some(&interpreter));
            (program, [prefix, args].concat())
        }
        _ => (program, args),
    };
    let format = test_core::parse_output_format(&framework.output_format)
        .map_err(|_| "this test framework's output-format is not one this build understands")?;
    let code = test_core::run_on_env(
        &host,
        &test_core::TestRunHandle::new(),
        &program.to_string_lossy(),
        &args,
        &run.env,
        root,
        format,
        framework.report_glob.as_deref(),
        &mut Discard,
    )
    .map_err(|failure| match failure {
        test_core::RunFailure::NotFound => "test framework program not found".to_string(),
        test_core::RunFailure::Io(message) => message,
    })?;
    match code {
        Some(0) => Ok(()),
        Some(code) => Err(format!("exit {code}")),
        None => Err("the run was stopped".to_string()),
    }
}

fn saved_config(id: &str) -> Result<run_core::RunConfig, ffi::FfiResult> {
    let Some(root) = super::current_project_root() else {
        return Err(errors::failure(
            errors::CODE_NO_PROJECT,
            "no project is open",
        ));
    };
    app_config::project_settings::load(&root)
        .unwrap_or_default()
        .run_configs
        .unwrap_or_default()
        .into_iter()
        .find(|config| config.id == id)
        .filter(is_phpunit)
        .ok_or_else(|| {
            errors::failure(
                errors::CODE_UNKNOWN_RUN_CONFIG,
                "unknown PHPUnit run configuration",
            )
        })
}

impl ffi::TestService {
    /// The selection with an editor run (gutter marker, caret, data set)
    /// replaced by the temporary `phpunit` configuration it stands for.
    pub(super) fn resolve_editor_run(
        &self,
        selection: Option<RerunSelection>,
        root: &Path,
    ) -> Result<Option<RerunSelection>, String> {
        Ok(match selection {
            Some(RerunSelection::Marker(marker)) => Some(RerunSelection::Configuration(Box::new(
                marker_config(&marker, root)?,
            ))),
            Some(RerunSelection::DataSet(id)) => {
                let file = self
                    .located(&QString::from(id.as_str()))
                    .map(|(location, host, root)| location.local_path(&host, &root));
                match data_set_config(id.as_str(), file.as_deref(), root) {
                    Some(config) => Some(RerunSelection::Configuration(Box::new(config))),
                    None => Some(RerunSelection::Node(id)),
                }
            }
            other => other,
        })
    }

    /// Run the saved `phpunit` configuration `config_id`.
    pub fn run_configuration(
        self: Pin<&mut Self>,
        config_id: &QString,
        with_coverage: bool,
    ) -> ffi::FfiResult {
        match saved_config(&config_id.to_string()) {
            Ok(config) => self.start(
                Some(RerunSelection::Configuration(Box::new(config))),
                Vec::new(),
                with_coverage,
            ),
            Err(refusal) => refusal,
        }
    }

    /// Stage `config_id` for debugging, like `prepare_debug_marker`.
    pub fn prepare_debug_configuration(
        self: Pin<&mut Self>,
        config_id: &QString,
    ) -> ffi::FfiResult {
        match saved_config(&config_id.to_string()) {
            Ok(config) => {
                *self.pending_debug.borrow_mut() =
                    Some(RerunSelection::Configuration(Box::new(config)));
                ffi::FfiResult::default()
            }
            Err(refusal) => refusal,
        }
    }
}
