//! Volume actions and Dashboard (C4): create/remove/prune, plus a
//! snapshot-backed dashboard — same shape as `networks.rs`.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::ops::{self, OpError};
use container_core::tree::NodeKind;
use container_core::volumes::{self, VolumeSpec};

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult, FfiVolumeDashboard, FfiVolumeSpec};

use super::actions::parse_node_id;
use super::images::{containers_to_ffi, report_action};
use super::service;

fn parse_pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

fn to_volume_spec(spec: &FfiVolumeSpec) -> VolumeSpec {
    VolumeSpec {
        name: spec.name.to_string(),
        driver: spec.driver.to_string(),
        labels: parse_pairs(&spec.labels.to_string()),
        options: parse_pairs(&spec.options.to_string()),
    }
}

impl ffi::ContainerService {
    pub fn create_volume(
        mut self: Pin<&mut Self>,
        connection_id: &QString,
        spec: &FfiVolumeSpec,
    ) -> FfiResult {
        let connection_id = connection_id.to_string();
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let args = volumes::create_args(&to_volume_spec(spec));
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result: Result<(), OpError> =
                ops::run_op(&invocation, &args, &work_dir).map(|_| ());
            report_action(qt_thread, String::new(), result, connection_id);
        });
        FfiResult::default()
    }

    pub fn remove_volume(mut self: Pin<&mut Self>, node_id: &QString, force: bool) -> FfiResult {
        let node_id_str = node_id.to_string();
        let Some((connection_id, resource_id)) = parse_node_id(&node_id_str, NodeKind::Volume)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("'{node_id_str}' is not a volume node"),
            );
        };
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let args = volumes::remove_args(&resource_id, force);
            let result: Result<(), OpError> =
                ops::run_op(&invocation, &args, &work_dir).map(|_| ());
            report_action(qt_thread, node_id_str, result, connection_id);
        });
        FfiResult::default()
    }

    pub fn prune_volumes(mut self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        let connection_id = connection_id.to_string();
        let invocation = match service::connection_invocation(&connection_id) {
            Ok(invocation) => invocation,
            Err(result) => return result,
        };
        let work_dir = service::work_dir();
        let qt_thread = self.as_mut().qt_thread();
        let for_thread = connection_id.clone();
        std::thread::spawn(move || {
            let result = ops::run_op(&invocation, &volumes::prune_args(), &work_dir).map(|_| ());
            report_action(qt_thread, String::new(), result, for_thread);
        });
        FfiResult::default()
    }

    pub fn volume_dashboard(&self, node_id: &QString) -> FfiVolumeDashboard {
        let Some((connection_id, resource_id)) =
            parse_node_id(&node_id.to_string(), NodeKind::Volume)
        else {
            return FfiVolumeDashboard::default();
        };
        let connections = self.connections.borrow();
        let Some(snapshot) = connections
            .get(&connection_id)
            .and_then(|connection| connection.snapshot.as_ref())
        else {
            return FfiVolumeDashboard::default();
        };
        let Some(volume) = snapshot
            .volumes
            .iter()
            .find(|volume| volume.name == resource_id)
        else {
            return FfiVolumeDashboard::default();
        };
        let using = volumes::containers_using(&volume.name, &snapshot.containers);
        let labels = volume
            .labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        FfiVolumeDashboard {
            name: QString::from(volume.name.as_str()),
            driver: QString::from(volume.driver.as_str()),
            mountpoint: QString::from(volume.mountpoint.as_str()),
            containers: QString::from(containers_to_ffi(&connection_id, &using).as_str()),
            labels: QString::from(labels.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_value_pairs_skipping_blanks_and_bad_entries() {
        assert_eq!(
            parse_pairs("type=nfs\n\nbad-entry\n o = v "),
            vec![
                ("type".to_string(), "nfs".to_string()),
                ("o".to_string(), "v".to_string())
            ]
        );
    }
}
