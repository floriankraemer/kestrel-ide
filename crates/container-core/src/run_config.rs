//! Run-configuration argv compilers (C5, ADR-0056): every container-kind
//! run configuration's option struct — [`app_config::container_run::ContainerImageRunSetting`],
//! [`app_config::container_run::ContainerfileRunSetting`], [`app_config::container_run::ComposeRunSetting`] —
//! turned into the exact argv JetBrains' own option tables name, plus a
//! shell-quoted [`preview`] string for the dialog's "Command preview" and a
//! `compose config --services` runner for the Services picker.
//!
//! Engine-agnostic on purpose, like [`crate::ops`]: every flag here is one
//! `docker`/`podman` (and `docker compose`/`podman compose`) already share
//! byte for byte (ADR-0055). `run-core` is the one caller that prepends the
//! engine program itself (via a connection's [`crate::connection::Invocation`]).

use std::path::Path;

use app_config::container_run::{
    ComposeRunSetting, ContainerImageRunSetting, ContainerfileRunSetting,
};

use crate::connection::Invocation;
use crate::ops::{run_op, OpError};

/// Split a free-form options string (`run_options`, `build_options`) into
/// argv the way a POSIX shell's word-splitting would, honouring single and
/// double quotes and a backslash escape — enough for the flags a user
/// actually pastes in here (`--label foo="a b"`, `-v '/a b/:/x'`), not a
/// full shell grammar (no globbing, no `$VAR`, no `;`). An unterminated
/// quote is closed implicitly at end of string rather than treated as an
/// error: this is a preview-and-run text field, not a script.
pub fn split_shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if let Some(&next) = chars.peek() {
                        current.push(next);
                        chars.next();
                    }
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    in_word = true;
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                        in_word = true;
                    }
                }
                c if c.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                c => {
                    current.push(c);
                    in_word = true;
                }
            },
        }
    }
    if in_word || quote.is_some() {
        words.push(current);
    }
    words
}

/// Shell-quote one argv token for display: unquoted when it needs nothing,
/// single-quoted (with an embedded `'` escaped as `'\''`) otherwise. Never
/// executed — only ever fed to [`preview`] — so this only has to *look*
/// right, not round-trip through a real shell.
pub(crate) fn quote(word: &str) -> String {
    let needs_quoting = word.is_empty()
        || !word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=,@%+".contains(c));
    if !needs_quoting {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// One line: `argv`, shell-quoted and space-joined — the dialog's "Command
/// preview".
pub fn preview(argv: &[String]) -> String {
    argv.iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One `-v`/`--mount` bind-mount argument. The SELinux `:z` suffix rule
/// itself lives in [`crate::selinux::relabel_suffix`] — shared with
/// [`crate::target::wrap_launch`] and [`crate::recreate::recreate_argv`]
/// (C9) so the three argv builders that emit bind mounts cannot drift.
pub(crate) fn mount_arg(
    host_path: &str,
    container_path: &str,
    read_only: bool,
    selinux_relabel: bool,
) -> String {
    let mut spec = format!("{host_path}:{container_path}");
    let mut options = Vec::new();
    if read_only {
        options.push("ro");
    }
    if selinux_relabel {
        if let Some(suffix) = crate::selinux::relabel_suffix(host_path) {
            options.push(suffix);
        }
    }
    if !options.is_empty() {
        spec.push(':');
        spec.push_str(&options.join(","));
    }
    spec
}

/// One `-p`/`--publish` port-binding argument:
/// `[host_ip:]host_port:container_port[/protocol]`.
pub(crate) fn port_arg(binding: &app_config::container_run::PortBinding) -> String {
    let mut spec = String::new();
    if !binding.host_ip.is_empty() {
        spec.push_str(&binding.host_ip);
        spec.push(':');
    }
    spec.push_str(&binding.host_port);
    spec.push(':');
    spec.push_str(&binding.container_port);
    if binding.protocol == "udp" {
        spec.push_str("/udp");
    }
    spec
}

/// `pull_policy`'s persisted string, defaulted the way an unrecognised or
/// empty value should read: `missing`, the CLI's own default.
fn pull_policy_or_default(pull_policy: &str) -> &str {
    match pull_policy {
        "always" | "never" => pull_policy,
        _ => "missing",
    }
}

/// Shared body of [`image_run_argv`] and the run half of
/// [`containerfile_build_argv`]'s caller (`run-core`'s `to_launch_spec_in`):
/// every flag both an Image and a Containerfile configuration's run step
/// share, ending just before the image reference and command.
#[allow(clippy::too_many_arguments)]
fn run_flags(
    container_name: &str,
    publish_all_ports: bool,
    port_bindings: &[app_config::container_run::PortBinding],
    entrypoint: &str,
    bind_mounts: &[app_config::container_run::BindMount],
    env: &[(String, String)],
    attach: bool,
    pull_policy: &str,
    run_options: &str,
    selinux_relabel: bool,
) -> Vec<String> {
    let mut args = vec!["run".to_string()];
    args.push(if attach {
        "-a".to_string()
    } else {
        "-d".to_string()
    });
    if !container_name.is_empty() {
        args.push("--name".to_string());
        args.push(container_name.to_string());
    }
    if publish_all_ports {
        args.push("-P".to_string());
    }
    for binding in port_bindings {
        args.push("-p".to_string());
        args.push(port_arg(binding));
    }
    if !entrypoint.is_empty() {
        args.push("--entrypoint".to_string());
        args.push(entrypoint.to_string());
    }
    for mount in bind_mounts {
        args.push("-v".to_string());
        args.push(mount_arg(
            &mount.host_path,
            &mount.container_path,
            mount.read_only,
            selinux_relabel,
        ));
    }
    for (key, value) in env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    args.push("--pull".to_string());
    args.push(pull_policy_or_default(pull_policy).to_string());
    args.extend(split_shell_words(run_options));
    args
}

/// `run [-d|-a] [--name …] [-P] [-p …] [--entrypoint …] [-v …] [-e …]
/// --pull <policy> <run_options…> <image> <command…>` — the "Docker Image"
/// / "Container Image" configuration.
pub fn image_run_argv(
    setting: &ContainerImageRunSetting,
    _project_root: &Path,
    selinux_relabel: bool,
) -> Vec<String> {
    let mut args = run_flags(
        &setting.container_name,
        setting.publish_all_ports,
        &setting.port_bindings,
        &setting.entrypoint,
        &setting.bind_mounts,
        &setting.env,
        setting.attach,
        &setting.pull_policy,
        &setting.run_options,
        selinux_relabel,
    );
    args.push(setting.image.clone());
    args.extend(setting.command.iter().cloned());
    args
}

/// The run half of a "Dockerfile"/"Containerfile" configuration: the same
/// shape as [`image_run_argv`], with the just-built [`ContainerfileRunSetting::image_tag`]
/// as the image reference.
pub fn containerfile_run_argv(
    setting: &ContainerfileRunSetting,
    selinux_relabel: bool,
) -> Vec<String> {
    let mut args = run_flags(
        &setting.container_name,
        setting.publish_all_ports,
        &setting.port_bindings,
        &setting.entrypoint,
        &setting.bind_mounts,
        &setting.env,
        setting.attach,
        &setting.pull_policy,
        &setting.run_options,
        selinux_relabel,
    );
    args.push(setting.image_tag.clone());
    args.extend(setting.command.iter().cloned());
    args
}

/// `build -f <context_dir>/<dockerfile> -t <image_tag> [--build-arg k=v]…
/// <build_options…> <context_dir>` — the Containerfile configuration's
/// before-launch build step.
pub fn containerfile_build_argv(setting: &ContainerfileRunSetting) -> Vec<String> {
    let context_dir = if setting.context_dir.is_empty() {
        ".".to_string()
    } else {
        setting.context_dir.clone()
    };
    let dockerfile = if setting.dockerfile.is_empty() {
        format!("{context_dir}/Dockerfile")
    } else if Path::new(&setting.dockerfile).is_absolute() {
        setting.dockerfile.clone()
    } else {
        format!("{context_dir}/{}", setting.dockerfile)
    };

    let mut args = vec!["build".to_string(), "-f".to_string(), dockerfile];
    if !setting.image_tag.is_empty() {
        args.push("-t".to_string());
        args.push(setting.image_tag.clone());
    }
    for (key, value) in &setting.build_args {
        args.push("--build-arg".to_string());
        args.push(format!("{key}={value}"));
    }
    args.extend(split_shell_words(&setting.build_options));
    args.push(context_dir);
    args
}

/// `-f <file>` per compose file, then `-p <project_name>`, `--profile
/// <name>` per profile, `--env-file <file>` per env file, and
/// `--compatibility` — the flags every compose subcommand
/// ([`compose_up_argv`], [`compose_down_argv`], [`compose_stop_argv`],
/// [`compose_services`]) shares.
fn compose_common_args(setting: &ComposeRunSetting) -> Vec<String> {
    let mut args = vec!["compose".to_string()];
    for file in &setting.compose_files {
        args.push("-f".to_string());
        args.push(file.clone());
    }
    if !setting.project_name.is_empty() {
        args.push("-p".to_string());
        args.push(setting.project_name.clone());
    }
    for profile in &setting.profiles {
        args.push("--profile".to_string());
        args.push(profile.clone());
    }
    for env_file in &setting.env_files {
        args.push("--env-file".to_string());
        args.push(env_file.clone());
    }
    if setting.compatibility {
        args.push("--compatibility".to_string());
    }
    args
}

/// `<compose_common> up -d [--no-start|--no-deps] [-d|--attach-dependencies]
/// [--force-recreate|--no-recreate] [--build|--no-build]
/// [--always-recreate-deps] [-V] [--remove-orphans] [--no-log-prefix]
/// [--abort-on-container-exit] [--scale svc=n]… [services…]`.
pub fn compose_up_argv(setting: &ComposeRunSetting, _project_root: &Path) -> Vec<String> {
    let mut args = compose_common_args(setting);
    args.push("up".to_string());
    args.push("-d".to_string());

    match setting.start.as_str() {
        "none" => args.push("--no-start".to_string()),
        "selected_only" => args.push("--no-deps".to_string()),
        _ => {} // "selected_and_deps", the default
    }
    match setting.attach.as_str() {
        "none" => {} // -d already means detached; nothing more to add
        "selected_and_deps" => args.push("--attach-dependencies".to_string()),
        _ => {} // "selected", the default
    }
    match setting.recreate.as_str() {
        "all" => args.push("--force-recreate".to_string()),
        "none" => args.push("--no-recreate".to_string()),
        _ => {} // "changed", the default
    }
    match setting.build.as_str() {
        "never" => args.push("--no-build".to_string()),
        "always" => args.push("--build".to_string()),
        _ => {} // "missing", the default
    }
    if setting.always_recreate_deps {
        args.push("--always-recreate-deps".to_string());
    }
    if setting.renew_anon_volumes {
        args.push("-V".to_string());
    }
    if setting.remove_orphans {
        args.push("--remove-orphans".to_string());
    }
    if setting.no_log_prefix {
        args.push("--no-log-prefix".to_string());
    }
    if setting.abort_on_container_exit {
        args.push("--abort-on-container-exit".to_string());
    }
    if let Some(exit_from) = &setting.exit_code_from {
        args.push("--exit-code-from".to_string());
        args.push(exit_from.clone());
    }
    if let Some(timeout) = setting.sigkill_timeout {
        args.push("-t".to_string());
        args.push(timeout.to_string());
    }
    for (service, count) in &setting.scale {
        args.push("--scale".to_string());
        args.push(format!("{service}={count}"));
    }
    for (key, value) in &setting.env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    args.extend(setting.services.iter().cloned());
    args
}

/// `<compose_common> down [--remove-orphans] [-v] [--rmi all|local]`.
pub fn compose_down_argv(setting: &ComposeRunSetting) -> Vec<String> {
    let mut args = compose_common_args(setting);
    args.push("down".to_string());
    if setting.remove_orphans_on_down {
        args.push("--remove-orphans".to_string());
    }
    if setting.remove_volumes_on_down {
        args.push("-v".to_string());
    }
    match setting.remove_images_on_down.as_str() {
        "all" => {
            args.push("--rmi".to_string());
            args.push("all".to_string());
        }
        "local" => {
            args.push("--rmi".to_string());
            args.push("local".to_string());
        }
        _ => {} // "none", the default: nothing removed
    }
    args
}

/// `<compose_common> stop`.
pub fn compose_stop_argv(setting: &ComposeRunSetting) -> Vec<String> {
    let mut args = compose_common_args(setting);
    args.push("stop".to_string());
    args
}

/// `compose -f <files>… up -d --scale <service>=<n> --no-recreate` — the
/// tree's per-service "Scale…" action against an already-running project.
/// A standalone `up`, not `RunConfigSetting::compose.scale`, since scaling
/// one service from the tree must not also touch the run configuration's
/// own persisted service list or its other options.
pub fn compose_scale_argv(files: &[String], service: &str, count: u32) -> Vec<String> {
    let mut args = vec!["compose".to_string()];
    for file in files {
        args.push("-f".to_string());
        args.push(file.clone());
    }
    args.push("up".to_string());
    args.push("-d".to_string());
    args.push("--no-recreate".to_string());
    args.push("--scale".to_string());
    args.push(format!("{service}={count}"));
    args
}

/// The first problem with `bindings` that would stop `-p` compiling to
/// something the CLI accepts: a row with either port left blank.
fn validate_ports(bindings: &[app_config::container_run::PortBinding]) -> Option<String> {
    bindings
        .iter()
        .any(|p| p.host_port.trim().is_empty() || p.container_port.trim().is_empty())
        .then(|| "Each port binding needs a host port and a container port".to_string())
}

/// The first problem with `mounts` that would stop `-v` compiling: a row
/// with either path left blank.
fn validate_mounts(mounts: &[app_config::container_run::BindMount]) -> Option<String> {
    mounts
        .iter()
        .any(|m| m.host_path.trim().is_empty() || m.container_path.trim().is_empty())
        .then(|| "Each bind mount needs a host path and a container path".to_string())
}

/// The first problem with an Image configuration that would stop it
/// launching — the run-config dialog's validation (C5, ADR-0056). `None`
/// means it is savable.
pub fn validate_image(setting: &ContainerImageRunSetting) -> Option<String> {
    if setting.image.trim().is_empty() {
        return Some("Image reference must not be empty".to_string());
    }
    validate_ports(&setting.port_bindings).or_else(|| validate_mounts(&setting.bind_mounts))
}

/// The first problem with a Containerfile configuration, or `None`.
pub fn validate_containerfile(setting: &ContainerfileRunSetting) -> Option<String> {
    if setting.run_built_image && setting.image_tag.trim().is_empty() {
        return Some("Image tag must not be empty to run the built image".to_string());
    }
    validate_ports(&setting.port_bindings).or_else(|| validate_mounts(&setting.bind_mounts))
}

/// The first problem with a Compose configuration, or `None`.
pub fn validate_compose(setting: &ComposeRunSetting) -> Option<String> {
    if setting.compose_files.iter().all(|f| f.trim().is_empty()) {
        return Some("At least one compose file is required".to_string());
    }
    None
}

/// `compose -f <files>… config --services` — the ground-truth service list
/// for the dialog's Services picker and `composeStartAll`'s temporary
/// configuration.
pub fn compose_services(
    invocation: &Invocation,
    files: &[String],
    project_root: &Path,
) -> Result<Vec<String>, OpError> {
    let mut args = vec!["compose".to_string()];
    for file in files {
        args.push("-f".to_string());
        args.push(file.clone());
    }
    args.push("config".to_string());
    args.push("--services".to_string());
    let output = run_op(invocation, &args, project_root)?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Whether `service` has a `build:` key, per `compose -f <files>… config
/// --format json` — the New Target wizard's own answer to "does this
/// compose-service target need a before-launch build" (C8), computed once
/// at wizard time and stored on `ContainerTargetSetting::needs_build` rather
/// than re-derived on every launch (see that field's own doc comment: a
/// `compose config` call on every run would make every launch depend on the
/// daemon being reachable just to decide whether to build).
pub fn compose_service_needs_build(
    invocation: &Invocation,
    files: &[String],
    service: &str,
    project_root: &Path,
) -> Result<bool, OpError> {
    let mut args = vec!["compose".to_string()];
    for file in files {
        args.push("-f".to_string());
        args.push(file.clone());
    }
    args.push("config".to_string());
    args.push("--format".to_string());
    args.push("json".to_string());
    let output = run_op(invocation, &args, project_root)?;
    Ok(service_has_build_key(&output.stdout, service))
}

/// The pure half of [`compose_service_needs_build`]: does `service` have a
/// `build:` key in `compose config --format json`'s output? Separated out
/// so this is table-testable without spawning a real `compose`.
/// Malformed/unexpected JSON reads as "no", the same "absent means false"
/// default this whole feature follows.
fn service_has_build_key(compose_config_json: &[u8], service: &str) -> bool {
    let value: serde_json::Value =
        serde_json::from_slice(compose_config_json).unwrap_or(serde_json::Value::Null);
    value
        .get("services")
        .and_then(|services| services.get(service))
        .and_then(|svc| svc.get("build"))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_config::container_run::{BindMount, PortBinding};

    fn image(f: impl FnOnce(&mut ContainerImageRunSetting)) -> ContainerImageRunSetting {
        let mut setting = ContainerImageRunSetting {
            image: "nginx:1.27".to_string(),
            ..Default::default()
        };
        f(&mut setting);
        setting
    }

    #[test]
    fn a_bare_image_runs_detached_with_the_default_pull_policy() {
        let argv = image_run_argv(&image(|_| {}), Path::new("/p"), false);
        assert_eq!(argv, vec!["run", "-d", "--pull", "missing", "nginx:1.27"]);
    }

    #[test]
    fn every_image_option_maps_to_its_documented_flag() {
        let setting = image(|s| {
            s.container_name = "web".to_string();
            s.publish_all_ports = true;
            s.port_bindings = vec![PortBinding {
                host_ip: "127.0.0.1".to_string(),
                host_port: "8080".to_string(),
                container_port: "80".to_string(),
                protocol: "udp".to_string(),
            }];
            s.entrypoint = "/bin/sh".to_string();
            s.bind_mounts = vec![BindMount {
                host_path: "/home/f/site".to_string(),
                container_path: "/srv".to_string(),
                read_only: true,
            }];
            s.env = vec![("FOO".to_string(), "bar".to_string())];
            s.attach = true;
            s.pull_policy = "always".to_string();
            s.run_options = "--rm --label a=\"b c\"".to_string();
            s.command = vec!["echo".to_string(), "hi".to_string()];
        });
        let argv = image_run_argv(&setting, Path::new("/p"), false);
        assert_eq!(
            argv,
            vec![
                "run",
                "-a",
                "--name",
                "web",
                "-P",
                "-p",
                "127.0.0.1:8080:80/udp",
                "--entrypoint",
                "/bin/sh",
                "-v",
                "/home/f/site:/srv:ro",
                "-e",
                "FOO=bar",
                "--pull",
                "always",
                "--rm",
                "--label",
                "a=b c",
                "nginx:1.27",
                "echo",
                "hi",
            ]
        );
    }

    #[test]
    fn selinux_relabel_appends_z_to_a_mount_unless_the_host_path_is_top_level() {
        let setting = image(|s| {
            s.bind_mounts = vec![BindMount {
                host_path: "/home/f/project".to_string(),
                container_path: "/workspace".to_string(),
                read_only: false,
            }];
        });
        let argv = image_run_argv(&setting, Path::new("/p"), true);
        assert!(
            argv.contains(&"/home/f/project:/workspace:z".to_string()),
            "{argv:?}"
        );

        for top in ["/", "/bin", "/usr", "/etc", "/home"] {
            let setting = image(|s| {
                s.bind_mounts = vec![BindMount {
                    host_path: top.to_string(),
                    container_path: "/x".to_string(),
                    read_only: false,
                }];
            });
            let argv = image_run_argv(&setting, Path::new("/p"), true);
            let mount = argv.iter().find(|a| a.starts_with(top)).unwrap();
            assert_eq!(mount, &format!("{top}:/x"), "must not relabel {top}");
        }
    }

    #[test]
    fn ro_and_z_combine_on_one_mount() {
        let setting = image(|s| {
            s.bind_mounts = vec![BindMount {
                host_path: "/data".to_string(),
                container_path: "/data".to_string(),
                read_only: true,
            }];
        });
        let argv = image_run_argv(&setting, Path::new("/p"), true);
        assert!(argv.contains(&"/data:/data:ro,z".to_string()), "{argv:?}");
    }

    #[test]
    fn containerfile_build_argv_derives_the_dockerfile_path_from_the_context() {
        let setting = ContainerfileRunSetting {
            context_dir: "$PROJECT_DIR$".to_string(),
            image_tag: "myapp:dev".to_string(),
            build_args: vec![("VERSION".to_string(), "1.2.3".to_string())],
            build_options: "--no-cache".to_string(),
            ..Default::default()
        };
        assert_eq!(
            containerfile_build_argv(&setting),
            vec![
                "build",
                "-f",
                "$PROJECT_DIR$/Dockerfile",
                "-t",
                "myapp:dev",
                "--build-arg",
                "VERSION=1.2.3",
                "--no-cache",
                "$PROJECT_DIR$",
            ]
        );
    }

    #[test]
    fn an_explicit_dockerfile_name_is_joined_to_the_context_dir() {
        let setting = ContainerfileRunSetting {
            context_dir: "docker".to_string(),
            dockerfile: "Containerfile.prod".to_string(),
            ..Default::default()
        };
        assert_eq!(
            containerfile_build_argv(&setting)[..2],
            ["build".to_string(), "-f".to_string()]
        );
        assert_eq!(
            containerfile_build_argv(&setting)[2],
            "docker/Containerfile.prod"
        );
    }

    #[test]
    fn an_absolute_dockerfile_path_is_used_as_is() {
        let setting = ContainerfileRunSetting {
            dockerfile: "/etc/containers/Containerfile".to_string(),
            ..Default::default()
        };
        assert_eq!(
            containerfile_build_argv(&setting)[2],
            "/etc/containers/Containerfile"
        );
    }

    #[test]
    fn containerfile_run_argv_runs_the_built_image_tag() {
        let setting = ContainerfileRunSetting {
            image_tag: "myapp:dev".to_string(),
            attach: true,
            ..Default::default()
        };
        assert_eq!(
            containerfile_run_argv(&setting, false),
            vec!["run", "-a", "--pull", "missing", "myapp:dev"]
        );
    }

    fn compose(f: impl FnOnce(&mut ComposeRunSetting)) -> ComposeRunSetting {
        let mut setting = ComposeRunSetting {
            compose_files: vec!["docker-compose.yml".to_string()],
            ..Default::default()
        };
        f(&mut setting);
        setting
    }

    #[test]
    fn a_bare_compose_up_is_just_the_files_and_up_dash_d() {
        let argv = compose_up_argv(&compose(|_| {}), Path::new("/p"));
        assert_eq!(
            argv,
            vec!["compose", "-f", "docker-compose.yml", "up", "-d"]
        );
    }

    #[test]
    fn every_compose_up_option_maps_to_its_documented_flag() {
        let setting = compose(|s| {
            s.project_name = "shop".to_string();
            s.profiles = vec!["dev".to_string()];
            s.env_files = vec![".env".to_string()];
            s.compatibility = true;
            s.start = "selected_only".to_string();
            s.attach = "selected_and_deps".to_string();
            s.recreate = "all".to_string();
            s.build = "always".to_string();
            s.always_recreate_deps = true;
            s.renew_anon_volumes = true;
            s.remove_orphans = true;
            s.no_log_prefix = true;
            s.abort_on_container_exit = true;
            s.exit_code_from = Some("web".to_string());
            s.sigkill_timeout = Some(15);
            s.scale = vec![("web".to_string(), 3)];
            s.services = vec!["web".to_string()];
        });
        let argv = compose_up_argv(&setting, Path::new("/p"));
        assert_eq!(
            argv,
            vec![
                "compose",
                "-f",
                "docker-compose.yml",
                "-p",
                "shop",
                "--profile",
                "dev",
                "--env-file",
                ".env",
                "--compatibility",
                "up",
                "-d",
                "--no-deps",
                "--attach-dependencies",
                "--force-recreate",
                "--build",
                "--always-recreate-deps",
                "-V",
                "--remove-orphans",
                "--no-log-prefix",
                "--abort-on-container-exit",
                "--exit-code-from",
                "web",
                "-t",
                "15",
                "--scale",
                "web=3",
                "web",
            ]
        );
    }

    #[test]
    fn compose_down_maps_remove_images_and_orphans_and_volumes() {
        let setting = compose(|s| {
            s.remove_orphans_on_down = true;
            s.remove_volumes_on_down = true;
            s.remove_images_on_down = "local".to_string();
        });
        assert_eq!(
            compose_down_argv(&setting),
            vec![
                "compose",
                "-f",
                "docker-compose.yml",
                "down",
                "--remove-orphans",
                "-v",
                "--rmi",
                "local",
            ]
        );
    }

    #[test]
    fn compose_stop_is_just_the_files_and_stop() {
        assert_eq!(
            compose_stop_argv(&compose(|_| {})),
            vec!["compose", "-f", "docker-compose.yml", "stop"]
        );
    }

    #[test]
    fn compose_scale_argv_is_an_up_with_no_recreate() {
        let files = vec!["docker-compose.yml".to_string()];
        assert_eq!(
            compose_scale_argv(&files, "web", 3),
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

    #[test]
    fn split_shell_words_honours_quotes_and_escapes() {
        assert_eq!(
            split_shell_words(r#"--rm --label a="b c" -v '/x y/:/z'"#),
            vec!["--rm", "--label", "a=b c", "-v", "/x y/:/z"]
        );
        assert_eq!(split_shell_words(""), Vec::<String>::new());
        assert_eq!(split_shell_words("   "), Vec::<String>::new());
        assert_eq!(split_shell_words("a\\ b"), vec!["a b"]);
    }

    #[test]
    fn preview_quotes_only_the_words_that_need_it() {
        let argv = vec![
            "docker".to_string(),
            "run".to_string(),
            "--label".to_string(),
            "a=b c".to_string(),
        ];
        assert_eq!(preview(&argv), "docker run --label 'a=b c'");
    }

    #[test]
    fn compose_services_reports_the_engine_being_missing_rather_than_panicking() {
        let broken = Invocation {
            program: "definitely-not-a-real-engine-binary".to_string(),
            prefix_args: Vec::new(),
            env: Vec::new(),
            host: process_exec::host::ExecHost::Local,
        };
        let result = compose_services(&broken, &["docker-compose.yml".to_string()], Path::new("."));
        assert!(result.is_err());
    }

    #[test]
    fn compose_service_needs_build_reports_the_engine_being_missing_rather_than_panicking() {
        let broken = Invocation {
            program: "definitely-not-a-real-engine-binary".to_string(),
            prefix_args: Vec::new(),
            env: Vec::new(),
            host: process_exec::host::ExecHost::Local,
        };
        let result = compose_service_needs_build(
            &broken,
            &["docker-compose.yml".to_string()],
            "web",
            Path::new("."),
        );
        assert!(result.is_err());
    }

    #[test]
    fn service_has_build_key_reads_a_real_compose_config_json_shape() {
        let json = br#"{"services":{"web":{"image":"myapp:dev","build":{"context":"."}},"db":{"image":"postgres"}}}"#;
        assert!(service_has_build_key(json, "web"));
        assert!(!service_has_build_key(json, "db"));
        assert!(!service_has_build_key(json, "missing"));
    }

    #[test]
    fn service_has_build_key_is_false_on_malformed_json() {
        assert!(!service_has_build_key(b"not json at all", "web"));
        assert!(!service_has_build_key(b"", "web"));
    }

    #[test]
    fn validate_image_requires_a_reference_and_complete_ports_and_mounts() {
        assert!(validate_image(&ContainerImageRunSetting::default()).is_some());

        let mut setting = image(|_| {});
        assert!(validate_image(&setting).is_none());

        setting.port_bindings = vec![PortBinding {
            host_port: "8080".to_string(),
            ..Default::default()
        }];
        assert!(validate_image(&setting).is_some(), "container_port missing");

        setting.port_bindings.clear();
        setting.bind_mounts = vec![BindMount {
            host_path: "/data".to_string(),
            ..Default::default()
        }];
        assert!(validate_image(&setting).is_some(), "container_path missing");
    }

    #[test]
    fn validate_containerfile_requires_an_image_tag_only_when_running_the_built_image() {
        let mut setting = ContainerfileRunSetting::default();
        assert!(
            validate_containerfile(&setting).is_none(),
            "build-only is fine with no tag"
        );

        setting.run_built_image = true;
        assert!(validate_containerfile(&setting).is_some());

        setting.image_tag = "myapp:dev".to_string();
        assert!(validate_containerfile(&setting).is_none());
    }

    #[test]
    fn validate_compose_requires_at_least_one_compose_file() {
        assert!(validate_compose(&ComposeRunSetting::default()).is_some());
        assert!(validate_compose(&ComposeRunSetting {
            compose_files: vec!["".to_string()],
            ..Default::default()
        })
        .is_some());
        assert!(validate_compose(&ComposeRunSetting {
            compose_files: vec!["docker-compose.yml".to_string()],
            ..Default::default()
        })
        .is_none());
    }
}
