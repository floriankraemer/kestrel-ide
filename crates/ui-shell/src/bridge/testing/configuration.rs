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

use super::{MarkerSelection, RerunSelection, RunRequest, TreeRerun};
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
    // File macros stand for the test file the scope names (#458).
    let project = run_core::MacroContext::for_project(root);
    let context = match php_core::phpunit_run::macro_file(&setting) {
        Some(file) => run_core::MacroContext::for_file(
            root,
            root.join(run_core::macros::expand_cwd(&file, &project)),
        ),
        None => project,
    };
    let plan = php_core::phpunit_run::plan(
        &setting,
        |text| run_core::macros::expand_resolved(text, &context),
        &root.to_string_lossy(),
        cfg!(windows),
    )
    .and_then(|plan| php_core::phpunit_run::check_working_directory(&plan, root).map(|()| plan))
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

/// The temporary configuration a gutter or caret run of `selection` stands
/// for, unscoped to its package.
fn marker_config(selection: &MarkerSelection, root: &Path) -> Result<run_core::RunConfig, String> {
    php_core::phpunit_run::marker_config(
        &selection.marker,
        relative(&selection.file, root).as_deref(),
    )
    .map_err(|error| format!("This test cannot be run on its own: it {error}."))
}

/// The temporary configuration that reruns the data-set row `node` of
/// `file`; `None` when `node` is not a data-set row.
fn data_set_config(
    node: &test_core::TestNode,
    file: Option<&Path>,
    root: &Path,
) -> Option<run_core::RunConfig> {
    let symbol = node.location.as_ref()?.symbol.as_ref()?;
    let relative = file.and_then(|file| relative(&file.to_string_lossy(), root));
    php_core::phpunit_run::data_set_config(symbol, &node.name, relative.as_deref())
}

/// Persist `config` as a temporary configuration (the cap evicts older
/// ones); a saved or already-known id is left untouched, except that a
/// temporary one still named as an earlier version named it is renamed.
/// The error is why the project settings could not be written.
pub(super) fn remember(config: &run_core::RunConfig, root: &Path) -> Result<(), String> {
    let config = config.clone();
    app_config::project_settings::update(root, move |settings| {
        let mut configs = settings.run_configs.clone().unwrap_or_default();
        php_core::phpunit_run::forget_earlier(&mut configs, &config);
        run_core::remember_temporary(&mut configs, config.clone());
        if let Some(stored) = configs.iter_mut().find(|stored| stored.id == config.id) {
            if let Some(name) = php_core::phpunit_run::refreshed_name(stored) {
                stored.name = name;
            }
        }
        settings.run_configs = Some(configs);
    })
    .map_err(|error| error.to_string())
}

/// The settings a run in the project at `root` uses: the effective ones,
/// with a configuration's own interpreter in place of `[php]`'s. Safe off
/// the Qt thread.
pub(super) fn settings_for(root: &Path, run: Option<&ConfigRun>) -> app_config::Settings {
    let mut settings = crate::bridge::convert::load_resolved_settings_for(root);
    if let Some(interpreter) = run.and_then(|run| run.plan.interpreter.clone()) {
        settings.php.interpreter = Some(interpreter);
    }
    settings
}

/// The command line `config` runs, as the Run Configurations dialog
/// previews it: the selected framework's program, its dialect's scope
/// arguments and the interpreter prefix, spelled as the host it runs on
/// receives them, or why it cannot run. The program is looked for on this
/// machine only, never inside WSL or a container.
pub(crate) fn preview(config: &run_core::RunConfig, root: &Path) -> String {
    let argv = resolve(config, root).and_then(|run| {
        let settings = settings_for(root, Some(&run));
        let work_dir = root.join(run.plan.working_directory.as_deref().unwrap_or_default());
        let framework = super::Framework::detect(root, &work_dir, &settings, true)?;
        let (program, args) = framework.command(&settings, Some(&run), framework.scoped(&run)?);
        // What runs: on a WSL project, the Linux paths the distro gets.
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(framework
            .host
            .process_argv(&program.to_string_lossy(), &args))
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

/// The project's stored configuration for `config`, if any
/// (`php_core::phpunit_run::stored_for`): an editor run reuses the
/// temporary configuration the dialog may have edited.
pub(super) fn stored_or(config: run_core::RunConfig, root: &Path) -> run_core::RunConfig {
    let stored = app_config::project_settings::load(root)
        .ok()
        .and_then(|settings| settings.run_configs)
        .unwrap_or_default();
    php_core::phpunit_run::stored_for(&stored, config)
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
    /// What `selection` runs, from what only the Qt thread can read (the
    /// tree and the runs that filled it); the package probe and the rest of
    /// the disk work happen on the run's worker (`super::prepare`).
    pub(super) fn run_request(
        &self,
        selection: Option<RerunSelection>,
        root: &Path,
    ) -> Result<RunRequest, String> {
        // An editor run runs in the test file's monorepo package, as
        // PhpStorm runs a package-scoped configuration (#424).
        let in_package = |config: run_core::RunConfig, file: &Path| RunRequest::Editor {
            config: Box::new(config),
            dir: Some(file.parent().unwrap_or(root).to_path_buf()),
        };
        Ok(match selection {
            None => RunRequest::AdHoc,
            Some(RerunSelection::Marker(marker)) => {
                let config = marker_config(&marker, root)?;
                in_package(config, Path::new(&marker.file))
            }
            Some(RerunSelection::DataSet(id)) => {
                let file = self
                    .located(&QString::from(id.as_str()))
                    .map(|(location, host, work_dir)| location.local_path(&host, &work_dir));
                let config = self
                    .tree
                    .borrow()
                    .node(&id)
                    .and_then(|node| data_set_config(node, file.as_deref(), root));
                match (config, file) {
                    (Some(config), Some(file)) => in_package(config, &file),
                    (Some(config), None) => RunRequest::Editor {
                        config: Box::new(config),
                        dir: None,
                    },
                    (None, _) => self.rerun_request(TreeRerun::Node(id))?,
                }
            }
            Some(RerunSelection::Failed(ids)) => self.rerun_request(TreeRerun::Failed(ids))?,
            Some(RerunSelection::Node(id)) => self.rerun_request(TreeRerun::Node(id))?,
            Some(RerunSelection::Configuration(config)) => RunRequest::Configuration(config),
        })
    }

    /// A Tests dock rerun, run with the configuration of the runs that
    /// reported its tests (`php_core::phpunit_run::rerun_config`).
    fn rerun_request(&self, rerun: TreeRerun) -> Result<RunRequest, String> {
        let ids = match &rerun {
            TreeRerun::Failed(ids) => ids.clone(),
            TreeRerun::Node(id) => vec![id.clone()],
        };
        let runs = self.tree.borrow().runs_of(&ids);
        let configs = self.run_configs.borrow();
        let producers: Vec<Option<run_core::RunConfig>> = runs
            .iter()
            .map(|run| configs.get(run).cloned().flatten())
            .collect();
        let config =
            php_core::phpunit_run::rerun_config(&producers).map_err(|error| error.to_string())?;
        Ok(RunRequest::Rerun {
            config: config.map(Box::new),
            rerun,
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
        let restart = !config.allow_parallel && self.is_running();
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
