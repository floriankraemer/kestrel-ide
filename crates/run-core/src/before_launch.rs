//! Before-launch tasks (B2-1 … B2-3): what has to happen before a run
//! configuration's program starts.
//!
//! IntelliJ's list, minus the parts that need a build model we deliberately
//! do not have: build the project, run another configuration first, or run
//! an external tool. Executing them is the adapter's job — this module owns
//! what a task *is*, how it is persisted, and the two rules that decide
//! whether a list is runnable at all.

use crate::config::RunConfig;
use app_config::BeforeLaunchSetting;

/// One step to perform before a configuration launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeforeLaunchTask {
    /// Build the project with its own build tool. What that runs is
    /// `build-core`'s answer, not this crate's.
    Build,
    /// Run another configuration first, and wait for it to exit.
    RunConfiguration(String),
    /// Run an arbitrary program, and wait for it to exit.
    ExternalTool { program: String, args: Vec<String> },
}

/// Why a before-launch list cannot be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeforeLaunchError {
    /// A `RunConfiguration` task names a configuration that would, directly
    /// or through further tasks, run the one being launched.
    Cycle(String),
    /// A `RunConfiguration` task names a configuration that no longer
    /// exists — a renamed or deleted entry, which is worth saying rather
    /// than silently skipping.
    UnknownConfiguration(String),
}

impl std::fmt::Display for BeforeLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BeforeLaunchError::Cycle(id) => {
                write!(
                    f,
                    "\"{id}\" would run itself through its before-launch tasks"
                )
            }
            BeforeLaunchError::UnknownConfiguration(id) => {
                write!(
                    f,
                    "before-launch task refers to unknown configuration \"{id}\""
                )
            }
        }
    }
}

const KIND_BUILD: &str = "build";
const KIND_RUN_CONFIGURATION: &str = "run_configuration";
const KIND_EXTERNAL_TOOL: &str = "external_tool";

impl BeforeLaunchTask {
    /// The persisted form. A string kind rather than a serde-tagged enum for
    /// the reason ADR-0039 gives for `toolchain`: the shape lives in
    /// `app-config`, which depends on nothing, so the meaning is mapped
    /// here.
    pub fn to_setting(&self) -> BeforeLaunchSetting {
        match self {
            BeforeLaunchTask::Build => BeforeLaunchSetting {
                kind: KIND_BUILD.to_string(),
                ..BeforeLaunchSetting::default()
            },
            BeforeLaunchTask::RunConfiguration(id) => BeforeLaunchSetting {
                kind: KIND_RUN_CONFIGURATION.to_string(),
                config_id: Some(id.clone()),
                ..BeforeLaunchSetting::default()
            },
            BeforeLaunchTask::ExternalTool { program, args } => BeforeLaunchSetting {
                kind: KIND_EXTERNAL_TOOL.to_string(),
                program: Some(program.clone()),
                args: args.clone(),
                ..BeforeLaunchSetting::default()
            },
        }
    }

    /// The inverse. `None` for a kind this version does not know, or one
    /// missing the field it needs — a settings file written by a newer
    /// version loses that one task rather than failing to load.
    pub fn from_setting(setting: &BeforeLaunchSetting) -> Option<BeforeLaunchTask> {
        match setting.kind.as_str() {
            KIND_BUILD => Some(BeforeLaunchTask::Build),
            KIND_RUN_CONFIGURATION => setting
                .config_id
                .clone()
                .map(BeforeLaunchTask::RunConfiguration),
            KIND_EXTERNAL_TOOL => {
                setting
                    .program
                    .clone()
                    .map(|program| BeforeLaunchTask::ExternalTool {
                        program,
                        args: setting.args.clone(),
                    })
            }
            _ => None,
        }
    }
}

/// The tasks a configuration runs before launching, in order.
pub fn tasks_of(config: &RunConfig) -> Vec<BeforeLaunchTask> {
    config
        .before_launch
        .iter()
        .filter_map(BeforeLaunchTask::from_setting)
        .collect()
}

/// [`tasks_of`], with a containerfile configuration's auto build task
/// prepended (C5, ADR-0056): a `kind = "containerfile"` configuration whose
/// [`app_config::ContainerfileRunSetting::run_built_image`] is set always
/// builds before it runs, exactly the way a Cargo/Gradle project's detected
/// configuration always gets a [`BeforeLaunchTask::Build`] — except the
/// build command here is `docker build …`, resolved per-connection, so it
/// cannot be a plain persisted [`BeforeLaunchSetting`] the way `Build` is; it
/// is computed fresh from `config`/`containers` on every call instead of
/// being stored, so editing the connection or the Dockerfile path never
/// leaves a stale build command behind.
///
/// A separate function rather than changing [`tasks_of`] itself: cycle
/// validation ([`validate`]) and every other pre-C5 caller only care about
/// `RunConfiguration` references, so they keep working against configs with
/// no `[containers]` section to hand this one.
pub fn tasks_of_with_containers(
    config: &RunConfig,
    containers: &app_config::ContainerSettings,
) -> Vec<BeforeLaunchTask> {
    let mut tasks = tasks_of(config);
    if let Some(build) = crate::container_run::build_task(config, containers) {
        tasks.insert(0, build);
    }
    tasks
}

/// Check that launching `config_id` will terminate: no configuration in the
/// before-launch graph reachable from it runs it again, and every
/// configuration named still exists.
///
/// Called before the first task runs, so a cycle is a refusal rather than an
/// infinite chain of launches the user has to kill one at a time.
pub fn validate(config_id: &str, configs: &[RunConfig]) -> Result<(), BeforeLaunchError> {
    let mut path = Vec::new();
    walk(config_id, configs, &mut path)
}

fn walk(
    config_id: &str,
    configs: &[RunConfig],
    path: &mut Vec<String>,
) -> Result<(), BeforeLaunchError> {
    if path.iter().any(|seen| seen == config_id) {
        return Err(BeforeLaunchError::Cycle(config_id.to_string()));
    }
    let Some(config) = configs.iter().find(|c| c.id == config_id) else {
        return Err(BeforeLaunchError::UnknownConfiguration(
            config_id.to_string(),
        ));
    };

    path.push(config_id.to_string());
    for task in tasks_of(config) {
        if let BeforeLaunchTask::RunConfiguration(next) = task {
            walk(&next, configs, path)?;
        }
    }
    path.pop();
    Ok(())
}

/// The tasks a newly detected configuration starts life with.
///
/// A configuration whose toolchain has a build step gets a Build task, which
/// is IntelliJ's default and the reason Run on a compiled project runs the
/// code you just wrote. Anything else — a script, a hand-written
/// configuration — gets none.
pub fn default_tasks(config: &RunConfig, toolchain_builds: bool) -> Vec<BeforeLaunchTask> {
    if toolchain_builds && !config.temporary {
        vec![BeforeLaunchTask::Build]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(id: &str, tasks: Vec<BeforeLaunchTask>) -> RunConfig {
        RunConfig {
            id: id.into(),
            name: id.into(),
            program: "true".into(),
            before_launch: tasks.iter().map(BeforeLaunchTask::to_setting).collect(),
            ..RunConfig::default()
        }
    }

    #[test]
    fn every_task_kind_round_trips_through_its_persisted_form() {
        for task in [
            BeforeLaunchTask::Build,
            BeforeLaunchTask::RunConfiguration("run-1".into()),
            BeforeLaunchTask::ExternalTool {
                program: "make".into(),
                args: vec!["prepare".into()],
            },
        ] {
            let setting = task.to_setting();
            assert_eq!(BeforeLaunchTask::from_setting(&setting), Some(task));
        }
    }

    #[test]
    fn an_unknown_kind_loses_that_task_rather_than_the_file() {
        let setting = BeforeLaunchSetting {
            kind: "teleport".into(),
            ..BeforeLaunchSetting::default()
        };
        assert_eq!(BeforeLaunchTask::from_setting(&setting), None);
    }

    #[test]
    fn a_task_missing_the_field_it_needs_is_dropped() {
        let setting = BeforeLaunchSetting {
            kind: "run_configuration".into(),
            ..BeforeLaunchSetting::default()
        };
        assert_eq!(BeforeLaunchTask::from_setting(&setting), None);
    }

    #[test]
    fn a_plain_list_validates() {
        let configs = vec![
            config("a", vec![BeforeLaunchTask::Build]),
            config("b", vec![BeforeLaunchTask::RunConfiguration("a".into())]),
        ];
        assert_eq!(validate("b", &configs), Ok(()));
    }

    #[test]
    fn a_configuration_that_runs_itself_is_refused() {
        let configs = vec![config(
            "a",
            vec![BeforeLaunchTask::RunConfiguration("a".into())],
        )];
        assert_eq!(
            validate("a", &configs),
            Err(BeforeLaunchError::Cycle("a".into()))
        );
    }

    #[test]
    fn an_indirect_cycle_is_refused_too() {
        let configs = vec![
            config("a", vec![BeforeLaunchTask::RunConfiguration("b".into())]),
            config("b", vec![BeforeLaunchTask::RunConfiguration("c".into())]),
            config("c", vec![BeforeLaunchTask::RunConfiguration("a".into())]),
        ];
        assert_eq!(
            validate("a", &configs),
            Err(BeforeLaunchError::Cycle("a".into()))
        );
    }

    #[test]
    fn the_same_configuration_twice_in_one_list_is_not_a_cycle() {
        // Running the same preparation step for two different tasks is
        // wasteful, not circular — and refusing it would be surprising.
        let configs = vec![
            config("prep", vec![]),
            config(
                "a",
                vec![
                    BeforeLaunchTask::RunConfiguration("prep".into()),
                    BeforeLaunchTask::RunConfiguration("prep".into()),
                ],
            ),
        ];
        assert_eq!(validate("a", &configs), Ok(()));
    }

    #[test]
    fn a_task_naming_a_deleted_configuration_says_so() {
        let configs = vec![config(
            "a",
            vec![BeforeLaunchTask::RunConfiguration("gone".into())],
        )];
        assert_eq!(
            validate("a", &configs),
            Err(BeforeLaunchError::UnknownConfiguration("gone".into()))
        );
    }

    #[test]
    fn a_compiled_configuration_is_built_before_it_runs_and_a_temporary_one_is_not() {
        let saved = config("a", vec![]);
        assert_eq!(default_tasks(&saved, true), vec![BeforeLaunchTask::Build]);
        assert!(default_tasks(&saved, false).is_empty());

        let temporary = RunConfig {
            temporary: true,
            ..config("b", vec![])
        };
        assert!(
            default_tasks(&temporary, true).is_empty(),
            "a gutter click should start the program, not a full build"
        );
    }
}
