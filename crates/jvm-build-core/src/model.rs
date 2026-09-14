//! The project model a sync produces (ADR-0057 §4): exactly what Gradle's
//! init script or Maven's static/effective-pom reads reported, with no
//! IDE-owned interpretation layered on top — the same delegate-and-never-
//! model rule [`build-core`](../../build-core) follows for a build.

use std::path::PathBuf;
use std::time::SystemTime;

/// Which tool produced a [`BuildModel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Gradle,
    Maven,
}

impl Tool {
    /// The `run_core::ToolchainId::as_str()` this tool joins to.
    pub fn toolchain_id(self) -> &'static str {
        match self {
            Tool::Gradle => "gradle",
            Tool::Maven => "maven",
        }
    }
}

/// A source root's kind, so the project tree (B7) can pick a `folder-src`
/// vs. `folder-test` icon without re-deriving it from a path convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRootKind {
    Main,
    Test,
}

/// What language a source root holds, purely descriptive — this crate makes
/// no file-to-language decision of its own (ADR-0018 is `syntax-core`'s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceContent {
    Java,
    Kotlin,
    Groovy,
    Resources,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRoot {
    pub path: PathBuf,
    pub kind: SourceRootKind,
    pub content: SourceContent,
}

/// A conflict Gradle's or Maven's own dependency resolution already
/// reported — never re-derived, only carried through (ADR-0057 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Gradle: `selectionReason` names a newer requested version that won.
    OmittedForConflict { winner: String },
    /// Maven: `omitted for duplicate`.
    OmittedForDuplicate,
    /// Maven: `version managed from X` (a `<dependencyManagement>` entry
    /// overrode a transitive request).
    VersionManagedFrom { from: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub group: String,
    pub artifact: String,
    pub requested: String,
    pub resolved: String,
    pub scope: String,
    pub transitive: bool,
    pub file: Option<PathBuf>,
    pub conflict: Option<Conflict>,
    pub children: Vec<Dependency>,
}

/// A Gradle task or Maven goal a project offers to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// Gradle: the task path (`:app:test`). Maven: `plugin:goal` or a
    /// lifecycle phase name.
    pub path: String,
    pub name: String,
    pub group: String,
    pub description: String,
}

/// Maven only: a plugin bound into the build, with the goals the effective
/// POM's own `<executions>` actually bind — not every goal the plugin
/// offers, which `goals.rs`'s jar-reading path answers for build-file
/// editing (phase D) — this is "what runs", read straight out of the same
/// effective-pom document `EffectivePom::dependencies` already comes from,
/// so no extra process or jar read is needed to shape the dock's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub group: String,
    pub artifact: String,
    pub version: String,
    /// One entry per goal an `<execution>` binds, deduplicated and in
    /// declaration order.
    pub goals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// Gradle: the project path (`:app`). Maven: the module's own
    /// `groupId:artifactId`.
    pub path: String,
    pub name: String,
    pub dir: PathBuf,
    pub build_file: PathBuf,
    pub source_roots: Vec<SourceRoot>,
    pub output_dirs: Vec<PathBuf>,
    /// The JDK version the module reported building against, when the tool
    /// said so (Gradle's toolchain, Maven's `maven.compiler.target`).
    pub jdk: Option<String>,
    pub dependencies: Vec<Dependency>,
    /// Maven only: always empty for Gradle, which has no plugin/goal
    /// concept the init script models this way (a Gradle plugin is
    /// modelled as its own contributed tasks instead).
    pub plugins: Vec<Plugin>,
    pub children: Vec<String>,
}

/// A whole sync's result: one per project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildModel {
    pub tool: Tool,
    pub root: PathBuf,
    /// `rootProject.name` (Gradle) or the root `pom.xml`'s own
    /// `<artifactId>` (Maven) — what the dock's root row shows; the full
    /// path (`root`, above) moves to that row's tooltip instead (review
    /// fix: a raw absolute path is not what any IDE calls a project).
    pub root_name: String,
    pub modules: Vec<Module>,
    pub tasks: Vec<Task>,
    /// Anything the tool reported that fell short of a hard failure — an
    /// unresolvable optional dependency, a deprecated DSL call. Shown, not
    /// interpreted.
    pub warnings: Vec<String>,
    /// Maven only: declared profile ids the project offers to activate
    /// (`-P<id>`), unioned across every module's own `pom.xml` at the tool
    /// root rather than kept per-module — a profile is normally declared
    /// once, at the reactor root, and inherited, so a per-module list would
    /// only ever repeat the same handful of ids. Always empty for Gradle.
    pub profiles: Vec<String>,
    pub synced_at: SystemTime,
}

impl BuildModel {
    pub fn module(&self, path: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_is_found_by_its_path() {
        let model = BuildModel {
            tool: Tool::Gradle,
            root: PathBuf::from("/proj"),
            root_name: "proj".to_string(),
            modules: vec![Module {
                path: ":app".to_string(),
                name: "app".to_string(),
                dir: PathBuf::from("/proj/app"),
                build_file: PathBuf::from("/proj/app/build.gradle.kts"),
                source_roots: vec![],
                output_dirs: vec![],
                jdk: Some("21".to_string()),
                dependencies: vec![],
                plugins: vec![],
                children: vec![],
            }],
            tasks: vec![],
            warnings: vec![],
            profiles: vec![],
            synced_at: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(model.module(":app").unwrap().name, "app");
        assert!(model.module(":missing").is_none());
    }

    #[test]
    fn tool_toolchain_ids_match_run_core() {
        assert_eq!(Tool::Gradle.toolchain_id(), "gradle");
        assert_eq!(Tool::Maven.toolchain_id(), "maven");
    }
}
