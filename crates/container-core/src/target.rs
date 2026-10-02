//! Run targets (C8): running a *program* run configuration's launch inside
//! a container instead of on the local machine.
//!
//! This is a different feature from C5's container-kind run configurations
//! (`kind = "container-image"/"containerfile"/"compose"`, whose own launch
//! already *is* `docker run …`/`compose up …`). A run target wraps an
//! ordinary process launch — `program`/`args`/`cwd`/`env` — so it runs as
//! `docker run … <image> <program> <args…>` (or `compose run … <service>
//! <program> <args…>`) instead, the way `process_exec::host::ExecHost::Wsl`
//! wraps a launch to run inside a WSL distro (ADR-0052) rather than adding a
//! third place that builds argv.
//!
//! [`wrap_launch`] rewrites a *run configuration's* launch spec in
//! `run-core`, before `Supervisor::launch` sees it. [`exec_host`] (ADR-0067,
//! which supersedes ADR-0056's rejection of an `ExecHost::Container`
//! variant) instead builds an `ExecHost::Container` for tools the IDE runs
//! itself through `process_exec::run_on`/`spawn_on` — analyzers, test
//! frameworks, language servers. Both share [`PathMap`].

use std::path::{Path, PathBuf};

use app_config::container_run::ContainerfileRunSetting;
use app_config::{ContainerSettings, ContainerTargetSetting};
use process_exec::host::{ContainerHost, ExecHost};

use crate::connection::{ConnectionConfig, ConnectionKind, Engine, Invocation};
use crate::run_config::{self, mount_arg, port_arg, split_shell_words};

/// Where the project root mounts inside a target's container when the
/// setting leaves [`ContainerTargetSetting::workdir`] empty.
pub use process_exec::host::DEFAULT_WORKDIR;

/// Why [`wrap_launch`] refused to wrap a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// The launch's own `cwd` is not under `project_root` — there is no
    /// honest path inside the container's one mount to run it from.
    CwdOutsideProject(PathBuf),
}

impl std::fmt::Display for TargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetError::CwdOutsideProject(path) => write!(
                f,
                "the run's working directory ({}) is outside the project, so it has no path inside the container's mount",
                path.display()
            ),
        }
    }
}

/// The local project root <-> the container's mount root, both directions.
/// Lives in `process-exec` (ADR-0067) so `ExecHost::Container` can use it
/// without `process-exec` depending on this crate.
pub use process_exec::host::PathMap;

/// The minimal shape [`wrap_launch`] needs from a plain-process
/// `run_core::LaunchSpec` — this crate does not depend on `run-core`
/// (layering runs the other way), so `run-core` builds one of these from its
/// own `LaunchSpec` rather than this module borrowing that type.
pub struct SimpleLaunch<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env: &'a [(String, String)],
}

/// What [`wrap_launch`] produces: a new program/args/cwd/env to actually
/// spawn (through `invocation`'s own connection, on the local/host side),
/// plus the [`PathMap`] a console's file links and a build's diagnostic
/// paths resolve project paths back through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub path_map: PathMap,
}

/// The image tag a containerfile target's before-launch build produces, and
/// [`wrap_launch`] then runs: [`ContainerTargetSetting::image_tag`] when
/// set, else a deterministic `ide-target-<id>` — stable across rebuilds so
/// re-running the target after editing nothing still hits the same tag.
pub fn image_tag_for(target: &ContainerTargetSetting) -> String {
    match &target.image_tag {
        Some(tag) if !tag.is_empty() => tag.clone(),
        _ => format!("ide-target-{}", target.id),
    }
}

/// The mount source `-v` gives `docker`/`podman`: the project root as the
/// engine's own CLI actually sees it. `invocation.host` is
/// `process_exec::host::ExecHost::Wsl` exactly when the connection reaches
/// the engine by wrapping every command in `wsl.exe -d <distro> -- ...`
/// (`connection::ConnectionConfig::invocation`'s own `Wsl` arm) — the CLI
/// then runs *inside* that distro, so the mount source must already be the
/// distro's own Linux path, which `ExecHost::to_remote` gives.
fn mount_source(invocation: &Invocation, project_root: &Path) -> String {
    if invocation.host.runs_remotely() {
        invocation.host.to_remote(project_root)
    } else {
        project_root.to_string_lossy().replace('\\', "/")
    }
}

/// Rebase env values that are (or start with) a project-root path onto the
/// container's mount root — `macros::expand` has already turned
/// `$PROJECT_DIR$` into a real local path by the time this runs, so this is
/// the one place left that knows the value is about to run somewhere else.
/// A value that is not a project path is passed through untouched.
fn rebased_env(env: &[(String, String)], path_map: &PathMap) -> Vec<(String, String)> {
    env.iter()
        .map(|(key, value)| match path_map.to_remote(Path::new(value)) {
            Some(remote) => (key.clone(), remote),
            None => (key.clone(), value.clone()),
        })
        .collect()
}

/// Wrap `spec` to run inside `target`'s container, through `invocation`
/// (the resolved connection `target.connection_id` names).
///
/// `spec.cwd` — the run's own working directory, already macro-expanded by
/// the caller — must resolve under `project_root`
/// ([`TargetError::CwdOutsideProject`] otherwise): a run target has exactly
/// one mount, the project root, so any other `cwd` has no honest path inside
/// the container to start from.
pub fn wrap_launch(
    spec: &SimpleLaunch<'_>,
    target: &ContainerTargetSetting,
    invocation: &Invocation,
    project_root: &Path,
    selinux_relabel: bool,
) -> Result<WrappedLaunch, TargetError> {
    let path_map = PathMap::new(project_root.to_path_buf(), &target.workdir);

    let remote_cwd = match spec.cwd {
        Some(cwd) => path_map
            .to_remote(cwd)
            .ok_or_else(|| TargetError::CwdOutsideProject(cwd.to_path_buf()))?,
        None => path_map.remote_root.clone(),
    };

    let env = rebased_env(spec.env, &path_map);

    let mut argv: Vec<String> = Vec::new();

    if target.source == "compose-service" {
        argv.extend(compose_prefix(target));
        argv.push("run".to_string());
        argv.push("--rm".to_string());
        argv.push("--service-ports".to_string());
        argv.push("-w".to_string());
        argv.push(remote_cwd);
        for (key, value) in &env {
            argv.push("-e".to_string());
            argv.push(format!("{key}={value}"));
        }
        if !target.run_options.is_empty() {
            argv.extend(split_shell_words(&target.run_options));
        }
        argv.push(target.service.clone().unwrap_or_default());
    } else {
        argv.push("run".to_string());
        argv.push("--rm".to_string());
        argv.push("-i".to_string());
        argv.push("-t".to_string());
        argv.push("-v".to_string());
        argv.push(mount_arg(
            &mount_source(invocation, project_root),
            &path_map.remote_root,
            false,
            selinux_relabel,
        ));
        argv.push("-w".to_string());
        argv.push(remote_cwd);
        for (key, value) in &env {
            argv.push("-e".to_string());
            argv.push(format!("{key}={value}"));
        }
        if target.publish_all_ports {
            argv.push("-P".to_string());
        }
        for binding in &target.port_bindings {
            argv.push("-p".to_string());
            argv.push(port_arg(binding));
        }
        for mount in &target.extra_mounts {
            argv.push("-v".to_string());
            argv.push(mount_arg(
                &mount.host_path,
                &mount.container_path,
                mount.read_only,
                selinux_relabel,
            ));
        }
        if !target.run_options.is_empty() {
            argv.extend(split_shell_words(&target.run_options));
        }
        argv.push(image_reference(target));
    }

    argv.push(spec.program.to_string());
    argv.extend(spec.args.iter().cloned());

    Ok(WrappedLaunch {
        program: invocation.program.clone(),
        args: invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
        cwd: Some(project_root.to_path_buf()),
        env: invocation.env.clone(),
        path_map,
    })
}

/// `compose -f <file>...`: what precedes every compose verb for `target`.
fn compose_prefix(target: &ContainerTargetSetting) -> Vec<String> {
    let mut argv = vec!["compose".to_string()];
    for file in &target.compose_files {
        argv.push("-f".to_string());
        argv.push(file.clone());
    }
    argv
}

/// The invocation for the connection `connection_id` names; the default
/// local Docker one when it is blank or matches no configured connection —
/// never a hard failure, so a target with no server picked yet still
/// previews and attempts a command rather than refusing.
pub fn invocation_for(containers: &ContainerSettings, connection_id: &str) -> Invocation {
    match containers
        .connections
        .iter()
        .find(|c| c.id == connection_id)
    {
        Some(row) => ConnectionConfig::from_setting(row).invocation(),
        None => ConnectionConfig {
            engine: Engine::Docker,
            kind: ConnectionKind::Auto,
            executable: None,
            compose_executable: None,
        }
        .invocation(),
    }
}

/// [`exec_host`] for the target with id `target_id` in `containers`, with
/// its connection resolved; `None` when no such target exists.
pub fn exec_host_for(
    containers: &ContainerSettings,
    target_id: &str,
    project_root: &Path,
    mode: Option<ExecMode>,
) -> Option<ExecHost> {
    let target = containers.targets.iter().find(|t| t.id == target_id)?;
    Some(exec_host(
        target,
        &invocation_for(containers, &target.connection_id),
        project_root,
        mode,
        containers.selinux_relabel,
    ))
}

/// How a tool the IDE runs itself reaches its container (ADR-0067).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// `exec` into the running container or compose service.
    Exec,
    /// `run --rm -i` a fresh container.
    Run,
}

/// The mode a target gets when the setting does not choose: a compose
/// service is usually already up (`exec`); an image has nothing to `exec`
/// into, so it is always `run` (an explicit `Exec` on one is ignored).
fn effective_mode(target: &ContainerTargetSetting, requested: Option<ExecMode>) -> ExecMode {
    match (target.source.as_str(), requested) {
        ("compose-service", Some(mode)) => mode,
        ("compose-service", None) => ExecMode::Exec,
        _ => ExecMode::Run,
    }
}

/// The host that runs a tool inside `target`'s container, for
/// `process_exec::run_on`/`spawn_on` (ADR-0067): compose services `exec`
/// (`docker compose exec -T <svc> ...`) unless `mode` says `Run`; images
/// `run --rm -i` with the project bind-mounted at the target's workdir.
///
/// Shares [`PathMap`] with [`wrap_launch`], so a path the tool prints
/// (`ExecHost::path_from_tool`) maps back to the local file. Image ports
/// are not published: a one-shot analyzer run has nothing to listen on.
pub fn exec_host(
    target: &ContainerTargetSetting,
    invocation: &Invocation,
    project_root: &Path,
    mode: Option<ExecMode>,
    selinux_relabel: bool,
) -> ExecHost {
    let path_map = PathMap::new(project_root.to_path_buf(), &target.workdir);
    let mode = effective_mode(target, mode);

    let mut prefix_args = invocation.prefix_args.clone();
    let mut verb_args = Vec::new();
    let reference = if target.source == "compose-service" {
        prefix_args.extend(compose_prefix(target));
        let verb: &[&str] = match mode {
            ExecMode::Exec => &["exec", "-T"],
            ExecMode::Run => &["run", "--rm", "-T"],
        };
        verb_args.extend(verb.iter().map(|word| word.to_string()));
        target.service.clone().unwrap_or_default()
    } else {
        verb_args.extend(["run", "--rm", "-i", "-v"].map(String::from));
        verb_args.push(mount_arg(
            &mount_source(invocation, project_root),
            &path_map.remote_root,
            false,
            selinux_relabel,
        ));
        for mount in &target.extra_mounts {
            verb_args.push("-v".to_string());
            verb_args.push(mount_arg(
                &mount.host_path,
                &mount.container_path,
                mount.read_only,
                selinux_relabel,
            ));
        }
        verb_args.extend(split_shell_words(&target.run_options));
        image_reference(target)
    };

    ExecHost::Container(ContainerHost {
        program: invocation.program.clone(),
        prefix_args,
        engine_env: invocation.env.clone(),
        via_wsl: invocation.host.runs_remotely(),
        verb_args,
        target: vec![reference],
        path_map,
    })
}

/// The image reference `docker run`/`podman run` is given: as-is for an
/// `image` target, the before-launch build's own tag for a `containerfile`
/// one. Not called for `compose-service` — that source has no single image
/// reference, `compose run` resolves it from the service definition.
fn image_reference(target: &ContainerTargetSetting) -> String {
    match target.source.as_str() {
        "containerfile" => image_tag_for(target),
        _ => target.image.clone().unwrap_or_default(),
    }
}

/// One before-launch command a target needs run before [`wrap_launch`]'s
/// output — the image (or service image) has to exist first. `None` for an
/// `image` target (nothing to build) or a `compose-service` target whose
/// service has no `build:` key (`ContainerTargetSetting::needs_build`
/// false).
///
/// Returns a plain program/args pair, not a `run_core::BeforeLaunchTask`:
/// this crate does not depend on `run-core` (layering runs the other way),
/// so `run-core` wraps this into its own `BeforeLaunchTask::ExternalTool`,
/// the same split `container_run::build_task` already draws for C5's
/// containerfile configurations.
pub fn before_launch_for(
    target: &ContainerTargetSetting,
    invocation: &Invocation,
) -> Option<(String, Vec<String>)> {
    match target.source.as_str() {
        "containerfile" => {
            let setting = ContainerfileRunSetting {
                dockerfile: target.dockerfile.clone().unwrap_or_default(),
                context_dir: target.context_dir.clone().unwrap_or_default(),
                image_tag: image_tag_for(target),
                ..ContainerfileRunSetting::default()
            };
            let argv = run_config::containerfile_build_argv(&setting);
            Some((
                invocation.program.clone(),
                invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
            ))
        }
        "compose-service" if target.needs_build => {
            let mut argv = compose_prefix(target);
            argv.push("build".to_string());
            argv.push(target.service.clone().unwrap_or_default());
            Some((
                invocation.program.clone(),
                invocation.argv(&argv.iter().map(String::as_str).collect::<Vec<_>>()),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{ConnectionConfig, Engine};
    use app_config::container_run::{BindMount, PortBinding};
    use process_exec::host::WslHost;

    fn local_invocation() -> Invocation {
        ConnectionConfig {
            engine: Engine::Docker,
            kind: crate::connection::ConnectionKind::Auto,
            executable: None,
            compose_executable: None,
        }
        .invocation()
    }

    fn image_target() -> ContainerTargetSetting {
        ContainerTargetSetting {
            id: "t1".to_string(),
            name: "nginx".to_string(),
            source: "image".to_string(),
            image: Some("nginx:1.27".to_string()),
            workdir: String::new(),
            ..ContainerTargetSetting::default()
        }
    }

    fn spec<'a>(cwd: Option<&'a Path>, env: &'a [(String, String)]) -> SimpleLaunch<'a> {
        SimpleLaunch {
            program: "python3",
            args: &[],
            cwd,
            env,
        }
    }

    // -------------------------------------------------------- PathMap ----

    #[test]
    fn to_remote_maps_a_project_relative_path_onto_the_workdir() {
        let map = PathMap::new("/home/f/proj", "/workspace");
        assert_eq!(
            map.to_remote(Path::new("/home/f/proj/src/main.rs")),
            Some("/workspace/src/main.rs".to_string())
        );
        assert_eq!(
            map.to_remote(Path::new("/home/f/proj")),
            Some("/workspace".to_string())
        );
    }

    #[test]
    fn to_remote_is_none_outside_the_project_root() {
        let map = PathMap::new("/home/f/proj", "/workspace");
        assert_eq!(map.to_remote(Path::new("/home/f/other/file")), None);
    }

    #[test]
    fn to_local_is_the_inverse_of_to_remote() {
        let map = PathMap::new("/home/f/proj", "/workspace");
        let remote = map
            .to_remote(Path::new("/home/f/proj/src/main.rs"))
            .unwrap();
        assert_eq!(
            map.to_local(&remote),
            Some(PathBuf::from("/home/f/proj/src/main.rs"))
        );
        assert_eq!(
            map.to_local("/workspace"),
            Some(PathBuf::from("/home/f/proj"))
        );
    }

    #[test]
    fn to_local_is_none_outside_the_mount_root() {
        let map = PathMap::new("/home/f/proj", "/workspace");
        assert_eq!(map.to_local("/etc/passwd"), None);
    }

    #[test]
    fn a_windows_local_root_maps_case_and_separator_insensitively() {
        let map = PathMap::new(r"C:\proj", "/workspace");
        assert_eq!(
            map.to_remote(Path::new(r"c:/proj/src/main.rs")),
            Some("/workspace/src/main.rs".to_string())
        );
        assert_eq!(
            map.to_local("/workspace/src/main.rs"),
            Some(PathBuf::from(r"C:\proj\src\main.rs"))
        );
    }

    #[test]
    fn empty_workdir_defaults_to_slash_workspace() {
        let map = PathMap::new("/p", "");
        assert_eq!(map.remote_root, "/workspace");
    }

    // ------------------------------------------------------ wrap_launch --

    #[test]
    fn an_image_target_wraps_docker_run_with_the_mount_and_workdir() {
        let project_root = Path::new("/home/f/proj");
        let launch = spec(Some(project_root), &[]);
        let target = image_target();
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();

        assert_eq!(wrapped.program, "docker");
        assert_eq!(
            wrapped.args,
            vec![
                "run",
                "--rm",
                "-i",
                "-t",
                "-v",
                "/home/f/proj:/workspace",
                "-w",
                "/workspace",
                "nginx:1.27",
                "python3",
            ]
        );
        assert_eq!(wrapped.path_map.remote_root, "/workspace");
    }

    #[test]
    fn selinux_relabel_applies_to_the_workspace_mount_and_extra_mounts() {
        let project_root = Path::new("/home/f/proj");
        let launch = spec(Some(project_root), &[]);
        let mut target = image_target();
        target.extra_mounts = vec![app_config::container_run::BindMount {
            host_path: "/home/f/data".to_string(),
            container_path: "/data".to_string(),
            read_only: true,
        }];
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, true).unwrap();

        assert!(
            wrapped
                .args
                .contains(&"/home/f/proj:/workspace:z".to_string()),
            "{:?}",
            wrapped.args
        );
        assert!(
            wrapped
                .args
                .contains(&"/home/f/data:/data:ro,z".to_string()),
            "{:?}",
            wrapped.args
        );
    }

    #[test]
    fn a_relative_cwd_under_the_project_is_rebased_under_the_workdir() {
        let project_root = Path::new("/home/f/proj");
        let cwd = project_root.join("backend");
        let launch = spec(Some(&cwd), &[]);
        let target = image_target();
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();
        assert!(wrapped.args.contains(&"/workspace/backend".to_string()));
    }

    #[test]
    fn a_cwd_outside_the_project_is_refused() {
        let project_root = Path::new("/home/f/proj");
        let launch = spec(Some(Path::new("/home/f/elsewhere")), &[]);
        let target = image_target();
        let err =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap_err();
        assert_eq!(
            err,
            TargetError::CwdOutsideProject(PathBuf::from("/home/f/elsewhere"))
        );
    }

    #[test]
    fn env_values_that_are_project_paths_are_rebased() {
        let project_root = Path::new("/home/f/proj");
        let env = vec![("DATA_DIR".to_string(), "/home/f/proj/data".to_string())];
        let launch = spec(Some(project_root), &env);
        let target = image_target();
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();
        assert!(wrapped
            .args
            .contains(&"DATA_DIR=/workspace/data".to_string()));
    }

    #[test]
    fn env_values_unrelated_to_the_project_pass_through_unchanged() {
        let project_root = Path::new("/home/f/proj");
        let env = vec![("LOG_LEVEL".to_string(), "debug".to_string())];
        let launch = spec(Some(project_root), &env);
        let target = image_target();
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();
        assert!(wrapped.args.contains(&"LOG_LEVEL=debug".to_string()));
    }

    #[test]
    fn port_bindings_publish_all_and_extra_mounts_all_compile() {
        let project_root = Path::new("/p");
        let launch = spec(Some(project_root), &[]);
        let target = ContainerTargetSetting {
            publish_all_ports: true,
            port_bindings: vec![PortBinding {
                host_port: "8080".to_string(),
                container_port: "80".to_string(),
                ..Default::default()
            }],
            extra_mounts: vec![BindMount {
                host_path: "/data".to_string(),
                container_path: "/data".to_string(),
                read_only: true,
            }],
            ..image_target()
        };
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();
        assert!(wrapped.args.contains(&"-P".to_string()));
        assert!(wrapped.args.contains(&"8080:80".to_string()));
        assert!(wrapped.args.contains(&"/data:/data:ro".to_string()));
    }

    #[test]
    fn run_options_are_shell_word_split_and_appended() {
        let project_root = Path::new("/p");
        let launch = spec(Some(project_root), &[]);
        let target = ContainerTargetSetting {
            run_options: "--label foo=bar".to_string(),
            ..image_target()
        };
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();
        let pos = wrapped
            .args
            .iter()
            .position(|a| a == "--label")
            .expect("--label present");
        assert_eq!(wrapped.args[pos + 1], "foo=bar");
    }

    #[test]
    fn a_containerfile_target_runs_its_image_tag() {
        let project_root = Path::new("/p");
        let launch = spec(Some(project_root), &[]);
        let target = ContainerTargetSetting {
            source: "containerfile".to_string(),
            dockerfile: Some("Dockerfile".to_string()),
            context_dir: Some(".".to_string()),
            image_tag: Some("myapp:dev".to_string()),
            ..image_target()
        };
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();
        assert!(wrapped.args.contains(&"myapp:dev".to_string()));
    }

    #[test]
    fn a_containerfile_target_with_no_tag_gets_a_deterministic_one() {
        let target = ContainerTargetSetting {
            id: "abc".to_string(),
            source: "containerfile".to_string(),
            image_tag: None,
            ..image_target()
        };
        assert_eq!(image_tag_for(&target), "ide-target-abc");
    }

    #[test]
    fn a_compose_service_target_wraps_compose_run() {
        let project_root = Path::new("/p");
        let launch = spec(Some(project_root), &[]);
        let target = ContainerTargetSetting {
            source: "compose-service".to_string(),
            compose_files: vec!["docker-compose.yml".to_string()],
            service: Some("web".to_string()),
            image: None,
            ..image_target()
        };
        let wrapped =
            wrap_launch(&launch, &target, &local_invocation(), project_root, false).unwrap();
        assert_eq!(
            wrapped.args,
            vec![
                "compose",
                "-f",
                "docker-compose.yml",
                "run",
                "--rm",
                "--service-ports",
                "-w",
                "/workspace",
                "web",
                "python3",
            ]
        );
    }

    // ------------------------------------------------------ before_launch ----

    #[test]
    fn an_image_target_has_no_before_launch_task() {
        assert!(before_launch_for(&image_target(), &local_invocation()).is_none());
    }

    #[test]
    fn a_containerfile_target_builds_with_its_own_tag_dockerfile_and_context() {
        let target = ContainerTargetSetting {
            source: "containerfile".to_string(),
            dockerfile: Some("Dockerfile".to_string()),
            context_dir: Some(".".to_string()),
            image_tag: Some("myapp:dev".to_string()),
            ..image_target()
        };
        let (program, args) = before_launch_for(&target, &local_invocation()).unwrap();
        assert_eq!(program, "docker");
        assert_eq!(
            args,
            vec!["build", "-f", "./Dockerfile", "-t", "myapp:dev", "."]
        );
    }

    #[test]
    fn a_compose_service_target_builds_only_when_needs_build_is_set() {
        let mut target = ContainerTargetSetting {
            source: "compose-service".to_string(),
            compose_files: vec!["docker-compose.yml".to_string()],
            service: Some("web".to_string()),
            needs_build: false,
            ..image_target()
        };
        assert!(before_launch_for(&target, &local_invocation()).is_none());

        target.needs_build = true;
        let (program, args) = before_launch_for(&target, &local_invocation()).unwrap();
        assert_eq!(program, "docker");
        assert_eq!(
            args,
            vec!["compose", "-f", "docker-compose.yml", "build", "web"]
        );
    }

    // --------------------------------------------------------- WSL host --

    #[test]
    fn a_wsl_connection_mounts_the_distros_own_path() {
        let invocation = Invocation {
            program: "docker".to_string(),
            prefix_args: vec![
                "-d".to_string(),
                "Ubuntu".to_string(),
                "--".to_string(),
                "docker".to_string(),
            ],
            env: Vec::new(),
            host: ExecHost::Wsl(WslHost {
                distro: "Ubuntu".to_string(),
                unc_prefix: r"\\wsl.localhost\Ubuntu".to_string(),
            }),
        };
        let project_root = Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj");
        let launch = spec(Some(project_root), &[]);
        let target = image_target();
        let wrapped = wrap_launch(&launch, &target, &invocation, project_root, false).unwrap();
        assert!(wrapped.args.iter().any(|a| a == "/home/f/proj:/workspace"));
    }

    // ------------------------------------------------------ exec_host ----

    fn compose_target() -> ContainerTargetSetting {
        ContainerTargetSetting {
            source: "compose-service".to_string(),
            compose_files: vec!["docker-compose.yml".to_string()],
            service: Some("php".to_string()),
            image: None,
            workdir: "/var/www".to_string(),
            ..image_target()
        }
    }

    fn podman_invocation() -> Invocation {
        ConnectionConfig {
            engine: Engine::Podman,
            kind: crate::connection::ConnectionKind::Auto,
            executable: None,
            compose_executable: None,
        }
        .invocation()
    }

    fn argv_of(host: &ExecHost, program: &str, args: &[&str], cwd: &Path) -> Vec<String> {
        let (engine, mut argv) = host.argv(program, args, cwd);
        argv.insert(0, engine);
        argv
    }

    #[test]
    fn a_compose_service_defaults_to_exec() {
        let root = Path::new("/home/f/proj");
        let host = exec_host(&compose_target(), &local_invocation(), root, None, false);
        assert_eq!(
            argv_of(&host, "vendor/bin/phpstan", &["analyse"], &root.join("src")),
            [
                "docker",
                "compose",
                "-f",
                "docker-compose.yml",
                "exec",
                "-T",
                "-w",
                "/var/www/src",
                "php",
                "vendor/bin/phpstan",
                "analyse"
            ]
        );
    }

    #[test]
    fn a_compose_service_can_run_a_fresh_container_instead() {
        let root = Path::new("/home/f/proj");
        let host = exec_host(
            &compose_target(),
            &local_invocation(),
            root,
            Some(ExecMode::Run),
            false,
        );
        assert_eq!(
            argv_of(&host, "php", &["-v"], root),
            [
                "docker",
                "compose",
                "-f",
                "docker-compose.yml",
                "run",
                "--rm",
                "-T",
                "-w",
                "/var/www",
                "php",
                "php",
                "-v"
            ]
        );
    }

    #[test]
    fn podman_uses_its_own_program() {
        let root = Path::new("/home/f/proj");
        let host = exec_host(&compose_target(), &podman_invocation(), root, None, false);
        assert_eq!(argv_of(&host, "php", &[], root)[..2], ["podman", "compose"]);
    }

    #[test]
    fn an_image_always_runs_with_the_project_mounted() {
        let root = Path::new("/home/f/proj");
        // An explicit `exec` on an image has nothing to exec into.
        let host = exec_host(
            &image_target(),
            &local_invocation(),
            root,
            Some(ExecMode::Exec),
            false,
        );
        assert_eq!(
            argv_of(&host, "php", &["-r", "1;"], &root.join("app")),
            [
                "docker",
                "run",
                "--rm",
                "-i",
                "-v",
                "/home/f/proj:/workspace",
                "-w",
                "/workspace/app",
                "nginx:1.27",
                "php",
                "-r",
                "1;"
            ]
        );
    }

    #[test]
    fn an_image_target_carries_extra_mounts_options_and_selinux_labels() {
        let root = Path::new("/home/f/proj");
        let mut target = image_target();
        target.run_options = "--network host".to_string();
        target.extra_mounts = vec![BindMount {
            host_path: "/home/f/.composer".to_string(),
            container_path: "/root/.composer".to_string(),
            read_only: true,
        }];
        let host = exec_host(&target, &local_invocation(), root, None, true);
        let argv = argv_of(&host, "php", &[], root);
        assert!(
            argv.contains(&"/home/f/proj:/workspace:z".to_string()),
            "{argv:?}"
        );
        assert!(argv.contains(&"/home/f/.composer:/root/.composer:ro,z".to_string()));
        assert!(argv.windows(2).any(|w| w == ["--network", "host"]));
    }

    #[test]
    fn a_containerfile_target_runs_its_built_tag() {
        let root = Path::new("/p");
        let target = ContainerTargetSetting {
            source: "containerfile".to_string(),
            image_tag: Some("myapp:dev".to_string()),
            ..image_target()
        };
        let host = exec_host(&target, &local_invocation(), root, None, false);
        assert!(argv_of(&host, "php", &[], root).contains(&"myapp:dev".to_string()));
    }

    #[test]
    fn a_wsl_connection_mounts_the_distros_path_and_marks_the_engine_as_wsl() {
        let invocation = Invocation {
            program: "wsl.exe".to_string(),
            prefix_args: vec!["-d".into(), "Ubuntu".into(), "--".into(), "docker".into()],
            env: Vec::new(),
            host: ExecHost::Wsl(WslHost {
                distro: "Ubuntu".to_string(),
                unc_prefix: r"\\wsl.localhost\Ubuntu".to_string(),
            }),
        };
        let root = Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj");
        let host = exec_host(&image_target(), &invocation, root, None, false);
        let ExecHost::Container(container) = &host else {
            panic!("expected a container host")
        };
        assert!(container.via_wsl);
        assert_eq!(container.program, "wsl.exe");
        assert!(container
            .verb_args
            .contains(&"/home/f/proj:/workspace".to_string()));
        // A path the tool prints under the mount opens the UNC file.
        assert_eq!(
            host.path_from_tool("/workspace/src/A.php"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\f\proj\src\A.php")
        );
    }

    #[test]
    fn exec_host_for_resolves_target_and_connection_from_settings() {
        let containers = ContainerSettings {
            targets: vec![compose_target()],
            ..ContainerSettings::default()
        };
        let root = Path::new("/p");
        let host = exec_host_for(&containers, &compose_target().id, root, None).unwrap();
        assert_eq!(argv_of(&host, "php", &[], root)[0], "docker");
        assert!(exec_host_for(&containers, "missing", root, None).is_none());
    }
}
