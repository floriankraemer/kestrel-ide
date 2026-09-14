//! Run targets (C8): resolving `RunConfig::run_on` and wrapping a plain
//! process launch through `container_core::target::wrap_launch`.
//!
//! This is the one place `run-core` knows what `run_on` means — the same
//! split `container_run.rs` draws for a container-*kind* configuration's own
//! `connection_id`: persistence (`app_config`) stores a plain string,
//! `container-core` compiles argv, and this module is the seam that
//! resolves a connection/target id against `[containers]` and reports what
//! went wrong in a shape `RunService::launch` can surface to the user before
//! anything starts.

use app_config::ContainerSettings;
use container_core::connection::{ConnectionConfig, Engine, Invocation};
use container_core::target::{self, SimpleLaunch, TargetError, WrappedLaunch};

use crate::before_launch::BeforeLaunchTask;
use crate::config::{LaunchSpec, RunConfig};
use crate::macros::{self, MacroContext};

/// `run_on`'s persisted spelling is `"container:<target-id>"`; this is the
/// one place that parses it. `None` for every other value — `None` itself
/// included, which is how "local, as always" reads.
pub fn target_id(run_on: &str) -> Option<&str> {
    run_on
        .strip_prefix("container:")
        .filter(|id| !id.is_empty())
}

/// Why a configuration's `run_on` could not be honoured — surfaced to the
/// console the same way `BeforeLaunchError` is, before anything launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetLaunchError {
    /// `run_on` names a target id no `[containers.target]` row has —
    /// renamed or deleted since the configuration was saved.
    UnknownTarget(String),
    /// The launch's own `cwd` falls outside the project root, so it has no
    /// path inside the target's one mount.
    CwdOutsideProject(std::path::PathBuf),
}

impl std::fmt::Display for TargetLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetLaunchError::UnknownTarget(id) => {
                write!(f, "run target \"{id}\" no longer exists")
            }
            TargetLaunchError::CwdOutsideProject(path) => write!(
                f,
                "the run's working directory ({}) is outside the project",
                path.display()
            ),
        }
    }
}

impl From<TargetError> for TargetLaunchError {
    fn from(err: TargetError) -> Self {
        match err {
            TargetError::CwdOutsideProject(path) => TargetLaunchError::CwdOutsideProject(path),
        }
    }
}

fn find_target<'a>(
    containers: &'a ContainerSettings,
    target_id: &str,
) -> Option<&'a app_config::ContainerTargetSetting> {
    containers.targets.iter().find(|t| t.id == target_id)
}

fn invocation_for(containers: &ContainerSettings, connection_id: &str) -> Invocation {
    let Some(row) = containers
        .connections
        .iter()
        .find(|c| c.id == connection_id)
    else {
        return ConnectionConfig {
            engine: Engine::Docker,
            kind: container_core::connection::ConnectionKind::Auto,
            executable: None,
            compose_executable: None,
        }
        .invocation();
    };
    ConnectionConfig::from_setting(row).invocation()
}

/// Check `config.run_on` ahead of launching, the same role
/// `before_launch::validate` plays for a cycle: called on the Qt thread
/// before anything runs, so an unresolvable target is a refusal rather than
/// a launch that silently ran locally or failed deep inside `Supervisor`.
///
/// `Ok(())` for a configuration with no `run_on` at all, or one whose `kind`
/// is already a container kind (`run_on` is meaningless there — see
/// `RunConfigSetting::run_on`'s own doc comment).
pub fn validate_run_on(
    config: &RunConfig,
    containers: &ContainerSettings,
    project_root: &std::path::Path,
) -> Result<(), TargetLaunchError> {
    if config.kind.is_some() {
        return Ok(());
    }
    let Some(run_on) = config.run_on.as_deref() else {
        return Ok(());
    };
    let Some(id) = target_id(run_on) else {
        return Ok(());
    };
    let Some(target) = find_target(containers, id) else {
        return Err(TargetLaunchError::UnknownTarget(id.to_string()));
    };

    let context = MacroContext::for_project(project_root.to_path_buf());
    let cwd = config
        .cwd
        .as_deref()
        .map(|cwd| std::path::PathBuf::from(macros::expand(cwd, &context)));
    let invocation = invocation_for(containers, &target.connection_id);
    let simple = SimpleLaunch {
        program: &config.program,
        args: &[],
        cwd: cwd.as_deref(),
        env: &[],
    };
    target::wrap_launch(
        &simple,
        target,
        &invocation,
        project_root,
        containers.selinux_relabel,
    )
    .map(|_| ())
    .map_err(TargetLaunchError::from)
}

/// Wrap `spec` (a plain process launch, already macro-expanded) through the
/// target `target_id` names, if it still exists. `Ok(None)` for an unknown
/// target — the caller (`RunConfigExt::to_launch_spec_in`) falls back to the
/// unwrapped spec, matching every other "unknown id" rule in this crate;
/// `validate_run_on` is what turns the same condition into a reported error
/// on the interactive launch path.
pub fn wrap_process_spec(
    spec: &LaunchSpec,
    target_id: &str,
    context: &MacroContext,
    containers: &ContainerSettings,
) -> Result<Option<LaunchSpec>, TargetLaunchError> {
    let Some(target) = find_target(containers, target_id) else {
        return Ok(None);
    };
    let project_root = context
        .project_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let invocation = invocation_for(containers, &target.connection_id);
    let simple = SimpleLaunch {
        program: &spec.program,
        args: &spec.args,
        cwd: spec.cwd.as_deref(),
        env: &spec.env,
    };
    let wrapped: WrappedLaunch = target::wrap_launch(
        &simple,
        target,
        &invocation,
        &project_root,
        containers.selinux_relabel,
    )?;
    Ok(Some(LaunchSpec {
        program: wrapped.program,
        args: wrapped.args,
        cwd: wrapped.cwd,
        env: wrapped.env,
        console: spec.console,
        path_map: Some(wrapped.path_map),
    }))
}

/// The before-launch build task a run target needs — the image (or compose
/// service) has to exist before *anything* else in the configuration's own
/// before-launch list runs, so `before_launch::tasks_of_with_containers`
/// inserts this ahead of every other task, containerfile-configuration
/// build task included (see that function's own doc comment for the full
/// ordering rule).
pub fn target_build_task(
    config: &RunConfig,
    containers: &ContainerSettings,
) -> Option<BeforeLaunchTask> {
    if config.kind.is_some() {
        return None;
    }
    let id = target_id(config.run_on.as_deref()?)?;
    let target = find_target(containers, id)?;
    let invocation = invocation_for(containers, &target.connection_id);
    let (program, args) = target::before_launch_for(target, &invocation)?;
    Some(BeforeLaunchTask::ExternalTool { program, args })
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_config::ContainerTargetSetting;

    fn containers_with(target: ContainerTargetSetting) -> ContainerSettings {
        ContainerSettings {
            targets: vec![target],
            ..ContainerSettings::default()
        }
    }

    fn image_target() -> ContainerTargetSetting {
        ContainerTargetSetting {
            id: "t1".to_string(),
            name: "nginx".to_string(),
            source: "image".to_string(),
            image: Some("nginx:1.27".to_string()),
            ..ContainerTargetSetting::default()
        }
    }

    fn config(program: &str, run_on: Option<&str>) -> RunConfig {
        RunConfig {
            id: "c1".into(),
            name: "c1".into(),
            program: program.to_string(),
            run_on: run_on.map(str::to_string),
            ..RunConfig::default()
        }
    }

    #[test]
    fn target_id_parses_the_container_prefix() {
        assert_eq!(target_id("container:t1"), Some("t1"));
        assert_eq!(target_id("local"), None);
        assert_eq!(target_id("container:"), None);
    }

    #[test]
    fn no_run_on_validates_and_wraps_to_nothing() {
        let containers = ContainerSettings::default();
        let cfg = config("python3", None);
        assert!(validate_run_on(&cfg, &containers, std::path::Path::new("/p")).is_ok());
    }

    #[test]
    fn an_unknown_target_fails_validation() {
        let containers = ContainerSettings::default();
        let cfg = config("python3", Some("container:missing"));
        assert_eq!(
            validate_run_on(&cfg, &containers, std::path::Path::new("/p")),
            Err(TargetLaunchError::UnknownTarget("missing".to_string()))
        );
    }

    #[test]
    fn a_known_target_validates() {
        let containers = containers_with(image_target());
        let cfg = config("python3", Some("container:t1"));
        assert!(validate_run_on(&cfg, &containers, std::path::Path::new("/p")).is_ok());
    }

    #[test]
    fn a_container_kind_configuration_ignores_run_on() {
        let containers = ContainerSettings::default();
        let mut cfg = config("python3", Some("container:missing"));
        cfg.kind = Some("container-image".to_string());
        assert!(validate_run_on(&cfg, &containers, std::path::Path::new("/p")).is_ok());
    }

    #[test]
    fn wrap_process_spec_wraps_a_plain_launch() {
        let containers = containers_with(image_target());
        let context = MacroContext::for_project("/p");
        let spec = LaunchSpec {
            program: "python3".to_string(),
            args: vec!["app.py".to_string()],
            cwd: Some(std::path::PathBuf::from("/p")),
            env: Vec::new(),
            console: crate::config::ConsoleKind::Pty,
            path_map: None,
        };
        let wrapped = wrap_process_spec(&spec, "t1", &context, &containers)
            .unwrap()
            .unwrap();
        assert_eq!(wrapped.program, "docker");
        assert!(wrapped.args.contains(&"nginx:1.27".to_string()));
        assert!(wrapped.args.contains(&"python3".to_string()));
        assert!(wrapped.path_map.is_some());
    }

    #[test]
    fn wrap_process_spec_is_none_for_an_unknown_target() {
        let containers = ContainerSettings::default();
        let context = MacroContext::for_project("/p");
        let spec = LaunchSpec {
            program: "python3".to_string(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            console: crate::config::ConsoleKind::Pty,
            path_map: None,
        };
        assert_eq!(
            wrap_process_spec(&spec, "missing", &context, &containers),
            Ok(None)
        );
    }

    #[test]
    fn target_build_task_is_present_only_for_a_containerfile_target() {
        let containers = containers_with(image_target());
        let cfg = config("python3", Some("container:t1"));
        assert!(target_build_task(&cfg, &containers).is_none());

        let mut cf_target = image_target();
        cf_target.source = "containerfile".to_string();
        cf_target.image = None;
        cf_target.dockerfile = Some("Dockerfile".to_string());
        cf_target.image_tag = Some("myapp:dev".to_string());
        let containers = containers_with(cf_target);
        let task = target_build_task(&cfg, &containers).unwrap();
        match task {
            BeforeLaunchTask::ExternalTool { program, args } => {
                assert_eq!(program, "docker");
                assert!(args.contains(&"myapp:dev".to_string()));
            }
            _ => panic!("expected an ExternalTool task"),
        }
    }

    #[test]
    fn target_build_task_is_none_without_run_on() {
        let containers = ContainerSettings::default();
        let cfg = config("python3", None);
        assert!(target_build_task(&cfg, &containers).is_none());
    }
}
