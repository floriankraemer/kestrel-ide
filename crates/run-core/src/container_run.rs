//! Container-kind run configurations (C5, ADR-0056): what
//! `RunConfigExt::to_launch_spec_in` dispatches to when
//! `RunConfig::kind` is `"container-image"`, `"containerfile"` or
//! `"compose"`, plus the before-launch build task and the console's
//! Stop/Down commands for a compose configuration.
//!
//! Every argv comes from `container_core::run_config`; this module's own
//! job is resolving `connection_id` to an [`Invocation`] (falling back to a
//! bare local `docker` when the id is blank or unknown — the least
//! surprising default, same rule [`crate::toolchain::ToolchainId::from_id`]
//! follows for an unrecognised string) and expanding this crate's own
//! `$PROJECT_DIR$`-style macros through every string field a container
//! setting carries, exactly as [`crate::config::RunConfigExt::to_launch_spec_in`]
//! already does for a plain process configuration's `cwd`/`args`/`env`.

use app_config::container_run::{
    BindMount, ComposeRunSetting, ContainerImageRunSetting, ContainerfileRunSetting,
};
use app_config::ContainerSettings;
use container_core::connection::{ConnectionConfig, Engine, Invocation};
use container_core::run_config;

use crate::before_launch::BeforeLaunchTask;
use crate::config::{ConsoleKind, LaunchSpec, RunConfig};
use crate::macros::{self, MacroContext};
use crate::toolchain::ToolCommand;

/// The [`Invocation`] `connection_id` names, or a bare local `docker` when
/// it is blank or matches no configured connection — never a hard failure:
/// a container run configuration with no server picked yet should still
/// preview and attempt a command rather than refuse to launch.
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

fn e(value: &str, context: &MacroContext) -> String {
    macros::expand(value, context)
}

fn expand_image(
    setting: &ContainerImageRunSetting,
    context: &MacroContext,
) -> ContainerImageRunSetting {
    ContainerImageRunSetting {
        connection_id: setting.connection_id.clone(),
        image: e(&setting.image, context),
        container_name: e(&setting.container_name, context),
        publish_all_ports: setting.publish_all_ports,
        port_bindings: setting.port_bindings.clone(),
        entrypoint: e(&setting.entrypoint, context),
        command: setting.command.iter().map(|a| e(a, context)).collect(),
        bind_mounts: expand_mounts(&setting.bind_mounts, context),
        env: expand_env(&setting.env, context),
        run_options: e(&setting.run_options, context),
        attach: setting.attach,
        pull_policy: setting.pull_policy.clone(),
    }
}

fn expand_containerfile(
    setting: &ContainerfileRunSetting,
    context: &MacroContext,
) -> ContainerfileRunSetting {
    ContainerfileRunSetting {
        connection_id: setting.connection_id.clone(),
        container_name: e(&setting.container_name, context),
        publish_all_ports: setting.publish_all_ports,
        port_bindings: setting.port_bindings.clone(),
        entrypoint: e(&setting.entrypoint, context),
        command: setting.command.iter().map(|a| e(a, context)).collect(),
        bind_mounts: expand_mounts(&setting.bind_mounts, context),
        env: expand_env(&setting.env, context),
        run_options: e(&setting.run_options, context),
        attach: setting.attach,
        pull_policy: setting.pull_policy.clone(),
        dockerfile: e(&setting.dockerfile, context),
        context_dir: e(&setting.context_dir, context),
        image_tag: e(&setting.image_tag, context),
        build_args: expand_env(&setting.build_args, context),
        build_options: e(&setting.build_options, context),
        run_built_image: setting.run_built_image,
    }
}

fn expand_compose(setting: &ComposeRunSetting, context: &MacroContext) -> ComposeRunSetting {
    ComposeRunSetting {
        connection_id: setting.connection_id.clone(),
        compose_files: setting
            .compose_files
            .iter()
            .map(|f| e(f, context))
            .collect(),
        services: setting.services.clone(),
        project_name: e(&setting.project_name, context),
        profiles: setting.profiles.clone(),
        env: expand_env(&setting.env, context),
        env_files: setting.env_files.iter().map(|f| e(f, context)).collect(),
        ..setting.clone()
    }
}

fn expand_mounts(mounts: &[BindMount], context: &MacroContext) -> Vec<BindMount> {
    mounts
        .iter()
        .map(|m| BindMount {
            host_path: e(&m.host_path, context),
            container_path: e(&m.container_path, context),
            read_only: m.read_only,
        })
        .collect()
}

fn expand_env(pairs: &[(String, String)], context: &MacroContext) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.clone(), e(v, context)))
        .collect()
}

fn project_root_of(context: &MacroContext) -> std::path::PathBuf {
    context
        .project_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn spec_from(invocation: &Invocation, argv: Vec<String>, context: &MacroContext) -> LaunchSpec {
    LaunchSpec {
        program: invocation.program.clone(),
        args: invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
        cwd: Some(project_root_of(context)),
        env: invocation.env.clone(),
        console: ConsoleKind::Pty,
    }
}

/// `kind = "container-image"`: `docker run …` directly.
pub(crate) fn image_launch_spec(
    config: &RunConfig,
    context: &MacroContext,
    containers: &ContainerSettings,
) -> Option<LaunchSpec> {
    let setting = config.container_image.as_ref()?;
    let setting = expand_image(setting, context);
    let invocation = invocation_for(containers, &setting.connection_id);
    let argv = run_config::image_run_argv(
        &setting,
        &project_root_of(context),
        containers.selinux_relabel,
    );
    Some(spec_from(&invocation, argv, context))
}

/// `kind = "containerfile"`: when [`ContainerfileRunSetting::run_built_image`]
/// is set, `docker run <image_tag> …` (the build itself is a before-launch
/// task, see [`build_task`]); otherwise the "run" step *is* the build —
/// launching this configuration alone only builds the image, which is what
/// makes it usable as another configuration's before-launch
/// `RunConfiguration` task with nothing started afterward.
pub(crate) fn containerfile_launch_spec(
    config: &RunConfig,
    context: &MacroContext,
    containers: &ContainerSettings,
) -> Option<LaunchSpec> {
    let setting = config.containerfile.as_ref()?;
    let setting = expand_containerfile(setting, context);
    let invocation = invocation_for(containers, &setting.connection_id);
    let argv = if setting.run_built_image {
        run_config::containerfile_run_argv(&setting, containers.selinux_relabel)
    } else {
        run_config::containerfile_build_argv(&setting)
    };
    Some(spec_from(&invocation, argv, context))
}

/// `kind = "compose"`: `compose up -d …`, through the connection's own
/// compose program (`docker compose`/`podman compose`, or a
/// `compose_executable` override — see
/// [`container_core::connection::ConnectionConfig::compose_program`]).
pub(crate) fn compose_launch_spec(
    config: &RunConfig,
    context: &MacroContext,
    containers: &ContainerSettings,
) -> Option<LaunchSpec> {
    let setting = config.compose.as_ref()?;
    let setting = expand_compose(setting, context);
    let invocation = compose_invocation(containers, &setting.connection_id);
    let argv = run_config::compose_up_argv(&setting, &project_root_of(context));
    Some(spec_from(&invocation, argv, context))
}

/// The compose-flavored [`Invocation`]: same connection resolution as
/// [`invocation_for`], but its `program` is the connection's
/// `compose_program()`, not the bare engine.
fn compose_invocation(containers: &ContainerSettings, connection_id: &str) -> Invocation {
    let Some(row) = containers
        .connections
        .iter()
        .find(|c| c.id == connection_id)
    else {
        let config = ConnectionConfig {
            engine: Engine::Docker,
            kind: container_core::connection::ConnectionKind::Auto,
            executable: None,
            compose_executable: None,
        };
        return Invocation {
            program: config.compose_program(),
            ..config.invocation()
        };
    };
    let config = ConnectionConfig::from_setting(row);
    Invocation {
        program: config.compose_program(),
        ..config.invocation()
    }
}

/// The auto before-launch build task a containerfile configuration gets
/// when it also runs its built image — `docker build …`, run before the
/// `docker run` [`containerfile_launch_spec`] produces. `None` for any
/// other kind, or a containerfile configuration that only builds (nothing
/// to run first).
pub(crate) fn build_task(
    config: &RunConfig,
    containers: &ContainerSettings,
) -> Option<BeforeLaunchTask> {
    let setting = config.containerfile.as_ref()?;
    if !setting.run_built_image {
        return None;
    }
    let invocation = invocation_for(containers, &setting.connection_id);
    let argv = run_config::containerfile_build_argv(setting);
    Some(BeforeLaunchTask::ExternalTool {
        program: invocation.program.clone(),
        args: invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
    })
}

/// `compose stop` — the run console's Stop action for a compose
/// configuration, so stopping it leaves no containers running the way a
/// plain Ctrl-C against `compose up` would (JetBrains parity: `compose up`
/// killed outright leaves the containers up).
pub fn stop_command(config: &RunConfig, containers: &ContainerSettings) -> Option<ToolCommand> {
    let setting = config.compose.as_ref()?;
    let invocation = compose_invocation(containers, &setting.connection_id);
    let argv = run_config::compose_stop_argv(setting);
    Some(ToolCommand {
        program: invocation.program.clone(),
        args: invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
    })
}

/// `compose down`, with the configured remove flags — the console's "Down"
/// action.
pub fn down_command(config: &RunConfig, containers: &ContainerSettings) -> Option<ToolCommand> {
    let setting = config.compose.as_ref()?;
    let invocation = compose_invocation(containers, &setting.connection_id);
    let argv = run_config::compose_down_argv(setting);
    Some(ToolCommand {
        program: invocation.program.clone(),
        args: invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
    })
}

/// An ad-hoc `ComposeRunSetting` for acting directly on a Containers dock
/// compose project node — Start All/Stop/Down/Scale — rather than a saved
/// run configuration: it always targets every service the project's own
/// `docker-compose.yml`(s) define (`services` empty, `run_config`'s own
/// "empty means everything" rule) and is never persisted.
fn project_setting(connection_id: &str, files: &[String], project_name: &str) -> ComposeRunSetting {
    ComposeRunSetting {
        connection_id: connection_id.to_string(),
        compose_files: files.to_vec(),
        project_name: project_name.to_string(),
        ..ComposeRunSetting::default()
    }
}

fn to_tool_command(invocation: &Invocation, argv: &[String]) -> ToolCommand {
    ToolCommand {
        program: invocation.program.clone(),
        args: invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
    }
}

/// `compose -f <files>… -p <project_name> up -d` — a Containers dock
/// compose project's "Start All".
pub fn compose_project_up_spec(
    containers: &ContainerSettings,
    connection_id: &str,
    files: &[String],
    project_name: &str,
    project_root: &std::path::Path,
) -> LaunchSpec {
    let setting = project_setting(connection_id, files, project_name);
    let invocation = compose_invocation(containers, connection_id);
    let argv = run_config::compose_up_argv(&setting, project_root);
    LaunchSpec {
        program: invocation.program.clone(),
        args: invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
        cwd: Some(project_root.to_path_buf()),
        env: invocation.env.clone(),
        console: ConsoleKind::Pty,
    }
}

/// `compose -f <files>… -p <project_name> stop` — a compose project node's
/// "Stop".
pub fn compose_project_stop_command(
    containers: &ContainerSettings,
    connection_id: &str,
    files: &[String],
    project_name: &str,
) -> ToolCommand {
    let setting = project_setting(connection_id, files, project_name);
    let invocation = compose_invocation(containers, connection_id);
    to_tool_command(&invocation, &run_config::compose_stop_argv(&setting))
}

/// `compose -f <files>… -p <project_name> down` — a compose project node's
/// "Down".
pub fn compose_project_down_command(
    containers: &ContainerSettings,
    connection_id: &str,
    files: &[String],
    project_name: &str,
) -> ToolCommand {
    let setting = project_setting(connection_id, files, project_name);
    let invocation = compose_invocation(containers, connection_id);
    to_tool_command(&invocation, &run_config::compose_down_argv(&setting))
}

/// `compose -f <files>… up -d --no-recreate --scale <service>=<n>` — a
/// compose service node's "Scale...".
pub fn compose_project_scale_command(
    containers: &ContainerSettings,
    connection_id: &str,
    files: &[String],
    service: &str,
    count: u32,
) -> ToolCommand {
    let invocation = compose_invocation(containers, connection_id);
    to_tool_command(
        &invocation,
        &run_config::compose_scale_argv(files, service, count),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(kind: &str) -> RunConfig {
        RunConfig {
            id: "c1".into(),
            name: "c1".into(),
            kind: Some(kind.to_string()),
            ..RunConfig::default()
        }
    }

    #[test]
    fn an_unconfigured_connection_falls_back_to_local_docker() {
        let containers = ContainerSettings::default();
        let invocation = invocation_for(&containers, "missing");
        assert_eq!(invocation.program, "docker");
        assert!(invocation.prefix_args.is_empty());
    }

    #[test]
    fn image_launch_spec_runs_docker_run_with_the_configured_image() {
        let mut cfg = config("container-image");
        cfg.container_image = Some(ContainerImageRunSetting {
            image: "nginx:1.27".into(),
            ..Default::default()
        });
        let context = MacroContext::for_project("/project");
        let spec = image_launch_spec(&cfg, &context, &ContainerSettings::default()).unwrap();
        assert_eq!(spec.program, "docker");
        assert_eq!(
            spec.args,
            vec!["run", "-d", "--pull", "missing", "nginx:1.27"]
        );
        assert_eq!(spec.cwd, Some(std::path::PathBuf::from("/project")));
    }

    #[test]
    fn image_reference_macros_expand_before_compiling_argv() {
        let mut cfg = config("container-image");
        cfg.container_image = Some(ContainerImageRunSetting {
            image: "myrepo/app:$FILE_NAME$".into(),
            ..Default::default()
        });
        let context = MacroContext::for_file("/project", "/project/src/tag.txt");
        let spec = image_launch_spec(&cfg, &context, &ContainerSettings::default()).unwrap();
        assert!(spec.args.contains(&"myrepo/app:tag.txt".to_string()));
    }

    #[test]
    fn containerfile_launch_spec_runs_the_built_image_when_run_built_image_is_set() {
        let mut cfg = config("containerfile");
        cfg.containerfile = Some(ContainerfileRunSetting {
            image_tag: "myapp:dev".into(),
            run_built_image: true,
            ..Default::default()
        });
        let context = MacroContext::for_project("/project");
        let spec =
            containerfile_launch_spec(&cfg, &context, &ContainerSettings::default()).unwrap();
        assert!(spec.args.contains(&"myapp:dev".to_string()));
        assert!(!spec.args.contains(&"build".to_string()));
    }

    #[test]
    fn containerfile_launch_spec_only_builds_when_run_built_image_is_unset() {
        let mut cfg = config("containerfile");
        cfg.containerfile = Some(ContainerfileRunSetting {
            image_tag: "myapp:dev".into(),
            run_built_image: false,
            ..Default::default()
        });
        let context = MacroContext::for_project("/project");
        let spec =
            containerfile_launch_spec(&cfg, &context, &ContainerSettings::default()).unwrap();
        assert!(spec.args.contains(&"build".to_string()));
        assert!(!spec.args.contains(&"run".to_string()));
    }

    #[test]
    fn build_task_is_present_only_when_run_built_image_is_set() {
        let containers = ContainerSettings::default();
        let mut cfg = config("containerfile");
        cfg.containerfile = Some(ContainerfileRunSetting {
            image_tag: "myapp:dev".into(),
            run_built_image: true,
            ..Default::default()
        });
        assert!(build_task(&cfg, &containers).is_some());

        cfg.containerfile.as_mut().unwrap().run_built_image = false;
        assert!(build_task(&cfg, &containers).is_none());

        assert!(build_task(&config("container-image"), &containers).is_none());
    }

    #[test]
    fn compose_launch_spec_runs_compose_up_detached() {
        let mut cfg = config("compose");
        cfg.compose = Some(ComposeRunSetting {
            compose_files: vec!["docker-compose.yml".into()],
            ..Default::default()
        });
        let context = MacroContext::for_project("/project");
        let spec = compose_launch_spec(&cfg, &context, &ContainerSettings::default()).unwrap();
        assert_eq!(spec.program, "docker");
        assert_eq!(
            spec.args,
            vec!["compose", "-f", "docker-compose.yml", "up", "-d"]
        );
    }

    #[test]
    fn stop_and_down_commands_are_only_for_compose_configurations() {
        let containers = ContainerSettings::default();
        let mut compose_cfg = config("compose");
        compose_cfg.compose = Some(ComposeRunSetting {
            compose_files: vec!["docker-compose.yml".into()],
            remove_orphans_on_down: true,
            ..Default::default()
        });
        let stop = stop_command(&compose_cfg, &containers).unwrap();
        assert_eq!(
            stop.args,
            vec!["compose", "-f", "docker-compose.yml", "stop"]
        );
        let down = down_command(&compose_cfg, &containers).unwrap();
        assert_eq!(
            down.args,
            vec![
                "compose",
                "-f",
                "docker-compose.yml",
                "down",
                "--remove-orphans"
            ]
        );

        let image_cfg = config("container-image");
        assert!(stop_command(&image_cfg, &containers).is_none());
        assert!(down_command(&image_cfg, &containers).is_none());
    }

    #[test]
    fn compose_program_override_is_used_for_up_stop_and_down() {
        let containers = ContainerSettings {
            connections: vec![app_config::ContainerConnectionSetting {
                id: "conn".into(),
                compose_executable: "docker-compose".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut cfg = config("compose");
        cfg.compose = Some(ComposeRunSetting {
            connection_id: "conn".into(),
            compose_files: vec!["docker-compose.yml".into()],
            ..Default::default()
        });
        let context = MacroContext::for_project("/project");
        let spec = compose_launch_spec(&cfg, &context, &containers).unwrap();
        assert_eq!(spec.program, "docker-compose");
    }

    #[test]
    fn a_compose_project_node_compiles_start_all_stop_down_and_scale() {
        let containers = ContainerSettings::default();
        let files = vec!["docker-compose.yml".to_string()];

        let up =
            compose_project_up_spec(&containers, "", &files, "shop", std::path::Path::new("/p"));
        assert_eq!(up.program, "docker");
        assert_eq!(
            up.args,
            vec![
                "compose",
                "-f",
                "docker-compose.yml",
                "-p",
                "shop",
                "up",
                "-d"
            ]
        );

        let stop = compose_project_stop_command(&containers, "", &files, "shop");
        assert_eq!(
            stop.args,
            vec!["compose", "-f", "docker-compose.yml", "-p", "shop", "stop"]
        );

        let down = compose_project_down_command(&containers, "", &files, "shop");
        assert_eq!(
            down.args,
            vec!["compose", "-f", "docker-compose.yml", "-p", "shop", "down"]
        );

        let scale = compose_project_scale_command(&containers, "", &files, "web", 3);
        assert_eq!(
            scale.args,
            vec![
                "compose",
                "-f",
                "docker-compose.yml",
                "up",
                "-d",
                "--no-recreate",
                "--scale",
                "web=3",
            ]
        );
    }
}
