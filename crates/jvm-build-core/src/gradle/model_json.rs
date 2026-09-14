//! Parsing `ide-model.init.gradle`'s JSON output into [`crate::model::BuildModel`]
//! (A4).
//!
//! The `Raw*` structs mirror the init script's `buildIdeModel`/`buildModule`
//! functions field-for-field — verified against a real Gradle 8.10.2 daemon
//! run over `tests/fixtures/gradle-{single,multi-kts,catalog}`, not just
//! read off the Groovy source. `tests/fixtures/json/gradle-single-model.json`
//! is a hand-trimmed capture of that real output, kept small rather than the
//! full ~30-task, ~40-dependency-row dump a real run produces.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::model::{
    BuildModel, Dependency, Module, SourceContent, SourceRoot, SourceRootKind, Task, Tool,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModel {
    root_dir: String,
    #[allow(dead_code)] // carried through for a future "unsupported Gradle" diagnostic
    gradle_version: String,
    modules: Vec<RawModule>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModule {
    path: String,
    name: String,
    dir: String,
    #[serde(default)]
    source_roots: Vec<RawSourceRoot>,
    #[serde(default)]
    tasks: Vec<RawTask>,
    #[serde(default)]
    dependencies: Vec<RawDependency>,
}

#[derive(Debug, Deserialize)]
struct RawSourceRoot {
    path: String,
    /// `"Main"` or `"Test"` — the init script's own two-value vocabulary.
    kind: String,
}

#[derive(Debug, Deserialize)]
struct RawTask {
    path: String,
    name: String,
    group: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct RawDependency {
    configuration: String,
    #[serde(default)]
    group: String,
    name: String,
    #[serde(default)]
    version: String,
    /// Whether Gradle resolved this dependency straight off the
    /// configuration's own root (a direct/requested dependency) rather
    /// than through another dependency (transitive). Computed by the
    /// script from the resolution result's own edge
    /// (`dep.from == resolutionResult.root`), not from `selectionReason`'s
    /// text — that text reports `"requested"` for a transitively-pulled,
    /// BOM-managed artifact just as often as for a genuinely direct one,
    /// so it was never a reliable transitive signal.
    #[serde(default)]
    direct: bool,
    /// Gradle's own `selectionReason` text: `"requested"`, `"constraint"`
    /// (a BOM/platform-managed version), or a longer sentence when Gradle
    /// actually resolved a version conflict. Parsing that sentence into a
    /// [`crate::model::Conflict`] is deferred — see this module's
    /// `to_build_model` doc comment. Deserialized (so a future change can
    /// read it) but not yet consumed.
    #[allow(dead_code)]
    #[serde(default)]
    reason: String,
}

/// Parse one `ideModel` run's JSON output.
pub fn parse(text: &str) -> Result<BuildModel, serde_json::Error> {
    let raw: RawModel = serde_json::from_str(text)?;
    Ok(to_build_model(raw))
}

/// `reason` is carried through on every [`crate::model::Dependency`] as
/// free text but does not yet decide [`crate::model::Dependency::conflict`]
/// — Gradle's `selectionReason` can report an actual version conflict
/// ("selected by rule", "by conflict resolution"), which
/// [`crate::model::Conflict`] exists to carry; recognising those specific
/// sentences is deferred to whichever consumer (a future dependency-
/// analyzer view, phase D) is the first to actually need the distinction —
/// today's B-phase task tree has no view that would show it, so guessing at
/// the wording now would be an untested string match nothing exercises.
fn to_build_model(raw: RawModel) -> BuildModel {
    // Tasks travel with the module they came from only as far as this
    // function: `BuildModel::tasks` is one flat list (a Gradle task path
    // already carries its module, e.g. `:app:test`), so the per-module list
    // the JSON carries is flattened here rather than kept as a
    // `Module::tasks` field the model type doesn't have.
    let mut tasks = Vec::new();
    let modules: Vec<Module> = raw
        .modules
        .into_iter()
        .map(|m| {
            let (module, module_tasks) = to_module(m);
            tasks.extend(module_tasks);
            module
        })
        .collect();
    BuildModel {
        tool: Tool::Gradle,
        root: PathBuf::from(raw.root_dir),
        modules,
        tasks,
        warnings: Vec::new(),
        profiles: Vec::new(),
        synced_at: SystemTime::now(),
    }
}

fn to_module(raw: RawModule) -> (Module, Vec<Task>) {
    let source_roots = raw
        .source_roots
        .into_iter()
        .map(|r| SourceRoot {
            content: infer_source_content(&r.path),
            kind: match r.kind.as_str() {
                "Test" => SourceRootKind::Test,
                _ => SourceRootKind::Main,
            },
            path: PathBuf::from(r.path),
        })
        .collect();

    let dependencies = raw
        .dependencies
        .into_iter()
        .map(|d| Dependency {
            group: d.group,
            artifact: d.name,
            requested: d.version.clone(),
            resolved: d.version,
            scope: d.configuration,
            transitive: !d.direct,
            file: None,
            conflict: None,
            children: Vec::new(),
        })
        .collect();

    let tasks = raw
        .tasks
        .into_iter()
        .map(|t| Task {
            path: t.path,
            name: t.name,
            group: t.group,
            description: t.description,
        })
        .collect();

    let module = Module {
        path: raw.path,
        name: raw.name,
        dir: PathBuf::from(raw.dir.clone()),
        build_file: PathBuf::from(raw.dir),
        source_roots,
        output_dirs: Vec::new(),
        jdk: None,
        dependencies,
        children: Vec::new(),
    };
    (module, tasks)
}

/// A source root's directory name is Gradle's own convention
/// (`src/<set>/java`, `.../kotlin`, `.../groovy`, `.../resources`) — the
/// same file-shaped signal `syntax-core`'s registry would apply to a file
/// inside it, but this crate makes no file-to-language decision of its own
/// (ADR-0018), so this is a directory-name heuristic only, good enough for
/// which icon a project-tree row gets (B7) and nothing this crate itself
/// interprets further.
fn infer_source_content(path: &str) -> SourceContent {
    if path.ends_with("resources") {
        SourceContent::Resources
    } else if path.ends_with("kotlin") {
        SourceContent::Kotlin
    } else if path.ends_with("groovy") {
        SourceContent::Groovy
    } else {
        SourceContent::Java
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/json/gradle-single-model.json");

    #[test]
    fn the_fixture_model_parses() {
        let model = parse(FIXTURE).expect("valid model JSON");
        assert_eq!(model.tool, Tool::Gradle);
        assert_eq!(model.modules.len(), 1);
        let root_module = &model.modules[0];
        assert_eq!(root_module.path, ":");
        assert_eq!(root_module.name, "gradle-single");
    }

    #[test]
    fn source_roots_get_main_test_kind_and_resources_content() {
        let model = parse(FIXTURE).expect("valid");
        let roots = &model.modules[0].source_roots;
        assert_eq!(roots.len(), 4);
        let main_java = roots
            .iter()
            .find(|r| r.path.ends_with("main/java"))
            .expect("main java root");
        assert_eq!(main_java.kind, SourceRootKind::Main);
        assert_eq!(main_java.content, SourceContent::Java);
        let test_resources = roots
            .iter()
            .find(|r| r.path.ends_with("test/resources"))
            .expect("test resources root");
        assert_eq!(test_resources.kind, SourceRootKind::Test);
        assert_eq!(test_resources.content, SourceContent::Resources);
    }

    #[test]
    fn tasks_are_flattened_onto_the_build_model() {
        let model = parse(FIXTURE).expect("valid");
        assert!(model.tasks.iter().any(|t| t.path == ":test"));
        let test_task = model.tasks.iter().find(|t| t.path == ":test").unwrap();
        assert_eq!(test_task.group, "verification");
    }

    #[test]
    fn a_direct_dependency_is_not_transitive_and_a_constraint_is() {
        // Real-captured (A4/review fix): the direct/transitive split comes
        // from `direct`, the script's own resolution-result-edge computation
        // (`dep.from == resolutionResult.root`), not from `selectionReason`'s
        // free text — `junit-bom`, an explicitly imported platform, is the
        // dependency this fixture project actually declares straight on
        // `testCompileClasspath`, while `junit-jupiter` itself is reached
        // through the platform's own node in the graph once a platform is
        // in play, and a `"constraint"` reason is always transitive.
        let model = parse(FIXTURE).expect("valid");
        let deps = &model.modules[0].dependencies;
        let direct = deps.iter().find(|d| d.artifact == "junit-bom").unwrap();
        assert!(!direct.transitive);
        let constraint = deps
            .iter()
            .find(|d| d.artifact == "junit-jupiter-api")
            .unwrap();
        assert!(constraint.transitive);
    }

    #[test]
    fn a_project_dependency_has_no_group_or_version() {
        // Gradle reports an inter-module dependency as `"project :lib"`
        // with an empty group and version — captured from a real
        // gradle-multi-kts run (A4).
        let text = r#"{
            "rootDir": "/proj", "gradleVersion": "8.10.2",
            "modules": [{
                "path": ":app", "name": "app", "dir": "/proj/app",
                "sourceRoots": [], "tasks": [],
                "dependencies": [{
                    "configuration": "compileClasspath",
                    "group": "", "name": "project :lib", "version": "",
                    "direct": true, "reason": "requested"
                }]
            }]
        }"#;
        let model = parse(text).expect("valid");
        let dep = &model.modules[0].dependencies[0];
        assert_eq!(dep.artifact, "project :lib");
        assert_eq!(dep.group, "");
        assert!(!dep.transitive);
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_a_panic() {
        assert!(parse("not json").is_err());
    }
}
