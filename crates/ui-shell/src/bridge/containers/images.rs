//! Image actions and the Images console (C4): pull/remove/prune/tag,
//! Layers/Labels/Dashboard, Copy Image to another connection, local
//! completion and the quick "Create Container" dialog. A fourth `impl
//! ffi::ContainerService` block — see `mod.rs`'s doc comment for why this
//! is split from `service.rs`/`actions.rs`/`sessions.rs`.
//!
//! Labels/Dashboard/completion read the connection's already-fetched
//! `EngineSnapshot` (`self.connections`) directly — no CLI round trip, the
//! same reasoning `nodes()` already uses for the tree itself. Only
//! `imageLayers` (a fresh `history`), `copyImageTo` (`save`\|`load`), and
//! the mutating ops go through a worker thread.

use std::collections::BTreeMap;
use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::connection::Invocation;
use container_core::images::{self, Layer};
use container_core::model::{Container, Image};
use container_core::ops::{self, OpError};
use container_core::tree::NodeKind;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiCommand, FfiImageDashboard, FfiKeyValue, FfiLayer, FfiResult};

use super::actions::parse_node_id;
use super::service;

fn labels_to_ffi(labels: &BTreeMap<String, String>) -> Vec<FfiKeyValue> {
    labels
        .iter()
        .map(|(key, value)| FfiKeyValue {
            key: QString::from(key.as_str()),
            value: QString::from(value.as_str()),
        })
        .collect()
}

fn to_ffi_layer(layer: Layer) -> FfiLayer {
    FfiLayer {
        id: QString::from(layer.id.as_str()),
        created: QString::from(layer.created.as_str()),
        created_by: QString::from(layer.created_by.as_str()),
        size_bytes: layer.size,
        comment: QString::from(layer.comment.as_str()),
    }
}

/// `"<name>\t<node id>"` per container, `\n`-joined — a Dashboard's
/// "containers using it" list, clickable without a second lookup. Shared
/// with `networks.rs`/`volumes.rs`'s own dashboards.
pub(super) fn containers_to_ffi(connection_id: &str, containers: &[&Container]) -> String {
    containers
        .iter()
        .map(|container| {
            format!(
                "{}\t{connection_id}/{}/{}",
                container.name,
                NodeKind::Container.id(),
                container.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn to_ffi_command(spec: pty_core::ShellSpec) -> FfiCommand {
    FfiCommand {
        program: QString::from(spec.program.as_str()),
        args: QString::from(spec.args.join("\n").as_str()),
        env: QString::from(
            spec.env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("\n")
                .as_str(),
        ),
    }
}

impl ffi::ContainerService {
    pub fn pull_session_command(&self, connection_id: &QString, reference: &QString) -> FfiCommand {
        let Ok(invocation) = service::connection_invocation(&connection_id.to_string()) else {
            return FfiCommand::default();
        };
        to_ffi_command(container_core::session::pull_session(
            &invocation,
            &reference.to_string(),
        ))
    }

    pub fn remove_image(mut self: Pin<&mut Self>, node_id: &QString, force: bool) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Image)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not an image node"),
            );
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let args = images::rmi_args(&resource_id, force);
            let result: Result<(), OpError> =
                ops::run_op(&invocation, &args, &work_dir).map(|_| ());
            report_action(qt_thread, node_id_str, result, connection_id);
        });
        FfiResult::default()
    }

    pub fn prune_images(mut self: Pin<&mut Self>, connection_id: &QString, all: bool) -> FfiResult {
        let connection_id = connection_id.to_string();
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        let for_thread = connection_id.clone();
        std::thread::spawn(move || {
            let result =
                ops::run_op(&invocation, &images::image_prune_args(all), &work_dir).map(|_| ());
            report_action(qt_thread, String::new(), result, for_thread);
        });
        FfiResult::default()
    }

    pub fn tag_image(
        mut self: Pin<&mut Self>,
        node_id: &QString,
        new_reference: &QString,
    ) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Image)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not an image node"),
            );
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let new_reference = new_reference.to_string();
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let args = images::tag_args(&resource_id, &new_reference);
            let result: Result<(), OpError> =
                ops::run_op(&invocation, &args, &work_dir).map(|_| ());
            report_action(qt_thread, node_id_str, result, connection_id);
        });
        FfiResult::default()
    }

    pub fn image_layers(mut self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Image)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not an image node"),
            );
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let engine = match service::connection_engine(&connection_id) {
            Ok(engine) => engine,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result = images::history(&invocation, engine, &resource_id, &work_dir);
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ContainerService>| match result {
                    Ok(layers) => {
                        service.as_mut().layers_ready(
                            QString::from(node_id_str.as_str()),
                            layers.into_iter().map(to_ffi_layer).collect(),
                        );
                    }
                    Err(err) => {
                        service.as_mut().action_finished(
                            QString::from(node_id_str.as_str()),
                            false,
                            QString::from(err.message.as_str()),
                        );
                    }
                },
            );
        });
        FfiResult::default()
    }

    pub fn image_labels(&self, node_id: &QString) -> Vec<FfiKeyValue> {
        let Some((connection_id, resource_id)) =
            parse_node_id(&node_id.to_string(), NodeKind::Image)
        else {
            return Vec::new();
        };
        let connections = self.connections.borrow();
        connections
            .get(&connection_id)
            .and_then(|connection| connection.snapshot.as_ref())
            .and_then(|snapshot| snapshot.images.iter().find(|image| image.id == resource_id))
            .map(|image| labels_to_ffi(&image.labels))
            .unwrap_or_default()
    }

    /// `save`\|`load` on a worker thread: `source`'s invocation for
    /// `node_id`'s connection, `target_connection_id`'s for the other end.
    pub fn copy_image_to(
        mut self: Pin<&mut Self>,
        node_id: &QString,
        target_connection_id: &QString,
    ) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Image)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not an image node"),
            );
        };
        let source = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let target_connection_id = target_connection_id.to_string();
        let dest: Invocation = match service::connection_invocation(&target_connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result = images::copy_image(&source, &dest, &resource_id, &work_dir);
            let (ok, message) = match result {
                Ok(()) => (true, format!("Copied to '{target_connection_id}'")),
                Err(err) => (false, err.message),
            };
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
                service.as_mut().action_finished(
                    QString::from(node_id_str.as_str()),
                    ok,
                    QString::from(message.as_str()),
                );
            });
        });
        FfiResult::default()
    }

    pub fn image_completions(&self, connection_id: &QString, prefix: &QString) -> QString {
        let connections = self.connections.borrow();
        let images: Vec<Image> = connections
            .get(&connection_id.to_string())
            .and_then(|connection| connection.snapshot.as_ref())
            .map(|snapshot| snapshot.images.clone())
            .unwrap_or_default();
        let ranked = images::complete_images(&images, &prefix.to_string());
        QString::from(ranked.join("\n").as_str())
    }

    pub fn image_dashboard(&self, node_id: &QString) -> FfiImageDashboard {
        let Some((connection_id, resource_id)) =
            parse_node_id(&node_id.to_string(), NodeKind::Image)
        else {
            return FfiImageDashboard::default();
        };
        let connections = self.connections.borrow();
        let Some(connection) = connections.get(&connection_id) else {
            return FfiImageDashboard::default();
        };
        let Some(snapshot) = connection.snapshot.as_ref() else {
            return FfiImageDashboard::default();
        };
        let Some(image) = snapshot.images.iter().find(|image| image.id == resource_id) else {
            return FfiImageDashboard::default();
        };
        let using = images::containers_using(&image.id, &snapshot.containers);
        FfiImageDashboard {
            name: QString::from(image.display_name().as_str()),
            id: QString::from(image.id.as_str()),
            size_bytes: image.size as i64,
            created: QString::from(image.created.as_str()),
            tags: QString::from(image.repo_tags.join("\n").as_str()),
            digests: QString::from(image.repo_digests.join("\n").as_str()),
            containers: QString::from(containers_to_ffi(&connection_id, &using).as_str()),
        }
    }

    /// Until C5's run-config editor lands: `run -d [--name <name>] [-P]
    /// <image>`.
    pub fn create_container_quick(
        mut self: Pin<&mut Self>,
        node_id: &QString,
        name: &QString,
        publish_all: bool,
    ) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Image)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not an image node"),
            );
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let name = name.to_string();
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let mut args = vec!["run".to_string(), "-d".to_string()];
            if !name.is_empty() {
                args.push("--name".to_string());
                args.push(name);
            }
            if publish_all {
                args.push("-P".to_string());
            }
            args.push(resource_id);
            let result: Result<(), OpError> =
                ops::run_op(&invocation, &args, &work_dir).map(|_| ());
            report_action(qt_thread, String::new(), result, connection_id);
        });
        FfiResult::default()
    }
}

/// Emit `actionFinished` and re-snapshot `connection_id` — the same shape
/// `actions.rs::report` uses; `pub(super)` so `networks.rs`/`volumes.rs`
/// share it rather than each duplicating it again.
pub(super) fn report_action(
    qt_thread: cxx_qt::CxxQtThread<ffi::ContainerService>,
    node_id: String,
    result: Result<(), OpError>,
    connection_id: String,
) {
    let (ok, message) = match result {
        Ok(()) => (true, String::new()),
        Err(err) => (false, err.message),
    };
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
        service.as_mut().action_finished(
            QString::from(node_id.as_str()),
            ok,
            QString::from(message.as_str()),
        );
        let _ = service
            .as_mut()
            .refresh(&QString::from(connection_id.as_str()));
    });
}
