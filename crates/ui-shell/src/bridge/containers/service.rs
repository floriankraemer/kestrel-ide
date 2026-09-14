//! `ContainerService` (C2): the QObject behind the Containers dock.
//!
//! Translation only. Which connections exist is `app-config`'s answer,
//! what they contain is `container_core::watcher`'s, and what the tree
//! shows is `container_core::tree`'s — this adapter starts and stops
//! watchers, forwards their events onto the Qt thread, and re-flattens the
//! rows when asked. One watcher thread per connected connection plus one
//! forwarder thread that turns its `mpsc` events into `qt_thread().queue`
//! closures, exactly as `TestServiceRust`'s `QtSink` does for a test run.
//!
//! Nothing connects by itself: the dock starts with every connection
//! disconnected and the user clicks Connect (JetBrains' behaviour).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::SystemTime;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use container_core::connection::ConnectionConfig;
use container_core::model::{Age, AgeUnit};
use container_core::snapshot::EngineSnapshot;
use container_core::tree::{self, ConnectionRow, ConnectionState};
use container_core::watcher::{self, ContainerEvent, ContainerEventKind, WatcherHandle};

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};
use crate::bridge::settings::commit_to_project;

/// One connection this session has touched. Connections that were never
/// connected have no entry and read as `Disconnected`.
pub(crate) struct Connection {
    pub(crate) state: ConnectionState,
    pub(crate) snapshot: Option<EngineSnapshot>,
    pub(crate) handle: Option<WatcherHandle>,
    /// Bumped on every connect/disconnect so an event from a watcher that
    /// was already stopped cannot revive its row.
    generation: u64,
}

pub struct ContainerServiceRust {
    /// `pub(crate)`: C3's `actions.rs`/`sessions.rs` need a connection's
    /// `WatcherHandle` (to trigger a re-snapshot after an action finishes)
    /// the same way this file already does — a second `impl
    /// ffi::ContainerService` block in each of those files, exactly the
    /// split `bridge/language/{mod,lsp_surface,refactor}.rs` already uses
    /// for one QObject's surface across files under the size ratchet.
    pub(crate) connections: RefCell<BTreeMap<String, Connection>>,
    /// `(show_stopped, show_untagged)`, read from settings on first use
    /// and kept in step by `set_filter`.
    filter: RefCell<Option<(bool, bool)>>,
    search: RefCell<String>,
    next_generation: RefCell<u64>,
    /// C3: the last 10 commands run through "Exec…" for a container, most
    /// recent first, for the dialog's history combo. Keyed by resource id
    /// (the container, not the node id — a container keeps its history
    /// across a reconnect, which mints a fresh node id map only through
    /// `connections`, never through this one).
    pub(crate) exec_history: RefCell<BTreeMap<String, Vec<String>>>,
    /// C3: `openInspect`/`openFile` need `AppSession::open_virtual_document`
    /// — the same shared session every other adapter reads through
    /// `bridge::registry::shared_session`, so a container's Inspect tab
    /// dedups/focuses against the very same tab list the editor shows.
    pub(crate) session: Rc<RefCell<app_core::AppSession>>,
    /// C6: the compose lenses last handed to each open editor, so a click
    /// by index resolves against what is on screen.
    pub(crate) lenses: super::editor::LensState,
    /// C7: a registry's fetched repositories, keyed by its own id (not the
    /// `Registry` node id — see `registry_repo_next` below) — filled by
    /// `loadRegistryRepositories`, read back by `nodes()` so a registry's
    /// children survive the wholesale tree rebuild every `treeChanged`
    /// triggers.
    pub(crate) registry_repos: RefCell<BTreeMap<String, Vec<String>>>,
    /// C7 review follow-up: the `catalog` page cursor to pass as `last` for
    /// the *next* page of `registry_repos`, keyed the same way — `Some(_)`
    /// means the server named a further page (a "Load more…" row is
    /// rendered), `None` means the last page was already fetched, and an
    /// absent key (before the first fetch) renders no row either, the same
    /// as `registry_repos` being absent.
    pub(crate) registry_repo_next: RefCell<BTreeMap<String, Option<String>>>,
    /// C7: a repository's fetched tags (name, full reference), keyed by
    /// its `RegistryRepo` node id — the [`tree::registry_tag_nodes`]
    /// input `loadRegistryTags` fills in.
    pub(crate) registry_tags: RefCell<BTreeMap<String, Vec<(String, String)>>>,
    /// C7 review follow-up: `registry_repo_next`'s counterpart for
    /// `registry_tags`, keyed the same way (by `RegistryRepo` node id).
    pub(crate) registry_tag_next: RefCell<BTreeMap<String, Option<String>>>,
}

impl Default for ContainerServiceRust {
    fn default() -> Self {
        ContainerServiceRust {
            connections: RefCell::default(),
            filter: RefCell::default(),
            search: RefCell::default(),
            next_generation: RefCell::default(),
            exec_history: RefCell::default(),
            session: crate::bridge::registry::shared_session(),
            lenses: super::editor::LensState::default(),
            registry_repos: RefCell::default(),
            registry_repo_next: RefCell::default(),
            registry_tags: RefCell::default(),
            registry_tag_next: RefCell::default(),
        }
    }
}

pub(crate) fn configured_connections() -> Vec<app_config::ContainerConnectionSetting> {
    crate::bridge::convert::load_resolved_settings()
        .containers
        .connections
}

/// C7: every configured registry, in the order they were added — the
/// tree's registry root rows, `registries.rs`'s browsing/pull/push calls,
/// and `bridge/language/containers.rs`'s registry-repository completion
/// all read through this rather than re-resolving settings themselves.
pub(crate) fn configured_registries() -> Vec<app_config::RegistrySetting> {
    crate::bridge::convert::load_resolved_settings()
        .containers
        .registries
}

pub(crate) fn work_dir() -> std::path::PathBuf {
    crate::bridge::convert::current_project_root()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// `connection_id`'s own `podman machine` name (C9), when it is a
/// [`container_core::connection::ConnectionKind::PodmanMachine`]
/// connection — `None` for every other kind, so "Start machine"/"Stop
/// machine" are refused rather than run against a name that does not
/// apply here.
pub(crate) fn podman_machine_name(connection_id: &str) -> Option<String> {
    let setting = configured_connections()
        .into_iter()
        .find(|setting| setting.id == connection_id)?;
    match ConnectionConfig::from_setting(&setting).kind {
        container_core::connection::ConnectionKind::PodmanMachine { name } => Some(name),
        _ => None,
    }
}

/// The `Invocation` for a configured connection, by id — every C3 action
/// and session goes through this rather than re-deriving it, the same
/// `ConnectionConfig::from_setting` + minikube-env-if-needed sequence
/// `test_container_connection` (C1) already established.
pub(crate) fn connection_invocation(
    connection_id: &str,
) -> Result<container_core::connection::Invocation, FfiResult> {
    let Some(setting) = configured_connections()
        .into_iter()
        .find(|setting| setting.id == connection_id)
    else {
        return Err(errors::failure(
            errors::CODE_INVALID_ARGUMENT,
            format!("no container connection with id '{connection_id}' is configured"),
        ));
    };
    let config = ConnectionConfig::from_setting(&setting);
    let mut invocation = config.invocation();
    if matches!(
        config.kind,
        container_core::connection::ConnectionKind::Minikube
    ) {
        match container_core::connection::minikube_docker_env(&work_dir()) {
            Ok(env) => invocation = invocation.with_extra_env(env),
            Err(error) => {
                return Err(errors::failure(errors::CODE_REFUSED, error.to_string()));
            }
        }
    }
    Ok(invocation)
}

/// A configured connection's [`container_core::connection::Engine`], by
/// id (C4: `cleanUp`/`imageLayers`/history need to know which engine's
/// `history --format` to use). Cheap: no CLI call, just the setting.
pub(crate) fn connection_engine(
    connection_id: &str,
) -> Result<container_core::connection::Engine, FfiResult> {
    configured_connections()
        .into_iter()
        .find(|setting| setting.id == connection_id)
        .map(|setting| ConnectionConfig::from_setting(&setting).engine)
        .ok_or_else(|| {
            errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no container connection with id '{connection_id}' is configured"),
            )
        })
}

fn to_ffi_status(status: tree::NodeStatus) -> ffi::FfiContainerNodeStatus {
    use ffi::FfiContainerNodeStatus as Ffi;
    use tree::NodeStatus;
    match status {
        NodeStatus::None => Ffi::None,
        NodeStatus::Disconnected => Ffi::Disconnected,
        NodeStatus::Connecting => Ffi::Connecting,
        NodeStatus::Connected => Ffi::Connected,
        NodeStatus::Error => Ffi::Error,
        NodeStatus::Running => Ffi::Running,
        NodeStatus::Paused => Ffi::Paused,
        NodeStatus::Restarting => Ffi::Restarting,
        NodeStatus::Exited => Ffi::Exited,
        NodeStatus::Created => Ffi::Created,
        NodeStatus::Dead => Ffi::Dead,
        NodeStatus::Other => Ffi::Other,
    }
}

fn to_ffi_age_unit(age: Option<Age>) -> ffi::FfiAgeUnit {
    match age.map(|age| age.unit) {
        None => ffi::FfiAgeUnit::None,
        Some(AgeUnit::Seconds) => ffi::FfiAgeUnit::Seconds,
        Some(AgeUnit::Minutes) => ffi::FfiAgeUnit::Minutes,
        Some(AgeUnit::Hours) => ffi::FfiAgeUnit::Hours,
        Some(AgeUnit::Days) => ffi::FfiAgeUnit::Days,
        Some(AgeUnit::Months) => ffi::FfiAgeUnit::Months,
        Some(AgeUnit::Years) => ffi::FfiAgeUnit::Years,
    }
}

/// `-1` is the "not applicable" sentinel for a count a view only displays
/// — the same convention `FfiTestNode::durationMs` uses.
fn optional_count(value: Option<usize>) -> i64 {
    value.map(|value| value as i64).unwrap_or(-1)
}

fn to_ffi_node(node: &tree::TreeNode) -> ffi::FfiContainerNode {
    let (running, total) = node
        .running
        .map(|(running, total)| (running as i64, total as i64))
        .unwrap_or((-1, -1));
    ffi::FfiContainerNode {
        id: QString::from(node.id.as_str()),
        parent_id: QString::from(node.parent_id.as_str()),
        kind: QString::from(node.kind.id()),
        name: QString::from(node.name.as_str()),
        connection_id: QString::from(node.connection_id.as_str()),
        resource_id: QString::from(node.resource_id.as_str()),
        icon: QString::from(node.icon),
        status: to_ffi_status(node.status),
        status_text: QString::from(node.status_text.as_str()),
        exit_code: node.exit_code,
        age_unit: to_ffi_age_unit(node.age),
        age_value: node.age.map(|age| age.value as i64).unwrap_or(-1),
        count: optional_count(node.count),
        running,
        total,
        size_bytes: node.size_bytes.map(|bytes| bytes as i64).unwrap_or(-1),
        detail: QString::from(node.detail.as_str()),
        tooltip: QString::from(node.tooltip.as_str()),
    }
}

impl ContainerServiceRust {
    fn current_filter(&self) -> (bool, bool) {
        *self.filter.borrow_mut().get_or_insert_with(|| {
            let settings = crate::bridge::convert::load_resolved_settings().containers;
            (
                settings.show_stopped_containers_or_default(),
                settings.show_untagged_images_or_default(),
            )
        })
    }

    fn bump_generation(&self) -> u64 {
        let mut next = self.next_generation.borrow_mut();
        *next += 1;
        *next
    }
}

impl ffi::ContainerService {
    pub fn nodes(&self) -> Vec<ffi::FfiContainerNode> {
        let configured = configured_connections();
        let connections = self.connections.borrow();
        let (show_stopped, show_untagged) = self.current_filter();
        let search = self.search.borrow();
        let disconnected = ConnectionState::Disconnected;

        let views: Vec<Option<container_core::snapshot::SnapshotView<'_>>> = configured
            .iter()
            .map(|setting| {
                connections
                    .get(&setting.id)
                    .and_then(|connection| connection.snapshot.as_ref())
                    .map(|snapshot| snapshot.filter(show_stopped, show_untagged).search(&search))
            })
            .collect();
        let rows: Vec<ConnectionRow<'_>> = configured
            .iter()
            .zip(views.iter())
            .map(|(setting, view)| ConnectionRow {
                id: &setting.id,
                name: &setting.name,
                engine: ConnectionConfig::from_setting(setting).engine,
                state: connections
                    .get(&setting.id)
                    .map(|connection| &connection.state)
                    .unwrap_or(&disconnected),
                view: view.as_ref(),
            })
            .collect();
        let mut nodes = tree::flatten(&rows, SystemTime::now());
        nodes.extend(self.registry_nodes());
        nodes.iter().map(to_ffi_node).collect()
    }

    /// C7: the registry roots plus whatever repositories/tags have already
    /// been fetched for them — appended after every connection's own rows
    /// so a registry always renders as a top-level entry below the engine
    /// connections, matching the plan's dock mockup.
    fn registry_nodes(&self) -> Vec<tree::TreeNode> {
        let configured = super::service::configured_registries();
        let rows: Vec<tree::RegistryRow<'_>> = configured
            .iter()
            .map(|setting| tree::RegistryRow {
                id: &setting.id,
                name: &setting.name,
                address: &setting.address,
            })
            .collect();
        let mut nodes = tree::registry_nodes(&rows);

        let repos = self.registry_repos.borrow();
        let repo_next = self.registry_repo_next.borrow();
        let tags = self.registry_tags.borrow();
        let tag_next = self.registry_tag_next.borrow();
        for setting in &configured {
            let Some(repositories) = repos.get(&setting.id) else {
                continue;
            };
            let registry_node_id = format!("registry/{}", setting.id);
            nodes.extend(tree::registry_repo_nodes(&setting.id, repositories));
            // C7 review follow-up: a "Load more…" row only when the last
            // fetch actually named a next cursor — `Some(Some(_))`, not
            // merely present, since a key is inserted on every fetch
            // whether or not it was the last page.
            if matches!(repo_next.get(&setting.id), Some(Some(_))) {
                nodes.push(tree::registry_more_node(&registry_node_id, &setting.id));
            }
            for repository in repositories {
                let repo_node_id = format!("{registry_node_id}/repo/{repository}");
                if let Some(repo_tags) = tags.get(&repo_node_id) {
                    nodes.extend(tree::registry_tag_nodes(
                        &setting.id,
                        &repo_node_id,
                        repository,
                        repo_tags,
                    ));
                    if matches!(tag_next.get(&repo_node_id), Some(Some(_))) {
                        nodes.push(tree::registry_more_node(&repo_node_id, &setting.id));
                    }
                }
            }
        }
        nodes
    }

    /// Every configured connection (C4's Copy Image to... picker): id,
    /// name, engine, and whether it is currently connected — cheap, no
    /// snapshot needed.
    pub fn connections(&self) -> Vec<ffi::FfiConnectionSummary> {
        let connections = self.connections.borrow();
        configured_connections()
            .into_iter()
            .map(|setting| {
                let connected = connections
                    .get(&setting.id)
                    .map(|connection| !matches!(connection.state, ConnectionState::Disconnected))
                    .unwrap_or(false);
                ffi::FfiConnectionSummary {
                    id: QString::from(setting.id.as_str()),
                    name: QString::from(setting.name.as_str()),
                    engine: QString::from(ConnectionConfig::from_setting(&setting).engine.id()),
                    connected,
                }
            })
            .collect()
    }

    pub fn connection_state(&self, connection_id: &QString) -> ffi::FfiConnectionState {
        let connections = self.connections.borrow();
        let state = connections
            .get(&connection_id.to_string())
            .map(|connection| connection.state.clone())
            .unwrap_or(ConnectionState::Disconnected);
        let message = match &state {
            ConnectionState::Error(message) => message.clone(),
            ConnectionState::Connected(info) => info.server_version.clone(),
            _ => String::new(),
        };
        ffi::FfiConnectionState {
            state: QString::from(state.id()),
            message: QString::from(message.as_str()),
        }
    }

    pub fn filter(&self) -> ffi::FfiContainerFilter {
        let (show_stopped, show_untagged) = self.current_filter();
        ffi::FfiContainerFilter {
            show_stopped,
            show_untagged,
        }
    }

    pub fn connect_engine(mut self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        let id = connection_id.to_string();
        let Some(setting) = configured_connections()
            .into_iter()
            .find(|setting| setting.id == id)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no container connection with id '{id}' is configured"),
            );
        };
        if let Some(connection) = self.connections.borrow().get(&id) {
            if connection.handle.is_some() {
                return errors::failure(
                    errors::CODE_REFUSED,
                    format!("'{}' is already connected", setting.name),
                );
            }
        }

        let generation = self.bump_generation();
        let (events_tx, events_rx) = mpsc::channel::<ContainerEvent>();
        let handle = watcher::spawn(
            id.clone(),
            ConnectionConfig::from_setting(&setting),
            work_dir(),
            events_tx,
        );
        self.connections.borrow_mut().insert(
            id.clone(),
            Connection {
                state: ConnectionState::Connecting,
                snapshot: None,
                handle: Some(handle),
                generation,
            },
        );

        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            for event in events_rx {
                let queued = qt_thread.queue(move |service: Pin<&mut ffi::ContainerService>| {
                    apply(service, generation, event);
                });
                if queued.is_err() {
                    return;
                }
            }
        });

        self.as_mut()
            .connection_state_changed(connection_id.clone());
        self.as_mut().tree_changed();
        FfiResult::default()
    }

    pub fn disconnect_engine(mut self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        let id = connection_id.to_string();
        let generation = self.bump_generation();
        let handle = {
            let mut connections = self.connections.borrow_mut();
            let Some(connection) = connections.get_mut(&id) else {
                return errors::failure(errors::CODE_REFUSED, format!("'{id}' is not connected"));
            };
            connection.state = ConnectionState::Disconnected;
            connection.snapshot = None;
            connection.generation = generation;
            connection.handle.take()
        };
        if let Some(handle) = handle {
            handle.stop();
        }
        self.as_mut()
            .connection_state_changed(connection_id.clone());
        self.as_mut().tree_changed();
        FfiResult::default()
    }

    pub fn refresh(self: Pin<&mut Self>, connection_id: &QString) -> FfiResult {
        let connections = self.connections.borrow();
        match connections
            .get(&connection_id.to_string())
            .and_then(|connection| connection.handle.as_ref())
        {
            Some(handle) => {
                handle.refresh();
                FfiResult::default()
            }
            None => errors::failure(
                errors::CODE_REFUSED,
                format!("'{connection_id}' is not connected"),
            ),
        }
    }

    pub fn connect_all(mut self: Pin<&mut Self>) {
        for setting in configured_connections() {
            let _ = self
                .as_mut()
                .connect_engine(&QString::from(setting.id.as_str()));
        }
    }

    /// Re-snapshot every connected connection and re-read the configured
    /// list, so a connection added in Settings shows up without a restart.
    pub fn refresh_all(mut self: Pin<&mut Self>) {
        for connection in self.connections.borrow().values() {
            if let Some(handle) = &connection.handle {
                handle.refresh();
            }
        }
        self.as_mut().tree_changed();
    }

    pub fn set_filter(
        mut self: Pin<&mut Self>,
        show_stopped: bool,
        show_untagged: bool,
    ) -> FfiResult {
        *self.filter.borrow_mut() = Some((show_stopped, show_untagged));
        self.as_mut().tree_changed();
        persist_filter(show_stopped, show_untagged)
    }

    pub fn set_search(mut self: Pin<&mut Self>, text: &QString) {
        *self.search.borrow_mut() = text.to_string();
        self.as_mut().tree_changed();
    }
}

/// Write the two filters into whichever layer is in force: the project's
/// own `[containers]` section when it has one (ADR-0022), else the global
/// file.
fn persist_filter(show_stopped: bool, show_untagged: bool) -> FfiResult {
    let project = crate::bridge::convert::load_project_settings();
    if let Some(mut section) = project.containers {
        section.show_stopped_containers = Some(show_stopped);
        section.show_untagged_images = Some(show_untagged);
        return commit_to_project(|project| project.containers = Some(section));
    }
    let config_dir = app_core::resolve_config_dir();
    match app_config::update(&config_dir, |settings| {
        settings.containers.show_stopped_containers = Some(show_stopped);
        settings.containers.show_untagged_images = Some(show_untagged);
    }) {
        Ok(()) => FfiResult::default(),
        Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
    }
}

/// A watcher event, on the Qt thread. Ignored when the connection was
/// disconnected or reconnected since the watcher that sent it started.
fn apply(mut service: Pin<&mut ffi::ContainerService>, generation: u64, event: ContainerEvent) {
    let connection_id = QString::from(event.connection_id.as_str());
    let state_changed = {
        let mut connections = service.connections.borrow_mut();
        let Some(connection) = connections.get_mut(&event.connection_id) else {
            return;
        };
        if connection.generation != generation {
            return;
        }
        match event.kind {
            ContainerEventKind::Connected(info) => {
                connection.state = ConnectionState::Connected(info);
                true
            }
            ContainerEventKind::Snapshot(snapshot) => {
                connection.snapshot = Some(snapshot);
                false
            }
            ContainerEventKind::Error(message) => {
                connection.state = ConnectionState::Error(message);
                true
            }
            ContainerEventKind::Disconnected => {
                connection.state = ConnectionState::Disconnected;
                connection.snapshot = None;
                connection.handle = None;
                true
            }
        }
    };
    if state_changed {
        service.as_mut().connection_state_changed(connection_id);
    }
    service.as_mut().tree_changed();
}
