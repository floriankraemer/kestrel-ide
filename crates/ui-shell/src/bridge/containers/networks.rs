//! Network actions and Dashboard (C4): create/remove/prune, plus a
//! snapshot-backed dashboard — no CLI call beyond the mutations, same
//! reasoning as `images.rs`'s labels/dashboard accessors.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::networks::{self, NetworkSpec};
use container_core::ops::{self, OpError};
use container_core::tree::NodeKind;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiNetworkDashboard, FfiNetworkSpec, FfiResult};

use super::actions::parse_node_id;
use super::images::containers_to_ffi;
use super::service;

/// `"\n"`-joined `key=value` (`FfiNetworkSpec::labels`/`FfiVolumeSpec::
/// labels`'s own convention) back into pairs, blank lines and entries with
/// no `=` skipped.
fn parse_labels(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

fn to_network_spec(spec: &FfiNetworkSpec) -> NetworkSpec {
    NetworkSpec {
        name: spec.name.to_string(),
        driver: spec.driver.to_string(),
        subnet: spec.subnet.to_string(),
        gateway: spec.gateway.to_string(),
        internal: spec.internal,
        attachable: spec.attachable,
        labels: parse_labels(&spec.labels.to_string()),
    }
}

impl ffi::ContainerService {
    pub fn create_network(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        spec: &FfiNetworkSpec,
    ) -> FfiResult {
        let connection_id = connection_id.to_string();
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let args = networks::create_args(&to_network_spec(spec));
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result: Result<(), OpError> =
                ops::run_op(&invocation, &args, &work_dir).map(|_| ());
            super::images::report_action(qt_thread, String::new(), result, connection_id);
        });
        FfiResult::default()
    }

    pub fn remove_network(mut self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Network)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not a network node"),
            );
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let args = networks::remove_args(&resource_id);
            let result: Result<(), OpError> =
                ops::run_op(&invocation, &args, &work_dir).map(|_| ());
            super::images::report_action(qt_thread, node_id_str, result, connection_id);
        });
        FfiResult::default()
    }

    pub fn prune_networks(mut self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        let connection_id = connection_id.to_string();
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        let for_thread = connection_id.clone();
        std::thread::spawn(move || {
            let result = ops::run_op(&invocation, &networks::prune_args(), &work_dir).map(|_| ());
            super::images::report_action(qt_thread, String::new(), result, for_thread);
        });
        FfiResult::default()
    }

    pub fn network_dashboard(&self, node_id: &QString) -> FfiNetworkDashboard {
        let Some((connection_id, resource_id)) =
            parse_node_id(&node_id.to_string(), NodeKind::Network)
        else {
            return FfiNetworkDashboard::default();
        };
        let connections = self.connections.borrow();
        let Some(snapshot) = connections
            .get(&connection_id)
            .and_then(|connection| connection.snapshot.as_ref())
        else {
            return FfiNetworkDashboard::default();
        };
        let Some(network) = snapshot
            .networks
            .iter()
            .find(|network| network.id == resource_id)
        else {
            return FfiNetworkDashboard::default();
        };
        let using: Vec<_> = snapshot
            .containers
            .iter()
            .filter(|container| network.containers.contains_key(&container.id))
            .collect();
        let labels = network
            .labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        FfiNetworkDashboard {
            name: QString::from(network.name.as_str()),
            id: QString::from(network.id.as_str()),
            driver: QString::from(network.driver.as_str()),
            scope: QString::from(network.scope.as_str()),
            subnets: QString::from(
                networks::subnets_and_gateways(&network.raw)
                    .iter()
                    .map(|(subnet, gateway)| format!("{subnet}\t{gateway}"))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .as_str(),
            ),
            containers: QString::from(containers_to_ffi(&connection_id, &using).as_str()),
            labels: QString::from(labels.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_value_labels_skipping_blanks_and_bad_entries() {
        assert_eq!(
            parse_labels("env=dev\n\nteam = checkout\nnoequals"),
            vec![
                ("env".to_string(), "dev".to_string()),
                ("team".to_string(), "checkout".to_string()),
            ]
        );
    }
}
