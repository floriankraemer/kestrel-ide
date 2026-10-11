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

/// The settings a run uses: the effective ones, with a configuration's own
/// interpreter in place of `[php]`'s.
pub(super) fn settings_for(run: Option<&ConfigRun>) -> app_config::Settings {
    let mut settings = crate::bridge::convert::load_resolved_settings();
    if let Some(interpreter) = run.and_then(|run| run.plan.interpreter.clone()) {
        settings.php.interpreter = Some(interpreter);
    }
    settings
}

/// The command line `config` runs, as the Run Configurations dialog
/// previews it: the selected framework's program, its dialect's scope
/// arguments and the interpreter prefix, or why it cannot run. The program
/// is looked for on this machine only, never inside WSL or a container.
pub(crate) fn preview(config: &run_core::RunConfig, root: &Path) -> String {
    let argv = resolve(config, root).and_then(|run| {
        let settings = settings_for(Some(&run));
        let framework = super::Framework::detect(root, &settings, true)?;
        let (program, args) = framework.command(&settings, framework.scoped(&run)?);
        Ok([program.display().to_string()]
            .into_iter()
            .chain(args)
            .collect::<Vec<_>>())
    });
    match argv {
        Ok(argv) => container_core::run_config::preview(&argv),
        Err(reason) => reason,
    }
}

/// Run `config` for a before-launch task on the Tests dock's `TestService`
/// and wait for it, from a thread that is not the Qt thread (#416): the
/// results fill the Tests dock and its Stop cancels the run. `Ok` only when
/// the framework exits 0.
pub(crate) fn run_for_before_launch(config: run_core::RunConfig) -> Result<(), String> {
    let unavailable = || "the Tests dock is not available".to_string();
    let thread = crate::bridge::registry::test_service().ok_or_else(unavailable)?;
    let (waiter, finished) = std::sync::mpsc::channel();
    thread
        .queue(move |service: Pin<&mut ffi::TestService>| {
            let refused = waiter.clone();
            let result = service.start_with(
                Some(RerunSelection::Configuration(Box::new(config))),
                Vec::new(),
                false,
                Some(waiter),
            );
            if result.code != errors::CODE_OK {
                let _ = refused.send(Err(result.message.to_string()));
            }
        })
        .map_err(|_| unavailable())?;
    // Every sender is gone when the run ends without a word (the app quit).
    finished
        .recv()
        .unwrap_or_else(|_| Err("the test run ended without a result".to_string()))
}

/// The project's stored configuration with `config`'s id, if any: an editor
/// run reuses the temporary configuration the dialog may have edited.
fn stored_or(config: run_core::RunConfig, root: &Path) -> run_core::RunConfig {
    app_config::project_settings::load(root)
        .ok()
        .and_then(|settings| settings.run_configs)
        .and_then(|configs| {
            configs
                .into_iter()
                .find(|stored| stored.id == config.id && is_phpunit(stored))
        })
        .unwrap_or(config)
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
                stored_or(marker_config(&marker, root)?, root),
            ))),
            Some(RerunSelection::DataSet(id)) => {
                let file = self
                    .located(&QString::from(id.as_str()))
                    .map(|(location, host, root)| location.local_path(&host, &root));
                match data_set_config(id.as_str(), file.as_deref(), root) {
                    Some(config) => Some(RerunSelection::Configuration(Box::new(stored_or(
                        config, root,
                    )))),
                    None => Some(RerunSelection::Node(id)),
                }
            }
            other => other,
        })
    }

    /// Run the saved `phpunit` configuration `config_id`. Unless it allows
    /// parallel runs, a run in progress is stopped and this one starts when
    /// it has finished; the Tests dock shows one run, so a configuration that
    /// allows them is refused while another runs.
    pub fn run_configuration(
        self: Pin<&mut Self>,
        config_id: &QString,
        with_coverage: bool,
    ) -> ffi::FfiResult {
        let config = match saved_config(&config_id.to_string()) {
            Ok(config) => config,
            Err(refusal) => return refusal,
        };
        let restart = !config.allow_parallel && !self.runs.borrow().is_empty();
        let selection = RerunSelection::Configuration(Box::new(config));
        if restart {
            *self.queued.borrow_mut() = Some((selection, with_coverage));
            self.stop();
            return ffi::FfiResult::default();
        }
        self.start(Some(selection), Vec::new(), with_coverage)
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
