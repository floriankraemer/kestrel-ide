//! `RunConfigEditor` (F4-10): the dialog's draft object for the project's
//! run configurations, isomorphic to `LanguageServerEditor`
//! (`crate::bridge::settings`) — load, edit a working copy, validate, commit
//! back to `.ide/settings.toml` on save.

use std::cell::RefCell;
use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};

use super::container_form;
use super::{
    current_project_root, effective_container_settings, env_from_string, to_ffi_run_config,
};

/// Rust side of the `RunConfigEditor` QObject.
#[derive(Default)]
pub struct RunConfigEditorRust {
    draft: RefCell<Vec<run_core::RunConfig>>,
    /// What was last loaded or committed, for `revert()`.
    saved: RefCell<Vec<run_core::RunConfig>>,
}

/// An id for a freshly added configuration, unique enough within one
/// project's list without a UUID dependency: a nanosecond timestamp never
/// repeats within a single edit session, which is the only place two
/// `addConfiguration()` calls could otherwise collide.
fn generate_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("custom-{nanos}")
}

impl ffi::RunConfigEditor {
    pub fn begin_edit(&self) {
        let configs = current_project_root()
            .map(|root| {
                app_config::project_settings::load(&root)
                    .unwrap_or_default()
                    .run_configs
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        *self.saved.borrow_mut() = configs.clone();
        *self.draft.borrow_mut() = configs;
    }

    pub fn configurations(&self) -> Vec<ffi::FfiRunConfig> {
        self.draft.borrow().iter().map(to_ffi_run_config).collect()
    }

    pub fn add_configuration(&self) {
        self.draft.borrow_mut().push(run_core::RunConfig {
            id: generate_id(),
            name: "New Configuration".to_string(),
            ..run_core::RunConfig::default()
        });
    }

    pub fn remove_configuration(&self, index: u32) {
        let mut draft = self.draft.borrow_mut();
        if (index as usize) < draft.len() {
            draft.remove(index as usize);
        }
    }

    pub fn update_configuration(&self, index: u32, form: &ffi::FfiRunConfig) {
        let mut draft = self.draft.borrow_mut();
        let Some(config) = draft.get_mut(index as usize) else {
            return;
        };
        config.name = form.name.to_string();
        config.program = form.program.to_string();
        config.args = form
            .args
            .to_string()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let cwd = form.cwd.to_string();
        config.cwd = if cwd.trim().is_empty() {
            None
        } else {
            Some(cwd)
        };
        config.env = env_from_string(&form.env.to_string());
        config.allow_parallel = form.allow_parallel;
        config.before_launch = super::tasks_from_string(&form.before_launch.to_string());
        container_form::apply_options(config, &form.kind.to_string(), &form.container);
        super::sql_script_form::apply_options(config, &form.kind.to_string(), &form.sql_script);
        let run_on = form.run_on.to_string();
        config.run_on = (!run_on.trim().is_empty()).then_some(run_on);
        // Editing a temporary configuration is how IntelliJ's "Save
        // configuration" works: once it has been through the dialog it is
        // one the user meant to keep, so it stops being eviction fodder.
        config.temporary = false;
    }

    /// Add a configuration of `kind` (`"container-image"`/`"containerfile"`/
    /// `"compose"`) prefilled with `options`, for the run-config dialog's
    /// Add ▸ Containers submenu and "Create Container..." (replacing C4's
    /// `createContainerQuick`, ADR-0056 §6). Returns the new entry's index.
    pub fn add_container_configuration(
        &self,
        name: &QString,
        kind: &QString,
        options: &ffi::FfiContainerOptions,
    ) -> u32 {
        let mut config = run_core::RunConfig {
            id: generate_id(),
            name: name.to_string(),
            ..run_core::RunConfig::default()
        };
        container_form::apply_options(&mut config, &kind.to_string(), options);
        let mut draft = self.draft.borrow_mut();
        draft.push(config);
        (draft.len() - 1) as u32
    }

    /// The shell-quoted command `form` would run — the container-kind
    /// pages' live "Command preview" (C5, ADR-0056). Built from a scratch
    /// configuration rather than the draft entry at `index`, so it reflects
    /// the form as the user is still editing it, before Apply/OK commits
    /// anything.
    pub fn command_preview(&self, form: &ffi::FfiRunConfig) -> QString {
        let mut scratch = run_core::RunConfig {
            name: form.name.to_string(),
            program: form.program.to_string(),
            args: form
                .args
                .to_string()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            ..run_core::RunConfig::default()
        };
        container_form::apply_options(&mut scratch, &form.kind.to_string(), &form.container);

        let root = current_project_root().unwrap_or_default();
        let context = run_core::MacroContext::for_project(&root)
            .with_containers(effective_container_settings());
        let spec = {
            use run_core::RunConfigExt as _;
            scratch.to_launch_spec_in(&context)
        };
        let mut argv = vec![spec.program];
        argv.extend(spec.args);
        QString::from(container_core::run_config::preview(&argv).as_str())
    }

    /// The run targets the "Run on" combo lists (C8), effective settings
    /// (global with the project's override applied, same as every other
    /// container-settings read here) — read-only, edited through
    /// `AppSettings::containerTargets`/`saveContainerTargets`
    /// (`bridge/containers/settings.rs`) and the New Target wizard, never
    /// through this draft.
    pub fn container_targets(&self) -> Vec<ffi::FfiContainerTarget> {
        effective_container_settings()
            .targets
            .iter()
            .map(super::to_ffi_target)
            .collect()
    }

    /// The command the New Target wizard's live preview shows: `target`
    /// wrapped around a stand-in `echo hello` launch — never the actual
    /// configuration being edited, since the wizard runs before any
    /// configuration references the target at all.
    pub fn target_command_preview(&self, target: &ffi::FfiContainerTarget) -> QString {
        let containers = effective_container_settings();
        let setting = super::from_ffi_target(target);
        let root = current_project_root().unwrap_or_default();
        let invocation = containers
            .connections
            .iter()
            .find(|c| c.id == setting.connection_id)
            .map(container_core::connection::ConnectionConfig::from_setting)
            .unwrap_or(container_core::connection::ConnectionConfig {
                engine: container_core::connection::Engine::Docker,
                kind: container_core::connection::ConnectionKind::Auto,
                executable: None,
                compose_executable: None,
            })
            .invocation();
        let sample = container_core::target::SimpleLaunch {
            program: "echo",
            args: &["hello".to_string()],
            cwd: Some(root.as_path()),
            env: &[],
        };
        match container_core::target::wrap_launch(
            &sample,
            &setting,
            &invocation,
            &root,
            containers.selinux_relabel,
        ) {
            Ok(wrapped) => {
                let mut argv = vec![wrapped.program];
                argv.extend(wrapped.args);
                QString::from(container_core::run_config::preview(&argv).as_str())
            }
            Err(err) => QString::from(err.to_string().as_str()),
        }
    }

    /// Whether `service` needs a before-launch build (C8): worker-thread
    /// `compose config --format json` against `connection_id`, reported
    /// through `composeNeedsBuildReady` — same shape as
    /// `requestComposeServices`, the New Target wizard's page 2 for a
    /// compose-service source.
    pub fn request_compose_needs_build(
        self: Pin<&mut ffi::RunConfigEditor>,
        connection_id: &QString,
        files: &QString,
        service: &QString,
    ) {
        let Some(root) = current_project_root() else {
            let qt_thread = self.qt_thread();
            let _ = qt_thread.queue(|mut editor: Pin<&mut ffi::RunConfigEditor>| {
                editor.as_mut().compose_needs_build_ready(false);
            });
            return;
        };
        let containers = effective_container_settings();
        let connection_id = connection_id.to_string();
        let service = service.to_string();
        let files: Vec<String> = files
            .to_string()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let needs_build = containers
                .connections
                .iter()
                .find(|c| c.id == connection_id)
                .map(container_core::connection::ConnectionConfig::from_setting)
                .map(|c| c.invocation())
                .and_then(|invocation| {
                    container_core::run_config::compose_service_needs_build(
                        &invocation,
                        &files,
                        &service,
                        &root,
                    )
                    .ok()
                })
                .unwrap_or(false);
            let _ = qt_thread.queue(move |mut editor: Pin<&mut ffi::RunConfigEditor>| {
                editor.as_mut().compose_needs_build_ready(needs_build);
            });
        });
    }

    /// New Target wizard's Finish (C8): appends `target` to the *project's*
    /// `[containers].targets` (project scope always — the dialog is always
    /// opened against an open project, and a project override already wins
    /// over a global row of the same id, so there is nothing a global write
    /// here would buy over this simpler one) and returns the new id, freshly
    /// generated when `target.id` arrives blank. Immediate, not part of this
    /// editor's own draft/commit cycle: a target is a `[containers]` row,
    /// not a run configuration, the same reason `AppSettings::
    /// saveContainerConnections` commits at once rather than through a
    /// draft.
    pub fn add_container_target(&self, target: &ffi::FfiContainerTarget) -> QString {
        let Some(root) = current_project_root() else {
            return QString::default();
        };
        let mut row = super::from_ffi_target(target);
        if row.id.trim().is_empty() {
            row.id = generate_id();
        }
        let id = row.id.clone();
        let _ = app_config::project_settings::update(&root, |settings| {
            let mut containers = settings.containers.clone().unwrap_or_default();
            containers.targets.push(row.clone());
            settings.containers = Some(containers);
        });
        QString::from(id.as_str())
    }

    /// Settings > Containers > Run targets' Edit, reopening the wizard
    /// prefilled: replaces the project's row with `target.id` in place.
    pub fn update_container_target(&self, target: &ffi::FfiContainerTarget) -> FfiResult {
        let Some(root) = current_project_root() else {
            return FfiResult {
                code: errors::CODE_NO_PROJECT,
                message: QString::from("no project is open"),
            };
        };
        let row = super::from_ffi_target(target);
        match app_config::project_settings::update(&root, |settings| {
            let mut containers = settings.containers.clone().unwrap_or_default();
            if let Some(existing) = containers.targets.iter_mut().find(|t| t.id == row.id) {
                *existing = row.clone();
            }
            settings.containers = Some(containers);
        }) {
            Ok(()) => FfiResult::default(),
            Err(err) => FfiResult {
                code: errors::CODE_SETTINGS_IO,
                message: QString::from(err.to_string().as_str()),
            },
        }
    }

    /// Settings > Containers > Run targets' Remove.
    pub fn remove_container_target(&self, id: &QString) -> FfiResult {
        let Some(root) = current_project_root() else {
            return FfiResult {
                code: errors::CODE_NO_PROJECT,
                message: QString::from("no project is open"),
            };
        };
        let id = id.to_string();
        match app_config::project_settings::update(&root, |settings| {
            let mut containers = settings.containers.clone().unwrap_or_default();
            containers.targets.retain(|t| t.id != id);
            settings.containers = Some(containers);
        }) {
            Ok(()) => FfiResult::default(),
            Err(err) => FfiResult {
                code: errors::CODE_SETTINGS_IO,
                message: QString::from(err.to_string().as_str()),
            },
        }
    }

    /// `compose config --services` against `connection_id`, for the Compose
    /// page's Services picker: dispatched to a worker thread (a CLI call
    /// against a possibly slow or unreachable daemon, the same reason
    /// `ContainerService::testConnection` uses one), reported through
    /// `composeServicesReady`. `files` is `\n`-separated, project-relative
    /// paths; an unresolvable connection or a failed call both report an
    /// empty list rather than blocking or crashing the dialog.
    pub fn request_compose_services(
        self: Pin<&mut ffi::RunConfigEditor>,
        connection_id: &QString,
        files: &QString,
    ) {
        let Some(root) = current_project_root() else {
            let qt_thread = self.qt_thread();
            let _ = qt_thread.queue(|mut editor: Pin<&mut ffi::RunConfigEditor>| {
                editor.as_mut().compose_services_ready(QString::default());
            });
            return;
        };
        let containers = effective_container_settings();
        let connection_id = connection_id.to_string();
        let files: Vec<String> = files
            .to_string()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let services = containers
                .connections
                .iter()
                .find(|c| c.id == connection_id)
                .map(|row| {
                    let connection =
                        container_core::connection::ConnectionConfig::from_setting(row);
                    container_core::connection::Invocation {
                        program: connection.compose_program(),
                        ..connection.invocation()
                    }
                })
                .and_then(|invocation| {
                    container_core::run_config::compose_services(&invocation, &files, &root).ok()
                })
                .unwrap_or_default();
            let _ = qt_thread.queue(move |mut editor: Pin<&mut ffi::RunConfigEditor>| {
                editor
                    .as_mut()
                    .compose_services_ready(QString::from(services.join("\n").as_str()));
            });
        });
    }

    /// The first problem that would stop the dialog closing: an empty
    /// `program` for a plain process configuration
    /// (`run_core::RunError::InvalidConfig`'s own rule, mirrored here rather
    /// than calling into `run-core` since a single-field check this shallow
    /// does not warrant a second entry point into that crate), or the
    /// matching `container_core::run_config::validate_*` rule for a
    /// container-kind one — `program` is unused and always empty there, so
    /// the plain-process check would wrongly flag every one of them.
    pub fn validate(&self) -> FfiResult {
        let containers = effective_container_settings();
        for config in self.draft.borrow().iter() {
            let problem = match config.kind.as_deref() {
                Some("container-image") => config
                    .container_image
                    .as_ref()
                    .and_then(container_core::run_config::validate_image),
                Some("containerfile") => config
                    .containerfile
                    .as_ref()
                    .and_then(container_core::run_config::validate_containerfile),
                Some("compose") => config
                    .compose
                    .as_ref()
                    .and_then(container_core::run_config::validate_compose),
                _ => config
                    .program
                    .trim()
                    .is_empty()
                    .then(|| "has no program to run".to_string())
                    .or_else(|| {
                        config.run_on.as_deref().and_then(|run_on| {
                            let id = run_core::target_id(run_on)?;
                            containers.targets.iter().all(|t| t.id != id).then(|| {
                                format!("runs on a target (\"{id}\") that no longer exists")
                            })
                        })
                    }),
            };
            if let Some(problem) = problem {
                let label = if config.name.trim().is_empty() {
                    "one configuration".to_string()
                } else {
                    config.name.clone()
                };
                return FfiResult {
                    code: errors::CODE_EMPTY_PROGRAM,
                    message: QString::from(format!("\"{label}\" {problem}").as_str()),
                };
            }
        }
        FfiResult::default()
    }

    pub fn commit(&self) -> FfiResult {
        let refusal = self.validate();
        if refusal.code != 0 {
            return refusal;
        }
        let Some(root) = current_project_root() else {
            return FfiResult {
                code: errors::CODE_NO_PROJECT,
                message: QString::from("no project is open"),
            };
        };
        let draft = self.draft.borrow().clone();
        match app_config::project_settings::update(&root, |settings| {
            settings.run_configs = Some(draft.clone());
        }) {
            Ok(()) => {
                *self.saved.borrow_mut() = draft;
                FfiResult::default()
            }
            Err(err) => FfiResult {
                code: errors::CODE_SETTINGS_IO,
                message: QString::from(err.to_string().as_str()),
            },
        }
    }

    pub fn revert(&self) {
        *self.draft.borrow_mut() = self.saved.borrow().clone();
    }
}
