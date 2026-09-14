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
use crate::bridge::ffi::{
    self, FfiCommand, FfiContainerOptions, FfiImageDashboard, FfiKeyValue, FfiLayer, FfiResult,
};

use super::actions::parse_node_id;

/// Docker Hub's half of [`ffi::ContainerService::request_image_completions`]
/// (C6): a repository search for a bare prefix, or that repository's own
/// tag list once `prefix` names one with a `:` — the same split
/// `bridge/language/containers.rs`'s `fetch` uses for the editor's popup,
/// simplified to Hub only (no configured-registry browsing: a run target's
/// Image field names an image reference, not one of this project's
/// registries specifically). Never panics on a network failure — an empty
/// list just means the popup's Hub half stays whatever it last had.
fn fetch_hub(prefix: &str) -> Vec<container_core::completion::HubRepo> {
    match prefix.split_once(':') {
        Some((repository, _)) => container_core::image_ref::ImageRef::parse(repository)
            .and_then(|reference| reference.hub_repository())
            .and_then(|repository| container_registry::hub::tags(&repository).ok())
            .map(|tags| {
                tags.into_iter()
                    .map(|name| container_core::completion::HubRepo {
                        name,
                        is_official: false,
                        star_count: 0,
                        description: String::new(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        None => container_registry::hub::search(prefix).unwrap_or_default(),
    }
}
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

    /// [`Self::image_completions`]'s local answer at once, then Docker Hub's
    /// (C6, `container_registry::hub`) merged in once the network round
    /// trip answers, through [`ffi::ContainerService::image_completions_ready`]
    /// — the same "local now, Hub when it lands" split
    /// `bridge/language/containers.rs`'s editor popup already uses,
    /// `container_core::completion::image_completions` is the one ranking
    /// both go through so this list and that popup never disagree about
    /// what "official first" means. A `QCompleter`-driving widget (the New
    /// Target wizard's Image field, `run_config_container_pages.cpp`'s own
    /// Image field) calls this once per keystroke and replaces its model
    /// twice: immediately with the local half, again when the signal
    /// arrives.
    pub fn request_image_completions(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        prefix: &QString,
    ) -> QString {
        let connection_id = connection_id.to_string();
        let prefix = prefix.to_string();
        let mut local_names: Vec<String> = self
            .connections
            .borrow()
            .get(&connection_id)
            .and_then(|connection| connection.snapshot.as_ref())
            .map(|snapshot| {
                snapshot
                    .images
                    .iter()
                    .flat_map(|image| image.repo_tags.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();
        local_names.sort();
        local_names.dedup();

        let local_only = container_core::completion::image_completions(&prefix, &local_names, None);
        let immediate = QString::from(
            local_only
                .iter()
                .map(|item| item.insert.as_str())
                .collect::<Vec<_>>()
                .join("\n")
                .as_str(),
        );

        if !prefix.trim().is_empty() {
            let qt_thread = self.as_mut().qt_thread();
            std::thread::spawn(move || {
                let hub = fetch_hub(&prefix);
                let merged = container_core::completion::image_completions(
                    &prefix,
                    &local_names,
                    Some(&hub),
                );
                let joined = merged
                    .iter()
                    .map(|item| item.insert.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
                    service
                        .as_mut()
                        .image_completions_ready(QString::from(joined.as_str()));
                });
            });
        }
        immediate
    }

    /// "Analyze image" (C9): `save -o` + a headers-only tar walk, on a
    /// worker thread — an image of any real-world size still only reads
    /// headers, but it is still I/O on a temp file, not a snapshot lookup.
    pub fn analyze_image(mut self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
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
            let result = container_core::layer_fs::analyze(&invocation, &resource_id, &work_dir);
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ContainerService>| match result {
                    Ok(layers) => {
                        let lines = layers
                            .iter()
                            .flat_map(|layer| {
                                layer.entries.iter().map(move |entry| {
                                    let kind = match entry.kind {
                                        container_core::layer_fs::EntryKind::Added => "added",
                                        container_core::layer_fs::EntryKind::Modified => "modified",
                                        container_core::layer_fs::EntryKind::Deleted => "deleted",
                                    };
                                    format!(
                                        "{}\t{}\t{}\t{kind}",
                                        layer.layer_id, entry.path, entry.size
                                    )
                                })
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        service.as_mut().layer_fs_ready(
                            QString::from(node_id_str.as_str()),
                            QString::from(lines.as_str()),
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

    /// "Create Container..." from an image node's own reference — the
    /// run-config dialog's prefill (C5, ADR-0056 §6, replacing C4's
    /// `createContainerQuick`): a container-image configuration with
    /// `connection_id`/`image` already set, everything else left at its
    /// default for the user to fill in.
    pub fn image_run_defaults(&self, node_id: &QString) -> FfiContainerOptions {
        let Some((connection_id, resource_id)) =
            parse_node_id(&node_id.to_string(), NodeKind::Image)
        else {
            return FfiContainerOptions::default();
        };
        let connections = self.connections.borrow();
        let Some(connection) = connections.get(&connection_id) else {
            return FfiContainerOptions::default();
        };
        let Some(snapshot) = connection.snapshot.as_ref() else {
            return FfiContainerOptions::default();
        };
        let Some(image) = snapshot.images.iter().find(|image| image.id == resource_id) else {
            return FfiContainerOptions::default();
        };
        FfiContainerOptions {
            connection_id: QString::from(connection_id.as_str()),
            image: QString::from(image.display_name().as_str()),
            ..Default::default()
        }
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
