//! Translation between `FfiContainerOptions` (the run-config dialog's
//! structured container form, C5/ADR-0056) and `run_core::RunConfig`'s
//! `kind` + `container_image`/`containerfile`/`compose` sub-tables.
//!
//! Every list field crosses the seam as `Vec<FfiRow>` (ports, mounts, env,
//! build args, scale) or a `\n`-separated `QString` (compose files,
//! services, profiles, env files — plain string lists with no second
//! column, the same convention `FfiRunConfig::before_launch` already uses).
//! No JSON, no untyped blob: every field here is exactly the one
//! `app_config::container_run` struct field it round-trips to.

use cxx_qt_lib::QString;

use app_config::container_run::{
    BindMount, ComposeRunSetting, ContainerImageRunSetting, ContainerfileRunSetting, PortBinding,
};

use crate::bridge::ffi;

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn join_lines(items: &[String]) -> String {
    items.join("\n")
}

fn to_ffi_ports(ports: &[PortBinding]) -> Vec<ffi::FfiPortBinding> {
    ports
        .iter()
        .map(|p| ffi::FfiPortBinding {
            host_ip: QString::from(p.host_ip.as_str()),
            host_port: QString::from(p.host_port.as_str()),
            container_port: QString::from(p.container_port.as_str()),
            protocol: QString::from(p.protocol.as_str()),
        })
        .collect()
}

fn from_ffi_ports(ports: &[ffi::FfiPortBinding]) -> Vec<PortBinding> {
    ports
        .iter()
        .map(|p| PortBinding {
            host_ip: p.host_ip.to_string(),
            host_port: p.host_port.to_string(),
            container_port: p.container_port.to_string(),
            protocol: p.protocol.to_string(),
        })
        .collect()
}

fn to_ffi_mounts(mounts: &[BindMount]) -> Vec<ffi::FfiBindMount> {
    mounts
        .iter()
        .map(|m| ffi::FfiBindMount {
            host_path: QString::from(m.host_path.as_str()),
            container_path: QString::from(m.container_path.as_str()),
            read_only: m.read_only,
        })
        .collect()
}

fn from_ffi_mounts(mounts: &[ffi::FfiBindMount]) -> Vec<BindMount> {
    mounts
        .iter()
        .map(|m| BindMount {
            host_path: m.host_path.to_string(),
            container_path: m.container_path.to_string(),
            read_only: m.read_only,
        })
        .collect()
}

fn to_ffi_pairs(pairs: &[(String, String)]) -> Vec<ffi::FfiKeyValue> {
    pairs
        .iter()
        .map(|(k, v)| ffi::FfiKeyValue {
            key: QString::from(k.as_str()),
            value: QString::from(v.as_str()),
        })
        .collect()
}

fn from_ffi_pairs(pairs: &[ffi::FfiKeyValue]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|p| (p.key.to_string(), p.value.to_string()))
        .collect()
}

fn to_ffi_scale(scale: &[(String, u32)]) -> Vec<ffi::FfiScaleEntry> {
    scale
        .iter()
        .map(|(service, count)| ffi::FfiScaleEntry {
            service: QString::from(service.as_str()),
            count: *count,
        })
        .collect()
}

fn from_ffi_scale(scale: &[ffi::FfiScaleEntry]) -> Vec<(String, u32)> {
    scale
        .iter()
        .map(|s| (s.service.to_string(), s.count))
        .collect()
}

fn opt_string(text: &str) -> Option<String> {
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

/// `config.kind`'s sub-table, translated onto one `FfiContainerOptions` —
/// every field of whichever kind applies; every other kind's fields stay at
/// their default (never read by `apply_options` unless `kind` changes to
/// match them, which the dialog never does for an existing row — see
/// `FfiRunConfig::kind`'s own doc comment).
pub(super) fn to_ffi_options(config: &run_core::RunConfig) -> ffi::FfiContainerOptions {
    match config.kind.as_deref() {
        Some("container-image") => {
            let s = config.container_image.clone().unwrap_or_default();
            ffi::FfiContainerOptions {
                connection_id: QString::from(s.connection_id.as_str()),
                image: QString::from(s.image.as_str()),
                container_name: QString::from(s.container_name.as_str()),
                publish_all_ports: s.publish_all_ports,
                port_bindings: to_ffi_ports(&s.port_bindings),
                entrypoint: QString::from(s.entrypoint.as_str()),
                command: QString::from(s.command.join(" ").as_str()),
                bind_mounts: to_ffi_mounts(&s.bind_mounts),
                env: to_ffi_pairs(&s.env),
                run_options: QString::from(s.run_options.as_str()),
                run_attach: s.attach,
                pull_policy: QString::from(s.pull_policy.as_str()),
                ..Default::default()
            }
        }
        Some("containerfile") => {
            let s = config.containerfile.clone().unwrap_or_default();
            ffi::FfiContainerOptions {
                connection_id: QString::from(s.connection_id.as_str()),
                container_name: QString::from(s.container_name.as_str()),
                publish_all_ports: s.publish_all_ports,
                port_bindings: to_ffi_ports(&s.port_bindings),
                entrypoint: QString::from(s.entrypoint.as_str()),
                command: QString::from(s.command.join(" ").as_str()),
                bind_mounts: to_ffi_mounts(&s.bind_mounts),
                env: to_ffi_pairs(&s.env),
                run_options: QString::from(s.run_options.as_str()),
                run_attach: s.attach,
                pull_policy: QString::from(s.pull_policy.as_str()),
                dockerfile: QString::from(s.dockerfile.as_str()),
                context_dir: QString::from(s.context_dir.as_str()),
                image_tag: QString::from(s.image_tag.as_str()),
                build_args: to_ffi_pairs(&s.build_args),
                build_options: QString::from(s.build_options.as_str()),
                run_built_image: s.run_built_image,
                ..Default::default()
            }
        }
        Some("compose") => {
            let s = config.compose.clone().unwrap_or_default();
            ffi::FfiContainerOptions {
                connection_id: QString::from(s.connection_id.as_str()),
                compose_files: QString::from(join_lines(&s.compose_files).as_str()),
                services: QString::from(join_lines(&s.services).as_str()),
                project_name: QString::from(s.project_name.as_str()),
                profiles: QString::from(join_lines(&s.profiles).as_str()),
                env: to_ffi_pairs(&s.env),
                env_files: QString::from(join_lines(&s.env_files).as_str()),
                compatibility: s.compatibility,
                remove_orphans_on_down: s.remove_orphans_on_down,
                remove_volumes_on_down: s.remove_volumes_on_down,
                remove_images_on_down: QString::from(s.remove_images_on_down.as_str()),
                sigkill_timeout: QString::from(
                    s.sigkill_timeout
                        .map(|t| t.to_string())
                        .unwrap_or_default()
                        .as_str(),
                ),
                exit_code_from: QString::from(
                    s.exit_code_from.clone().unwrap_or_default().as_str(),
                ),
                scale: to_ffi_scale(&s.scale),
                always_recreate_deps: s.always_recreate_deps,
                renew_anon_volumes: s.renew_anon_volumes,
                remove_orphans: s.remove_orphans,
                no_log_prefix: s.no_log_prefix,
                start: QString::from(s.start.as_str()),
                compose_attach: QString::from(s.attach.as_str()),
                recreate: QString::from(s.recreate.as_str()),
                build: QString::from(s.build.as_str()),
                abort_on_container_exit: s.abort_on_container_exit,
                ..Default::default()
            }
        }
        _ => ffi::FfiContainerOptions::default(),
    }
}

/// The inverse: read `options` back into `config.kind`'s own sub-table,
/// clearing the other two — a configuration is exactly one kind at a time.
/// `kind` naming nothing this build knows leaves every sub-table `None`.
pub(super) fn apply_options(
    config: &mut run_core::RunConfig,
    kind: &str,
    options: &ffi::FfiContainerOptions,
) {
    config.container_image = None;
    config.containerfile = None;
    config.compose = None;
    config.kind = (!kind.is_empty()).then(|| kind.to_string());

    match kind {
        "container-image" => {
            config.container_image = Some(ContainerImageRunSetting {
                connection_id: options.connection_id.to_string(),
                image: options.image.to_string(),
                container_name: options.container_name.to_string(),
                publish_all_ports: options.publish_all_ports,
                port_bindings: from_ffi_ports(&options.port_bindings),
                entrypoint: options.entrypoint.to_string(),
                command: lines(&options.command.to_string()),
                bind_mounts: from_ffi_mounts(&options.bind_mounts),
                env: from_ffi_pairs(&options.env),
                run_options: options.run_options.to_string(),
                attach: options.run_attach,
                pull_policy: options.pull_policy.to_string(),
            });
        }
        "containerfile" => {
            config.containerfile = Some(ContainerfileRunSetting {
                connection_id: options.connection_id.to_string(),
                container_name: options.container_name.to_string(),
                publish_all_ports: options.publish_all_ports,
                port_bindings: from_ffi_ports(&options.port_bindings),
                entrypoint: options.entrypoint.to_string(),
                command: lines(&options.command.to_string()),
                bind_mounts: from_ffi_mounts(&options.bind_mounts),
                env: from_ffi_pairs(&options.env),
                run_options: options.run_options.to_string(),
                attach: options.run_attach,
                pull_policy: options.pull_policy.to_string(),
                dockerfile: options.dockerfile.to_string(),
                context_dir: options.context_dir.to_string(),
                image_tag: options.image_tag.to_string(),
                build_args: from_ffi_pairs(&options.build_args),
                build_options: options.build_options.to_string(),
                run_built_image: options.run_built_image,
            });
        }
        "compose" => {
            config.compose = Some(ComposeRunSetting {
                connection_id: options.connection_id.to_string(),
                compose_files: lines(&options.compose_files.to_string()),
                services: lines(&options.services.to_string()),
                project_name: options.project_name.to_string(),
                profiles: lines(&options.profiles.to_string()),
                env: from_ffi_pairs(&options.env),
                env_files: lines(&options.env_files.to_string()),
                compatibility: options.compatibility,
                remove_orphans_on_down: options.remove_orphans_on_down,
                remove_volumes_on_down: options.remove_volumes_on_down,
                remove_images_on_down: options.remove_images_on_down.to_string(),
                sigkill_timeout: opt_string(&options.sigkill_timeout.to_string())
                    .and_then(|t| t.parse().ok()),
                exit_code_from: opt_string(&options.exit_code_from.to_string()),
                scale: from_ffi_scale(&options.scale),
                always_recreate_deps: options.always_recreate_deps,
                renew_anon_volumes: options.renew_anon_volumes,
                remove_orphans: options.remove_orphans,
                no_log_prefix: options.no_log_prefix,
                start: options.start.to_string(),
                attach: options.compose_attach.to_string(),
                recreate: options.recreate.to_string(),
                build: options.build.to_string(),
                abort_on_container_exit: options.abort_on_container_exit,
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_image_options_round_trip_through_the_ffi_shape() {
        let mut config = run_core::RunConfig::default();
        let mut options = ffi::FfiContainerOptions {
            connection_id: QString::from("local-docker"),
            image: QString::from("nginx:1.27"),
            container_name: QString::from("web"),
            publish_all_ports: true,
            port_bindings: vec![ffi::FfiPortBinding {
                host_ip: QString::from("127.0.0.1"),
                host_port: QString::from("8080"),
                container_port: QString::from("80"),
                protocol: QString::from("tcp"),
            }],
            entrypoint: QString::from("/bin/sh"),
            command: QString::from("-c echo"),
            bind_mounts: vec![ffi::FfiBindMount {
                host_path: QString::from("/data"),
                container_path: QString::from("/data"),
                read_only: true,
            }],
            env: vec![ffi::FfiKeyValue {
                key: QString::from("FOO"),
                value: QString::from("bar"),
            }],
            run_options: QString::from("--rm"),
            run_attach: true,
            pull_policy: QString::from("always"),
            ..Default::default()
        };
        apply_options(&mut config, "container-image", &options);
        assert_eq!(config.kind.as_deref(), Some("container-image"));
        let setting = config.container_image.as_ref().unwrap();
        assert_eq!(setting.image, "nginx:1.27");
        assert_eq!(setting.port_bindings[0].host_port, "8080");
        assert_eq!(setting.env, vec![("FOO".to_string(), "bar".to_string())]);
        assert!(setting.attach);

        options = to_ffi_options(&config);
        assert_eq!(options.image.to_string(), "nginx:1.27");
        assert_eq!(options.env[0].key.to_string(), "FOO");
        assert!(options.run_attach);
    }

    #[test]
    fn compose_options_round_trip_including_optional_numeric_fields() {
        let mut config = run_core::RunConfig::default();
        let options = ffi::FfiContainerOptions {
            compose_files: QString::from("docker-compose.yml\ndocker-compose.override.yml"),
            services: QString::from("web\ndb"),
            project_name: QString::from("shop"),
            sigkill_timeout: QString::from("15"),
            exit_code_from: QString::from("web"),
            scale: vec![ffi::FfiScaleEntry {
                service: QString::from("web"),
                count: 3,
            }],
            start: QString::from("selected_only"),
            compose_attach: QString::from("none"),
            ..Default::default()
        };
        apply_options(&mut config, "compose", &options);
        let setting = config.compose.as_ref().unwrap();
        assert_eq!(
            setting.compose_files,
            vec![
                "docker-compose.yml".to_string(),
                "docker-compose.override.yml".to_string()
            ]
        );
        assert_eq!(setting.sigkill_timeout, Some(15));
        assert_eq!(setting.exit_code_from.as_deref(), Some("web"));
        assert_eq!(setting.scale, vec![("web".to_string(), 3)]);
        assert_eq!(setting.start, "selected_only");
        assert_eq!(setting.attach, "none");

        let back = to_ffi_options(&config);
        assert_eq!(back.sigkill_timeout.to_string(), "15");
        assert_eq!(back.compose_attach.to_string(), "none");
    }

    #[test]
    fn switching_kind_clears_the_other_sub_tables() {
        let mut config = run_core::RunConfig::default();
        apply_options(
            &mut config,
            "container-image",
            &ffi::FfiContainerOptions::default(),
        );
        assert!(config.container_image.is_some());
        apply_options(&mut config, "compose", &ffi::FfiContainerOptions::default());
        assert!(config.container_image.is_none());
        assert!(config.compose.is_some());
    }

    #[test]
    fn an_empty_kind_clears_every_sub_table() {
        let mut config = run_core::RunConfig::default();
        apply_options(&mut config, "compose", &ffi::FfiContainerOptions::default());
        apply_options(&mut config, "", &ffi::FfiContainerOptions::default());
        assert!(config.kind.is_none());
        assert!(config.compose.is_none());
    }
}
