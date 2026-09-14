//! The container node's Dashboard tab and "Recreate with changes" (C9):
//! reads straight off the cached snapshot (no CLI call), the same
//! reasoning `networks.rs`/`volumes.rs`'s dashboard accessors already
//! document; `recreate_container` is the one action here that actually
//! runs the engine, on a worker thread like every other lifecycle action.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::recreate::{self, RunSpec};
use container_core::tree::NodeKind;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiContainerDashboard, FfiResult};

use super::actions::parse_node_id;
use super::service;

impl ffi::ContainerService {
    pub fn container_dashboard(&self, node_id: &QString) -> FfiContainerDashboard {
        let Some((connection_id, resource_id)) =
            parse_node_id(&node_id.to_string(), NodeKind::Container)
        else {
            return FfiContainerDashboard::default();
        };
        let connections = self.connections.borrow();
        let Some(snapshot) = connections
            .get(&connection_id)
            .and_then(|connection| connection.snapshot.as_ref())
        else {
            return FfiContainerDashboard::default();
        };
        let Some(container) = snapshot
            .containers
            .iter()
            .find(|container| container.id == resource_id)
        else {
            return FfiContainerDashboard::default();
        };
        let spec = RunSpec::from_inspect(container);
        FfiContainerDashboard {
            name: QString::from(container.name.as_str()),
            id: QString::from(container.id.as_str()),
            image: QString::from(container.image.as_str()),
            status: QString::from(container.state.status.as_str()),
            env: QString::from(recreate::format_env_lines(&spec.env).join("\n").as_str()),
            ports: QString::from(recreate::format_port_lines(&spec.ports).join("\n").as_str()),
            mounts: QString::from(
                recreate::format_mount_lines(&spec.mounts)
                    .join("\n")
                    .as_str(),
            ),
            network: QString::from(
                spec.networks
                    .first()
                    .map(|network| network.name.as_str())
                    .unwrap_or(""),
            ),
            restart_policy: QString::from(spec.restart_policy.as_str()),
        }
    }

    /// `env`/`ports`/`mounts`: `\n`-joined lines in
    /// [`FfiContainerDashboard`]'s own formats, whatever the Dashboard's
    /// tables currently hold. Everything else in the spec (image, cmd,
    /// entrypoint, network, restart policy, labels, ...) is re-read from
    /// the container's own `inspect` right before recreating, so an edit
    /// session that only touched the Env table never has to round-trip
    /// fields it never showed.
    pub fn recreate_container(
        mut self: Pin<&mut Self>,
        node_id: &QString,
        env: &QString,
        ports: &QString,
        mounts: &QString,
    ) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Container)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not a container node"),
            );
        };
        let spec = {
            let connections = self.connections.borrow();
            let Some(snapshot) = connections
                .get(&connection_id)
                .and_then(|connection| connection.snapshot.as_ref())
            else {
                return errors::failure(
                    errors::CODE_INVALID_ARGUMENT,
                    format!("connection '{connection_id}' has no snapshot"),
                );
            };
            let Some(container) = snapshot
                .containers
                .iter()
                .find(|container| container.id == resource_id)
            else {
                return errors::failure(
                    errors::CODE_INVALID_ARGUMENT,
                    format!("'{resource_id}' is not in the current snapshot"),
                );
            };
            if recreate::is_compose_managed(container) {
                return errors::failure(
                    errors::CODE_REFUSED,
                    "this container is managed by a compose project — edit its compose file instead",
                );
            }
            RunSpec::from_inspect(container)
        };
        let env_lines: Vec<String> = env.to_string().lines().map(str::to_string).collect();
        let port_lines: Vec<String> = ports.to_string().lines().map(str::to_string).collect();
        let mount_lines: Vec<String> = mounts.to_string().lines().map(str::to_string).collect();
        let edited = spec
            .with_env(recreate::parse_env_lines(&env_lines))
            .with_ports(recreate::parse_port_lines(&port_lines))
            .with_mounts(recreate::parse_mount_lines(&mount_lines));

        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let selinux_relabel = crate::bridge::convert::load_resolved_settings()
            .containers
            .selinux_relabel;
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result = recreate::recreate(
                &invocation,
                &resource_id,
                &edited,
                selinux_relabel,
                &work_dir,
            )
            .map(|_new_id| ());
            super::images::report_action(qt_thread, node_id_str, result, connection_id);
        });
        FfiResult::default()
    }
}
