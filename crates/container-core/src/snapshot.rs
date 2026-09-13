//! One engine's whole state at one moment: every container, image,
//! volume, network and (Podman) pod, plus the compose projects derived
//! from container labels.
//!
//! [`EngineSnapshot::fetch`] is the only I/O here — `ls -q` for the ids,
//! then one `inspect` per resource kind. Everything else is pure:
//! [`EngineSnapshot::from_parts`] groups, [`EngineSnapshot::filter`] and
//! [`SnapshotView::search`] narrow, and the tests run against the
//! `testdata/inspect` fixtures rather than a live daemon.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::connection::{Engine, Invocation};
use crate::model::{self, Container, Image, Network, Pod, Volume};
use crate::probe::ConnectionError;

/// A compose service: one name, the containers currently carrying its
/// label (usually one; more after `compose up --scale`).
#[derive(Debug, Clone, PartialEq)]
pub struct ComposeService {
    pub name: String,
    /// Indices into [`EngineSnapshot::containers`], in that vector's order.
    pub containers: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposeProject {
    pub name: String,
    pub config_files: Vec<String>,
    pub working_dir: String,
    /// Sorted by service name.
    pub services: Vec<ComposeService>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EngineSnapshot {
    pub containers: Vec<Container>,
    pub images: Vec<Image>,
    pub volumes: Vec<Volume>,
    pub networks: Vec<Network>,
    pub pods: Vec<Pod>,
    /// Sorted by project name.
    pub compose: Vec<ComposeProject>,
}

impl EngineSnapshot {
    /// Assemble a snapshot and derive its compose projects. Every list is
    /// sorted by display name so the tree order is stable across
    /// re-snapshots regardless of the order the engine listed them in.
    pub fn from_parts(
        mut containers: Vec<Container>,
        mut images: Vec<Image>,
        mut volumes: Vec<Volume>,
        mut networks: Vec<Network>,
        mut pods: Vec<Pod>,
    ) -> Self {
        containers.sort_by(|a, b| a.name.cmp(&b.name));
        images.sort_by_key(|image| image.display_name());
        volumes.sort_by(|a, b| a.name.cmp(&b.name));
        networks.sort_by(|a, b| a.name.cmp(&b.name));
        pods.sort_by(|a, b| a.name.cmp(&b.name));
        let compose = group_compose(&containers);
        EngineSnapshot {
            containers,
            images,
            volumes,
            networks,
            pods,
            compose,
        }
    }

    /// Query the engine behind `invocation` for everything. Pods are only
    /// asked for on Podman — `docker pod` does not exist.
    pub fn fetch(
        invocation: &Invocation,
        engine: Engine,
        work_dir: &Path,
    ) -> Result<Self, ConnectionError> {
        let cli = Cli {
            invocation,
            engine,
            work_dir,
        };
        let containers = cli.inspect_all("ps", &["-aq"], "container", Container::from_value)?;
        let images = cli.inspect_all(
            "image",
            &["ls", "-q", "--no-trunc"],
            "image",
            Image::from_value,
        )?;
        let volumes = cli.inspect_all("volume", &["ls", "-q"], "volume", Volume::from_value)?;
        let networks = cli.inspect_all("network", &["ls", "-q"], "network", Network::from_value)?;
        let pods = match engine {
            Engine::Podman => {
                let json = cli.run(&["pod", "ls", "--format", "json"])?;
                cli.parse(&json, Pod::from_value)?
            }
            Engine::Docker => Vec::new(),
        };
        Ok(Self::from_parts(
            containers, images, volumes, networks, pods,
        ))
    }

    /// Apply the two dock filters. Containers that are not live are
    /// dropped when `show_stopped` is off; untagged images when
    /// `show_untagged` is off. Compose services keep only the containers
    /// that survive, and a service or project left with none disappears.
    pub fn filter(&self, show_stopped: bool, show_untagged: bool) -> SnapshotView<'_> {
        let containers: Vec<usize> = (0..self.containers.len())
            .filter(|&index| show_stopped || self.containers[index].status().is_live())
            .collect();
        let images: Vec<usize> = (0..self.images.len())
            .filter(|&index| show_untagged || !self.images[index].is_untagged())
            .collect();
        let compose = self
            .compose
            .iter()
            .enumerate()
            .map(|(project_index, project)| ComposeView {
                project: project_index,
                services: project
                    .services
                    .iter()
                    .enumerate()
                    .map(|(service_index, service)| ServiceView {
                        service: service_index,
                        containers: service
                            .containers
                            .iter()
                            .copied()
                            .filter(|index| containers.contains(index))
                            .collect(),
                    })
                    .filter(|service| !service.containers.is_empty())
                    .collect(),
            })
            .filter(|project| !project.services.is_empty())
            .collect();
        SnapshotView {
            snapshot: self,
            containers,
            images,
            volumes: (0..self.volumes.len()).collect(),
            networks: (0..self.networks.len()).collect(),
            pods: (0..self.pods.len()).collect(),
            compose,
        }
    }
}

/// Which containers carry a compose project label, grouped by project then
/// service. Either implementation's label prefix counts
/// ([`Container::compose_label`]).
fn group_compose(containers: &[Container]) -> Vec<ComposeProject> {
    let mut projects: BTreeMap<&str, ComposeProject> = BTreeMap::new();
    for (index, container) in containers.iter().enumerate() {
        let Some(project_name) = container.compose_label("project") else {
            continue;
        };
        let service_name = container
            .compose_label("service")
            .unwrap_or(container.name.as_str());
        let project = projects
            .entry(project_name)
            .or_insert_with(|| ComposeProject {
                name: project_name.to_string(),
                config_files: Vec::new(),
                working_dir: String::new(),
                services: Vec::new(),
            });
        if project.config_files.is_empty() {
            if let Some(files) = container.compose_label("project.config_files") {
                project.config_files = files.split(',').map(str::to_string).collect();
            }
        }
        if project.working_dir.is_empty() {
            if let Some(dir) = container.compose_label("project.working_dir") {
                project.working_dir = dir.to_string();
            }
        }
        match project
            .services
            .iter_mut()
            .find(|service| service.name == service_name)
        {
            Some(service) => service.containers.push(index),
            None => project.services.push(ComposeService {
                name: service_name.to_string(),
                containers: vec![index],
            }),
        }
    }
    let mut projects: Vec<ComposeProject> = projects.into_values().collect();
    for project in &mut projects {
        project.services.sort_by(|a, b| a.name.cmp(&b.name));
    }
    projects
}

/// A compose project as seen through a view: indices into the snapshot's
/// `compose` list and, per service, the container indices still visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeView {
    pub project: usize,
    pub services: Vec<ServiceView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceView {
    pub service: usize,
    pub containers: Vec<usize>,
}

/// A filtered, searched selection of one snapshot — indices into its
/// vectors, so nothing is copied and every list stays in the snapshot's
/// (sorted) order.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotView<'a> {
    pub snapshot: &'a EngineSnapshot,
    pub containers: Vec<usize>,
    pub images: Vec<usize>,
    pub volumes: Vec<usize>,
    pub networks: Vec<usize>,
    pub pods: Vec<usize>,
    pub compose: Vec<ComposeView>,
}

impl<'a> SnapshotView<'a> {
    /// Case-insensitive substring match on names, ids and tags. An empty
    /// (or whitespace) query changes nothing. A compose project whose own
    /// name matches keeps every service; otherwise only the services whose
    /// name or containers match.
    pub fn search(mut self, query: &str) -> Self {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self;
        }
        let snapshot = self.snapshot;
        let matches = |haystacks: &[&str]| {
            haystacks
                .iter()
                .any(|text| text.to_lowercase().contains(&needle))
        };
        let container_matches = |index: usize| {
            let container = &snapshot.containers[index];
            matches(&[&container.name, &container.id, &container.image])
        };
        self.containers.retain(|&index| container_matches(index));
        self.images.retain(|&index| {
            let image = &snapshot.images[index];
            let mut haystacks: Vec<&str> = image.repo_tags.iter().map(String::as_str).collect();
            haystacks.push(&image.id);
            matches(&haystacks)
        });
        self.volumes
            .retain(|&index| matches(&[&snapshot.volumes[index].name]));
        self.networks.retain(|&index| {
            let network = &snapshot.networks[index];
            matches(&[&network.name, &network.id])
        });
        self.pods.retain(|&index| {
            let pod = &snapshot.pods[index];
            matches(&[&pod.name, &pod.id])
        });
        self.compose = self
            .compose
            .into_iter()
            .filter_map(|mut view| {
                let project = &snapshot.compose[view.project];
                if matches(&[&project.name]) {
                    return Some(view);
                }
                view.services.retain_mut(|service_view| {
                    let service = &project.services[service_view.service];
                    if matches(&[&service.name]) {
                        return true;
                    }
                    service_view
                        .containers
                        .retain(|&index| container_matches(index));
                    !service_view.containers.is_empty()
                });
                (!view.services.is_empty()).then_some(view)
            })
            .collect();
        self
    }
}

/// The CLI calls behind [`EngineSnapshot::fetch`], bundled so each step
/// reads as `ls` → `inspect` → parse.
struct Cli<'a> {
    invocation: &'a Invocation,
    engine: Engine,
    work_dir: &'a Path,
}

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

impl Cli<'_> {
    fn run(&self, args: &[&str]) -> Result<String, ConnectionError> {
        let output = self
            .invocation
            .run(args, self.work_dir, QUERY_TIMEOUT)
            .map_err(|failure| {
                ConnectionError::from_process_failure(&self.invocation.program, failure)
            })?;
        if !output.status.success() {
            return Err(ConnectionError::from_stderr(
                self.engine,
                &String::from_utf8_lossy(&output.stderr),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn parse<T>(
        &self,
        json: &str,
        from_value: fn(serde_json::Value) -> Result<T, serde_json::Error>,
    ) -> Result<Vec<T>, ConnectionError> {
        model::parse_list(json, from_value).map_err(|error| ConnectionError::UnparsableOutput {
            detail: error.to_string(),
        })
    }

    /// `<list_cmd> <list_args>` for the ids, then `<kind> inspect <ids>`
    /// — skipped entirely when there is nothing to inspect, since
    /// `inspect` with no ids is a usage error on both engines.
    fn inspect_all<T>(
        &self,
        list_cmd: &str,
        list_args: &[&str],
        kind: &str,
        from_value: fn(serde_json::Value) -> Result<T, serde_json::Error>,
    ) -> Result<Vec<T>, ConnectionError> {
        let mut list_argv = vec![list_cmd];
        list_argv.extend_from_slice(list_args);
        let listing = self.run(&list_argv)?;
        let ids = dedupe_ids(&listing);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut inspect_argv = vec![kind, "inspect"];
        inspect_argv.extend(ids.iter().map(String::as_str));
        let json = self.run(&inspect_argv)?;
        self.parse(&json, from_value)
    }
}

/// One id per line, blanks dropped, duplicates removed in first-seen order
/// — `image ls -q --no-trunc` repeats an id once per tag.
pub fn dedupe_ids(listing: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for line in listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !seen.iter().any(|existing: &String| existing == line) {
            seen.push(line.to_string());
        }
    }
    seen
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    fn read(engine: &str, file: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/inspect")
            .join(engine)
            .join(file);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    pub(crate) fn snapshot(engine: &str) -> EngineSnapshot {
        let pods = if engine == "podman" {
            model::parse_list(&read(engine, "pods.json"), Pod::from_value).unwrap()
        } else {
            Vec::new()
        };
        EngineSnapshot::from_parts(
            model::parse_list(&read(engine, "containers.json"), Container::from_value).unwrap(),
            model::parse_list(&read(engine, "images.json"), Image::from_value).unwrap(),
            model::parse_list(&read(engine, "volumes.json"), Volume::from_value).unwrap(),
            model::parse_list(&read(engine, "networks.json"), Network::from_value).unwrap(),
            pods,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::snapshot;
    use super::*;
    use crate::model::ContainerStatus;

    fn names<'a>(snapshot: &'a EngineSnapshot, indices: &[usize]) -> Vec<&'a str> {
        indices
            .iter()
            .map(|&index| snapshot.containers[index].name.as_str())
            .collect()
    }

    #[test]
    fn docker_fixtures_parse_into_a_sorted_snapshot() {
        let snapshot = snapshot("docker");
        assert_eq!(snapshot.containers.len(), 6);
        assert_eq!(
            snapshot.containers[0].name, "jagdonline-2d80272f-admin-1",
            "sorted by name"
        );
        let redis = snapshot
            .containers
            .iter()
            .find(|c| c.name == "redis")
            .expect("redis container");
        assert_eq!(redis.status(), ContainerStatus::Running);
        assert_eq!(redis.image, "redis:7.2-alpine");
        assert_eq!(redis.ports.len(), 2);
        assert_eq!(redis.ports[0].display(), "0.0.0.0:6379 -> 6379/tcp");
        assert_eq!(redis.mounts[0].destination, "/data");
        assert!(redis.env.iter().any(|e| e.starts_with("PATH=")));
        let web = snapshot
            .containers
            .iter()
            .find(|c| c.name == "jagdonline-2d80272f-web-1")
            .unwrap();
        assert_eq!(web.status(), ContainerStatus::Exited(255));
        let api = snapshot
            .containers
            .iter()
            .find(|c| c.name == "jagdonline-2d80272f-api-1")
            .unwrap();
        assert!(api.env.iter().any(|e| e.ends_with("=<redacted>")));

        assert_eq!(snapshot.images.len(), 5);
        assert!(snapshot.images.iter().all(|image| !image.is_untagged()));
        let postgres = snapshot
            .images
            .iter()
            .find(|i| i.repo_tags == ["postgres:16-alpine"])
            .unwrap();
        assert_eq!(postgres.size, 274_852_063);
        assert_eq!(postgres.repo_digests.len(), 1);

        assert_eq!(snapshot.volumes.len(), 3);
        assert_eq!(snapshot.volumes[0].driver, "local");
        assert_eq!(snapshot.networks.len(), 4);
        let bridge = snapshot
            .networks
            .iter()
            .find(|n| n.name == "bridge")
            .unwrap();
        assert_eq!(bridge.driver, "bridge");
        assert!(bridge.containers.is_empty());
        assert!(snapshot.pods.is_empty());
    }

    #[test]
    fn podman_fixtures_parse_including_pods_and_lowercase_networks() {
        let snapshot = snapshot("podman");
        assert_eq!(snapshot.containers.len(), 4);
        let web = snapshot
            .containers
            .iter()
            .find(|c| c.name == "web")
            .unwrap();
        assert_eq!(web.image, "docker.io/library/nginx:1.27");
        assert!(!web.pod.is_empty());
        assert_eq!(web.ports[0].display(), "0.0.0.0:8080 -> 80/tcp");
        let statuses: Vec<ContainerStatus> =
            snapshot.containers.iter().map(Container::status).collect();
        assert!(statuses.contains(&ContainerStatus::Paused));
        assert!(statuses.contains(&ContainerStatus::Created));
        assert!(statuses.contains(&ContainerStatus::Exited(3)));

        assert_eq!(snapshot.images.len(), 4);
        assert_eq!(
            snapshot.images.iter().filter(|i| i.is_untagged()).count(),
            1
        );
        assert_eq!(snapshot.networks.len(), 2);
        assert_eq!(snapshot.networks[0].name, "podman");
        assert_eq!(
            snapshot.networks[0]
                .containers
                .values()
                .next()
                .map(String::as_str),
            Some("web")
        );
        assert_eq!(snapshot.pods.len(), 1);
        assert_eq!(snapshot.pods[0].name, "shop");
        assert_eq!(snapshot.pods[0].status, "Running");
        assert_eq!(snapshot.pods[0].containers.len(), 2);
    }

    #[test]
    fn docker_compose_labels_group_into_projects_and_services() {
        let snapshot = snapshot("docker");
        let projects: Vec<&str> = snapshot.compose.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            projects,
            vec!["event-sourcing-chess-example", "jagdonline-2d80272f"]
        );
        let jagd = &snapshot.compose[1];
        let services: Vec<&str> = jagd.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(services, vec!["admin", "api", "db", "mailpit", "web"]);
        assert_eq!(
            jagd.working_dir,
            "/home/user/projects/jagdonline/.claude/worktrees/trophy-scoring"
        );
        assert_eq!(jagd.config_files.len(), 1);
        assert_eq!(
            snapshot.compose[0].config_files.len(),
            2,
            "comma-separated list"
        );
        let db = jagd.services.iter().find(|s| s.name == "db").unwrap();
        assert_eq!(
            names(&snapshot, &db.containers),
            vec!["jagdonline-2d80272f-db-1"]
        );
    }

    #[test]
    fn podman_compose_labels_group_too() {
        let snapshot = snapshot("podman");
        assert_eq!(snapshot.compose.len(), 1);
        let shop = &snapshot.compose[0];
        assert_eq!(shop.name, "shop");
        assert_eq!(shop.working_dir, "/home/user/projects/shop");
        let services: Vec<&str> = shop.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(services, vec!["app", "db"]);
    }

    #[test]
    fn a_service_label_missing_falls_back_to_the_container_name() {
        let raw = serde_json::json!({
            "Id": "x", "Name": "/lonely",
            "Config": {"Labels": {"com.docker.compose.project": "p"}}
        });
        let snapshot = EngineSnapshot::from_parts(
            vec![Container::from_value(raw).unwrap()],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        assert_eq!(snapshot.compose[0].services[0].name, "lonely");
    }

    #[test]
    fn filter_hides_stopped_containers_and_prunes_empty_compose_projects() {
        let snapshot = snapshot("docker");
        let everything = snapshot.filter(true, true);
        assert_eq!(everything.containers.len(), 6);
        assert_eq!(everything.compose.len(), 2);

        let live = snapshot.filter(false, true);
        assert_eq!(names(&snapshot, &live.containers), vec!["redis"]);
        assert_eq!(live.compose.len(), 1, "jagdonline has no live container");
        assert_eq!(
            snapshot.compose[live.compose[0].project].name,
            "event-sourcing-chess-example"
        );
        assert_eq!(live.volumes.len(), 3, "filters never touch volumes");
        assert_eq!(live.networks.len(), 4);
    }

    #[test]
    fn filter_hides_untagged_images() {
        let snapshot = snapshot("podman");
        assert_eq!(snapshot.filter(true, true).images.len(), 4);
        let tagged = snapshot.filter(true, false);
        assert_eq!(tagged.images.len(), 3);
        assert!(tagged
            .images
            .iter()
            .all(|&index| !snapshot.images[index].is_untagged()));
    }

    #[test]
    fn filter_keeps_paused_containers_as_live() {
        let snapshot = snapshot("podman");
        let live = snapshot.filter(false, true);
        let mut visible = names(&snapshot, &live.containers);
        visible.sort_unstable();
        assert_eq!(visible, vec!["shop_db_1", "web"]);
    }

    #[test]
    fn search_is_case_insensitive_and_spans_names_ids_and_tags() {
        let snapshot = snapshot("docker");
        let view = snapshot.filter(true, true).search("REDIS");
        assert_eq!(names(&snapshot, &view.containers), vec!["redis"]);
        assert_eq!(view.images.len(), 1, "redis:7.2-alpine tag");
        assert!(view.volumes.is_empty());
        assert!(view.networks.is_empty());
        assert_eq!(view.compose.len(), 1);
        assert_eq!(view.compose[0].services.len(), 1);

        let by_id = snapshot.filter(true, true).search("a8bb0a8333c1");
        assert_eq!(by_id.networks.len(), 1);
        assert!(by_id.containers.is_empty());

        let by_image = snapshot.filter(true, true).search("postgres");
        assert_eq!(
            names(&snapshot, &by_image.containers),
            vec!["jagdonline-2d80272f-db-1"]
        );
    }

    #[test]
    fn search_keeps_a_whole_project_when_its_name_matches() {
        let snapshot = snapshot("docker");
        let view = snapshot.filter(true, true).search("jagdonline-2d80272f");
        assert_eq!(view.compose.len(), 1);
        assert_eq!(view.compose[0].services.len(), 5);
        assert_eq!(view.containers.len(), 5);
    }

    #[test]
    fn search_narrows_to_matching_services_otherwise() {
        let snapshot = snapshot("docker");
        let view = snapshot.filter(true, true).search("mailpit");
        assert_eq!(view.compose.len(), 1);
        assert_eq!(view.compose[0].services.len(), 1);
        assert_eq!(
            snapshot.compose[view.compose[0].project].services[view.compose[0].services[0].service]
                .name,
            "mailpit"
        );
    }

    #[test]
    fn blank_search_changes_nothing() {
        let snapshot = snapshot("docker");
        let base = snapshot.filter(true, true);
        assert_eq!(base.clone().search("   "), base);
    }

    #[test]
    fn dedupe_ids_keeps_first_seen_order() {
        assert_eq!(
            dedupe_ids("sha256:a\nsha256:b\n\nsha256:a\n  sha256:c  \n"),
            vec!["sha256:a", "sha256:b", "sha256:c"]
        );
        assert!(dedupe_ids("\n\n").is_empty());
    }
}
