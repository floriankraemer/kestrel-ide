//! `FfiContainerTarget` <-> `app_config::ContainerTargetSetting` (C8):
//! structured, no JSON — the same reason `container_form.rs` translates
//! `FfiContainerOptions` this way. Split out of `mod.rs` under the file-size
//! ratchet.

use cxx_qt_lib::QString;

use crate::bridge::ffi;

fn join_lines(items: &[String]) -> String {
    items.join("\n")
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn opt_string(text: &str) -> Option<String> {
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

/// One `[containers.target]` row (C8), translated onto its `FfiContainerTarget`.
pub(crate) fn to_ffi_target(
    target: &app_config::ContainerTargetSetting,
) -> ffi::FfiContainerTarget {
    ffi::FfiContainerTarget {
        id: QString::from(target.id.as_str()),
        name: QString::from(target.name.as_str()),
        connection_id: QString::from(target.connection_id.as_str()),
        source: QString::from(target.source.as_str()),
        image: QString::from(target.image.clone().unwrap_or_default().as_str()),
        dockerfile: QString::from(target.dockerfile.clone().unwrap_or_default().as_str()),
        context_dir: QString::from(target.context_dir.clone().unwrap_or_default().as_str()),
        image_tag: QString::from(target.image_tag.clone().unwrap_or_default().as_str()),
        compose_files: QString::from(join_lines(&target.compose_files).as_str()),
        service: QString::from(target.service.clone().unwrap_or_default().as_str()),
        needs_build: target.needs_build,
        workdir: QString::from(target.workdir.as_str()),
        run_options: QString::from(target.run_options.as_str()),
        env: target
            .env
            .iter()
            .map(|(k, v)| ffi::FfiKeyValue {
                key: QString::from(k.as_str()),
                value: QString::from(v.as_str()),
            })
            .collect(),
        port_bindings: target
            .port_bindings
            .iter()
            .map(|p| ffi::FfiPortBinding {
                host_ip: QString::from(p.host_ip.as_str()),
                host_port: QString::from(p.host_port.as_str()),
                container_port: QString::from(p.container_port.as_str()),
                protocol: QString::from(p.protocol.as_str()),
            })
            .collect(),
        publish_all_ports: target.publish_all_ports,
        extra_mounts: target
            .extra_mounts
            .iter()
            .map(|m| ffi::FfiBindMount {
                host_path: QString::from(m.host_path.as_str()),
                container_path: QString::from(m.container_path.as_str()),
                read_only: m.read_only,
            })
            .collect(),
    }
}

/// The inverse of [`to_ffi_target`].
pub(crate) fn from_ffi_target(row: &ffi::FfiContainerTarget) -> app_config::ContainerTargetSetting {
    app_config::ContainerTargetSetting {
        id: row.id.to_string(),
        name: row.name.to_string(),
        connection_id: row.connection_id.to_string(),
        source: row.source.to_string(),
        image: opt_string(&row.image.to_string()),
        dockerfile: opt_string(&row.dockerfile.to_string()),
        context_dir: opt_string(&row.context_dir.to_string()),
        image_tag: opt_string(&row.image_tag.to_string()),
        compose_files: split_lines(&row.compose_files.to_string()),
        service: opt_string(&row.service.to_string()),
        needs_build: row.needs_build,
        workdir: row.workdir.to_string(),
        run_options: row.run_options.to_string(),
        env: row
            .env
            .iter()
            .map(|p| (p.key.to_string(), p.value.to_string()))
            .collect(),
        port_bindings: row
            .port_bindings
            .iter()
            .map(|p| app_config::container_run::PortBinding {
                host_ip: p.host_ip.to_string(),
                host_port: p.host_port.to_string(),
                container_port: p.container_port.to_string(),
                protocol: p.protocol.to_string(),
            })
            .collect(),
        publish_all_ports: row.publish_all_ports,
        extra_mounts: row
            .extra_mounts
            .iter()
            .map(|m| app_config::container_run::BindMount {
                host_path: m.host_path.to_string(),
                container_path: m.container_path.to_string(),
                read_only: m.read_only,
            })
            .collect(),
    }
}
