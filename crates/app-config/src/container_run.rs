//! Container-kind run configurations (C5, ADR-0056): the three optional
//! sub-tables a `[[run_config]]` row gains when its `kind` names a container
//! flavor, plus the small shared shapes (`PortBinding`, `BindMount`) they
//! both use.
//!
//! Persistence only, like every other struct in this crate (ADR-0017): what
//! a field *becomes* on a command line is `container_core::run_config`'s
//! job. Field names follow JetBrains' own Docker run-configuration option
//! tables verbatim, so a user who has used that plugin recognises every one
//! of them; the "meaning" behind a free-form string (`pull_policy`,
//! `remove_images_on_down`, ...) is likewise `container-core`'s to interpret,
//! with an unrecognised value reading as the least surprising default —
//! `RunConfigSetting::toolchain`'s own rule (ADR-0039).

use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !b
}

/// One `-p host_ip:host_port:container_port/protocol` entry.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct PortBinding {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_ip: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_port: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub container_port: String,
    /// `"tcp"` or `"udp"`. Empty (and any unrecognised value) reads as
    /// `tcp`, the CLI's own default when `/proto` is omitted.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub protocol: String,
}

/// One `-v host_path:container_path[:ro]` entry.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct BindMount {
    #[serde(default)]
    pub host_path: String,
    #[serde(default)]
    pub container_path: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_only: bool,
}

/// The "Docker Image" / "Container Image" run configuration's options
/// (JetBrains' Docker Image page, ~12 options).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerImageRunSetting {
    /// Which `[containers.connection]` row runs this — the "Server" combo.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub container_name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub publish_all_ports: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_bindings: Vec<PortBinding>,
    /// Overrides the image's own `ENTRYPOINT`. Empty means "unset".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entrypoint: String,
    /// The command appended after the image reference (overrides `CMD`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bind_mounts: Vec<BindMount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
    /// Free-form extra `run` arguments, appended verbatim (shell-word
    /// split) after everything this struct's own fields already produced.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_options: String,
    /// Attach to the container's console (`-a`) rather than detaching
    /// (`-d`) once started.
    #[serde(default, skip_serializing_if = "is_false")]
    pub attach: bool,
    /// `"missing"` (default), `"always"`, or `"never"` — `docker run
    /// --pull <value>`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pull_policy: String,
}

/// The "Dockerfile" / "Containerfile" run configuration's options: every
/// [`ContainerImageRunSetting`] field (the container this produces is run
/// exactly the way an Image configuration's is) plus what it takes to build
/// the image first.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerfileRunSetting {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub container_name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub publish_all_ports: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_bindings: Vec<PortBinding>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bind_mounts: Vec<BindMount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_options: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub attach: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pull_policy: String,

    /// Path to the `Dockerfile`/`Containerfile`, relative to
    /// [`ContainerfileRunSetting::context_dir`] (JetBrains' default:
    /// `Dockerfile` at the context root).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dockerfile: String,
    /// The build context directory, project-relative (or a `$PROJECT_DIR$`
    /// macro — expanded by `run-core`, never here).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context_dir: String,
    /// The tag the built image is given, and what the run step launches.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image_tag: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_args: Vec<(String, String)>,
    /// Free-form extra `build` arguments, appended verbatim.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub build_options: String,
    /// Whether launching this configuration also runs the image it just
    /// built. Off means "build only" — usable as another configuration's
    /// before-launch task with nothing started afterward.
    #[serde(default, skip_serializing_if = "is_false")]
    pub run_built_image: bool,
}

/// The "Docker Compose" run configuration's options (JetBrains' Compose
/// page, ~25 options).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposeRunSetting {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connection_id: String,
    /// `-f <file>` per entry, in order — project-relative paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compose_files: Vec<String>,
    /// Services to start; empty means every service the files define.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_files: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub compatibility: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub remove_orphans_on_down: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub remove_volumes_on_down: bool,
    /// `"none"` (default), `"all"`, or `"local"` — `compose down --rmi
    /// <value>`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remove_images_on_down: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigkill_timeout: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code_from: Option<String>,
    /// `service` -> replica count, `--scale service=n` per entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scale: Vec<(String, u32)>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub always_recreate_deps: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub renew_anon_volumes: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub remove_orphans: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_log_prefix: bool,
    /// `"selected_and_deps"` (default), `"none"`, or `"selected_only"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start: String,
    /// `"selected"` (default), `"none"`, or `"selected_and_deps"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attach: String,
    /// `"changed"` (default), `"all"`, or `"none"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recreate: String,
    /// `"missing"` (default), `"never"`, or `"always"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub build: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub abort_on_container_exit: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_image_setting_writes_nothing() {
        let text = toml::to_string(&ContainerImageRunSetting::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn an_untouched_containerfile_setting_writes_nothing() {
        let text = toml::to_string(&ContainerfileRunSetting::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn an_untouched_compose_setting_writes_nothing() {
        let text = toml::to_string(&ComposeRunSetting::default()).expect("serialize");
        assert_eq!(text.trim(), "");
    }

    #[test]
    fn an_image_setting_round_trips_through_toml() {
        let setting = ContainerImageRunSetting {
            connection_id: "local-docker".to_string(),
            image: "nginx:1.27".to_string(),
            container_name: "web".to_string(),
            publish_all_ports: true,
            port_bindings: vec![PortBinding {
                host_ip: "127.0.0.1".to_string(),
                host_port: "8080".to_string(),
                container_port: "80".to_string(),
                protocol: "tcp".to_string(),
            }],
            entrypoint: "/bin/sh".to_string(),
            command: vec!["-c".to_string(), "nginx -g 'daemon off;'".to_string()],
            bind_mounts: vec![BindMount {
                host_path: "/home/f/site".to_string(),
                container_path: "/usr/share/nginx/html".to_string(),
                read_only: true,
            }],
            env: vec![("FOO".to_string(), "bar".to_string())],
            run_options: "--rm".to_string(),
            attach: true,
            pull_policy: "always".to_string(),
        };
        let text = toml::to_string(&setting).expect("serialize");
        let parsed: ContainerImageRunSetting = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, setting);
    }

    #[test]
    fn a_containerfile_setting_round_trips_through_toml() {
        let setting = ContainerfileRunSetting {
            connection_id: "local-docker".to_string(),
            dockerfile: "Dockerfile".to_string(),
            context_dir: "$PROJECT_DIR$".to_string(),
            image_tag: "myapp:dev".to_string(),
            build_args: vec![("VERSION".to_string(), "1.2.3".to_string())],
            build_options: "--no-cache".to_string(),
            run_built_image: true,
            container_name: "myapp".to_string(),
            ..ContainerfileRunSetting::default()
        };
        let text = toml::to_string(&setting).expect("serialize");
        let parsed: ContainerfileRunSetting = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, setting);
    }

    #[test]
    fn a_compose_setting_round_trips_through_toml() {
        let setting = ComposeRunSetting {
            connection_id: "local-docker".to_string(),
            compose_files: vec!["docker-compose.yml".to_string()],
            services: vec!["web".to_string(), "db".to_string()],
            project_name: "shop".to_string(),
            profiles: vec!["dev".to_string()],
            env: vec![("FOO".to_string(), "bar".to_string())],
            env_files: vec![".env".to_string()],
            compatibility: true,
            remove_orphans_on_down: true,
            remove_volumes_on_down: false,
            remove_images_on_down: "local".to_string(),
            sigkill_timeout: Some(15),
            exit_code_from: Some("web".to_string()),
            scale: vec![("web".to_string(), 3)],
            always_recreate_deps: true,
            renew_anon_volumes: true,
            remove_orphans: true,
            no_log_prefix: true,
            start: "selected_only".to_string(),
            attach: "none".to_string(),
            recreate: "all".to_string(),
            build: "always".to_string(),
            abort_on_container_exit: true,
        };
        let text = toml::to_string(&setting).expect("serialize");
        let parsed: ComposeRunSetting = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, setting);
    }
}
