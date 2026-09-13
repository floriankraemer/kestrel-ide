//! The Containers dock's tree, flattened: one [`TreeNode`] per row, parent
//! ids rather than nesting, in the exact order the view shows them.
//!
//! Every decision the tree makes lives here — which groups exist, what a
//! row says, which icon it gets, what its stable id is — so `ui-shell`'s
//! panel only builds `QTreeWidgetItem`s from the rows and `ContainerService`
//! only forwards them across the seam.

use std::time::SystemTime;

use crate::connection::Engine;
use crate::model::{self, ContainerStatus};
use crate::probe::EngineInfo;
use crate::snapshot::SnapshotView;

/// What kind of row this is. Crosses the seam as [`NodeKind::id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Connection,
    ContainersGroup,
    ImagesGroup,
    NetworksGroup,
    VolumesGroup,
    ComposeGroup,
    PodsGroup,
    Container,
    Image,
    Network,
    Volume,
    ComposeProject,
    ComposeService,
    Pod,
}

impl NodeKind {
    pub fn id(self) -> &'static str {
        match self {
            NodeKind::Connection => "connection",
            NodeKind::ContainersGroup => "containers-group",
            NodeKind::ImagesGroup => "images-group",
            NodeKind::NetworksGroup => "networks-group",
            NodeKind::VolumesGroup => "volumes-group",
            NodeKind::ComposeGroup => "compose-group",
            NodeKind::PodsGroup => "pods-group",
            NodeKind::Container => "container",
            NodeKind::Image => "image",
            NodeKind::Network => "network",
            NodeKind::Volume => "volume",
            NodeKind::ComposeProject => "compose-project",
            NodeKind::ComposeService => "compose-service",
            NodeKind::Pod => "pod",
        }
    }
}

/// Where a connection stands, as the tree paints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected(EngineInfo),
    Error(String),
}

impl ConnectionState {
    pub fn id(&self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "disconnected",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Connected(_) => "connected",
            ConnectionState::Error(_) => "error",
        }
    }

    /// The text beside the connection's name.
    pub fn label(&self) -> String {
        match self {
            ConnectionState::Disconnected => "disconnected".to_string(),
            ConnectionState::Connecting => "connecting...".to_string(),
            ConnectionState::Connected(info) => info.to_string(),
            ConnectionState::Error(message) => {
                // One row, one line: the hint lines behind the first one
                // stay in the tooltip and the Dashboard.
                format!("error: {}", message.lines().next().unwrap_or_default())
            }
        }
    }

    /// The full text for a tooltip: the whole error, hint lines included.
    pub fn tooltip(&self) -> String {
        match self {
            ConnectionState::Error(message) => format!("error: {message}"),
            other => other.label(),
        }
    }
}

/// One row. `id` is stable across snapshots (`<conn>/container/<id>`,
/// `<conn>/images-group`, ...) so the view can keep selection and expansion
/// through a wholesale rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: String,
    /// Empty for a connection row.
    pub parent_id: String,
    pub kind: NodeKind,
    pub name: String,
    /// Second column: `running (2 h)`, `exited (1)`, `3 items`, ...
    pub status: String,
    /// Third column: image, driver, size, working directory, ...
    pub detail: String,
    pub connection_id: String,
    /// What the engine calls this row: a container/image/network/pod id,
    /// a volume name, a compose project or service name, or the
    /// connection id. Empty for group rows. What "Copy ID" copies and what
    /// later tasks' operations address.
    pub resource_id: String,
    pub tooltip: String,
    /// The icon key the view looks up: `docker`, `podman`,
    /// `container-running`, `container-stopped`, `image`, `network`,
    /// `volume`, `compose`, `pod`.
    pub icon: &'static str,
}

/// One configured connection, as the tree needs it.
pub struct ConnectionRow<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub engine: Engine,
    pub state: &'a ConnectionState,
    /// `None` until the first snapshot arrives.
    pub view: Option<&'a SnapshotView<'a>>,
}

/// Flatten every connection, in the order given, into rows. `now` is what
/// ages are measured against.
pub fn flatten(connections: &[ConnectionRow<'_>], now: SystemTime) -> Vec<TreeNode> {
    let mut nodes = Vec::new();
    for connection in connections {
        let root_id = connection.id.to_string();
        nodes.push(TreeNode {
            id: root_id.clone(),
            parent_id: String::new(),
            kind: NodeKind::Connection,
            name: connection.name.to_string(),
            status: connection.state.label(),
            detail: String::new(),
            connection_id: root_id.clone(),
            resource_id: root_id.clone(),
            tooltip: connection.state.tooltip(),
            icon: match connection.engine {
                Engine::Docker => "docker",
                Engine::Podman => "podman",
            },
        });
        if let (ConnectionState::Connected(_), Some(view)) = (connection.state, connection.view) {
            push_groups(&mut nodes, connection, view, now);
        }
    }
    nodes
}

fn count_label(count: usize) -> String {
    format!("{count}")
}

fn push_groups(
    nodes: &mut Vec<TreeNode>,
    connection: &ConnectionRow<'_>,
    view: &SnapshotView<'_>,
    now: SystemTime,
) {
    let snapshot = view.snapshot;
    let conn = connection.id;
    let group = |nodes: &mut Vec<TreeNode>, kind: NodeKind, name: &str, count, icon| {
        let id = format!("{conn}/{}", kind.id());
        nodes.push(TreeNode {
            id: id.clone(),
            parent_id: conn.to_string(),
            kind,
            name: name.to_string(),
            status: count_label(count),
            detail: String::new(),
            connection_id: conn.to_string(),
            resource_id: String::new(),
            tooltip: String::new(),
            icon,
        });
        id
    };

    let group_id = group(
        nodes,
        NodeKind::ContainersGroup,
        "Containers",
        view.containers.len(),
        "container-running",
    );
    for &index in &view.containers {
        nodes.push(container_node(
            conn,
            &group_id,
            &snapshot.containers[index],
            now,
        ));
    }

    let group_id = group(
        nodes,
        NodeKind::ImagesGroup,
        "Images",
        view.images.len(),
        "image",
    );
    for &index in &view.images {
        let image = &snapshot.images[index];
        nodes.push(TreeNode {
            id: format!("{conn}/image/{}", image.id),
            parent_id: group_id.clone(),
            kind: NodeKind::Image,
            name: image.display_name(),
            status: model::human_age(&image.created, now),
            detail: model::human_size(image.size),
            connection_id: conn.to_string(),
            resource_id: image.id.clone(),
            tooltip: format!("{}\n{}", image.repo_tags.join("\n"), image.id),
            icon: "image",
        });
    }

    let group_id = group(
        nodes,
        NodeKind::NetworksGroup,
        "Networks",
        view.networks.len(),
        "network",
    );
    for &index in &view.networks {
        let network = &snapshot.networks[index];
        nodes.push(TreeNode {
            id: format!("{conn}/network/{}", network.id),
            parent_id: group_id.clone(),
            kind: NodeKind::Network,
            name: network.name.clone(),
            status: count_containers(network.containers.len()),
            detail: network.driver.clone(),
            connection_id: conn.to_string(),
            resource_id: network.id.clone(),
            tooltip: network.id.clone(),
            icon: "network",
        });
    }

    let group_id = group(
        nodes,
        NodeKind::VolumesGroup,
        "Volumes",
        view.volumes.len(),
        "volume",
    );
    for &index in &view.volumes {
        let volume = &snapshot.volumes[index];
        nodes.push(TreeNode {
            id: format!("{conn}/volume/{}", volume.name),
            parent_id: group_id.clone(),
            kind: NodeKind::Volume,
            name: volume.name.clone(),
            status: model::human_age(&volume.created_at, now),
            detail: volume.driver.clone(),
            connection_id: conn.to_string(),
            resource_id: volume.name.clone(),
            tooltip: volume.mountpoint.clone(),
            icon: "volume",
        });
    }

    if !view.compose.is_empty() {
        let group_id = group(
            nodes,
            NodeKind::ComposeGroup,
            "Compose",
            view.compose.len(),
            "compose",
        );
        for compose_view in &view.compose {
            let project = &snapshot.compose[compose_view.project];
            let project_id = format!("{conn}/compose/{}", project.name);
            let running = compose_view
                .services
                .iter()
                .flat_map(|service| service.containers.iter())
                .filter(|&&index| snapshot.containers[index].status() == ContainerStatus::Running)
                .count();
            let total: usize = compose_view
                .services
                .iter()
                .map(|service| service.containers.len())
                .sum();
            nodes.push(TreeNode {
                id: project_id.clone(),
                parent_id: group_id.clone(),
                kind: NodeKind::ComposeProject,
                name: project.name.clone(),
                status: format!("{running}/{total} running"),
                detail: project.working_dir.clone(),
                connection_id: conn.to_string(),
                resource_id: project.name.clone(),
                tooltip: project.config_files.join("\n"),
                icon: "compose",
            });
            for service_view in &compose_view.services {
                let service = &project.services[service_view.service];
                let service_running = service_view
                    .containers
                    .iter()
                    .filter(|&&index| {
                        snapshot.containers[index].status() == ContainerStatus::Running
                    })
                    .count();
                let image = service_view
                    .containers
                    .first()
                    .map(|&index| snapshot.containers[index].image.clone())
                    .unwrap_or_default();
                nodes.push(TreeNode {
                    id: format!("{project_id}/{}", service.name),
                    parent_id: project_id.clone(),
                    kind: NodeKind::ComposeService,
                    name: service.name.clone(),
                    status: format!("{service_running}/{}", service_view.containers.len()),
                    detail: image,
                    connection_id: conn.to_string(),
                    resource_id: service.name.clone(),
                    tooltip: String::new(),
                    icon: if service_running > 0 {
                        "container-running"
                    } else {
                        "container-stopped"
                    },
                });
            }
        }
    }

    if connection.engine == Engine::Podman {
        let group_id = group(nodes, NodeKind::PodsGroup, "Pods", view.pods.len(), "pod");
        for &index in &view.pods {
            let pod = &snapshot.pods[index];
            nodes.push(TreeNode {
                id: format!("{conn}/pod/{}", pod.id),
                parent_id: group_id.clone(),
                kind: NodeKind::Pod,
                name: pod.name.clone(),
                status: pod.status.to_lowercase(),
                detail: count_containers(pod.containers.len()),
                connection_id: conn.to_string(),
                resource_id: pod.id.clone(),
                tooltip: pod.id.clone(),
                icon: "pod",
            });
        }
    }
}

fn count_containers(count: usize) -> String {
    match count {
        1 => "1 container".to_string(),
        n => format!("{n} containers"),
    }
}

fn container_node(
    conn: &str,
    group_id: &str,
    container: &model::Container,
    now: SystemTime,
) -> TreeNode {
    let status = container.status();
    let since = match status {
        ContainerStatus::Running | ContainerStatus::Paused | ContainerStatus::Restarting => {
            model::human_age(&container.state.started_at, now)
        }
        ContainerStatus::Exited(_) | ContainerStatus::Dead => {
            model::human_age(&container.state.finished_at, now)
        }
        ContainerStatus::Created | ContainerStatus::Other(_) => {
            model::human_age(&container.created, now)
        }
    };
    let status_text = if since.is_empty() {
        status.label()
    } else {
        format!("{} ({since})", status.label())
    };
    TreeNode {
        id: format!("{conn}/container/{}", container.id),
        parent_id: group_id.to_string(),
        kind: NodeKind::Container,
        name: container.name.clone(),
        status: status_text,
        detail: container.image.clone(),
        connection_id: conn.to_string(),
        resource_id: container.id.clone(),
        tooltip: container.id.clone(),
        icon: if status.is_live() {
            "container-running"
        } else {
            "container-stopped"
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::fixtures::snapshot;
    use crate::snapshot::EngineSnapshot;
    use std::time::{Duration, UNIX_EPOCH};

    fn info(engine: Engine) -> EngineInfo {
        EngineInfo {
            engine,
            client_version: "29.6.2".to_string(),
            server_version: "29.6.2".to_string(),
            api_version: "1.53".to_string(),
        }
    }

    fn now() -> SystemTime {
        // 2026-09-13T00:00:00Z
        UNIX_EPOCH + Duration::from_secs(20_709 * 86_400)
    }

    fn kinds(nodes: &[TreeNode]) -> Vec<NodeKind> {
        nodes.iter().map(|node| node.kind).collect()
    }

    #[test]
    fn a_disconnected_connection_is_a_single_row() {
        let state = ConnectionState::Disconnected;
        let nodes = flatten(
            &[ConnectionRow {
                id: "local",
                name: "Docker (local)",
                engine: Engine::Docker,
                state: &state,
                view: None,
            }],
            now(),
        );
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "local");
        assert_eq!(nodes[0].parent_id, "");
        assert_eq!(nodes[0].status, "disconnected");
        assert_eq!(nodes[0].icon, "docker");
        assert_eq!(nodes[0].resource_id, "local");
    }

    #[test]
    fn connecting_and_error_states_label_the_row() {
        assert_eq!(ConnectionState::Connecting.label(), "connecting...");
        assert_eq!(ConnectionState::Connecting.id(), "connecting");
        assert_eq!(
            ConnectionState::Error("boom".to_string()).label(),
            "error: boom"
        );
        let with_hint = ConnectionState::Error("denied\nAdd your user to the group".to_string());
        assert_eq!(with_hint.label(), "error: denied");
        assert_eq!(
            with_hint.tooltip(),
            "error: denied\nAdd your user to the group"
        );
        assert_eq!(ConnectionState::Error(String::new()).id(), "error");
        assert_eq!(
            ConnectionState::Connected(info(Engine::Podman)).label(),
            "Podman Engine 29.6.2"
        );
    }

    #[test]
    fn groups_come_in_fixed_order_and_items_nest_under_them() {
        let snapshot = snapshot("docker");
        let view = snapshot.filter(true, true);
        let state = ConnectionState::Connected(info(Engine::Docker));
        let nodes = flatten(
            &[ConnectionRow {
                id: "local",
                name: "Docker (local)",
                engine: Engine::Docker,
                state: &state,
                view: Some(&view),
            }],
            now(),
        );

        let groups: Vec<(&str, &str)> = nodes
            .iter()
            .filter(|node| node.parent_id == "local")
            .map(|node| (node.name.as_str(), node.status.as_str()))
            .collect();
        assert_eq!(
            groups,
            vec![
                ("Containers", "6"),
                ("Images", "5"),
                ("Networks", "4"),
                ("Volumes", "3"),
                ("Compose", "2"),
            ],
            "no Pods group on Docker"
        );

        let containers: Vec<&str> = nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Container)
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(containers[0], "jagdonline-2d80272f-admin-1");
        assert_eq!(*containers.last().unwrap(), "redis");
        let redis = nodes.iter().find(|node| node.name == "redis").unwrap();
        assert_eq!(
            redis.id,
            format!("local/container/{}", snapshot.containers[5].id)
        );
        assert_eq!(redis.parent_id, "local/containers-group");
        assert_eq!(redis.resource_id, snapshot.containers[5].id);
        assert!(
            nodes[1].resource_id.is_empty(),
            "groups have no resource id"
        );
        assert_eq!(redis.status, "running (1 d)");
        assert_eq!(redis.detail, "redis:7.2-alpine");
        assert_eq!(redis.icon, "container-running");

        let web = nodes
            .iter()
            .find(|node| node.name == "jagdonline-2d80272f-web-1")
            .unwrap();
        assert!(web.status.starts_with("exited (255) ("), "{}", web.status);
        assert_eq!(web.icon, "container-stopped");

        let postgres = nodes
            .iter()
            .find(|node| node.name == "postgres:16-alpine")
            .unwrap();
        assert_eq!(postgres.kind, NodeKind::Image);
        assert_eq!(postgres.detail, "275 MB");
        assert_eq!(postgres.status, "1 y");

        let bridge = nodes.iter().find(|node| node.name == "bridge").unwrap();
        assert_eq!(bridge.kind, NodeKind::Network);
        assert_eq!(bridge.status, "0 containers");
        assert_eq!(bridge.detail, "bridge");
    }

    #[test]
    fn compose_services_nest_under_their_project() {
        let snapshot = snapshot("docker");
        let view = snapshot.filter(true, true);
        let state = ConnectionState::Connected(info(Engine::Docker));
        let nodes = flatten(
            &[ConnectionRow {
                id: "c",
                name: "d",
                engine: Engine::Docker,
                state: &state,
                view: Some(&view),
            }],
            now(),
        );
        let project = nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::ComposeProject && node.name == "jagdonline-2d80272f"
            })
            .unwrap();
        assert_eq!(project.parent_id, "c/compose-group");
        assert_eq!(project.status, "0/5 running");
        assert_eq!(
            project.detail,
            "/home/user/projects/jagdonline/.claude/worktrees/trophy-scoring"
        );
        let services: Vec<&TreeNode> = nodes
            .iter()
            .filter(|node| node.parent_id == project.id)
            .collect();
        assert_eq!(services.len(), 5);
        assert_eq!(services[0].name, "admin");
        assert_eq!(services[0].id, "c/compose/jagdonline-2d80272f/admin");
        assert_eq!(services[0].status, "0/1");
        assert_eq!(services[0].icon, "container-stopped");

        let chess = nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::ComposeProject && node.name != "jagdonline-2d80272f"
            })
            .unwrap();
        assert_eq!(chess.status, "1/1 running");
        let redis_service = nodes
            .iter()
            .find(|node| node.kind == NodeKind::ComposeService && node.parent_id == chess.id)
            .unwrap();
        assert_eq!(redis_service.icon, "container-running");
        assert_eq!(redis_service.detail, "redis:7.2-alpine");
    }

    #[test]
    fn podman_gets_a_pods_group_and_multiple_connections_stack() {
        let podman = snapshot("podman");
        let podman_view = podman.filter(true, true);
        let docker_state = ConnectionState::Error("socket missing".to_string());
        let podman_state = ConnectionState::Connected(info(Engine::Podman));
        let nodes = flatten(
            &[
                ConnectionRow {
                    id: "d",
                    name: "Docker",
                    engine: Engine::Docker,
                    state: &docker_state,
                    view: None,
                },
                ConnectionRow {
                    id: "p",
                    name: "Podman",
                    engine: Engine::Podman,
                    state: &podman_state,
                    view: Some(&podman_view),
                },
            ],
            now(),
        );
        assert_eq!(nodes[0].id, "d");
        assert_eq!(nodes[0].status, "error: socket missing");
        assert_eq!(nodes[1].id, "p");
        assert_eq!(nodes[1].icon, "podman");
        assert_eq!(nodes[1].status, "Podman Engine 29.6.2");
        let pods_group = nodes
            .iter()
            .find(|node| node.kind == NodeKind::PodsGroup)
            .unwrap();
        assert_eq!(pods_group.id, "p/pods-group");
        assert_eq!(pods_group.status, "1");
        let pod = nodes
            .iter()
            .find(|node| node.kind == NodeKind::Pod)
            .unwrap();
        assert_eq!(pod.name, "shop");
        assert_eq!(pod.status, "running");
        assert_eq!(pod.detail, "2 containers");
        assert!(kinds(&nodes).contains(&NodeKind::ComposeGroup));
        let paused = nodes.iter().find(|node| node.name == "shop_db_1").unwrap();
        assert!(paused.status.starts_with("paused ("), "{}", paused.status);
        let created = nodes.iter().find(|node| node.name == "shop_app_1").unwrap();
        assert!(
            created.status.starts_with("created ("),
            "{}",
            created.status
        );
        let untagged = nodes
            .iter()
            .find(|node| node.kind == NodeKind::Image && node.name == "ee55ff66aa77")
            .unwrap();
        assert_eq!(untagged.detail, "7.3 MB");
    }

    #[test]
    fn a_connected_engine_with_nothing_still_shows_the_groups() {
        let empty = EngineSnapshot::default();
        let view = empty.filter(true, true);
        let state = ConnectionState::Connected(info(Engine::Docker));
        let nodes = flatten(
            &[ConnectionRow {
                id: "x",
                name: "x",
                engine: Engine::Docker,
                state: &state,
                view: Some(&view),
            }],
            now(),
        );
        assert_eq!(
            kinds(&nodes),
            vec![
                NodeKind::Connection,
                NodeKind::ContainersGroup,
                NodeKind::ImagesGroup,
                NodeKind::NetworksGroup,
                NodeKind::VolumesGroup,
            ],
            "no Compose group without projects, no Pods on Docker"
        );
        assert!(nodes[1..].iter().all(|node| node.status == "0"));
    }

    #[test]
    fn every_kind_id_is_distinct() {
        let all = [
            NodeKind::Connection,
            NodeKind::ContainersGroup,
            NodeKind::ImagesGroup,
            NodeKind::NetworksGroup,
            NodeKind::VolumesGroup,
            NodeKind::ComposeGroup,
            NodeKind::PodsGroup,
            NodeKind::Container,
            NodeKind::Image,
            NodeKind::Network,
            NodeKind::Volume,
            NodeKind::ComposeProject,
            NodeKind::ComposeService,
            NodeKind::Pod,
        ];
        let mut ids: Vec<&str> = all.iter().map(|kind| kind.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
    }
}
