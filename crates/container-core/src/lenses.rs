//! Compose code lenses (C6): what the editor shows above each service of
//! an open compose file, derived from the connected engines' snapshots —
//! "running (1/1)", "exited (1)", "stopped", and one "Open
//! localhost:<port>" per published port of a running service.
//!
//! Pure derivation: the file's services come from
//! [`crate::compose_file::services_with_lines`], the state from
//! [`EngineSnapshot`]s the caller already holds. The view formats the
//! words (they are translated there); this module decides the facts.

use std::path::Path;

use crate::compose_file::{services_with_lines, ComposeServiceSpan};
use crate::model::ContainerStatus;
use crate::snapshot::{ComposeProject, EngineSnapshot};
use crate::tree::NodeKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LensKind {
    /// The service's containers: `running` of `total` are up. `exit_code`
    /// is the first non-zero exit among the rest, when there is one.
    Status {
        running: usize,
        total: usize,
        exit_code: Option<i64>,
    },
    /// A published port of a running container of the service.
    OpenUrl { host_port: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeLens {
    /// 0-based line of the service key.
    pub line: u32,
    pub kind: LensKind,
    /// For a `Status` lens with at least one container: the tree node id
    /// (`<connection>/container/<id>`) of its first container, so a click
    /// can select it in the Containers dock. Empty otherwise.
    pub container_node_id: String,
}

/// The lenses for compose file `file` with contents `text`, across
/// `snapshots` (`(connection id, snapshot)` per connected engine). The
/// first engine that has a project for this file wins; a file no engine
/// knows shows every service as stopped.
pub fn compose_lenses(
    file: &Path,
    text: &str,
    snapshots: &[(&str, &EngineSnapshot)],
) -> Vec<ComposeLens> {
    let Some((_, services)) = services_with_lines(text) else {
        return Vec::new();
    };
    let owner = snapshots.iter().find_map(|(connection_id, snapshot)| {
        project_for_file(snapshot, file).map(|project| (*connection_id, *snapshot, project))
    });
    services
        .iter()
        .flat_map(|service| lenses_for_service(service, owner))
        .collect()
}

/// The compose project `file` belongs to, by the engine's own record of
/// which files started it (`config_files`), then by its working
/// directory, then by compose's default project name (the directory
/// name, lowercased).
fn project_for_file<'a>(snapshot: &'a EngineSnapshot, file: &Path) -> Option<&'a ComposeProject> {
    let file_text = file.to_string_lossy();
    let parent = file.parent().map(|p| p.to_string_lossy().into_owned());
    let default_name = file
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().to_lowercase());
    snapshot
        .compose
        .iter()
        .find(|project| project.config_files.iter().any(|f| *f == file_text))
        .or_else(|| {
            snapshot.compose.iter().find(|project| {
                !project.working_dir.is_empty() && Some(&project.working_dir) == parent.as_ref()
            })
        })
        .or_else(|| {
            snapshot
                .compose
                .iter()
                .find(|project| Some(&project.name) == default_name.as_ref())
        })
}

fn lenses_for_service(
    service: &ComposeServiceSpan,
    owner: Option<(&str, &EngineSnapshot, &ComposeProject)>,
) -> Vec<ComposeLens> {
    let containers: Vec<&crate::model::Container> = owner
        .and_then(|(_, snapshot, project)| {
            project
                .services
                .iter()
                .find(|s| s.name == service.name)
                .map(|s| {
                    s.containers
                        .iter()
                        .map(|&i| &snapshot.containers[i])
                        .collect()
                })
        })
        .unwrap_or_default();
    let running: Vec<&&crate::model::Container> = containers
        .iter()
        .filter(|c| c.status() == ContainerStatus::Running)
        .collect();
    let exit_code = containers.iter().find_map(|c| match c.status() {
        ContainerStatus::Exited(code) if code != 0 => Some(code),
        _ => None,
    });
    let container_node_id = match (owner, containers.first()) {
        (Some((connection_id, _, _)), Some(first)) => {
            format!("{connection_id}/{}/{}", NodeKind::Container.id(), first.id)
        }
        _ => String::new(),
    };
    let mut lenses = vec![ComposeLens {
        line: service.line,
        kind: LensKind::Status {
            running: running.len(),
            total: containers.len(),
            exit_code,
        },
        container_node_id: container_node_id.clone(),
    }];
    let mut host_ports: Vec<String> = running
        .iter()
        .flat_map(|c| c.ports.iter().map(|p| p.host_port.clone()))
        .filter(|port| !port.is_empty())
        .collect();
    host_ports.sort();
    host_ports.dedup();
    lenses.extend(host_ports.into_iter().map(|host_port| ComposeLens {
        line: service.line,
        kind: LensKind::OpenUrl { host_port },
        container_node_id: String::new(),
    }));
    lenses
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::fixtures::snapshot;

    const CHESS: &str = "services:\n  redis:\n    image: redis:7\n    ports:\n      - \"6379:6379\"\n  app:\n    build: .\n";

    #[test]
    fn a_running_service_gets_a_status_lens_and_one_open_lens_per_published_port() {
        let docker = snapshot("docker");
        let file = Path::new("/home/user/projects/event-sourcing-chess-example/compose.yaml");
        let lenses = compose_lenses(file, CHESS, &[("local", &docker)]);
        assert_eq!(lenses.len(), 3);
        assert_eq!(
            lenses[0].kind,
            LensKind::Status {
                running: 1,
                total: 1,
                exit_code: None
            }
        );
        assert_eq!(lenses[0].line, 1);
        assert!(lenses[0].container_node_id.starts_with("local/container/"));
        assert_eq!(
            lenses[1].kind,
            LensKind::OpenUrl {
                host_port: "6379".into()
            },
            "0.0.0.0 and :: bindings of one port collapse to one lens"
        );
        assert_eq!(
            lenses[2],
            ComposeLens {
                line: 5,
                kind: LensKind::Status {
                    running: 0,
                    total: 0,
                    exit_code: None
                },
                container_node_id: String::new()
            },
            "a service with no container yet"
        );
    }

    #[test]
    fn exited_services_report_the_exit_code_and_no_open_lens() {
        let docker = snapshot("docker");
        let file = Path::new(
            "/home/user/projects/jagdonline/.claude/worktrees/trophy-scoring/docker-compose.yml",
        );
        let text = "services:\n  admin:\n    image: x\n    ports: [\"8081:80\"]\n";
        let lenses = compose_lenses(file, text, &[("local", &docker)]);
        assert_eq!(lenses.len(), 1);
        assert_eq!(
            lenses[0].kind,
            LensKind::Status {
                running: 0,
                total: 1,
                exit_code: Some(1)
            }
        );
    }

    #[test]
    fn the_project_is_found_by_working_dir_or_default_name_when_config_files_differ() {
        let docker = snapshot("docker");
        let by_dir =
            Path::new("/home/user/projects/event-sourcing-chess-example/compose.prod.yaml");
        assert_eq!(
            compose_lenses(by_dir, CHESS, &[("local", &docker)]).len(),
            3
        );
        let by_name = Path::new("/elsewhere/event-sourcing-chess-example/compose.yaml");
        assert_eq!(
            compose_lenses(by_name, CHESS, &[("local", &docker)]).len(),
            3
        );
    }

    #[test]
    fn an_unknown_file_or_no_snapshot_shows_every_service_stopped() {
        let docker = snapshot("docker");
        let lenses = compose_lenses(
            Path::new("/nowhere/compose.yaml"),
            CHESS,
            &[("local", &docker)],
        );
        assert_eq!(lenses.len(), 2);
        assert!(lenses.iter().all(|lens| {
            lens.kind
                == LensKind::Status {
                    running: 0,
                    total: 0,
                    exit_code: None,
                }
        }));
        assert_eq!(
            compose_lenses(Path::new("/x/compose.yaml"), CHESS, &[]).len(),
            2
        );
        assert!(compose_lenses(Path::new("/x/compose.yaml"), "volumes: {}\n", &[]).is_empty());
    }
}
