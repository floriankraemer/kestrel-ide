//! The Containers dock's tree, flattened: one [`TreeNode`] per row, parent
//! ids rather than nesting, in the exact order the view shows them.
//!
//! Every *decision* the tree makes lives here — which groups exist, which
//! status a row is in, which age bucket applies, which icon it gets, what
//! its stable id is. The *words* do not: a row carries discrete data
//! ([`NodeStatus`], an exit code, an [`Age`], counts, bytes) and the view
//! turns them into translated text, so no English leaves this crate.

use std::time::SystemTime;

use crate::connection::Engine;
use crate::model::{self, Age, ContainerStatus};
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

/// Where a connection stands.
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
}

/// A row's status, as data. The view owns the word for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Rows with no status of their own (groups, images, volumes).
    None,
    Disconnected,
    Connecting,
    /// `status_text` carries the server version.
    Connected,
    /// `status_text` carries the error message (hint lines included).
    Error,
    Running,
    Paused,
    Restarting,
    /// `exit_code` carries the code.
    Exited,
    Created,
    Dead,
    /// An engine status word this crate does not classify — a Podman pod's
    /// `Degraded`, an undocumented container state; `status_text` carries
    /// it verbatim.
    Other,
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
    /// The engine's own name for the row (container name, image tag,
    /// network/volume/project/service/pod name, connection name). Group
    /// rows have no name — the view labels them by `kind`.
    pub name: String,
    pub connection_id: String,
    /// What the engine calls this row: a container/image/network/pod id,
    /// a volume name, a compose project or service name, or the
    /// connection id. Empty for group rows. What "Copy ID" copies and what
    /// later tasks' operations address.
    pub resource_id: String,
    /// The icon key the view looks up: `docker`, `podman`,
    /// `container-running`, `container-stopped`, `image`, `network`,
    /// `volume`, `compose`, `pod`.
    pub icon: &'static str,
    pub status: NodeStatus,
    /// Engine-supplied text that goes with `status` (see the variants).
    pub status_text: String,
    /// With `NodeStatus::Exited`.
    pub exit_code: i64,
    /// How long the row has been in its status (containers), or how old it
    /// is (images, volumes).
    pub age: Option<Age>,
    /// Group rows: items in the group. Networks and pods: connected
    /// containers.
    pub count: Option<usize>,
    /// Compose projects and services: `(running, total)` containers.
    pub running: Option<(usize, usize)>,
    /// Images: size in bytes.
    pub size_bytes: Option<u64>,
    /// Engine text for the third column: a container's image reference, a
    /// network's or volume's driver, a project's working directory, a
    /// service's image, an engine's product name (`Docker`/`Podman`).
    pub detail: String,
    /// Engine text for the tooltip: full ids, tags, mount points, config
    /// files. The view adds nothing of its own.
    pub tooltip: String,
}

impl TreeNode {
    fn new(id: String, parent_id: String, kind: NodeKind, connection_id: &str) -> Self {
        TreeNode {
            id,
            parent_id,
            kind,
            name: String::new(),
            connection_id: connection_id.to_string(),
            resource_id: String::new(),
            icon: "container-stopped",
            status: NodeStatus::None,
            status_text: String::new(),
            exit_code: 0,
            age: None,
            count: None,
            running: None,
            size_bytes: None,
            detail: String::new(),
            tooltip: String::new(),
        }
    }
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
        let mut root = TreeNode::new(
            connection.id.to_string(),
            String::new(),
            NodeKind::Connection,
            connection.id,
        );
        root.name = connection.name.to_string();
        root.resource_id = connection.id.to_string();
        root.icon = match connection.engine {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
        };
        root.detail = match connection.engine {
            Engine::Docker => "Docker",
            Engine::Podman => "Podman",
        }
        .to_string();
        match connection.state {
            ConnectionState::Disconnected => root.status = NodeStatus::Disconnected,
            ConnectionState::Connecting => root.status = NodeStatus::Connecting,
            ConnectionState::Connected(info) => {
                root.status = NodeStatus::Connected;
                root.status_text = info.server_version.clone();
            }
            ConnectionState::Error(message) => {
                root.status = NodeStatus::Error;
                root.status_text = message.clone();
                root.tooltip = message.clone();
            }
        }
        nodes.push(root);
        // A connection that dropped into `Error` (or is `Connecting`
        // again after having been connected) keeps whatever snapshot it
        // last had rather than hiding its children: the status text on
        // the connection row already says something is wrong, and losing
        // the whole tree under it on every hiccup would be a worse signal
        // than a stale-but-recognisable one. Only `Disconnected` clears
        // the snapshot (`ContainerServiceRust::disconnect_engine`), which
        // is what actually empties this.
        if let Some(view) = connection.view {
            push_groups(&mut nodes, connection, view, now);
        }
    }
    nodes
}

/// Which actions apply to a container node in `status` — the *rule* lives
/// here (Rust), never in `cpp/`: a view only reads the flags this returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeActions {
    pub can_start: bool,
    pub can_stop: bool,
    pub can_restart: bool,
    pub can_pause: bool,
    pub can_unpause: bool,
    pub can_remove: bool,
}

/// The actions matrix for a container node's current [`NodeStatus`].
/// `Running` is the only state that can be paused; `Paused` is the only
/// one that can be unpaused; `Restarting` allows nothing (an operation is
/// already in flight) except Remove, which is always available — even a
/// stuck container can be force-removed.
pub fn actions_for(status: NodeStatus) -> NodeActions {
    match status {
        NodeStatus::Running => NodeActions {
            can_stop: true,
            can_restart: true,
            can_pause: true,
            can_remove: true,
            ..NodeActions::default()
        },
        NodeStatus::Paused => NodeActions {
            can_unpause: true,
            can_stop: true,
            can_remove: true,
            ..NodeActions::default()
        },
        NodeStatus::Exited | NodeStatus::Created | NodeStatus::Dead => NodeActions {
            can_start: true,
            can_remove: true,
            ..NodeActions::default()
        },
        NodeStatus::Restarting => NodeActions {
            can_remove: true,
            ..NodeActions::default()
        },
        NodeStatus::Other => NodeActions {
            can_start: true,
            can_stop: true,
            can_restart: true,
            can_remove: true,
            ..NodeActions::default()
        },
        NodeStatus::None
        | NodeStatus::Disconnected
        | NodeStatus::Connecting
        | NodeStatus::Connected
        | NodeStatus::Error => NodeActions::default(),
    }
}

fn push_groups(
    nodes: &mut Vec<TreeNode>,
    connection: &ConnectionRow<'_>,
    view: &SnapshotView<'_>,
    now: SystemTime,
) {
    let snapshot = view.snapshot;
    let conn = connection.id;
    let group = |nodes: &mut Vec<TreeNode>, kind: NodeKind, count: usize, icon| {
        let id = format!("{conn}/{}", kind.id());
        let mut node = TreeNode::new(id.clone(), conn.to_string(), kind, conn);
        node.icon = icon;
        node.count = Some(count);
        nodes.push(node);
        id
    };

    let group_id = group(
        nodes,
        NodeKind::ContainersGroup,
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

    let group_id = group(nodes, NodeKind::ImagesGroup, view.images.len(), "image");
    for &index in &view.images {
        let image = &snapshot.images[index];
        let mut node = TreeNode::new(
            format!("{conn}/image/{}", image.id),
            group_id.clone(),
            NodeKind::Image,
            conn,
        );
        node.name = image.display_name();
        node.resource_id = image.id.clone();
        node.icon = "image";
        node.age = model::age(&image.created, now);
        node.size_bytes = Some(image.size);
        node.tooltip = format!("{}\n{}", image.repo_tags.join("\n"), image.id);
        nodes.push(node);
    }

    let group_id = group(
        nodes,
        NodeKind::NetworksGroup,
        view.networks.len(),
        "network",
    );
    for &index in &view.networks {
        let network = &snapshot.networks[index];
        let mut node = TreeNode::new(
            format!("{conn}/network/{}", network.id),
            group_id.clone(),
            NodeKind::Network,
            conn,
        );
        node.name = network.name.clone();
        node.resource_id = network.id.clone();
        node.icon = "network";
        node.count = Some(network.containers.len());
        node.detail = network.driver.clone();
        node.tooltip = network.id.clone();
        nodes.push(node);
    }

    let group_id = group(nodes, NodeKind::VolumesGroup, view.volumes.len(), "volume");
    for &index in &view.volumes {
        let volume = &snapshot.volumes[index];
        let mut node = TreeNode::new(
            format!("{conn}/volume/{}", volume.name),
            group_id.clone(),
            NodeKind::Volume,
            conn,
        );
        node.name = volume.name.clone();
        node.resource_id = volume.name.clone();
        node.icon = "volume";
        node.age = model::age(&volume.created_at, now);
        node.detail = volume.driver.clone();
        node.tooltip = volume.mountpoint.clone();
        nodes.push(node);
    }

    if !view.compose.is_empty() {
        let group_id = group(nodes, NodeKind::ComposeGroup, view.compose.len(), "compose");
        for compose_view in &view.compose {
            let project = &snapshot.compose[compose_view.project];
            let project_id = format!("{conn}/compose/{}", project.name);
            let running_in = |containers: &[usize]| {
                containers
                    .iter()
                    .filter(|&&index| {
                        snapshot.containers[index].status() == ContainerStatus::Running
                    })
                    .count()
            };
            let total: usize = compose_view
                .services
                .iter()
                .map(|service| service.containers.len())
                .sum();
            let running: usize = compose_view
                .services
                .iter()
                .map(|service| running_in(&service.containers))
                .sum();
            let mut node = TreeNode::new(
                project_id.clone(),
                group_id.clone(),
                NodeKind::ComposeProject,
                conn,
            );
            node.name = project.name.clone();
            node.resource_id = project.name.clone();
            node.icon = "compose";
            node.running = Some((running, total));
            node.detail = project.working_dir.clone();
            node.tooltip = project.config_files.join("\n");
            nodes.push(node);

            for service_view in &compose_view.services {
                let service = &project.services[service_view.service];
                let service_running = running_in(&service_view.containers);
                let mut node = TreeNode::new(
                    format!("{project_id}/{}", service.name),
                    project_id.clone(),
                    NodeKind::ComposeService,
                    conn,
                );
                node.name = service.name.clone();
                node.resource_id = service.name.clone();
                node.icon = if service_running > 0 {
                    "container-running"
                } else {
                    "container-stopped"
                };
                node.running = Some((service_running, service_view.containers.len()));
                node.detail = service_view
                    .containers
                    .first()
                    .map(|&index| snapshot.containers[index].image.clone())
                    .unwrap_or_default();
                nodes.push(node);
            }
        }
    }

    if connection.engine == Engine::Podman {
        let group_id = group(nodes, NodeKind::PodsGroup, view.pods.len(), "pod");
        for &index in &view.pods {
            let pod = &snapshot.pods[index];
            let mut node = TreeNode::new(
                format!("{conn}/pod/{}", pod.id),
                group_id.clone(),
                NodeKind::Pod,
                conn,
            );
            node.name = pod.name.clone();
            node.resource_id = pod.id.clone();
            node.icon = "pod";
            node.status = pod_status(&pod.status);
            node.status_text = pod.status.clone();
            node.count = Some(pod.containers.len());
            node.tooltip = pod.id.clone();
            nodes.push(node);
        }
    }
}

/// `podman pod ls` prints `Running`, `Exited`, `Created`, `Paused`,
/// `Degraded`, `Stopped`, ... — the ones with a container analogue map to
/// it, the rest stay `Other` with the word in `status_text`.
fn pod_status(status: &str) -> NodeStatus {
    match status.to_ascii_lowercase().as_str() {
        "running" => NodeStatus::Running,
        "paused" => NodeStatus::Paused,
        "exited" | "stopped" => NodeStatus::Exited,
        "created" => NodeStatus::Created,
        "dead" => NodeStatus::Dead,
        _ => NodeStatus::Other,
    }
}

fn container_node(
    conn: &str,
    group_id: &str,
    container: &model::Container,
    now: SystemTime,
) -> TreeNode {
    let status = container.status();
    let mut node = TreeNode::new(
        format!("{conn}/container/{}", container.id),
        group_id.to_string(),
        NodeKind::Container,
        conn,
    );
    node.name = container.name.clone();
    node.resource_id = container.id.clone();
    node.icon = if status.is_live() {
        "container-running"
    } else {
        "container-stopped"
    };
    node.detail = container.image.clone();
    node.tooltip = container.id.clone();
    // The age is "since when": since it started for a live container,
    // since it finished for a stopped one, since creation otherwise.
    let since = match &status {
        ContainerStatus::Running | ContainerStatus::Paused | ContainerStatus::Restarting => {
            &container.state.started_at
        }
        ContainerStatus::Exited(_) | ContainerStatus::Dead => &container.state.finished_at,
        ContainerStatus::Created | ContainerStatus::Other(_) => &container.created,
    };
    node.age = model::age(since, now);
    node.status = match &status {
        ContainerStatus::Running => NodeStatus::Running,
        ContainerStatus::Paused => NodeStatus::Paused,
        ContainerStatus::Restarting => NodeStatus::Restarting,
        ContainerStatus::Exited(code) => {
            node.exit_code = *code;
            NodeStatus::Exited
        }
        ContainerStatus::Created => NodeStatus::Created,
        ContainerStatus::Dead => NodeStatus::Dead,
        ContainerStatus::Other(word) => {
            node.status_text = word.clone();
            NodeStatus::Other
        }
    };
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgeUnit;
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

    fn age(unit: AgeUnit, value: u64) -> Option<Age> {
        Some(Age { unit, value })
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
        assert_eq!(nodes[0].status, NodeStatus::Disconnected);
        assert_eq!(nodes[0].icon, "docker");
        assert_eq!(nodes[0].resource_id, "local");
        assert_eq!(nodes[0].detail, "Docker");
    }

    #[test]
    fn connection_states_map_to_row_statuses_with_their_text() {
        let error = ConnectionState::Error("denied\nAdd your user to the group".to_string());
        let connecting = ConnectionState::Connecting;
        let connected = ConnectionState::Connected(info(Engine::Podman));
        let nodes = flatten(
            &[
                ConnectionRow {
                    id: "a",
                    name: "a",
                    engine: Engine::Docker,
                    state: &error,
                    view: None,
                },
                ConnectionRow {
                    id: "b",
                    name: "b",
                    engine: Engine::Docker,
                    state: &connecting,
                    view: None,
                },
                ConnectionRow {
                    id: "c",
                    name: "c",
                    engine: Engine::Podman,
                    state: &connected,
                    view: None,
                },
            ],
            now(),
        );
        assert_eq!(nodes[0].status, NodeStatus::Error);
        assert_eq!(nodes[0].status_text, "denied\nAdd your user to the group");
        assert_eq!(nodes[0].tooltip, nodes[0].status_text);
        assert_eq!(nodes[1].status, NodeStatus::Connecting);
        assert_eq!(nodes[2].status, NodeStatus::Connected);
        assert_eq!(nodes[2].status_text, "29.6.2");
        assert_eq!(nodes[2].detail, "Podman");
        assert_eq!(nodes[2].icon, "podman");
        assert_eq!(
            nodes.len(),
            3,
            "connected without a snapshot has no children yet"
        );
        assert_eq!(ConnectionState::Connecting.id(), "connecting");
        assert_eq!(ConnectionState::Error(String::new()).id(), "error");
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

        let groups: Vec<(NodeKind, Option<usize>)> = nodes
            .iter()
            .filter(|node| node.parent_id == "local")
            .map(|node| (node.kind, node.count))
            .collect();
        assert_eq!(
            groups,
            vec![
                (NodeKind::ContainersGroup, Some(6)),
                (NodeKind::ImagesGroup, Some(5)),
                (NodeKind::NetworksGroup, Some(4)),
                (NodeKind::VolumesGroup, Some(3)),
                (NodeKind::ComposeGroup, Some(2)),
            ],
            "no Pods group on Docker"
        );
        assert!(
            nodes[1].name.is_empty(),
            "groups carry no name; the view labels them by kind"
        );
        assert!(
            nodes[1].resource_id.is_empty(),
            "groups have no resource id"
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
        assert_eq!(redis.status, NodeStatus::Running);
        assert_eq!(redis.age, age(AgeUnit::Days, 1), "since it started");
        assert_eq!(redis.detail, "redis:7.2-alpine");
        assert_eq!(redis.icon, "container-running");

        let web = nodes
            .iter()
            .find(|node| node.name == "jagdonline-2d80272f-web-1")
            .unwrap();
        assert_eq!(web.status, NodeStatus::Exited);
        assert_eq!(web.exit_code, 255);
        assert_eq!(
            web.age.map(|age| age.unit),
            Some(AgeUnit::Days),
            "since it finished"
        );
        assert_eq!(web.icon, "container-stopped");

        let postgres = nodes
            .iter()
            .find(|node| node.name == "postgres:16-alpine")
            .unwrap();
        assert_eq!(postgres.kind, NodeKind::Image);
        assert_eq!(postgres.status, NodeStatus::None);
        assert_eq!(postgres.size_bytes, Some(274_852_063));
        assert_eq!(postgres.age, age(AgeUnit::Years, 1));

        let bridge = nodes.iter().find(|node| node.name == "bridge").unwrap();
        assert_eq!(bridge.kind, NodeKind::Network);
        assert_eq!(bridge.count, Some(0));
        assert_eq!(bridge.detail, "bridge");

        let volume = nodes
            .iter()
            .find(|node| node.kind == NodeKind::Volume)
            .unwrap();
        assert_eq!(volume.detail, "local");
        assert!(volume.age.is_some());
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
        assert_eq!(project.running, Some((0, 5)));
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
        assert_eq!(services[0].running, Some((0, 1)));
        assert_eq!(services[0].icon, "container-stopped");

        let chess = nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::ComposeProject && node.name != "jagdonline-2d80272f"
            })
            .unwrap();
        assert_eq!(chess.running, Some((1, 1)));
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
        assert_eq!(nodes[0].status, NodeStatus::Error);
        assert_eq!(nodes[1].id, "p");
        assert_eq!(nodes[1].icon, "podman");
        let pods_group = nodes
            .iter()
            .find(|node| node.kind == NodeKind::PodsGroup)
            .unwrap();
        assert_eq!(pods_group.id, "p/pods-group");
        assert_eq!(pods_group.count, Some(1));
        let pod = nodes
            .iter()
            .find(|node| node.kind == NodeKind::Pod)
            .unwrap();
        assert_eq!(pod.name, "shop");
        assert_eq!(pod.status, NodeStatus::Running);
        assert_eq!(
            pod.status_text, "Running",
            "the engine's own word rides along"
        );
        assert_eq!(pod.count, Some(2));
        assert!(kinds(&nodes).contains(&NodeKind::ComposeGroup));
        let paused = nodes.iter().find(|node| node.name == "shop_db_1").unwrap();
        assert_eq!(paused.status, NodeStatus::Paused);
        assert_eq!(paused.icon, "container-running");
        let created = nodes.iter().find(|node| node.name == "shop_app_1").unwrap();
        assert_eq!(created.status, NodeStatus::Created);
        assert_eq!(
            created.age.map(|age| age.unit),
            Some(AgeUnit::Days),
            "since creation"
        );
        let scratch = nodes.iter().find(|node| node.name == "scratch").unwrap();
        assert_eq!(scratch.status, NodeStatus::Exited);
        assert_eq!(scratch.exit_code, 3);
        let untagged = nodes
            .iter()
            .find(|node| node.kind == NodeKind::Image && node.name == "ee55ff66aa77")
            .unwrap();
        assert_eq!(untagged.size_bytes, Some(7_340_032));
    }

    #[test]
    fn pod_status_words_classify_case_insensitively() {
        assert_eq!(pod_status("Running"), NodeStatus::Running);
        assert_eq!(pod_status("Exited"), NodeStatus::Exited);
        assert_eq!(pod_status("stopped"), NodeStatus::Exited);
        assert_eq!(pod_status("Created"), NodeStatus::Created);
        assert_eq!(pod_status("Paused"), NodeStatus::Paused);
        assert_eq!(pod_status("Dead"), NodeStatus::Dead);
        assert_eq!(pod_status("Degraded"), NodeStatus::Other);
    }

    #[test]
    fn an_unclassified_container_state_is_other_with_the_engine_word() {
        let raw = serde_json::json!({
            "Id": "x", "Name": "/odd", "State": {"Status": "removing"}
        });
        let snapshot = EngineSnapshot::from_parts(
            vec![model::Container::from_value(raw).unwrap()],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let view = snapshot.filter(true, true);
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
        let odd = nodes.iter().find(|node| node.name == "odd").unwrap();
        assert_eq!(odd.status, NodeStatus::Other);
        assert_eq!(odd.status_text, "removing");
        assert_eq!(odd.age, None, "no timestamps at all");
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
        assert!(nodes[1..].iter().all(|node| node.count == Some(0)));
    }

    #[test]
    fn an_error_state_keeps_the_last_snapshot_visible_instead_of_hiding_children() {
        // C3: reconnecting must not blank out the dock — the status text
        // on the connection row is the signal, the tree underneath stays.
        let docker = snapshot("docker");
        let view = docker.filter(true, true);
        let state = ConnectionState::Error("connection reset".to_string());
        let nodes = flatten(
            &[ConnectionRow {
                id: "d",
                name: "Docker",
                engine: Engine::Docker,
                state: &state,
                view: Some(&view),
            }],
            now(),
        );
        assert_eq!(nodes[0].status, NodeStatus::Error);
        assert!(
            nodes
                .iter()
                .any(|node| node.kind == NodeKind::ContainersGroup),
            "children must still be present under an errored connection \
             that has a stale snapshot"
        );
    }

    #[test]
    fn a_disconnected_connection_with_no_snapshot_has_no_children() {
        let state = ConnectionState::Disconnected;
        let nodes = flatten(
            &[ConnectionRow {
                id: "d",
                name: "Docker",
                engine: Engine::Docker,
                state: &state,
                view: None,
            }],
            now(),
        );
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn node_actions_matrix_per_status() {
        let running = actions_for(NodeStatus::Running);
        assert!(running.can_stop && running.can_restart && running.can_pause && running.can_remove);
        assert!(!running.can_start && !running.can_unpause);

        let paused = actions_for(NodeStatus::Paused);
        assert!(paused.can_unpause && paused.can_stop && paused.can_remove);
        assert!(!paused.can_start && !paused.can_restart && !paused.can_pause);

        for stopped in [NodeStatus::Exited, NodeStatus::Created, NodeStatus::Dead] {
            let actions = actions_for(stopped);
            assert!(actions.can_start && actions.can_remove);
            assert!(!actions.can_stop && !actions.can_restart && !actions.can_pause);
        }

        let restarting = actions_for(NodeStatus::Restarting);
        assert!(restarting.can_remove);
        assert!(!restarting.can_start && !restarting.can_stop && !restarting.can_pause);

        for inert in [
            NodeStatus::None,
            NodeStatus::Disconnected,
            NodeStatus::Connecting,
            NodeStatus::Connected,
            NodeStatus::Error,
        ] {
            assert_eq!(actions_for(inert), NodeActions::default());
        }
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
