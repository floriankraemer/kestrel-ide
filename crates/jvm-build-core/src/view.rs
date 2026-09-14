//! Shaping a synced [`BuildModel`] into the tree the Build Tools dock shows
//! (B2): Qt-free and unit-tested here, so `cpp/build_tools_panel.cpp` only
//! paints whatever [`rows`] returns and encodes no shaping decision itself
//! (`docs/architecture/layering.md`'s "cpp/ is a humble view" rule).

use crate::model::{BuildModel, Conflict, Dependency};

/// What kind of row a [`Node`] is — the dock's icon and indent, decided by
/// `cpp/`, never this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    ToolRoot,
    Group,
    Task,
    Module,
    SourceRoot,
    Dependency,
}

/// One row of the dock's tree, flattened and parent-qualified — the same
/// shape `FfiTestNode`/`FfiTreeNode` already use so a `QTreeWidget` builds
/// its own hierarchy from `parent_id` rather than nesting a `Vec` inside a
/// `Vec`, which cxx does not support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub parent_id: String,
    pub kind: NodeKind,
    pub label: String,
    pub detail: String,
    /// The task/goal path a double-click on this row runs
    /// (`jvm_build_core::run::task_config`'s own `Task::path`), empty for a
    /// row that runs nothing.
    pub task_path: String,
    /// The build file "Open build file" opens — a module's own
    /// `build_file`, empty for a row that names none.
    pub build_file: String,
}

fn node(id: String, parent_id: &str, kind: NodeKind, label: impl Into<String>) -> Node {
    Node {
        id,
        parent_id: parent_id.to_string(),
        kind,
        label: label.into(),
        detail: String::new(),
        task_path: String::new(),
        build_file: String::new(),
    }
}

/// A dependency's conflict, as one line for the row's `detail` column — the
/// same "carried through, never re-derived" rule [`crate::model::Conflict`]
/// itself follows.
fn conflict_detail(dependency: &Dependency) -> String {
    match &dependency.conflict {
        Some(Conflict::OmittedForConflict { winner }) => {
            format!("omitted for conflict, resolved to {winner}")
        }
        Some(Conflict::OmittedForDuplicate) => "omitted for duplicate".to_string(),
        Some(Conflict::VersionManagedFrom { from }) => format!("version managed from {from}"),
        None => String::new(),
    }
}

/// The dock's whole tree for one synced model: one tool root, with
/// `Tasks` (grouped by the tool's own `group`/lifecycle string), `Modules`
/// (each with its source roots as children) and `Dependencies` (grouped by
/// scope/configuration, per module) underneath.
pub fn rows(model: &BuildModel) -> Vec<Node> {
    let mut out = Vec::new();
    let root_id = format!("root:{}", model.tool.toolchain_id());
    out.push(node(
        root_id.clone(),
        "",
        NodeKind::ToolRoot,
        model.root.display().to_string(),
    ));

    let tasks_id = format!("{root_id}:tasks");
    out.push(node(tasks_id.clone(), &root_id, NodeKind::Group, "Tasks"));
    let mut groups: Vec<&str> = model.tasks.iter().map(|t| t.group.as_str()).collect();
    groups.sort_unstable();
    groups.dedup();
    for group in groups {
        let group_label = if group.is_empty() { "Other" } else { group };
        let group_id = format!("{tasks_id}:{group_label}");
        out.push(node(
            group_id.clone(),
            &tasks_id,
            NodeKind::Group,
            group_label,
        ));
        for task in model.tasks.iter().filter(|t| t.group == group) {
            let mut row = node(
                format!("{group_id}:{}", task.path),
                &group_id,
                NodeKind::Task,
                task.name.as_str(),
            );
            row.detail = task.description.clone();
            row.task_path = task.path.clone();
            out.push(row);
        }
    }

    let modules_id = format!("{root_id}:modules");
    out.push(node(
        modules_id.clone(),
        &root_id,
        NodeKind::Group,
        "Modules",
    ));
    for module in &model.modules {
        let module_id = format!("{modules_id}:{}", module.path);
        let mut module_row = node(
            module_id.clone(),
            &modules_id,
            NodeKind::Module,
            module.name.as_str(),
        );
        module_row.detail = module.dir.display().to_string();
        module_row.build_file = module.build_file.display().to_string();
        out.push(module_row);
        for root in &module.source_roots {
            let mut root_row = node(
                format!("{module_id}:src:{}", root.path.display()),
                &module_id,
                NodeKind::SourceRoot,
                root.path.display().to_string(),
            );
            root_row.detail = format!("{:?} / {:?}", root.kind, root.content);
            out.push(root_row);
        }
    }

    let deps_id = format!("{root_id}:dependencies");
    out.push(node(
        deps_id.clone(),
        &root_id,
        NodeKind::Group,
        "Dependencies",
    ));
    for module in &model.modules {
        if module.dependencies.is_empty() {
            continue;
        }
        let mut scopes: Vec<&str> = module
            .dependencies
            .iter()
            .map(|d| d.scope.as_str())
            .collect();
        scopes.sort_unstable();
        scopes.dedup();
        for scope in scopes {
            let scope_id = format!("{deps_id}:{}:{scope}", module.path);
            out.push(node(
                scope_id.clone(),
                &deps_id,
                NodeKind::Group,
                format!("{} ({scope})", module.name),
            ));
            for dependency in module.dependencies.iter().filter(|d| d.scope == scope) {
                let mut dep_row = node(
                    format!(
                        "{scope_id}:{}:{}:{}",
                        dependency.group, dependency.artifact, dependency.resolved
                    ),
                    &scope_id,
                    NodeKind::Dependency,
                    format!(
                        "{}:{}:{}",
                        dependency.group, dependency.artifact, dependency.resolved
                    ),
                );
                dep_row.detail = conflict_detail(dependency);
                out.push(dep_row);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Module, SourceContent, SourceRoot, SourceRootKind, Task, Tool};
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn sample_model() -> BuildModel {
        BuildModel {
            tool: Tool::Gradle,
            root: PathBuf::from("/proj"),
            modules: vec![Module {
                path: ":app".to_string(),
                name: "app".to_string(),
                dir: PathBuf::from("/proj/app"),
                build_file: PathBuf::from("/proj/app/build.gradle.kts"),
                source_roots: vec![SourceRoot {
                    path: PathBuf::from("/proj/app/src/main/java"),
                    kind: SourceRootKind::Main,
                    content: SourceContent::Java,
                }],
                output_dirs: vec![],
                jdk: None,
                dependencies: vec![Dependency {
                    group: "com.google.guava".to_string(),
                    artifact: "guava".to_string(),
                    requested: "31.0".to_string(),
                    resolved: "32.0".to_string(),
                    scope: "implementation".to_string(),
                    transitive: false,
                    file: None,
                    conflict: Some(Conflict::OmittedForConflict {
                        winner: "32.0".to_string(),
                    }),
                    children: vec![],
                }],
                children: vec![],
            }],
            tasks: vec![
                Task {
                    path: ":app:test".to_string(),
                    name: "test".to_string(),
                    group: "verification".to_string(),
                    description: "Runs the tests".to_string(),
                },
                Task {
                    path: ":app:build".to_string(),
                    name: "build".to_string(),
                    group: "build".to_string(),
                    description: String::new(),
                },
            ],
            warnings: vec![],
            synced_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn every_row_parents_to_a_row_that_already_exists() {
        let rows = rows(&sample_model());
        let ids: std::collections::HashSet<&str> = rows.iter().map(|n| n.id.as_str()).collect();
        for row in &rows {
            assert!(
                row.parent_id.is_empty() || ids.contains(row.parent_id.as_str()),
                "{} has no parent row {}",
                row.id,
                row.parent_id
            );
        }
    }

    #[test]
    fn a_task_row_carries_its_own_runnable_path() {
        let rows = rows(&sample_model());
        let build = rows
            .iter()
            .find(|n| n.kind == NodeKind::Task && n.label == "build")
            .unwrap();
        assert_eq!(build.task_path, ":app:build");
    }

    #[test]
    fn a_dependency_row_carries_its_conflict_as_the_detail() {
        let rows = rows(&sample_model());
        let dep = rows
            .iter()
            .find(|n| n.kind == NodeKind::Dependency)
            .unwrap();
        assert_eq!(dep.detail, "omitted for conflict, resolved to 32.0");
    }

    #[test]
    fn a_source_root_nests_under_its_own_module() {
        let rows = rows(&sample_model());
        let module = rows
            .iter()
            .find(|n| n.kind == NodeKind::Module)
            .unwrap()
            .id
            .clone();
        let source_root = rows
            .iter()
            .find(|n| n.kind == NodeKind::SourceRoot)
            .unwrap();
        assert_eq!(source_root.parent_id, module);
    }
}
