//! Container lifecycle actions (C3): start/stop/restart/remove/pause/
//! unpause/prune, Inspect, and Processes. A second `impl
//! ffi::ContainerService` block — see `mod.rs`'s doc comment for why this
//! is split from `service.rs`.
//!
//! Every action here runs the actual `docker`/`podman` call on a worker
//! thread and reports back through `qt_thread().queue`, exactly the
//! `test_container_connection` (C1)/`connect_engine` (C2) shape: never
//! block the Qt thread on a process that may hang talking to an
//! unreachable daemon.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::ops::{self, OpError};
use container_core::tree::{self, NodeActions, NodeKind, NodeStatus};

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiNodeActions, FfiResult};

use super::service;

/// Parse a tree node id (`container_core::tree::flatten`'s
/// `"<conn>/<kind>/<resource-id>"`) into `(connection_id, resource_id)`,
/// for a node of `kind`. Every resource id (container/image/network id,
/// volume name) is either hex or a name the engine itself rejects `/` in,
/// so this is exact, not heuristic.
pub(super) fn parse_node_id(node_id: &str, kind: NodeKind) -> Option<(String, String)> {
    let (connection_id, rest) = node_id.split_once('/')?;
    let (found_kind, resource_id) = rest.split_once('/')?;
    if found_kind != kind.id() {
        return None;
    }
    if connection_id.is_empty() || resource_id.is_empty() {
        return None;
    }
    Some((connection_id.to_string(), resource_id.to_string()))
}

/// [`parse_node_id`] specialised to `container` — C3's original entry
/// point, kept so its many call sites in this file and `sessions.rs` read
/// the same as before C4 generalised the parser.
pub(super) fn parse_container_node_id(node_id: &str) -> Option<(String, String)> {
    parse_node_id(node_id, NodeKind::Container)
}

/// `cleanUp`'s `kind` string into a [`container_core::prune::CleanUpKind`]
/// — the words a cpp `QAction`'s `data()` carries, kept stable since the
/// view stores them (`containers_menu.cpp`'s Clean Up ▾ menu).
fn parse_clean_up_kind(kind: &str) -> Option<container_core::prune::CleanUpKind> {
    use container_core::prune::CleanUpKind;
    Some(match kind {
        "all" => CleanUpKind::All,
        "stopped-containers" => CleanUpKind::StoppedContainers,
        "unused-networks" => CleanUpKind::UnusedNetworks,
        "unused-volumes" => CleanUpKind::UnusedVolumes,
        "dangling-images" => CleanUpKind::DanglingImages,
        "build-cache" => CleanUpKind::BuildCache,
        _ => return None,
    })
}

fn to_ffi_node_actions(actions: NodeActions, can_recreate: bool) -> FfiNodeActions {
    FfiNodeActions {
        can_start: actions.can_start,
        can_stop: actions.can_stop,
        can_restart: actions.can_restart,
        can_pause: actions.can_pause,
        can_unpause: actions.can_unpause,
        can_remove: actions.can_remove,
        can_pull: actions.can_pull,
        can_tag: actions.can_tag,
        can_create_container: actions.can_create_container,
        can_copy: actions.can_copy,
        can_clean_up: actions.can_clean_up,
        can_create: actions.can_create,
        can_recreate,
    }
}

/// The reverse of `service::to_ffi_status` — recovering a `NodeStatus`
/// from the row `nodes()` already flattened is cheaper here than adding a
/// second, container-only path back into `container_core::snapshot` just
/// to answer "what are this node's actions".
fn node_status_from_ffi(status: ffi::FfiContainerNodeStatus) -> NodeStatus {
    use ffi::FfiContainerNodeStatus as Ffi;
    match status {
        Ffi::None => NodeStatus::None,
        Ffi::Disconnected => NodeStatus::Disconnected,
        Ffi::Connecting => NodeStatus::Connecting,
        Ffi::Connected => NodeStatus::Connected,
        Ffi::Error => NodeStatus::Error,
        Ffi::Running => NodeStatus::Running,
        Ffi::Paused => NodeStatus::Paused,
        Ffi::Restarting => NodeStatus::Restarting,
        Ffi::Exited => NodeStatus::Exited,
        Ffi::Created => NodeStatus::Created,
        Ffi::Dead => NodeStatus::Dead,
        _ => NodeStatus::Other,
    }
}

impl ffi::ContainerService {
    pub fn node_actions(&self, node_id: &QString) -> FfiNodeActions {
        let node_id = node_id.to_string();
        let Some(node) = self
            .nodes()
            .into_iter()
            .find(|node| node.id.to_string() == node_id)
        else {
            return FfiNodeActions::default();
        };
        let kind = NodeKind::from_id(&node.kind.to_string()).unwrap_or(NodeKind::Connection);
        let status = node_status_from_ffi(node.status);
        let can_recreate = kind == NodeKind::Container && self.container_recreatable(&node_id);
        to_ffi_node_actions(tree::actions_for(kind, status), can_recreate)
    }

    /// Whether `node_id` (a container node) is a real, still-present
    /// container that is not compose-managed
    /// ([`container_core::recreate::is_compose_managed`]) — the
    /// Dashboard's "Recreate with changes" is refused for both a compose-
    /// managed container (edit the compose file instead) and a node that
    /// no longer resolves to a snapshot entry.
    fn container_recreatable(&self, node_id: &str) -> bool {
        let Some((connection_id, resource_id)) = parse_container_node_id(node_id) else {
            return false;
        };
        let connections = self.connections.borrow();
        let Some(snapshot) = connections
            .get(&connection_id)
            .and_then(|connection| connection.snapshot.as_ref())
        else {
            return false;
        };
        snapshot
            .containers
            .iter()
            .find(|container| container.id == resource_id)
            .is_some_and(|container| !container_core::recreate::is_compose_managed(container))
    }

    pub fn start_container(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_lifecycle_op(self, node_id, ops::start_args)
    }

    pub fn stop_container(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_lifecycle_op(self, node_id, ops::stop_args)
    }

    pub fn restart_container(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_lifecycle_op(self, node_id, ops::restart_args)
    }

    pub fn pause_container(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_lifecycle_op(self, node_id, ops::pause_args)
    }

    pub fn unpause_container(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_lifecycle_op(self, node_id, ops::unpause_args)
    }

    pub fn remove_container(self: Pin<&mut Self>, node_id: &QString, force: bool) -> FfiResult {
        run_lifecycle_op(self, node_id, move |id| ops::remove_args(id, force))
    }

    // --- C9: pods ----------------------------------------------------

    pub fn start_pod(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_pod_lifecycle_op(self, node_id, container_core::pods::start_args)
    }

    pub fn stop_pod(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_pod_lifecycle_op(self, node_id, container_core::pods::stop_args)
    }

    pub fn restart_pod(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        run_pod_lifecycle_op(self, node_id, container_core::pods::restart_args)
    }

    pub fn remove_pod(self: Pin<&mut Self>, node_id: &QString, force: bool) -> FfiResult {
        run_pod_lifecycle_op(self, node_id, move |id| {
            container_core::pods::remove_args(id, force)
        })
    }

    // --- C9: Podman machines, from the connection node's own menu ----

    pub fn start_machine(mut self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        run_machine_op(
            self.as_mut(),
            connection_id,
            container_core::machine::start_args,
        )
    }

    pub fn stop_machine(mut self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        run_machine_op(
            self.as_mut(),
            connection_id,
            container_core::machine::stop_args,
        )
    }

    /// The Containers group's "Clean Up": `container prune -f` on
    /// `connection_id`. Not per-container, so it does not go through
    /// [`run_lifecycle_op`]'s node-id parsing; `actionFinished`'s
    /// `node_id` is empty, which the panel reads as "this connection",
    /// not any one row.
    pub fn prune_containers(mut self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        let connection_id = connection_id.to_string();
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        let for_thread = connection_id.clone();
        std::thread::spawn(move || {
            let result = ops::run_op(&invocation, &ops::prune_args(), &work_dir).map(|_| ());
            report(qt_thread, QString::from(""), result, for_thread);
        });
        FfiResult::default()
    }

    /// Open `node_id`'s `inspect` JSON as a read-only virtual document —
    /// synchronous (the round trip is a single small `inspect` call, same
    /// order of magnitude as `probe`), mirroring `LanguageService`'s own
    /// `virtualDocumentOpened` split: build the tab, then focus it. Works
    /// on a container node (`inspect <id>`) or a pod node (`pod inspect
    /// <id>`, [`container_core::pods::inspect_args`]) — every other kind
    /// is refused, same as before this grew the pod case.
    pub fn open_inspect(mut self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        let node_id_str = node_id.to_string();
        let container = parse_container_node_id(&node_id_str);
        let pod = parse_node_id(&node_id_str, NodeKind::Pod);
        let (connection_id, resource_id, kind_word, args) = match (container, pod) {
            (Some((connection_id, resource_id)), _) => {
                let args = vec!["inspect".to_string(), resource_id.clone()];
                (connection_id, resource_id, "container", args)
            }
            (None, Some((connection_id, resource_id))) => {
                let args = container_core::pods::inspect_args(&resource_id);
                (connection_id, resource_id, "pod", args)
            }
            (None, None) => {
                return errors::failure(
                    errors::CODE_INVALID_ARGUMENT,
                    format!("'{node_id_str}' is not a container or pod node"),
                );
            }
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let json = match container_core::session::inspect_json_with_args(
            &invocation,
            kind_word,
            &resource_id,
            &args,
            &work_dir,
        ) {
            Ok(json) => json,
            Err(err) => return errors::failure(errors::CODE_REFUSED, err.message),
        };
        let key = format!("{connection_id}/{kind_word}/{resource_id}/inspect.json");
        let opened = self
            .session
            .borrow_mut()
            .open_virtual_document("container", &key, &json);
        self.as_mut().virtual_document_opened(
            opened.id.raw(),
            QString::from(opened.title.as_str()),
            opened.newly_opened,
        );
        FfiResult::default()
    }

    /// One "Clean Up" menu entry (C4): `kind` is one of
    /// [`container_core::prune::CleanUpKind`]'s own ids
    /// (`"all"`/`"stopped-containers"`/`"unused-networks"`/
    /// `"unused-volumes"`/`"dangling-images"`/`"build-cache"`). Runs every
    /// command the kind needs, in order, stopping at the first failure.
    pub fn clean_up(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        kind: &QString,
    ) -> FfiResult {
        let connection_id = connection_id.to_string();
        let Some(kind) = parse_clean_up_kind(&kind.to_string()) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "unknown Clean Up kind");
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let engine = match service::connection_engine(&connection_id) {
            Ok(engine) => engine,
            Err(result) => return result,
        };
        if !container_core::prune::available(kind, engine) {
            return errors::failure(
                errors::CODE_REFUSED,
                "this Clean Up option is not available on this connection's engine",
            );
        }
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let mut result: Result<(), OpError> = Ok(());
            for command in container_core::prune::commands(kind, engine) {
                if let Err(err) = ops::run_op(&invocation, &command, &work_dir) {
                    result = Err(err);
                    break;
                }
            }
            report(qt_thread, QString::from(""), result, connection_id);
        });
        FfiResult::default()
    }

    pub fn processes(mut self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_container_node_id(&node_id_str) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not a container node"),
            );
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result = ops::run_op(&invocation, &ops::top_args(&resource_id), &work_dir);
            let queued =
                qt_thread.queue(
                    move |mut service: Pin<&mut ffi::ContainerService>| match result {
                        Ok(output) => {
                            let table = ops::parse_top(&String::from_utf8_lossy(&output.stdout));
                            service.as_mut().processes_ready(
                                QString::from(node_id_str.as_str()),
                                QString::from(table.titles.join("\t").as_str()),
                                table
                                    .rows
                                    .iter()
                                    .map(|row| ffi::FfiProcessRow {
                                        cells: QString::from(row.join("\t").as_str()),
                                    })
                                    .collect(),
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
            let _ = queued;
        });
        FfiResult::default()
    }
}

/// Shared shape for the six single-container lifecycle actions: parse the
/// node id, resolve its connection's `Invocation`, run `build_args`'s argv
/// on a worker thread, then report through `actionFinished` and
/// re-snapshot the connection (so the tree reflects the new state without
/// waiting for the next `events` line).
fn run_lifecycle_op(
    mut service: Pin<&mut ffi::ContainerService>,
    node_id: &QString,
    build_args: impl FnOnce(&str) -> Vec<String> + Send + 'static,
) -> FfiResult {
    let node_id_str = node_id.to_string();
    let Some((connection_id, resource_id)) = parse_container_node_id(&node_id_str) else {
        return errors::failure(
            errors::CODE_INVALID_ARGUMENT,
            format!("'{node_id_str}' is not a container node"),
        );
    };
    let invocation = match service::connection_invocation(&connection_id) {
        Ok(invocation) => invocation,
        Err(result) => return result,
    };
    let work_dir = service::work_dir();
    let qt_thread = service.as_mut().qt_thread();
    std::thread::spawn(move || {
        let args = build_args(&resource_id);
        let result: Result<(), OpError> = ops::run_op(&invocation, &args, &work_dir).map(|_| ());
        report(
            qt_thread,
            QString::from(node_id_str.as_str()),
            result,
            connection_id,
        );
    });
    FfiResult::default()
}

/// [`run_lifecycle_op`], for a pod node instead of a container node — same
/// shape, `parse_node_id(.., NodeKind::Pod)` in place of
/// [`parse_container_node_id`].
fn run_pod_lifecycle_op(
    mut service: Pin<&mut ffi::ContainerService>,
    node_id: &QString,
    build_args: impl FnOnce(&str) -> Vec<String> + Send + 'static,
) -> FfiResult {
    let node_id_str = node_id.to_string();
    let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Pod) else {
        return errors::failure(
            errors::CODE_INVALID_ARGUMENT,
            format!("'{node_id_str}' is not a pod node"),
        );
    };
    let invocation = match service::connection_invocation(&connection_id) {
        Ok(invocation) => invocation,
        Err(result) => return result,
    };
    let work_dir = service::work_dir();
    let qt_thread = service.as_mut().qt_thread();
    std::thread::spawn(move || {
        let args = build_args(&resource_id);
        let result: Result<(), OpError> = ops::run_op(&invocation, &args, &work_dir).map(|_| ());
        report(
            qt_thread,
            QString::from(node_id_str.as_str()),
            result,
            connection_id,
        );
    });
    FfiResult::default()
}

/// `machine start`/`machine stop` for `connection_id`'s own Podman
/// machine (its [`container_core::connection::ConnectionKind::
/// PodmanMachine`] name) — refused for any other connection kind, rather
/// than running the CLI against a machine name that does not apply here.
fn run_machine_op(
    mut service: Pin<&mut ffi::ContainerService>,
    connection_id: &QString,
    build_args: impl FnOnce(&str) -> Vec<String> + Send + 'static,
) -> FfiResult {
    let connection_id_str = connection_id.to_string();
    let Some(machine_name) = super::service::podman_machine_name(&connection_id_str) else {
        return errors::failure(
            errors::CODE_INVALID_ARGUMENT,
            format!("'{connection_id_str}' is not a Podman machine connection"),
        );
    };
    let invocation = match service::connection_invocation(&connection_id_str) {
        Ok(invocation) => invocation,
        Err(result) => return result,
    };
    let work_dir = service::work_dir();
    let qt_thread = service.as_mut().qt_thread();
    std::thread::spawn(move || {
        let args = build_args(&machine_name);
        let result: Result<(), OpError> = ops::run_op(&invocation, &args, &work_dir).map(|_| ());
        report(qt_thread, QString::from(""), result, connection_id_str);
    });
    FfiResult::default()
}

/// Emit `actionFinished` on the Qt thread and, on success, re-snapshot the
/// connection the action touched. `report`'s two call sites (a
/// per-container op and "Clean Up") differ only in whether there is a
/// connection id to refresh; the prune call passes it explicitly below.
fn report(
    qt_thread: cxx_qt::CxxQtThread<ffi::ContainerService>,
    node_id: QString,
    result: Result<(), OpError>,
    connection_id: String,
) {
    let (ok, message) = match result {
        Ok(()) => (true, String::new()),
        Err(err) => (false, err.message),
    };
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::ContainerService>| {
        service
            .as_mut()
            .action_finished(node_id, ok, QString::from(message.as_str()));
        let _ = service
            .as_mut()
            .refresh(&QString::from(connection_id.as_str()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_container_node_id() {
        assert_eq!(
            parse_container_node_id("d/container/3f2a9b"),
            Some(("d".to_string(), "3f2a9b".to_string()))
        );
    }

    #[test]
    fn parses_every_clean_up_kind_and_rejects_unknown_words() {
        use container_core::prune::CleanUpKind;
        assert_eq!(parse_clean_up_kind("all"), Some(CleanUpKind::All));
        assert_eq!(
            parse_clean_up_kind("stopped-containers"),
            Some(CleanUpKind::StoppedContainers)
        );
        assert_eq!(
            parse_clean_up_kind("unused-networks"),
            Some(CleanUpKind::UnusedNetworks)
        );
        assert_eq!(
            parse_clean_up_kind("unused-volumes"),
            Some(CleanUpKind::UnusedVolumes)
        );
        assert_eq!(
            parse_clean_up_kind("dangling-images"),
            Some(CleanUpKind::DanglingImages)
        );
        assert_eq!(
            parse_clean_up_kind("build-cache"),
            Some(CleanUpKind::BuildCache)
        );
        assert_eq!(parse_clean_up_kind("nonsense"), None);
    }

    #[test]
    fn rejects_a_non_container_node_id() {
        assert_eq!(parse_container_node_id("d/containers-group"), None);
        assert_eq!(parse_container_node_id("d/image/abc"), None);
        assert_eq!(parse_container_node_id("d"), None);
        assert_eq!(parse_container_node_id(""), None);
        assert_eq!(parse_container_node_id("d/container/"), None);
        assert_eq!(parse_container_node_id("/container/abc"), None);
    }
}
