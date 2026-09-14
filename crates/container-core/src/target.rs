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
//! No `process_exec::host::ExecHost::Container` variant: that enum is
//! path-derived (`ExecHost::for_path`) and every seam that uses it classifies
//! a filesystem path, which a container connection is not one of — the plan
//! documents this as a seam fact. [`wrap_launch`] instead rewrites the
//! launch spec itself, in `run-core`, before `Supervisor::launch` ever sees
//! it.

use std::path::{Path, PathBuf};

use app_config::container_run::ContainerfileRunSetting;
use app_config::ContainerTargetSetting;

use crate::connection::Invocation;
use crate::run_config::{self, mount_arg, port_arg, split_shell_words};

/// Where the project root mounts inside a target's container when the
/// setting leaves [`ContainerTargetSetting::workdir`] empty.
pub const DEFAULT_WORKDIR: &str = "/workspace";

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
///
/// `local_root` is kept exactly as the IDE already knows the project root
/// (a plain path, or a Windows UNC path for a WSL-hosted project — see
/// [`wrap_launch`]'s own doc comment on the WSL case) — matching is an exact
/// prefix match after normalising separators, never a filesystem lookup, so
/// this stays a pure, cheaply-testable mapping the way
/// `process_exec::host::ExecHost::to_remote`/`to_local` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMap {
    pub local_root: PathBuf,
    pub remote_root: String,
}

/// Normalise a path string for prefix comparison: backslashes to forward
/// slashes, and (Windows-safe) a leading drive letter lower-cased — `C:\`
/// and `c:/` must match the same root.
fn normalize(path: &str) -> String {
    let out = path.replace('\\', "/");
    let mut chars = out.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            format!("{}:{}", drive.to_ascii_lowercase(), &out[2..])
        }
        _ => out,
    }
}

impl PathMap {
    /// `local_root` joined with `remote_root` defaulted to
    /// [`DEFAULT_WORKDIR`] when `workdir` is empty.
    pub fn new(local_root: impl Into<PathBuf>, workdir: &str) -> Self {
        let remote_root = if workdir.is_empty() {
            DEFAULT_WORKDIR.to_string()
        } else {
            workdir.to_string()
        };
        PathMap {
            local_root: local_root.into(),
            remote_root,
        }
    }

    /// `path` under [`Self::local_root`] -> the same path under
    /// [`Self::remote_root`]. `None` when `path` is not under the root at
    /// all — the caller decides whether that is an error
    /// ([`TargetError::CwdOutsideProject`]) or simply "leave it alone" (an
    /// env value that happens not to be a project path).
    pub fn to_remote(&self, path: &Path) -> Option<String> {
        let root = normalize(&self.local_root.to_string_lossy());
        let candidate = normalize(&path.to_string_lossy());
        let tail = if candidate.eq_ignore_ascii_case(&root) {
            ""
        } else {
            let prefix = if root.ends_with('/') {
                root.clone()
            } else {
                format!("{root}/")
            };
            if candidate.len() >= prefix.len()
                && candidate[..prefix.len()].eq_ignore_ascii_case(&prefix)
            {
                &candidate[prefix.len()..]
            } else {
                return None;
            }
        };
        if tail.is_empty() {
            Some(self.remote_root.clone())
        } else {
            Some(format!("{}/{tail}", self.remote_root.trim_end_matches('/')))
        }
    }

    /// The inverse of [`Self::to_remote`]: `remote` under
    /// [`Self::remote_root`] -> the same path under [`Self::local_root`],
    /// played back with `local_root`'s own separator style — the same
    /// "canonical to the root's own spelling" rule
    /// `process_exec::host::ExecHost::to_local` documents. `None` when
    /// `remote` is not under [`Self::remote_root`].
    pub fn to_local(&self, remote: &str) -> Option<PathBuf> {
        let root = self.remote_root.trim_end_matches('/');
        let candidate = remote.trim_end_matches('/');
        let tail = if candidate == root {
            ""
        } else {
            let prefix = format!("{root}/");
            candidate.strip_prefix(prefix.as_str())?
        };
        let local = self.local_root.to_string_lossy();
        let sep = if local.contains('\\') { '\\' } else { '/' };
        if tail.is_empty() {
            Some(self.local_root.clone())
        } else {
            let tail = tail.replace('/', &sep.to_string());
            let joined = if local.ends_with(sep) {
                format!("{local}{tail}")
            } else {
                format!("{local}{sep}{tail}")
            };
            Some(PathBuf::from(joined))
        }
    }
}

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
    if invocation.host.is_remote() {
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
        for file in &target.compose_files {
            argv.push("-f".to_string());
            argv.push(file.clone());
        }
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
            let mut argv = Vec::new();
            for file in &target.compose_files {
                argv.push("-f".to_string());
                argv.push(file.clone());
            }
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
    use process_exec::host::{ExecHost, WslHost};

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
        assert_eq!(args, vec!["-f", "docker-compose.yml", "build", "web"]);
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
}
