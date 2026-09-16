//! Shaping a synced [`BuildModel`] into the tree the Build Tools dock shows
//! (B2): Qt-free and unit-tested here, so `cpp/build_tools_panel.cpp` only
//! paints whatever [`rows`] returns and encodes no shaping decision itself
//! (`docs/architecture/layering.md`'s "cpp/ is a humble view" rule).
//!
//! Gradle and Maven do not share one shape (review fix): Gradle's own
//! vocabulary is Tasks (grouped IntelliJ-style by the task's own `group`)
//! and Modules; Maven's is Lifecycle (the fixed phase list), Plugins (each
//! bound into the build, with the goals its own `<executions>` actually
//! run) and no separate Modules group — a reactor's per-module detail
//! already lives on each dependency/source-root row rather than needing
//! its own top-level grouping the plan never asked for.

use std::collections::HashSet;

use crate::model::{BuildModel, Conflict, Dependency, Tool};

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
    /// A Maven profile id, checkable — B3's profile checkboxes.
    Profile,
    /// A Maven plugin bound into the build (`crate::model::Plugin`).
    Plugin,
    /// A goal one plugin's `<executions>` binds.
    Goal,
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
    /// Every row's tooltip (`cpp/`'s job, review fix round 6 — there is no
    /// Detail column any more, it cost every row's `label` too much width
    /// on a narrow dock for too little of its own). Whatever a user scans
    /// the tree *by* belongs in `label` instead — a relative source-root
    /// path, a dependency's full coordinate, a plugin's artifactId; this is
    /// for the rest: the root row's full path, a conflict's reason, a
    /// plugin's groupId:version.
    pub detail: String,
    /// The task/goal path a double-click on this row runs
    /// (`jvm_build_core::run::task_config`'s own `Task::path`), empty for a
    /// row that runs nothing.
    pub task_path: String,
    /// The build file "Open build file" opens — a module's own
    /// `build_file`, empty for a row that names none.
    pub build_file: String,
    /// Meaningful only for `NodeKind::Profile`: whether this profile is in
    /// the caller's checked set (`rows`'s own `checked_profiles`
    /// argument) — the service keeps the checked set, this crate only
    /// answers "is `label` in it" for the row the service asked to shape.
    pub checked: bool,
}

fn node(id: String, parent_id: &str, kind: NodeKind, label: impl Into<String>) -> Node {
    Node {
        id,
        parent_id: parent_id.to_string(),
        kind,
        label: label.into(),
        checked: false,
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

/// The dock's whole tree for one synced model — see this module's own doc
/// comment for why Gradle and Maven diverge past the root row.
pub fn rows(model: &BuildModel, checked_profiles: &HashSet<String>) -> Vec<Node> {
    let mut out = Vec::new();
    let root_id = format!("root:{}", model.tool.toolchain_id());
    let mut root_row = node(
        root_id.clone(),
        "",
        NodeKind::ToolRoot,
        // `rootProject.name`/the root POM's `artifactId` — never the raw
        // path, which is what `detail` (the tooltip) is for instead.
        model.root_name.as_str(),
    );
    root_row.detail = model.root.display().to_string();
    out.push(root_row);

    match model.tool {
        Tool::Gradle => {
            gradle_tasks(&mut out, model, &root_id);
            modules_group(&mut out, model, &root_id);
        }
        Tool::Maven => {
            maven_lifecycle(&mut out, model, &root_id);
            maven_plugins(&mut out, model, &root_id);
        }
    }
    dependencies_group(&mut out, model, &root_id);
    profiles_group(&mut out, model, &root_id, checked_profiles);

    out
}

/// Gradle: `Tasks`, grouped by the task's own `group` (IntelliJ's own
/// "build"/"verification"/… convention, whatever the project's plugins
/// declared).
fn gradle_tasks(out: &mut Vec<Node>, model: &BuildModel, root_id: &str) {
    let tasks_id = format!("{root_id}:tasks");
    out.push(node(tasks_id.clone(), root_id, NodeKind::Group, "Tasks"));
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
}

/// Maven: `Lifecycle`, the fixed phase list `model.tasks` already carries
/// (every entry shares the one `"lifecycle"` group, so — unlike Gradle —
/// there is no second level of grouping to nest).
fn maven_lifecycle(out: &mut Vec<Node>, model: &BuildModel, root_id: &str) {
    let lifecycle_id = format!("{root_id}:lifecycle");
    out.push(node(
        lifecycle_id.clone(),
        root_id,
        NodeKind::Group,
        "Lifecycle",
    ));
    for task in &model.tasks {
        let mut row = node(
            format!("{lifecycle_id}:{}", task.path),
            &lifecycle_id,
            NodeKind::Task,
            task.name.as_str(),
        );
        row.task_path = task.path.clone();
        out.push(row);
    }
}

/// Maven: `Plugins`, unioned across every module (deduplicated by
/// group:artifact:version, since the same plugin is normally bound in
/// every module of a reactor and a per-module repeat would only ever show
/// the same goals again) — each with the goals its own `<executions>`
/// bind as children.
fn maven_plugins(out: &mut Vec<Node>, model: &BuildModel, root_id: &str) {
    let plugins_id = format!("{root_id}:plugins");
    out.push(node(
        plugins_id.clone(),
        root_id,
        NodeKind::Group,
        "Plugins",
    ));
    let mut seen = HashSet::new();
    for module in &model.modules {
        for plugin in &module.plugins {
            let key = format!("{}:{}:{}", plugin.group, plugin.artifact, plugin.version);
            if !seen.insert(key.clone()) {
                continue;
            }
            let plugin_id = format!("{plugins_id}:{key}");
            let mut plugin_row = node(
                plugin_id.clone(),
                &plugins_id,
                NodeKind::Plugin,
                plugin.artifact.as_str(),
            );
            plugin_row.detail = format!("{}:{}:{}", plugin.group, plugin.artifact, plugin.version);
            out.push(plugin_row);
            for goal in &plugin.goals {
                out.push(node(
                    format!("{plugin_id}:{goal}"),
                    &plugin_id,
                    NodeKind::Goal,
                    goal.as_str(),
                ));
            }
        }
    }
}

/// Gradle only: `Modules`, each with its own source roots as children.
fn modules_group(out: &mut Vec<Node>, model: &BuildModel, root_id: &str) {
    let modules_id = format!("{root_id}:modules");
    out.push(node(
        modules_id.clone(),
        root_id,
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
            // Relative to the module, never the absolute path (review fix,
            // round 6) — `src/main/java` scans; the module's own dir
            // (already this row's own parent's tooltip) would only repeat
            // itself on every one of its source roots.
            let relative = root
                .path
                .strip_prefix(&module.dir)
                .unwrap_or(&root.path)
                .display()
                .to_string();
            let mut root_row = node(
                format!("{module_id}:src:{}", root.path.display()),
                &module_id,
                NodeKind::SourceRoot,
                relative,
            );
            root_row.detail = format!("{:?} / {:?}", root.kind, root.content);
            out.push(root_row);
        }
    }
}

/// `Dependencies` — Gradle nests module → configuration → dependency
/// (review fix, round 6: a module can carry 6-10 resolvable configurations,
/// which made a flat "module (configuration)" row per configuration
/// unwieldy); Maven's own 3-4 scopes stay flat as a single "module (scope)"
/// row, plenty scannable at that count.
fn dependencies_group(out: &mut Vec<Node>, model: &BuildModel, root_id: &str) {
    let deps_id = format!("{root_id}:dependencies");
    out.push(node(
        deps_id.clone(),
        root_id,
        NodeKind::Group,
        "Dependencies",
    ));
    match model.tool {
        Tool::Gradle => gradle_dependencies(out, model, &deps_id),
        Tool::Maven => maven_dependencies(out, model, &deps_id),
    }
}

/// The configurations almost every task actually resolves against, first;
/// everything else (the declaration-only configurations like
/// `implementation`, and anything a plugin contributed) alphabetical after
/// them.
const PRIORITY_CONFIGURATIONS: [&str; 4] = [
    "compileClasspath",
    "runtimeClasspath",
    "testCompileClasspath",
    "testRuntimeClasspath",
];

fn gradle_dependencies(out: &mut Vec<Node>, model: &BuildModel, deps_id: &str) {
    for module in &model.modules {
        if module.dependencies.is_empty() {
            continue;
        }
        let module_id = format!("{deps_id}:{}", module.path);
        out.push(node(
            module_id.clone(),
            deps_id,
            NodeKind::Group,
            module.name.as_str(),
        ));
        let mut configurations: Vec<&str> = module
            .dependencies
            .iter()
            .map(|d| d.scope.as_str())
            .collect();
        configurations.sort_unstable();
        configurations.dedup();
        // A stable sort by (priority rank, name) keeps the priority four in
        // their own fixed order and leaves everything else alphabetical —
        // one sort rather than two.
        configurations.sort_by_key(|configuration| {
            let rank = PRIORITY_CONFIGURATIONS
                .iter()
                .position(|p| p == configuration)
                .unwrap_or(PRIORITY_CONFIGURATIONS.len());
            (rank, *configuration)
        });
        for configuration in configurations {
            let configuration_id = format!("{module_id}:{configuration}");
            out.push(node(
                configuration_id.clone(),
                &module_id,
                NodeKind::Group,
                configuration,
            ));
            push_dependency_rows(out, module, configuration, &configuration_id);
        }
    }
}

fn maven_dependencies(out: &mut Vec<Node>, model: &BuildModel, deps_id: &str) {
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
                deps_id,
                NodeKind::Group,
                format!("{} ({scope})", module.name),
            ));
            push_dependency_rows(out, module, scope, &scope_id);
        }
    }
}

/// One dependency row per entry of `module.dependencies` whose `scope`
/// matches, parented to `group_id` — the leaf shape both `gradle_dependencies`
/// and `maven_dependencies` share once the tool-specific grouping above it
/// is decided.
fn push_dependency_rows(
    out: &mut Vec<Node>,
    module: &crate::model::Module,
    scope: &str,
    group_id: &str,
) {
    for dependency in module.dependencies.iter().filter(|d| d.scope == scope) {
        // The full coordinate, not just the artifact id (review fix, round
        // 6): IntelliJ shows it in full and expects the row to scroll
        // rather than truncate a GAV, which is exactly what a one-column,
        // no-Detail tree now lets it do.
        let mut dep_row = node(
            format!(
                "{group_id}:{}:{}:{}",
                dependency.group, dependency.artifact, dependency.resolved
            ),
            group_id,
            NodeKind::Dependency,
            format!(
                "{}:{}:{}",
                dependency.group, dependency.artifact, dependency.resolved
            ),
        );
        dep_row.detail = conflict_detail(dependency);
        // D8: "Go to Declaration" needs the owning module's build file —
        // the same field `Module` rows already carry `build_file` in for
        // "Open Build File", read by a different bridge lookup for this
        // kind (`deps::declaration_site`, which needs the line too, not
        // just the file).
        dep_row.build_file = module.build_file.display().to_string();
        out.push(dep_row);
    }
}

/// `Profiles` (Maven only in practice — `model.profiles` is always empty
/// for Gradle): one checkable row per declared id.
fn profiles_group(
    out: &mut Vec<Node>,
    model: &BuildModel,
    root_id: &str,
    checked_profiles: &HashSet<String>,
) {
    if model.profiles.is_empty() {
        return;
    }
    let profiles_id = format!("{root_id}:profiles");
    out.push(node(
        profiles_id.clone(),
        root_id,
        NodeKind::Group,
        "Profiles",
    ));
    for profile in &model.profiles {
        let mut row = node(
            format!("{profiles_id}:{profile}"),
            &profiles_id,
            NodeKind::Profile,
            profile.as_str(),
        );
        row.checked = checked_profiles.contains(profile);
        out.push(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Module, Plugin, SourceContent, SourceRoot, SourceRootKind, Task, Tool};
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn gradle_model() -> BuildModel {
        BuildModel {
            tool: Tool::Gradle,
            root: PathBuf::from("/proj"),
            root_name: "my-gradle-app".to_string(),
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
                plugins: vec![],
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
            profiles: vec!["ci".to_string(), "release".to_string()],
            synced_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn maven_model() -> BuildModel {
        BuildModel {
            tool: Tool::Maven,
            root: PathBuf::from("/proj"),
            root_name: "my-maven-app".to_string(),
            modules: vec![Module {
                path: "com.example:app".to_string(),
                name: "app".to_string(),
                dir: PathBuf::from("/proj"),
                build_file: PathBuf::from("/proj/pom.xml"),
                source_roots: vec![],
                output_dirs: vec![],
                jdk: None,
                dependencies: vec![Dependency {
                    group: "org.junit.jupiter".to_string(),
                    artifact: "junit-jupiter".to_string(),
                    requested: "5.10.3".to_string(),
                    resolved: "5.10.3".to_string(),
                    scope: "test".to_string(),
                    transitive: false,
                    file: None,
                    conflict: None,
                    children: vec![],
                }],
                plugins: vec![Plugin {
                    group: "org.apache.maven.plugins".to_string(),
                    artifact: "maven-surefire-plugin".to_string(),
                    version: "3.3.1".to_string(),
                    goals: vec!["test".to_string()],
                }],
                children: vec![],
            }],
            tasks: crate::maven::goals::LIFECYCLE_PHASES
                .iter()
                .map(|phase| Task {
                    path: phase.to_string(),
                    name: phase.to_string(),
                    group: "lifecycle".to_string(),
                    description: String::new(),
                })
                .collect(),
            warnings: vec![],
            profiles: vec!["ci".to_string()],
            synced_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn empty_checked() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn every_row_parents_to_a_row_that_already_exists() {
        for model in [gradle_model(), maven_model()] {
            let rows = rows(&model, &empty_checked());
            let ids: HashSet<&str> = rows.iter().map(|n| n.id.as_str()).collect();
            for row in &rows {
                assert!(
                    row.parent_id.is_empty() || ids.contains(row.parent_id.as_str()),
                    "{} has no parent row {}",
                    row.id,
                    row.parent_id
                );
            }
        }
    }

    #[test]
    fn the_root_row_shows_the_project_name_and_the_path_only_as_detail() {
        let rows = rows(&gradle_model(), &empty_checked());
        let root = rows.iter().find(|n| n.kind == NodeKind::ToolRoot).unwrap();
        assert_eq!(root.label, "my-gradle-app");
        assert_eq!(root.detail, "/proj");
    }

    #[test]
    fn gradle_shows_tasks_and_modules_not_maven_vocabulary() {
        let model = gradle_model();
        let rows = rows(&model, &empty_checked());
        let root_id = format!("root:{}", model.tool.toolchain_id());
        let group_labels: Vec<&str> = rows
            .iter()
            .filter(|n| n.kind == NodeKind::Group && n.parent_id == root_id)
            .map(|n| n.label.as_str())
            .collect();
        assert_eq!(
            group_labels,
            vec!["Tasks", "Modules", "Dependencies", "Profiles"]
        );
    }

    #[test]
    fn maven_shows_lifecycle_plugins_dependencies_profiles_not_tasks_or_modules() {
        let model = maven_model();
        let rows = rows(&model, &empty_checked());
        let root_id = format!("root:{}", model.tool.toolchain_id());
        let group_labels: Vec<&str> = rows
            .iter()
            .filter(|n| n.kind == NodeKind::Group && n.parent_id == root_id)
            .map(|n| n.label.as_str())
            .collect();
        assert_eq!(
            group_labels,
            vec!["Lifecycle", "Plugins", "Dependencies", "Profiles"]
        );
        assert!(!rows
            .iter()
            .any(|n| n.label == "Tasks" || n.label == "Modules"));
    }

    #[test]
    fn maven_lifecycle_lists_every_phase_flat_with_no_extra_subgroup() {
        let rows = rows(&maven_model(), &empty_checked());
        let lifecycle = rows.iter().find(|n| n.label == "Lifecycle").unwrap();
        let phases: Vec<&Node> = rows
            .iter()
            .filter(|n| n.parent_id == lifecycle.id)
            .collect();
        assert_eq!(
            phases.len(),
            crate::maven::goals::LIFECYCLE_PHASES.len(),
            "every phase is a direct child of Lifecycle, no per-phase subgroup"
        );
        assert!(phases.iter().all(|n| n.kind == NodeKind::Task));
    }

    #[test]
    fn a_maven_plugin_row_carries_its_bound_goals_as_children() {
        let rows = rows(&maven_model(), &empty_checked());
        let plugin = rows
            .iter()
            .find(|n| n.kind == NodeKind::Plugin)
            .expect("one plugin row");
        assert_eq!(plugin.label, "maven-surefire-plugin");
        assert_eq!(
            plugin.detail,
            "org.apache.maven.plugins:maven-surefire-plugin:3.3.1"
        );
        let goal = rows
            .iter()
            .find(|n| n.kind == NodeKind::Goal)
            .expect("one goal row");
        assert_eq!(goal.label, "test");
        assert_eq!(goal.parent_id, plugin.id);
    }

    #[test]
    fn a_task_row_carries_its_own_runnable_path() {
        let rows = rows(&gradle_model(), &empty_checked());
        let build = rows
            .iter()
            .find(|n| n.kind == NodeKind::Task && n.label == "build")
            .unwrap();
        assert_eq!(build.task_path, ":app:build");
    }

    #[test]
    fn a_dependency_row_carries_its_conflict_as_the_detail() {
        let rows = rows(&gradle_model(), &empty_checked());
        let dep = rows
            .iter()
            .find(|n| n.kind == NodeKind::Dependency)
            .unwrap();
        assert_eq!(dep.detail, "omitted for conflict, resolved to 32.0");
    }

    #[test]
    fn a_source_root_nests_under_its_own_module() {
        let rows = rows(&gradle_model(), &empty_checked());
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

    #[test]
    fn a_source_root_label_is_relative_to_its_module_dir_kind_goes_in_the_tooltip() {
        // gradle_model()'s one source root is `/proj/app/src/main/java`
        // under a module whose `dir` is `/proj/app`.
        let rows = rows(&gradle_model(), &empty_checked());
        let root = rows
            .iter()
            .find(|n| n.kind == NodeKind::SourceRoot)
            .unwrap();
        assert_eq!(root.label, "src/main/java");
        assert!(root.detail.contains("Main"), "detail was {}", root.detail);
    }

    #[test]
    fn a_dependency_row_shows_the_full_coordinate_as_its_label() {
        let rows = rows(&gradle_model(), &empty_checked());
        let dep = rows
            .iter()
            .find(|n| n.kind == NodeKind::Dependency)
            .unwrap();
        assert_eq!(dep.label, "com.google.guava:guava:32.0");
    }

    #[test]
    fn gradle_dependencies_nest_module_then_configuration_priority_ones_first() {
        let mut model = gradle_model();
        let dep = |scope: &str| Dependency {
            group: "g".to_string(),
            artifact: format!("a-{scope}"),
            requested: "1".to_string(),
            resolved: "1".to_string(),
            scope: scope.to_string(),
            transitive: false,
            file: None,
            conflict: None,
            children: vec![],
        };
        model.modules[0].dependencies = vec![
            dep("testRuntimeClasspath"),
            dep("annotationProcessor"),
            dep("compileClasspath"),
            dep("runtimeClasspath"),
            dep("testCompileClasspath"),
        ];
        let rows = rows(&model, &empty_checked());
        let deps_group = rows.iter().find(|n| n.label == "Dependencies").unwrap();
        let module_group = rows
            .iter()
            .find(|n| n.parent_id == deps_group.id)
            .expect("one module group under Dependencies");
        assert_eq!(module_group.label, "app");
        let configurations: Vec<&str> = rows
            .iter()
            .filter(|n| n.parent_id == module_group.id)
            .map(|n| n.label.as_str())
            .collect();
        assert_eq!(
            configurations,
            vec![
                "compileClasspath",
                "runtimeClasspath",
                "testCompileClasspath",
                "testRuntimeClasspath",
                "annotationProcessor",
            ]
        );
        let compile_classpath = rows.iter().find(|n| n.label == "compileClasspath").unwrap();
        assert!(rows
            .iter()
            .any(|n| n.parent_id == compile_classpath.id && n.kind == NodeKind::Dependency));
    }

    #[test]
    fn maven_dependencies_stay_flat_as_one_module_scope_row() {
        let rows = rows(&maven_model(), &empty_checked());
        let deps_group = rows.iter().find(|n| n.label == "Dependencies").unwrap();
        let scope_group = rows
            .iter()
            .find(|n| n.parent_id == deps_group.id)
            .expect("one scope group directly under Dependencies");
        assert_eq!(scope_group.label, "app (test)");
        assert!(rows
            .iter()
            .any(|n| n.parent_id == scope_group.id && n.kind == NodeKind::Dependency));
    }

    #[test]
    fn every_declared_profile_gets_its_own_row_under_one_profiles_group() {
        let rows = rows(&gradle_model(), &empty_checked());
        let profile_rows: Vec<&Node> = rows
            .iter()
            .filter(|n| n.kind == NodeKind::Profile)
            .collect();
        assert_eq!(
            profile_rows
                .iter()
                .map(|n| n.label.as_str())
                .collect::<Vec<_>>(),
            vec!["ci", "release"]
        );
        let group_id = profile_rows[0].parent_id.clone();
        assert!(profile_rows.iter().all(|n| n.parent_id == group_id));
        assert_eq!(
            rows.iter().find(|n| n.id == group_id).unwrap().label,
            "Profiles"
        );
    }

    #[test]
    fn a_profile_in_the_checked_set_is_reported_checked_the_rest_are_not() {
        let checked: HashSet<String> = ["release".to_string()].into_iter().collect();
        let rows = rows(&gradle_model(), &checked);
        let ci = rows.iter().find(|n| n.label == "ci").unwrap();
        let release = rows.iter().find(|n| n.label == "release").unwrap();
        assert!(!ci.checked);
        assert!(release.checked);
    }

    #[test]
    fn a_model_with_no_profiles_has_no_profiles_group_at_all() {
        let mut model = gradle_model();
        model.profiles.clear();
        let rows = rows(&model, &empty_checked());
        assert!(rows.iter().all(|n| n.kind != NodeKind::Profile));
        assert!(!rows.iter().any(|n| n.label == "Profiles"));
    }
}
